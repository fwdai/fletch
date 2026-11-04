//! The box's only way out: an HTTP `CONNECT` proxy on a unix socket that
//! libkrun maps to the guest's vsock port 6000. DNS resolves on the host, the
//! allow-list is checked before anything is connected, and every decision is an
//! [`EgressEvent`] for the engine to surface.
//!
//! A host off the allow-list is answered by the proxy's [`Mode`]: let through
//! (`Open`, `Override`), refused at once (`Refuse`), or held (`Hold`) while the
//! engine asks someone. A host the engine declined is refused under every mode
//! until a list allows it. A held connection waits on the policy: every
//! [`EgressProxy::update`] and [`EgressProxy::decline`] wakes it to check
//! again, so an allow that lands anywhere above travels the one path a policy
//! change already has, and dropping the proxy ends every hold at once. Each
//! hold reports its own end ([`EgressEvent::HoldEnded`]), whichever way it
//! ended, so whoever asked a question on its behalf can close it. A hold
//! nobody resolves ends in a refusal after [`HOLD_TIMEOUT`]. Nothing here
//! knows who is asked or where an answer is kept; that is the engine's.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UnixListener, UnixStream};
use tokio::sync::watch;
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
/// How long a held connection waits for the policy to admit its host before
/// it is refused. Shared by every connection to one host: the clock starts
/// with the first, so a burst to one host ends together.
pub const HOLD_TIMEOUT: Duration = Duration::from_secs(60);
/// The most hosts one proxy holds at a time. A connection to a further host
/// is refused at once, so a guest that reaches for everything cannot queue
/// an unbounded number of questions.
pub const MAX_HELD_HOSTS: usize = 4;

/// What the proxy does with a host the policy does not allow and the engine
/// has not declined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Let it through on an allowed port, telemetry included.
    Open,
    /// [`Mode::Open`] by a standing override of the engine's rather than the
    /// policy itself: a host it admits is reported as [`Admission::Override`],
    /// so the record tells the two apart however the mode changes while the
    /// tunnel is up.
    Override,
    /// Hold the connection and emit [`EgressEvent::Held`] until the policy
    /// changes, the host is declined, or [`HOLD_TIMEOUT`] passes. Hosts on
    /// the [`QUIET`] list are refused at once instead: nothing needs them.
    Hold,
    /// Refuse at once.
    Refuse,
}

/// Why a tunnel was opened, decided as it was admitted and reported with it
/// when it closes, for the record the engine keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// The host is on the allow-list.
    Listed,
    /// The mode was [`Mode::Open`] and the host was not on the list.
    Open,
    /// The mode was [`Mode::Override`] and the host was not on the list.
    Override,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EgressEvent {
    /// A tunnel that was opened and has now closed.
    Allowed {
        host: String,
        port: u16,
        admission: Admission,
        bytes_up: u64,
        bytes_down: u64,
    },
    Denied {
        host: String,
        port: u16,
        reason: Denial,
    },
    /// A connection to `host` is being held for a decision, under
    /// [`Mode::Hold`]. Emitted once per host while any connection to it is
    /// held, however many arrive: the answer is one for all of them.
    Held { host: String, port: u16 },
    /// The hold on `host` is over for every connection that joined it, one
    /// per [`Held`](Self::Held): `admitted` when the policy came to allow the
    /// host (each connection then opens its tunnel and reports `Allowed` as it
    /// closes), else refused (each reports `Denied`), whether by a decline,
    /// a mode that stopped holding, the deadline, or the proxy going away.
    /// Reported as the hold ends, not as the tunnels close, so a question
    /// asked on its behalf can be closed at once.
    HoldEnded {
        host: String,
        port: u16,
        admitted: bool,
    },
    /// A malformed request, or an allowed host that could not be reached.
    Error {
        host: Option<String>,
        message: String,
    },
}

type OnEvent = Arc<dyn Fn(EgressEvent) + Send + Sync>;

