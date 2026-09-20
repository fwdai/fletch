//! A client's end of the protocol: many secure channels to many hosts.
//!
//! The socket and the Noise state live here, so the app around this module only
//! ever sees plaintext JSON and the network only ever sees ciphertext
//! (docs/remote-protocol.md, "Transport" and "Secure channel").
//!
//! Every connection has an id that [`Dialer::connect`] hands back and that
//! [`Dialer::send`], [`Dialer::close`] and every [`ClientEvent`] carry. Nothing
//! here acts on "whatever is connected right now": connections are independent,
//! a new `connect` never disturbs an existing one, and a caller that holds a
//! stale id cannot send on, close, or hear another caller's connection.
//!
//! There is no Tauri in here. Both apps wrap this in four thin commands
//! (`remote_connect`, `remote_send`, `remote_close`,
//! `remote_device_public_key`) and hand [`ClientEvent`] to their event bus, so
//! the phone and the desktop dial hosts through the same code.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::{Bytes, Message};

use crate::dial::{self, Ws};
use crate::keys::{StaticKey, DEVICE_KEY_FILE};
use crate::noise::{initiate, Channel};
use crate::Result;

type Writer = SplitSink<Ws, Message>;
type Reader = SplitStream<Ws>;

/// Identifies one live connection for the life of the process. Handed out by
/// [`Dialer::connect`]; never reused.
pub type ConnectionId = u64;

/// WebSocket close code for an abrupt drop — no close frame arrived.
const CLOSE_ABNORMAL: u16 = 1006;
/// The protocol's code for a failed handshake or a cleartext frame.
const CLOSE_BAD_FRAME: u16 = 4001;
/// The reason on the close this end raises when the peer stops answering pings.
pub const PONG_TIMEOUT: &str = "pong timeout";

/// How this end tells a dead socket from a quiet one.
///
/// The host pings, but a socket the OS froze under a suspended app does not
/// carry the host's close frame back — and a read on it stays pending — so
/// without pings of its own the client learns nothing until TCP gives up,
/// minutes later, while every request it sends in the meantime hangs. Over the
/// relay the pong comes from the relay's runtime (docs/remote-protocol.md,
/// "Relay"), so this detects the device↔relay hop, which is the one a phone
/// loses in the background.
#[derive(Clone, Copy, Debug)]
pub struct Keepalive {
    pub interval: Duration,
    /// Pings sent without a pong in between before the connection is declared
    /// dead. The connection is closed on the tick *after* the last allowed
    /// miss, so the worst case is `(max_missed + 1) * interval`.
    pub max_missed: u32,
}

impl Default for Keepalive {
    /// Ten seconds and two misses: a dead socket is closed within thirty
    /// seconds of the app resuming, and a foregrounded idle phone sends six
    /// tiny frames a minute.
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(10),
            max_missed: 2,
        }
    }
}

/// Where to dial, whose identity to insist on, and how long to spend.
pub struct Target {
    /// A LAN `ws://` or a relay `wss://` — one candidate, already chosen.
    pub url: String,
    /// The host key the caller pins, base64url. `None` is trust on first use:
    /// whatever the handshake authenticates comes back in [`ConnectResult`].
    pub host_key: Option<String>,
    /// Bounds the dial and the handshake together. See [`Dialer::connect`].
    pub timeout_ms: Option<u64>,
}

/// What a completed [`Dialer::connect`] tells its caller.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectResult {
    /// The responder's static key, base64url — the host's identity. The caller
    /// pins this when it had none to compare against.
    pub host_key: String,
    /// What `send`, `close` and every event refer to.
    pub connection_id: ConnectionId,
}

/// Everything one connection tells the app after it is open. The payloads are
/// the event payloads on the wire between Rust and the webview, so they are
/// `camelCase` here rather than at each app's emit site.
pub enum ClientEvent {
    Message(TextPayload),
    Close(ClosePayload),
    Error(ErrorPayload),
}

