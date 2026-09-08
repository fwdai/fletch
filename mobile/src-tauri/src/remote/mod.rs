// The phone's end of the remote protocol's secure channel. The socket and the
// Noise state live here, so the webview only ever sees plaintext JSON and the
// network only ever sees ciphertext (docs/remote-protocol.md, "Transport" and
// "Secure channel").
//
// One live connection at a time, but every connection has an id that the
// webview gets back from `remote_connect` and must present to `remote_send`
// and `remote_close`, and that every event carries. Nothing acts on "whatever
// is connected right now": a wrapper that was superseded while its handshake
// was in flight cannot close its successor, send on it, or hear its events.

mod secure;

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use secure::{Channel, DeviceKey, Handshake, HOST_KEY_MISMATCH};
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
type Writer = SplitSink<Ws, Message>;
type Reader = SplitStream<Ws>;
type Slot = Arc<Mutex<Option<Connection>>>;

/// Identifies one connection attempt for the life of the process. Handed to
/// the webview by `remote_connect`; never reused.
type ConnectionId = u64;

/// WebSocket close code for an abrupt drop — no close frame arrived.
const CLOSE_ABNORMAL: u16 = 1006;
/// The protocol's code for a failed handshake or a cleartext frame.
const CLOSE_BAD_FRAME: u16 = 4001;

struct Connection {
    id: ConnectionId,
    writer: Writer,
    /// One Noise object drives both directions; sends and receives are small
    /// enough that sharing it behind the connection's lock costs nothing.
    channel: Channel,
}

#[derive(Default)]
pub struct Remote {
    slot: Slot,
    /// The id most recently handed out. An attempt that finds a newer one when
    /// it comes back from its handshake has been superseded and stands down.
    latest: AtomicU64,
    /// The device identity, loaded from disk once per process and shared by
    /// every command. One place to load it means one place to create it: two
    /// commands racing on first use cannot each generate a key and fight over
    /// the file, and whichever one runs first hands the same identity to the
    /// rest. A failed load is not cached, so a fixed disk is retried.
    identity: std::sync::Mutex<Option<Arc<DeviceKey>>>,
}

