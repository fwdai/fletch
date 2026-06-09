//! File-mailbox RPC between a sandboxed agent and the app.
//!
//! A sandboxed agent can't run actions outside its worktree, so it asks the
//! app to. This is tool-calling over files: the agent writes a JSON **request**
//! (`op` + `args`) into its mailbox; an in-app watcher (see
//! `supervisor::spawn_rpc_watcher`) executes a deterministic, allowlisted action
//! and writes a JSON **response**; the agent reads it. The exchange is
//! synchronous (the agent waits for the response), yields a single final payload
//! (no streaming), and is **allowlist-only** — anything not in [`dispatch`] is
//! rejected, which keeps the sandbox meaningful.
//!
//! The mailbox lives at `~/.quorum/rpc/<agent-id>/` — a private (0700) per-agent
//! dir on the sandbox's write-allow list (see `sandbox.rs`) but entirely outside
//! the git worktree tree, so it never pollutes a repo and is immune to git
//! operations (`git clean`, branch switches) the agent might run. Its path is
//! handed to the agent via the `QUORUM_RPC_DIR` env var, injected at spawn.
//!
//! ```text
//! $QUORUM_RPC_DIR/
//!   requests/<uuid>.json     # agent writes
//!   responses/<uuid>.json    # app writes
//! ```
//!
//! This module is intentionally self-contained — no `Supervisor` or Tauri
//! dependency — so the protocol is unit-testable in isolation. The lifecycle
//! glue (start a watcher per agent, stop it on teardown) lives in `supervisor`.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::process::Command;

use crate::error::{Error, Result};

/// App-side ceiling on a single op. A hung command surfaces as an error
/// response rather than blocking the watcher forever. The agent runs its own
/// (shorter) poll timeout independently — see the instruction block.
const OP_TIMEOUT: Duration = Duration::from_secs(120);

/// How old an unparseable request must be before we give up on it. A compliant
/// agent writes atomically (tmp + rename), so a renamed file is always complete;
/// this grace window only tolerates a non-compliant agent caught mid-write.
/// Past it, the file is treated as malformed: answered with an error and removed.
const STALE_REQUEST_AGE: Duration = Duration::from_secs(5);

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

impl Response {
    fn ok(id: &str, exit_code: i32, stdout: String, stderr: String) -> Self {
        Self {
            id: id.to_string(),
            ok: true,
            exit_code: Some(exit_code),
            stdout: Some(stdout),
            stderr: Some(stderr),
            error: None,
        }
    }

    fn err(id: &str, error: impl Into<String>) -> Self {
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

/// Process every pending request file once: parse → dispatch → write response →
/// delete the request. Driven on a fixed tick by the per-agent watcher. Errors
/// on a single request are logged and isolated — one bad file can't stall the
/// rest. Commands run in `cwd` (the agent's primary worktree).
pub async fn process_pending(rpc_dir: &Path, cwd: &Path) {
    let requests = rpc_dir.join("requests");
    let responses = rpc_dir.join("responses");

    let entries = match std::fs::read_dir(&requests) {
        Ok(e) => e,
        // No mailbox yet (or removed during teardown) — nothing to do.
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        // Only consume finished `<uuid>.json` files. The agent writes
        // atomically (tmp + rename) so we never observe a half-written one;
        // any other extension (e.g. a stray `.tmp`) is ignored.
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        handle_request_file(&path, &responses, cwd).await;
    }
}

/// Handle one request file. The response filename is derived from the request's
/// file stem (the `<uuid>` the agent polls), not the in-body `id`, so the agent
/// always finds its reply where it expects. The stem must be a safe token —
/// a defense-in-depth guard against a malformed id escaping the mailbox.
async fn handle_request_file(path: &Path, responses: &Path, cwd: &Path) {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return;
    };
    if !is_safe_key(stem) {
        tracing::warn!(file = %path.display(), "rpc: unsafe request filename, skipping");
        return;
    }

    let raw = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error = %e, file = %path.display(), "rpc: read request failed");
            return;
        }
    };

    let req: Request = match serde_json::from_slice(&raw) {
        Ok(r) => r,
        Err(e) => {
            // Could be a mid-write read (only if the agent ignored the
            // write-atomically contract) or a genuinely malformed file. Within
            // the grace window, leave it — a transient partial resolves on the
            // next tick. Past it, give up: answer with an error so the agent
            // stops waiting, and delete it so we stop re-reading (and
            // re-logging) it every tick.
            if file_age(path).is_some_and(|age| age >= STALE_REQUEST_AGE) {
                tracing::warn!(error = %e, file = %path.display(), "rpc: malformed request, answering with error");
                let resp = Response::err(stem, format!("malformed request JSON: {e}"));
                if write_response_atomic(responses, stem, &resp).is_ok() {
                    let _ = std::fs::remove_file(path);
                }
            } else {
                tracing::debug!(error = %e, file = %path.display(), "rpc: unparseable request (will retry)");
            }
            return;
        }
    };

    let resp = dispatch(stem, &req.op, &req.args, cwd).await;

    if let Err(e) = write_response_atomic(responses, stem, &resp) {
        tracing::warn!(error = %e, id = %stem, "rpc: write response failed");
        // Leave the request so a later tick can retry rather than dropping it.
        return;
    }

    // Answered — remove the request so it's processed exactly once.
    if let Err(e) = std::fs::remove_file(path) {
        tracing::warn!(error = %e, file = %path.display(), "rpc: remove request failed");
    }
}

