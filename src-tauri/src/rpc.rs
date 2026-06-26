//! File-mailbox RPC between a sandboxed agent and the app.
//!
//! This module owns the transport: mailbox layout, atomic request/response
//! handling, and the dispatcher trait. Feature-specific behavior lives behind
//! dispatchers such as `rpc::git::GitDispatcher`.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};

/// How old an unparseable request must be before we give up on it. A compliant
/// agent writes atomically (tmp + rename), so a renamed file is always complete;
/// this grace window only tolerates a non-compliant agent caught mid-write.
/// Past it, the file is treated as malformed: answered with an error and removed.
const STALE_REQUEST_AGE: Duration = Duration::from_secs(5);

#[path = "rpc/git.rs"]
pub mod git;

/// One request from the agent. The `id` is carried in the filename (the pairing
/// key), so it's not parsed from the body here; `args` defaults to null when
/// omitted. Unknown body fields (including a redundant `id`) are ignored.
#[derive(Debug, Deserialize)]
pub struct Request {
    pub op: String,
    #[serde(default)]
    pub args: Value,
}

/// One response written back to the agent. `exit_code`/`stdout`/`stderr` are
/// present on success; `error` on failure. Serialized fields are omitted when
/// `None` so the two shapes stay clean on disk.
#[derive(Debug, Serialize)]
pub struct Response {
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A structured side effect emitted by a dispatcher. The transport keeps this
/// generic so feature handlers can report whatever state the supervisor needs
/// to persist or forward.
#[derive(Debug, Clone)]
pub enum RpcEvent {
    Named { name: String, payload: Value },
}

impl RpcEvent {
    pub fn named(name: impl Into<String>, payload: Value) -> Self {
        Self::Named {
            name: name.into(),
            payload,
        }
    }
}

pub type RpcFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Feature-specific RPC dispatcher. The transport knows how to read and write
/// mailbox files; the dispatcher knows what operations the app supports.
pub trait RpcDispatcher: Send + Sync {
    fn dispatch<'a>(
        &'a self,
        id: &'a str,
        op: &'a str,
        args: &'a Value,
    ) -> RpcFuture<'a, (Response, Vec<RpcEvent>)>;
}

impl Response {
    pub(crate) fn ok(id: &str, exit_code: i32, stdout: String, stderr: String) -> Self {
        Self {
            id: id.to_string(),
            ok: true,
            exit_code: Some(exit_code),
            stdout: Some(stdout),
            stderr: Some(stderr),
            error: None,
        }
    }

    pub(crate) fn err(id: &str, error: impl Into<String>) -> Self {
        Self {
            id: id.to_string(),
            ok: false,
            exit_code: None,
            stdout: None,
            stderr: None,
            error: Some(error.into()),
        }
    }
}

/// `~/.quorum/rpc/<agent-id>/` — the agent's private mailbox root.
pub fn mailbox_dir(agent_id: &str) -> Result<PathBuf> {
    let home =
        dirs::home_dir().ok_or_else(|| Error::Other("HOME directory not available".into()))?;
    Ok(home.join(".quorum").join("rpc").join(agent_id))
}

/// Create the mailbox and its `requests/`/`responses/` subdirs. Idempotent;
/// called at spawn. The agent's dir is locked down to 0700 — it's a private
/// control channel, not shared.
pub fn ensure_mailbox(dir: &Path) -> Result<()> {
    let requests = dir.join("requests");
    let responses = dir.join("responses");
    for sub in [&requests, &responses] {
        std::fs::create_dir_all(sub)
            .map_err(|e| Error::Other(format!("create rpc mailbox {}: {e}", sub.display())))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // 0700 on the root *and* both subdirs — the responses can carry
        // sensitive tool output, so don't rely on umask for the children.
        for p in [dir, requests.as_path(), responses.as_path()] {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| Error::Other(format!("chmod rpc mailbox {}: {e}", p.display())))?;
        }
    }
    Ok(())
}

/// Process every pending request file once: parse -> dispatch -> write
/// response -> delete the request. Driven on a fixed tick by the per-agent
/// watcher. Errors on a single request are logged and isolated — one bad file
/// can't stall the rest.
pub async fn process_pending(rpc_dir: &Path, dispatcher: &dyn RpcDispatcher) -> Vec<RpcEvent> {
    let requests = rpc_dir.join("requests");
    let responses = rpc_dir.join("responses");

    let entries = match std::fs::read_dir(&requests) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut effects = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        effects.extend(handle_request_file(&path, &responses, dispatcher).await);
    }
    effects
}

async fn handle_request_file(
    path: &Path,
    responses: &Path,
    dispatcher: &dyn RpcDispatcher,
) -> Vec<RpcEvent> {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return Vec::new();
    };
    if !is_safe_key(stem) {
        tracing::warn!(file = %path.display(), "rpc: unsafe request filename, skipping");
        return Vec::new();
    }

    let raw = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error = %e, file = %path.display(), "rpc: read request failed");
            return Vec::new();
        }
    };

    let req: Request = match serde_json::from_slice(&raw) {
        Ok(r) => r,
        Err(e) => {
            if file_age(path).is_some_and(|age| age >= STALE_REQUEST_AGE) {
                tracing::warn!(error = %e, file = %path.display(), "rpc: malformed request, answering with error");
                let resp = Response::err(stem, format!("malformed request JSON: {e}"));
                if write_response_atomic(responses, stem, &resp).is_ok() {
                    let _ = std::fs::remove_file(path);
                }
            } else {
                tracing::debug!(error = %e, file = %path.display(), "rpc: unparseable request (will retry)");
            }
            return Vec::new();
        }
    };

    let (resp, effects) = dispatcher.dispatch(stem, &req.op, &req.args).await;

    if let Err(e) = write_response_atomic(responses, stem, &resp) {
        tracing::warn!(error = %e, id = %stem, "rpc: write response failed");
        return effects;
    }

    if let Err(e) = std::fs::remove_file(path) {
        tracing::warn!(error = %e, file = %path.display(), "rpc: remove request failed");
    }

    effects
}

