//! The box's only way out: an HTTP `CONNECT` proxy on a unix socket that
//! libkrun maps to the guest's vsock port 6000. DNS resolves on the host, the
//! allow-list is checked before anything is connected, and every decision is an
//! [`EgressEvent`] for the engine to surface.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UnixListener, UnixStream};
use tokio::task::JoinHandle;

mod policy;

pub use policy::{
    is_quiet, normalize_host, Category, Denial, EgressPolicy, HostPattern, PatternError,
    DEFAULT_ALLOW, DEFAULT_PORTS, QUIET, TEMPLATE,
};

/// A proxy request's whole header must arrive within this.
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_HEADER: usize = 8 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EgressEvent {
    /// A tunnel that was opened and has now closed.
    Allowed {
        host: String,
        port: u16,
        bytes_up: u64,
        bytes_down: u64,
    },
    Denied {
        host: String,
        port: u16,
        reason: Denial,
    },
    /// A malformed request, or an allowed host that could not be reached.
    Error {
        host: Option<String>,
        message: String,
    },
}

type OnEvent = Arc<dyn Fn(EgressEvent) + Send + Sync>;

pub struct EgressProxy {
    socket: PathBuf,
    policy: Arc<RwLock<EgressPolicy>>,
    accept: JoinHandle<()>,
}

impl EgressProxy {
    /// Binds `socket` (replacing a stale file) and serves it on the current
    /// tokio runtime until [`stop`](Self::stop) or drop.
    pub fn start(
        socket: &Path,
        policy: EgressPolicy,
        on_event: impl Fn(EgressEvent) + Send + Sync + 'static,
    ) -> io::Result<EgressProxy> {
        crate::fsutil::remove_if_exists(socket)?;
        let listener = UnixListener::bind(socket)?;
        let policy = Arc::new(RwLock::new(policy));
        let on_event: OnEvent = Arc::new(on_event);
        let accept = {
            let policy = policy.clone();
            tokio::spawn(async move {
                loop {
                    match listener.accept().await {
                        Ok((stream, _)) => {
                            tokio::spawn(serve(stream, policy.clone(), on_event.clone()));
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "egress accept failed");
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                }
            })
        };
        Ok(EgressProxy {
            socket: socket.to_path_buf(),
            policy,
            accept,
        })
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    pub fn policy(&self) -> EgressPolicy {
        self.policy
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Replaces the policy for connections from now on; open tunnels are unaffected.
    pub fn update(&self, policy: EgressPolicy) {
        *self.policy.write().unwrap_or_else(|p| p.into_inner()) = policy;
    }

    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for EgressProxy {
    fn drop(&mut self) {
        self.accept.abort();
        let _ = std::fs::remove_file(&self.socket);
    }
}

async fn serve(mut client: UnixStream, policy: Arc<RwLock<EgressPolicy>>, on_event: OnEvent) {
    let (head, mut leftover) =
        match tokio::time::timeout(HEADER_TIMEOUT, read_header(&mut client)).await {
            Ok(Ok(parts)) => parts,
            Ok(Err(e)) => {
                on_event(EgressEvent::Error {
                    host: None,
                    message: format!("bad request: {e}"),
                });
                let _ = respond(&mut client, "400 Bad Request", &e.to_string()).await;
                return;
            }
            Err(_) => return,
        };

    let request_line = head.lines().next().unwrap_or("");
    let mut words = request_line.split_whitespace();
    let (method, target) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
    if method != "CONNECT" {
        on_event(EgressEvent::Error {
            host: None,
            message: format!("refused {method} {target}: only CONNECT is supported"),
        });
        let _ = respond(
            &mut client,
            "405 Method Not Allowed",
            "only CONNECT is supported by the box egress proxy",
        )
        .await;
        return;
    }
    let Some((host, port)) = parse_target(target) else {
        on_event(EgressEvent::Error {
            host: None,
            message: format!("bad CONNECT target {target:?}"),
        });
        let _ = respond(&mut client, "400 Bad Request", "CONNECT needs host:port").await;
        return;
    };

    let decision = policy
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .check(&host, port);
    if let Err(reason) = decision {
        on_event(EgressEvent::Denied {
            host: host.clone(),
            port,
            reason,
        });
        let _ = respond(
            &mut client,
            "403 Forbidden",
            &format!("{host}:{port}: {reason} by the box egress policy"),
        )
        .await;
        return;
    }

    let connect = TcpStream::connect((host.as_str(), port));
    let mut upstream = match tokio::time::timeout(CONNECT_TIMEOUT, connect).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => {
            return gateway_error(&mut client, &on_event, host, port, e.to_string()).await
        }
        Err(_) => {
            return gateway_error(
                &mut client,
                &on_event,
                host,
                port,
                "connect timed out".into(),
            )
            .await
        }
    };
    if client
        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
        .await
        .is_err()
    {
        return;
    }
    // A client that sent its first bytes together with the header gets them delivered.
    if !leftover.is_empty() && upstream.write_all(&leftover).await.is_err() {
        return;
    }
    leftover.clear();
    let (bytes_up, bytes_down) = tokio::io::copy_bidirectional(&mut client, &mut upstream)
        .await
        .unwrap_or((0, 0));
    on_event(EgressEvent::Allowed {
        host,
        port,
        bytes_up,
        bytes_down,
    });
}

async fn gateway_error(
    client: &mut UnixStream,
    on_event: &OnEvent,
    host: String,
    port: u16,
    message: String,
) {
    on_event(EgressEvent::Error {
        host: Some(host.clone()),
        message: format!("{host}:{port}: {message}"),
    });
    let _ = respond(client, "502 Bad Gateway", &message).await;
}

/// The header block as text, and any bytes that followed it in the same reads.
async fn read_header(client: &mut UnixStream) -> io::Result<(String, Vec<u8>)> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(end) = find_header_end(&buf) {
            let leftover = buf.split_off(end);
            return Ok((String::from_utf8_lossy(&buf).into_owned(), leftover));
        }
        if buf.len() >= MAX_HEADER {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "header too long",
            ));
        }
        let n = client.read(&mut chunk).await?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed before the header ended",
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

/// `host:port`, with `[v6]:port` for literal IPv6. The host comes back normalized.
fn parse_target(target: &str) -> Option<(String, u16)> {
    let (host, port) = target.rsplit_once(':')?;
    let port = port.parse().ok()?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let host = if host.parse::<std::net::IpAddr>().is_ok() {
        host.to_ascii_lowercase()
    } else {
        normalize_host(host)?
    };
    Some((host, port))
}

async fn respond(client: &mut UnixStream, status: &str, body: &str) -> io::Result<()> {
    let body = format!("{body}\n");
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    client.write_all(head.as_bytes()).await?;
    client.write_all(body.as_bytes()).await?;
    client.shutdown().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    async fn start(
        dir: &Path,
        policy: EgressPolicy,
    ) -> (EgressProxy, mpsc::UnboundedReceiver<EgressEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let proxy = EgressProxy::start(&dir.join("egress.sock"), policy, move |e| {
            let _ = tx.send(e);
        })
        .unwrap();
        (proxy, rx)
    }

    async fn request(sock: &Path, head: &str) -> (UnixStream, String) {
        let mut client = UnixStream::connect(sock).await.unwrap();
        client.write_all(head.as_bytes()).await.unwrap();
        let mut buf = vec![0u8; 1024];
        let n = client.read(&mut buf).await.unwrap();
        (client, String::from_utf8_lossy(&buf[..n]).into_owned())
    }

    #[tokio::test]
    async fn allowed_hosts_are_tunnelled_and_counted() {
        let dir = tempfile::tempdir().unwrap();
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = upstream.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = upstream.accept().await.unwrap();
            let mut buf = [0u8; 5];
            s.read_exact(&mut buf).await.unwrap();
            s.write_all(b"pong!!").await.unwrap();
        });
        let mut policy = EgressPolicy::new(["localhost"]).unwrap();
        policy.ports = vec![port];
        let (proxy, mut events) = start(dir.path(), policy).await;

