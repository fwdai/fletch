//! The WebSocket listener: one task per connection, requests dispatched
//! concurrently, events fanned out from the shared broadcast channel.
//!
//! Requests are answered off the reader loop on purpose. Some ops (`push_agent`,
//! `create_pr`) take tens of seconds, and a reader that awaited them inline
//! would stop reading pongs and hang up on a healthy phone mid-push.
//!
//! Everything a connection owns is bounded and ends with it: the outbound queue
//! holds `OUTBOUND_BUFFER` frames, at most `MAX_IN_FLIGHT` dispatches run at
//! once, and both the dispatch tasks and the registry entry are dropped on
//! every exit path out of `serve`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinSet;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tokio_tungstenite::tungstenite::{http, Bytes, Error as WsError, Message};
use tokio_tungstenite::WebSocketStream;

use super::auth::DeviceRecord;
use super::session::{CloseRequest, SessionGuard};
use super::{RemoteState, WS_PATH};

/// Largest frame/message the host accepts. tungstenite answers anything larger
/// with close code 1009 on its own.
const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
/// Ping cadence, and how many may go unanswered before the socket is dropped.
const PING_INTERVAL: Duration = Duration::from_secs(20);
const MAX_MISSED_PONGS: u32 = 2;
/// How long a closing connection keeps reading (and discarding) the peer's
/// bytes before the socket is dropped. See `drain`.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);
/// Frames one connection may have queued but not yet written. A control panel
/// reads its own responses; a peer that lets this many pile up is either gone
/// or not reading, and the session is dropped rather than buffered for.
const OUTBOUND_BUFFER: usize = 64;
/// Requests one connection may have in flight at once. The phone issues a
/// handful per screen; past this the host answers `TOO_MANY_IN_FLIGHT` without
/// dispatching, so a flood costs a response instead of a task and a `Supervisor`
/// call each.
const MAX_IN_FLIGHT: usize = 8;

/// The first frame was neither `pair` nor `hello`.
const CLOSE_BAD_FIRST_FRAME: CloseCode = CloseCode::Library(4001);
/// Unauthenticated, or a bad/revoked credential.
const CLOSE_UNAUTHENTICATED: CloseCode = CloseCode::Library(4003);
/// A frame past `MAX_FRAME_BYTES`. tungstenite surfaces this as a capacity
/// error without closing, so the host sends the RFC's 1009 itself — the phone
/// has no client-side cap and relies on this code to know what happened.
const CLOSE_TOO_LARGE: CloseCode = CloseCode::Size;

type Ws = WebSocketStream<TcpStream>;

pub(super) async fn accept_loop(
    state: Arc<RemoteState>,
    listener: TcpListener,
    mut shutdown: broadcast::Receiver<()>,
) {
    loop {
        tokio::select! {
            _ = shutdown.recv() => return,
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    // Interactive control traffic: small frames, latency over
                    // throughput.
                    let _ = stream.set_nodelay(true);
                    tokio::spawn(serve(state.clone(), stream, peer));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "remote: accept failed");
                }
            },
        }
    }
}

async fn serve(state: Arc<RemoteState>, stream: TcpStream, peer: SocketAddr) {
    let mut config = WebSocketConfig::default();
    config.max_message_size = Some(MAX_FRAME_BYTES);
    config.max_frame_size = Some(MAX_FRAME_BYTES);
    let ws = match tokio_tungstenite::accept_hdr_async_with_config(stream, check_path, Some(config))
        .await
    {
        Ok(ws) => ws,
        Err(e) => {
            tracing::debug!(error = %e, %peer, "remote: handshake rejected");
            return;
        }
    };

    let (sink, stream) = ws.split();
    let (tx, rx) = mpsc::channel::<Message>(OUTBOUND_BUFFER);
    let (close_tx, close_rx) = mpsc::channel::<CloseRequest>(1);
    // Registered before the first frame, so turning remote access off closes a
    // socket that is still handshaking rather than leaving it free to pair
    // against a stopped listener. The guard deregisters the session when it
    // drops, which is every way out of this function.
    let session = state.sessions().register(close_tx.clone());
    let out = Outbox {
        tx,
        close: close_tx,
    };
    let writer = tokio::spawn(pump(sink, rx));

    let stream = read_loop(&state, stream, &out, close_rx, &session, peer).await;

    // Dropping the guard and the outbox releases the last senders, which lets
    // the writer flush whatever is queued (a close frame last) and hand the
    // sink back, so the socket can be drained before drop.
    drop(session);
    drop(out);
    if let Ok(sink) = writer.await {
        if let Ok(ws) = stream.reunite(sink) {
            drain(ws).await;
        }
    }
}

