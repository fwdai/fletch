// The phone's end of the remote protocol's secure channel. The socket and the
// Noise state live here, so the webview only ever sees plaintext JSON and the
// network only ever sees ciphertext (docs/remote-protocol.md, "Transport" and
// "Secure channel").
//
// One connection at a time, identified by a generation: a socket that has been
// replaced or closed reports nothing, so a late drop cannot tear down its
// successor.

mod secure;

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use secure::{Channel, DeviceKey, Handshake, HOST_KEY_MISMATCH};
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
type Writer = SplitSink<Ws, Message>;
type Reader = SplitStream<Ws>;
type Slot = Arc<Mutex<Option<Connection>>>;

/// WebSocket close code for an abrupt drop — no close frame arrived.
const CLOSE_ABNORMAL: u16 = 1006;
/// The protocol's code for a failed handshake or a cleartext frame.
const CLOSE_BAD_FRAME: u16 = 4001;

struct Connection {
    generation: u64,
    writer: Writer,
    /// One Noise object drives both directions; sends and receives are small
    /// enough that sharing it behind the connection's lock costs nothing.
    channel: Channel,
}

#[derive(Default)]
pub struct Remote {
    slot: Slot,
    generations: AtomicU64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectResult {
    /// The responder's static key, base64url — the host's identity. The caller
    /// pins this when it had none to compare against.
    host_key: String,
}

#[derive(Clone, Serialize)]
struct TextPayload {
    text: String,
}

#[derive(Clone, Serialize)]
struct ClosePayload {
    code: u16,
    reason: String,
}

#[derive(Clone, Serialize)]
struct ErrorPayload {
    message: String,
}

/// Open a socket, run the Noise handshake and start reading. Replaces any
/// connection already open.
#[tauri::command]
pub async fn remote_connect(
    app: AppHandle,
    state: State<'_, Remote>,
    url: String,
    host_key: Option<String>,
) -> Result<ConnectResult, String> {
    close_slot(&state.slot).await;
    let key = device_key(&app)?;
    let (mut ws, _) = connect_async(&url)
        .await
        .map_err(|e| format!("cannot reach {url}: {e}"))?;
    let channel = match handshake(&mut ws, &key, host_key.as_deref()).await {
        Ok(channel) => channel,
        Err(e) => {
            close_ws(&mut ws, CLOSE_BAD_FRAME, &e).await;
            return Err(e);
        }
    };
    let seen = channel.remote_static_base64()?;
    let generation = state.generations.fetch_add(1, Ordering::Relaxed) + 1;
    let (writer, reader) = ws.split();
    *state.slot.lock().await = Some(Connection {
        generation,
        writer,
        channel,
    });
    spawn_reader(app, state.slot.clone(), reader, generation);
    Ok(ConnectResult { host_key: seen })
}

/// Encrypt one JSON document and send it as a single binary message.
#[tauri::command]
pub async fn remote_send(state: State<'_, Remote>, text: String) -> Result<(), String> {
    let mut guard = state.slot.lock().await;
    let conn = guard.as_mut().ok_or_else(|| "not connected".to_string())?;
    let frame = conn.channel.encrypt(text.as_bytes())?;
    conn.writer
        .send(Message::binary(frame))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn remote_close(state: State<'_, Remote>) -> Result<(), String> {
    close_slot(&state.slot).await;
    Ok(())
}

/// This device's public key, base64url — what the host records when pairing.
#[tauri::command]
pub fn remote_device_public_key(app: AppHandle) -> Result<String, String> {
    Ok(device_key(&app)?.public_base64())
}

// --- internals ---------------------------------------------------------------

fn device_key(app: &AppHandle) -> Result<DeviceKey, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("no app data dir: {e}"))?;
    DeviceKey::load_or_create(&dir)
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
/// report how it ended — unless this connection has already been replaced, in
/// which case its successor owns the events.
fn spawn_reader(app: AppHandle, slot: Slot, mut reader: Reader, generation: u64) {
    tauri::async_runtime::spawn(async move {
        let mut code = CLOSE_ABNORMAL;
        let mut reason = String::new();
        while let Some(message) = reader.next().await {
            match message {
                Ok(Message::Binary(bytes)) => {
                    let plaintext = {
                        let mut guard = slot.lock().await;
                        match guard.as_mut() {
                            Some(conn) if conn.generation == generation => {
                                conn.channel.decrypt(&bytes)
                            }
                            // Replaced or closed while this frame was in
                            // flight: stay quiet, the live socket is someone
                            // else's.
                            _ => return,
                        }
                    };
                    match plaintext.and_then(text_of) {
                        Ok(text) => {
                            let _ = app.emit("remote:message", TextPayload { text });
                        }
                        Err(e) => {
                            let _ = app.emit("remote:error", ErrorPayload { message: e.clone() });
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
                            message: message.clone(),
                        },
                    );
                    reason = message;
                    break;
                }
            }
        }
        report_close(&app, &slot, generation, code, reason).await;
    });
}

fn text_of(plaintext: Vec<u8>) -> Result<String, String> {
    String::from_utf8(plaintext).map_err(|_| "frame is not UTF-8".to_string())
}

/// Hand the close to the webview, but only while this connection is still the
/// live one.
async fn report_close(app: &AppHandle, slot: &Slot, generation: u64, code: u16, reason: String) {
    let ours = {
        let mut guard = slot.lock().await;
        let ours = guard
            .as_ref()
            .is_some_and(|conn| conn.generation == generation);
        if ours {
            if let Some(mut conn) = guard.take() {
                let _ = conn.writer.close().await;
            }
        }
        ours
    };
    if ours {
        let _ = app.emit("remote:close", ClosePayload { code, reason });
    }
}

/// Close whatever is in the slot with 1000. Emits nothing: the caller asked
/// for it, and the reader task sees its generation is gone.
async fn close_slot(slot: &Slot) {
    let taken = slot.lock().await.take();
    if let Some(mut conn) = taken {
        let _ = conn
            .writer
            .send(Message::Close(Some(CloseFrame {
                code: CloseCode::Normal,
                reason: Default::default(),
            })))
            .await;
        let _ = conn.writer.close().await;
    }
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