impl ClientEvent {
    /// The event name both apps emit this under. One place, so the phone and
    /// the desktop cannot drift.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Message(_) => "remote:message",
            Self::Close(_) => "remote:close",
            Self::Error(_) => "remote:error",
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextPayload {
    pub connection_id: ConnectionId,
    pub text: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClosePayload {
    pub connection_id: ConnectionId,
    pub code: u16,
    pub reason: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorPayload {
    pub connection_id: ConnectionId,
    pub message: String,
}

struct Conn {
    writer: Writer,
    /// One Noise object drives both directions; sends and receives are small
    /// enough that sharing it behind the connection's own lock costs nothing.
    channel: Channel,
    /// Pings sent since the last pong. The keepalive task raises it, the read
    /// loop zeroes it (see [`Keepalive`]).
    missed_pongs: u32,
}

/// Every live connection this client holds, plus the device identity they all
/// authenticate with.
///
/// Held in an `Arc` because each connection's read loop is a task that outlives
/// the `connect` call that started it and has to reach back into the map — to
/// decrypt with the connection's channel, and to report the close.
pub struct Dialer {
    /// The directory holding `device_key`. Each app picks it; this never
    /// touches any other key file.
    dir: PathBuf,
    /// The device identity, loaded from disk on first use and shared by every
    /// connection. One place to load it means one place to create it: two
    /// callers racing on first use cannot each generate a key and fight over
    /// the file. A failed load is not cached, so a fixed disk is retried.
    identity: std::sync::Mutex<Option<Arc<StaticKey>>>,
    /// Keyed by connection id. The per-connection lock is what keeps one host's
    /// stalled socket from blocking every other connection's reads.
    conns: Mutex<HashMap<ConnectionId, Arc<Mutex<Conn>>>>,
    next_id: AtomicU64,
    events: Box<dyn Fn(ClientEvent) + Send + Sync>,
    keepalive: Keepalive,
}

impl Dialer {
    /// A dialer whose device key lives in `dir` and whose connections report
    /// themselves through `events`, pinging on the default [`Keepalive`].
    pub fn new(
        dir: impl Into<PathBuf>,
        events: Box<dyn Fn(ClientEvent) + Send + Sync>,
    ) -> Arc<Self> {
        Self::with_keepalive(dir, events, Keepalive::default())
    }

    /// [`Dialer::new`] with the ping schedule chosen — tests run it fast.
    pub fn with_keepalive(
        dir: impl Into<PathBuf>,
        events: Box<dyn Fn(ClientEvent) + Send + Sync>,
        keepalive: Keepalive,
    ) -> Arc<Self> {
        Arc::new(Self {
            dir: dir.into(),
            identity: std::sync::Mutex::new(None),
            conns: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(0),
            events,
            keepalive,
        })
    }

    /// Open a socket, run the Noise handshake and start reading. The new
    /// connection stands beside the ones already open: nothing is superseded,
    /// because the connection id is what everything afterwards names.
    ///
    /// `timeout_ms` bounds the dial and the handshake together. It is the only
    /// timeout: the caller walks a list of candidates (LAN, then relay) and
    /// needs a candidate it has given up on to be really gone, which a timer on
    /// the app side could not deliver — so the budget is enforced here, where
    /// the socket is, and expiring closes it.
    pub async fn connect(self: &Arc<Self>, target: Target) -> Result<ConnectResult> {
        let key = self.identity()?;
        let deadline = target
            .timeout_ms
            .map(|ms| Instant::now() + Duration::from_millis(ms));
        // Both address families race inside `dial::connect`, so one that
        // blackholes cannot spend the budget on behalf of the other.
        let dialled = within(deadline, dial::connect(&target.url))
            .await
            .ok_or_else(|| timed_out(&target.url, target.timeout_ms))?;
        let mut ws = dialled.map_err(|e| format!("cannot reach {}: {e}", target.url))?;
        // The borrow of `ws` ends with the statement, so the socket is ours
        // again whether the handshake finished, failed or ran out of time.
        let handshaken = within(
            deadline,
            initiate(&mut ws, &key, target.host_key.as_deref()),
        )
        .await;
        let channel = match handshaken {
            Some(Ok(channel)) => channel,
            Some(Err(e)) => {
                close_ws(&mut ws, CLOSE_BAD_FRAME, &e).await;
                return Err(e);
            }
            None => {
                let e = timed_out(&target.url, target.timeout_ms);
                close_ws(&mut ws, u16::from(CloseCode::Normal), &e).await;
                return Err(e);
            }
        };
        let host_key = channel.remote_static_base64()?;

        // The id is claimed here, once there is a connection to name: an
        // attempt that failed leaves no id behind for a caller to hold.
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (writer, reader) = ws.split();
        let conn = Arc::new(Mutex::new(Conn {
            writer,
            channel,
            missed_pongs: 0,
        }));
        self.conns.lock().await.insert(id, conn);
        self.clone().spawn_reader(reader, id);
        self.clone().spawn_keepalive(id);
        Ok(ConnectResult {
            host_key,
            connection_id: id,
        })
    }

    /// Encrypt one JSON document and send it as a single binary message on
    /// `id`. "not connected" if that connection is gone.
    pub async fn send(&self, id: ConnectionId, text: &str) -> Result<()> {
        let conn = self
            .conn(id)
            .await
            .ok_or_else(|| "not connected".to_string())?;
        let mut conn = conn.lock().await;
        let frame = conn.channel.encrypt_frame(text.as_bytes())?;
        conn.writer
            .send(Message::binary(frame))
            .await
            .map_err(|e| e.to_string())
    }

    /// Close `id` with 1000. A no-op if it is already gone, and it never
    /// touches another connection. Emits nothing: the caller asked for this,
    /// and the read loop sees its id is gone and stays quiet.
    pub async fn close(&self, id: ConnectionId) -> Result<()> {
        let taken = self.conns.lock().await.remove(&id);
        if let Some(conn) = taken {
            let mut conn = conn.lock().await;
            let _ = conn
                .writer
                .send(Message::Close(Some(CloseFrame {
                    code: CloseCode::Normal,
                    reason: Default::default(),
                })))
                .await;
            let _ = conn.writer.close().await;
        }
        Ok(())
    }

    /// This device's public key, base64url — what a host records when pairing.
    pub fn device_public_key(&self) -> Result<String> {
        Ok(self.identity()?.public_base64())
    }

    // --- internals -----------------------------------------------------------

    /// The device key from `self.dir`, creating it on first use. Serialized by
    /// the lock for the whole load, so concurrent callers see one identity.
    fn identity(&self) -> Result<Arc<StaticKey>> {
        let mut cached = self
            .identity
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(key) = cached.as_ref() {
            return Ok(key.clone());
        }
        let key = Arc::new(StaticKey::load_or_create(&self.dir, DEVICE_KEY_FILE)?);
        *cached = Some(key.clone());
        Ok(key)
    }

    async fn conn(&self, id: ConnectionId) -> Option<Arc<Mutex<Conn>>> {
        self.conns.lock().await.get(&id).cloned()
    }

    /// Decrypt every binary message onto the event sink until the socket ends,
    /// then report how it ended. Every event names the connection, and the task
    /// stops the moment the map no longer holds it: a closed connection's last
    /// frames go nowhere.
    fn spawn_reader(self: Arc<Self>, mut reader: Reader, id: ConnectionId) {
        tokio::spawn(async move {
            let mut code = CLOSE_ABNORMAL;
            let mut reason = String::new();
            while let Some(message) = reader.next().await {
                match message {
                    Ok(Message::Binary(bytes)) => {
                        let plaintext = match self.conn(id).await {
                            Some(conn) => conn.lock().await.channel.decrypt_frame(&bytes),
                            // Closed while this frame was in flight: stay quiet.
                            None => return,
                        };
                        match plaintext.and_then(text_of) {
                            Ok(text) => (self.events)(ClientEvent::Message(TextPayload {
                                connection_id: id,
                                text,
                            })),
                            Err(e) => {
                                (self.events)(ClientEvent::Error(ErrorPayload {
                                    connection_id: id,
                                    message: e.clone(),
                                }));
                                code = CLOSE_BAD_FRAME;
                                reason = e;
                                break;
                            }
                        }
                    }
                    // After the handshake every protocol frame is encrypted, so
                    // a text frame is a violation.
                    Ok(Message::Text(_)) => {
                        code = CLOSE_BAD_FRAME;
                        reason = "the host sent a cleartext frame".into();
                        break;
                    }
                    Ok(Message::Close(frame)) => {
                        if let Some(frame) = frame {
                            code = frame.code.into();
                            reason = frame.reason.to_string();
                        }
                        break;
                    }
                    // The answer to our keepalive: the socket is alive.
                    Ok(Message::Pong(_)) => {
                        if let Some(conn) = self.conn(id).await {
                            conn.lock().await.missed_pongs = 0;
                        }
                    }
                    // The host's own pings are answered by the socket itself;
                    // frames are the transport's business and stay unencrypted.
                    Ok(_) => {}
                    Err(e) => {
                        let message = e.to_string();
                        (self.events)(ClientEvent::Error(ErrorPayload {
                            connection_id: id,
                            message: message.clone(),
                        }));
                        reason = message;
                        break;
                    }
                }
            }
            self.report_close(id, code, reason).await;
        });
    }

    /// Ping `id` on the [`Keepalive`] schedule and close it as abnormal (1006,
    /// [`PONG_TIMEOUT`]) once too many pings go unanswered. Stops on its own
    /// when the connection is gone from the map, whoever removed it.
    fn spawn_keepalive(self: Arc<Self>, id: ConnectionId) {
        let Keepalive {
            interval,
            max_missed,
        } = self.keepalive;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(conn) = self.conn(id).await else {
                    return;
                };
                let mut guard = conn.lock().await;
                if guard.missed_pongs >= max_missed {
                    drop(guard);
                    self.report_close(id, CLOSE_ABNORMAL, PONG_TIMEOUT.into())
                        .await;
                    return;
                }
                guard.missed_pongs += 1;
                let sent = guard.writer.send(Message::Ping(Bytes::new())).await;
                drop(guard);
                if let Err(e) = sent {
                    // A write the socket refuses is the same news, sooner.
                    self.report_close(id, CLOSE_ABNORMAL, e.to_string()).await;
                    return;
                }
            }
        });
    }

