//! `exec --read-only <dir>`: run the command with `<dir>` read-only. Inside a
//! box every ssh session is the one guest user, so a file mode cannot keep one
//! process off a tree another may write; the kernel's namespaces can. The
//! command gets namespaces of its own:
//!
//! - a user namespace mapping the guest user to itself (so `id`, file
//!   ownership and git's `safe.directory` check read as before), which is what
//!   lets an unprivileged process make the rest;
//! - a mount namespace in which each `<dir>` is bind-mounted on itself
//!   read-only, and the data disk's own mountpoint (`/data`, where the same
//!   trees are reachable as `/data/ws/…`) is covered by an empty read-only
//!   tmpfs; the identity binds at the host paths are separate mounts and stay;
//! - a pid namespace with a private `/proc`. Without it `/proc/<pid>/root` of
//!   any other process of the guest user is a same-user path back into the
//!   guest's own mount namespace, where the tree is writable.
//!
//! Then every capability goes and `no_new_privs` is set, so the command can
//! neither remount nor unmount what was set up. A user namespace it creates
//! itself inherits these mounts locked (`MNT_LOCK_READONLY`, `MNT_LOCKED`): the
//! kernel refuses to make them writable or peel them off there too.
//!
//! Mounts below a `<dir>` (none in a box today) keep their own mode. What the
//! confinement does not cover is everything outside the named trees that the
//! guest user may write anyway (its home, `/tmp`) and what another process of
//! that user may be asked to do over a socket: the network namespace is the
//! guest's.
//!
//! Every step fails closed: the command never runs unconfined because a step
//! did not work.

use std::path::PathBuf;

/// The exit status of an `exec` whose `--read-only` could not be set up: never
/// the command's own, and apart from the shell's 126 (could not run) and 127
/// (not found), so the host can tell a launch refused its read-only view from
/// one whose binary is missing. `fletch_box::guest::EXEC_CONFINE_FAILED` on the
/// host side.
pub const CONFINE_FAILED: i32 = 125;

/// Each path must be an existing directory: checked before anything is
/// unshared, for a message that names the path rather than a mount error.
pub fn check_paths(paths: &[PathBuf]) -> Result<(), String> {
    for path in paths {
        let meta =
            std::fs::metadata(path).map_err(|e| format!("--read-only {}: {e}", path.display()))?;
        if !meta.is_dir() {
            return Err(format!("--read-only {}: not a directory", path.display()));
        }
    }
    Ok(())
}