/// Read and discard whatever the peer is still sending, briefly, before the
/// socket is dropped.
///
/// Dropping a TCP socket that still has unread inbound bytes makes the kernel
/// send RST instead of FIN, and a reset discards anything the peer had not yet
/// read — including the close frame that carries the code. The case that
/// matters is the oversized frame: tungstenite rejects it on the header, so
/// the host is closing while the phone is still writing the body, and without
/// this the phone sees a broken pipe rather than 1009. The same applies, less
/// often, to a client that keeps talking past a 4003. Raw reads, not WebSocket
/// frames: after a capacity error the remaining bytes are payload, not frames.
async fn drain(mut ws: Ws) {
    use tokio::io::AsyncReadExt;
    let tcp = ws.get_mut();
    let mut buf = [0u8; 16 * 1024];
    let _ = tokio::time::timeout(DRAIN_TIMEOUT, async {
        while let Ok(n) = tcp.read(&mut buf).await {
            if n == 0 {
                break;
            }
        }
    })
    .await;
}

/// Reject anything but `/ws` at the handshake, so a stray browser hitting the
/// port gets a 404 instead of an open socket.
///
/// The signature is tungstenite's `Callback` contract, so the large `Err`
/// (`http::Response`) cannot be boxed away.
#[allow(clippy::result_large_err)]
fn check_path(req: &Request, response: Response) -> std::result::Result<Response, ErrorResponse> {
    if req.uri().path() == WS_PATH {
        return Ok(response);
    }
    let mut err = ErrorResponse::new(Some(format!("only {WS_PATH} is served")));
    *err.status_mut() = http::StatusCode::NOT_FOUND;
    Err(err)
}

/// One connection's outbound side: the bounded queue plus the switch that ends
/// the session when that queue can no longer be written to.
///
/// Handed to the dispatch and event-forwarding tasks, which have no other way
/// to stop a connection whose peer has stopped reading.
#[derive(Clone)]
struct Outbox {
    tx: mpsc::Sender<Message>,
    close: mpsc::Sender<CloseRequest>,
}

impl Outbox {
    /// Queue a message. `false` means the session is over — the socket is gone,
    /// or the peer let `OUTBOUND_BUFFER` frames pile up unread — and the
    /// connection has been asked to close, so a caller in a loop should stop.
    ///
    /// A backpressure kill asks for a close with no frame: the queue a close
    /// frame would travel through is exactly what is full.
    fn send(&self, msg: Message) -> bool {
        if self.tx.try_send(msg).is_ok() {
            return true;
        }
        let _ = self.close.try_send(None);
        false
    }
}

/// Serialize the outbound queue onto the socket. A `Close` is the last thing
/// written; anything queued behind it is dropped. Hands the sink back so the
/// caller can reunite it with the reader and drain the socket before drop.
async fn pump(
    mut sink: SplitSink<Ws, Message>,
    mut rx: mpsc::Receiver<Message>,
) -> SplitSink<Ws, Message> {
    while let Some(msg) = rx.recv().await {
        let closing = matches!(msg, Message::Close(_));
        if sink.send(msg).await.is_err() {
            return sink;
        }
        if closing {
            let _ = sink.close().await;
            return sink;
        }
    }
    let _ = sink.flush().await;
    sink
}

