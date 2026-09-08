//! The host link: one outbound WebSocket to the relay, carrying every off-LAN
//! device as a numbered virtual connection.
//!
//! The contract is `docs/remote-protocol.md` → "Relay". The relay is a dumb
//! pipe that routes on the host ID and sees only the ciphertext the secure
//! channel already produces, so nothing here touches the protocol above the
//! transport: a virtual connection is handed to the same `server::serve` a LAN
//! socket is, and that state machine cannot tell the difference.
//!
//! Three pieces:
//!
//! - [`Frame`], the multiplexing codec (`type || connId || payload`).
//! - [`VirtualConn`], a `server::WsTransport` whose inbound side is fed by the
//!   demux and whose outbound side writes DATA/CLOSE frames onto the link's
//!   shared queue.
//! - [`RelayLink`], the task that dials, proves possession of the host key,
//!   demultiplexes, and reconnects with backoff.

use std::collections::HashMap;
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use futures_util::stream::SplitSink;
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinSet;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tokio_tungstenite::tungstenite::{Bytes, Error as WsError, Message};
use tracing::Instrument;

use super::secure::HostKey;
use super::server;
use super::RemoteState;

// ---------------------------------------------------------------------------
// The multiplexing codec
// ---------------------------------------------------------------------------

const TYPE_OPEN: u8 = 0x01;
const TYPE_DATA: u8 = 0x02;
const TYPE_CLOSE: u8 = 0x03;
const TYPE_TEXT: u8 = 0x04;
const TYPE_NOTIFY: u8 = 0x05;

/// The `connId` a NOTIFY frame carries. It belongs to no virtual connection —
/// the doc fixes it at 0 so the header stays one shape for every frame type.
const NOTIFY_CONN: u32 = 0;

/// Length of the fixed header: `type (1) || connId (u32 big-endian)`.
const HEADER_LEN: usize = 5;

/// One frame on the host link. `OPEN` and `TEXT` only ever arrive; `DATA` and
/// `CLOSE` travel both ways.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Frame {
    /// A device link attached; payload is empty.
    Open { conn: u32 },
    /// One device WebSocket binary message, verbatim.
    Data { conn: u32, payload: Bytes },
    /// The virtual connection is over: `code (u16 big-endian) || reason`.
    Close {
        conn: u32,
        code: u16,
        reason: String,
    },
    /// One device WebSocket *text* message, verbatim, so the host can apply its
    /// own 4001 rule to it rather than the relay guessing.
    Text { conn: u32, text: String },
    /// A push request for the relay itself to forward to APNs: UTF-8 JSON on
    /// `connId` 0. Host → relay only, and fire and forget — the relay sends no
    /// result frame, and one that does not know this type ignores it.
    Notify { payload: Bytes },
}

impl Frame {
    pub(super) fn conn(&self) -> u32 {
        match self {
            Frame::Open { conn }
            | Frame::Data { conn, .. }
            | Frame::Close { conn, .. }
            | Frame::Text { conn, .. } => *conn,
            Frame::Notify { .. } => NOTIFY_CONN,
        }
    }

    pub(super) fn encode(&self) -> Bytes {
        let mut out = Vec::with_capacity(HEADER_LEN + 32);
        out.push(match self {
            Frame::Open { .. } => TYPE_OPEN,
            Frame::Data { .. } => TYPE_DATA,
            Frame::Close { .. } => TYPE_CLOSE,
            Frame::Text { .. } => TYPE_TEXT,
            Frame::Notify { .. } => TYPE_NOTIFY,
        });
        out.extend_from_slice(&self.conn().to_be_bytes());
        match self {
            Frame::Open { .. } => {}
            Frame::Data { payload, .. } => out.extend_from_slice(payload),
            Frame::Close { code, reason, .. } => {
                out.extend_from_slice(&code.to_be_bytes());
                out.extend_from_slice(reason.as_bytes());
            }
            Frame::Text { text, .. } => out.extend_from_slice(text.as_bytes()),
            Frame::Notify { payload } => out.extend_from_slice(payload),
        }
        Bytes::from(out)
    }

