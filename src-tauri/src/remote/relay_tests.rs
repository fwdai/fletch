//! The host link, exercised against a fake relay.
//!
//! The fake relay is the other half of `docs/remote-protocol.md` → "Relay":
//! it accepts `/v1/host/<hostId>`, runs the DH challenge, and then speaks the
//! multiplexing codec, wrapping a real Noise initiator inside DATA frames. So
//! these tests drive the same code path a phone on the far side of a Cloudflare
//! Worker would, minus the Worker.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::{Bytes, Message};
use tokio_tungstenite::WebSocketStream;

use super::relay::{self, Frame, RelayState, RelayTiming};
use super::secure::{self, HostKey, SecureChannel};
use super::tests::{device, Device, StubDispatch};
use super::{RemoteState, DEFAULT_RELAY_URL};

/// Every await in these tests is bounded: a link that never arrives should fail
/// the test, not hang the suite.
const PATIENCE: Duration = Duration::from_secs(5);

async fn within<F: Future>(what: &str, f: F) -> F::Output {
    tokio::time::timeout(PATIENCE, f)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

/// Short enough that the reconnect and wind-down paths run inside a test, long
/// enough that the `error` window between attempts is observable by a poll.
fn fast() -> RelayTiming {
    RelayTiming {
        backoff_initial: Duration::from_millis(100),
        backoff_max: Duration::from_millis(100),
        auth_timeout: Duration::from_secs(2),
        // Longer than any test: the host↔relay keepalive is not what these
        // exercise, and a stray ping would only add noise to the frame stream.
        ping_interval: Duration::from_secs(30),
        shutdown_grace: Duration::from_millis(500),
    }
}

// ---------------------------------------------------------------------------
// The fake relay
// ---------------------------------------------------------------------------

/// How the fake relay answers the host's proof.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Verify the proof exactly as the doc specifies, then send `ready`.
    Accept,
    /// Close 4003 without looking, as the relay does for a bad proof.
    Reject,
}

struct FakeRelay {
    url: String,
    links: mpsc::Receiver<HostLink>,
    /// Upgrades accepted, so a reconnect can be asserted even when the link
    /// never gets as far as `ready`.
    attempts: Arc<AtomicU32>,
}

impl FakeRelay {
    async fn start(verdict: Verdict) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, links) = mpsc::channel(4);
        let attempts = Arc::new(AtomicU32::new(0));
        tokio::spawn(accept_loop(listener, verdict, tx, attempts.clone()));
        Self {
            url: format!("ws://127.0.0.1:{port}"),
            links,
            attempts,
        }
    }

    async fn next_link(&mut self) -> HostLink {
        within("a host link", self.links.recv())
            .await
            .expect("the relay's accept loop died")
    }
}

/// The relay's own static key. Fixed rather than random so a failing test can
/// be replayed, and so the proof vector below is reproducible.
const RELAY_PRIVATE: [u8; 32] = [
    0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02,
    0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02,
];
const RELAY_NONCE: [u8; 32] = [
    0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03,
    0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03,
];

