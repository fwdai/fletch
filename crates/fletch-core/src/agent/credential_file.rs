//! How the host's logins touch the disk: how a credential file is written and
//! stamped, and how the host writes into a directory a sandboxed agent can
//! also write (its codex overlay, a checkout's `.git/info` and `.codegraph`,
//! its RPC mailbox) without following anything the agent planted there.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Every temp file the host writes a credential through starts with this, so
/// the seatbelt profile can deny reading one beside a login it hides,
/// including one a crash left behind (`seatbelt::deny_host_codex_logins`).
pub(crate) const TEMP_PREFIX: &str = ".fletch-auth-";

/// Replace `path` with a 0600 file holding `contents` in one rename, synced
/// first, so a reader never sees half a credential and a crash never leaves a
/// rotated token only in the page cache.
pub(crate) fn write_private_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("credential file has no directory"))?;
    let mut tmp = tempfile::Builder::new()
        .prefix(TEMP_PREFIX)
        .tempfile_in(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tmp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    tmp.write_all(contents)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// A file's identity for telling whether it changed since it was last seen:
/// modification time and size. `None` when the file is absent.
pub(crate) fn file_stamp(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    Some(format!("{mtime}:{}", meta.len()))
}

/// What sits at a name inside a [`PrivateDir`], looked at without following
/// a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Entry {
    Missing,
    Dir,
    Link(PathBuf),
    Other,
}

pub(crate) use imp::PrivateDir;

#[cfg(unix)]
mod imp {
    use std::ffi::{CStr, CString};
    use std::fs::File;
    use std::io::{Error, ErrorKind, Result, Write};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::path::{Component, Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use nix::libc;

    use super::{Entry, TEMP_PREFIX};

    /// A fresh temp name for a write about to be renamed into place.
    fn temp_name() -> String {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        format!(
            "{TEMP_PREFIX}{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// A directory opened by handle, never by path again: every host write
    /// into a directory a sandboxed agent can also write goes through one, so
    /// a symlink the agent swaps in for the directory, or plants inside it,
    /// is never followed. Each name it takes is a single component.
    pub(crate) struct PrivateDir {
        dir: File,
        path: PathBuf,
    }

    fn cname(name: &str) -> Result<CString> {
        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "not a single path component",
            ));
        }
        CString::new(name).map_err(|_| Error::new(ErrorKind::InvalidInput, "NUL in name"))
    }

    fn check(ret: libc::c_int) -> Result<libc::c_int> {
        if ret < 0 {
            Err(Error::last_os_error())
        } else {
            Ok(ret)
        }
    }

    /// A refusal to open because the last component is a link (or not a
    /// directory at all).
    fn is_not_a_dir(e: &Error) -> bool {
        matches!(e.raw_os_error(), Some(libc::ELOOP) | Some(libc::ENOTDIR))
    }

    impl PrivateDir {
        /// Open `path`, refusing a symlink as its last component. The
        /// components above it must be the host's own.
        pub(crate) fn open(path: &Path) -> Result<Self> {
            let dir = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(path)?;
            Ok(Self {
                dir,
                path: path.to_path_buf(),
            })
        }

        pub(crate) fn path(&self) -> &Path {
            &self.path
        }

        fn openat_dir(&self, name: &str) -> Result<Self> {
            self.openat_dir_c(&cname(name)?)
        }

