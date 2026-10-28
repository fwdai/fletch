//! stdio to a unix socket: ssh's `ProxyCommand` onto the box's `ssh.sock`
//! (port of the spike's `sockpipe.py`). macOS `nc -U` is unusable against
//! libkrun's listen sockets, and this keeps the host down to one binary.

use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;

pub fn run(socket: &str) -> Result<(), String> {
    let stream = UnixStream::connect(socket).map_err(|e| format!("connect {socket}: {e}"))?;
    let mut to_socket = stream
        .try_clone()
        .map_err(|e| format!("clone socket: {e}"))?;
    std::thread::spawn(move || {
        let _ = io::copy(&mut io::stdin().lock(), &mut to_socket);
        // EOF on stdin ends the client's half; the reply side keeps draining.
        let _ = to_socket.shutdown(Shutdown::Write);
    });
    let mut from_socket = stream;
    // Rust's stdout is line-buffered; SSH frames have no newlines, so every
    // chunk is flushed as it arrives.
    let mut stdout = io::stdout().lock();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = match from_socket.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("read socket: {e}")),
        };
        stdout
            .write_all(&buf[..n])
            .and_then(|()| stdout.flush())
            .map_err(|e| format!("write stdout: {e}"))?;
    }
}
