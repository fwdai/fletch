use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use super::auth::DeviceRecord;
use super::dispatch::{self, Dispatch, DispatchFuture, TOO_MANY_IN_FLIGHT, UNKNOWN_OP};
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
// Device credentials
// ---------------------------------------------------------------------------

#[test]
fn device_token_verifies_until_revoked() {
    let dir = tempfile::tempdir().unwrap();
    let store = DeviceStore::load(dir.path());
    let (record, token) = store.register("Alex's iPhone", "ios").unwrap();

    assert_eq!(token.len(), 43, "32 random bytes as base64url");
    let found = store.verify(&token).expect("fresh token verifies");
    assert_eq!(found.device_id, record.device_id);

    assert!(store.revoke(&record.device_id).unwrap());
    assert!(store.verify(&token).is_none(), "revoked token still works");
    assert!(
        !store.revoke(&record.device_id).unwrap(),
        "revoke is idempotent"
    );
}

#[test]
fn unknown_device_token_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let store = DeviceStore::load(dir.path());
    store.register("phone", "ios").unwrap();
    assert!(store.verify("not-a-real-token").is_none());
}

#[test]
fn devices_json_holds_no_plaintext_token_and_is_owner_only() {
    let dir = tempfile::tempdir().unwrap();
    let store = DeviceStore::load(dir.path());
    let (_, token) = store.register("phone", "ios").unwrap();

    let path = dir.path().join("devices.json");
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(!raw.contains(&token), "plaintext token reached disk");
    assert!(raw.contains("tokenHash"));

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
    let token = {
        let store = DeviceStore::load(dir.path());
        store.register("phone", "ios").unwrap().1
    };
    let reopened = DeviceStore::load(dir.path());
    assert!(reopened.verify(&token).is_some());
    assert_eq!(reopened.list().len(), 1);
}

