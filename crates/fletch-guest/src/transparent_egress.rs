//! `transparent-egress <http address> <tls address> <vsock port>`: takes the
//! connections containers open to the sentinel address every outside name
//! resolves to (`sentinel-dns`), which init's nftables rule sends here: port
//! 80 to `<http address>`, 443 to `<tls address>`. Each names its host in its
//! first bytes (the `Host` header, the TLS server name) and becomes a
//! `CONNECT <host>:<port>` tunnel through the host's egress proxy on vsock
//! `<vsock port>`, those bytes first. One that names no host, or that the
//! proxy refuses, is closed.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use crate::sniff::{self, Sniff};
use crate::{parse_port, pump::pump, ready, vsock, Result};

/// Room for a ClientHello as large as one TLS record carries.
const MAX_FIRST_BYTES: usize = 5 + 16 * 1024;
const FIRST_BYTES_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_REPLY_HEAD: usize = 1024;

type HostIn = fn(&[u8]) -> Sniff;

pub fn run(args: &[String]) -> Result<()> {
    let [http_address, tls_address, vsock_port] = args else {
        return Err(
            "transparent-egress: expected <http address> <tls address> <vsock port>".into(),
        );
    };
    let vsock_port = parse_port(Some(vsock_port), "vsock port")?;
    let http = bind(http_address)?;
    let tls = bind(tls_address)?;
    ready::announce(&format!(
        "{http_address} (80) and {tls_address} (443) -> vsock {vsock_port}"
    ));

    thread::spawn(move || accept(http, 80, sniff::http_host, vsock_port));
    accept(tls, 443, sniff::tls_server_name, vsock_port);
    Ok(())
}

fn bind(address: &str) -> Result<TcpListener> {
    TcpListener::bind(address)
        .map_err(|e| format!("transparent-egress: cannot listen on {address}: {e}").into())
}

fn accept(listener: TcpListener, port: u16, host_in: HostIn, vsock_port: u32) {
    for conn in listener.incoming() {
        match conn {
            Ok(conn) => {
                thread::spawn(move || {
                    if let Err(err) = tunnel(conn, port, host_in, vsock_port) {
                        eprintln!("fletch-guest: transparent-egress: :{port}: {err}");
                    }
                });
            }
            Err(err) => eprintln!("fletch-guest: transparent-egress: accept: {err}"),
        }
    }
}

fn tunnel(client: TcpStream, port: u16, host_in: HostIn, vsock_port: u32) -> io::Result<()> {
    client.set_read_timeout(Some(FIRST_BYTES_TIMEOUT))?;
    let (host, first) = read_host(&client, host_in)?;
    client.set_read_timeout(None)?;

    let mut proxy = vsock::connect(vsock_port)?;
    write!(
        proxy,
        "CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n\r\n"
    )?;
    if tunnel_opened(&mut proxy)? {
        proxy.write_all(&first)?;
        pump(client, proxy);
    }
    Ok(())
}

/// Reads until `host_in` finds the host; it and every byte read.
fn read_host(mut client: &TcpStream, host_in: HostIn) -> io::Result<(String, Vec<u8>)> {
    let mut first = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match host_in(&first) {
            Sniff::Host(host) => return Ok((host, first)),
            Sniff::Incomplete if first.len() < MAX_FIRST_BYTES => {}
            _ => return Err(io::Error::other("no host name in the first bytes")),
        }
        let n = client.read(&mut chunk)?;
        if n == 0 {
            return Err(io::Error::other("closed before naming a host"));
        }
        first.extend_from_slice(&chunk[..n]);
    }
}

/// Whether the proxy's reply to `CONNECT` is a 200. Read up to its blank line
/// a byte at a time, so nothing of the tunnel after it is consumed.
fn tunnel_opened(proxy: &mut impl Read) -> io::Result<bool> {
    let mut head = Vec::new();
    let mut byte = [0u8];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() >= MAX_REPLY_HEAD || proxy.read(&mut byte)? == 0 {
            return Ok(false);
        }
        head.push(byte[0]);
    }
    Ok(head.split(|&b| b == b' ').nth(1) == Some(b"200"))
}

#[cfg(test)]
mod tests {
    use super::tunnel_opened;
    use std::io::Read;

    #[test]
    fn a_200_opens_the_tunnel_and_what_follows_it_stays_unread() {
        let mut reply: &[u8] = b"HTTP/1.1 200 Connection established\r\n\r\nserver first";
        assert!(tunnel_opened(&mut reply).unwrap());
        let mut rest = String::new();
        reply.read_to_string(&mut rest).unwrap();
        assert_eq!(rest, "server first");
    }

    #[test]
    fn a_refusal_or_a_cut_reply_does_not() {
        let mut refused: &[u8] = b"HTTP/1.1 403 Forbidden\r\nContent-Length: 2\r\n\r\nno";
        assert!(!tunnel_opened(&mut refused).unwrap());
        let mut cut: &[u8] = b"HTTP/1.1 200 Connection established\r\n";
        assert!(!tunnel_opened(&mut cut).unwrap());
        let mut endless: &[u8] = &[b'x'; 4096];
        assert!(!tunnel_opened(&mut endless).unwrap());
    }
}