fn write_response_atomic(responses: &Path, key: &str, resp: &Response) -> Result<()> {
    let json = serde_json::to_vec_pretty(resp)
        .map_err(|e| Error::Other(format!("serialize rpc response: {e}")))?;
    let final_path = responses.join(format!("{key}.json"));
    let tmp_path = responses.join(format!("{key}.json.tmp"));
    std::fs::write(&tmp_path, &json)
        .map_err(|e| Error::Other(format!("write rpc response tmp: {e}")))?;
    std::fs::rename(&tmp_path, &final_path)
        .map_err(|e| Error::Other(format!("rename rpc response: {e}")))?;
    Ok(())
}

fn is_safe_key(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn file_age(path: &Path) -> Option<Duration> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    SystemTime::now().duration_since(modified).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_request(requests: &Path, name: &str, body: &str) {
        std::fs::write(requests.join(name), body).unwrap();
    }

    struct MockDispatcher;

    impl RpcDispatcher for MockDispatcher {
        fn dispatch<'a>(
            &'a self,
            id: &'a str,
            op: &'a str,
            _args: &'a Value,
        ) -> RpcFuture<'a, (Response, Vec<RpcEvent>)> {
            Box::pin(async move {
                match op {
                    "ping" => (
                        Response::ok(id, 0, "pong".to_string(), String::new()),
                        Vec::new(),
                    ),
                    other => (
                        Response::err(id, format!("unknown op: {other}")),
                        Vec::new(),
                    ),
                }
            })
        }
    }

    #[test]
    fn ensure_mailbox_creates_both_subdirs() {
        let td = tempfile::tempdir().unwrap();
        let dir = td.path().join(".quorum-rpc");
        ensure_mailbox(&dir).unwrap();
        assert!(dir.join("requests").is_dir());
        assert!(dir.join("responses").is_dir());
        ensure_mailbox(&dir).unwrap();
    }

    #[test]
    fn file_age_is_small_for_a_fresh_file() {
        let td = tempfile::tempdir().unwrap();
        let f = td.path().join("fresh");
        std::fs::write(&f, b"x").unwrap();
        let age = file_age(&f).expect("fresh file has an age");
        assert!(age < STALE_REQUEST_AGE);
        assert!(file_age(&td.path().join("nope")).is_none());
    }

    #[test]
    fn is_safe_key_rejects_traversal() {
        assert!(is_safe_key("abc-123_DEF"));
        assert!(!is_safe_key(""));
        assert!(!is_safe_key("../escape"));
        assert!(!is_safe_key("a/b"));
        assert!(!is_safe_key("with.dot"));
    }

    #[tokio::test]
    async fn ping_round_trips_to_a_response_file() {
        let td = tempfile::tempdir().unwrap();
        let rpc_dir = td.path().join(".quorum-rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        write_request(
            &rpc_dir.join("requests"),
            "req-1.json",
            r#"{"id":"req-1","op":"ping"}"#,
        );

        let dispatcher = MockDispatcher;
        process_pending(&rpc_dir, &dispatcher).await;

        assert!(!rpc_dir.join("requests/req-1.json").exists());
        let body = std::fs::read_to_string(rpc_dir.join("responses/req-1.json")).unwrap();
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["id"], "req-1");
        assert_eq!(v["ok"], true);
        assert_eq!(v["exit_code"], 0);
        assert_eq!(v["stdout"], "pong");
        assert!(!rpc_dir.join("responses/req-1.json.tmp").exists());
    }

    #[tokio::test]
    async fn unknown_op_is_rejected() {
        let td = tempfile::tempdir().unwrap();
        let rpc_dir = td.path().join(".quorum-rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        write_request(
            &rpc_dir.join("requests"),
            "req-2.json",
            r#"{"id":"req-2","op":"rm_rf_everything","args":{}}"#,
        );

        let dispatcher = MockDispatcher;
        process_pending(&rpc_dir, &dispatcher).await;

        let body = std::fs::read_to_string(rpc_dir.join("responses/req-2.json")).unwrap();
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().contains("unknown op"));
    }

    #[tokio::test]
    async fn fresh_unparseable_request_is_left_for_retry() {
        let td = tempfile::tempdir().unwrap();
        let rpc_dir = td.path().join(".quorum-rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        write_request(&rpc_dir.join("requests"), "req-3.json", "{ not json");

        let dispatcher = MockDispatcher;
        process_pending(&rpc_dir, &dispatcher).await;

        assert!(rpc_dir.join("requests/req-3.json").exists());
        assert!(!rpc_dir.join("responses/req-3.json").exists());
    }
}
