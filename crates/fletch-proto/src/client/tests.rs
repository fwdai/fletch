// The dialer against a real host end: a loopback listener that runs `respond`,
// so every test below goes through a genuine WebSocket upgrade, a genuine XX
// handshake and the real frame codec. What is being tested is the part that is
// new — that the connections are independent — so nothing here mocks the wire.

use super::*;
use crate::noise::respond;
use std::net::Ipv4Addr;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::{accept_async, WebSocketStream};

/// The host's end of one accepted connection: the Noise channel and the
/// socket, so a test can push a frame at the dialer and read what it sent.
struct HostEnd {
    ws: WebSocketStream<TcpStream>,
    channel: Channel,
}

impl HostEnd {
    async fn send(&mut self, text: &str) {
        let frame = self.channel.encrypt_frame(text.as_bytes()).unwrap();
        self.ws.send(Message::binary(frame)).await.unwrap();
    }

    /// The next plaintext the dialer sent on this connection.
    async fn next_text(&mut self) -> String {
        loop {
            match self.ws.next().await.unwrap().unwrap() {
                Message::Binary(bytes) => {
                    let plaintext = self.channel.decrypt_frame(&bytes).unwrap();
                    return String::from_utf8(plaintext).unwrap();
                }
                Message::Ping(_) | Message::Pong(_) => continue,
                other => panic!("expected a binary frame, got {other:?}"),
            }
        }
    }

    /// The close code the dialer sent, once it sends one.
    async fn next_close(&mut self) -> u16 {
        loop {
            match self.ws.next().await.unwrap().unwrap() {
                Message::Close(frame) => return frame.map(|f| f.code.into()).unwrap_or(1005),
                _ => continue,
            }
        }
    }

    async fn close_with(&mut self, code: u16, reason: &str) {
        self.ws
            .send(Message::Close(Some(CloseFrame {
                code: CloseCode::from(code),
                reason: reason.to_string().into(),
            })))
            .await
            .unwrap();
        // The close frame above is the handshake; `close` only flushes it, and
        // tungstenite calls that "send after closing".
        let _ = self.ws.close(None).await;
    }
}

/// Listen on loopback, and hand back every accepted connection once its
/// handshake is done. One listener serves as many connections as a test dials.
async fn host(key: Arc<StaticKey>) -> (String, mpsc::UnboundedReceiver<HostEnd>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let url = format!("ws://{}/ws", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let key = key.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut ws = accept_async(stream).await.unwrap();
                let (channel, _device) = respond(&mut ws, &key).await.unwrap();
                let _ = tx.send(HostEnd { ws, channel });
            });
        }
    });
    (url, rx)
}

/// A dialer with its key in a throwaway directory, and the events it emits.
fn dialer() -> (
    Arc<Dialer>,
    mpsc::UnboundedReceiver<ClientEvent>,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    let dialer = Dialer::new(
        dir.path(),
        Box::new(move |event| {
            let _ = tx.send(event);
        }),
    );
    (dialer, rx, dir)
}

fn target(url: &str, host_key: Option<String>) -> Target {
    Target {
        url: url.to_string(),
        host_key,
        timeout_ms: Some(10_000),
    }
}

/// The next event, which must be a message.
async fn next_message(events: &mut mpsc::UnboundedReceiver<ClientEvent>) -> TextPayload {
    match events.recv().await.unwrap() {
        ClientEvent::Message(payload) => payload,
        ClientEvent::Error(e) => panic!("unexpected error event: {}", e.message),
        ClientEvent::Close(c) => panic!("unexpected close event: {} {}", c.code, c.reason),
    }
}

