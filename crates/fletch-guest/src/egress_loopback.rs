//! `egress-loopback <tcp port> <vsock port>`: relays guest `127.0.0.1:<tcp port>`
//! to the host's vsock `<vsock port>`, where the egress proxy listens. Processes
//! that only honour `HTTPS_PROXY` get a loopback URL (`http://127.0.0.1:8899`).

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::thread;

use crate::{parse_port, ready, vsock, Result};

pub fn run(args: &[String]) -> Result<()> {
    let tcp_port = parse_port(args.first(), "tcp port")?;
    let vsock_port = parse_port(args.get(1), "vsock port")?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, u16::try_from(tcp_port)?))?;
    ready::announce(&format!("127.0.0.1:{tcp_port} -> vsock {vsock_port}"));

    for conn in listener.incoming() {
        let conn = match conn {
            Ok(conn) => conn,
            Err(err) => {
                eprintln!("fletch-guest: egress-loopback: accept: {err}");
                continue;
            }
        };
        thread::spawn(move || match vsock::connect(vsock_port) {
            Ok(upstream) => pump(conn, upstream),
            Err(err) => eprintln!("fletch-guest: egress-loopback: vsock {vsock_port}: {err}"),
        });
    }
    Ok(())
}

/// A connected stream that can be duplicated and half-closed.
trait Stream: Read + Write + Send + Sized + 'static {
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
fn pump<A: Stream, B: Stream>(a: A, b: B) {
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