/// The `Err` of tungstenite's upgrade callback is an `http::Response`, which
/// clippy finds large; it is the crate's signature, not ours (see
/// `server::check_path`, which carries the same allow).
#[allow(clippy::result_large_err)]
async fn accept_loop(
    listener: TcpListener,
    verdict: Verdict,
    links: mpsc::Sender<HostLink>,
    attempts: Arc<AtomicU32>,
) {
    let relay_key = HostKey::from_private(RELAY_PRIVATE).unwrap();
    while let Ok((tcp, _)) = listener.accept().await {
        attempts.fetch_add(1, Ordering::SeqCst);
        let seen = Arc::new(Mutex::new(String::new()));
        let path = seen.clone();
        let capture = move |req: &Request, response: Response| -> Result<Response, ErrorResponse> {
            *path.lock() = req.uri().path().to_string();
            Ok(response)
        };
        let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(tcp, capture).await else {
            continue;
        };
        let path = seen.lock().clone();

        // 1. challenge
        let challenge = json!({
            "type": "challenge",
            "nonce": B64.encode(RELAY_NONCE),
            "relayKey": B64.encode(relay_key.public_bytes()),
        });
        ws.send(Message::Text(challenge.to_string().into()))
            .await
            .unwrap();

        // 2. proof, verified from the relay's side of the same DH: the host ID
        //    in the path *is* the public key to check against, so the relay
        //    stores nothing.
        let proof = match next_text(&mut ws).await {
            Some(text) => text,
            None => continue,
        };
        let host_public: [u8; 32] = B64
            .decode(path.rsplit('/').next().unwrap_or_default())
            .expect("the host id in the path is base64url")
            .try_into()
            .expect("the host id is 32 bytes");
        let shared = relay_key.diffie_hellman(&host_public).unwrap();
        let mut digest = <sha2::Sha256 as sha2::Digest>::new();
        sha2::Digest::update(&mut digest, shared);
        sha2::Digest::update(&mut digest, RELAY_NONCE);
        sha2::Digest::update(&mut digest, host_public);
        let expected = B64.encode(sha2::Digest::finalize(digest));
        let offered: Value = serde_json::from_str(&proof).expect("the proof frame is JSON");
        let good = offered["type"] == "proof" && offered["proof"] == expected;

        // 3. ready, or 4003
        if verdict == Verdict::Reject || !good {
            let _ = ws
                .send(Message::Close(Some(CloseFrame {
                    code: CloseCode::Library(4003),
                    reason: "bad proof".into(),
                })))
                .await;
            continue;
        }
        ws.send(Message::Text("{\"type\":\"ready\"}".into()))
            .await
            .unwrap();
        if links
            .send(HostLink {
                ws,
                path,
                pending: VecDeque::new(),
            })
            .await
            .is_err()
        {
            return;
        }
    }
}

async fn next_text(ws: &mut WebSocketStream<TcpStream>) -> Option<String> {
    while let Some(Ok(msg)) = ws.next().await {
        match msg {
            Message::Text(text) => return Some(text.to_string()),
            Message::Ping(_) | Message::Pong(_) => continue,
            _ => return None,
        }
    }
    None
}

/// One accepted host link, from the relay's side.
struct HostLink {
    ws: WebSocketStream<TcpStream>,
    path: String,
    /// Frames for a connection other than the one currently being read, kept
    /// so two devices can be driven in any order.
    pending: VecDeque<Frame>,
}

impl HostLink {
    async fn send(&mut self, frame: Frame) {
        self.ws
            .send(Message::Binary(frame.encode()))
            .await
            .expect("the host link accepts frames");
    }

    /// The next frame, or `None` once the link is over.
    async fn read(&mut self) -> Option<Frame> {
        loop {
            match within("a link frame", self.ws.next()).await {
                Some(Ok(Message::Binary(bytes))) => {
                    return Some(Frame::decode(&bytes).expect("a decodable frame"))
                }
                Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => continue,
                Some(Ok(Message::Close(_))) | None => return None,
                Some(Ok(other)) => panic!("unexpected message on the link: {other:?}"),
                Some(Err(_)) => return None,
            }
        }
    }

    /// The next frame as raw bytes, so a test can assert the wire layout the
    /// relay's own decoder is written against rather than this codec's idea of
    /// it. `None` once the link is over.
    async fn read_raw(&mut self) -> Option<Bytes> {
        loop {
            match within("a link frame", self.ws.next()).await {
                Some(Ok(Message::Binary(bytes))) => return Some(bytes),
                Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => continue,
                Some(Ok(Message::Close(_))) | None => return None,
                Some(Ok(other)) => panic!("unexpected message on the link: {other:?}"),
                Some(Err(_)) => return None,
            }
        }
    }

    /// The next frame belonging to `conn`, buffering the others.
    async fn read_for(&mut self, conn: u32) -> Frame {
        if let Some(i) = self.pending.iter().position(|f| f.conn() == conn) {
            return self.pending.remove(i).unwrap();
        }
        loop {
            let frame = self.read().await.expect("the link ended early");
            if frame.conn() == conn {
                return frame;
            }
            self.pending.push_back(frame);
        }
    }