/// Confines this process to a read-only view of `paths` (module docs).
/// Returns only in the process that is to exec the command, which starts in
/// `/`: the directory it was in may lie in one of the trees, reached through
/// the writable mount underneath. The processes that stay behind wait for it
/// and exit with its status; they never return from here.
pub fn read_only(paths: &[PathBuf]) -> Result<(), String> {
    imp::read_only(paths)
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

    /// The kernel's `statvfs` `f_flag` bits (`ST_*`, include/linux/statfs.h),
    /// the same on every Linux architecture and libc.
    const ST_NOSUID: libc::c_ulong = 0x0002;
    const ST_NODEV: libc::c_ulong = 0x0004;
    const ST_NOEXEC: libc::c_ulong = 0x0008;
    const ST_NOATIME: libc::c_ulong = 0x0400;
    const ST_NODIRATIME: libc::c_ulong = 0x0800;
    const ST_RELATIME: libc::c_ulong = 0x1000;

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

    pub fn read_only(paths: &[PathBuf]) -> Result<(), String> {
        let targets = paths
            .iter()
            .map(PathBuf::as_path)
            .map(c_path)
            .collect::<Result<Vec<_>, _>>()?;
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
        for (path, target) in paths.iter().zip(&targets) {
            bind_read_only(target).map_err(|e| format!("--read-only {}: {e}", path.display()))?;
        }

        // pid 1 of a namespace is the reaper of every orphan in it and
        // ignores the signals it has no handler for: not a job for the
        // command. It runs in the next process, and this one waits for it.
        fork_and_wait()?;
        drop_capabilities()?;
        std::env::set_current_dir("/").map_err(|e| format!("chdir /: {e}"))?;
        Ok(())
    }

    fn c_path(path: &Path) -> Result<CString, String> {
        CString::new(path.as_os_str().as_bytes())
            .map_err(|_| format!("--read-only {}: contains a NUL byte", path.display()))
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

    /// `target` bound on itself, then that bind remounted read-only. The
    /// remount must repeat the mount's `nosuid`/`nodev`/`noexec` and atime
    /// flags: in a user namespace they are locked to what the guest's own
    /// namespace set, and a remount that would drop one fails with `EPERM`.
    fn bind_read_only(target: &CStr) -> io::Result<()> {
        mount(
            Some(target),
            target,
            None,
            libc::MS_BIND | libc::MS_REC,
            None,
        )?;
        // SAFETY: an all-zero statvfs is a valid out-buffer for statvfs(3).
        let mut vfs: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: target is NUL-terminated and vfs is a valid out-pointer.
        check(unsafe { libc::statvfs(target.as_ptr(), &mut vfs) })?;
        mount(
            None,
            target,
            None,
            libc::MS_REMOUNT | libc::MS_BIND | libc::MS_RDONLY | locked_flags(vfs.f_flag),
            None,
        )
    }

    /// The `MS_*` flags a bind remount must carry to keep what `statvfs`
    /// reports in `f_flag`. No atime flag at all is `strictatime`, which the
    /// remount has to say, as it would otherwise default to `relatime`.
    pub(super) fn locked_flags(f_flag: libc::c_ulong) -> libc::c_ulong {
        const MAP: [(libc::c_ulong, libc::c_ulong); 6] = [
            (ST_NOSUID, libc::MS_NOSUID),
            (ST_NODEV, libc::MS_NODEV),
            (ST_NOEXEC, libc::MS_NOEXEC),
            (ST_NOATIME, libc::MS_NOATIME),
            (ST_NODIRATIME, libc::MS_NODIRATIME),
            (ST_RELATIME, libc::MS_RELATIME),
        ];
        let flags: libc::c_ulong = MAP
            .iter()
            .filter(|(st, _)| f_flag & st != 0)
            .fold(0, |acc, (_, ms)| acc | ms);
        if f_flag & (ST_NOATIME | ST_RELATIME) == 0 {
            flags | libc::MS_STRICTATIME
        } else {
            flags
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

        /// `ST_RDONLY`, which the remount sets anyway.
        const ST_RDONLY: libc::c_ulong = 0x0001;

        #[test]
        fn a_remount_keeps_the_locked_flags() {
            // The data disk's binds: nosuid, nodev, relatime.
            assert_eq!(
                locked_flags(ST_NOSUID | ST_NODEV | ST_RELATIME),
                libc::MS_NOSUID | libc::MS_NODEV | libc::MS_RELATIME
            );
            assert_eq!(
                locked_flags(ST_NOEXEC | ST_NOATIME | ST_NODIRATIME),
                libc::MS_NOEXEC | libc::MS_NOATIME | libc::MS_NODIRATIME
            );
            // No atime flag at all: strictatime, said out loud.
            assert_eq!(locked_flags(ST_RDONLY), libc::MS_STRICTATIME);
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

    pub fn read_only(_paths: &[PathBuf]) -> Result<(), String> {
        Err("--read-only needs Linux namespaces".into())
    }
}

#[cfg(test)]
mod tests {
    use super::check_paths;

    #[test]
    fn read_only_paths_must_be_directories() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, b"").unwrap();
        assert!(check_paths(&[dir.path().to_path_buf()]).is_ok());
        assert!(check_paths(&[]).is_ok());
        let err = check_paths(&[file]).unwrap_err();
        assert!(err.contains("not a directory"), "{err}");
        let err = check_paths(&[dir.path().join("missing")]).unwrap_err();
        assert!(err.contains("missing"), "{err}");
    }
}
