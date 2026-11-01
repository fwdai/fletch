//! `exec --read-only <dir> --writable <path>`: run the command with each
//! `<dir>` read-only, whole, and each `<path>` inside one writable again.
//! Inside a box every ssh session is the one guest user, so a file mode cannot
//! keep one process off a tree another may write; the kernel's namespaces can.
//! The command gets namespaces of its own:
//!
//! - a user namespace mapping the guest user to itself (so `id`, file
//!   ownership and git's `safe.directory` check read as before), which is what
//!   lets an unprivileged process make the rest;
//! - a mount namespace in which each `<dir>` is bind-mounted on itself with
//!   every mount below it, and that whole subtree made read-only at once
//!   (`mount_setattr(2)` with `AT_RECURSIVE`), so a mount nested in the tree
//!   is read-only too; then each `<path>` is bound on itself again and that
//!   one bind alone made writable. The data disk's own mountpoint (`/data`,
//!   where the same trees are reachable as `/data/ws/…`) is covered by an
//!   empty read-only tmpfs; the identity binds at the host paths are separate
//!   mounts and stay;
//! - a pid namespace with a private `/proc`. Without it `/proc/<pid>/root` of
//!   any other process of the guest user is a same-user path back into the
//!   guest's own mount namespace, where the trees are writable.
//!
//! Then every capability goes and `no_new_privs` is set, so the command can
//! neither remount nor unmount what was set up. A user namespace it creates
//! itself inherits these mounts locked (`MNT_LOCK_READONLY`, `MNT_LOCKED`): the
//! kernel refuses to make them writable or peel them off there too.
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

/// Checked before anything is unshared, for a message that names the path
/// rather than a mount error. Each `read_only` path must be an existing
/// directory, each `writable` one an existing directory or file strictly
/// inside one of them (outside, it is writable or not as the guest has it
/// already). Neither may reach its target through a symbolic link: a mount
/// follows one, so a link another process of the guest user planted at a
/// writable path would open whatever it points to.
pub fn check_paths(read_only: &[PathBuf], writable: &[PathBuf]) -> Result<(), String> {
    for dir in read_only {
        let meta = resolved(dir, "--read-only")?;
        if !meta.is_dir() {
            return Err(format!("--read-only {}: not a directory", dir.display()));
        }
    }
    for path in writable {
        resolved(path, "--writable")?;
        if !read_only.iter().any(|dir| inside(path, dir)) {
            return Err(format!(
                "--writable {}: not inside any --read-only directory",
                path.display()
            ));
        }
    }
    Ok(())
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

/// Confines this process to a view in which each of `read_only` is read-only
/// and each of `writable` writable again (module docs). Returns only in the
/// process that is to exec the command, which starts in `/`: the directory it
/// was in may lie in one of the trees, reached through the writable mount
/// underneath. The processes that stay behind wait for it and exit with its
/// status; they never return from here.
pub fn confine(read_only: &[PathBuf], writable: &[PathBuf]) -> Result<(), String> {
    imp::confine(read_only, writable)
}

#[cfg(target_os = "linux")]
mod imp {
    use std::ffi::{CStr, CString};
    use std::fs::OpenOptions;
    use std::io::{self, Write};
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicI32, Ordering};

    use super::CONFINE_FAILED;

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

    pub fn confine(read_only: &[PathBuf], writable: &[PathBuf]) -> Result<(), String> {
        let read_only = c_paths(read_only, "--read-only")?;
        let writable = c_paths(writable, "--writable")?;
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
    use std::path::PathBuf;

    pub fn confine(_read_only: &[PathBuf], _writable: &[PathBuf]) -> Result<(), String> {
        Err("--read-only needs Linux namespaces".into())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::check_paths;

    /// A temporary directory by its canonical path (on macOS the temp dir
    /// lies behind the `/var` link).
    fn tempdir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = std::fs::canonicalize(dir.path()).unwrap();
        (dir, path)
    }

    #[test]
    fn read_only_paths_must_be_directories() {
        let (_guard, dir) = tempdir();
        let file = dir.join("f");
        std::fs::write(&file, b"").unwrap();
        assert!(check_paths(&[dir.clone()], &[]).is_ok());
        assert!(check_paths(&[], &[]).is_ok());
        let err = check_paths(&[file], &[]).unwrap_err();
        assert!(err.contains("not a directory"), "{err}");
        let err = check_paths(&[dir.join("missing")], &[]).unwrap_err();
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
        assert!(check_paths(&read_only, &[own.clone(), state]).is_ok());

        for (writable, why) in [
            (other, "not inside any --read-only directory"),
            (home.clone(), "not inside any --read-only directory"),
            (home.join("missing"), "missing"),
        ] {
            let err = check_paths(&read_only, &[writable]).unwrap_err();
            assert!(err.contains(why), "{err}");
        }
        // No read-only directory at all: nothing to make writable again.
        let err = check_paths(&[], &[own]).unwrap_err();
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
        let err = check_paths(&[home], &[link.clone()]).unwrap_err();
        assert!(err.contains("symbolic link"), "{err}");
        let err = check_paths(&[link], &[]).unwrap_err();
        assert!(err.contains("symbolic link"), "{err}");
    }
}