/// The connection's state machine. Returns the reader half so the socket can be
/// reunited and drained.
async fn read_loop(
    state: &Arc<RemoteState>,
    mut stream: futures_util::stream::SplitStream<Ws>,
    out: &Outbox,
    mut close_rx: mpsc::Receiver<CloseRequest>,
    session: &SessionGuard,
    peer: SocketAddr,
) -> futures_util::stream::SplitStream<Ws> {
    let mut authenticated = false;
    let mut event_task: Option<tokio::task::JoinHandle<()>> = None;
    // Owns the dispatch tasks, so they are aborted when this loop ends instead
    // of outliving the socket they were going to answer on.
    let mut in_flight: JoinSet<()> = JoinSet::new();
    let mut first_frame = true;
    let mut missed_pongs = 0u32;
    let mut ping = tokio::time::interval(PING_INTERVAL);
    // `interval` fires immediately; the first real ping belongs one period out.
    ping.tick().await;

    loop {
        tokio::select! {
            request = close_rx.recv() => {
                // The host is hanging up: remote access was disabled, this
                // device was revoked, or the outbound queue backed up (which
                // is the `None` case — no frame can be written).
                if let Some(Some(reason)) = request {
                    out.send(close_frame(CloseCode::Library(reason.code), reason.reason));
                }
                break;
            }
            _ = ping.tick() => {
                if missed_pongs >= MAX_MISSED_PONGS {
                    tracing::debug!(%peer, "remote: pong timeout");
                    break;
                }
                missed_pongs += 1;
                if !out.send(Message::Ping(Bytes::new())) {
                    break;
                }
            }
            incoming = stream.next() => {
                let msg = match incoming {
                    Some(Ok(msg)) => msg,
                    Some(Err(WsError::Capacity(e))) => {
                        tracing::debug!(error = %e, %peer, "remote: frame over the 4 MiB cap");
                        out.send(close_frame(CLOSE_TOO_LARGE, "frame too large"));
                        break;
                    }
                    // A protocol error or a dropped socket: nothing useful to
                    // say back, and the peer is already gone or misbehaving.
                    Some(Err(_)) | None => break,
                };
                match msg {
                    Message::Pong(_) => missed_pongs = 0,
                    Message::Close(_) => break,
                    Message::Binary(_) => {
                        // The protocol is text-only; a binary first frame is a
                        // client that does not speak it at all.
                        if first_frame {
                            out.send(close_frame(CLOSE_BAD_FIRST_FRAME, "expected pair or hello"));
                            break;
                        }
                    }
                    Message::Text(text) => {
                        let frame: RequestFrame = match serde_json::from_str(&text) {
                            Ok(frame) => frame,
                            Err(e) => {
                                if first_frame {
                                    out.send(close_frame(CLOSE_BAD_FIRST_FRAME, "expected pair or hello"));
                                    break;
                                }
                                // No id to answer with; the client will time its
                                // own request out.
                                tracing::debug!(error = %e, %peer, "remote: unparseable frame");
                                continue;
                            }
                        };
                        let handshake = frame.op == "pair" || frame.op == "hello";
                        if first_frame && !handshake {
                            out.send(close_frame(CLOSE_BAD_FIRST_FRAME, "expected pair or hello"));
                            break;
                        }
                        first_frame = false;

                        if !authenticated {
                            if !handshake {
                                out.send(close_frame(CLOSE_UNAUTHENTICATED, "hello required"));
                                break;
                            }
                            match authenticate(state, &frame).await {
                                Some((record, result)) => {
                                    // Subscribe before the reply goes out: an
                                    // event emitted in the gap would otherwise
                                    // be lost, and the phone would render a
                                    // snapshot it can't see the next change to.
                                    event_task = Some(spawn_event_forwarder(state, out.clone()));
                                    // Makes this socket revocable and visible
                                    // as `connected`.
                                    session.bind_device(&record.device_id);
                                    // The credential could have been revoked
                                    // between `verify` above and the line
                                    // above, in which case `revoke_device`
                                    // found no session to close and this is
                                    // where that is caught.
                                    if !state.devices().contains(&record.device_id) {
                                        out.send(close_frame(CLOSE_UNAUTHENTICATED, "bad credential"));
                                        break;
                                    }
                                    state.devices().touch(&record.device_id);
                                    // Reply last: the phone must not be able to
                                    // observe itself as authenticated before it
                                    // is on the fan-out and in the registry.
                                    out.send(text_frame(ok_frame(&frame.id, result)));
                                    tracing::info!(
                                        device = %record.name,
                                        platform = %record.platform,
                                        "remote: device authenticated"
                                    );
                                    authenticated = true;
                                }
                                None => {
                                    out.send(close_frame(CLOSE_UNAUTHENTICATED, "bad credential"));
                                    break;
                                }
                            }
                            continue;
                        }

                        if handshake {
                            // Already authenticated: the handshake ops are not
                            // part of the dispatchable surface.
                            out.send(text_frame(err_frame(&frame.id, super::dispatch::UNKNOWN_OP)));
                            continue;
                        }

                        // Reap finished dispatches before counting, so the cap
                        // is on work actually outstanding.
                        while in_flight.try_join_next().is_some() {}
                        if in_flight.len() >= MAX_IN_FLIGHT {
                            out.send(text_frame(err_frame(&frame.id, super::dispatch::TOO_MANY_IN_FLIGHT)));
                            continue;
                        }

                        let state = state.clone();
                        let out = out.clone();
                        let RequestFrame { id, op, args } = frame;
                        in_flight.spawn(async move {
                            let outcome = state.dispatch(&op, args).await;
                            let frame = match outcome {
                                Ok(result) => ok_frame(&id, result),
                                Err(error) => err_frame(&id, &error),
                            };
                            out.send(text_frame(frame));
                        });
                    }
                    _ => {}
                }
            }
        }
    }

    // Nothing this connection started may outlive it. Git ops run inside
    // `spawn_blocking` and run to completion regardless; aborting only stops
    // the host from waiting for them and from answering into a dead socket.
    in_flight.shutdown().await;
    if let Some(task) = event_task {
        task.abort();
        // Awaited so the task's `Outbox` clone is gone before the caller drops
        // its own and waits for the writer.
        let _ = task.await;
    }
    stream
}

