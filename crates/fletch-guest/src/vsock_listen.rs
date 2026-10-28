//! `vsock-listen <port> -- <cmd...>`: inetd for AF_VSOCK. Every connection
//! becomes one `cmd` process with the socket as stdin and stdout; stderr stays
//! on the console. Init uses it for `sshd -i`, so there is no TCP listener in
//! the guest at all.

use std::os::fd::OwnedFd;
use std::process::{Command, Stdio};
use std::thread;

use crate::{parse_port, ready, split_command, vsock, Result};

pub fn run(args: &[String]) -> Result<()> {
    let (opts, cmd) = split_command(args)?;
    let port = parse_port(opts.first(), "vsock port")?;
    let listener = vsock::listen(port)?;
    ready::announce(&format!("vsock {port}"));

    loop {
        let conn = match listener.accept() {
            Ok(conn) => conn,
            Err(err) => {
                eprintln!("fletch-guest: vsock-listen {port}: accept: {err}");
                continue;
            }
        };
        let stdin = OwnedFd::from(conn.try_clone()?);
        let spawned = Command::new(&cmd[0])
            .args(&cmd[1..])
            .stdin(Stdio::from(stdin))
            .stdout(Stdio::from(OwnedFd::from(conn)))
            .spawn();
        match spawned {
            // Reap in the background; the accept loop must not wait for a session.
            Ok(mut child) => {
                thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(err) => eprintln!("fletch-guest: vsock-listen {port}: spawn {}: {err}", cmd[0]),
        }
    }
}
