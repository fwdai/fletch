// Opening the WebSocket's TCP socket with both address families in the race.
//
// `tokio_tungstenite::connect_async` resolves the host, then tries the
// addresses one at a time with no bound on any of them. On a cellular network
// the phone holds a global IPv6 address, so the resolver lists the relay's
// AAAA records first — and when the carrier's IPv6 path to the relay
// blackholes, that first SYN sits until the caller's whole open budget is
// gone and the IPv4 addresses are never dialled. The symptom was a relay that
// worked from any IPv4-only Wi-Fi and "timed out after 15000 ms" on LTE.
//
// This dials the way a browser does (RFC 8305, "Happy Eyeballs"): addresses
// interleaved by family, each attempt started a short stagger after the last,
// the first socket to connect wins and the rest are dropped. TLS and the
// WebSocket upgrade then run on the winner exactly as `connect_async` would
// have done.

use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use futures_util::stream::{FuturesUnordered, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::{client_async_tls, MaybeTlsStream, WebSocketStream};

pub type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// RFC 8305 suggests 250 ms between attempts. A little more here: the
/// cellular round trip this exists for is often that long by itself, and a
/// working first address should not be undercut by the second.
pub const STAGGER: Duration = Duration::from_millis(300);

/// Dial `url` and complete the WebSocket upgrade (TLS included for `wss://`).
/// The caller bounds the whole thing; nothing here waits on its own account
/// beyond the stagger between attempts.
pub async fn connect(url: &str) -> Result<Ws, String> {
    let request = url.into_client_request().map_err(|e| e.to_string())?;
    let (host, port) = host_port(&request)?;
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|e| format!("cannot resolve {host}: {e}"))?
        .collect();
    let stream = race(interleave(addrs)).await.map_err(|e| e.to_string())?;
    let (ws, _) = client_async_tls(request, stream)
        .await
        .map_err(|e| e.to_string())?;
    Ok(ws)
}

/// The host to resolve and the port to dial, with the scheme's default port
/// when the URL names none — the same rule `connect_async` applies.
fn host_port(request: &Request) -> Result<(String, u16), String> {
    let uri = request.uri();
    let host = uri.host().ok_or("the URL names no host")?;
    // `host()` keeps the brackets of an IPv6 literal; the resolver wants them off.
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let port = uri
        .port_u16()
        .or_else(|| match uri.scheme_str() {
            Some("wss") => Some(443),
            Some("ws") => Some(80),
            _ => None,
        })
        .ok_or_else(|| format!("unsupported URL scheme in {uri}"))?;
    Ok((host, port))
}

/// RFC 8305 §4: alternate address families, leading with the resolver's own
/// first choice, so the second attempt is always on the other family.
fn interleave(addrs: Vec<SocketAddr>) -> Vec<SocketAddr> {
    let Some(first) = addrs.first() else {
        return addrs;
    };
    let lead = first.is_ipv6();
    let total = addrs.len();
    let (lead_family, other): (Vec<_>, Vec<_>) =
        addrs.into_iter().partition(|addr| addr.is_ipv6() == lead);
    let mut lead_family = lead_family.into_iter();
    let mut other = other.into_iter();
    let mut out = Vec::with_capacity(total);
    loop {
        match (lead_family.next(), other.next()) {
            (None, None) => return out,
            (a, b) => {
                out.extend(a);
                out.extend(b);
            }
        }
    }
}

/// Start one connect per address, `STAGGER` apart, and take the first that
/// succeeds. Returning drops the set, which abandons every attempt still in
/// flight. When all of them fail the last error is the one reported, as
/// `TcpStream::connect` does for a list.
async fn race(addrs: Vec<SocketAddr>) -> io::Result<TcpStream> {
    let mut attempts: FuturesUnordered<_> = addrs
        .into_iter()
        .enumerate()
        .map(|(i, addr)| async move {
            tokio::time::sleep(STAGGER * i as u32).await;
            TcpStream::connect(addr).await
        })
        .collect();
    let mut last = None;
    while let Some(result) = attempts.next().await {
        match result {
            Ok(stream) => return Ok(stream),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "the host resolved to no address")
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn v4(n: u8) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::new(10, 0, 0, n), 443))
    }

    fn v6(n: u16) -> SocketAddr {
        SocketAddr::from((Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, n), 443))
    }

    #[test]
    fn interleaves_families_leading_with_the_resolvers_first_choice() {
        assert_eq!(
            interleave(vec![v6(1), v6(2), v4(1), v4(2)]),
            vec![v6(1), v4(1), v6(2), v4(2)]
        );
        assert_eq!(
            interleave(vec![v4(1), v4(2), v6(1)]),
            vec![v4(1), v6(1), v4(2)]
        );
        // One family, or nothing: untouched.
        assert_eq!(interleave(vec![v4(1), v4(2)]), vec![v4(1), v4(2)]);
        assert!(interleave(vec![]).is_empty());
    }

    #[test]
    fn host_port_applies_the_scheme_default_and_strips_ipv6_brackets() {
        let req = "wss://relay.fletch.sh/v1/device/k"
            .into_client_request()
            .unwrap();
        assert_eq!(host_port(&req).unwrap(), ("relay.fletch.sh".into(), 443));
        let req = "ws://192.168.1.24:47285/ws".into_client_request().unwrap();
        assert_eq!(host_port(&req).unwrap(), ("192.168.1.24".into(), 47285));
        let req = "ws://[::1]:8080/ws".into_client_request().unwrap();
        assert_eq!(host_port(&req).unwrap(), ("::1".into(), 8080));
    }

    /// The point of the race: an address that does not answer must not cost
    /// the addresses after it their turn.
    #[test]
    fn a_refused_first_address_falls_through_to_one_that_answers() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let live = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let live_addr = live.local_addr().unwrap();
            // A port that was ours a moment ago and is now closed: refused.
            let dead_addr = {
                let l = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
                l.local_addr().unwrap()
            };
            let stream = race(vec![dead_addr, live_addr]).await.unwrap();
            assert_eq!(stream.peer_addr().unwrap(), live_addr);
            let (_, peer) = live.accept().await.unwrap();
            assert_eq!(peer, stream.local_addr().unwrap());
        });
    }

    #[test]
    fn no_address_at_all_is_an_error_not_a_hang() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let err = rt.block_on(race(vec![])).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