/// Forward the shared event stream to one connection. Owned by a task rather
/// than the reader's `select!` so a lagging phone can't stall request handling.
fn spawn_event_forwarder(state: &Arc<RemoteState>, out: Outbox) -> tokio::task::JoinHandle<()> {
    // Subscribed here, not inside the task, so the connection is on the
    // fan-out the instant this returns.
    let mut rx = state.subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(frame) => {
                    if !out.send(Message::Text(frame.as_ref().into())) {
                        return;
                    }
                }
                // Best-effort delivery by contract: the phone refetches on
                // reconnect and on returning to the foreground.
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::debug!(dropped = n, "remote: event fan-out lagged")
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    })
}

/// Resolve a `pair`/`hello` frame into an authenticated device plus the result
/// to answer with. `None` means close code 4003.
async fn authenticate(
    state: &Arc<RemoteState>,
    frame: &RequestFrame,
) -> Option<(DeviceRecord, Value)> {
    let host = super::host_info();
    match frame.op.as_str() {
        "pair" => {
            let args: PairArgs = serde_json::from_value(frame.args.clone()).ok()?;
            if !state.pairing().consume(&args.token) {
                return None;
            }
            let info = args.device.unwrap_or_default();
            let (record, token) = state
                .devices()
                .register(&info.name(), &info.platform())
                .map_err(|e| tracing::warn!(error = %e, "remote: pairing failed"))
                .ok()?;
            // No snapshot here, by contract: `pair` only authenticates and
            // starts the event stream, and the client issues its own
            // `get_workspace` next. Only `hello` carries a snapshot.
            let result = json!({
                "deviceId": record.device_id,
                "deviceToken": token,
                "host": host,
            });
            Some((record, result))
        }
        "hello" => {
            let args: HelloArgs = serde_json::from_value(frame.args.clone()).ok()?;
            let record = state.devices().verify(&args.device_token)?;
            let workspace = state
                .dispatch("get_workspace", json!({}))
                .await
                .unwrap_or(Value::Null);
            let result = json!({ "host": host, "workspace": workspace });
            Some((record, result))
        }
        _ => None,
    }
}

fn text_frame(frame: String) -> Message {
    Message::Text(frame.into())
}

fn close_frame(code: CloseCode, reason: &str) -> Message {
    Message::Close(Some(CloseFrame {
        code,
        reason: reason.into(),
    }))
}

fn ok_frame(id: &str, result: Value) -> String {
    json!({ "id": id, "ok": true, "result": result }).to_string()
}

fn err_frame(id: &str, error: &str) -> String {
    json!({ "id": id, "ok": false, "error": error }).to_string()
}

#[derive(Deserialize)]
struct RequestFrame {
    id: String,
    op: String,
    #[serde(default = "empty_args")]
    args: Value,
}

fn empty_args() -> Value {
    json!({})
}

#[derive(Deserialize)]
struct PairArgs {
    token: String,
    #[serde(default)]
    device: Option<ClientInfo>,
}

/// `hello`'s documented `client` object is accepted and ignored: the device
/// record already carries the name and platform from `pair`, and serde drops
/// unknown keys.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HelloArgs {
    device_token: String,
}

#[derive(Deserialize, Default)]
struct ClientInfo {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    platform: Option<String>,
}

impl ClientInfo {
    fn name(&self) -> String {
        self.name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| "Unnamed device".to_string())
    }

    fn platform(&self) -> String {
        self.platform
            .clone()
            .unwrap_or_else(|| "unknown".to_string())
    }
}
