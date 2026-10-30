//! Host file access inside a tree an agent can write.
//!
//! The host runs unsandboxed, yet several directories it writes into belong to
//! an agent: a checkout (`.codegraph`, `.git/info/exclude`), the agent's
//! `.fletch-profile` and `.fletch-attachments`, its RPC mailbox. The agent can
//! plant a symlink at any name in them, and it can do so *while* the host
//! works. Path-based calls (`fs::write`, `fs::copy`, `create_dir_all`) resolve
//! every component afresh and follow links, so checking a path and then using
//! it leaves a window in which the agent swaps a component for a link and the
//! host's write lands outside the tree, with the host's privileges.
//!
//! A [`Dir`] closes the window by holding a descriptor instead of a path. It is
//! opened from a directory the agent cannot replace, one component at a time
//! with `O_NOFOLLOW`, and everything below goes through `*at` calls relative to
//! it, so nothing is resolved twice:
//!
//! - a link where a directory belongs fails the walk, or is removed — never
//!   followed — and a real directory made, as the caller chooses;
//! - a file is written to a fresh `O_EXCL` temp and renamed over its name,
//!   which replaces whatever entry is there (a link, a FIFO, a hard link to an
//!   outside inode) without ever opening it;
//! - a file is read only when it is regular, singly linked and opened without
//!   following, so a link, a FIFO or a hard link to an outside file reads as
//!   nothing.
//!
//! Racing still lets the agent make an operation *fail*; it no longer lets it
//! point one anywhere else.
//!
//! Where the walk starts is the caller's: an agent's grant covers its
//! workspace dir and its mailbox from their own entries down — it can rename
//! either away and leave a link in its place — so the walk starts at their
//! parent, which only the host writes.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nix::errno::Errno;
use nix::fcntl::{self, AtFlags, OFlag};
use nix::sys::stat::{self, Mode, SFlag};
use nix::unistd::{self, UnlinkatFlags};

/// Attempts at a step the agent can undo between two of our syscalls (making a
/// directory where it keeps putting a link). Past this it is racing on
/// purpose, and the operation fails rather than spinning.
const RACE_RETRIES: usize = 8;

const DIR_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_CLOEXEC);

/// A directory held open, so nothing done below it re-resolves its path.
#[derive(Debug)]
pub struct Dir {
    fd: OwnedFd,
    path: PathBuf,
}

impl Dir {
    /// Open `path` following links: only for a directory the agent cannot
    /// replace, every component of it being the host's.
    pub fn trusted(path: &Path) -> io::Result<Self> {
        let fd = fcntl::open(
            path,
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
            Mode::empty(),
        )?;
        Ok(Self {
            fd: owned(fd),
            path: path.to_path_buf(),
        })
    }

    /// Open `path`, whose last `untrusted` components an agent may have
    /// replaced: the directory above them is opened [trusted](Self::trusted),
    /// and they are walked without following links.
    pub fn open(path: &Path, untrusted: usize) -> io::Result<Self> {
        let mut tail = Vec::with_capacity(untrusted);
        let mut anchor = path;
        for _ in 0..untrusted {
            match (anchor.parent(), anchor.file_name()) {
                (Some(parent), Some(name)) => {
                    tail.push(name);
                    anchor = parent;
                }
                _ => {
                    return Err(invalid(format!(
                        "{} has no {untrusted} components to walk",
                        path.display()
                    )))
                }
            }
        }
        let anchor = if anchor.as_os_str().is_empty() {
            Path::new(".")
        } else {
            anchor
        };
        tail.iter()
            .rev()
            .try_fold(Self::trusted(anchor)?, |dir, name| dir.open_dir(name))
    }