impl Remote {
    /// The device key from `dir`, creating it on first use. Serialized by the
    /// lock for the whole load, so concurrent callers see one identity.
    fn identity(&self, dir: &std::path::Path) -> Result<Arc<DeviceKey>, String> {
        let mut cached = self
            .identity
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(key) = cached.as_ref() {
            return Ok(key.clone());
        }
        let key = Arc::new(DeviceKey::load_or_create(dir)?);
        *cached = Some(key.clone());
        Ok(key)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectResult {
    /// The responder's static key, base64url — the host's identity. The caller
    /// pins this when it had none to compare against.
    host_key: String,
    /// What `remote_send`, `remote_close` and every event refer to.
    connection_id: ConnectionId,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TextPayload {
    connection_id: ConnectionId,
    text: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClosePayload {
    connection_id: ConnectionId,
    code: u16,
    reason: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorPayload {
    connection_id: ConnectionId,
    message: String,
}

/// Open a socket, run the Noise handshake and start reading. Supersedes any
/// connection already open or still handshaking: the newest attempt owns the
/// slot, and an older one that finishes later discards its socket.
///
/// `timeout_ms` bounds the dial and the handshake together. It is the only
/// timeout: the caller walks a list of candidates (LAN, then relay) and needs
/// a candidate it has given up on to be really gone, which a timer on the
/// webview side could not deliver — so the budget is enforced here, where the
/// socket is, and expiring closes it.
#[tauri::command]
pub async fn remote_connect(
    app: AppHandle,
    state: State<'_, Remote>,
    url: String,
    host_key: Option<String>,
    timeout_ms: Option<u64>,
) -> Result<ConnectResult, String> {
    // Claim the id first, so a concurrent attempt can tell who is newer.
    let id = state.latest.fetch_add(1, Ordering::AcqRel) + 1;
    close_current(&state.slot).await;
    let key = device_key(&app, &state)?;
    let deadline = timeout_ms.map(|ms| Instant::now() + Duration::from_millis(ms));
    let dialled = within(deadline, connect_async(&url))
        .await
        .ok_or_else(|| timed_out(&url, timeout_ms))?;
    let (mut ws, _) = dialled.map_err(|e| format!("cannot reach {url}: {e}"))?;
    // The borrow of `ws` ends with the statement, so the socket is ours again
    // whether the handshake finished, failed or ran out of time.
    let handshaken = within(deadline, handshake(&mut ws, &key, host_key.as_deref())).await;
    let channel = match handshaken {
        Some(Ok(channel)) => channel,
        Some(Err(e)) => {
            close_ws(&mut ws, CLOSE_BAD_FRAME, &e).await;
            return Err(e);
        }
        None => {
            let e = timed_out(&url, timeout_ms);
            close_ws(&mut ws, u16::from(CloseCode::Normal), &e).await;
            return Err(e);
        }
    };
    let seen = channel.remote_static_base64()?;

    let mut guard = state.slot.lock().await;
    if state.latest.load(Ordering::Acquire) != id {
        // A newer attempt started while this one was handshaking. It owns the
        // slot (or will); this socket must not replace it.
        drop(guard);
        close_ws(&mut ws, u16::from(CloseCode::Normal), "superseded").await;
        return Err("connection superseded by a newer attempt".into());
    }
    let (writer, reader) = ws.split();
    *guard = Some(Connection {
        id,
        writer,
        channel,
    });
    drop(guard);
    spawn_reader(app, state.slot.clone(), reader, id);
    Ok(ConnectResult {
        host_key: seen,
        connection_id: id,
    })
}

/// Encrypt one JSON document and send it as a single binary message on the
/// connection `connection_id`. "not connected" if that connection is gone.
#[tauri::command]
pub async fn remote_send(
    state: State<'_, Remote>,
    connection_id: ConnectionId,
    text: String,
) -> Result<(), String> {
    let mut guard = state.slot.lock().await;
    let conn = guard
        .as_mut()
        .filter(|conn| conn.id == connection_id)
        .ok_or_else(|| "not connected".to_string())?;
    let frame = conn.channel.encrypt(text.as_bytes())?;
    conn.writer
        .send(Message::binary(frame))
        .await
        .map_err(|e| e.to_string())
}

/// Close the connection `connection_id` with 1000. A no-op if it is already
/// gone or was superseded — never touches a newer connection.
#[tauri::command]
pub async fn remote_close(
    state: State<'_, Remote>,
    connection_id: ConnectionId,
) -> Result<(), String> {
    let taken = {
        let mut guard = state.slot.lock().await;
        if guard.as_ref().is_some_and(|conn| conn.id == connection_id) {
            guard.take()
        } else {
            None
        }
    };
    if let Some(conn) = taken {
        close_connection(conn).await;
    }
    Ok(())
}

/// This device's public key, base64url — what the host records when pairing.
#[tauri::command]
pub fn remote_device_public_key(
    app: AppHandle,
    state: State<'_, Remote>,
) -> Result<String, String> {
    Ok(device_key(&app, &state)?.public_base64())
}

// --- internals ---------------------------------------------------------------

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

/// The process-wide device identity (see `Remote::identity`).
fn device_key(app: &AppHandle, state: &Remote) -> Result<Arc<DeviceKey>, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("no app data dir: {e}"))?;
    state.identity(&dir)
}

/// `-> e`, `<- e, ee, s, es`, `-> s, se`, all as binary messages with empty
/// payloads. The phone is the initiator.
///
/// `expected` is the host key the caller insists on. It is checked as soon as
/// message 2 delivers it, which is before message 3 reveals this device's own
/// identity — an impostor learns nothing.
async fn handshake(
    ws: &mut Ws,
    key: &DeviceKey,
    expected: Option<&str>,
) -> Result<Channel, String> {
    let mut hs = Handshake::new(key, true)?;
    let first = hs.write()?;
    send(ws, first).await?;
    let response = next_binary(ws).await?;
    hs.read(&response)?;
    let seen = hs.remote_static_base64()?;
    if let Some(expected) = expected {
        if expected != seen {
            return Err(format!(
                "{HOST_KEY_MISMATCH}: expected {expected}, host presented {seen}"
            ));
        }
    }
    let third = hs.write()?;
    send(ws, third).await?;
    hs.finish()
}

async fn send(ws: &mut Ws, bytes: Vec<u8>) -> Result<(), String> {
    ws.send(Message::binary(bytes))
        .await
        .map_err(|e| format!("handshake failed: {e}"))
}

/// The next binary message. Anything else at handshake time is a protocol
/// violation, which the caller turns into a 4001.
async fn next_binary(ws: &mut Ws) -> Result<Vec<u8>, String> {
    while let Some(message) = ws.next().await {
        match message.map_err(|e| format!("handshake failed: {e}"))? {
            Message::Binary(bytes) => return Ok(bytes.to_vec()),
            Message::Text(_) => return Err("the host sent a cleartext frame".into()),
            Message::Close(_) => break,
            _ => {}
        }
    }
    Err("the host closed during the handshake".into())
}

/// Decrypt every binary message onto the event bus until the socket ends, then
/// report how it ended. Every event names the connection, and the task stops
/// the moment the slot no longer holds this connection: a superseded socket's
/// last frames go nowhere.
fn spawn_reader(app: AppHandle, slot: Slot, mut reader: Reader, id: ConnectionId) {
    tauri::async_runtime::spawn(async move {
        let mut code = CLOSE_ABNORMAL;
        let mut reason = String::new();
        while let Some(message) = reader.next().await {
            match message {
                Ok(Message::Binary(bytes)) => {
                    let plaintext = {
                        let mut guard = slot.lock().await;
                        match guard.as_mut() {
                            Some(conn) if conn.id == id => conn.channel.decrypt(&bytes),
                            // Replaced or closed while this frame was in
                            // flight: stay quiet, the live socket is someone
                            // else's.
                            _ => return,
                        }
                    };
                    match plaintext.and_then(text_of) {
                        Ok(text) => {
                            let _ = app.emit(
                                "remote:message",
                                TextPayload {
                                    connection_id: id,
                                    text,
                                },
                            );
                        }
                        Err(e) => {
                            let _ = app.emit(
                                "remote:error",
                                ErrorPayload {
                                    connection_id: id,
                                    message: e.clone(),
                                },
                            );
                            code = CLOSE_BAD_FRAME;
                            reason = e;
                            break;
                        }
                    }
                }
                // After the handshake every protocol frame is encrypted, so a
                // text frame is a violation.
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
                // Ping/pong are the transport's business and stay unencrypted.
                Ok(_) => {}
                Err(e) => {
                    let message = e.to_string();
                    let _ = app.emit(
                        "remote:error",
                        ErrorPayload {
                            connection_id: id,
                            message: message.clone(),
                        },
                    );
                    reason = message;
                    break;
                }
            }
        }
        report_close(&app, &slot, id, code, reason).await;
    });
}

fn text_of(plaintext: Vec<u8>) -> Result<String, String> {
    String::from_utf8(plaintext).map_err(|_| "frame is not UTF-8".to_string())
}

/// Hand the close to the webview, but only while this connection is still the
/// live one.
async fn report_close(app: &AppHandle, slot: &Slot, id: ConnectionId, code: u16, reason: String) {
    let taken = {
        let mut guard = slot.lock().await;
        if guard.as_ref().is_some_and(|conn| conn.id == id) {
            guard.take()
        } else {
            None
        }
    };
    if let Some(mut conn) = taken {
        let _ = conn.writer.close().await;
        let _ = app.emit(
            "remote:close",
            ClosePayload {
                connection_id: id,
                code,
                reason,
            },
        );
    }
}

/// Close whatever connection is live, on behalf of a newer attempt. Emits
/// nothing: the reader task sees its id is gone and stays quiet.
async fn close_current(slot: &Slot) {
    let taken = slot.lock().await.take();
    if let Some(conn) = taken {
        close_connection(conn).await;
    }
}

/// A clean 1000 on a connection that has already left the slot.
async fn close_connection(mut conn: Connection) {
    let _ = conn
        .writer
        .send(Message::Close(Some(CloseFrame {
            code: CloseCode::Normal,
            reason: Default::default(),
        })))
        .await;
    let _ = conn.writer.close().await;
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
mod tests {
    use super::*;

    /// First use from several commands at once must yield one identity and one
    /// file: `remote_connect` and `remote_device_public_key` both go through
    /// `Remote::identity`, which is what makes that true.
    #[test]
    fn concurrent_first_use_settles_on_one_identity() {
        let dir = std::env::temp_dir().join(format!("fletch-identity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let remote = Arc::new(Remote::default());

        let workers: Vec<_> = (0..8)
            .map(|_| {
                let remote = remote.clone();
                let dir = dir.clone();
                std::thread::spawn(move || remote.identity(&dir).unwrap().public_base64())
            })
            .collect();
        let seen: std::collections::HashSet<String> =
            workers.into_iter().map(|w| w.join().unwrap()).collect();

        assert_eq!(seen.len(), 1, "every caller got the same identity");
        let persisted = DeviceKey::load_or_create(&dir).unwrap().public_base64();
        assert!(seen.contains(&persisted), "and it is the one on disk");
        assert!(!dir.join("device_key.tmp").exists());
    }

    /// A candidate that never answers has to be distinguishable from one that
    /// refused, since the caller walks a list and reports the last failure.
    #[test]
    fn a_timeout_says_so_and_names_the_url() {
        let with_budget = timed_out("ws://192.168.1.24:47285/ws", Some(3000));
        assert!(with_budget.contains("timed out"), "{with_budget}");
        assert!(with_budget.contains("3000 ms"), "{with_budget}");
        assert!(with_budget.contains("ws://192.168.1.24:47285/ws"));
        assert!(timed_out("wss://relay.fletch.sh/v1/device/k", None).contains("timed out"));
    }

    /// `within` is what makes the budget real: a future that has not finished
    /// is dropped, which is what closes a half-open socket.
    #[test]
    fn within_bounds_a_future_and_passes_one_through_unbounded() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let deadline = Some(Instant::now() + Duration::from_millis(5));
            assert_eq!(within(deadline, async { 7 }).await, Some(7));
            assert_eq!(
                within(deadline, tokio::time::sleep(Duration::from_secs(30))).await,
                None,
                "an overrunning future is abandoned"
            );
            assert_eq!(within(None, async { 7 }).await, Some(7));
        });
    }
}
