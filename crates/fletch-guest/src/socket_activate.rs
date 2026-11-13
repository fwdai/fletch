//! `socket-activate <path> -- <cmd...>`: listens on the unix socket `<path>`
//! and, once something connects, becomes `<cmd>` with the listening socket as
//! fd 3, as systemd's socket activation hands it over (`LISTEN_FDS=1`,
//! `LISTEN_PID`). The connection that woke it waits in the socket's backlog
//! and is the daemon's first. Init starts `dockerd -H fd://` this way, so no
//! container runtime runs until something asks for one.

use std::io;
use std::os::fd::{AsRawFd, IntoRawFd};
use std::os::unix::net::UnixListener;
use std::os::unix::process::CommandExt;
use std::process::Command;

use crate::{ready, split_command, Result};

/// Where the protocol's sockets start (`SD_LISTEN_FDS_START`).
const LISTEN_FD: libc::c_int = 3;

pub fn run(args: &[String]) -> Result<()> {
    let (opts, cmd) = split_command(args)?;
    let [path] = opts else {
        return Err("socket-activate: expected one socket path before `--`".into());
    };
    let listener = UnixListener::bind(path)
        .map_err(|e| format!("socket-activate: cannot listen on {path}: {e}"))?;
    ready::announce(path);

    wait_for_connection(&listener)?;
    pass_on(listener)?;
    let err = Command::new(&cmd[0])
        .args(&cmd[1..])
        .env("LISTEN_FDS", "1")
        .env("LISTEN_PID", std::process::id().to_string())
        .exec();
    Err(format!("socket-activate: exec {}: {err}", cmd[0]).into())
}

/// Returns once a connection is pending, without accepting it.
fn wait_for_connection(listener: &UnixListener) -> io::Result<()> {
    let mut pending = libc::pollfd {
        fd: listener.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        // SAFETY: one valid pollfd, living for the call.
        if unsafe { libc::poll(&mut pending, 1, -1) } >= 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}

/// Moves the listener to [`LISTEN_FD`], left open across exec.
fn pass_on(listener: UnixListener) -> io::Result<()> {
    let fd = listener.into_raw_fd();
    // SAFETY: plain descriptor calls on a descriptor this process owns.
    unsafe {
        if fd != LISTEN_FD {
            check(libc::dup2(fd, LISTEN_FD))?;
            libc::close(fd);
        }
        check(libc::fcntl(LISTEN_FD, libc::F_SETFD, 0))?;
    }
    Ok(())
}

fn check(ret: libc::c_int) -> io::Result<()> {
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