/// The op allowlist. Adding an op is one arm here plus a line in the instruction
/// block (`instructions/rpc_protocol.md`). Anything unmatched is rejected, which
/// is what keeps the sandbox meaningful — the agent can only invoke vetted,
/// deterministic actions.
async fn dispatch(id: &str, op: &str, _args: &Value, cwd: &Path) -> Response {
    match op {
        // Liveness probe — proves the round-trip with no side effects.
        "ping" => Response::ok(id, 0, "pong".to_string(), String::new()),
        // Example real op: run a deterministic command and report its result.
        "git_status" => {
            run_command(id, cwd, "git", &["status", "--porcelain=v1", "--branch"]).await
        }
        other => Response::err(id, format!("unknown op: {other}")),
    }
}

/// Run a fixed command in `cwd`, capturing exit/stdout/stderr, bounded by
/// [`OP_TIMEOUT`]. `kill_on_drop` ensures a timed-out child is reaped when the
/// timeout future is dropped.
async fn run_command(id: &str, cwd: &Path, program: &str, args: &[&str]) -> Response {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Response::err(id, format!("spawn {program}: {e}")),
    };

    match tokio::time::timeout(OP_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(out)) => Response::ok(
            id,
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ),
        Ok(Err(e)) => Response::err(id, format!("run {program}: {e}")),
        Err(_) => Response::err(
            id,
            format!("op timed out after {}s", OP_TIMEOUT.as_secs()),
        ),
    }
}

/// Write `responses/<key>.json` atomically: write a sibling `.tmp`, then rename
/// into place (atomic on the same filesystem), so the agent never reads a
/// half-written response.
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

/// A request/response key (the `<uuid>` stem) must be a plain token — no path
/// separators, dots, or empties — so it can't escape the mailbox dir.
fn is_safe_key(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// How long ago `path` was last modified, or `None` if that can't be determined
/// (missing file, or an mtime in the future from clock skew — treated as fresh).
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

    #[test]
    fn ensure_mailbox_creates_both_subdirs() {
        let td = tempfile::tempdir().unwrap();
        let dir = td.path().join(".quorum-rpc");
        ensure_mailbox(&dir).unwrap();
        assert!(dir.join("requests").is_dir());
        assert!(dir.join("responses").is_dir());
        // Idempotent.
        ensure_mailbox(&dir).unwrap();
    }

    #[test]
    fn file_age_is_small_for_a_fresh_file() {
        let td = tempfile::tempdir().unwrap();
        let f = td.path().join("fresh");
        std::fs::write(&f, b"x").unwrap();
        let age = file_age(&f).expect("fresh file has an age");
        // Just written, so well within the grace window (the give-up branch
        // keys off `age >= STALE_REQUEST_AGE`).
        assert!(age < STALE_REQUEST_AGE);
        // Missing file → None (not treated as stale).
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

        process_pending(&rpc_dir, td.path()).await;

        // Request consumed; response written.
        assert!(!rpc_dir.join("requests/req-1.json").exists());
        let body = std::fs::read_to_string(rpc_dir.join("responses/req-1.json")).unwrap();
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["id"], "req-1");
        assert_eq!(v["ok"], true);
        assert_eq!(v["exit_code"], 0);
        assert_eq!(v["stdout"], "pong");
        // No leftover temp file.
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

        process_pending(&rpc_dir, td.path()).await;

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

        process_pending(&rpc_dir, td.path()).await;

        // Within the grace window: left in place (could be a mid-write), no
        // response fabricated. Once older than STALE_REQUEST_AGE it would
        // instead get an ok:false error and be removed (see file_age logic).
        assert!(rpc_dir.join("requests/req-3.json").exists());
        assert!(!rpc_dir.join("responses/req-3.json").exists());
    }

    #[tokio::test]
    async fn git_status_runs_a_real_command() {
        // A non-repo cwd still exercises the spawn/capture/timeout path: git
        // exits non-zero with a message on stderr, which we report faithfully.
        let td = tempfile::tempdir().unwrap();
        let rpc_dir = td.path().join(".quorum-rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        write_request(
            &rpc_dir.join("requests"),
            "req-4.json",
            r#"{"id":"req-4","op":"git_status"}"#,
        );

        process_pending(&rpc_dir, td.path()).await;

        let body = std::fs::read_to_string(rpc_dir.join("responses/req-4.json")).unwrap();
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["id"], "req-4");
        // ok:true means the command ran (regardless of git's own exit code);
        // exit_code is present.
        assert_eq!(v["ok"], true);
        assert!(v["exit_code"].is_number());
    }
}
