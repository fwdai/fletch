use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::{Bytes, Message};
use tokio_tungstenite::WebSocketStream;

use super::auth::DeviceRecord;
use super::dispatch::{self, Dispatch, DispatchFuture, TOO_MANY_IN_FLIGHT, UNKNOWN_OP};
use super::secure::{self, SecureChannel};
use super::{DeviceStore, PairingTokens, RemoteState};

// ---------------------------------------------------------------------------
// Pairing tokens
// ---------------------------------------------------------------------------

#[test]
fn pairing_token_uses_the_documented_alphabet() {
    let minted = PairingTokens::new().mint();
    assert_eq!(minted.token.len(), 8);
    assert!(
        minted
            .token
            .chars()
            .all(|c| c.is_ascii_uppercase() || ('2'..='9').contains(&c)),
        "token {} left the A-Z2-9 alphabet",
        minted.token
    );
}

#[test]
fn pairing_token_is_single_use() {
    let tokens = PairingTokens::new();
    let minted = tokens.mint();
    assert!(tokens.consume(&minted.token));
    assert!(!tokens.consume(&minted.token));
}

#[test]
fn pairing_token_expires() {
    let tokens = PairingTokens::new();
    let minted = tokens.mint_with_ttl(Duration::ZERO);
    assert!(!tokens.consume(&minted.token));
}

#[test]
fn unminted_pairing_token_is_rejected() {
    assert!(!PairingTokens::new().consume("AAAAAAAA"));
}

#[test]
fn minting_does_not_invalidate_an_outstanding_token() {
    let tokens = PairingTokens::new();
    let first = tokens.mint();
    let second = tokens.mint();
    assert!(tokens.consume(&first.token));
    assert!(tokens.consume(&second.token));
}

// ---------------------------------------------------------------------------
// Device identities
// ---------------------------------------------------------------------------

/// A distinct static key per test record. The bytes need not be a real
/// curve point for the store's purposes — it only ever compares them.
fn key(seed: u8) -> [u8; 32] {
    let mut key = [seed; 32];
    key[0] = seed;
    key[1] = seed.wrapping_mul(7);
    key
}

#[test]
fn device_key_is_found_until_revoked() {
    let dir = tempfile::tempdir().unwrap();
    let store = DeviceStore::load(dir.path());
    let record = store.register("Alex's iPhone", "ios", &key(1)).unwrap();

    assert_eq!(record.public_key.len(), 43, "32 bytes as base64url, no pad");
    let found = store.find_by_key(&key(1)).expect("a paired key is found");
    assert_eq!(found.device_id, record.device_id);

    assert!(store.revoke(&record.device_id).unwrap());
    assert!(
        store.find_by_key(&key(1)).is_none(),
        "a revoked key still resolves"
    );
    assert!(
        !store.revoke(&record.device_id).unwrap(),
        "revoke is idempotent"
    );
}

#[test]
fn unknown_device_key_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let store = DeviceStore::load(dir.path());
    store.register("phone", "ios", &key(1)).unwrap();
    assert!(store.find_by_key(&key(2)).is_none());
}

#[test]
fn pairing_the_same_key_twice_updates_one_record() {
    let dir = tempfile::tempdir().unwrap();
    let store = DeviceStore::load(dir.path());
    let first = store.register("iPhone", "ios", &key(1)).unwrap();
    let second = store.register("Renamed iPhone", "ios", &key(1)).unwrap();

    assert_eq!(store.list().len(), 1, "one device, one record");
    assert_eq!(second.device_id, first.device_id, "the id is kept");
    assert_eq!(store.list()[0].name, "Renamed iPhone");
}

#[test]
fn devices_json_holds_the_public_key_and_is_owner_only() {
    let dir = tempfile::tempdir().unwrap();
    let store = DeviceStore::load(dir.path());
    let record = store.register("phone", "ios", &key(1)).unwrap();

    let path = dir.path().join("devices.json");
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.contains(&record.public_key));
    assert!(!raw.contains("tokenHash"), "no v1 credential is written");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "devices.json should not be group/world readable"
        );
    }
}