/// The point of the module: two hosts at once, each hearing and saying only its
/// own. A second `connect` used to supersede the first.
#[tokio::test]
async fn two_connections_are_independent_and_carry_their_own_ids() {
    let key = Arc::new(StaticKey::generate().unwrap());
    let (url, mut accepted) = host(key.clone()).await;
    let (dialer, mut events, _dir) = dialer();

    let first = dialer.connect(target(&url, None)).await.unwrap();
    let mut host_a = accepted.recv().await.unwrap();
    let second = dialer
        .connect(target(&url, Some(key.public_base64())))
        .await
        .unwrap();
    let mut host_b = accepted.recv().await.unwrap();

    assert_ne!(first.connection_id, second.connection_id);
    assert_eq!(first.host_key, key.public_base64());
    assert_eq!(second.host_key, key.public_base64());

    // Sends land on the socket named by the id, and nowhere else.
    dialer
        .send(first.connection_id, "{\"to\":\"a\"}")
        .await
        .unwrap();
    assert_eq!(host_a.next_text().await, "{\"to\":\"a\"}");
    dialer
        .send(second.connection_id, "{\"to\":\"b\"}")
        .await
        .unwrap();
    assert_eq!(host_b.next_text().await, "{\"to\":\"b\"}");

    // And each host's frames arrive stamped with that host's connection.
    host_b.send("{\"from\":\"b\"}").await;
    let from_b = next_message(&mut events).await;
    assert_eq!(from_b.connection_id, second.connection_id);
    assert_eq!(from_b.text, "{\"from\":\"b\"}");
    host_a.send("{\"from\":\"a\"}").await;
    let from_a = next_message(&mut events).await;
    assert_eq!(from_a.connection_id, first.connection_id);
    assert_eq!(from_a.text, "{\"from\":\"a\"}");
}

#[tokio::test]
async fn closing_one_connection_leaves_the_other_alone() {
    let key = Arc::new(StaticKey::generate().unwrap());
    let (url, mut accepted) = host(key.clone()).await;
    let (dialer, mut events, _dir) = dialer();

    let first = dialer.connect(target(&url, None)).await.unwrap();
    let mut host_a = accepted.recv().await.unwrap();
    let second = dialer.connect(target(&url, None)).await.unwrap();
    let mut host_b = accepted.recv().await.unwrap();

    dialer.close(first.connection_id).await.unwrap();
    assert_eq!(host_a.next_close().await, u16::from(CloseCode::Normal));
    assert_eq!(
        dialer.send(first.connection_id, "gone").await.unwrap_err(),
        "not connected"
    );
    // Closing an id twice, or one that was never handed out, is a no-op.
    dialer.close(first.connection_id).await.unwrap();
    dialer.close(9999).await.unwrap();

    dialer
        .send(second.connection_id, "still here")
        .await
        .unwrap();
    assert_eq!(host_b.next_text().await, "still here");
    host_b.send("and so are you").await;
    let heard = next_message(&mut events).await;
    assert_eq!(heard.connection_id, second.connection_id);
    assert_eq!(heard.text, "and so are you");
}

/// A close the app asked for is its own business; a close the host sent has to
/// reach the app, with the host's code and the right id on it.
#[tokio::test]
async fn a_close_from_the_host_is_reported_against_that_connection() {
    let key = Arc::new(StaticKey::generate().unwrap());
    let (url, mut accepted) = host(key.clone()).await;
    let (dialer, mut events, _dir) = dialer();

    let first = dialer.connect(target(&url, None)).await.unwrap();
    let mut host_a = accepted.recv().await.unwrap();
    let second = dialer.connect(target(&url, None)).await.unwrap();
    let _host_b = accepted.recv().await.unwrap();

    host_a.close_with(4003, "device revoked").await;
    let ClientEvent::Close(payload) = events.recv().await.unwrap() else {
        panic!("expected a close event");
    };
    assert_eq!(payload.connection_id, first.connection_id);
    assert_eq!(payload.code, 4003);
    assert_eq!(payload.reason, "device revoked");

    // The other connection is untouched by its neighbour's close.
    dialer.send(second.connection_id, "alive").await.unwrap();
}

/// The one failure the caller distinguishes: it stops the candidate list
/// instead of quietly moving the client to another path.
#[tokio::test]
async fn a_host_key_mismatch_surfaces_as_such_and_registers_nothing() {
    let key = Arc::new(StaticKey::generate().unwrap());
    let (url, _accepted) = host(key.clone()).await;
    let (dialer, _events, _dir) = dialer();

    let impostor = StaticKey::generate().unwrap().public_base64();
    let err = dialer
        .connect(target(&url, Some(impostor)))
        .await
        .unwrap_err();
    assert!(err.contains(crate::noise::HOST_KEY_MISMATCH), "{err}");
    assert!(err.contains(&key.public_base64()), "{err}");
    // The id was claimed but nothing was registered under it, so the app cannot
    // talk to a host whose identity was refused.
    assert_eq!(dialer.send(1, "hello").await.unwrap_err(), "not connected");
}

