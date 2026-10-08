//! Checks against a real login on this Mac. Ignored: they spend a real
//! refresh and a real model turn. Name the managed account to use with
//! `FLETCH_LIVE_CLAUDE_ACCOUNT`; nothing secret is printed, only expiries,
//! stamps and short digests.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use super::super::*;
use crate::agent::host_login::{Demand, HostLogin};

fn account_dir() -> PathBuf {
    let id = std::env::var("FLETCH_LIVE_CLAUDE_ACCOUNT")
        .expect("set FLETCH_LIVE_CLAUDE_ACCOUNT to a managed claude account id");
    dirs::home_dir()
        .unwrap()
        .join(".fletch/accounts/claude")
        .join(id)
}

fn digest(secret: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(secret.as_bytes())
        .iter()
        .take(4)
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn stored_pair(store: &HostStore) -> Pair {
    let text = store.load().unwrap().expect("the account is signed in").0;
    parse_pair(&serde_json::from_str(&text).unwrap()).unwrap()
}

/// A forced refresh rotates the pair, writes it back to the account's
/// Keychain item (its `mdat` advances) and keeps every other field.
#[tokio::test]
#[ignore]
async fn live_forced_refresh_rotates_and_stores_the_pair() {
    let dir = account_dir();
    let store = HostStore::for_account(Some(&dir)).unwrap();
    let service = store.service.clone();
    let before_stamp = store.stamp();
    let before = stored_pair(&store);
    let before_blob: Value = serde_json::from_str(&store.load().unwrap().unwrap().0).unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    let login = ClaudeLogin::for_account(Some(&dir)).unwrap();
    let demand = Demand::Replace {
        rejected_expires_at_ms: before.expires_at_ms,
    };
    let token = tokio::task::spawn_blocking(move || HostLogin::new(login).credential(demand))
        .await
        .unwrap()
        .expect("refresh");

    let store = HostStore::for_account(Some(&dir)).unwrap();
    let after = stored_pair(&store);
    let after_blob: Value = serde_json::from_str(&store.load().unwrap().unwrap().0).unwrap();
    println!(
        "service={service} access {}->{} refresh {}->{} \
             expires_in_min {}->{} stamp {:?}->{:?}",
        digest(&before.access),
        digest(&after.access),
        digest(before.refresh.as_deref().unwrap_or("")),
        digest(after.refresh.as_deref().unwrap_or("")),
        (before.expires_at_ms - super::super::super::now_ms()) / 60_000,
        (after.expires_at_ms - super::super::super::now_ms()) / 60_000,
        before_stamp,
        store.stamp(),
    );
    assert_eq!(token.secret(), after.access);
    assert_ne!(after.access, before.access);
    assert!(after.expires_at_ms > before.expires_at_ms);
    assert_ne!(store.stamp(), before_stamp);
    for field in ["scopes", "subscriptionType", "rateLimitTier"] {
        assert_eq!(
            after_blob["claudeAiOauth"][field], before_blob["claudeAiOauth"][field],
            "{field}"
        );
    }
}

/// One `claude -p` turn under the seatbelt profile a launch builds, in
/// the shared default config dir, with an env holding nothing but the
/// plan's own (the token included) and the bare essentials.
fn seatbelt_turn(token: &AccessToken) -> (Value, Value, bool) {
    let td = tempfile::tempdir().unwrap();
    let root = td.path().join("agent");
    let cwd = root.join("repo");
    std::fs::create_dir_all(&cwd).unwrap();
    seatbelt_run(
        token,
        &root,
        &cwd,
        &["-p", "Reply with exactly the word: pong"],
    )
}

/// `claude <args> --output-format stream-json --verbose --model haiku`
/// in `cwd` under the profile, with `root` as the writable root.
fn seatbelt_run(
    token: &AccessToken,
    root: &Path,
    cwd: &Path,
    args: &[&str],
) -> (Value, Value, bool) {
    use crate::sandbox::AgentLaunchCtx;
    let home = dirs::home_dir().unwrap();
    let td = tempfile::tempdir().unwrap();
    let rpc = td.path().join("rpc");
    std::fs::create_dir_all(&rpc).unwrap();
    let claude = crate::agent::resolve_agent_bin("claude", "claude", "Claude Code", &home).unwrap();
    let ctx = AgentLaunchCtx {
        agent_id: "live",
        provider: "claude",
        writable_root: root,
        source_repos: &[],
        rpc_dir: &rpc,
        cwd,
        home: &home,
        interactive: false,
        blackboard: None,
        account_dir: None,
        oauth_token: Some(token),
        codex_home: None,
    };
    let plan = crate::sandbox::engine_for(crate::sandbox::EngineKind::SandboxExec)
        .unwrap()
        .launch_agent(&ctx, &claude)
        .unwrap();
    let out = std::process::Command::new(&plan.program)
        .args(&plan.prefix_args)
        .args(args)
        .args([
            "--output-format",
            "stream-json",
            "--verbose",
            "--model",
            "haiku",
        ])
        .current_dir(cwd)
        .env_clear()
        .env("HOME", &home)
        .env("PATH", std::env::var("PATH").unwrap())
        .env("USER", std::env::var("USER").unwrap())
        .env("TERM", "xterm-256color")
        .envs(plan.env.iter().map(|(k, v)| (k, v)))
        .output()
        .unwrap();
    let events: Vec<Value> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let find =
        |pred: &dyn Fn(&Value) -> bool| events.iter().find(|e| pred(e)).cloned().expect("event");
    let init = find(&|e| e["type"] == "system" && e["subtype"] == "init");
    let result = find(&|e| e["type"] == "result");
    let rate_limit = events.iter().any(|e| e["type"] == "rate_limit_event");
    (init, result, rate_limit)
}

/// The account's host-resolved token alone completes a turn under the
/// sandbox, and it is what claude signs in with: the same run with an
/// invalid token in its place is refused, though the shared config
/// dir holds a valid `/login` of its own.
#[tokio::test]
#[ignore]
async fn live_seatbelt_turn_runs_on_the_env_token_alone() {
    let token = access_token_for_launch(Some(&account_dir()))
        .await
        .expect("token");
    let (init, result, rate_limit) = seatbelt_turn(&token);
    println!(
            "real token: apiKeySource={} model={} | is_error={} text={} | rate_limit_event={rate_limit}",
            init["apiKeySource"], init["model"], result["is_error"], result["result"],
        );
    assert_eq!(result["is_error"], false, "{result}");

    let bogus = AccessToken::for_test("sk-ant-oat01-not-a-real-token", 0);
    let (_, result, _) = seatbelt_turn(&bogus);
    println!(
        "invalid token: is_error={} api_error_status={} text={}",
        result["is_error"], result["api_error_status"], result["result"],
    );
    assert_eq!(result["api_error_status"], 401, "{result}");
}

const MCP_PROBE: &str = r#"import json, sys
for line in sys.stdin:
msg = json.loads(line)
mid = msg.get("id")
method = msg.get("method")
if mid is None:
    continue
if method == "initialize":
    result = {"protocolVersion": msg["params"]["protocolVersion"],
              "capabilities": {"tools": {}},
              "serverInfo": {"name": "probe", "version": "1"}}
elif method == "tools/list":
    result = {"tools": [{"name": "marker", "description": "Returns the marker word.",
                         "inputSchema": {"type": "object", "properties": {}}}]}
elif method == "tools/call":
    result = {"content": [{"type": "text", "text": "zebra-quartz-41"}]}
else:
    result = {}
sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": mid, "result": result}) + "\n")
sys.stdout.flush()
"#;

/// The Keychain deny doesn't cost claude its MCP servers: a stdio
/// server's tool is called and answers under the same profile, on the
/// env token alone.
#[tokio::test]
#[ignore]
async fn live_seatbelt_turn_uses_a_stdio_mcp_server() {
    let token = access_token_for_launch(Some(&account_dir()))
        .await
        .expect("token");
    let td = tempfile::tempdir().unwrap();
    let root = td.path().join("agent");
    let cwd = root.join("repo");
    std::fs::create_dir_all(&cwd).unwrap();
    let server = root.join("probe_mcp.py");
    std::fs::write(&server, MCP_PROBE).unwrap();
    let config = root.join("mcp.json");
    let servers = serde_json::json!({"mcpServers": {"probe": {
        "type": "stdio", "command": "/usr/bin/python3", "args": [server],
    }}});
    std::fs::write(&config, servers.to_string()).unwrap();
    let config_arg = config.to_string_lossy().into_owned();
    let (init, result, _) = seatbelt_run(
        &token,
        &root,
        &cwd,
        &[
            "--mcp-config",
            &config_arg,
            "--strict-mcp-config",
            "--allowedTools",
            "mcp__probe__marker",
            "-p",
            "Call the marker tool, then reply with exactly the text it returned.",
        ],
    );
    println!(
        "mcp_servers={} | is_error={} text={}",
        init["mcp_servers"], result["is_error"], result["result"]
    );
    assert_eq!(result["is_error"], false, "{result}");
    assert!(
        result["result"]
            .as_str()
            .is_some_and(|t| t.contains("zebra-quartz-41")),
        "{result}"
    );
}

/// A session written under the account dir before accounts became
/// token sources resumes once moved into the default dir. Works on a
/// copy, under a new id, of the session `FLETCH_LIVE_LEGACY_SESSION`
/// names (a `.jsonl` under a claude account's `projects`), run in
/// `FLETCH_LIVE_LEGACY_CWD`; the copy is removed afterwards.
#[tokio::test]
#[ignore]
async fn live_legacy_account_session_resumes_from_the_default_dir() {
    let source = PathBuf::from(std::env::var("FLETCH_LIVE_LEGACY_SESSION").unwrap());
    let cwd = PathBuf::from(std::env::var("FLETCH_LIVE_LEGACY_CWD").unwrap());
    let old_id = source.file_stem().unwrap().to_string_lossy().into_owned();
    let new_id = uuid::Uuid::new_v4().to_string();
    let copy = source.with_file_name(format!("{new_id}.jsonl"));
    let text = std::fs::read_to_string(&source)
        .unwrap()
        .replace(&old_id, &new_id);
    std::fs::write(&copy, text).unwrap();

    let moved = crate::transcripts::adopt_account_session(&new_id, &cwd)
        .unwrap()
        .expect("the copy moves into the default dir");
    println!("moved to {}", moved.display());
    assert!(!copy.exists());

    let token = access_token_for_launch(Some(&account_dir()))
        .await
        .expect("token");
    let (_, result, _) = seatbelt_run(
        &token,
        &cwd,
        &cwd,
        &[
            "--resume",
            &new_id,
            "-p",
            "Reply with exactly the word: resumed",
        ],
    );
    let _ = std::fs::remove_file(&moved);
    println!(
        "resume: is_error={} num_turns={} text={}",
        result["is_error"], result["num_turns"], result["result"]
    );
    assert_eq!(result["is_error"], false, "{result}");
}