        let (mut client, reply) = request(
            proxy.socket(),
            &format!("CONNECT localhost:{port} HTTP/1.1\r\nHost: localhost\r\n\r\n"),
        )
        .await;
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        client.write_all(b"ping!").await.unwrap();
        let mut echoed = [0u8; 6];
        client.read_exact(&mut echoed).await.unwrap();
        assert_eq!(&echoed, b"pong!!");
        drop(client);

        assert_eq!(
            events.recv().await.unwrap(),
            EgressEvent::Allowed {
                host: "localhost".into(),
                port,
                bytes_up: 5,
                bytes_down: 6
            }
        );
    }

    #[tokio::test]
    async fn other_hosts_methods_and_ports_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (proxy, mut events) =
            start(dir.path(), EgressPolicy::new(["allowed.test"]).unwrap()).await;

        let (_c, reply) = request(proxy.socket(), "CONNECT Example.com:443 HTTP/1.1\r\n\r\n").await;
        assert!(reply.starts_with("HTTP/1.1 403"), "{reply}");
        assert_eq!(
            events.recv().await.unwrap(),
            EgressEvent::Denied {
                host: "example.com".into(),
                port: 443,
                reason: Denial::Host
            }
        );

        let (_c, reply) = request(proxy.socket(), "CONNECT allowed.test:22 HTTP/1.1\r\n\r\n").await;
        assert!(reply.starts_with("HTTP/1.1 403"), "{reply}");
        assert_eq!(
            events.recv().await.unwrap(),
            EgressEvent::Denied {
                host: "allowed.test".into(),
                port: 22,
                reason: Denial::Port
            }
        );

        let (_c, reply) =
            request(proxy.socket(), "GET http://allowed.test/ HTTP/1.1\r\n\r\n").await;
        assert!(reply.starts_with("HTTP/1.1 405"), "{reply}");
        assert!(
            reply.ends_with("only CONNECT is supported by the box egress proxy\n"),
            "{reply}"
        );
        assert!(matches!(
            events.recv().await.unwrap(),
            EgressEvent::Error { host: None, .. }
        ));

        proxy.update(EgressPolicy::new(["example.com"]).unwrap());
        assert_eq!(proxy.policy().check("example.com", 443), Ok(()));
        let sock = proxy.socket().to_path_buf();
        proxy.stop();
        assert!(!sock.exists());
    }

    #[test]
    fn targets_parse() {
        assert_eq!(
            parse_target("Example.com:443"),
            Some(("example.com".into(), 443))
        );
        assert_eq!(parse_target("[::1]:80"), Some(("::1".into(), 80)));
        assert_eq!(parse_target("nocolon"), None);
        assert_eq!(parse_target("a b:1"), None);
        assert_eq!(parse_target("x:notaport"), None);
    }
}