/// What every connection consults: the policy and the mode, and the live
/// hold state.
struct Shared {
    policy: RwLock<EgressPolicy>,
    mode: RwLock<Mode>,
    /// Hosts the engine declined for this proxy's lifetime: refused under
    /// every mode, never held again, until a list allows them.
    declined: Mutex<HashSet<String>>,
    /// Hosts with a connection held right now.
    held: Mutex<HashMap<String, Hold>>,
    /// Follows the proxy's generation, which ticks on every change a held
    /// connection should re-check on; each hold takes a clone. The sender is
    /// the proxy's own ([`EgressProxy::generation`]), so dropping the proxy
    /// closes the channel and ends every hold, whatever tasks are still up.
    generation: watch::Receiver<u64>,
    hold_timeout: Duration,
}

/// One host's hold: when the first connection arrived, on which port, and how
/// many are waiting.
struct Hold {
    since: Instant,
    port: u16,
    waiting: usize,
}

impl Shared {
    fn check(&self, host: &str, port: u16) -> Result<(), Denial> {
        self.policy
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .check(host, port)
    }

    fn mode(&self) -> Mode {
        *self.mode.read().unwrap_or_else(|p| p.into_inner())
    }

    fn is_declined(&self, host: &str) -> bool {
        self.declined
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(host)
    }

    /// Join the hold on `host`, or start one. `None` when the proxy already
    /// holds [`MAX_HELD_HOSTS`] other hosts. The bool says whether this is
    /// the first connection, which announces the hold.
    fn join_hold(&self, host: &str, port: u16) -> Option<(Instant, bool)> {
        let mut held = self.held.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(hold) = held.get_mut(host) {
            hold.waiting += 1;
            return Some((hold.since, false));
        }
        if held.len() >= MAX_HELD_HOSTS {
            return None;
        }
        let since = Instant::now();
        held.insert(
            host.to_string(),
            Hold {
                since,
                port,
                waiting: 1,
            },
        );
        Some((since, true))
    }

    /// Leave the hold on `host`; `Some(port)` when this was the last
    /// connection in it, which ends the hold.
    fn leave_hold(&self, host: &str) -> Option<u16> {
        let mut held = self.held.lock().unwrap_or_else(|p| p.into_inner());
        let hold = held.get_mut(host)?;
        hold.waiting -= 1;
        if hold.waiting > 0 {
            return None;
        }
        held.remove(host).map(|h| h.port)
    }
}

pub struct EgressProxy {
    socket: PathBuf,
    shared: Arc<Shared>,
    /// Ticks on every [`update`](Self::update) and [`decline`](Self::decline).
    /// Owned here, not shared: when the proxy goes, the channel closes and
    /// every held connection refuses at once rather than at its deadline.
    generation: watch::Sender<u64>,
    accept: JoinHandle<()>,
}

impl EgressProxy {
    /// Binds `socket` (replacing a stale file) and serves it on the current
    /// tokio runtime until [`stop`](Self::stop) or drop.
    pub fn start(
        socket: &Path,
        policy: EgressPolicy,
        mode: Mode,
        on_event: impl Fn(EgressEvent) + Send + Sync + 'static,
    ) -> io::Result<EgressProxy> {
        Self::start_with_timeout(socket, policy, mode, HOLD_TIMEOUT, on_event)
    }