#[test]
fn concurrent_writers_leave_the_file_agreeing_with_memory() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DeviceStore::load(dir.path()));

    // Every writer persists the whole list, so two of them racing used to be
    // able to rename each other's temp file and land out of order —
    // resurrecting a revoked credential at the next launch.
    let workers: Vec<_> = (0..8)
        .map(|w| {
            let store = store.clone();
            std::thread::spawn(move || {
                for i in 0..20 {
                    let (record, _) = store.register(&format!("phone-{w}-{i}"), "ios").unwrap();
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

async fn connect_path(port: u16, path: &str) -> std::io::Result<WebSocketStream<TcpStream>> {
    let tcp = TcpStream::connect(("127.0.0.1", port)).await?;
    tokio_tungstenite::client_async(format!("ws://127.0.0.1:{port}{path}"), tcp)
        .await
        .map(|(ws, _)| ws)
        .map_err(|e| std::io::Error::other(e.to_string()))
}

async fn connect(port: u16) -> WebSocketStream<TcpStream> {
    connect_path(port, "/ws").await.expect("handshake")
}

async fn request(ws: &mut WebSocketStream<TcpStream>, id: &str, op: &str, args: Value) {
    let frame = json!({ "id": id, "op": op, "args": args }).to_string();
    ws.send(Message::Text(frame.into())).await.unwrap();
}

/// Next application frame, skipping the ping/pong keepalive traffic.
async fn next_json(ws: &mut WebSocketStream<TcpStream>) -> Value {
    loop {
        match ws
            .next()
            .await
            .expect("stream ended")
            .expect("socket error")
        {
            Message::Text(text) => return serde_json::from_str(&text).unwrap(),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("expected a text frame, got {other:?}"),
        }
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

    // pair
    let mut ws = connect(host.port).await;
    request(
        &mut ws,
        "1",
        "pair",
        json!({
            "token": minted.token,
            "device": { "name": "Alex's iPhone", "platform": "ios", "appVersion": "0.1.0" }
        }),
    )
    .await;
    let paired = next_json(&mut ws).await;
    assert_eq!(paired["id"], "1");
    assert_eq!(paired["ok"], true);
    let device_token = paired["result"]["deviceToken"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(device_token.len(), 43);
    assert!(paired["result"]["deviceId"].is_string());
    assert_eq!(paired["result"]["host"]["os"], std::env::consts::OS);
    // `pair` authenticates and opens the event stream; it never pushes a
    // snapshot. The client calls `get_workspace` itself.
    assert!(paired["result"].get("workspace").is_none());

    // The pairing token is spent, and the device is now in the census.
    assert!(!host.state.pairing().consume(&minted.token));
    let status = host.state.status();
    assert_eq!(status.devices.len(), 1);
    assert!(status.devices[0].connected);
    assert!(status.devices[0].last_seen_at.is_some());

    // Events reach the paired connection.
    host.state
        .forward_event("agent:status", r#"{"agentId":"arabia","status":"running"}"#);
    let event = next_json(&mut ws).await;
    assert_eq!(event["event"], "agent:status");
    assert_eq!(event["payload"]["agentId"], "arabia");
    assert!(event.get("id").is_none(), "events carry no id");

    drop(ws);

    // hello on a fresh connection, with the token pair handed out.
    let mut ws = connect(host.port).await;
    request(
        &mut ws,
        "2",
        "hello",
        json!({
            "deviceToken": device_token,
            "client": { "name": "Alex's iPhone", "platform": "ios", "appVersion": "0.1.0" }
        }),
    )
    .await;
    let hello = next_json(&mut ws).await;
    assert_eq!(hello["id"], "2");
    assert_eq!(hello["ok"], true);
    assert!(hello["result"]["workspace"].is_object());

    request(&mut ws, "3", "get_workspace", json!({})).await;
    let reply = next_json(&mut ws).await;
    assert_eq!(reply["id"], "3");
    assert_eq!(reply["ok"], true);
    assert_eq!(reply["result"]["agents"], json!([]));

    request(&mut ws, "4", "db_select", json!({ "table": "settings" })).await;
    let denied = next_json(&mut ws).await;
    assert_eq!(denied["id"], "4");
    assert_eq!(denied["ok"], false);
    assert_eq!(denied["error"], UNKNOWN_OP);
}

#[tokio::test]
async fn first_frame_other_than_pair_or_hello_closes_4001() {
    let host = boot();
    let mut ws = connect(host.port).await;
    request(&mut ws, "1", "get_workspace", json!({})).await;
    assert_eq!(close_code(&mut ws).await, 4001);
}

#[tokio::test]
async fn unparseable_first_frame_closes_4001() {
    let host = boot();
    let mut ws = connect(host.port).await;
    ws.send(Message::Text("not json".into())).await.unwrap();
    assert_eq!(close_code(&mut ws).await, 4001);
}

#[tokio::test]
async fn bad_device_token_closes_4003() {
    let host = boot();
    let mut ws = connect(host.port).await;
    request(&mut ws, "1", "hello", json!({ "deviceToken": "nope" })).await;
    assert_eq!(close_code(&mut ws).await, 4003);
}

#[tokio::test]
async fn revoked_device_token_closes_4003() {
    let host = boot();
    let (record, token) = host.state.devices().register("phone", "ios").unwrap();
    host.state.revoke_device(&record.device_id).unwrap();

    let mut ws = connect(host.port).await;
    request(&mut ws, "1", "hello", json!({ "deviceToken": token })).await;
    assert_eq!(close_code(&mut ws).await, 4003);
}

#[tokio::test]
async fn bad_pairing_token_closes_4003() {
    let host = boot();
    let mut ws = connect(host.port).await;
    request(&mut ws, "1", "pair", json!({ "token": "ZZZZZZZZ" })).await;
    assert_eq!(close_code(&mut ws).await, 4003);
    assert!(host.state.status().devices.is_empty());
}

#[tokio::test]
async fn disabling_remote_access_closes_live_connections_4004() {
    let host = boot();
    let (_, token) = host.state.devices().register("phone", "ios").unwrap();
    let mut ws = connect(host.port).await;
    request(&mut ws, "1", "hello", json!({ "deviceToken": token })).await;
    assert_eq!(next_json(&mut ws).await["ok"], true);

    host.state.stop();
    assert_eq!(close_code(&mut ws).await, 4004);
    assert!(!host.state.status().listening);
}

#[tokio::test]
async fn revoking_a_device_closes_its_live_connection_4003() {
    let host = boot();
    let (record, token) = host.state.devices().register("phone", "ios").unwrap();
    let mut ws = connect(host.port).await;
    request(&mut ws, "1", "hello", json!({ "deviceToken": token })).await;
    assert_eq!(next_json(&mut ws).await["ok"], true);
    assert!(host.state.status().devices[0].connected);

    // The credential is gone *and* the socket that was using it is hung up on:
    // an already-authenticated connection is not re-checked per request.
    host.state.revoke_device(&record.device_id).unwrap();
    assert_eq!(close_code(&mut ws).await, 4003);
    assert!(host.state.status().devices.is_empty());
}

/// The hang-up must not hinge on the disk write: with `devices.json`
/// unwritable, revoke reports the persistence error, but the credential is
/// gone from memory and the socket that was using it is closed all the same.
#[cfg(unix)]
#[tokio::test]
async fn revoking_closes_the_live_connection_even_when_persisting_fails() {
    use std::os::unix::fs::PermissionsExt;

    let host = boot();
    let (record, token) = host.state.devices().register("phone", "ios").unwrap();
    let mut ws = connect(host.port).await;
    request(&mut ws, "1", "hello", json!({ "deviceToken": token })).await;
    assert_eq!(next_json(&mut ws).await["ok"], true);

    // Read-only store directory: the tmp file for the rewrite cannot be created.
    let dir = host.dir.path();
    let writable = std::fs::metadata(dir).unwrap().permissions();
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let outcome = host.state.revoke_device(&record.device_id);
    std::fs::set_permissions(dir, writable).unwrap();

    assert!(outcome.is_err(), "the persistence failure is reported");
    assert_eq!(close_code(&mut ws).await, 4003);
    assert!(host.state.devices().verify(&token).is_none());
    assert!(host.state.status().devices.is_empty());
}

#[tokio::test]
async fn revoking_one_device_leaves_another_connected() {
    let host = boot();
    let (first, first_token) = host.state.devices().register("phone", "ios").unwrap();
    let (_, second_token) = host.state.devices().register("tablet", "android").unwrap();

    let mut doomed = connect(host.port).await;
    request(
        &mut doomed,
        "1",
        "hello",
        json!({ "deviceToken": first_token }),
    )
    .await;
    assert_eq!(next_json(&mut doomed).await["ok"], true);
    let mut kept = connect(host.port).await;
    request(
        &mut kept,
        "1",
        "hello",
        json!({ "deviceToken": second_token }),
    )
    .await;
    assert_eq!(next_json(&mut kept).await["ok"], true);

    host.state.revoke_device(&first.device_id).unwrap();
    assert_eq!(close_code(&mut doomed).await, 4003);

    request(&mut kept, "2", "get_workspace", json!({})).await;
    let reply = next_json(&mut kept).await;
    assert_eq!(reply["id"], "2");
    assert_eq!(reply["ok"], true);
}

#[tokio::test]
async fn requests_past_the_in_flight_cap_are_refused_without_dispatching() {
    let host = boot_with(Arc::new(HangingDispatch));
    let (_, token) = host.state.devices().register("phone", "ios").unwrap();
    let mut ws = connect(host.port).await;
    request(&mut ws, "0", "hello", json!({ "deviceToken": token })).await;
    assert_eq!(next_json(&mut ws).await["ok"], true);

    // Eight ops that never answer fill the connection's slots; the reader
    // handles frames in order, so the ninth is over the cap by construction.
    for i in 1..=8 {
        request(&mut ws, &i.to_string(), "get_git_state", json!({})).await;
    }
    request(&mut ws, "9", "get_git_state", json!({})).await;

    let refused = next_json(&mut ws).await;
    assert_eq!(refused["id"], "9");
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["error"], TOO_MANY_IN_FLIGHT);
}

#[tokio::test]
async fn oversized_frame_is_rejected_and_never_dispatched() {
    let host = boot();
    let (_, token) = host.state.devices().register("phone", "ios").unwrap();
    let mut ws = connect(host.port).await;
    request(&mut ws, "1", "hello", json!({ "deviceToken": token })).await;
    assert_eq!(next_json(&mut ws).await["ok"], true);

    // Past the 4 MiB cap. tungstenite rejects on the frame *header*, while the
    // client is still writing the body; the host drains the socket before it
    // drops it (see `server::drain`), which is what makes the 1009 deliverable
    // instead of being destroyed by a reset. The phone has no client-side cap,
    // so this code is the only thing that tells it why the socket went away.
    let huge = json!({ "id": "2", "op": "get_workspace", "args": { "pad": "x".repeat(5 << 20) } });
    ws.send(Message::Text(huge.to_string().into()))
        .await
        .expect("host keeps reading until the client is done writing");
    loop {
        match ws.next().await {
            Some(Ok(Message::Close(Some(frame)))) => {
                assert_eq!(u16::from(frame.code), 1009);
                break;
            }
            Some(Ok(Message::Text(text))) => panic!("host answered an oversized frame: {text}"),
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