#[tokio::test]
async fn an_unreachable_url_fails_without_registering_a_connection() {
    let (dialer, _events, _dir) = dialer();
    // A port that was ours a moment ago and is now closed: refused.
    let dead = {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.local_addr().unwrap()
    };
    let err = dialer
        .connect(target(&format!("ws://{dead}/ws"), None))
        .await
        .unwrap_err();
    assert!(err.contains("cannot reach"), "{err}");
    assert_eq!(dialer.send(1, "hello").await.unwrap_err(), "not connected");
}

/// First use from several callers at once must yield one identity and one file:
/// `connect` and `device_public_key` both go through `Dialer::identity`, which
/// is what makes that true.
#[test]
fn concurrent_first_use_settles_on_one_identity() {
    let dir = tempfile::tempdir().unwrap();
    let dialer = Dialer::new(dir.path(), Box::new(|_| {}));

    let workers: Vec<_> = (0..8)
        .map(|_| {
            let dialer = dialer.clone();
            std::thread::spawn(move || dialer.device_public_key().unwrap())
        })
        .collect();
    let seen: std::collections::HashSet<String> =
        workers.into_iter().map(|w| w.join().unwrap()).collect();

    assert_eq!(seen.len(), 1, "every caller got the same identity");
    let persisted = StaticKey::load_or_create(dir.path(), DEVICE_KEY_FILE)
        .unwrap()
        .public_base64();
    assert!(seen.contains(&persisted), "and it is the one on disk");
    assert!(!dir.path().join("device_key.tmp").exists());
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

/// `within` is what makes the budget real: a future that has not finished is
/// dropped, which is what closes a half-open socket.
#[tokio::test]
async fn within_bounds_a_future_and_passes_one_through_unbounded() {
    let deadline = Some(Instant::now() + Duration::from_millis(5));
    assert_eq!(within(deadline, async { 7 }).await, Some(7));
    assert_eq!(
        within(deadline, tokio::time::sleep(Duration::from_secs(30))).await,
        None,
        "an overrunning future is abandoned"
    );
    assert_eq!(within(None, async { 7 }).await, Some(7));
}

/// The three event names and the payload keys are the contract the webview
/// reads; both apps emit these strings and nothing else.
#[test]
fn event_names_and_payload_keys_are_the_ones_the_app_listens_for() {
    let message = ClientEvent::Message(TextPayload {
        connection_id: 7,
        text: "hi".into(),
    });
    let close = ClientEvent::Close(ClosePayload {
        connection_id: 7,
        code: 1000,
        reason: "bye".into(),
    });
    let error = ClientEvent::Error(ErrorPayload {
        connection_id: 7,
        message: "oops".into(),
    });
    assert_eq!(message.name(), "remote:message");
    assert_eq!(close.name(), "remote:close");
    assert_eq!(error.name(), "remote:error");

    let ClientEvent::Message(payload) = message else {
        unreachable!()
    };
    assert_eq!(
        serde_json::to_value(&payload).unwrap(),
        serde_json::json!({ "connectionId": 7, "text": "hi" })
    );
    let ClientEvent::Close(payload) = close else {
        unreachable!()
    };
    assert_eq!(
        serde_json::to_value(&payload).unwrap(),
        serde_json::json!({ "connectionId": 7, "code": 1000, "reason": "bye" })
    );
    let ClientEvent::Error(payload) = error else {
        unreachable!()
    };
    assert_eq!(
        serde_json::to_value(&payload).unwrap(),
        serde_json::json!({ "connectionId": 7, "message": "oops" })
    );
    assert_eq!(
        serde_json::to_value(ConnectResult {
            host_key: "k".into(),
            connection_id: 7,
        })
        .unwrap(),
        serde_json::json!({ "hostKey": "k", "connectionId": 7 })
    );
}