    async fn data_for(&mut self, conn: u32) -> Bytes {
        match self.read_for(conn).await {
            Frame::Data { payload, .. } => payload,
            other => panic!("expected DATA for {conn}, got {other:?}"),
        }
    }

    async fn close_for(&mut self, conn: u32) -> (u16, String) {
        match self.read_for(conn).await {
            Frame::Close { code, reason, .. } => (code, reason),
            other => panic!("expected CLOSE for {conn}, got {other:?}"),
        }
    }

    /// Every frame until the link goes away, so a wind-down can be asserted in
    /// full without depending on the order two connections finish in.
    async fn drain(&mut self) -> Vec<Frame> {
        let mut frames: Vec<Frame> = self.pending.drain(..).collect();
        while let Some(frame) = self.read().await {
            frames.push(frame);
        }
        frames
    }
}

// ---------------------------------------------------------------------------
// A phone on the far side of the relay
// ---------------------------------------------------------------------------

/// A device link as the relay presents it: one virtual connection carrying the
/// same Noise channel a LAN socket would.
struct Phone {
    conn: u32,
    channel: SecureChannel,
}

impl Phone {
    /// OPEN the connection and run the initiator's half of the handshake, each
    /// message wrapped in a DATA frame.
    async fn open(link: &mut HostLink, conn: u32, phone: &Device) -> Self {
        link.send(Frame::Open { conn }).await;
        let mut handshake = secure::initiator(&phone.private).unwrap();
        let mut buf = [0u8; 65535];

        let n = handshake.write_message(&[], &mut buf).unwrap();
        link.send(Frame::Data {
            conn,
            payload: Bytes::copy_from_slice(&buf[..n]),
        })
        .await;

        let second = link.data_for(conn).await;
        handshake.read_message(&second, &mut buf).unwrap();

        let n = handshake.write_message(&[], &mut buf).unwrap();
        link.send(Frame::Data {
            conn,
            payload: Bytes::copy_from_slice(&buf[..n]),
        })
        .await;

        Self {
            conn,
            channel: SecureChannel::new(handshake.into_transport_mode().unwrap()),
        }
    }

    async fn request(&mut self, link: &mut HostLink, id: &str, op: &str, args: Value) {
        let frame = json!({ "id": id, "op": op, "args": args }).to_string();
        let payload = self.channel.encrypt_frame(frame.as_bytes()).unwrap();
        link.send(Frame::Data {
            conn: self.conn,
            payload: Bytes::from(payload),
        })
        .await;
    }

    async fn next_json(&mut self, link: &mut HostLink) -> Value {
        let payload = link.data_for(self.conn).await;
        let plaintext = self.channel.decrypt_frame(&payload).unwrap();
        serde_json::from_slice(&plaintext).unwrap()
    }
}

// ---------------------------------------------------------------------------
// A host with a relay configured
// ---------------------------------------------------------------------------

struct Host {
    _dir: tempfile::TempDir,
    state: Arc<RemoteState>,
}

/// A started host pointed at `url`, with the relay's waits shortened.
fn boot(url: &str) -> Host {
    let dir = tempfile::tempdir().unwrap();
    let state = RemoteState::new(dir.path(), Arc::new(StubDispatch));
    state.set_relay_timing(fast());
    state.set_relay(Some(url.to_string())).unwrap();
    state.start(0).unwrap();
    Host { _dir: dir, state }
}