        fn openat_dir_c(&self, c: &CStr) -> Result<Self> {
            // SAFETY: a valid dir fd and a NUL-terminated name; the returned
            // fd is owned by the `File` built from it.
            let fd = check(unsafe {
                libc::openat(
                    self.dir.as_raw_fd(),
                    c.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            })?;
            Ok(Self {
                dir: unsafe { File::from_raw_fd(fd) },
                path: self.path.join(std::ffi::OsStr::from_bytes(c.to_bytes())),
            })
        }

        /// The subdirectory `name`, created if missing. Anything else at the
        /// name (a link, a file) is removed and a directory made in its place.
        pub(crate) fn subdir(&self, name: &str) -> Result<Self> {
            for _ in 0..2 {
                match self.openat_dir(name) {
                    Ok(dir) => return Ok(dir),
                    Err(e) if e.kind() == ErrorKind::NotFound => {}
                    Err(e) if is_not_a_dir(&e) => self.remove(name)?,
                    Err(e) => return Err(e),
                }
                let c = cname(name)?;
                // SAFETY: as in `openat_dir`.
                let made = check(unsafe { libc::mkdirat(self.dir.as_raw_fd(), c.as_ptr(), 0o755) });
                match made {
                    Ok(_) => {}
                    Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(e),
                }
            }
            self.openat_dir(name)
        }

        /// The subdirectory `name`, created if missing. Unlike
        /// [`subdir`](Self::subdir) nothing at the name is replaced: a link or
        /// a file there is refused, for a directory that isn't the host's to
        /// recreate (a repository's `.git`, which a linked worktree keeps as a
        /// file).
        pub(crate) fn dir(&self, name: &str) -> Result<Self> {
            match self.openat_dir(name) {
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                opened => return opened,
            }
            let c = cname(name)?;
            // SAFETY: as in `openat_dir`.
            match check(unsafe { libc::mkdirat(self.dir.as_raw_fd(), c.as_ptr(), 0o755) }) {
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
            self.openat_dir(name)
        }

        /// The directory at `rel` beneath this one, every component opened
        /// without following a link: a link (or a file) anywhere along it is
        /// refused, and missing directories are created when `create`, else
        /// reported as `NotFound`.
        pub(crate) fn walk(&self, rel: &Path, create: bool) -> Result<Self> {
            let mut names = rel.components().map(|c| match c {
                Component::Normal(name) => name
                    .to_str()
                    .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "non-UTF-8 component")),
                _ => Err(Error::new(
                    ErrorKind::InvalidInput,
                    "not a plain relative path",
                )),
            });
            let step = |dir: &Self, name: &str| {
                if create {
                    dir.dir(name)
                } else {
                    dir.openat_dir(name)
                }
            };
            let first = names
                .next()
                .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "empty path"))??;
            let mut dir = step(self, first)?;
            for name in names {
                dir = step(&dir, name?)?;
            }
            Ok(dir)
        }

        /// What is at `name`, without following it.
        pub(crate) fn entry(&self, name: &str) -> Result<Entry> {
            let c = cname(name)?;
            let mut st: libc::stat = unsafe { std::mem::zeroed() };
            // SAFETY: as in `openat_dir`; `st` is a valid out pointer.
            let ret = unsafe {
                libc::fstatat(
                    self.dir.as_raw_fd(),
                    c.as_ptr(),
                    &mut st,
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            if ret < 0 {
                let e = Error::last_os_error();
                return if e.kind() == ErrorKind::NotFound {
                    Ok(Entry::Missing)
                } else {
                    Err(e)
                };
            }
            Ok(match st.st_mode & libc::S_IFMT {
                libc::S_IFDIR => Entry::Dir,
                libc::S_IFLNK => Entry::Link(self.read_link(&c)?),
                _ => Entry::Other,
            })
        }

        fn read_link(&self, c: &CString) -> Result<PathBuf> {
            let mut buf = vec![0u8; 4096];
            // SAFETY: `buf` is valid for `buf.len()` bytes.
            let n = unsafe {
                libc::readlinkat(
                    self.dir.as_raw_fd(),
                    c.as_ptr(),
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                )
            };
            if n < 0 {
                return Err(Error::last_os_error());
            }
            buf.truncate(n as usize);
            Ok(PathBuf::from(std::ffi::OsStr::from_bytes(&buf)))
        }

        /// Remove whatever is at `name`: a link itself (never its target), a
        /// file, or a directory with everything in it. Nothing there is fine.
        pub(crate) fn remove(&self, name: &str) -> Result<()> {
            self.remove_c(&cname(name)?)
        }

        /// [`remove`](Self::remove) by the raw name bytes, so an entry whose
        /// name isn't UTF-8 (one the agent made) is removed like any other.
        fn remove_c(&self, c: &CStr) -> Result<()> {
            // SAFETY: as in `openat_dir`.
            let ret = unsafe { libc::unlinkat(self.dir.as_raw_fd(), c.as_ptr(), 0) };
            if ret == 0 {
                return Ok(());
            }
            let e = Error::last_os_error();
            match e.raw_os_error() {
                Some(libc::ENOENT) => Ok(()),
                Some(libc::EISDIR) | Some(libc::EPERM) => {
                    let sub = self.openat_dir_c(c)?;
                    sub.clear()?;
                    // SAFETY: as in `openat_dir`.
                    check(unsafe {
                        libc::unlinkat(self.dir.as_raw_fd(), c.as_ptr(), libc::AT_REMOVEDIR)
                    })
                    .map(|_| ())
                }
                _ => Err(e),
            }
        }

        /// Remove every entry whose name starts with `prefix` and that was
        /// last modified more than `age` ago, without following any of them.
        pub(crate) fn remove_stale(&self, prefix: &str, age: std::time::Duration) -> Result<()> {
            let cutoff = std::time::SystemTime::now()
                .checked_sub(age)
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            for name in self.names()? {
                if !name.to_bytes().starts_with(prefix.as_bytes()) {
                    continue;
                }
                let mut st: libc::stat = unsafe { std::mem::zeroed() };
                // SAFETY: as in `openat_dir`; `st` is a valid out pointer.
                let ret = unsafe {
                    libc::fstatat(
                        self.dir.as_raw_fd(),
                        name.as_ptr(),
                        &mut st,
                        libc::AT_SYMLINK_NOFOLLOW,
                    )
                };
                if ret == 0 && (st.st_mtime as i64) < cutoff {
                    self.remove_c(&name)?;
                }
            }
            Ok(())
        }

        fn clear(&self) -> Result<()> {
            for name in self.names()? {
                self.remove_c(&name)?;
            }
            Ok(())
        }

        /// The names in this directory as raw bytes, `.` and `..` aside.
        fn names(&self) -> Result<Vec<CString>> {
            // SAFETY: `dup` of a valid fd; `fdopendir` takes ownership of the
            // duplicate and `closedir` releases it; each `dirent` is read
            // before the next `readdir` call.
            unsafe {
                let fd = check(libc::dup(self.dir.as_raw_fd()))?;
                let dir = libc::fdopendir(fd);
                if dir.is_null() {
                    let e = Error::last_os_error();
                    libc::close(fd);
                    return Err(e);
                }
                libc::rewinddir(dir);
                let mut out = Vec::new();
                loop {
                    let entry = libc::readdir(dir);
                    if entry.is_null() {
                        break;
                    }
                    let name = CStr::from_ptr((*entry).d_name.as_ptr());
                    if name.to_bytes() != b"." && name.to_bytes() != b".." {
                        out.push(name.to_owned());
                    }
                }
                libc::closedir(dir);
                Ok(out)
            }
        }

        /// A symlink `name` → `target`.
        pub(crate) fn symlink(&self, target: &Path, name: &str) -> Result<()> {
            let c = cname(name)?;
            let t = CString::new(target.as_os_str().as_bytes())
                .map_err(|_| Error::new(ErrorKind::InvalidInput, "NUL in link target"))?;
            // SAFETY: two NUL-terminated strings and a valid dir fd.
            check(unsafe { libc::symlinkat(t.as_ptr(), self.dir.as_raw_fd(), c.as_ptr()) })
                .map(|_| ())
        }

        /// Replace `name` with a file holding `contents` (mode `mode`), synced,
        /// in one rename within this directory: a reader sees the old file or
        /// the new one, and a link at `name` is replaced, never written
        /// through.
        pub(crate) fn write_file(&self, name: &str, contents: &[u8], mode: u32) -> Result<()> {
            self.write_stream(name, &mut &contents[..], mode)
        }

        /// [`write_file`](Self::write_file) from a reader, for a file too big
        /// to hold.
        pub(crate) fn write_stream(
            &self,
            name: &str,
            contents: &mut dyn std::io::Read,
            mode: u32,
        ) -> Result<()> {
            let target = cname(name)?;
            let (tmp_name, file) = loop {
                let tmp_name = temp_name();
                let c = cname(&tmp_name)?;
                // SAFETY: as in `openat_dir`.
                let fd = unsafe {
                    libc::openat(
                        self.dir.as_raw_fd(),
                        c.as_ptr(),
                        libc::O_WRONLY
                            | libc::O_CREAT
                            | libc::O_EXCL
                            | libc::O_NOFOLLOW
                            | libc::O_CLOEXEC,
                        mode as libc::c_uint,
                    )
                };
                if fd >= 0 {
                    break (tmp_name, unsafe { File::from_raw_fd(fd) });
                }
                let e = Error::last_os_error();
                if e.kind() != ErrorKind::AlreadyExists {
                    return Err(e);
                }
            };
            let tmp = cname(&tmp_name)?;
            let written = (|| -> Result<()> {
                let mut file = file;
                // `mode` exactly, not narrowed by the umask: by handle, so the
                // file chmod-ed is the one just created.
                file.set_permissions(std::fs::Permissions::from_mode(mode))?;
                std::io::copy(contents, &mut file)?;
                file.flush()?;
                file.sync_all()?;
                // SAFETY: both names are single components of this directory.
                check(unsafe {
                    libc::renameat(
                        self.dir.as_raw_fd(),
                        tmp.as_ptr(),
                        self.dir.as_raw_fd(),
                        target.as_ptr(),
                    )
                })
                .map(|_| ())
            })();
            if written.is_err() {
                // SAFETY: as above.
                unsafe { libc::unlinkat(self.dir.as_raw_fd(), tmp.as_ptr(), 0) };
            }
            written
        }

        /// Replace `name` with a copy of the host file `from`, the way
        /// [`write_file`](Self::write_file) replaces it: a link at `name` is
        /// replaced, never written through. On macOS a copy-on-write clone
        /// where the volume allows, as `std::fs::copy` makes one, so a big
        /// file shared by many checkouts costs its blocks once.
        pub(crate) fn copy_file(&self, name: &str, from: &Path, mode: u32) -> Result<()> {
            let src = File::open(from)?;
            #[cfg(target_vendor = "apple")]
            if self.clone_file(name, &src).is_ok() {
                return Ok(());
            }
            let mut reader = &src;
            self.write_stream(name, &mut reader, mode)
        }

        /// [`copy_file`](Self::copy_file)'s clone: `fclonefileat` into a temp
        /// name of this directory (it never follows or replaces anything at
        /// the name), then renamed into place.
        #[cfg(target_vendor = "apple")]
        fn clone_file(&self, name: &str, src: &File) -> Result<()> {
            let target = cname(name)?;
            let tmp = loop {
                let tmp = cname(&temp_name())?;
                // SAFETY: valid fds and a NUL-terminated name.
                let ret = unsafe {
                    libc::fclonefileat(src.as_raw_fd(), self.dir.as_raw_fd(), tmp.as_ptr(), 0)
                };
                if ret == 0 {
                    break tmp;
                }
                let e = Error::last_os_error();
                if e.kind() != ErrorKind::AlreadyExists {
                    return Err(e);
                }
            };
            // SAFETY: both names are single components of this directory.
            let renamed = check(unsafe {
                libc::renameat(
                    self.dir.as_raw_fd(),
                    tmp.as_ptr(),
                    self.dir.as_raw_fd(),
                    target.as_ptr(),
                )
            });
            if renamed.is_err() {
                // SAFETY: as above.
                unsafe { libc::unlinkat(self.dir.as_raw_fd(), tmp.as_ptr(), 0) };
            }
            renamed.map(|_| ())
        }

        /// Rename `from` to `to` within this directory: a link at `from` is
        /// moved as the link, never resolved.
        pub(crate) fn rename(&self, from: &str, to: &str) -> Result<()> {
            let (from, to) = (cname(from)?, cname(to)?);
            // SAFETY: both names are single components of this directory.
            check(unsafe {
                libc::renameat(
                    self.dir.as_raw_fd(),
                    from.as_ptr(),
                    self.dir.as_raw_fd(),
                    to.as_ptr(),
                )
            })
            .map(|_| ())
        }

        /// The bytes of the regular file `name`. Anything else there — a
        /// link, a directory, a FIFO that would block the read forever — is
        /// refused as `InvalidInput`.
        pub(crate) fn read_file(&self, name: &str) -> Result<Vec<u8>> {
            let c = cname(name)?;
            // SAFETY: as in `openat_dir`. `O_NONBLOCK` so opening a FIFO
            // returns at once; it changes nothing for a regular file.
            let fd = unsafe {
                libc::openat(
                    self.dir.as_raw_fd(),
                    c.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                let e = Error::last_os_error();
                return Err(match e.raw_os_error() {
                    Some(libc::ELOOP) => not_a_file(),
                    _ => e,
                });
            }
            let mut file = unsafe { File::from_raw_fd(fd) };
            if !file.metadata()?.is_file() {
                return Err(not_a_file());
            }
            let mut out = Vec::new();
            std::io::Read::read_to_end(&mut file, &mut out)?;
            Ok(out)
        }

        /// When `name` was last modified, without following it.
        pub(crate) fn modified(&self, name: &str) -> Result<std::time::SystemTime> {
            let c = cname(name)?;
            let mut st: libc::stat = unsafe { std::mem::zeroed() };
            // SAFETY: as in `entry`.
            check(unsafe {
                libc::fstatat(
                    self.dir.as_raw_fd(),
                    c.as_ptr(),
                    &mut st,
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            })?;
            let since_epoch = std::time::Duration::new(
                st.st_mtime.max(0) as u64,
                st.st_mtime_nsec.clamp(0, 999_999_999) as u32,
            );
            Ok(std::time::UNIX_EPOCH + since_epoch)
        }

        /// The names in this directory that are UTF-8; no caller has a use
        /// for one that isn't.
        pub(crate) fn list(&self) -> Result<Vec<String>> {
            Ok(self
                .names()?
                .into_iter()
                .filter_map(|n| n.into_string().ok())
                .collect())
        }

        /// Set this directory's own mode, by handle.
        pub(crate) fn set_mode(&self, mode: u32) -> Result<()> {
            self.dir
                .set_permissions(std::fs::Permissions::from_mode(mode))
        }
    }

    fn not_a_file() -> Error {
        Error::new(ErrorKind::InvalidInput, "not a regular file")
    }
}

#[cfg(not(unix))]
mod imp {
    use std::io::Result;
    use std::path::{Path, PathBuf};

    use super::Entry;

    /// Path-based on platforms without `openat`: Fletch's sandboxes are
    /// macOS and Linux only, so no agent shares these directories here.
    pub(crate) struct PrivateDir {
        path: PathBuf,
    }

    impl PrivateDir {
        pub(crate) fn open(path: &Path) -> Result<Self> {
            if !path.is_dir() {
                return Err(std::io::Error::other("not a directory"));
            }
            Ok(Self {
                path: path.to_path_buf(),
            })
        }
        pub(crate) fn path(&self) -> &Path {
            &self.path
        }
        pub(crate) fn subdir(&self, name: &str) -> Result<Self> {
            let path = self.path.join(name);
            std::fs::create_dir_all(&path)?;
            Ok(Self { path })
        }
        pub(crate) fn dir(&self, name: &str) -> Result<Self> {
            self.subdir(name)
        }
        pub(crate) fn walk(&self, rel: &Path, create: bool) -> Result<Self> {
            let path = self.path.join(rel);
            if create {
                std::fs::create_dir_all(&path)?;
            } else if !path.is_dir() {
                return Err(std::io::ErrorKind::NotFound.into());
            }
            Ok(Self { path })
        }
        pub(crate) fn entry(&self, name: &str) -> Result<Entry> {
            let path = self.path.join(name);
            Ok(match path.symlink_metadata() {
                Err(_) => Entry::Missing,
                Ok(m) if m.file_type().is_symlink() => Entry::Link(std::fs::read_link(&path)?),
                Ok(m) if m.is_dir() => Entry::Dir,
                Ok(_) => Entry::Other,
            })
        }
        pub(crate) fn remove_stale(&self, prefix: &str, age: std::time::Duration) -> Result<()> {
            for entry in std::fs::read_dir(&self.path)?.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let old = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|elapsed| elapsed > age);
                if name.starts_with(prefix) && old {
                    self.remove(&name)?;
                }
            }
            Ok(())
        }
        pub(crate) fn remove(&self, name: &str) -> Result<()> {
            let path = self.path.join(name);
            match self.entry(name)? {
                Entry::Missing => Ok(()),
                Entry::Dir => std::fs::remove_dir_all(path),
                _ => std::fs::remove_file(path),
            }
        }
        pub(crate) fn symlink(&self, target: &Path, name: &str) -> Result<()> {
            let dst = self.path.join(name);
            if target.is_dir() {
                std::os::windows::fs::symlink_dir(target, dst)
            } else {
                std::os::windows::fs::symlink_file(target, dst)
            }
        }
        pub(crate) fn write_file(&self, name: &str, contents: &[u8], _mode: u32) -> Result<()> {
            super::write_private_file(&self.path.join(name), contents)
        }
        pub(crate) fn write_stream(
            &self,
            name: &str,
            contents: &mut dyn std::io::Read,
            mode: u32,
        ) -> Result<()> {
            let mut buf = Vec::new();
            contents.read_to_end(&mut buf)?;
            self.write_file(name, &buf, mode)
        }
        pub(crate) fn copy_file(&self, name: &str, from: &Path, _mode: u32) -> Result<()> {
            std::fs::copy(from, self.path.join(name)).map(drop)
        }
        pub(crate) fn rename(&self, from: &str, to: &str) -> Result<()> {
            std::fs::rename(self.path.join(from), self.path.join(to))
        }
        pub(crate) fn read_file(&self, name: &str) -> Result<Vec<u8>> {
            std::fs::read(self.path.join(name))
        }
        pub(crate) fn modified(&self, name: &str) -> Result<std::time::SystemTime> {
            self.path.join(name).symlink_metadata()?.modified()
        }
        pub(crate) fn list(&self) -> Result<Vec<String>> {
            Ok(std::fs::read_dir(&self.path)?
                .flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .collect())
        }
        pub(crate) fn set_mode(&self, _mode: u32) -> Result<()> {
            Ok(())
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_private_file_is_owner_only_and_leaves_no_temp_behind() {
        use std::os::unix::fs::PermissionsExt;
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("auth.json");
        write_private_file(&path, b"{}").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let names: Vec<_> = std::fs::read_dir(td.path()).unwrap().collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn a_stamp_changes_when_the_file_is_rewritten() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("auth.json");
        assert_eq!(file_stamp(&path), None);
        std::fs::write(&path, "one").unwrap();
        let first = file_stamp(&path).unwrap();
        std::fs::write(&path, "longer").unwrap();
        assert_ne!(file_stamp(&path).unwrap(), first);
    }

    #[test]
    fn opening_a_symlinked_directory_is_refused() {
        let td = tempfile::tempdir().unwrap();
        let real = td.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = td.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(PrivateDir::open(&link).is_err());
        assert!(PrivateDir::open(&real).is_ok());
    }

    #[test]
    fn a_write_replaces_a_planted_link_instead_of_following_it() {
        let td = tempfile::tempdir().unwrap();
        let victim = td.path().join("victim");
        std::fs::write(&victim, "keep").unwrap();
        let dir_path = td.path().join("dir");
        std::fs::create_dir_all(&dir_path).unwrap();
        std::os::unix::fs::symlink(&victim, dir_path.join("auth.json")).unwrap();
        let dir = PrivateDir::open(&dir_path).unwrap();

        dir.write_file("auth.json", b"new", 0o600).unwrap();

        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
        assert_eq!(dir.entry("auth.json").unwrap(), Entry::Other);
        assert_eq!(dir.read_file("auth.json").unwrap(), b"new");
    }

    #[test]
    fn a_subdir_swapped_for_a_link_is_replaced_by_a_real_one() {
        let td = tempfile::tempdir().unwrap();
        let elsewhere = td.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let dir_path = td.path().join("dir");
        std::fs::create_dir_all(&dir_path).unwrap();
        std::os::unix::fs::symlink(&elsewhere, dir_path.join("sessions")).unwrap();
        let dir = PrivateDir::open(&dir_path).unwrap();

        let sessions = dir.subdir("sessions").unwrap();
        sessions.write_file("x", b"1", 0o644).unwrap();

        assert_eq!(dir.entry("sessions").unwrap(), Entry::Dir);
        assert!(!elsewhere.join("x").exists());
    }

    /// A walk opens every component without following a link: one anywhere
    /// along the way is refused, not replaced (it may be a repository's own
    /// `.git`), and missing directories are made only when asked.
    #[test]
    fn a_walk_refuses_a_link_anywhere_along_it() {
        let td = tempfile::tempdir().unwrap();
        let root_path = td.path().join("checkout");
        let elsewhere = td.path().join("elsewhere");
        std::fs::create_dir_all(elsewhere.join("info")).unwrap();
        std::fs::create_dir_all(&root_path).unwrap();
        std::os::unix::fs::symlink(&elsewhere, root_path.join(".git")).unwrap();
        let root = PrivateDir::open(&root_path).unwrap();

        assert!(root.walk(Path::new(".git/info"), true).is_err());
        assert!(matches!(root.entry(".git").unwrap(), Entry::Link(_)));

        std::fs::remove_file(root_path.join(".git")).unwrap();
        let missing = root.walk(Path::new(".git/info"), false).err().unwrap();
        assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
        let info = root.walk(Path::new(".git/info"), true).unwrap();
        info.write_file("exclude", b"x\n", 0o644).unwrap();
        assert_eq!(
            std::fs::read(root_path.join(".git/info/exclude")).unwrap(),
            b"x\n"
        );
        assert!(root.walk(Path::new("../elsewhere"), true).is_err());
    }

    /// A linked worktree keeps `.git` as a file; `dir` leaves it be.
    #[test]
    fn dir_refuses_a_file_where_a_directory_belongs() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join(".git"), "gitdir: /elsewhere\n").unwrap();
        let root = PrivateDir::open(td.path()).unwrap();
        assert!(root.dir(".git").is_err());
        assert_eq!(
            std::fs::read_to_string(td.path().join(".git")).unwrap(),
            "gitdir: /elsewhere\n"
        );
    }

    #[test]
    fn a_copy_replaces_a_planted_link_and_reads_refuse_one() {
        let td = tempfile::tempdir().unwrap();
        let victim = td.path().join("victim");
        std::fs::write(&victim, "keep").unwrap();
        let source = td.path().join("source");
        std::fs::write(&source, "index").unwrap();
        let dir_path = td.path().join("dir");
        std::fs::create_dir_all(&dir_path).unwrap();
        std::os::unix::fs::symlink(&victim, dir_path.join("db")).unwrap();
        let dir = PrivateDir::open(&dir_path).unwrap();

        let refused = dir.read_file("db").unwrap_err();
        assert_eq!(refused.kind(), std::io::ErrorKind::InvalidInput);
        dir.copy_file("db", &source, 0o644).unwrap();

        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
        assert_eq!(dir.entry("db").unwrap(), Entry::Other);
        assert_eq!(dir.read_file("db").unwrap(), b"index");
    }

    /// A FIFO planted where the host reads would block the read until a
    /// writer came; it is refused at once, as is a directory.
    #[test]
    fn a_read_refuses_a_fifo_or_a_directory_without_blocking() {
        use std::os::unix::ffi::OsStrExt;
        let td = tempfile::tempdir().unwrap();
        let fifo = std::ffi::CString::new(td.path().join("x.json").as_os_str().as_bytes()).unwrap();
        // SAFETY: a NUL-terminated path.
        assert_eq!(unsafe { nix::libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        std::fs::create_dir(td.path().join("d.json")).unwrap();
        let path = td.path().to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let dir = PrivateDir::open(&path).unwrap();
            let kinds = ["x.json", "d.json"].map(|n| dir.read_file(n).unwrap_err().kind());
            tx.send(kinds).unwrap();
        });
        let kinds = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("reading a FIFO blocked");
        assert_eq!(kinds, [std::io::ErrorKind::InvalidInput; 2]);
    }

    #[test]
    fn a_rename_moves_a_link_and_a_write_sets_its_mode_exactly() {
        use std::os::unix::fs::PermissionsExt;
        let td = tempfile::tempdir().unwrap();
        let dir = PrivateDir::open(td.path()).unwrap();
        dir.symlink(Path::new("/nonexistent"), "hook").unwrap();
        dir.rename("hook", "hook.orig").unwrap();
        assert_eq!(
            dir.entry("hook.orig").unwrap(),
            Entry::Link(PathBuf::from("/nonexistent"))
        );
        assert_eq!(dir.entry("hook").unwrap(), Entry::Missing);

        dir.write_file("hook", b"#!/bin/sh\n", 0o755).unwrap();
        let mode = std::fs::metadata(td.path().join("hook"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    /// An agent can name a file with bytes that aren't UTF-8; removing the
    /// directory it sits in must still empty and remove it.
    #[test]
    fn removing_a_directory_takes_entries_whose_names_are_not_utf8() {
        use std::os::unix::ffi::OsStrExt;
        let td = tempfile::tempdir().unwrap();
        let dir_path = td.path().join("dir");
        std::fs::create_dir_all(dir_path.join("junk")).unwrap();
        let odd = std::ffi::OsStr::from_bytes(b"bad-\xff-name");
        // APFS refuses such a name outright, so on macOS there is nothing to
        // remove; Linux filesystems (and a container's bind mounts) take it.
        if let Err(e) = std::fs::write(dir_path.join("junk").join(odd), "x") {
            assert_eq!(e.raw_os_error(), Some(nix::libc::EILSEQ), "{e}");
            return;
        }
        let dir = PrivateDir::open(&dir_path).unwrap();

        dir.remove("junk").unwrap();

        assert_eq!(dir.entry("junk").unwrap(), Entry::Missing);
    }

    #[test]
    fn removing_a_directory_takes_its_contents_but_not_what_its_links_point_at() {
        let td = tempfile::tempdir().unwrap();
        let outside = td.path().join("outside");
        std::fs::write(&outside, "keep").unwrap();
        let dir_path = td.path().join("dir");
        std::fs::create_dir_all(dir_path.join("junk/deeper")).unwrap();
        std::os::unix::fs::symlink(&outside, dir_path.join("junk/deeper/link")).unwrap();
        let dir = PrivateDir::open(&dir_path).unwrap();

        dir.remove("junk").unwrap();

        assert_eq!(dir.entry("junk").unwrap(), Entry::Missing);
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep");
    }
}
