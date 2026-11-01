//! `exec --read-only <dir> --writable <path> --cow <dir> <store> --cow-file
//! <file> <copy>`: run the command with each `--read-only` directory
//! read-only, whole, each `--writable` path inside one writable again, and
//! each `--cow` directory and `--cow-file` file inside one a private
//! copy-on-write view of its own. Inside a box every ssh session is the one
//! guest user, so a file mode cannot keep one process off a tree another may
//! write; the kernel's namespaces can. The command gets namespaces of its own:
//!
//! - a user namespace mapping the guest user to itself (so `id`, file
//!   ownership and git's `safe.directory` check read as before), which is what
//!   lets an unprivileged process make the rest;
//! - a mount namespace in which each `--read-only` directory is bind-mounted
//!   on itself with every mount below it, and that whole subtree made
//!   read-only at once (`mount_setattr(2)` with `AT_RECURSIVE`), so a mount
//!   nested in the tree is read-only too; then each `--writable` path is bound
//!   on itself again and that one bind alone made writable. Then each `--cow`
//!   directory gets an overlay mounted on itself: the directory as it is is
//!   the lower layer the command reads, and everything it writes there lands
//!   in `<store>/upper`, which must lie in a writable path, so nothing it
//!   writes reaches the directory itself, whether the file it writes existed
//!   before or not. And each `--cow-file` file has its private `<copy>` bound
//!   over it, made from the file the first time. The data disk's own
//!   mountpoint (`/data`, where the same trees are reachable as `/data/ws/…`)
//!   is covered by an empty read-only tmpfs; the identity binds at the host
//!   paths are separate mounts and stay;
//! - a pid namespace with a private `/proc`. Without it `/proc/<pid>/root` of
//!   any other process of the guest user is a same-user path back into the
//!   guest's own mount namespace, where the trees are writable.
//!
//! Then every capability goes and `no_new_privs` is set, so the command can
//! neither remount nor unmount what was set up. A user namespace it creates
//! itself inherits these mounts locked (`MNT_LOCK_READONLY`, `MNT_LOCKED`): the
//! kernel refuses to make them writable or peel them off there too.
//!
//! An overlay's lower layer is one filesystem: a mount below a `--cow`
//! directory would be hidden by it, so none of the `--writable` paths may lie
//! in one. Its upper and work directories are made inside the namespace, after
//! the read-only and writable binds, where the only paths the command's user
//! may write are its own: a symbolic link planted in a store can lead nowhere
//! else.
//!
//! What the confinement does not cover is everything outside the named trees
//! that the guest user may write anyway (`/tmp` and the other scratch tmpfs)
//! and what another process of that user may be asked to do over a socket:
//! the network namespace is the guest's.
//!
//! Every step fails closed: the command never runs unconfined because a step
//! did not work.

use std::path::{Path, PathBuf};

/// The exit status of an `exec` whose `--read-only` could not be set up: never
/// the command's own, and apart from the shell's 126 (could not run) and 127
/// (not found), so the host can tell a launch refused its read-only view from
/// one whose binary is missing. `fletch_box::guest::EXEC_CONFINE_FAILED` on the
/// host side.
pub const CONFINE_FAILED: i32 = 125;

/// What `exec` confines its command to (module docs), every path absolute, in
/// the order its options named them.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct View {
    /// `--read-only <dir>`.
    pub read_only: Vec<PathBuf>,
    /// `--writable <path>`.
    pub writable: Vec<PathBuf>,
    /// `--cow <dir> <store>`.
    pub cow: Vec<(PathBuf, PathBuf)>,
    /// `--cow-file <file> <copy>`.
    pub cow_files: Vec<(PathBuf, PathBuf)>,
}

impl View {
    /// Whether no option asked for confinement at all.
    pub fn is_empty(&self) -> bool {
        self.read_only.is_empty()
            && self.writable.is_empty()
            && self.cow.is_empty()
            && self.cow_files.is_empty()
    }
}

