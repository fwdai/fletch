//! The host a client names in the first bytes it sends: the server name of a
//! TLS ClientHello, or the `Host` header of a plain HTTP request.

use crate::wire::Reader;

#[derive(Debug, PartialEq)]
pub enum Sniff {
    /// The bytes so far end before the part that names the host.
    Incomplete,
    Host(String),
    NoHost,
}

const RECORD_HEADER_LEN: usize = 5;
const HANDSHAKE: u8 = 22;
const CLIENT_HELLO: u8 = 1;
const SERVER_NAME: u16 = 0;
const HOST_NAME: u8 = 0;

/// The server name of the ClientHello in the TLS record `bytes` start with.
/// A ClientHello spread over several records (one over 16 KiB) has none.
pub fn tls_server_name(bytes: &[u8]) -> Sniff {
    let mut record = Reader::new(bytes);
    let Some(header) = record.take(RECORD_HEADER_LEN) else {
        return Sniff::Incomplete;
    };
    if header[0] != HANDSHAKE {
        return Sniff::NoHost;
    }
    let Some(handshake) = record.take(u16::from_be_bytes([header[3], header[4]]).into()) else {
        return Sniff::Incomplete;
    };
    found(client_hello_server_name(handshake))
}

/// The `Host` header of the request head `bytes` start with, without its port.
pub fn http_host(bytes: &[u8]) -> Sniff {
    let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") else {
        return Sniff::Incomplete;
    };
    let head = String::from_utf8_lossy(&bytes[..end]);
    let host = head
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("host"))
        .and_then(|(_, value)| value.trim().split(':').next())
        .map(str::to_string);
    found(host)
}

fn client_hello_server_name(handshake: &[u8]) -> Option<String> {
    let mut r = Reader::new(handshake);
    if r.u8()? != CLIENT_HELLO {
        return None;
    }
    let len = r.u24()?;
    let mut hello = Reader::new(r.take(len)?);
    hello.take(2 + 32)?; // version, random
    hello.block8()?; // session id
    hello.block16()?; // cipher suites
    hello.block8()?; // compression methods
    let mut extensions = Reader::new(hello.block16()?);
    while !extensions.is_empty() {
        let kind = extensions.u16()?;
        let data = extensions.block16()?;
        if kind == SERVER_NAME {
            return host_name(data);
        }
    }
    None
}

fn host_name(server_name_extension: &[u8]) -> Option<String> {
    let mut names = Reader::new(Reader::new(server_name_extension).block16()?);
    while !names.is_empty() {
        let kind = names.u8()?;
        let name = names.block16()?;
        if kind == HOST_NAME {
            return String::from_utf8(name.to_vec()).ok();
        }
    }
    None
}

fn found(host: Option<String>) -> Sniff {
    match host {
        Some(host) if is_host_name(&host) => Sniff::Host(host),
        _ => Sniff::NoHost,
    }
}

/// A name goes into a `CONNECT` line, so only what a DNS name is made of.
fn is_host_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block16(body: &[u8]) -> Vec<u8> {
        [&(body.len() as u16).to_be_bytes()[..], body].concat()
    }

    fn extension(kind: u16, data: &[u8]) -> Vec<u8> {
        [&kind.to_be_bytes()[..], &block16(data)].concat()
    }

    fn server_name(name: &str) -> Vec<u8> {
        let entry = [&[HOST_NAME][..], &block16(name.as_bytes())].concat();
        extension(SERVER_NAME, &block16(&entry))
    }

    /// One TLS record holding a ClientHello with these extensions.
    fn record(extensions: &[Vec<u8>]) -> Vec<u8> {
        let mut hello = vec![3, 3];
        hello.extend_from_slice(&[7; 32]);
        hello.extend_from_slice(&[32]);
        hello.extend_from_slice(&[9; 32]);
        hello.extend_from_slice(&block16(&[0x13, 0x01, 0x13, 0x02]));
        hello.extend_from_slice(&[1, 0]);
        hello.extend_from_slice(&block16(&extensions.concat()));
        let len = (hello.len() as u32).to_be_bytes();
        let handshake = [&[CLIENT_HELLO, len[1], len[2], len[3]][..], &hello].concat();
        [&[HANDSHAKE, 3, 1][..], &block16(&handshake)].concat()
    }

    fn supported_versions() -> Vec<u8> {
        extension(43, &[2, 3, 4])
    }

    #[test]
    fn the_server_name_of_a_client_hello() {
        let hello = record(&[supported_versions(), server_name("registry.npmjs.org")]);
        assert_eq!(
            tls_server_name(&hello),
            Sniff::Host("registry.npmjs.org".into())
        );
    }

    #[test]
    fn a_client_hello_is_incomplete_until_its_record_is_whole() {
        let hello = record(&[server_name("example.com")]);
        for cut in 0..hello.len() {
            assert_eq!(
                tls_server_name(&hello[..cut]),
                Sniff::Incomplete,
                "cut at {cut}"
            );
        }
    }

    #[test]
    fn no_host_without_a_usable_server_name() {
        assert_eq!(
            tls_server_name(&record(&[supported_versions()])),
            Sniff::NoHost
        );
        assert_eq!(
            tls_server_name(&record(&[server_name("a b\r\n")])),
            Sniff::NoHost
        );
        assert_eq!(tls_server_name(&record(&[server_name("")])), Sniff::NoHost);
        assert_eq!(tls_server_name(b"GET / HTTP/1.1\r\n"), Sniff::NoHost);
    }

    #[test]
    fn the_host_header_without_its_port() {
        let head = b"GET /debian/dists/trixie/InRelease HTTP/1.1\r\nUser-Agent: x\r\nhost: Deb.Debian.org:80\r\n\r\nbody";
        assert_eq!(http_host(head), Sniff::Host("Deb.Debian.org".into()));
        let head = b"GET / HTTP/1.1\r\nHost:example.com\r\n\r\n";
        assert_eq!(http_host(head), Sniff::Host("example.com".into()));
    }

    #[test]
    fn a_request_head_is_incomplete_until_its_blank_line() {
        let head = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
        for cut in 0..head.len() {
            assert_eq!(http_host(&head[..cut]), Sniff::Incomplete, "cut at {cut}");
        }
    }

    #[test]
    fn no_host_without_a_usable_host_header() {
        assert_eq!(http_host(b"GET / HTTP/1.0\r\n\r\n"), Sniff::NoHost);
        assert_eq!(
            http_host(b"GET / HTTP/1.1\r\nHost: \r\n\r\n"),
            Sniff::NoHost
        );
        assert_eq!(
            http_host(b"GET / HTTP/1.1\r\nHost: a b\r\n\r\n"),
            Sniff::NoHost
        );
        assert_eq!(
            http_host(b"GET / HTTP/1.1\r\nX-Host: example.com\r\n\r\n"),
            Sniff::NoHost
        );
    }
}