    /// Decode one frame. `Err` carries what was wrong with it, for the log; a
    /// frame the host cannot parse is dropped, not fatal, since the relay is
    /// free to add frame types the host does not know yet.
    pub(super) fn decode(bytes: &[u8]) -> std::result::Result<Self, String> {
        if bytes.len() < HEADER_LEN {
            return Err(format!("frame of {} bytes has no header", bytes.len()));
        }
        let conn = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
        let body = &bytes[HEADER_LEN..];
        match bytes[0] {
            TYPE_OPEN => Ok(Frame::Open { conn }),
            TYPE_DATA => Ok(Frame::Data {
                conn,
                payload: Bytes::copy_from_slice(body),
            }),
            TYPE_CLOSE => {
                if body.len() < 2 {
                    return Err("close frame without a code".to_string());
                }
                Ok(Frame::Close {
                    conn,
                    code: u16::from_be_bytes([body[0], body[1]]),
                    reason: String::from_utf8_lossy(&body[2..]).into_owned(),
                })
            }
            TYPE_TEXT => Ok(Frame::Text {
                conn,
                text: String::from_utf8_lossy(body).into_owned(),
            }),
            // Decoded for the round-trip tests and for symmetry; the host never
            // receives one (see `route`). `conn` is not checked against 0: a
            // frame this side only ever writes cannot arrive with another value
            // unless the relay invented it, and dropping it in `route` is
            // already the answer to that.
            TYPE_NOTIFY => Ok(Frame::Notify {
                payload: Bytes::copy_from_slice(body),
            }),
            other => Err(format!("unknown frame type {other:#04x}")),
        }
    }

    fn into_message(self) -> Message {
        Message::Binary(self.encode())
    }
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// What `remote_status.relay.state` reports. `off` covers both "no URL set" and
/// "remote access is disabled"; `error` stands between reconnect attempts and
/// carries the last failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RelayState {
    Off,
    Connecting,
    Connected,
    Error,
}

/// `remote_status.relay`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayStatus {
    /// The configured base URL, reported whether or not the link is running.
    pub url: Option<String>,
    pub state: RelayState,
    pub error: Option<String>,
}

impl RelayStatus {
    /// No URL configured, or remote access is off.
    pub(super) fn off(url: Option<String>) -> Self {
        Self {
            url,
            state: RelayState::Off,
            error: None,
        }
    }
}

/// The link task's published state, read by `remote_status`.
struct Published {
    state: RelayState,
    error: Option<String>,
}

impl Published {
    fn set(&mut self, state: RelayState, error: Option<String>) {
        self.state = state;
        self.error = error;
    }
}

// ---------------------------------------------------------------------------
// Timing
// ---------------------------------------------------------------------------

/// Everything the link waits on, in one place so tests do not sleep.
#[derive(Debug, Clone, Copy)]
pub(super) struct RelayTiming {
    /// First reconnect delay, doubled per failure up to `backoff_max` and reset
    /// by a successful `ready`.
    pub backoff_initial: Duration,
    pub backoff_max: Duration,
    /// How long the relay has to complete challenge → proof → ready.
    pub auth_timeout: Duration,
    /// Host↔relay keepalive. This hop has its own ping/pong, never forwarded.
    pub ping_interval: Duration,
    /// How long a link that has been asked to go away keeps writing, so the
    /// 4004 its virtual connections just queued actually reaches the relay.
    pub shutdown_grace: Duration,
}

impl Default for RelayTiming {
    fn default() -> Self {
        Self {
            backoff_initial: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            auth_timeout: Duration::from_secs(10),
            ping_interval: Duration::from_secs(20),
            shutdown_grace: Duration::from_secs(2),
        }
    }
}

/// Two missed pongs drop the link, as on the LAN path.
const MAX_MISSED_PONGS: u32 = 2;
/// Frames queued for the relay across every virtual connection. Each virtual
/// connection already has its own 64-frame outbox inside `serve`; this is the
/// one hop after it, so it only fills when the relay itself stops reading.
const LINK_OUTBOUND_BUFFER: usize = 256;
/// Messages queued *into* one virtual connection. `serve` reads continuously
/// and dispatches off its reader loop, so this only fills for a device that is
/// flooding, which costs that device its link and nothing else.
const CONN_INBOUND_BUFFER: usize = 64;
/// The device message cap (4 MiB, protocol doc) plus room for the mux header.
const MAX_LINK_MESSAGE: usize = 4 * 1024 * 1024 + 1024;

// ---------------------------------------------------------------------------
// The link handle
// ---------------------------------------------------------------------------

/// A running host link. Dropping it asks the task to wind down — it does not
/// abort, so the CLOSE frames the virtual connections queued on the way out
/// still reach the relay (see `RemoteState::stop`).
pub(super) struct RelayLink {
    published: Arc<Mutex<Published>>,
    /// The live link's outbound queue, `Some` only while the link is connected.
    /// The demux publishes it here so code that is not inside the read loop
    /// (the push triggers) can enqueue a frame; everything else reaches the
    /// queue through a `VirtualConn`, which has its own clone.
    outbound: Outgoing,
    shutdown: broadcast::Sender<()>,
}