/// Checked before anything is unshared, for a message that names the path
/// rather than a mount error. Each `--read-only` path must be an existing
/// directory, each `--writable` one an existing directory or file strictly
/// inside one of them (outside, it is writable or not as the guest has it
/// already). Each `--cow` directory must be an existing directory strictly
/// inside a read-only one, holding no writable path (the overlay would hide
/// its mount) and overlapping no other `--cow` directory; each `--cow-file`
/// file an existing regular file strictly inside a read-only directory and in
/// no `--cow` one. A store or copy must lie where the command may write
/// (strictly inside a writable path, or outside every read-only directory),
/// clear of its directory; it need not exist yet. None of them may reach its
/// target through a symbolic link: a mount follows one, so a link another
/// process of the guest user planted at a path would open whatever it points
/// to.
pub fn check_paths(view: &View) -> Result<(), String> {
    let View {
        read_only,
        writable,
        cow,
        cow_files,
    } = view;
    let held = |path: &Path| read_only.iter().any(|dir| inside(path, dir));
    for dir in read_only {
        let meta = resolved(dir, "--read-only")?;
        if !meta.is_dir() {
            return Err(format!("--read-only {}: not a directory", dir.display()));
        }
    }
    for path in writable {
        resolved(path, "--writable")?;
        if !held(path) {
            return Err(format!(
                "--writable {}: not inside any --read-only directory",
                path.display()
            ));
        }
    }
    for (i, (dir, store)) in cow.iter().enumerate() {
        let meta = resolved(dir, "--cow")?;
        if !meta.is_dir() {
            return Err(format!("--cow {}: not a directory", dir.display()));
        }
        if !held(dir) {
            return Err(format!(
                "--cow {}: not inside any --read-only directory",
                dir.display()
            ));
        }
        if let Some(path) = writable.iter().find(|path| path.starts_with(dir)) {
            return Err(format!(
                "--cow {}: holds --writable {}, which the overlay would hide",
                dir.display(),
                path.display()
            ));
        }
        let overlaps = |other: &Path| dir.starts_with(other) || other.starts_with(dir);
        if let Some((other, _)) = cow[..i].iter().find(|(other, _)| overlaps(other)) {
            return Err(format!(
                "--cow {}: overlaps another --cow directory, {}",
                dir.display(),
                other.display()
            ));
        }
        check_private(store, "--cow", view)?;
        if store.starts_with(dir) || dir.starts_with(store) {
            return Err(format!(
                "--cow {} {}: the store and the directory overlap",
                dir.display(),
                store.display()
            ));
        }
        if store.exists() && !store.is_dir() {
            return Err(format!("--cow {}: not a directory", store.display()));
        }
    }
    for (file, copy) in cow_files {
        let meta = resolved(file, "--cow-file")?;
        if !meta.is_file() {
            return Err(format!("--cow-file {}: not a file", file.display()));
        }
        if !held(file) {
            return Err(format!(
                "--cow-file {}: not inside any --read-only directory",
                file.display()
            ));
        }
        if let Some((dir, _)) = cow.iter().find(|(dir, _)| file.starts_with(dir)) {
            return Err(format!(
                "--cow-file {}: inside --cow {}, which is private already",
                file.display(),
                dir.display()
            ));
        }
        check_private(copy, "--cow-file", view)?;
        if copy.exists() && !copy.is_file() {
            return Err(format!("--cow-file {}: not a file", copy.display()));
        }
    }
    Ok(())
}

/// A `--cow` store or `--cow-file` copy: where the command may write in
/// `view`, strictly inside a `--writable` path or outside every `--read-only`
/// directory, and as much of it as exists already its own canonical form.
fn check_private(path: &Path, option: &str, view: &View) -> Result<(), String> {
    let carved_out = view.writable.iter().any(|dir| inside(path, dir));
    if !carved_out && view.read_only.iter().any(|dir| path.starts_with(dir)) {
        return Err(format!(
            "{option} {}: in a --read-only directory and no --writable path",
            path.display()
        ));
    }
    let existing = path
        .ancestors()
        .find_map(|at| Some((at, std::fs::canonicalize(at).ok()?)));
    match existing {
        Some((at, canonical)) if canonical != at => Err(format!(
            "{option} {}: {} resolves to {} (a symbolic link on the way)",
            path.display(),
            at.display(),
            canonical.display()
        )),
        _ => Ok(()),
    }
}

/// `path`'s metadata, when it exists and is its own canonical form.
fn resolved(path: &Path, option: &str) -> Result<std::fs::Metadata, String> {
    let canonical =
        std::fs::canonicalize(path).map_err(|e| format!("{option} {}: {e}", path.display()))?;
    if canonical != path {
        return Err(format!(
            "{option} {}: resolves to {} (a symbolic link on the way)",
            path.display(),
            canonical.display()
        ));
    }
    std::fs::metadata(path).map_err(|e| format!("{option} {}: {e}", path.display()))
}

/// Whether `path` lies strictly below `dir`, by whole components.
fn inside(path: &Path, dir: &Path) -> bool {
    path != dir && path.starts_with(dir)
}

/// Confines this process to `view` (module docs), which [`check_paths`] has
/// passed. Returns only in the process that is to exec the command, which
/// starts in `/`: the directory it was in may lie in one of the trees,
/// reached through the writable mount underneath. The processes that stay
/// behind wait for it and exit with its status; they never return from here.
pub fn confine(view: &View) -> Result<(), String> {
    imp::confine(view)
}

