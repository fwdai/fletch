//! Relays two connected streams both ways: the tunnels of the egress loopback
//! and of transparent egress, a guest or container client on one side and the
//! host's egress proxy (a vsock stream) on the other.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::os::unix::net::UnixStream;
use std::thread;

/// A connected stream that can be duplicated and half-closed.
pub trait Stream: Read + Write + Send + Sized + 'static {
    fn try_clone(&self) -> io::Result<Self>;
    fn shutdown_write(&self) -> io::Result<()>;
}

impl Stream for TcpStream {
    fn try_clone(&self) -> io::Result<Self> {
        TcpStream::try_clone(self)
    }
    fn shutdown_write(&self) -> io::Result<()> {
        self.shutdown(Shutdown::Write)
    }
}

impl Stream for UnixStream {
    fn try_clone(&self) -> io::Result<Self> {
        UnixStream::try_clone(self)
    }
    fn shutdown_write(&self) -> io::Result<()> {
        self.shutdown(Shutdown::Write)
    }
}

/// Copies both directions until each side has closed. A half-close is forwarded
/// so a CONNECT tunnel ends cleanly whichever end hangs up first.
pub fn pump<A: Stream, B: Stream>(a: A, b: B) {
    let (Ok(a2), Ok(b2)) = (a.try_clone(), b.try_clone()) else {
        return;
    };
    let forward = thread::spawn(move || copy_then_close(a2, b2));
    copy_then_close(b, a);
    let _ = forward.join();
}

fn copy_then_close<R: Read, W: Stream>(mut from: R, mut to: W) {
    let _ = io::copy(&mut from, &mut to);
    let _ = to.shutdown_write();
}
