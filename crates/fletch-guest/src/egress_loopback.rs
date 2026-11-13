//! `egress-loopback <tcp port> <vsock port>`: relays guest `127.0.0.1:<tcp port>`
//! to the host's vsock `<vsock port>`, where the egress proxy listens. Processes
//! that only honour `HTTPS_PROXY` get a loopback URL (`http://127.0.0.1:8899`).

use std::net::{Ipv4Addr, TcpListener};
use std::thread;

use crate::{parse_port, pump::pump, ready, vsock, Result};

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