/// The slot [`RelayLink::outbound`] lives in, shared with the link task.
type Outgoing = Arc<Mutex<Option<mpsc::Sender<Message>>>>;

impl RelayLink {
    /// Dial `url` and keep the link up until this handle is dropped.
    ///
    /// Must be called from inside the async runtime. The task holds an
    /// `Arc<RemoteState>`, so the state outlives the link rather than the other
    /// way round; every path that stops caring about the link drops this handle,
    /// which is what lets the task (and that `Arc`) go.
    pub(super) fn spawn(state: Arc<RemoteState>, url: String, timing: RelayTiming) -> Self {
        let published = Arc::new(Mutex::new(Published {
            state: RelayState::Connecting,
            error: None,
        }));
        let outbound: Outgoing = Arc::new(Mutex::new(None));
        let (shutdown, stop) = broadcast::channel(1);
        tokio::spawn(run(
            state,
            url,
            timing,
            published.clone(),
            outbound.clone(),
            stop,
        ));
        Self {
            published,
            outbound,
            shutdown,
        }
    }

    pub(super) fn snapshot(&self) -> (RelayState, Option<String>) {
        let published = self.published.lock();
        (published.state, published.error.clone())
    }

    /// Queue one NOTIFY frame for the relay. `false` when the link is not
    /// connected, or when its queue is full because the relay stopped reading.
    ///
    /// Never blocks and never waits: a push alert is worth less than the link
    /// it would travel on, so a dropped one is by contract (the phone will see
    /// the state when it next connects) and the caller logs it at debug.
    pub(super) fn send_notify(&self, payload: String) -> bool {
        let Some(tx) = self.outbound.lock().clone() else {
            return false;
        };
        tx.try_send(
            Frame::Notify {
                payload: Bytes::from(payload),
            }
            .into_message(),
        )
        .is_ok()
    }
}

impl Drop for RelayLink {
    fn drop(&mut self) {
        let _ = self.shutdown.send(());
    }
}

/// `<base>/v1/host/<hostId>`, per the doc's endpoint table.
pub(super) fn host_endpoint(base: &str, host_id: &str) -> String {
    format!("{}/v1/host/{host_id}", base.trim_end_matches('/'))
}

/// Reconnect forever: dial, authenticate, demultiplex, back off, repeat.
async fn run(
    state: Arc<RemoteState>,
    url: String,
    timing: RelayTiming,
    published: Arc<Mutex<Published>>,
    outbound: Outgoing,
    mut shutdown: broadcast::Receiver<()>,
) {
    let mut backoff = timing.backoff_initial;
    loop {
        published.lock().set(RelayState::Connecting, None);
        match attempt(&state, &url, &timing, &published, &outbound, &mut shutdown).await {
            Outcome::Done => {
                published.lock().set(RelayState::Off, None);
                return;
            }
            Outcome::Retry { error, connected } => {
                tracing::info!(%url, %error, "remote: relay link down, retrying");
                published.lock().set(RelayState::Error, Some(error));
                // A link that got as far as `ready` proved the URL and the key
                // are good, so the next failure starts from the short delay.
                if connected {
                    backoff = timing.backoff_initial;
                }
                tokio::select! {
                    _ = shutdown.recv() => {
                        published.lock().set(RelayState::Off, None);
                        return;
                    }
                    _ = tokio::time::sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(timing.backoff_max);
            }
        }
    }
}

enum Outcome {
    /// The handle was dropped: stop reconnecting.
    Done,
    /// Reconnect. `connected` records whether this attempt ever reached
    /// `ready`, which is what resets the backoff.
    Retry { error: String, connected: bool },
}

fn retry(error: impl Into<String>) -> Outcome {
    Outcome::Retry {
        error: error.into(),
        connected: false,
    }
}

/// One connection attempt, from the dial to the link going away.
async fn attempt(
    state: &Arc<RemoteState>,
    url: &str,
    timing: &RelayTiming,
    published: &Arc<Mutex<Published>>,
    outbound: &Outgoing,
    shutdown: &mut broadcast::Receiver<()>,
) -> Outcome {
    let Some(host_id) = state.host_key().map(HostKey::host_id) else {
        // No identity: nothing to route on and nothing to prove. The same
        // condition already stands in `RemoteStatus::error`.
        return retry("this host has no key, so it cannot attach to a relay");
    };
    let endpoint = host_endpoint(url, &host_id);

    let mut config = WebSocketConfig::default();
    config.max_message_size = Some(MAX_LINK_MESSAGE);
    config.max_frame_size = Some(MAX_LINK_MESSAGE);
    let dial = tokio_tungstenite::connect_async_with_config(&endpoint, Some(config), true);
    let mut ws = tokio::select! {
        _ = shutdown.recv() => return Outcome::Done,
        dialed = dial => match dialed {
            Ok((ws, _)) => ws,
            Err(e) => return retry(format!("{url} could not be reached: {e}")),
        },
    };

    let authenticated = tokio::select! {
        _ = shutdown.recv() => return Outcome::Done,
        outcome = tokio::time::timeout(timing.auth_timeout, authenticate(&mut ws, state)) => outcome,
    };
    match authenticated {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return retry(e),
        Err(_) => return retry("the relay did not finish the challenge in time"),
    }

    tracing::info!(%endpoint, "remote: relay link up");
    published.lock().set(RelayState::Connected, None);
    match demux(state, ws, timing, outbound, shutdown).await {
        Outcome::Done => Outcome::Done,
        Outcome::Retry { error, .. } => Outcome::Retry {
            error,
            connected: true,
        },
    }
}

// ---------------------------------------------------------------------------
// Host link authentication
// ---------------------------------------------------------------------------

/// What the relay says before the link carries frames.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum RelayHello {
    Challenge {
        nonce: String,
        #[serde(rename = "relayKey")]
        relay_key: String,
    },
    Ready,
}