    /// A repo checkout, `<root>/<workspace>/<repo>` (or a run's
    /// `<runs>/<run>/<repo>`): the agent's grant covers the directory above
    /// the checkout from its entry down, so both components are walked.
    pub fn checkout(path: &Path) -> io::Result<Self> {
        Self::open(path, 2)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The real directory at `name`; a link, a file or nothing there is an
    /// error.
    pub fn open_dir(&self, name: impl AsRef<OsStr>) -> io::Result<Self> {
        let name = checked(name.as_ref())?;
        let fd = self.open_at(name, DIR_FLAGS, Mode::empty())?;
        Ok(self.child(fd, name))
    }

    /// The real directory at `name`, made when nothing is there. Anything else
    /// at the name is an error and stays: for a directory the host must not
    /// delete on the agent's say-so (a checkout's `.git`, which a linked
    /// worktree keeps as a file).
    pub fn create_dir(&self, name: impl AsRef<OsStr>) -> io::Result<Self> {
        let name = checked(name.as_ref())?;
        for _ in 0..RACE_RETRIES {
            match self.open_at(name, DIR_FLAGS, Mode::empty()) {
                Ok(fd) => return Ok(self.child(fd, name)),
                Err(Errno::ENOENT) => self.mkdir(name)?,
                Err(e) => return Err(e.into()),
            }
        }
        Err(raced(&self.path.join(name)))
    }

    /// The real directory at `name`, whatever the agent left there: a link or
    /// a file is removed (never followed) and a directory made in its place.
    /// For directories only the host writes into, like `.fletch-profile`,
    /// `.codegraph` or a mailbox.
    pub fn claim_dir(&self, name: impl AsRef<OsStr>) -> io::Result<Self> {
        let name = checked(name.as_ref())?;
        for _ in 0..RACE_RETRIES {
            match self.open_at(name, DIR_FLAGS, Mode::empty()) {
                Ok(fd) => return Ok(self.child(fd, name)),
                Err(Errno::ENOENT) => self.mkdir(name)?,
                Err(e) if is_not_dir(e) => self.unlink(name)?,
                Err(e) => return Err(e.into()),
            }
        }
        Err(raced(&self.path.join(name)))
    }

    /// The contents of the regular file at `name`; `None` when there is none:
    /// nothing at the name, or something the host won't read through — a link,
    /// a FIFO, a directory, or a file hard-linked from elsewhere.
    pub fn read(&self, name: impl AsRef<OsStr>) -> io::Result<Option<Vec<u8>>> {
        let name = checked(name.as_ref())?;
        // `O_NONBLOCK` so a FIFO's open doesn't wait for a writer.
        let flags = OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC;
        let fd = match self.open_at(name, flags, Mode::empty()) {
            Ok(fd) => fd,
            Err(Errno::ENOENT) => return Ok(None),
            // A link (`EMLINK` is the BSDs' answer to `O_NOFOLLOW`), or a socket.
            Err(Errno::ELOOP | Errno::EMLINK | Errno::ENXIO | Errno::EOPNOTSUPP) => {
                return Ok(None)
            }
            Err(e) => return Err(e.into()),
        };
        let st = stat::fstat(fd.as_raw_fd())?;
        let regular = SFlag::from_bits_truncate(st.st_mode) & SFlag::S_IFMT == SFlag::S_IFREG;
        if !regular || st.st_nlink != 1 {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        File::from(fd).read_to_end(&mut bytes)?;
        Ok(Some(bytes))
    }

    /// Write `bytes` as the file at `name`: a fresh temp renamed over it, so
    /// whatever was at the name (a link, a FIFO, a hard link) is replaced
    /// rather than written through. A directory there is removed first.
    pub fn write(&self, name: impl AsRef<OsStr>, bytes: &[u8]) -> io::Result<()> {
        self.write_with(name.as_ref(), |file| file.write_all(bytes))
    }

    /// [`write`](Self::write), streaming the contents from `src`.
    pub fn write_from(&self, name: impl AsRef<OsStr>, mut src: impl Read) -> io::Result<()> {
        self.write_with(name.as_ref(), |file| io::copy(&mut src, file).map(drop))
    }

    /// Move the host file at `from` to `name` here, replacing what is there;
    /// across volumes, a copy [written](Self::write_from) here and the
    /// original removed.
    pub fn move_in(&self, from: &Path, name: impl AsRef<OsStr>) -> io::Result<()> {
        let name = checked(name.as_ref())?;
        for _ in 0..RACE_RETRIES {
            match fcntl::renameat(None, from, Some(self.raw()), name) {
                Ok(()) => return Ok(()),
                Err(Errno::EXDEV) => {
                    self.write_from(name, File::open(from)?)?;
                    let _ = std::fs::remove_file(from);
                    return Ok(());
                }
                Err(e) if is_dir_in_the_way(e) => self.remove(name)?,
                Err(e) => return Err(e.into()),
            }
        }
        Err(raced(&self.path.join(name)))
    }

    /// Remove whatever is at `name` without following it: a link goes, not
    /// its target, and a directory goes with everything in it. Nothing there
    /// is success.
    pub fn remove(&self, name: impl AsRef<OsStr>) -> io::Result<()> {
        let name = checked(name.as_ref())?;
        match unistd::unlinkat(Some(self.raw()), name, UnlinkatFlags::NoRemoveDir) {
            Ok(()) | Err(Errno::ENOENT) => return Ok(()),
            // A directory: `EISDIR` on Linux, `EPERM` on macOS.
            Err(Errno::EISDIR | Errno::EPERM) => {}
            Err(e) => return Err(e.into()),
        }
        let dir = match self.open_at(name, DIR_FLAGS, Mode::empty()) {
            Ok(fd) => self.child(fd, name),
            Err(Errno::ENOENT) => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        for entry in dir.entries()? {
            dir.remove(&entry)?;
        }
        drop(dir);
        match unistd::unlinkat(Some(self.raw()), name, UnlinkatFlags::RemoveDir) {
            Ok(()) | Err(Errno::ENOENT) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// The names in this directory, `.` and `..` left out.
    pub fn entries(&self) -> io::Result<Vec<OsString>> {
        let mut dir = nix::dir::Dir::openat(
            Some(self.raw()),
            ".",
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
            Mode::empty(),
        )?;
        let mut names = Vec::new();
        for entry in dir.iter() {
            let name = entry?.file_name().to_bytes().to_vec();
            if name != b"." && name != b".." {
                names.push(OsString::from(OsStr::from_bytes(&name)));
            }
        }
        Ok(names)
    }

    /// Whether anything is at `name`, a link counting as itself.
    pub fn contains(&self, name: impl AsRef<OsStr>) -> io::Result<bool> {
        let name = checked(name.as_ref())?;
        match stat::fstatat(Some(self.raw()), name, AtFlags::AT_SYMLINK_NOFOLLOW) {
            Ok(_) => Ok(true),
            Err(Errno::ENOENT) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// When the entry at `name` (a link itself, not its target) last changed.
    pub fn modified(&self, name: impl AsRef<OsStr>) -> io::Result<SystemTime> {
        let name = checked(name.as_ref())?;
        let st = stat::fstatat(Some(self.raw()), name, AtFlags::AT_SYMLINK_NOFOLLOW)?;
        let secs = u64::try_from(st.st_mtime).unwrap_or(0);
        let nanos = u32::try_from(st.st_mtime_nsec).unwrap_or(0);
        Ok(UNIX_EPOCH + Duration::new(secs, nanos))
    }

    /// Restrict this directory to its owner (`0700`).
    pub fn make_private(&self) -> io::Result<()> {
        Ok(stat::fchmod(self.raw(), Mode::S_IRWXU)?)
    }

    fn write_with(
        &self,
        name: &OsStr,
        fill: impl FnOnce(&mut File) -> io::Result<()>,
    ) -> io::Result<()> {
        let name = checked(name)?;
        let temp = format!(".fletch-{}.tmp", uuid::Uuid::new_v4().simple());
        let temp = OsStr::new(&temp);
        let flags =
            OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
        let mut file = File::from(self.open_at(temp, flags, Mode::from_bits_truncate(0o666))?);
        let filled = fill(&mut file);
        drop(file);
        let written = filled.and_then(|()| self.rename_over(temp, name));
        if written.is_err() {
            let _ = unistd::unlinkat(Some(self.raw()), temp, UnlinkatFlags::NoRemoveDir);
        }
        written
    }

    /// Rename `from` over `to`, both here, clearing a directory at `to`.
    fn rename_over(&self, from: &OsStr, to: &OsStr) -> io::Result<()> {
        for _ in 0..RACE_RETRIES {
            match fcntl::renameat(Some(self.raw()), from, Some(self.raw()), to) {
                Ok(()) => return Ok(()),
                Err(e) if is_dir_in_the_way(e) => self.remove(to)?,
                Err(e) => return Err(e.into()),
            }
        }
        Err(raced(&self.path.join(to)))
    }

    /// Unlink the non-directory at `name`. A directory found there instead —
    /// put back in a race — stays for the next open: claiming a name never
    /// deletes a directory.
    fn unlink(&self, name: &OsStr) -> io::Result<()> {
        match unistd::unlinkat(Some(self.raw()), name, UnlinkatFlags::NoRemoveDir) {
            Ok(()) | Err(Errno::ENOENT | Errno::EISDIR | Errno::EPERM) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn mkdir(&self, name: &OsStr) -> io::Result<()> {
        match stat::mkdirat(Some(self.raw()), name, Mode::from_bits_truncate(0o777)) {
            // Lost a race with whoever made it; the next open decides.
            Ok(()) | Err(Errno::EEXIST) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn open_at(&self, name: &OsStr, flags: OFlag, mode: Mode) -> nix::Result<OwnedFd> {
        fcntl::openat(Some(self.raw()), name, flags | OFlag::O_NOFOLLOW, mode).map(owned)
    }

    fn child(&self, fd: OwnedFd, name: &OsStr) -> Self {
        Self {
            fd,
            path: self.path.join(name),
        }
    }

    fn raw(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

/// Open the regular file at `path` for reading, without following a link at
/// its last component or waiting on a FIFO, and refusing a file hard-linked
/// from elsewhere: for a file the host reads in a tree an agent can write,
/// where only the file itself is the agent's to swap (`.git/index`).
pub fn open_regular(path: &Path) -> io::Result<File> {
    let flags = OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC;
    let file = File::from(owned(fcntl::open(path, flags, Mode::empty())?));
    let st = stat::fstat(file.as_raw_fd())?;
    let regular = SFlag::from_bits_truncate(st.st_mode) & SFlag::S_IFMT == SFlag::S_IFREG;
    if !regular || st.st_nlink != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not a regular file", path.display()),
        ));
    }
    Ok(file)
}

fn owned(fd: RawFd) -> OwnedFd {
    // SAFETY: `fd` was just returned by a successful open, and nothing else
    // holds it.
    unsafe { OwnedFd::from_raw_fd(fd) }
}

/// A single path component: no separator, and not `.` or `..`, either of which
/// would step outside the walk.
fn checked(name: &OsStr) -> io::Result<&OsStr> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes == b"." || bytes == b".." || bytes.contains(&b'/') {
        return Err(invalid(format!("not a file name: {name:?}")));
    }
    Ok(name)
}

/// An open with `O_DIRECTORY | O_NOFOLLOW` that met something other than a
/// directory: a link (`ELOOP`, `EMLINK` on the BSDs, `ENOTDIR` on Linux) or a
/// file (`ENOTDIR`).
fn is_not_dir(e: Errno) -> bool {
    matches!(e, Errno::ELOOP | Errno::EMLINK | Errno::ENOTDIR)
}

/// A rename onto a name a directory holds.
fn is_dir_in_the_way(e: Errno) -> bool {
    matches!(e, Errno::EISDIR | Errno::ENOTEMPTY | Errno::EEXIST)
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg)
}

fn raced(path: &Path) -> io::Error {
    io::Error::other(format!("{} kept changing under Fletch", path.display()))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;

    /// `<td>/ws/repo`, the shape of a checkout, and a directory outside it
    /// standing in for the rest of the user's disk.
    fn tree() -> (tempfile::TempDir, PathBuf, tempfile::TempDir) {
        let td = tempfile::tempdir().unwrap();
        let checkout = td.path().join("ws").join("repo");
        std::fs::create_dir_all(&checkout).unwrap();
        (td, checkout, tempfile::tempdir().unwrap())
    }

    fn is_link(path: &Path) -> bool {
        std::fs::symlink_metadata(path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
    }

    fn empty(dir: &Path) -> bool {
        std::fs::read_dir(dir).unwrap().next().is_none()
    }

    #[test]
    fn a_linked_component_above_the_target_refuses_the_walk() {
        let (td, checkout, outside) = tree();
        // The agent renames its workspace dir away and leaves a link behind.
        let ws = td.path().join("ws");
        std::fs::rename(&ws, td.path().join("moved")).unwrap();
        std::fs::create_dir(outside.path().join("repo")).unwrap();
        symlink(outside.path(), &ws).unwrap();

        assert!(Dir::checkout(&checkout).is_err());
        // The anchor above the walk may be reached through links: it is the host's.
        assert!(Dir::trusted(&checkout).is_ok());
    }

    #[test]
    fn claim_dir_replaces_a_link_without_following_it() {
        let (_td, checkout, outside) = tree();
        symlink(outside.path(), checkout.join(".codegraph")).unwrap();

        let root = Dir::checkout(&checkout).unwrap();
        let index = root.claim_dir(".codegraph").unwrap();
        index.write("codegraph.db", b"db").unwrap();

        assert!(!is_link(&checkout.join(".codegraph")));
        assert_eq!(
            std::fs::read(checkout.join(".codegraph/codegraph.db")).unwrap(),
            b"db"
        );
        assert!(empty(outside.path()));
    }

    #[test]
    fn claim_dir_replaces_a_file() {
        let (_td, checkout, _outside) = tree();
        std::fs::write(checkout.join(".codegraph"), "x").unwrap();
        Dir::checkout(&checkout)
            .unwrap()
            .claim_dir(".codegraph")
            .unwrap();
        assert!(checkout.join(".codegraph").is_dir());
    }

    #[test]
    fn create_dir_makes_a_missing_dir_and_leaves_anything_else_alone() {
        let (_td, checkout, outside) = tree();
        let root = Dir::checkout(&checkout).unwrap();
        root.create_dir("made").unwrap();
        assert!(checkout.join("made").is_dir());

        std::fs::write(checkout.join(".git"), "gitdir: elsewhere").unwrap();
        assert!(root.create_dir(".git").is_err());
        assert_eq!(
            std::fs::read_to_string(checkout.join(".git")).unwrap(),
            "gitdir: elsewhere"
        );

        symlink(outside.path(), checkout.join("linked")).unwrap();
        assert!(root.create_dir("linked").is_err());
        assert!(is_link(&checkout.join("linked")));
    }

    #[test]
    fn write_replaces_a_link_instead_of_writing_through_it() {
        let (_td, checkout, outside) = tree();
        let victim = outside.path().join("victim");
        std::fs::write(&victim, "before").unwrap();
        symlink(&victim, checkout.join("exclude")).unwrap();
        // A dangling link too: following it would create the target.
        symlink(outside.path().join("created"), checkout.join("dangling")).unwrap();

        let root = Dir::checkout(&checkout).unwrap();
        root.write("exclude", b"after").unwrap();
        root.write("dangling", b"after").unwrap();

        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "before");
        assert!(!outside.path().join("created").exists());
        assert!(!is_link(&checkout.join("exclude")));
        assert_eq!(
            std::fs::read_to_string(checkout.join("exclude")).unwrap(),
            "after"
        );
    }

    #[test]
    fn write_replaces_a_hard_link_instead_of_writing_its_inode() {
        let (_td, checkout, outside) = tree();
        let victim = outside.path().join("victim");
        std::fs::write(&victim, "before").unwrap();
        std::fs::hard_link(&victim, checkout.join("exclude")).unwrap();

        Dir::checkout(&checkout)
            .unwrap()
            .write("exclude", b"after")
            .unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "before");
    }

    #[test]
    fn write_clears_a_directory_at_the_name() {
        let (_td, checkout, outside) = tree();
        let blocker = checkout.join("mcp.json");
        std::fs::create_dir_all(blocker.join("nested")).unwrap();
        // A link inside the doomed directory goes, its target stays.
        std::fs::write(outside.path().join("keep"), "x").unwrap();
        symlink(outside.path(), blocker.join("nested/out")).unwrap();

        Dir::checkout(&checkout)
            .unwrap()
            .write("mcp.json", b"{}")
            .unwrap();
        assert!(blocker.is_file());
        assert!(outside.path().join("keep").exists());
    }

    #[test]
    fn write_leaves_no_temp_behind() {
        let (_td, checkout, _outside) = tree();
        let root = Dir::checkout(&checkout).unwrap();
        root.write("a", b"1").unwrap();
        root.write_from("b", &b"2"[..]).unwrap();
        let mut names = root.entries().unwrap();
        names.sort();
        assert_eq!(names, vec![OsString::from("a"), OsString::from("b")]);
    }

    #[test]
    fn read_returns_only_a_regular_singly_linked_file() {
        let (_td, checkout, outside) = tree();
        let secret = outside.path().join("secret");
        std::fs::write(&secret, "private").unwrap();
        std::fs::write(checkout.join("plain"), "mine").unwrap();
        symlink(&secret, checkout.join("link")).unwrap();
        std::fs::hard_link(&secret, checkout.join("hard")).unwrap();
        unistd::mkfifo(&checkout.join("fifo"), Mode::S_IRWXU).unwrap();
        std::fs::create_dir(checkout.join("dir")).unwrap();

        let root = Dir::checkout(&checkout).unwrap();
        assert_eq!(root.read("plain").unwrap().as_deref(), Some(&b"mine"[..]));
        for name in ["link", "hard", "fifo", "dir", "missing"] {
            assert_eq!(root.read(name).unwrap(), None, "{name}");
        }
    }

    #[test]
    fn open_regular_refuses_a_link_a_fifo_and_a_hard_link() {
        let (_td, checkout, outside) = tree();
        let secret = outside.path().join("secret");
        std::fs::write(&secret, "private").unwrap();
        std::fs::write(checkout.join("plain"), "mine").unwrap();
        symlink(&secret, checkout.join("link")).unwrap();
        std::fs::hard_link(&secret, checkout.join("hard")).unwrap();
        unistd::mkfifo(&checkout.join("fifo"), Mode::S_IRWXU).unwrap();

        let mut body = String::new();
        open_regular(&checkout.join("plain"))
            .unwrap()
            .read_to_string(&mut body)
            .unwrap();
        assert_eq!(body, "mine");
        for name in ["link", "hard", "fifo", "missing"] {
            assert!(open_regular(&checkout.join(name)).is_err(), "{name}");
        }
    }

    #[test]
    fn a_held_dir_keeps_writing_where_it_was_opened() {
        // The race the handle exists for: the agent swaps the directory for a
        // link *after* the host opened it. Writes follow the descriptor to the
        // agent's own renamed directory, never through the link.
        let (_td, checkout, outside) = tree();
        let root = Dir::checkout(&checkout).unwrap();
        let index = root.claim_dir(".codegraph").unwrap();

        std::fs::rename(checkout.join(".codegraph"), checkout.join("moved")).unwrap();
        symlink(outside.path(), checkout.join(".codegraph")).unwrap();

        index.write("codegraph.db", b"db").unwrap();
        assert!(empty(outside.path()));
        assert!(checkout.join("moved/codegraph.db").exists());
    }

    #[test]
    fn remove_takes_a_link_not_its_target() {
        let (_td, checkout, outside) = tree();
        std::fs::write(outside.path().join("keep"), "x").unwrap();
        symlink(outside.path(), checkout.join("link")).unwrap();
        let root = Dir::checkout(&checkout).unwrap();
        root.remove("link").unwrap();
        root.remove("missing").unwrap();
        assert!(!is_link(&checkout.join("link")));
        assert!(outside.path().join("keep").exists());
    }

    #[test]
    fn names_that_step_outside_the_dir_are_refused() {
        let (_td, checkout, _outside) = tree();
        let root = Dir::checkout(&checkout).unwrap();
        for name in ["", ".", "..", "a/b", "../x"] {
            assert!(root.write(name, b"x").is_err(), "{name:?}");
            assert!(root.claim_dir(name).is_err(), "{name:?}");
        }
    }

    #[test]
    fn move_in_lands_in_the_held_dir() {
        let (_td, checkout, outside) = tree();
        let staged = outside.path().join("staged.png");
        std::fs::write(&staged, "png").unwrap();
        Dir::checkout(&checkout)
            .unwrap()
            .move_in(&staged, "shot.png")
            .unwrap();
        assert!(!staged.exists());
        assert_eq!(
            std::fs::read_to_string(checkout.join("shot.png")).unwrap(),
            "png"
        );
    }

    /// Run `host` over and over while an agent flips `<checkout>/<name>`
    /// between a real directory and a link to `target` as fast as it can,
    /// until it has flipped enough times for a path-based write to have
    /// escaped many times over (one escapes within a few dozen), or a few
    /// seconds pass. Returns the number of flips.
    pub(crate) fn race_dir_against_link(
        checkout: &Path,
        name: &str,
        target: &Path,
        mut host: impl FnMut(),
    ) -> usize {
        let stop = Arc::new(AtomicBool::new(false));
        let flips = Arc::new(AtomicUsize::new(0));
        let agent = {
            let (dir, aside) = (checkout.join(name), checkout.join(format!("{name}.aside")));
            let (target, stop, flips) = (target.to_path_buf(), stop.clone(), flips.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let _ = std::fs::rename(&dir, &aside);
                    let _ = symlink(&target, &dir);
                    let _ = std::fs::remove_file(&dir);
                    let _ = std::fs::remove_dir_all(&aside);
                    let _ = std::fs::create_dir(&dir);
                    flips.fetch_add(1, Ordering::Relaxed);
                }
            })
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while flips.load(Ordering::Relaxed) < 1000 && std::time::Instant::now() < deadline {
            host();
        }
        stop.store(true, Ordering::Relaxed);
        agent.join().unwrap();
        flips.load(Ordering::Relaxed)
    }

    #[test]
    fn writes_never_escape_while_the_agent_swaps_the_dir_for_a_link() {
        // The host keeps claiming `.codegraph` and writing into it while the
        // agent flips it. Some host writes may fail; none may land outside.
        let (_td, checkout, outside) = tree();
        let root = Dir::checkout(&checkout).unwrap();
        let flips = race_dir_against_link(&checkout, ".codegraph", outside.path(), || {
            if let Ok(index) = root.claim_dir(".codegraph") {
                let _ = index.write("codegraph.db", b"db");
            }
        });

        assert!(flips > 0, "the agent never ran");
        assert!(
            empty(outside.path()),
            "a host write escaped through the link"
        );
    }
}