    fn start_with_timeout(
        socket: &Path,
        policy: EgressPolicy,
        mode: Mode,
        hold_timeout: Duration,
        on_event: impl Fn(EgressEvent) + Send + Sync + 'static,
    ) -> io::Result<EgressProxy> {
        crate::fsutil::remove_if_exists(socket)?;
        let listener = UnixListener::bind(socket)?;
        let (generation, follow) = watch::channel(0u64);
        let shared = Arc::new(Shared {
            policy: RwLock::new(policy),
            mode: RwLock::new(mode),
            declined: Mutex::new(HashSet::new()),
            held: Mutex::new(HashMap::new()),
            generation: follow,
            hold_timeout,
        });
        let on_event: OnEvent = Arc::new(on_event);
        let accept = {
            let shared = shared.clone();
            tokio::spawn(async move {
                loop {
                    match listener.accept().await {
                        Ok((stream, _)) => {
                            tokio::spawn(serve(stream, shared.clone(), on_event.clone()));
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
            shared,
            generation,
            accept,
        })
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    pub fn policy(&self) -> EgressPolicy {
        self.shared
            .policy
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn mode(&self) -> Mode {
        self.shared.mode()
    }

    /// Replaces the policy and the mode for connections from now on, and wakes
    /// every held connection to check again; open tunnels are unaffected.
    pub fn update(&self, policy: EgressPolicy, mode: Mode) {
        *self
            .shared
            .policy
            .write()
            .unwrap_or_else(|p| p.into_inner()) = policy;
        *self.shared.mode.write().unwrap_or_else(|p| p.into_inner()) = mode;
        self.bump();
    }

    /// Refuse `host` from now on under every mode, the held connections to it
    /// included, until a list allows it. `host` is normalized here.
    pub fn decline(&self, host: &str) {
        let Some(host) = normalize_host(host) else {
            return;
        };
        self.shared
            .declined
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(host);
        self.bump();
    }

    fn bump(&self) {
        self.generation.send_modify(|g| *g += 1);
    }

    /// The hosts with a connection held right now.
    pub fn held_hosts(&self) -> Vec<String> {
        let mut hosts: Vec<String> = self
            .shared
            .held
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .keys()
            .cloned()
            .collect();
        hosts.sort();
        hosts
    }

    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for EgressProxy {
    /// No new connection is accepted, and `generation` goes with the proxy,
    /// so every held connection wakes, refuses and reports its hold ended.
    /// Open tunnels run until their peers close them.
    fn drop(&mut self) {
        self.accept.abort();
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// How a connection to a host is answered now.
enum Verdict {
    Allow(Admission),
    Refuse(Denial),
    Hold,
}

/// The list first, then a decline, then the mode: a listed host passes under
/// any mode; a declined host is the engine's answer about this host on this
/// proxy and outranks the mode, so switching to `Open` or `Override` does not
/// admit it again; only a list does.
fn decide(shared: &Shared, host: &str, port: u16) -> Verdict {
    match shared.check(host, port) {
        Ok(()) => Verdict::Allow(Admission::Listed),
        Err(Denial::Port) => Verdict::Refuse(Denial::Port),
        Err(Denial::Host) if shared.is_declined(host) => Verdict::Refuse(Denial::Host),
        Err(Denial::Host) => match shared.mode() {
            Mode::Open => Verdict::Allow(Admission::Open),
            Mode::Override => Verdict::Allow(Admission::Override),
            Mode::Refuse => Verdict::Refuse(Denial::Host),
            Mode::Hold if is_quiet(host) => Verdict::Refuse(Denial::Host),
            Mode::Hold => Verdict::Hold,
        },
    }
}

async fn serve(mut client: UnixStream, shared: Arc<Shared>, on_event: OnEvent) {
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

    let admission = match decide(&shared, &host, port) {
        Verdict::Allow(admission) => admission,
        Verdict::Refuse(reason) => return refuse(&mut client, &on_event, host, port, reason).await,
        Verdict::Hold => match hold(&shared, &on_event, &host, port).await {
            Some(admission) => admission,
            None => return refuse(&mut client, &on_event, host, port, Denial::Host).await,
        },
    };

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
        admission,
        bytes_up,
        bytes_down,
    });
}

/// Wait for the policy to admit `host`: `Some` with how it was admitted,
/// `None` when the host was declined, the mode stopped holding, the proxy
/// went away, or the hold's deadline passed. The hold is announced once per
/// host, by the first connection to join it, and its end reported once, by
/// the last to leave.
///
/// The subscription comes first and the policy is checked again before the
/// first wait: a change that lands between the caller's decision and the
/// subscription is seen by that check, and one that lands after it wakes the
/// wait, so no update is ever missed. An answer given from inside the `Held`
/// callback itself resolves the hold too.
async fn hold(shared: &Shared, on_event: &OnEvent, host: &str, port: u16) -> Option<Admission> {
    let mut generation = shared.generation.clone();
    // Seen up to here; the check below covers everything before it.
    let _ = generation.borrow_and_update();
    let (since, first) = shared.join_hold(host, port)?;
    if first {
        on_event(EgressEvent::Held {
            host: host.to_string(),
            port,
        });
    }
    let deadline = since + shared.hold_timeout;
    let admitted = loop {
        match decide(shared, host, port) {
            Verdict::Allow(admission) => break Some(admission),
            Verdict::Refuse(_) => break None,
            Verdict::Hold => {}
        }
        match tokio::time::timeout_at(deadline.into(), generation.changed()).await {
            Ok(Ok(())) => continue,
            // The proxy was dropped, or the deadline passed: nothing will
            // admit this host now.
            Ok(Err(_)) | Err(_) => break None,
        }
    };
    if let Some(port) = shared.leave_hold(host) {
        on_event(EgressEvent::HoldEnded {
            host: host.to_string(),
            port,
            admitted: admitted.is_some(),
        });
    }
    admitted
}

async fn refuse(
    client: &mut UnixStream,
    on_event: &OnEvent,
    host: String,
    port: u16,
    reason: Denial,
) {
    on_event(EgressEvent::Denied {
        host: host.clone(),
        port,
        reason,
    });
    let _ = respond(
        client,
        "403 Forbidden",
        &format!("{host}:{port}: {reason} by the box egress policy"),
    )
    .await;
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
        mode: Mode,
    ) -> (EgressProxy, mpsc::UnboundedReceiver<EgressEvent>) {
        start_with(dir, policy, mode, HOLD_TIMEOUT).await
    }

    async fn start_with(
        dir: &Path,
        policy: EgressPolicy,
        mode: Mode,
        hold_timeout: Duration,
    ) -> (EgressProxy, mpsc::UnboundedReceiver<EgressEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let proxy = EgressProxy::start_with_timeout(
            &dir.join("egress.sock"),
            policy,
            mode,
            hold_timeout,
            move |e| {
                let _ = tx.send(e);
            },
        )
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

    /// A local upstream that echoes "pong!!" to the first 5 bytes it reads,
    /// `times` times over.
    async fn echo_upstream(times: usize) -> u16 {
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = upstream.local_addr().unwrap().port();
        tokio::spawn(async move {
            for _ in 0..times {
                let (mut s, _) = upstream.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buf = [0u8; 5];
                    s.read_exact(&mut buf).await.unwrap();
                    s.write_all(b"pong!!").await.unwrap();
                });
            }
        });
        port
    }

    async fn ping(client: &mut UnixStream) {
        client.write_all(b"ping!").await.unwrap();
        let mut echoed = [0u8; 6];
        client.read_exact(&mut echoed).await.unwrap();
        assert_eq!(&echoed, b"pong!!");
    }

    fn connect_head(port: u16) -> String {
        format!("CONNECT localhost:{port} HTTP/1.1\r\nHost: localhost\r\n\r\n")
    }

    /// The next `n` events, whatever order their tasks emitted them in.
    async fn next_events(
        events: &mut mpsc::UnboundedReceiver<EgressEvent>,
        n: usize,
    ) -> Vec<EgressEvent> {
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(events.recv().await.unwrap());
        }
        out
    }

    #[tokio::test]
    async fn allowed_hosts_are_tunnelled_and_counted() {
        let dir = tempfile::tempdir().unwrap();
        let port = echo_upstream(1).await;
        let mut policy = EgressPolicy::new(["localhost"]).unwrap();
        policy.ports = vec![port];
        let (proxy, mut events) = start(dir.path(), policy, Mode::Refuse).await;

        let (mut client, reply) = request(proxy.socket(), &connect_head(port)).await;
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        ping(&mut client).await;
        drop(client);

        assert_eq!(
            events.recv().await.unwrap(),
            EgressEvent::Allowed {
                host: "localhost".into(),
                port,
                admission: Admission::Listed,
                bytes_up: 5,
                bytes_down: 6
            }
        );
    }

    #[tokio::test]
    async fn other_hosts_methods_and_ports_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (proxy, mut events) = start(
            dir.path(),
            EgressPolicy::new(["allowed.test"]).unwrap(),
            Mode::Refuse,
        )
        .await;

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

        proxy.update(EgressPolicy::new(["example.com"]).unwrap(), Mode::Refuse);
        assert_eq!(proxy.policy().check("example.com", 443), Ok(()));
        let sock = proxy.socket().to_path_buf();
        proxy.stop();
        assert!(!sock.exists());
    }

    /// Under `Open` a host off the list is tunnelled and reported as such; a
    /// port off the list is still refused, as is everything under `Refuse`.
    #[tokio::test]
    async fn open_mode_admits_unlisted_hosts_on_allowed_ports_only() {
        let dir = tempfile::tempdir().unwrap();
        let port = echo_upstream(1).await;
        let mut policy = EgressPolicy::new(["other.test"]).unwrap();
        policy.ports = vec![port];
        let (proxy, mut events) = start(dir.path(), policy, Mode::Open).await;

        let (mut client, reply) = request(proxy.socket(), &connect_head(port)).await;
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        ping(&mut client).await;
        drop(client);
        assert!(matches!(
            events.recv().await.unwrap(),
            EgressEvent::Allowed {
                admission: Admission::Open,
                ..
            }
        ));

        let (_c, reply) = request(proxy.socket(), "CONNECT localhost:22 HTTP/1.1\r\n\r\n").await;
        assert!(reply.starts_with("HTTP/1.1 403"), "{reply}");
        assert!(matches!(
            events.recv().await.unwrap(),
            EgressEvent::Denied {
                reason: Denial::Port,
                ..
            }
        ));
    }

    /// A declined host stays refused under `Open` and `Override` alike: the
    /// decline is the engine's answer about that host, and only a list allows
    /// it again.
    #[tokio::test]
    async fn a_decline_outranks_the_open_modes_and_a_list_outranks_the_decline() {
        let dir = tempfile::tempdir().unwrap();
        let port = echo_upstream(1).await;
        let mut policy = EgressPolicy::new(["other.test"]).unwrap();
        policy.ports = vec![port];
        let (proxy, mut events) = start(dir.path(), policy.clone(), Mode::Open).await;

        proxy.decline("LocalHost");
        for mode in [Mode::Open, Mode::Override] {
            proxy.update(policy.clone(), mode);
            let (_c, reply) = request(proxy.socket(), &connect_head(port)).await;
            assert!(reply.starts_with("HTTP/1.1 403"), "{mode:?}: {reply}");
            assert_eq!(
                events.recv().await.unwrap(),
                EgressEvent::Denied {
                    host: "localhost".into(),
                    port,
                    reason: Denial::Host
                }
            );
        }

        policy.allow.push("localhost".parse().unwrap());
        proxy.update(policy, Mode::Open);
        let (mut client, reply) = request(proxy.socket(), &connect_head(port)).await;
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        ping(&mut client).await;
        drop(client);
        assert!(matches!(
            events.recv().await.unwrap(),
            EgressEvent::Allowed {
                admission: Admission::Listed,
                ..
            }
        ));
    }

    /// Under `Hold`, two connections to one unlisted host announce one hold;
    /// an `update` that lists the host ends the hold as admitted, once, and
    /// releases both into tunnels.
    #[tokio::test]
    async fn held_connections_share_one_hold_and_follow_an_allow() {
        let dir = tempfile::tempdir().unwrap();
        let port = echo_upstream(2).await;
        let mut policy = EgressPolicy::new(["other.test"]).unwrap();
        policy.ports = vec![port];
        let (proxy, mut events) = start(dir.path(), policy.clone(), Mode::Hold).await;

        let sock = proxy.socket().to_path_buf();
        let first = tokio::spawn({
            let sock = sock.clone();
            async move { request(&sock, &connect_head(port)).await }
        });
        assert_eq!(
            events.recv().await.unwrap(),
            EgressEvent::Held {
                host: "localhost".into(),
                port
            }
        );
        let second = tokio::spawn({
            let sock = sock.clone();
            async move { request(&sock, &connect_head(port)).await }
        });
        // The second joins the hold without announcing again.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(proxy.held_hosts(), vec!["localhost".to_string()]);
        assert!(events.try_recv().is_err(), "one hold per host");

        policy.allow.push("localhost".parse().unwrap());
        proxy.update(policy, Mode::Hold);
        for handle in [first, second] {
            let (mut client, reply) = handle.await.unwrap();
            assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
            ping(&mut client).await;
            drop(client);
        }
        assert!(proxy.held_hosts().is_empty());
        let seen = next_events(&mut events, 3).await;
        assert_eq!(
            seen.iter()
                .filter(|e| matches!(e, EgressEvent::HoldEnded { admitted: true, .. }))
                .count(),
            1,
            "{seen:?}"
        );
        assert_eq!(
            seen.iter()
                .filter(|e| matches!(
                    e,
                    EgressEvent::Allowed {
                        admission: Admission::Listed,
                        ..
                    }
                ))
                .count(),
            2,
            "{seen:?}"
        );
    }

    /// A hold released by the mode opening, not by the list, ends as admitted
    /// the moment the mode changes, before its tunnel has run its course.
    #[tokio::test]
    async fn a_mode_that_opens_ends_the_hold_as_admitted_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let port = echo_upstream(1).await;
        let mut policy = EgressPolicy::new(["other.test"]).unwrap();
        policy.ports = vec![port];
        let (proxy, mut events) = start(dir.path(), policy.clone(), Mode::Hold).await;

        let sock = proxy.socket().to_path_buf();
        let held = tokio::spawn(async move { request(&sock, &connect_head(port)).await });
        assert!(matches!(
            events.recv().await.unwrap(),
            EgressEvent::Held { .. }
        ));
        proxy.update(policy, Mode::Open);
        let (mut client, reply) = held.await.unwrap();
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        // Reported while the tunnel is still up.
        assert_eq!(
            events.recv().await.unwrap(),
            EgressEvent::HoldEnded {
                host: "localhost".into(),
                port,
                admitted: true
            }
        );
        ping(&mut client).await;
        drop(client);
        assert!(matches!(
            events.recv().await.unwrap(),
            EgressEvent::Allowed {
                admission: Admission::Open,
                ..
            }
        ));
    }

    /// A declined host's held connection is refused with the ordinary denial
    /// after its hold is reported over, and a later connection to it is
    /// refused at once with no hold; an allow still wins over the decline.
    #[tokio::test]
    async fn a_decline_refuses_the_hold_and_every_connection_after() {
        let dir = tempfile::tempdir().unwrap();
        let (proxy, mut events) = start(
            dir.path(),
            EgressPolicy::new(["other.test"]).unwrap(),
            Mode::Hold,
        )
        .await;
        let sock = proxy.socket().to_path_buf();
        let held = tokio::spawn({
            let sock = sock.clone();
            async move { request(&sock, "CONNECT Example.com:443 HTTP/1.1\r\n\r\n").await }
        });
        assert!(matches!(
            events.recv().await.unwrap(),
            EgressEvent::Held { .. }
        ));

        proxy.decline("Example.COM");
        let (_c, reply) = held.await.unwrap();
        assert!(reply.starts_with("HTTP/1.1 403"), "{reply}");
        assert_eq!(
            events.recv().await.unwrap(),
            EgressEvent::HoldEnded {
                host: "example.com".into(),
                port: 443,
                admitted: false
            }
        );
        assert_eq!(
            events.recv().await.unwrap(),
            EgressEvent::Denied {
                host: "example.com".into(),
                port: 443,
                reason: Denial::Host
            }
        );

        let (_c, reply) = request(&sock, "CONNECT example.com:443 HTTP/1.1\r\n\r\n").await;
        assert!(reply.starts_with("HTTP/1.1 403"), "{reply}");
        assert!(matches!(
            events.recv().await.unwrap(),
            EgressEvent::Denied { .. }
        ));

        proxy.update(EgressPolicy::new(["example.com"]).unwrap(), Mode::Hold);
        assert!(matches!(
            decide(&proxy.shared, "example.com", 443),
            Verdict::Allow(Admission::Listed)
        ));
    }

    /// A hold nobody resolves is refused when its deadline passes, with the
    /// ordinary denial; one the mode stops holding is refused at once.
    #[tokio::test]
    async fn an_unanswered_hold_expires_into_a_refusal() {
        let dir = tempfile::tempdir().unwrap();
        let (proxy, mut events) = start_with(
            dir.path(),
            EgressPolicy::new(["other.test"]).unwrap(),
            Mode::Hold,
            Duration::from_millis(200),
        )
        .await;
        let sock = proxy.socket().to_path_buf();
        let started = Instant::now();
        let (_c, reply) = request(&sock, "CONNECT slow.test:443 HTTP/1.1\r\n\r\n").await;
        assert!(reply.starts_with("HTTP/1.1 403"), "{reply}");
        assert!(started.elapsed() >= Duration::from_millis(200));
        assert!(matches!(
            events.recv().await.unwrap(),
            EgressEvent::Held { .. }
        ));
        assert_eq!(
            events.recv().await.unwrap(),
            EgressEvent::HoldEnded {
                host: "slow.test".into(),
                port: 443,
                admitted: false
            }
        );
        assert_eq!(
            events.recv().await.unwrap(),
            EgressEvent::Denied {
                host: "slow.test".into(),
                port: 443,
                reason: Denial::Host
            }
        );

        let held = tokio::spawn({
            let sock = sock.clone();
            async move { request(&sock, "CONNECT later.test:443 HTTP/1.1\r\n\r\n").await }
        });
        assert!(matches!(
            events.recv().await.unwrap(),
            EgressEvent::Held { .. }
        ));
        proxy.update(EgressPolicy::new(["other.test"]).unwrap(), Mode::Refuse);
        let (_c, reply) = held.await.unwrap();
        assert!(reply.starts_with("HTTP/1.1 403"), "{reply}");
    }

    /// Telemetry is never held, and once `MAX_HELD_HOSTS` hosts wait, a
    /// further host is refused at once rather than queued. Dropping the proxy
    /// ends every hold at once, not at its deadline.
    #[tokio::test]
    async fn quiet_hosts_and_hosts_past_the_cap_are_refused_without_a_hold() {
        let dir = tempfile::tempdir().unwrap();
        let (proxy, mut events) = start(
            dir.path(),
            EgressPolicy::new(["other.test"]).unwrap(),
            Mode::Hold,
        )
        .await;
        let sock = proxy.socket().to_path_buf();

        let (_c, reply) = request(&sock, "CONNECT o1.ingest.sentry.io:443 HTTP/1.1\r\n\r\n").await;
        assert!(reply.starts_with("HTTP/1.1 403"), "{reply}");
        assert!(matches!(
            events.recv().await.unwrap(),
            EgressEvent::Denied { .. }
        ));

        let mut held = Vec::new();
        for i in 0..MAX_HELD_HOSTS {
            held.push(tokio::spawn({
                let sock = sock.clone();
                async move { request(&sock, &format!("CONNECT h{i}.test:443 HTTP/1.1\r\n\r\n")).await }
            }));
            assert!(matches!(
                events.recv().await.unwrap(),
                EgressEvent::Held { .. }
            ));
        }
        assert_eq!(proxy.held_hosts().len(), MAX_HELD_HOSTS);
        let (_c, reply) = request(&sock, "CONNECT one-too-many.test:443 HTTP/1.1\r\n\r\n").await;
        assert!(reply.starts_with("HTTP/1.1 403"), "{reply}");
        assert_eq!(
            events.recv().await.unwrap(),
            EgressEvent::Denied {
                host: "one-too-many.test".into(),
                port: 443,
                reason: Denial::Host
            }
        );

        // Dropping the proxy refuses what it held, now: the deadline is a
        // minute away.
        let dropped = Instant::now();
        drop(proxy);
        for handle in held {
            let (_c, reply) = handle.await.unwrap();
            assert!(reply.starts_with("HTTP/1.1 403"), "{reply}");
        }
        assert!(
            dropped.elapsed() < Duration::from_secs(5),
            "held connections ended with the proxy, not at the deadline"
        );
        let seen = next_events(&mut events, 2 * MAX_HELD_HOSTS).await;
        assert_eq!(
            seen.iter()
                .filter(|e| matches!(
                    e,
                    EgressEvent::HoldEnded {
                        admitted: false,
                        ..
                    }
                ))
                .count(),
            MAX_HELD_HOSTS,
            "{seen:?}"
        );
    }

    /// An answer that lands while the hold is being set up — here from inside
    /// the `Held` callback itself, before the old code subscribed — is not
    /// missed: the connection is admitted at once, not at the deadline.
    #[tokio::test]
    async fn an_answer_during_the_held_callback_is_not_missed() {
        let dir = tempfile::tempdir().unwrap();
        let port = echo_upstream(1).await;
        let mut policy = EgressPolicy::new(["other.test"]).unwrap();
        policy.ports = vec![port];
        let (tx, mut events) = mpsc::unbounded_channel();
        let handle: Arc<std::sync::OnceLock<std::sync::Weak<EgressProxy>>> = Arc::default();
        let proxy = {
            let handle = handle.clone();
            let base = policy.clone();
            Arc::new(
                EgressProxy::start_with_timeout(
                    &dir.path().join("egress.sock"),
                    policy,
                    Mode::Hold,
                    Duration::from_secs(5),
                    move |e| {
                        if let EgressEvent::Held { host, .. } = &e {
                            // Allow it right here, inside the window.
                            let mut allowed = base.clone();
                            allowed.allow.push(host.parse().unwrap());
                            handle
                                .get()
                                .and_then(std::sync::Weak::upgrade)
                                .unwrap()
                                .update(allowed, Mode::Hold);
                        }
                        let _ = tx.send(e);
                    },
                )
                .unwrap(),
            )
        };
        handle.set(Arc::downgrade(&proxy)).ok().unwrap();

        let started = Instant::now();
        let (mut client, reply) = request(proxy.socket(), &connect_head(port)).await;
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "admitted by the answer, not by the deadline"
        );
        ping(&mut client).await;
        drop(client);
        assert!(matches!(
            events.recv().await.unwrap(),
            EgressEvent::Held { .. }
        ));
        assert!(matches!(
            events.recv().await.unwrap(),
            EgressEvent::HoldEnded { admitted: true, .. }
        ));
        assert!(matches!(
            events.recv().await.unwrap(),
            EgressEvent::Allowed {
                admission: Admission::Listed,
                ..
            }
        ));
    }

    /// How a tunnel was admitted is decided as it opens and reported as such
    /// when it closes, whatever the mode became in between.
    #[tokio::test]
    async fn an_override_admission_survives_a_mode_change_while_the_tunnel_is_up() {
        let dir = tempfile::tempdir().unwrap();
        let port = echo_upstream(1).await;
        let mut policy = EgressPolicy::new(["other.test"]).unwrap();
        policy.ports = vec![port];
        let (proxy, mut events) = start(dir.path(), policy.clone(), Mode::Override).await;

        let (mut client, reply) = request(proxy.socket(), &connect_head(port)).await;
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        // The override ends while the tunnel is up.
        proxy.update(policy, Mode::Open);
        ping(&mut client).await;
        drop(client);
        assert!(matches!(
            events.recv().await.unwrap(),
            EgressEvent::Allowed {
                admission: Admission::Override,
                ..
            }
        ));
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