#[test]
fn devices_survive_a_reload() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = DeviceStore::load(dir.path());
        store.register("phone", "ios", &key(1)).unwrap();
    }
    let reopened = DeviceStore::load(dir.path());
    assert!(reopened.find_by_key(&key(1)).is_some());
    assert_eq!(reopened.list().len(), 1);
}

/// Token-era records carry a `tokenHash` and no `publicKey`: they cannot
/// authenticate anything under v2, so the doc has them dropped at load and the
/// device pairs again.
#[test]
fn token_era_records_are_dropped_at_load() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("devices.json"),
        serde_json::to_vec(&json!([
            {
                "deviceId": "v1",
                "name": "old phone",
                "platform": "ios",
                "tokenHash": "beef",
                "createdAt": "2026-01-01T00:00:00Z",
                "lastSeenAt": null,
            },
            {
                "deviceId": "v2",
                "name": "new phone",
                "platform": "ios",
                "publicKey": super::secure::encode_key(&key(1)),
                "createdAt": "2026-01-02T00:00:00Z",
                "lastSeenAt": null,
            },
        ]))
        .unwrap(),
    )
    .unwrap();

    let store = DeviceStore::load(dir.path());
    let ids: Vec<String> = store.list().into_iter().map(|d| d.device_id).collect();
    assert_eq!(ids, vec!["v2".to_string()]);
    assert!(store.find_by_key(&key(1)).is_some());
}