    /// Hand the close to the app, but only while this connection is still
    /// registered — a `close` the app asked for has already been accounted for.
    async fn report_close(&self, id: ConnectionId, code: u16, reason: String) {
        let taken = self.conns.lock().await.remove(&id);
        if let Some(conn) = taken {
            let _ = conn.lock().await.writer.close().await;
            (self.events)(ClientEvent::Close(ClosePayload {
                connection_id: id,
                code,
                reason,
            }));
        }
    }
}

/// Run `fut` under `deadline`, or unbounded when there is none. `None` means it
/// ran out of time and the future has been dropped.
async fn within<F: std::future::Future>(deadline: Option<Instant>, fut: F) -> Option<F::Output> {
    match deadline {
        Some(at) => tokio::time::timeout_at(at, fut).await.ok(),
        None => Some(fut.await),
    }
}

/// The error a caller sees when opening a connection overran its budget. The
/// wording matters only in that it says "timed out": that is what tells a
/// candidate that did not answer from one that refused.
fn timed_out(url: &str, timeout_ms: Option<u64>) -> String {
    match timeout_ms {
        Some(ms) => format!("cannot reach {url}: timed out after {ms} ms"),
        None => format!("cannot reach {url}: timed out"),
    }
}

fn text_of(plaintext: Vec<u8>) -> Result<String> {
    String::from_utf8(plaintext).map_err(|_| "frame is not UTF-8".to_string())
}

async fn close_ws(ws: &mut Ws, code: u16, reason: &str) {
    let _ = ws
        .send(Message::Close(Some(CloseFrame {
            code: CloseCode::from(code),
            reason: reason.to_string().into(),
        })))
        .await;
    let _ = ws.close(None).await;
}

#[cfg(test)]
mod tests;