#[cfg(target_os = "linux")]
mod imp {
    use std::ffi::{CStr, CString};
    use std::fs::{DirBuilder, OpenOptions};
    use std::io::{self, Write};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicI32, Ordering};

    use super::{View, CONFINE_FAILED};

    /// The name of a `--cow` store's upper layer: what the command wrote under
    /// the directory, path for path (`fletch_box::guest::EXEC_COW_UPPER` on
    /// the host side, which reads the command's own files from it).
    const COW_UPPER: &str = "upper";
    /// The name of a `--cow` store's work directory, which an overlay needs on
    /// its upper layer's filesystem for its atomic copy-ups.
    const COW_WORK: &str = "work";

    /// `mount_setattr(2)` (Linux 5.12), by number: the syscall table is one
    /// for every architecture from 424 on, so x86_64 and aarch64 both call it
    /// 442, and defining it here does not depend on which `libc` release
    /// spells it. A kernel without it answers `ENOSYS`, which fails closed.
    const SYS_MOUNT_SETATTR: libc::c_long = 442;
    /// `mount_setattr`'s flag to apply to the whole mount tree at the path,
    /// include/uapi/linux/fcntl.h.
    const AT_RECURSIVE: libc::c_uint = 0x8000;
    /// `MOUNT_ATTR_RDONLY`, include/uapi/linux/mount.h.
    const MOUNT_ATTR_RDONLY: u64 = 0x0000_0001;

    /// `struct mount_attr`, include/uapi/linux/mount.h: its first version
    /// (`MOUNT_ATTR_SIZE_VER0`, 32 bytes), which every kernel with the
    /// syscall takes. The kernel reads the fields, through the pointer the
    /// syscall gets; nothing here does, which is what the lint allowance says.
    #[repr(C)]
    #[allow(dead_code)]
    struct MountAttr {
        attr_set: u64,
        attr_clr: u64,
        propagation: u64,
        userns_fd: u64,
    }

    /// `si_code` of a signal the kernel raised itself (a terminal's `SIGINT`,
    /// `SIGHUP` on hangup), include/uapi/asm-generic/siginfo.h.
    const SI_KERNEL: libc::c_int = 0x80;

    /// `capset(2)`'s version 3 header: the version, then the pid (0, this
    /// process); the data that follows is two `{effective, permitted,
    /// inheritable}` triples, capabilities 0-31 then 32-63.
    const LINUX_CAPABILITY_VERSION_3: u32 = 0x2008_0522;

    /// What the processes that wait pass on to the one they wait for.
    const FORWARDED: [libc::c_int; 3] = [libc::SIGTERM, libc::SIGINT, libc::SIGHUP];

    /// The process a waiting parent passes signals to; 0 while there is none.
    static CHILD: AtomicI32 = AtomicI32::new(0);

    pub fn confine(view: &View) -> Result<(), String> {
        let read_only = c_paths(&view.read_only, "--read-only")?;
        let writable = c_paths(&view.writable, "--writable")?;
        // Before the fork, so a signal never finds a waiting parent with the
        // default action; the handlers do not survive the command's exec.
        forward_signals().map_err(|e| format!("installing signal handlers: {e}"))?;

        // SAFETY: getuid and getgid cannot fail and touch no memory of ours.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        let namespaces = libc::CLONE_NEWUSER | libc::CLONE_NEWNS | libc::CLONE_NEWPID;
        // SAFETY: unshare takes flags only; this process is single-threaded,
        // which a new user namespace requires.
        check(unsafe { libc::unshare(namespaces) }).map_err(|e| format!("unshare: {e}"))?;
        write_proc("/proc/self/setgroups", "deny")?;
        write_proc("/proc/self/uid_map", &format!("{uid} {uid} 1"))?;
        write_proc("/proc/self/gid_map", &format!("{gid} {gid} 1"))?;

        // The pid namespace holds this process's children, not this process:
        // the next one is its pid 1, which sets up the mounts (a `/proc` of
        // the new namespace can only be mounted from inside it) and then
        // stays as its init.
        fork_and_wait()?;
        mount(None, c"/", None, libc::MS_REC | libc::MS_PRIVATE, None)
            .map_err(|e| format!("making the mounts private: {e}"))?;
        mount(
            Some(c"proc"),
            c"/proc",
            Some(c"proc"),
            libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
            None,
        )
        .map_err(|e| format!("mounting a private /proc: {e}"))?;
        mount(
            Some(c"tmpfs"),
            c"/data",
            Some(c"tmpfs"),
            libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
            Some(c"mode=755"),
        )
        .map_err(|e| format!("hiding /data: {e}"))?;
        // Every read-only tree before any writable path: a path is bound from
        // the tree's read-only bind, and so starts out read-only itself.
        for (path, target) in &read_only {
            bind_read_only(target).map_err(|e| format!("--read-only {}: {e}", path.display()))?;
        }
        for (path, target) in &writable {
            bind_writable(target).map_err(|e| format!("--writable {}: {e}", path.display()))?;
        }
        // After the writable binds, which hold every store and copy: they are
        // made here, where nothing but the command's own paths is writable.
        for (dir, store) in &view.cow {
            mount_cow(dir, store)
                .map_err(|e| format!("--cow {} {}: {e}", dir.display(), store.display()))?;
        }
        for (file, copy) in &view.cow_files {
            bind_copy(file, copy)
                .map_err(|e| format!("--cow-file {} {}: {e}", file.display(), copy.display()))?;
        }

        // pid 1 of a namespace is the reaper of every orphan in it and
        // ignores the signals it has no handler for: not a job for the
        // command. It runs in the next process, and this one waits for it.
        fork_and_wait()?;
        drop_capabilities()?;
        std::env::set_current_dir("/").map_err(|e| format!("chdir /: {e}"))?;
        Ok(())
    }

    /// Each of `paths` with its C string, for `mount`.
    fn c_paths<'a>(paths: &'a [PathBuf], flag: &str) -> Result<Vec<(&'a Path, CString)>, String> {
        paths
            .iter()
            .map(|path| {
                CString::new(path.as_os_str().as_bytes())
                    .map(|c| (path.as_path(), c))
                    .map_err(|_| format!("{flag} {}: contains a NUL byte", path.display()))
            })
            .collect()
    }

    fn check(ret: libc::c_int) -> io::Result<()> {
        if ret < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// One `write(2)`, as the id map files require.
    fn write_proc(path: &str, contents: &str) -> Result<(), String> {
        OpenOptions::new()
            .write(true)
            .open(path)
            .and_then(|mut f| f.write_all(contents.as_bytes()))
            .map_err(|e| format!("writing {path}: {e}"))
    }

    fn mount(
        source: Option<&CStr>,
        target: &CStr,
        fstype: Option<&CStr>,
        flags: libc::c_ulong,
        data: Option<&CStr>,
    ) -> io::Result<()> {
        let ptr = |s: Option<&CStr>| s.map_or(std::ptr::null(), CStr::as_ptr);
        // SAFETY: every pointer is null or a NUL-terminated string that
        // outlives the call.
        check(unsafe {
            libc::mount(
                ptr(source),
                target.as_ptr(),
                ptr(fstype),
                flags,
                ptr(data).cast::<libc::c_void>(),
            )
        })
    }

    /// `target` bound on itself with every mount below it, then that whole
    /// subtree made read-only in one call. `mount_setattr` only adds the
    /// read-only bit, so unlike a `MS_REMOUNT` it need not repeat the
    /// `nosuid`/`nodev`/atime flags the user namespace locked, and it reaches
    /// the nested mounts a remount of the top one would leave writable.
    fn bind_read_only(target: &CStr) -> io::Result<()> {
        mount(
            Some(target),
            target,
            None,
            libc::MS_BIND | libc::MS_REC,
            None,
        )?;
        set_read_only(target, true, AT_RECURSIVE)
    }

    /// `target`, inside a tree [`bind_read_only`] made read-only, bound on
    /// itself again (a new bind copies the read-only bit of the mount it is
    /// taken from), and that top bind alone made writable. Clearing the bit is
    /// allowed because this namespace set it: the kernel locks only what a
    /// mount had when the namespace was made. Mounts nested below `target`
    /// come along and stay read-only.
    fn bind_writable(target: &CStr) -> io::Result<()> {
        mount(
            Some(target),
            target,
            None,
            libc::MS_BIND | libc::MS_REC,
            None,
        )?;
        set_read_only(target, false, 0)
    }

    /// An overlay mounted on `dir`, inside a read-only tree, with `dir` as it
    /// is for its one lower layer and `<store>/upper` for the upper one, so
    /// the command reads what is there and writes only its own copy. The
    /// lower layer is resolved before the overlay covers the path, so the
    /// mountpoint may be the lower directory itself. `userxattr` keeps the
    /// overlay's own metadata in `user.*` extended attributes, which is what
    /// lets an unprivileged user namespace mount it (Linux 5.11). `index=off`,
    /// whatever the kernel's default: the same store is mounted again by each
    /// `exec` the host runs in the view on the launch's behalf (its seed, its
    /// credential files), and overlayfs refuses an upper layer another mount
    /// holds only when it keeps an index.
    fn mount_cow(dir: &Path, store: &Path) -> Result<(), String> {
        let (upper, work) = (store.join(COW_UPPER), store.join(COW_WORK));
        for layer in [&upper, &work] {
            private_dir(layer)?;
        }
        let data = CString::new(overlay_options(dir, &upper, &work))
            .map_err(|_| "a path contains a NUL byte".to_string())?;
        mount(
            Some(c"overlay"),
            &c_path(dir)?,
            Some(c"overlay"),
            libc::MS_NOSUID | libc::MS_NODEV,
            Some(data.as_c_str()),
        )
        .map_err(|e| format!("mounting the overlay: {e}"))
    }

    /// `file`, inside a read-only tree, covered by a bind of `copy`, which is
    /// made from `file` the first time (owner-only) and kept after, so the
    /// command goes on with the state it left there.
    fn bind_copy(file: &Path, copy: &Path) -> Result<(), String> {
        if let Some(parent) = copy.parent() {
            private_dir(parent)?;
        }
        if std::fs::symlink_metadata(copy).is_err() {
            let bytes =
                std::fs::read(file).map_err(|e| format!("reading {}: {e}", file.display()))?;
            let made = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(copy)
                .and_then(|mut f| f.write_all(&bytes));
            match made {
                // Another launch of the same command made it first.
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                made => made.map_err(|e| format!("copying to {}: {e}", copy.display()))?,
            }
        }
        // A link would make the bind open whatever it points to.
        let is_file = std::fs::symlink_metadata(copy).is_ok_and(|meta| meta.is_file());
        if !is_file {
            return Err(format!("{}: not a regular file", copy.display()));
        }
        let (src, dst) = (c_path(copy)?, c_path(file)?);
        mount(Some(src.as_c_str()), &dst, None, libc::MS_BIND, None)
            .map_err(|e| format!("binding the copy: {e}"))
    }

    /// `dir` made (owner-only) where it is missing, and then a directory that
    /// is its own canonical form: no link another process left on the way
    /// redirects what the overlay or the bind is given.
    fn private_dir(dir: &Path) -> Result<(), String> {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| format!("creating {}: {e}", dir.display()))?;
        let meta = super::resolved(dir, "store")?;
        if !meta.is_dir() {
            return Err(format!("{}: not a directory", dir.display()));
        }
        Ok(())
    }

    /// The mount options of an overlay of `lower` with `upper` and `work`.
    /// Overlayfs splits its options at `,` and a lower layer list at `:`, and
    /// takes a backslash before either (or before itself) as the character
    /// itself, in every path it is given.
    fn overlay_options(lower: &Path, upper: &Path, work: &Path) -> Vec<u8> {
        let mut data = Vec::new();
        for (key, path) in [
            ("lowerdir=", lower),
            (",upperdir=", upper),
            (",workdir=", work),
        ] {
            data.extend_from_slice(key.as_bytes());
            for &byte in path.as_os_str().as_bytes() {
                if matches!(byte, b'\\' | b',' | b':') {
                    data.push(b'\\');
                }
                data.push(byte);
            }
        }
        data.extend_from_slice(b",userxattr,index=off");
        data
    }

    /// `path` as a C string, for `mount`.
    fn c_path(path: &Path) -> Result<CString, String> {
        CString::new(path.as_os_str().as_bytes())
            .map_err(|_| format!("{}: contains a NUL byte", path.display()))
    }

    /// Sets (`read_only`) or clears the read-only bit of the mount at
    /// `target`, and with [`AT_RECURSIVE`] in `flags` of every mount below it,
    /// atomically: either all of them change or none does.
    fn set_read_only(target: &CStr, read_only: bool, flags: libc::c_uint) -> io::Result<()> {
        let (attr_set, attr_clr) = if read_only {
            (MOUNT_ATTR_RDONLY, 0)
        } else {
            (0, MOUNT_ATTR_RDONLY)
        };
        let attr = MountAttr {
            attr_set,
            attr_clr,
            propagation: 0,
            userns_fd: 0,
        };
        // SAFETY: target is NUL-terminated and attr is a mount_attr of the
        // size passed, both living for the call.
        let ret = unsafe {
            libc::syscall(
                SYS_MOUNT_SETATTR,
                libc::AT_FDCWD,
                target.as_ptr(),
                flags,
                std::ptr::addr_of!(attr),
                std::mem::size_of::<MountAttr>(),
            )
        };
        if ret < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Forks. Returns in the child; the parent waits for it and exits with
    /// its status ([`wait_for`]).
    fn fork_and_wait() -> Result<(), String> {
        // SAFETY: this process is single-threaded, so the child may do
        // anything the parent could.
        match unsafe { libc::fork() } {
            -1 => Err(format!("fork: {}", io::Error::last_os_error())),
            0 => Ok(()),
            child => wait_for(child),
        }
    }

    /// A minimal init: passes the [`FORWARDED`] signals on to `child`, reaps
    /// every process that ends (in a pid namespace's init, the orphans
    /// reparented to it), and exits with `child`'s status once it ends —
    /// which, for pid 1, also ends every process left in the namespace.
    fn wait_for(child: libc::pid_t) -> ! {
        CHILD.store(child, Ordering::SeqCst);
        loop {
            let mut status: libc::c_int = 0;
            // SAFETY: status is a valid out-pointer.
            let pid = unsafe { libc::waitpid(-1, &mut status, 0) };
            if pid == child {
                std::process::exit(exit_code(status));
            }
            if pid < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                eprintln!("fletch-guest: exec: waiting for the confined command: {err}");
                std::process::exit(CONFINE_FAILED);
            }
        }
    }

    /// The status a shell would report for a `waitpid` status (the encoding
    /// every Linux libc's `W*` macros read): the exit code, or 128 plus the
    /// signal that ended it.
    pub(super) fn exit_code(status: libc::c_int) -> i32 {
        match status & 0x7f {
            0 => (status >> 8) & 0xff,
            // Stopped or continued, which `waitpid` without `WUNTRACED` never
            // reports.
            0x7f => CONFINE_FAILED,
            signal => 128 + signal,
        }
    }

    /// An `SA_SIGINFO` handler.
    type Handler = extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void);

    extern "C" fn forward(
        signal: libc::c_int,
        info: *mut libc::siginfo_t,
        _context: *mut libc::c_void,
    ) {
        // A signal the kernel raised for the terminal went to the whole
        // foreground process group, the command included; passing it on too
        // would deliver it twice. Anything else was sent to this process.
        // SAFETY: with SA_SIGINFO the kernel passes a valid siginfo.
        if !info.is_null() && unsafe { (*info).si_code } == SI_KERNEL {
            return;
        }
        let child = CHILD.load(Ordering::SeqCst);
        if child > 0 {
            // SAFETY: kill is async-signal-safe and takes integers only.
            unsafe { libc::kill(child, signal) };
        }
    }

    fn forward_signals() -> io::Result<()> {
        let handler: Handler = forward;
        for signal in FORWARDED {
            // SAFETY: an all-zero sigaction (empty mask) is valid; the handler
            // does only async-signal-safe work.
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = handler as libc::sighandler_t;
            action.sa_flags = libc::SA_SIGINFO | libc::SA_RESTART;
            // SAFETY: action is a valid sigaction; the old one is not wanted.
            check(unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) })?;
        }
        Ok(())
    }

    /// Empties the bounding set (no exec can bring a capability back), sets
    /// `no_new_privs`, and clears the permitted, effective and inheritable
    /// sets, which empties the ambient set with them.
    fn drop_capabilities() -> Result<(), String> {
        let (zero, one): (libc::c_ulong, libc::c_ulong) = (0, 1);
        for cap in 0..64u8 {
            // SAFETY: prctl with integer arguments only.
            let ret = unsafe {
                libc::prctl(
                    libc::PR_CAPBSET_DROP,
                    libc::c_ulong::from(cap),
                    zero,
                    zero,
                    zero,
                )
            };
            if ret != 0 {
                let err = io::Error::last_os_error();
                // Past the kernel's last capability.
                if err.raw_os_error() == Some(libc::EINVAL) {
                    break;
                }
                return Err(format!("dropping capability {cap}: {err}"));
            }
        }
        // SAFETY: prctl with integer arguments only.
        check(unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, one, zero, zero, zero) })
            .map_err(|e| format!("setting no_new_privs: {e}"))?;
        let header: [u32; 2] = [LINUX_CAPABILITY_VERSION_3, 0];
        let data = [0u32; 6];
        // SAFETY: header and data are laid out as capset(2) version 3 expects
        // and live for the call.
        let ret = unsafe { libc::syscall(libc::SYS_capset, header.as_ptr(), data.as_ptr()) };
        if ret != 0 {
            let err = io::Error::last_os_error();
            return Err(format!("clearing capabilities: {err}"));
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The kernel reads `struct mount_attr` by the size it is passed: its
        /// first version is four `u64`s.
        #[test]
        fn mount_attr_is_the_first_kernel_version() {
            assert_eq!(std::mem::size_of::<MountAttr>(), 32);
        }

        /// What overlayfs splits its options on is escaped in every path,
        /// the overlay keeps its metadata in user xattrs, and it keeps no
        /// index, so a second mount of the same store is not refused.
        #[test]
        fn overlay_options_escape_what_overlayfs_splits_on() {
            let data = overlay_options(
                Path::new("/Users/u/.cursor"),
                Path::new("/w/own/.fletch-cow/a:b,c\\d/upper"),
                Path::new("/w/own/.fletch-cow/x/work"),
            );
            assert_eq!(
                String::from_utf8(data).unwrap(),
                "lowerdir=/Users/u/.cursor,upperdir=/w/own/.fletch-cow/a\\:b\\,c\\\\d/upper,\
                 workdir=/w/own/.fletch-cow/x/work,userxattr,index=off"
            );
        }

        #[test]
        fn exit_statuses_read_as_a_shell_reports_them() {
            assert_eq!(exit_code(0), 0);
            assert_eq!(exit_code(3 << 8), 3);
            assert_eq!(exit_code(255 << 8), 255);
            assert_eq!(exit_code(libc::SIGTERM), 128 + libc::SIGTERM);
            // SIGKILL with the core-dump bit.
            assert_eq!(exit_code(libc::SIGKILL | 0x80), 128 + libc::SIGKILL);
            assert_eq!(exit_code((libc::SIGSTOP << 8) | 0x7f), CONFINE_FAILED);
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::View;

    pub fn confine(_view: &View) -> Result<(), String> {
        Err("--read-only needs Linux namespaces".into())
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{check_paths, View};

    /// A temporary directory by its canonical path (on macOS the temp dir
    /// lies behind the `/var` link).
    fn tempdir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = std::fs::canonicalize(dir.path()).unwrap();
        (dir, path)
    }

    /// [`check_paths`] of a view with only read-only and writable paths.
    fn check(read_only: &[PathBuf], writable: &[PathBuf]) -> Result<(), String> {
        check_paths(&View {
            read_only: read_only.to_vec(),
            writable: writable.to_vec(),
            ..View::default()
        })
    }

    #[test]
    fn read_only_paths_must_be_directories() {
        let (_guard, dir) = tempdir();
        let file = dir.join("f");
        std::fs::write(&file, b"").unwrap();
        assert!(check(std::slice::from_ref(&dir), &[]).is_ok());
        assert!(check_paths(&View::default()).is_ok());
        let err = check(&[file], &[]).unwrap_err();
        assert!(err.contains("not a directory"), "{err}");
        let err = check(&[dir.join("missing")], &[]).unwrap_err();
        assert!(err.contains("missing"), "{err}");
    }

    /// A writable path is a directory or a file that exists, strictly inside
    /// a read-only directory.
    #[test]
    fn writable_paths_lie_inside_a_read_only_dir() {
        let (_guard, root) = tempdir();
        let (home, other) = (root.join("home"), root.join("other"));
        let (own, state) = (home.join("own"), home.join(".state.json"));
        for dir in [&home, &other, &own] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(&state, b"{}").unwrap();
        let read_only = [home.clone()];
        assert!(check(&read_only, &[own.clone(), state]).is_ok());

        for (writable, why) in [
            (other, "not inside any --read-only directory"),
            (home.clone(), "not inside any --read-only directory"),
            (home.join("missing"), "missing"),
        ] {
            let err = check(&read_only, &[writable]).unwrap_err();
            assert!(err.contains(why), "{err}");
        }
        // No read-only directory at all: nothing to make writable again.
        let err = check(&[], &[own]).unwrap_err();
        assert!(err.contains("not inside"), "{err}");
    }

    /// A path reached through a symbolic link is refused: the bind would
    /// follow it to wherever it points.
    #[cfg(unix)]
    #[test]
    fn paths_through_a_symbolic_link_are_refused() {
        let (_guard, root) = tempdir();
        let (home, elsewhere) = (root.join("home"), root.join("elsewhere"));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        let link = home.join("cache");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        let err = check(&[home], std::slice::from_ref(&link)).unwrap_err();
        assert!(err.contains("symbolic link"), "{err}");
        let err = check(&[link], &[]).unwrap_err();
        assert!(err.contains("symbolic link"), "{err}");
    }

    /// A reviewer's tree: a home it only reads, its own root in it, a shared
    /// state dir and state file, and a directory and file outside the home.
    struct Tree {
        _guard: tempfile::TempDir,
        home: PathBuf,
        own: PathBuf,
        state: PathBuf,
        file: PathBuf,
        outside: PathBuf,
    }

    impl Tree {
        fn new() -> Tree {
            let (guard, root) = tempdir();
            let home = root.join("home");
            let (own, state) = (home.join("ws/own"), home.join(".cursor"));
            let outside = root.join("outside");
            for dir in [&own, &state.join("sub"), &own.join("x"), &outside] {
                std::fs::create_dir_all(dir).unwrap();
            }
            let file = home.join(".claude.json");
            for path in [&file, &state.join("mcp.json"), &outside.join("f")] {
                std::fs::write(path, b"{}").unwrap();
            }
            Tree {
                _guard: guard,
                home,
                own,
                state,
                file,
                outside,
            }
        }

        /// The view with these `--cow` and `--cow-file` pairs.
        fn view<P: AsRef<Path>>(&self, cow: &[(P, P)], cow_files: &[(P, P)]) -> View {
            let owned = |pairs: &[(P, P)]| -> Vec<(PathBuf, PathBuf)> {
                pairs
                    .iter()
                    .map(|(a, b)| (a.as_ref().to_path_buf(), b.as_ref().to_path_buf()))
                    .collect()
            };
            View {
                read_only: vec![self.home.clone()],
                writable: vec![self.own.clone()],
                cow: owned(cow),
                cow_files: owned(cow_files),
            }
        }
    }

    /// A copy-on-write directory or file is one that exists inside a
    /// read-only tree; its store or copy lies where the command may write,
    /// need not exist yet, and is reached through no link.
    #[test]
    fn copy_on_write_paths_pair_a_shared_one_with_a_private_one() {
        let tree = Tree::new();
        let (home, own) = (tree.home.as_path(), tree.own.as_path());
        let stores = own.join(".fletch-cow");
        let (store, copy) = (stores.join(".cursor"), stores.join(".claude.json"));
        let view = tree.view(&[(&tree.state, &store)], &[(&tree.file, &copy)]);
        assert!(check_paths(&view).is_ok(), "{:?}", check_paths(&view));
        // Outside every read-only tree, the guest's own mode lets it write.
        let elsewhere = tree.outside.join("s");
        assert!(check_paths(&tree.view(&[(&tree.state, &elsewhere)], &[])).is_ok());

        let (missing, home_dir) = (home.join("missing"), home.to_path_buf());
        let (holder, nested) = (home.join("ws"), own.join("x"));
        let in_nested = nested.join("s");
        let (astray, own_dir) = (home.join("s"), own.to_path_buf());
        for ((dir, private), why) in [
            ((&missing, &store), "missing"),
            ((&tree.file, &store), "not a directory"),
            ((&tree.outside, &store), "not inside any --read-only"),
            ((&home_dir, &store), "not inside any --read-only"),
            ((&holder, &store), "which the overlay would hide"),
            ((&tree.state, &astray), "no --writable path"),
            ((&tree.state, &own_dir), "no --writable path"),
            ((&nested, &in_nested), "the store and the directory"),
        ] {
            let err = check_paths(&tree.view(&[(dir, private)], &[])).unwrap_err();
            assert!(err.contains(why), "{}: {err}", dir.display());
        }

        // One overlay inside another.
        let (sub, sub_store) = (tree.state.join("sub"), stores.join("sub"));
        let view = tree.view(&[(&tree.state, &store), (&sub, &sub_store)], &[]);
        let err = check_paths(&view).unwrap_err();
        assert!(err.contains("overlaps another --cow directory"), "{err}");

        let mcp = tree.state.join("mcp.json");
        let outside_file = tree.outside.join("f");
        let astray_copy = home.join("copy");
        for ((file, private), why) in [
            ((&tree.state, &copy), "not a file"),
            ((&outside_file, &copy), "not inside any --read-only"),
            ((&tree.file, &astray_copy), "no --writable path"),
            // An existing copy that is a directory.
            ((&tree.file, &nested), "not a file"),
        ] {
            let err = check_paths(&tree.view(&[], &[(file, private)])).unwrap_err();
            assert!(err.contains(why), "{}: {err}", file.display());
        }
        // A file in a copy-on-write directory is private already.
        let view = tree.view(&[(&tree.state, &store)], &[(&mcp, &copy)]);
        let err = check_paths(&view).unwrap_err();
        assert!(err.contains("private already"), "{err}");
    }

    /// A store or copy reached through a link planted in the writable path
    /// is refused, wherever the link points.
    #[cfg(unix)]
    #[test]
    fn a_store_through_a_symbolic_link_is_refused() {
        let tree = Tree::new();
        let link = tree.own.join(".fletch-cow");
        std::os::unix::fs::symlink(&tree.state, &link).unwrap();
        let store = link.join(".cursor");
        let err = check_paths(&tree.view(&[(&tree.state, &store)], &[])).unwrap_err();
        assert!(err.contains("symbolic link"), "{err}");
        let copy = link.join(".claude.json");
        let err = check_paths(&tree.view(&[], &[(&tree.file, &copy)])).unwrap_err();
        assert!(err.contains("symbolic link"), "{err}");
    }
}