#[test]
fn concurrent_writers_leave_the_file_agreeing_with_memory() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DeviceStore::load(dir.path()));

    // Every writer persists the whole list, so two of them racing used to be
    // able to rename each other's temp file and land out of order —
    // resurrecting a revoked credential at the next launch.
    let workers: Vec<_> = (0..8u8)
        .map(|w| {
            let store = store.clone();
            std::thread::spawn(move || {
                for i in 0..20u8 {
                    // A distinct key per record: the same key would update one
                    // record rather than adding another.
                    let mut public = [0u8; 32];
                    public[0] = w;
                    public[1] = i;
                    let record = store
                        .register(&format!("phone-{w}-{i}"), "ios", &public)
                        .unwrap();
                    store.touch(&record.device_id);
                    if i % 2 == 0 {
                        assert!(store.revoke(&record.device_id).unwrap());
                    }
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }

    let in_memory: Vec<String> = store.list().into_iter().map(|d| d.device_id).collect();
    let raw = std::fs::read(dir.path().join("devices.json")).unwrap();
    let on_disk: Vec<String> = serde_json::from_slice::<Vec<DeviceRecord>>(&raw)
        .unwrap()
        .into_iter()
        .map(|d| d.device_id)
        .collect();

    assert_eq!(in_memory.len(), 8 * 10, "half of each worker's are revoked");
    assert_eq!(on_disk, in_memory, "devices.json diverged from memory");
    assert!(
        !dir.path().join("devices.json.tmp").exists(),
        "a temp file outlived its rename"
    );
}

#[test]
fn an_unusable_device_store_becomes_a_status_error() {
    let dir = tempfile::tempdir().unwrap();
    // A path already occupied by a regular file: `create_dir_all` cannot make
    // the store directory, which used to leave `RemoteState` unmanaged and
    // panic `remote_status` the moment Settings opened.
    let occupied = dir.path().join("remote");
    std::fs::write(&occupied, b"not a directory").unwrap();

    let state = RemoteState::new(&occupied, Arc::new(StubDispatch));
    let status = state.status();
    assert!(
        status.error.is_some(),
        "an unusable device store must surface an error"
    );
    assert!(status.devices.is_empty());
    assert!(!status.listening);
    assert_eq!(
        status.host_id, "",
        "no host key could be written, so there is no identity to advertise"
    );
}

/// The host identity is generated once and kept: regenerating it would silently
/// invalidate every pairing.
#[test]
fn the_host_key_persists_across_state_and_is_owner_only() {
    let dir = tempfile::tempdir().unwrap();
    let first = RemoteState::new(dir.path(), Arc::new(StubDispatch)).status();
    let second = RemoteState::new(dir.path(), Arc::new(StubDispatch)).status();

    assert_eq!(first.host_id.len(), 43, "43 base64url chars, no padding");
    assert_eq!(first.host_id, second.host_id);
    assert_eq!(first.error, None);

    let path = dir.path().join("host_key");
    assert_eq!(std::fs::read(&path).unwrap().len(), 32);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the host private key must be owner-only");
    }
}

#[test]
fn the_pairing_url_carries_the_host_id_and_an_address() {
    let dir = tempfile::tempdir().unwrap();
    let state = RemoteState::new(dir.path(), Arc::new(StubDispatch));
    let invite = state.begin_pairing();
    let host_id = state.status().host_id;

    assert!(
        invite.url.starts_with("fletch://pair?host="),
        "unexpected url {}",
        invite.url
    );
    assert!(invite.url.contains(&format!("host={host_id}")));
    assert!(invite.url.contains(&format!("&token={}", invite.token)));
    assert!(invite.url.contains("&addr="), "url {}", invite.url);
    assert!(invite.url.contains("&name="));
}

// ---------------------------------------------------------------------------
// Op allowlist
// ---------------------------------------------------------------------------

#[test]
fn allowlist_matches_the_protocol_table() {
    // The 26 rows of docs/remote-protocol.md's op table, spelled out here so a
    // silent widening of the wire surface fails this test.
    let documented = [
        "get_workspace",
        "allocate_draft_name",
        "spawn_agent",
        "send_user_message",
        "answer_tool_use",
        "stop_agent",
        "resume_agent",
        "archive_agent",
        "set_agent_model",
        "set_agent_effort",
        "read_session_records",
        "read_user_turns",
        "get_git_state",
        "get_agent_diff_stats",
        "list_checkout_tree",
        "read_checkout_file",
        "get_file_diff",
        "commit_agent",
        "push_agent",
        "create_pr",
        "get_pr_state",
        "get_pr_checks",
        "get_pr_live",
        "list_repo_branches",
        "repo_default_branch",
        "discover_supported_models",
    ];
    assert_eq!(dispatch::OPS, documented.as_slice());
}

#[test]
fn never_exposed_ops_are_not_dispatchable() {
    // One per family from the doc's "never exposed, by design" paragraph.
    for op in [
        "db_select",
        "db_insert",
        "db_query",
        "write_checkout_file",
        "rename_checkout_path",
        "delete_checkout_path",
        "create_checkout_file",
        "copy_checkout_file",
        "open_agent_shell",
        "write_to_shell",
        "write_to_agent",
        "open_in_editor",
        "reveal_logs",
        "track_event",
        "install_agent",
        "wf_start_run",
        "roadmap_enqueue",
        "run_start",
        "fork_agent",
        "merge_pr",
        "delete_project",
        "",
        "hello",
        "pair",
    ] {
        assert!(!dispatch::is_allowed(op), "{op} must not be dispatchable");
    }
}

// ---------------------------------------------------------------------------
// Socket-level behaviour
// ---------------------------------------------------------------------------

/// Answers only `get_workspace`, so the server can be exercised without a live
/// `Supervisor`. Production runs `SupervisorDispatch`.
struct StubDispatch;

impl Dispatch for StubDispatch {
    fn dispatch<'a>(&'a self, op: &'a str, _args: Value) -> DispatchFuture<'a> {
        Box::pin(async move {
            match op {
                "get_workspace" => Ok(json!({ "projects": [], "agents": [] })),
                _ => Err(UNKNOWN_OP.to_string()),
            }
        })
    }
}

/// Answers the `hello` snapshot and then never answers anything, so a
/// connection can be driven past its in-flight cap.
struct HangingDispatch;

impl Dispatch for HangingDispatch {
    fn dispatch<'a>(&'a self, op: &'a str, _args: Value) -> DispatchFuture<'a> {
        Box::pin(async move {
            match op {
                "get_workspace" => Ok(json!({ "projects": [], "agents": [] })),
                _ => std::future::pending().await,
            }
        })
    }
}

struct Host {
    dir: tempfile::TempDir,
    state: Arc<RemoteState>,
    port: u16,
}

fn boot() -> Host {
    boot_with(Arc::new(StubDispatch))
}

fn boot_with(dispatch: Arc<dyn Dispatch>) -> Host {
    let dir = tempfile::tempdir().unwrap();
    let state = RemoteState::new(dir.path(), dispatch);
    let port = state.start(0).unwrap();
    Host { dir, state, port }
}

/// One phone's static identity: the keypair whose public half the host records
/// and whose private half proves it in the handshake.
struct Device {
    private: Vec<u8>,
    public: [u8; 32],
}

fn device() -> Device {
    let keypair = secure::generate_keypair().unwrap();
    Device {
        public: keypair.public.clone().try_into().unwrap(),
        private: keypair.private,
    }
}

async fn connect_path(port: u16, path: &str) -> std::io::Result<WebSocketStream<TcpStream>> {
    let tcp = TcpStream::connect(("127.0.0.1", port)).await?;
    tokio_tungstenite::client_async(format!("ws://127.0.0.1:{port}{path}"), tcp)
        .await
        .map(|(ws, _)| ws)
        .map_err(|e| std::io::Error::other(e.to_string()))
}

async fn connect_plain(port: u16) -> WebSocketStream<TcpStream> {
    connect_path(port, "/ws").await.expect("ws handshake")
}

/// A connection past the Noise handshake, as the phone's Rust layer has it: the
/// WebSocket plus the transport state every frame goes through.
struct SecureWs {
    ws: WebSocketStream<TcpStream>,
    channel: SecureChannel,
}

/// Connect and run the initiator's half of `Noise_XX_25519_ChaChaPoly_BLAKE2s`:
/// three binary messages, empty payloads, `-> e`, `<- e, ee, s, es`, `-> s, se`.
async fn secure_connect(port: u16, device: &Device) -> SecureWs {
    let mut ws = connect_plain(port).await;
    let mut handshake = secure::initiator(&device.private).unwrap();
    let mut buf = [0u8; 65535];

    let n = handshake.write_message(&[], &mut buf).unwrap();
    ws.send(Message::Binary(Bytes::copy_from_slice(&buf[..n])))
        .await
        .unwrap();

    let second = next_binary(&mut ws).await;
    handshake.read_message(&second, &mut buf).unwrap();

    let n = handshake.write_message(&[], &mut buf).unwrap();
    ws.send(Message::Binary(Bytes::copy_from_slice(&buf[..n])))
        .await
        .unwrap();

    let channel = SecureChannel::new(handshake.into_transport_mode().unwrap());
    SecureWs { ws, channel }
}

async fn next_binary(ws: &mut WebSocketStream<TcpStream>) -> Bytes {
    loop {
        match ws
            .next()
            .await
            .expect("stream ended")
            .expect("socket error")
        {
            Message::Binary(bytes) => return bytes,
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("expected a binary frame, got {other:?}"),
        }
    }
}

impl SecureWs {
    async fn request(&mut self, id: &str, op: &str, args: Value) {
        let frame = json!({ "id": id, "op": op, "args": args }).to_string();
        self.send_json(frame.as_bytes()).await;
    }

    async fn send_json(&mut self, plaintext: &[u8]) {
        let frame = self.channel.encrypt_frame(plaintext).unwrap();
        self.ws
            .send(Message::Binary(Bytes::from(frame)))
            .await
            .unwrap();
    }

    /// Next application frame, skipping the ping/pong keepalive traffic.
    async fn next_json(&mut self) -> Value {
        let frame = next_binary(&mut self.ws).await;
        let plaintext = self.channel.decrypt_frame(&frame).unwrap();
        serde_json::from_slice(&plaintext).unwrap()
    }

    async fn close_code(&mut self) -> u16 {
        close_code(&mut self.ws).await
    }
}

async fn close_code(ws: &mut WebSocketStream<TcpStream>) -> u16 {
    while let Some(msg) = ws.next().await {
        match msg {
            Ok(Message::Close(Some(frame))) => return u16::from(frame.code),
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => continue,
            Ok(other) => panic!("expected a close frame, got {other:?}"),
            Err(e) => panic!("expected a close frame, got error {e}"),
        }
    }
    panic!("stream ended with no close frame");
}

#[tokio::test]
async fn pair_then_hello_then_op_then_event_fanout() {
    let host = boot();
    let minted = host.state.pairing().mint();
    let phone = device();

    // pair
    let mut ws = secure_connect(host.port, &phone).await;
    ws.request(
        "1",
        "pair",
        json!({
            "token": minted.token,
            "device": { "name": "Alex's iPhone", "platform": "ios", "appVersion": "0.1.0" }
        }),
    )
    .await;
    let paired = ws.next_json().await;
    assert_eq!(paired["id"], "1");
    assert_eq!(paired["ok"], true);
    assert!(paired["result"]["deviceId"].is_string());
    assert_eq!(paired["result"]["host"]["os"], std::env::consts::OS);
    assert!(
        paired["result"].get("deviceToken").is_none(),
        "v2 hands out no credential: the handshake is the credential"
    );
    // `pair` authenticates and opens the event stream; it never pushes a
    // snapshot. The client calls `get_workspace` itself.
    assert!(paired["result"].get("workspace").is_none());

    // The pairing token is spent, and the device is now in the census under the
    // key the handshake proved.
    assert!(!host.state.pairing().consume(&minted.token));
    let status = host.state.status();
    assert_eq!(status.devices.len(), 1);
    assert!(status.devices[0].connected);
    assert!(status.devices[0].last_seen_at.is_some());
    assert!(host.state.devices().find_by_key(&phone.public).is_some());

    // Events reach the paired connection.
    host.state
        .forward_event("agent:status", r#"{"agentId":"arabia","status":"running"}"#);
    let event = ws.next_json().await;
    assert_eq!(event["event"], "agent:status");
    assert_eq!(event["payload"]["agentId"], "arabia");
    assert!(event.get("id").is_none(), "events carry no id");

    drop(ws);

    // hello on a fresh connection, with the same device key.
    let mut ws = secure_connect(host.port, &phone).await;
    ws.request(
        "2",
        "hello",
        json!({ "client": { "name": "Alex's iPhone", "platform": "ios", "appVersion": "0.1.0" } }),
    )
    .await;
    let hello = ws.next_json().await;
    assert_eq!(hello["id"], "2");
    assert_eq!(hello["ok"], true);
    assert!(hello["result"]["workspace"].is_object());

    ws.request("3", "get_workspace", json!({})).await;
    let reply = ws.next_json().await;
    assert_eq!(reply["id"], "3");
    assert_eq!(reply["ok"], true);
    assert_eq!(reply["result"]["agents"], json!([]));

    ws.request("4", "db_select", json!({ "table": "settings" }))
        .await;
    let denied = ws.next_json().await;
    assert_eq!(denied["id"], "4");
    assert_eq!(denied["ok"], false);
    assert_eq!(denied["error"], UNKNOWN_OP);
}

/// A second `pair` on the same device key is the same device, not a new row —
/// a phone whose code lapsed mid-pairing must not leave two records behind.
#[tokio::test]
async fn pairing_twice_on_one_key_does_not_duplicate_the_device() {
    let host = boot();
    let phone = device();

    for name in ["iPhone", "iPhone renamed"] {
        let minted = host.state.pairing().mint();
        let mut ws = secure_connect(host.port, &phone).await;
        ws.request(
            "1",
            "pair",
            json!({ "token": minted.token, "device": { "name": name, "platform": "ios" } }),
        )
        .await;
        assert_eq!(ws.next_json().await["ok"], true);
    }

    let devices = host.state.status().devices;
    assert_eq!(devices.len(), 1, "one key, one record");
    assert_eq!(devices[0].name, "iPhone renamed");
}

/// The handshake is not optional: a client that opens the socket and starts
/// talking JSON never gets a channel.
#[tokio::test]
async fn a_garbage_first_handshake_message_closes_4001() {
    let host = boot();
    let mut ws = connect_plain(host.port).await;
    ws.send(Message::Binary(Bytes::from_static(b"not a noise message")))
        .await
        .unwrap();
    assert_eq!(close_code(&mut ws).await, 4001);
}

#[tokio::test]
async fn a_text_handshake_message_closes_4001() {
    let host = boot();
    let mut ws = connect_plain(host.port).await;
    ws.send(Message::Text("{\"op\":\"hello\"}".into()))
        .await
        .unwrap();
    assert_eq!(close_code(&mut ws).await, 4001);
}

/// Past the handshake every protocol frame is encrypted, so binary. A text
/// frame is a client that does not speak v2.
#[tokio::test]
async fn a_text_frame_after_the_handshake_closes_4001() {
    let host = boot();
    let mut ws = secure_connect(host.port, &device()).await;
    ws.ws
        .send(Message::Text("{\"id\":\"1\",\"op\":\"hello\"}".into()))
        .await
        .unwrap();
    assert_eq!(ws.close_code().await, 4001);
}

#[tokio::test]
async fn an_undecryptable_frame_closes_4001() {
    let host = boot();
    let mut ws = secure_connect(host.port, &device()).await;
    ws.ws
        .send(Message::Binary(Bytes::from_static(&[0, 20, 9, 9, 9])))
        .await
        .unwrap();
    assert_eq!(ws.close_code().await, 4001);
}

#[tokio::test]
async fn first_frame_other_than_pair_or_hello_closes_4001() {
    let host = boot();
    let mut ws = secure_connect(host.port, &device()).await;
    ws.request("1", "get_workspace", json!({})).await;
    assert_eq!(ws.close_code().await, 4001);
}

#[tokio::test]
async fn unparseable_first_frame_closes_4001() {
    let host = boot();
    let mut ws = secure_connect(host.port, &device()).await;
    ws.send_json(b"not json").await;
    assert_eq!(ws.close_code().await, 4001);
}

/// A key the host has never seen — never paired, or revoked — cannot say hello,
/// and the handshake succeeding does not change that.
#[tokio::test]
async fn hello_from_an_unregistered_key_closes_4003() {
    let host = boot();
    let mut ws = secure_connect(host.port, &device()).await;
    ws.request("1", "hello", json!({})).await;
    assert_eq!(ws.close_code().await, 4003);
}

#[tokio::test]
async fn revoked_device_key_closes_4003() {
    let host = boot();
    let phone = device();
    let record = host
        .state
        .devices()
        .register("phone", "ios", &phone.public)
        .unwrap();
    host.state.revoke_device(&record.device_id).unwrap();

    let mut ws = secure_connect(host.port, &phone).await;
    ws.request("1", "hello", json!({})).await;
    assert_eq!(ws.close_code().await, 4003);
}

#[tokio::test]
async fn bad_pairing_token_closes_4003() {
    let host = boot();
    let mut ws = secure_connect(host.port, &device()).await;
    ws.request("1", "pair", json!({ "token": "ZZZZZZZZ" }))
        .await;
    assert_eq!(ws.close_code().await, 4003);
    assert!(host.state.status().devices.is_empty());
}

/// Registered before the handshake has to mean reachable during it: turning
/// remote access off while a socket is still handshaking closes it with 4004
/// right away, not after the handshake finishes (or its 10 s timeout fires),
/// when a `pair` or `hello` that was already buffered could otherwise have
/// been authenticated first.
#[tokio::test]
async fn disabling_during_the_handshake_closes_4004_at_once() {
    let host = boot();
    let phone = device();
    let mut ws = connect_plain(host.port).await;
    let mut handshake = secure::initiator(&phone.private).unwrap();
    let mut buf = [0u8; 65535];
    let n = handshake.write_message(&[], &mut buf).unwrap();
    ws.send(Message::Binary(Bytes::copy_from_slice(&buf[..n])))
        .await
        .unwrap();
    // Message 2 arrives: the host is now parked waiting for message 3.
    let _second = next_binary(&mut ws).await;

    host.state.stop();
    let code = tokio::time::timeout(Duration::from_secs(2), close_code(&mut ws))
        .await
        .expect("closed at once, not after the handshake timeout");
    assert_eq!(code, 4004);
}

#[tokio::test]
async fn disabling_remote_access_closes_live_connections_4004() {
    let host = boot();
    let phone = device();
    host.state
        .devices()
        .register("phone", "ios", &phone.public)
        .unwrap();
    let mut ws = secure_connect(host.port, &phone).await;
    ws.request("1", "hello", json!({})).await;
    assert_eq!(ws.next_json().await["ok"], true);

    host.state.stop();
    assert_eq!(ws.close_code().await, 4004);
    assert!(!host.state.status().listening);
}

#[tokio::test]
async fn revoking_a_device_closes_its_live_connection_4003() {
    let host = boot();
    let phone = device();
    let record = host
        .state
        .devices()
        .register("phone", "ios", &phone.public)
        .unwrap();
    let mut ws = secure_connect(host.port, &phone).await;
    ws.request("1", "hello", json!({})).await;
    assert_eq!(ws.next_json().await["ok"], true);
    assert!(host.state.status().devices[0].connected);

    // The record is gone *and* the socket that was using it is hung up on:
    // an already-authenticated connection is not re-checked per request.
    host.state.revoke_device(&record.device_id).unwrap();
    assert_eq!(ws.close_code().await, 4003);
    assert!(host.state.status().devices.is_empty());
}

/// The hang-up must not hinge on the disk write: with `devices.json`
/// unwritable, revoke reports the persistence error, but the record is gone
/// from memory and the socket that was using it is closed all the same. The
/// failure then stays visible in `status().error` and the write is retried from
/// `status` until it lands, so the stale on-disk record cannot bring the device
/// back at the next launch.
#[cfg(unix)]
#[tokio::test]
async fn revoking_closes_the_live_connection_even_when_persisting_fails() {
    use std::os::unix::fs::PermissionsExt;

    let host = boot();
    let phone = device();
    let record = host
        .state
        .devices()
        .register("phone", "ios", &phone.public)
        .unwrap();
    let mut ws = secure_connect(host.port, &phone).await;
    ws.request("1", "hello", json!({})).await;
    assert_eq!(ws.next_json().await["ok"], true);

    // Read-only store directory: the tmp file for the rewrite cannot be created.
    let dir = host.dir.path();
    let writable = std::fs::metadata(dir).unwrap().permissions();
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let outcome = host.state.revoke_device(&record.device_id);

    assert!(outcome.is_err(), "the persistence failure is reported");
    assert_eq!(ws.close_code().await, 4003);
    assert!(host.state.devices().find_by_key(&phone.public).is_none());
    let status = host.state.status();
    assert!(status.devices.is_empty());
    assert!(
        status
            .error
            .as_deref()
            .is_some_and(|e| e.contains("could not be saved")),
        "the failed write stays visible, not just the Err of the one call"
    );
    // While the disk is unwritable the old record is still there, which is
    // exactly what a relaunch would reload.
    assert!(DeviceStore::load(dir).find_by_key(&phone.public).is_some());

    // Disk recovers: the next status poll retries the write, clears the error,
    // and the record is gone from disk too.
    std::fs::set_permissions(dir, writable).unwrap();
    let status = host.state.status();
    assert_eq!(status.error, None);
    assert!(DeviceStore::load(dir).find_by_key(&phone.public).is_none());
}

#[tokio::test]
async fn revoking_one_device_leaves_another_connected() {
    let host = boot();
    let (one, two) = (device(), device());
    let first = host
        .state
        .devices()
        .register("phone", "ios", &one.public)
        .unwrap();
    host.state
        .devices()
        .register("tablet", "android", &two.public)
        .unwrap();

    let mut doomed = secure_connect(host.port, &one).await;
    doomed.request("1", "hello", json!({})).await;
    assert_eq!(doomed.next_json().await["ok"], true);
    let mut kept = secure_connect(host.port, &two).await;
    kept.request("1", "hello", json!({})).await;
    assert_eq!(kept.next_json().await["ok"], true);

    host.state.revoke_device(&first.device_id).unwrap();
    assert_eq!(doomed.close_code().await, 4003);

    kept.request("2", "get_workspace", json!({})).await;
    let reply = kept.next_json().await;
    assert_eq!(reply["id"], "2");
    assert_eq!(reply["ok"], true);
}

#[tokio::test]
async fn requests_past_the_in_flight_cap_are_refused_without_dispatching() {
    let host = boot_with(Arc::new(HangingDispatch));
    let phone = device();
    host.state
        .devices()
        .register("phone", "ios", &phone.public)
        .unwrap();
    let mut ws = secure_connect(host.port, &phone).await;
    ws.request("0", "hello", json!({})).await;
    assert_eq!(ws.next_json().await["ok"], true);

    // Eight ops that never answer fill the connection's slots; the reader
    // handles frames in order, so the ninth is over the cap by construction.
    for i in 1..=8 {
        ws.request(&i.to_string(), "get_git_state", json!({})).await;
    }
    ws.request("9", "get_git_state", json!({})).await;

    let refused = ws.next_json().await;
    assert_eq!(refused["id"], "9");
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["error"], TOO_MANY_IN_FLIGHT);
}

#[tokio::test]
async fn oversized_frame_is_rejected_and_never_dispatched() {
    let host = boot();
    let phone = device();
    host.state
        .devices()
        .register("phone", "ios", &phone.public)
        .unwrap();
    let mut ws = secure_connect(host.port, &phone).await;
    ws.request("1", "hello", json!({})).await;
    assert_eq!(ws.next_json().await["ok"], true);

    // Past the 4 MiB cap. tungstenite rejects on the frame *header*, while the
    // client is still writing the body; the host drains the socket before it
    // drops it (see `server::drain`), which is what makes the 1009 deliverable
    // instead of being destroyed by a reset. The phone has no client-side cap,
    // so this code is the only thing that tells it why the socket went away.
    let huge = json!({ "id": "2", "op": "get_workspace", "args": { "pad": "x".repeat(5 << 20) } });
    ws.send_json(huge.to_string().as_bytes()).await;
    loop {
        match ws.ws.next().await {
            Some(Ok(Message::Close(Some(frame)))) => {
                assert_eq!(u16::from(frame.code), 1009);
                break;
            }
            Some(Ok(Message::Binary(_))) => panic!("host answered an oversized frame"),
            Some(Ok(_)) => continue,
            Some(Err(e)) => panic!("expected a 1009 close frame, got error {e}"),
            None => panic!("stream ended with no close frame"),
        }
    }
    wait_disconnected(&host.state).await;
}

/// Wait for the host to drop every device from its census — the connection task
/// does that as it unwinds, so it lands just after the socket dies.
async fn wait_disconnected(state: &Arc<RemoteState>) {
    for _ in 0..100 {
        if state.status().devices.iter().all(|d| !d.connected) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("device stayed marked connected after the socket died");
}

#[tokio::test]
async fn only_the_ws_path_is_served() {
    let host = boot();
    assert!(connect_path(host.port, "/").await.is_err());
    assert!(connect_path(host.port, "/admin").await.is_err());
    assert!(connect_path(host.port, "/ws").await.is_ok());
}