/// Poll `status()` until `check` holds. The link is a task, so everything it
/// publishes lands a moment after the call that caused it.
async fn until(state: &Arc<RemoteState>, what: &str, check: impl Fn(&super::RemoteStatus) -> bool) {
    for _ in 0..500 {
        if check(&state.status()) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{what} never happened: {:?}", state.status().relay);
}

// ---------------------------------------------------------------------------
// Authentication
// ---------------------------------------------------------------------------

/// The known-answer vector for the host link challenge, from fixed keys, so the
/// relay implementation can be checked against exactly these bytes:
///
/// - host private  `AQEB…` (32 × 0x01)
/// - relay private `AgIC…` (32 × 0x02)
/// - nonce         `AwMD…` (32 × 0x03)
///
/// `proof = base64url( SHA-256( X25519(hostPrivate, relayPublic) || nonce || hostPublic ) )`.
///
/// The relay's own published vector (`relay/test-vector.json`, produced with
/// Node's crypto and verified in workerd) must come out of this implementation
/// byte for byte — the one place the two codebases are checked against the
/// same numbers rather than each against itself. The fixed-key test below it
/// is the desktop's own vector, printed for comparison.
#[test]
fn the_challenge_proof_matches_the_relays_published_vector() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../relay/test-vector.json");
    let raw = std::fs::read_to_string(path).expect("relay/test-vector.json is in the repo");
    let vector: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let field = |name: &str| -> [u8; 32] {
        B64.decode(vector[name].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap_or_else(|_| panic!("{name} is not 32 bytes"))
    };

    let host = HostKey::from_private(field("hostPrivate")).unwrap();
    assert_eq!(*host.public_bytes(), field("hostId"), "host public key");
    let relay_key = field("relayKey");
    assert_eq!(
        host.diffie_hellman(&relay_key).unwrap(),
        field("shared"),
        "X25519 shared secret"
    );
    let proof = relay::challenge_proof(&host, &relay_key, &field("nonce")).unwrap();
    assert_eq!(proof, field("proof"), "proof");
}

#[test]
fn the_challenge_proof_matches_a_known_answer_vector() {
    let host = HostKey::from_private([0x01; 32]).unwrap();
    let relay = HostKey::from_private([0x02; 32]).unwrap();
    let nonce = [0x03u8; 32];

    // Both ends must reach the same secret from opposite sides, which is the
    // whole point of the challenge: the relay verifies with its own private key
    // against the host ID it is routing on.
    let from_host = host.diffie_hellman(relay.public_bytes()).unwrap();
    let from_relay = relay.diffie_hellman(host.public_bytes()).unwrap();
    assert_eq!(from_host, from_relay, "X25519 is not symmetric?");

    let proof = relay::challenge_proof(&host, relay.public_bytes(), &nonce).unwrap();
    println!("host private:  {}", B64.encode([0x01u8; 32]));
    println!("host public:   {}", B64.encode(host.public_bytes()));
    println!("relay private: {}", B64.encode([0x02u8; 32]));
    println!("relay public:  {}", B64.encode(relay.public_bytes()));
    println!("nonce:         {}", B64.encode(nonce));
    println!("shared:        {}", B64.encode(from_host));
    println!("proof:         {}", B64.encode(proof));

    assert_eq!(
        B64.encode(host.public_bytes()),
        "pOCSkrZRwni5dyxWn1-puxPZBrRqtoyd-dwrRAn4ogk"
    );
    assert_eq!(
        B64.encode(relay.public_bytes()),
        "zo060cy2M-x7cMF4FKXHbs0CloUFDTRHRboFhw5YfVk"
    );
    assert_eq!(
        B64.encode(from_host),
        "LtdqtUmx5zwDHrSclEjweYrqgbaYJ5oMPcPkn7_EuVM"
    );
    assert_eq!(
        B64.encode(proof),
        "BzDo2Jh0uedvZ-WrLiP4wkMcCFhvU9L9REMa8_die2U"
    );
}

#[test]
fn the_host_endpoint_is_the_documented_one() {
    assert_eq!(
        relay::host_endpoint("wss://relay.fletch.app", "abc"),
        "wss://relay.fletch.app/v1/host/abc"
    );
    assert_eq!(
        relay::host_endpoint("wss://relay.fletch.app/", "abc"),
        "wss://relay.fletch.app/v1/host/abc"
    );
}

/// The link is only up once the relay has verified the proof, and what it
/// verified against is the host ID in the path.
#[tokio::test]
async fn the_link_authenticates_with_the_host_key_and_reports_connected() {
    let mut relay = FakeRelay::start(Verdict::Accept).await;
    let host = boot(&relay.url);
    let link = relay.next_link().await;

    assert_eq!(
        link.path,
        format!("/v1/host/{}", host.state.status().host_id)
    );
    until(&host.state, "the relay link connects", |s| {
        s.relay.state == RelayState::Connected
    })
    .await;
    let status = host.state.status();
    assert_eq!(status.relay.url.as_deref(), Some(relay.url.as_str()));
    assert_eq!(status.relay.error, None);
}

/// A relay that will not take the proof is a standing error the pane can show,
/// and the host keeps trying: the operator may be fixing the relay.
#[tokio::test]
async fn a_rejected_proof_becomes_an_error_and_the_host_retries() {
    let relay = FakeRelay::start(Verdict::Reject).await;
    let host = boot(&relay.url);

    until(&host.state, "the rejection surfaces", |s| {
        s.relay.state == RelayState::Error
            && s.relay
                .error
                .as_deref()
                .is_some_and(|e| e.contains("rejected"))
    })
    .await;
    // The URL stays on the status while the link is between attempts.
    assert_eq!(
        host.state.status().relay.url.as_deref(),
        Some(relay.url.as_str())
    );

    for _ in 0..200 {
        if relay.attempts.load(Ordering::SeqCst) >= 2 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the host stopped after one rejected attempt");
}

/// The link dropping is not the end of it either.
#[tokio::test]
async fn a_dropped_link_reconnects() {
    let mut relay = FakeRelay::start(Verdict::Accept).await;
    let host = boot(&relay.url);
    let mut first = relay.next_link().await;
    until(&host.state, "the first link connects", |s| {
        s.relay.state == RelayState::Connected
    })
    .await;

    // The relay hangs up, as a Durable Object being evicted would.
    let _ = first.ws.close(None).await;
    drop(first);

    let second = relay.next_link().await;
    assert!(second.path.starts_with("/v1/host/"));
    until(&host.state, "the link comes back", |s| {
        s.relay.state == RelayState::Connected
    })
    .await;
}

// ---------------------------------------------------------------------------
// Virtual connections
// ---------------------------------------------------------------------------

/// The whole point: a device that arrives over the relay is served by the same
/// state machine a LAN socket is, Noise handshake and all.
#[tokio::test]
async fn a_relayed_device_handshakes_and_says_hello() {
    let mut relay = FakeRelay::start(Verdict::Accept).await;
    let host = boot(&relay.url);
    let mut link = relay.next_link().await;

    let phone = device();
    host.state
        .devices()
        .register("phone", "ios", &phone.public)
        .unwrap();

    let mut conn = Phone::open(&mut link, 1, &phone).await;
    conn.request(&mut link, "1", "hello", json!({})).await;
    let hello = conn.next_json(&mut link).await;
    assert_eq!(hello["id"], "1");
    assert_eq!(hello["ok"], true);
    assert!(hello["result"]["workspace"].is_object());

    // And it is a session like any other: visible as connected, and on the
    // event fan-out.
    until(&host.state, "the relayed device counts as connected", |s| {
        s.devices.first().is_some_and(|d| d.connected)
    })
    .await;
    host.state
        .forward_event("agent:status", r#"{"agentId":"arabia","status":"running"}"#);
    let event = conn.next_json(&mut link).await;
    assert_eq!(event["event"], "agent:status");

    conn.request(&mut link, "2", "get_workspace", json!({}))
        .await;
    let reply = conn.next_json(&mut link).await;
    assert_eq!(reply["id"], "2");
    assert_eq!(reply["result"]["agents"], json!([]));
}

#[tokio::test]
async fn two_relayed_devices_are_served_independently() {
    let mut relay = FakeRelay::start(Verdict::Accept).await;
    let host = boot(&relay.url);
    let mut link = relay.next_link().await;

    let (one, two) = (device(), device());
    host.state
        .devices()
        .register("phone", "ios", &one.public)
        .unwrap();
    host.state
        .devices()
        .register("tablet", "android", &two.public)
        .unwrap();

    // Interleaved on purpose: the two handshakes share one link, and a mux that
    // mixed the connections up would produce a Noise failure rather than a
    // wrong answer.
    let mut first = Phone::open(&mut link, 1, &one).await;
    let mut second = Phone::open(&mut link, 2, &two).await;
    first.request(&mut link, "a", "hello", json!({})).await;
    second.request(&mut link, "b", "hello", json!({})).await;
    assert_eq!(second.next_json(&mut link).await["id"], "b");
    assert_eq!(first.next_json(&mut link).await["id"], "a");

    first
        .request(&mut link, "a2", "get_workspace", json!({}))
        .await;
    second
        .request(&mut link, "b2", "get_workspace", json!({}))
        .await;
    assert_eq!(first.next_json(&mut link).await["id"], "a2");
    assert_eq!(second.next_json(&mut link).await["id"], "b2");
}

/// Revoking reaches through the relay exactly as it reaches a socket, and it
/// reaches only the revoked device's connection.
#[tokio::test]
async fn revoking_closes_only_that_virtual_connection() {
    let mut relay = FakeRelay::start(Verdict::Accept).await;
    let host = boot(&relay.url);
    let mut link = relay.next_link().await;

    let (one, two) = (device(), device());
    let doomed = host
        .state
        .devices()
        .register("phone", "ios", &one.public)
        .unwrap();
    host.state
        .devices()
        .register("tablet", "android", &two.public)
        .unwrap();

    let mut first = Phone::open(&mut link, 1, &one).await;
    let mut second = Phone::open(&mut link, 2, &two).await;
    first.request(&mut link, "a", "hello", json!({})).await;
    assert_eq!(first.next_json(&mut link).await["ok"], true);
    second.request(&mut link, "b", "hello", json!({})).await;
    assert_eq!(second.next_json(&mut link).await["ok"], true);

    host.state.revoke_device(&doomed.device_id).unwrap();
    assert_eq!(
        link.close_for(1).await,
        (4003, "device revoked".to_string())
    );

    second
        .request(&mut link, "b2", "get_workspace", json!({}))
        .await;
    assert_eq!(second.next_json(&mut link).await["id"], "b2");
}

/// Turning remote access off sends the host's own 4004 out over every virtual
/// connection *before* the link goes, which is what lets the phone tell "the
/// Mac said no" from "the relay lost the Mac".
#[tokio::test]
async fn stopping_closes_every_virtual_connection_with_4004_then_drops_the_link() {
    let mut relay = FakeRelay::start(Verdict::Accept).await;
    let host = boot(&relay.url);
    let mut link = relay.next_link().await;

    let (one, two) = (device(), device());
    for (name, phone) in [("phone", &one), ("tablet", &two)] {
        host.state
            .devices()
            .register(name, "ios", &phone.public)
            .unwrap();
    }
    let mut first = Phone::open(&mut link, 1, &one).await;
    let mut second = Phone::open(&mut link, 2, &two).await;
    first.request(&mut link, "a", "hello", json!({})).await;
    assert_eq!(first.next_json(&mut link).await["ok"], true);
    second.request(&mut link, "b", "hello", json!({})).await;
    assert_eq!(second.next_json(&mut link).await["ok"], true);

    host.state.stop();

    // Both closes land, then the link itself ends — `drain` returns when the
    // relay's read side sees the close.
    let frames = link.drain().await;
    let closes: Vec<(u32, u16)> = frames
        .iter()
        .filter_map(|f| match f {
            Frame::Close { conn, code, .. } => Some((*conn, *code)),
            _ => None,
        })
        .collect();
    assert!(
        closes.contains(&(1, 4004)) && closes.contains(&(2, 4004)),
        "expected a 4004 for each virtual connection, got {frames:?}"
    );
    assert_eq!(
        host.state.status().relay.state,
        RelayState::Off,
        "with remote access off there is no link to report on"
    );
}

/// A device link that drops arrives as CLOSE 1006 from the relay. The session
/// ends with it — nothing on the host waits for a socket that is gone.
#[tokio::test]
async fn a_relay_close_ends_the_session() {
    let mut relay = FakeRelay::start(Verdict::Accept).await;
    let host = boot(&relay.url);
    let mut link = relay.next_link().await;

    let phone = device();
    host.state
        .devices()
        .register("phone", "ios", &phone.public)
        .unwrap();
    let mut conn = Phone::open(&mut link, 1, &phone).await;
    conn.request(&mut link, "1", "hello", json!({})).await;
    assert_eq!(conn.next_json(&mut link).await["ok"], true);
    until(&host.state, "the device registers as connected", |s| {
        s.devices.first().is_some_and(|d| d.connected)
    })
    .await;

    link.send(Frame::Close {
        conn: 1,
        code: 1006,
        reason: "device link dropped".to_string(),
    })
    .await;

    until(&host.state, "the session ends", |s| {
        s.devices.iter().all(|d| !d.connected)
    })
    .await;
    // The relay closed it, so the host does not answer with a close of its own.
    assert_eq!(
        host.state.status().relay.state,
        RelayState::Connected,
        "one device going away is not the link going away"
    );
}

/// DATA (or CLOSE) for a connection the host has finished with is ignored, not
/// fatal: the relay may still be forwarding a device's last message.
#[tokio::test]
async fn frames_for_an_unknown_connection_are_ignored() {
    let mut relay = FakeRelay::start(Verdict::Accept).await;
    let host = boot(&relay.url);
    let mut link = relay.next_link().await;

    link.send(Frame::Data {
        conn: 99,
        payload: Bytes::from_static(b"who is this for"),
    })
    .await;
    link.send(Frame::Close {
        conn: 98,
        code: 1006,
        reason: "gone".to_string(),
    })
    .await;

    // The link still works, which is the whole assertion.
    let phone = device();
    host.state
        .devices()
        .register("phone", "ios", &phone.public)
        .unwrap();
    let mut conn = Phone::open(&mut link, 1, &phone).await;
    conn.request(&mut link, "1", "hello", json!({})).await;
    assert_eq!(conn.next_json(&mut link).await["ok"], true);
    assert_eq!(host.state.status().relay.state, RelayState::Connected);
}

// ---------------------------------------------------------------------------
// Push notifications
// ---------------------------------------------------------------------------

/// A NOTIFY reaches the relay as `type 0x05` on connId 0, with the JSON body
/// verbatim — checked on the raw bytes, since those are what the relay's own
/// decoder reads.
#[tokio::test]
async fn a_notify_travels_as_type_0x05_on_connid_zero() {
    let mut relay = FakeRelay::start(Verdict::Accept).await;
    let host = boot(&relay.url);
    let mut link = relay.next_link().await;
    until(&host.state, "the relay link connects", |s| {
        s.relay.state == RelayState::Connected
    })
    .await;

    let payload = json!({
        "tokens": [{ "token": "a1b2", "environment": "sandbox" }],
        "title": "Turn complete",
        "body": "Fix login crash",
        "kind": "turn_complete",
        "agentId": "arabia",
        "collapseId": "arabia",
    })
    .to_string();
    assert!(host.state.send_notify(payload.clone()));

    let bytes = link.read_raw().await.expect("a frame on the link");
    assert_eq!(bytes[0], 0x05, "NOTIFY is type 0x05");
    assert_eq!(&bytes[1..5], &[0, 0, 0, 0], "NOTIFY belongs to connId 0");
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes[5..]).unwrap(),
        serde_json::from_str::<Value>(&payload).unwrap()
    );

    // Fire and forget: the relay answers nothing, and the link carries on. A
    // device attaching afterwards is served as if the NOTIFY had not happened.
    let phone = device();
    host.state
        .devices()
        .register("phone", "ios", &phone.public)
        .unwrap();
    let mut conn = Phone::open(&mut link, 1, &phone).await;
    conn.request(&mut link, "1", "hello", json!({})).await;
    assert_eq!(conn.next_json(&mut link).await["ok"], true);
    assert_eq!(host.state.status().relay.state, RelayState::Connected);
}

/// A NOTIFY frame the relay sends back is not part of the contract (it is host
/// to relay only). Dropped, not fatal — a later relay revision must not be able
/// to knock this host off its own link.
#[tokio::test]
async fn a_notify_from_the_relay_is_ignored() {
    let mut relay = FakeRelay::start(Verdict::Accept).await;
    let host = boot(&relay.url);
    let mut link = relay.next_link().await;

    link.send(Frame::Notify {
        payload: Bytes::from_static(br#"{"kind":"turn_complete"}"#),
    })
    .await;

    let phone = device();
    host.state
        .devices()
        .register("phone", "ios", &phone.public)
        .unwrap();
    let mut conn = Phone::open(&mut link, 1, &phone).await;
    conn.request(&mut link, "1", "hello", json!({})).await;
    assert_eq!(conn.next_json(&mut link).await["ok"], true);
    assert_eq!(host.state.status().relay.state, RelayState::Connected);
}

/// With no link there is nothing to queue on, and saying so is the whole answer
/// — a dropped alert is by contract.
#[tokio::test]
async fn a_notify_without_a_link_is_refused_not_queued() {
    let dir = tempfile::tempdir().unwrap();
    let state = RemoteState::new(dir.path(), Arc::new(StubDispatch));
    assert!(!state.send_notify("{}".to_string()), "no relay configured");

    // Configured but never reachable: the link is between attempts, so still
    // nothing to queue on.
    state.set_relay_timing(fast());
    state
        .set_relay(Some("ws://127.0.0.1:1".to_string()))
        .unwrap();
    state.start(0).unwrap();
    until(&state, "the dial fails", |s| {
        s.relay.state == RelayState::Error
    })
    .await;
    assert!(!state.send_notify("{}".to_string()));
}

/// The link going away takes the outbound queue with it: an alert must not be
/// enqueued onto a link nothing is writing any more.
#[tokio::test]
async fn a_dropped_link_stops_accepting_notifies() {
    let mut relay = FakeRelay::start(Verdict::Accept).await;
    let host = boot(&relay.url);
    let mut first = relay.next_link().await;
    until(&host.state, "the first link connects", |s| {
        s.relay.state == RelayState::Connected
    })
    .await;
    assert!(host.state.send_notify("{}".to_string()));

    let _ = first.ws.close(None).await;
    drop(first);
    until(&host.state, "the link goes down", |s| {
        s.relay.state != RelayState::Connected
    })
    .await;
    assert!(!host.state.send_notify("{}".to_string()));

    // And it works again on the reconnect.
    let mut second = relay.next_link().await;
    until(&host.state, "the link comes back", |s| {
        s.relay.state == RelayState::Connected
    })
    .await;
    assert!(host.state.send_notify("{}".to_string()));
    assert_eq!(
        second.read_raw().await.expect("a frame on the new link")[0],
        0x05
    );
}

// ---------------------------------------------------------------------------
// The setting
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_relay_url_is_normalized_and_validated() {
    let dir = tempfile::tempdir().unwrap();
    let state = RemoteState::new(dir.path(), Arc::new(StubDispatch));

    // Absent, empty and whitespace all mean "no relay", and the status says so
    // without a link ever starting.
    for url in [None, Some(String::new()), Some("   ".to_string())] {
        state.set_relay(url).unwrap();
        let relay = state.status().relay;
        assert_eq!(relay.url, None);
        assert_eq!(relay.state, RelayState::Off);
    }

    // The scheme is the one thing worth refusing: a typo here is a link that
    // could never connect, and the pane can say so at once.
    for bad in ["relay.fletch.app", "https://relay.fletch.app", "wss:/x"] {
        assert!(
            state.set_relay(Some(bad.to_string())).is_err(),
            "{bad} should be refused"
        );
    }

    state
        .set_relay(Some(format!("  {DEFAULT_RELAY_URL}/  ")))
        .unwrap();
    assert_eq!(
        state.status().relay.url.as_deref(),
        Some(DEFAULT_RELAY_URL),
        "trimmed, with the trailing slash off"
    );
    assert_eq!(
        state.status().relay.state,
        RelayState::Off,
        "a URL alone starts nothing: remote access is still off"
    );

    // And the pairing link carries it, url-encoded, only once it is set.
    assert!(state.begin_pairing().url.contains(&format!(
        "&relay={}",
        DEFAULT_RELAY_URL.replace(':', "%3A").replace('/', "%2F")
    )));
    state.set_relay(None).unwrap();
    assert!(!state.begin_pairing().url.contains("relay="));
}
