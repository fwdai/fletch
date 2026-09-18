//! A device's whole first conversation with a headless host, over the real
//! transport.
//!
//! This is the one test that proves the point of the binary: a `fletch-host`
//! booted the way `serve` boots it is a host the existing phone can pair with.
//! So nothing here is a stub — the engine is the engine, the listener is the
//! listener, the handshake is Noise `XX` driven by the same `fletch-proto` the
//! phone uses, and the admin socket is the one the CLI talks to. Only the
//! client is written out by hand, because the client is the phone.
//!
//! No agent is ever spawned: that would need a provider binary, and none of
//! what is under test here goes anywhere near one.

use std::time::Duration;

use fletch_core::host::HeadlessRelay;
use fletch_host::{admin, serve};
use fletch_proto::keys::StaticKey;
use fletch_proto::{dial, noise, Channel};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::{Bytes, Message};

/// A device revoked under a live connection is closed with this
/// (`docs/remote-protocol.md`, and `remote::session::CLOSE_REVOKED`).
const CLOSE_REVOKED: u16 = 4003;

/// Long enough that a loaded machine does not fail the test, short enough that
/// a genuine hang is a failure rather than a stalled suite.
const PATIENCE: Duration = Duration::from_secs(10);

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_pairs_with_a_headless_host_and_a_revoke_hangs_up_on_it() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().to_path_buf();

    serve::start(serve::Config {
        data_dir: data_dir.clone(),
        // An ephemeral port: the test asks the host which one it got, the same
        // way `status` does for a person.
        port: Some(0),
        relay: HeadlessRelay::Off,
        name: Some("test host".to_string()),
        // The handler is process-wide and calls `exit`; a test must not install
        // one.
        handle_signals: false,
    })
    .await
    .expect("the host should boot");

    // Everything the operator would do from a terminal goes over the socket.
    let status = admin::call(&data_dir, "status", json!({}))
        .await
        .expect("status");
    assert_eq!(status["listening"], json!(true), "{status}");
    assert_eq!(
        status["dataDir"],
        json!(data_dir.to_string_lossy()),
        "status carries the host's own data dir"
    );
    let port = status["port"].as_u64().expect("a bound port") as u16;
    let host_id = status["hostId"].as_str().expect("a host id").to_string();

    let invite = admin::call(&data_dir, "begin_pairing", json!({}))
        .await
        .expect("begin_pairing");
    let token = invite["token"].as_str().expect("a token").to_string();
    assert!(
        invite["url"]
            .as_str()
            .unwrap_or_default()
            .starts_with("fletch://pair?host="),
        "the invite carries the deep link the phone reads: {invite}"
    );

    // 1. Pair. A fresh device key, the host key pinned from the invite.
    let phone = StaticKey::generate().unwrap();
    let (mut ws, mut channel) = dial(port, &host_id, &phone).await;
    let paired = request(
        &mut ws,
        &mut channel,
        "1",
        "pair",
        json!({
            "token": token,
            "device": { "name": "test phone", "platform": "ios" },
        }),
    )
    .await
    .expect("pair");
    let device_id = paired["deviceId"]
        .as_str()
        .expect("pair returns the device id")
        .to_string();
    assert_eq!(
        paired["host"]["name"],
        json!("test host"),
        "the host answers with the name `serve --name` gave it: {paired}"
    );
    // Phase 0 of the multi-host plan adds `protocol: { version: 2, ops, events,
    // features }` to this result and is not on this branch, so the version is
    // asserted only if the field is there — the day it lands, this covers it.
    if let Some(protocol) = paired.get("protocol") {
        assert_eq!(protocol["version"], json!(2), "{paired}");
    }

    // 2. A second connection, as the phone makes on every launch after the
    //    first: `hello` on the key `pair` registered, then a snapshot op.
    let (mut ws, mut channel) = dial(port, &host_id, &phone).await;
    let hello = request(&mut ws, &mut channel, "2", "hello", json!({}))
        .await
        .expect("hello");
    assert_eq!(hello["host"]["name"], json!("test host"), "{hello}");
    assert!(
        hello.get("workspace").is_some(),
        "hello carries the workspace snapshot: {hello}"
    );
    let workspace = request(&mut ws, &mut channel, "3", "get_workspace", json!({}))
        .await
        .expect("get_workspace");
    assert!(
        workspace.get("projects").is_some() || workspace.is_object(),
        "get_workspace answers with the workspace: {workspace}"
    );

    // 3. Revoke from the socket. The live connection above must not survive it.
    admin::call(&data_dir, "revoke_device", json!({ "deviceId": device_id }))
        .await
        .expect("revoke_device");
    assert_eq!(
        close_code(&mut ws).await,
        Some(CLOSE_REVOKED),
        "a revoked device is hung up on with {CLOSE_REVOKED}"
    );
    assert!(
        admin::call(&data_dir, "devices_list", json!({}))
            .await
            .expect("devices_list")
            .as_array()
            .is_some_and(|devices| devices.is_empty()),
        "the revoked device is gone from the store"
    );
}

/// Open a connection and run the Noise handshake, pinning the host key the
/// pairing invite named — what `fletch_proto::noise::initiate` is for, and what
/// the phone's dialer does around it.
async fn dial(port: u16, host_id: &str, key: &StaticKey) -> (dial::Ws, Channel) {
    let mut ws = tokio::time::timeout(
        PATIENCE,
        dial::connect(&format!("ws://127.0.0.1:{port}/ws")),
    )
    .await
    .expect("the listener should accept a connection")
    .expect("the websocket upgrade should succeed");
    let channel = noise::initiate(&mut ws, key, Some(host_id))
        .await
        .expect("the handshake should succeed");
    (ws, channel)
}

/// Send one request and return its answer, skipping the event frames the host
/// starts pushing the moment a connection authenticates.
///
/// Frames have to be decrypted in the order they arrive — the channel's nonce
/// counter is the connection's, not the request's — so skipping means decrypt
/// and discard, never "leave it on the socket".
async fn request(
    ws: &mut dial::Ws,
    channel: &mut Channel,
    id: &str,
    op: &str,
    args: Value,
) -> Result<Value, String> {
    let frame = json!({ "id": id, "op": op, "args": args }).to_string();
    let frame = channel.encrypt_frame(frame.as_bytes()).unwrap();
    ws.send(Message::Binary(Bytes::from(frame)))
        .await
        .expect("the request should reach the host");

    tokio::time::timeout(PATIENCE, async {
        loop {
            let message = ws
                .next()
                .await
                .expect("the host should answer, not hang up")
                .expect("the socket should stay healthy");
            let Message::Binary(payload) = message else {
                // Pings are the transport's; nothing else is expected.
                continue;
            };
            let plaintext = channel
                .decrypt_frame(&payload)
                .expect("a decryptable frame");
            let value: Value = serde_json::from_slice(&plaintext).expect("a JSON frame");
            if value["id"] != json!(id) {
                continue;
            }
            return if value["ok"] == json!(true) {
                Ok(value["result"].clone())
            } else {
                Err(value["error"].as_str().unwrap_or_default().to_string())
            };
        }
    })
    .await
    .expect("the host should answer within the test's patience")
}

/// The close code the host hangs up with, reading past whatever else is still
/// in flight.
async fn close_code(ws: &mut dial::Ws) -> Option<u16> {
    tokio::time::timeout(PATIENCE, async {
        while let Some(Ok(message)) = ws.next().await {
            if let Message::Close(frame) = message {
                return frame.map(|frame| u16::from(frame.code));
            }
        }
        None
    })
    .await
    .expect("the host should close the connection")
}