/// Prove possession of the host key: read the challenge, answer with
/// `SHA-256(shared || nonce || hostKey)`, and wait for `ready`.
///
/// `shared` is `X25519(host private, relayKey)`, so a relay that does not hold
/// the private half of `relayKey` cannot verify the proof and one that does
/// still learns nothing it could replay against another relay — the host public
/// key is hashed in, and it is the ID the relay routes on anyway.
async fn authenticate<S>(ws: &mut S, state: &Arc<RemoteState>) -> std::result::Result<(), String>
where
    S: Stream<Item = std::result::Result<Message, WsError>>
        + Sink<Message, Error = WsError>
        + Unpin,
{
    let (nonce, relay_key) = match next_text(ws).await? {
        RelayHello::Challenge { nonce, relay_key } => (nonce, relay_key),
        RelayHello::Ready => return Err("the relay sent ready before a challenge".to_string()),
    };
    let nonce = B64
        .decode(nonce)
        .map_err(|e| format!("the relay's nonce is not base64url: {e}"))?;
    let relay_key: [u8; 32] = B64
        .decode(relay_key)
        .map_err(|e| format!("the relay's key is not base64url: {e}"))?
        .try_into()
        .map_err(|_| "the relay's key is not 32 bytes".to_string())?;

    let host = state
        .host_key()
        .ok_or_else(|| "this host has no key".to_string())?;
    let proof = challenge_proof(host, &relay_key, &nonce).map_err(|e| e.to_string())?;
    let answer = format!("{{\"type\":\"proof\",\"proof\":\"{}\"}}", B64.encode(proof));
    ws.send(Message::Text(answer.into()))
        .await
        .map_err(|e| format!("the proof could not be sent: {e}"))?;

    match next_text(ws).await? {
        RelayHello::Ready => Ok(()),
        RelayHello::Challenge { .. } => Err("the relay challenged twice".to_string()),
    }
}

/// `SHA-256( X25519(hostPrivate, relayKey) || nonce || hostKey )`, raw bytes
/// throughout (`docs/remote-protocol.md` → "Relay").
pub(super) fn challenge_proof(
    host: &HostKey,
    relay_key: &[u8; 32],
    nonce: &[u8],
) -> crate::error::Result<[u8; 32]> {
    let shared = host.diffie_hellman(relay_key)?;
    let mut digest = Sha256::new();
    digest.update(shared);
    digest.update(nonce);
    digest.update(host.public_bytes());
    Ok(digest.finalize().into())
}

/// The next JSON text frame of the authentication exchange. Ping/pong are the
/// transport's business and can interleave; anything else ends the attempt.
async fn next_text<S>(ws: &mut S) -> std::result::Result<RelayHello, String>
where
    S: Stream<Item = std::result::Result<Message, WsError>> + Unpin,
{
    loop {
        let msg = ws
            .next()
            .await
            .ok_or_else(|| "the relay closed the link during the challenge".to_string())?
            .map_err(|e| format!("the relay link failed during the challenge: {e}"))?;
        match msg {
            Message::Text(text) => {
                return serde_json::from_str(&text)
                    .map_err(|e| format!("the relay sent {text}, which is not a challenge: {e}"))
            }
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(frame) => {
                let code = frame.as_ref().map_or(0, |f| u16::from(f.code));
                return Err(match code {
                    4003 => "the relay rejected this host's proof".to_string(),
                    4409 => "another link for this host replaced this one".to_string(),
                    0 => "the relay closed the link during the challenge".to_string(),
                    other => format!("the relay closed the link with {other}"),
                });
            }
            other => return Err(format!("the relay sent an unexpected frame: {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------
// Demultiplexing
// ---------------------------------------------------------------------------

type LinkWs =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// One virtual connection's registration on the link.
struct Registered {
    /// The demux's end of the connection's inbound queue. Dropping it ends the
    /// `Stream`, which is how a CLOSE turns into end-of-stream for `serve`.
    inbound: mpsc::Sender<std::result::Result<Message, WsError>>,
    shared: Arc<ConnShared>,
}

/// The one bit of state the demux and a virtual connection both touch: whether
/// a CLOSE has already gone out for this connection, so it is sent exactly once
/// no matter which end finished first.
struct ConnShared {
    closed: AtomicBool,
}

/// Read the link until it goes away, routing frames to virtual connections and
/// keeping the hop alive with its own ping/pong.
async fn demux(
    state: &Arc<RemoteState>,
    ws: LinkWs,
    timing: &RelayTiming,
    outbound: &Outgoing,
    shutdown: &mut broadcast::Receiver<()>,
) -> Outcome {
    let (sink, mut stream) = ws.split();
    let (out_tx, out_rx) = mpsc::channel::<Message>(LINK_OUTBOUND_BUFFER);
    let writer = tokio::spawn(write_loop(sink, out_rx));
    // Published only for the life of this attempt: a NOTIFY enqueued against a
    // link that has gone away would sit in a queue nothing is writing.
    *outbound.lock() = Some(out_tx.clone());

    let mut conns: HashMap<u32, Registered> = HashMap::new();
    // Owns the per-connection `serve` tasks, so winding the link down can wait
    // for their close frames instead of cutting them off.
    let mut serving: JoinSet<()> = JoinSet::new();
    let mut missed_pongs = 0u32;
    let mut ping = tokio::time::interval(timing.ping_interval);
    // `interval` fires immediately; the first real ping belongs one period out.
    ping.tick().await;

    let outcome = loop {
        tokio::select! {
            biased;
            _ = shutdown.recv() => break Outcome::Done,
            _ = ping.tick() => {
                if missed_pongs >= MAX_MISSED_PONGS {
                    break retry("the relay stopped answering pings");
                }
                missed_pongs += 1;
                // `try_send`, not `send`: a full queue means the relay has
                // stopped reading, and awaiting a slot here would park the
                // demux — pongs included — so the link could never time out.
                if out_tx.try_send(Message::Ping(Bytes::new())).is_err() {
                    break retry("the relay stopped reading");
                }
            }
            incoming = stream.next() => {
                let msg = match incoming {
                    Some(Ok(msg)) => msg,
                    Some(Err(e)) => break retry(format!("the relay link failed: {e}")),
                    None => break retry("the relay closed the link"),
                };
                match msg {
                    Message::Binary(bytes) => match Frame::decode(&bytes) {
                        Ok(frame) => route(state, frame, &mut conns, &mut serving, &out_tx),
                        Err(e) => tracing::debug!(error = %e, "remote: undecodable relay frame"),
                    },
                    Message::Pong(_) => missed_pongs = 0,
                    Message::Close(frame) => {
                        let code = frame.as_ref().map_or(0, |f| u16::from(f.code));
                        break retry(format!("the relay closed the link with {code}"));
                    }
                    // Text after `ready` is not part of the contract. Ignored
                    // rather than fatal, so a relay that grows a control
                    // message does not knock older hosts off.
                    Message::Text(text) => {
                        tracing::debug!(%text, "remote: unexpected text frame on the relay link");
                    }
                    _ => {}
                }
            }
            // Reaped so a long-lived link does not accumulate finished tasks.
            Some(_) = serving.join_next(), if !serving.is_empty() => {}
        }
    };

    // Wind-down. The virtual connections have been asked to close by whoever
    // is taking the link away (`stop` closes every session with 4004 before
    // dropping this handle), and their close frames are still travelling
    // through `out_tx`; give them a bounded moment to land before the socket
    // goes. Then drop the senders so the writer flushes and finishes.
    *outbound.lock() = None;
    drop(conns);
    let _ = tokio::time::timeout(timing.shutdown_grace, async {
        while serving.join_next().await.is_some() {}
    })
    .await;
    serving.shutdown().await;
    drop(out_tx);
    let _ = tokio::time::timeout(timing.shutdown_grace, writer).await;
    outcome
}

/// Serialize the link's outbound queue onto the socket. One task, so the frames
/// of every virtual connection interleave in exactly the order they were queued.
async fn write_loop(mut sink: SplitSink<LinkWs, Message>, mut rx: mpsc::Receiver<Message>) {
    while let Some(msg) = rx.recv().await {
        if sink.send(msg).await.is_err() {
            return;
        }
    }
    let _ = sink.close().await;
}

/// Route one inbound frame. OPEN creates a virtual connection; DATA/TEXT/CLOSE
/// for a connection the host does not know are ignored, which is what the doc
/// asks for — the relay may still be forwarding a device's last message when
/// the host has already finished with it.
fn route(
    state: &Arc<RemoteState>,
    frame: Frame,
    conns: &mut HashMap<u32, Registered>,
    serving: &mut JoinSet<()>,
    out_tx: &mpsc::Sender<Message>,
) {
    let conn = frame.conn();
    match frame {
        Frame::Open { .. } => {
            if conns.contains_key(&conn) {
                tracing::warn!(conn_id = conn, "remote: relay reopened a live connection");
                return;
            }
            let (inbound_tx, inbound_rx) = mpsc::channel(CONN_INBOUND_BUFFER);
            let shared = Arc::new(ConnShared {
                closed: AtomicBool::new(false),
            });
            let virtual_conn = VirtualConn {
                conn,
                inbound: inbound_rx,
                self_inbound: inbound_tx.clone(),
                out: Outbound::Idle(out_tx.clone()),
                shared: shared.clone(),
                done: false,
            };
            conns.insert(
                conn,
                Registered {
                    inbound: inbound_tx,
                    shared: shared.clone(),
                },
            );
            let state = state.clone();
            let out_tx = out_tx.clone();
            serving.spawn(async move {
                // A synthetic peer: there is no socket address behind a virtual
                // connection, so the identifiable part is the span's `conn_id`.
                let peer = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0);
                let span = tracing::info_span!("relay_conn", conn_id = conn);
                // The `Option<S>` is for the TCP path's drain, which has no
                // meaning here: there is no socket under this to reset.
                let _ = server::serve(state, virtual_conn, peer)
                    .instrument(span)
                    .await;
                // `serve` is done and never sent a close of its own (a dropped
                // socket, on the LAN). The relay still has a device link open,
                // so it needs to be told.
                if !shared.closed.swap(true, Ordering::SeqCst) {
                    let _ = out_tx
                        .send(
                            Frame::Close {
                                conn,
                                code: 1000,
                                reason: String::new(),
                            }
                            .into_message(),
                        )
                        .await;
                }
            });
        }
        Frame::Data { payload, .. } => {
            deliver(conns, conn, out_tx, Ok(Message::Binary(payload)));
        }
        Frame::Text { text, .. } => {
            deliver(conns, conn, out_tx, Ok(Message::Text(text.into())));
        }
        // Host → relay only. A relay sending one back is either confused or
        // speaking a later protocol; dropped, as the doc has every frame the
        // host does not expect.
        Frame::Notify { .. } => {
            tracing::debug!("remote: the relay sent a NOTIFY frame, which is host to relay only");
        }
        Frame::Close { code, reason, .. } => {
            if let Some(registered) = conns.remove(&conn) {
                // The relay has already closed the device link, so nothing more
                // may be sent for this connection.
                registered.shared.closed.store(true, Ordering::SeqCst);
                let frame = CloseFrame {
                    code: CloseCode::from(code),
                    reason: reason.into(),
                };
                let _ = registered.inbound.try_send(Ok(Message::Close(Some(frame))));
                // Dropping the sender ends the connection's inbound stream.
            }
        }
    }
}

/// Hand one message to a virtual connection. A full inbound queue means the
/// device is talking faster than the host can read it, which costs that one
/// connection: it is closed with 1008 and its stream ends.
fn deliver(
    conns: &mut HashMap<u32, Registered>,
    conn: u32,
    out_tx: &mpsc::Sender<Message>,
    msg: std::result::Result<Message, WsError>,
) {
    let Some(registered) = conns.get(&conn) else {
        tracing::debug!(
            conn_id = conn,
            "remote: relay frame for an unknown connection"
        );
        return;
    };
    if registered.inbound.try_send(msg).is_err() {
        tracing::warn!(
            conn_id = conn,
            "remote: relayed device outran its inbound queue"
        );
        if let Some(registered) = conns.remove(&conn) {
            if !registered.shared.closed.swap(true, Ordering::SeqCst) {
                let _ = out_tx.try_send(
                    Frame::Close {
                        conn,
                        code: 1008,
                        reason: "inbound backlog".to_string(),
                    }
                    .into_message(),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The virtual connection
// ---------------------------------------------------------------------------

/// One device link as a `WsTransport`: inbound comes from the demux, outbound
/// becomes DATA and CLOSE frames on the link's shared queue.
///
/// This is the whole of the relay's intrusion into the server. `serve` sees a
/// WebSocket; the frames inside it are the same end-to-end encrypted frames a
/// LAN socket carries.
struct VirtualConn {
    conn: u32,
    inbound: mpsc::Receiver<std::result::Result<Message, WsError>>,
    /// Our own inbound sender, so a liveness ping can be answered locally: the
    /// doc keeps ping/pong per hop, so a `Ping` the host writes to a virtual
    /// connection must never reach the relay.
    self_inbound: mpsc::Sender<std::result::Result<Message, WsError>>,
    out: Outbound,
    shared: Arc<ConnShared>,
    /// Set once a CLOSE has been queued: the sink is finished, and anything
    /// after it is dropped rather than reordered past the close.
    done: bool,
}

/// The outbound half's permit machinery. `tokio_util::sync::PollSender` is
/// exactly this, but `tokio-util` is not a direct dependency of this crate and
/// one small `Sink` impl is cheaper than one more crate.
enum Outbound {
    Idle(mpsc::Sender<Message>),
    Reserving(Reserving),
    Ready(mpsc::OwnedPermit<Message>),
    Gone,
}

/// `Sender::reserve_owned` in flight: the one thing that can be `Pending` here,
/// and the whole reason this is a state machine rather than a `try_send`.
type Reserving = Pin<
    Box<
        dyn Future<
                Output = std::result::Result<
                    mpsc::OwnedPermit<Message>,
                    mpsc::error::SendError<()>,
                >,
            > + Send,
    >,
>;

fn link_gone() -> WsError {
    WsError::Io(std::io::Error::other("the relay link is gone"))
}

impl Stream for VirtualConn {
    type Item = std::result::Result<Message, WsError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inbound.poll_recv(cx)
    }
}

impl Sink<Message> for VirtualConn {
    type Error = WsError;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), WsError>> {
        loop {
            match std::mem::replace(&mut self.out, Outbound::Gone) {
                Outbound::Idle(tx) => self.out = Outbound::Reserving(Box::pin(tx.reserve_owned())),
                Outbound::Reserving(mut reserving) => match reserving.as_mut().poll(cx) {
                    Poll::Ready(Ok(permit)) => {
                        self.out = Outbound::Ready(permit);
                        return Poll::Ready(Ok(()));
                    }
                    Poll::Ready(Err(_)) => return Poll::Ready(Err(link_gone())),
                    Poll::Pending => {
                        self.out = Outbound::Reserving(reserving);
                        return Poll::Pending;
                    }
                },
                Outbound::Ready(permit) => {
                    self.out = Outbound::Ready(permit);
                    return Poll::Ready(Ok(()));
                }
                Outbound::Gone => return Poll::Ready(Err(link_gone())),
            }
        }
    }

    fn start_send(mut self: Pin<&mut Self>, msg: Message) -> Result<(), WsError> {
        if self.done {
            return Ok(());
        }
        let conn = self.conn;
        let frame = match msg {
            Message::Binary(payload) => Frame::Data { conn, payload },
            Message::Close(frame) => {
                self.done = true;
                // Exactly one CLOSE per connection: if the demux already saw one
                // from the relay, the device link is gone and this is a no-op.
                if self.shared.closed.swap(true, Ordering::SeqCst) {
                    return Ok(());
                }
                let (code, reason) = frame
                    .map(|f| (u16::from(f.code), f.reason.to_string()))
                    .unwrap_or((1000, String::new()));
                Frame::Close { conn, code, reason }
            }
            // Per the doc, liveness pings to a virtual connection are answered
            // locally; this hop's own ping/pong is the link's, not a device's.
            Message::Ping(payload) => {
                let _ = self.self_inbound.try_send(Ok(Message::Pong(payload)));
                return Ok(());
            }
            Message::Pong(_) | Message::Frame(_) => return Ok(()),
            // Unreachable: past the handshake `pump` encrypts every protocol
            // frame into a binary message, and the handshake itself is binary
            // too. A text frame here would be a host bug, and forwarding it as
            // DATA would show the phone plaintext where it expects ciphertext —
            // so the connection ends instead.
            Message::Text(_) => {
                tracing::error!(
                    conn_id = conn,
                    "remote: a text frame reached the relay link"
                );
                return Err(WsError::Io(std::io::Error::other(
                    "a text frame cannot travel over the relay",
                )));
            }
        };
        match std::mem::replace(&mut self.out, Outbound::Gone) {
            Outbound::Ready(permit) => {
                self.out = Outbound::Idle(permit.send(frame.into_message()));
                Ok(())
            }
            // `Sink`'s contract is `poll_ready` before every `start_send`; being
            // here means the queue is gone (or the caller broke the contract),
            // and either way the frame cannot be written.
            other => {
                self.out = other;
                Err(link_gone())
            }
        }
    }

    /// Nothing is buffered here: `start_send` hands the frame straight to the
    /// link's queue, and the writer task is what flushes it.
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), WsError>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), WsError>> {
        self.done = true;
        self.out = Outbound::Gone;
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod codec_tests {
    use super::*;

    fn roundtrip(frame: Frame) {
        let encoded = frame.encode();
        assert_eq!(Frame::decode(&encoded).unwrap(), frame, "{frame:?}");
    }

    #[test]
    fn every_frame_type_roundtrips() {
        roundtrip(Frame::Open { conn: 0 });
        roundtrip(Frame::Open { conn: u32::MAX });
        roundtrip(Frame::Data {
            conn: 7,
            payload: Bytes::from_static(&[0, 1, 2, 255]),
        });
        roundtrip(Frame::Data {
            conn: 7,
            payload: Bytes::new(),
        });
        roundtrip(Frame::Close {
            conn: 9,
            code: 4004,
            reason: "remote access disabled".to_string(),
        });
        roundtrip(Frame::Close {
            conn: 9,
            code: 1000,
            reason: String::new(),
        });
        roundtrip(Frame::Text {
            conn: 3,
            text: "{\"op\":\"hello\"}".to_string(),
        });
        roundtrip(Frame::Notify {
            payload: Bytes::from_static(br#"{"kind":"turn_complete"}"#),
        });
        roundtrip(Frame::Notify {
            payload: Bytes::new(),
        });
    }

    /// The header the doc fixes: `type (1) || connId (u32 big-endian)`, then the
    /// payload, and for CLOSE a `u16` big-endian code before the reason.
    #[test]
    fn the_wire_layout_is_the_documented_one() {
        assert_eq!(
            Frame::Data {
                conn: 0x01020304,
                payload: Bytes::from_static(b"hi"),
            }
            .encode()
            .as_ref(),
            &[0x02, 0x01, 0x02, 0x03, 0x04, b'h', b'i']
        );
        assert_eq!(
            Frame::Close {
                conn: 1,
                code: 4004,
                reason: "x".to_string(),
            }
            .encode()
            .as_ref(),
            &[0x03, 0, 0, 0, 1, 0x0f, 0xa4, b'x']
        );
        assert_eq!(
            Frame::Open { conn: 1 }.encode().as_ref(),
            &[0x01, 0, 0, 0, 1]
        );
        // NOTIFY is `0x05` on connId 0 — it belongs to no virtual connection.
        assert_eq!(
            Frame::Notify {
                payload: Bytes::from_static(b"{}"),
            }
            .encode()
            .as_ref(),
            &[0x05, 0, 0, 0, 0, b'{', b'}']
        );
    }

    #[test]
    fn malformed_frames_are_rejected_not_guessed() {
        for bytes in [
            vec![],
            vec![0x02],
            vec![0x02, 0, 0, 0],
            // CLOSE without room for a code.
            vec![0x03, 0, 0, 0, 1],
            vec![0x03, 0, 0, 0, 1, 0x0f],
            // A type the host does not know.
            vec![0x09, 0, 0, 0, 1],
        ] {
            assert!(
                Frame::decode(&bytes).is_err(),
                "{bytes:?} should not decode"
            );
        }
    }
}
