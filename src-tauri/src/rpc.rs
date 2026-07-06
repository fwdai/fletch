//! File-mailbox RPC between a sandboxed agent and the app.
//!
//! This module owns the transport: mailbox layout, atomic request/response
//! handling, and the dispatcher trait. Feature-specific behavior lives behind
//! dispatchers such as `rpc::git::GitDispatcher`.

use std::collections::HashSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use parking_lot::Mutex;
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

/// Env var overriding the mailbox root (default `~/.fletch/rpc`). The Run
/// sandbox forbids writes to the host's `~/.fletch/rpc`, so a nested Fletch
/// launched as a Run process (dogfooding: Fletch running Fletch) is pointed at
/// a sandbox-writable root instead — see `sandbox::nested_rpc_root`.
pub const RPC_ROOT_ENV: &str = "FLETCH_RPC_ROOT";

/// `<root>/<agent-id>/` — the agent's private mailbox root. `<root>` is
/// `$FLETCH_RPC_ROOT` when set and non-empty, else `~/.fletch/rpc`.
pub fn mailbox_dir(agent_id: &str) -> Result<PathBuf> {
    Ok(rpc_root()?.join(agent_id))
}

fn rpc_root() -> Result<PathBuf> {
    match std::env::var_os(RPC_ROOT_ENV).filter(|v| !v.is_empty()) {
        Some(root) => Ok(PathBuf::from(root)),
        None => {
            let home = dirs::home_dir()
                .ok_or_else(|| Error::Other("HOME directory not available".into()))?;
            Ok(home.join(".fletch").join("rpc"))
        }
    }
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

    // Claim the request for this trigger. The response-file check below only
    // covers *answered* requests; without the claim, two concurrent triggers
    // (an overlapping watcher generation still draining a slow op, or a future
    // FS-event trigger racing the poll tick) could both scan the request
    // before either writes its response and dispatch it twice. In-memory is
    // enough: every trigger for a mailbox lives in this process, and after a
    // crash the request file survives for the next start to retry.
    let Some(_claim) = InFlightClaim::acquire(path) else {
        tracing::debug!(file = %path.display(), "rpc: request already in flight, skipping");
        return Vec::new();
    };

    // A response for this id already exists: a previous tick answered the
    // request but failed to remove its file, or two triggers raced on the same
    // scan. Never re-dispatch — ops can have side effects (e.g. a git push).
    // Just finish the cleanup.
    if responses.join(format!("{stem}.json")).exists() {
        remove_request_file(path);
        return Vec::new();
    }

    let raw = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // A concurrent trigger consumed the file between the directory
            // scan and this read — already handled, nothing to do.
            tracing::debug!(file = %path.display(), "rpc: request vanished before read");
            return Vec::new();
        }
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
                    remove_request_file(path);
                }
            } else {
                tracing::debug!(error = %e, file = %path.display(), "rpc: unparseable request (will retry)");
            }
            return Vec::new();
        }
    };

    let (resp, effects) = dispatcher.dispatch(stem, &req.op, &req.args).await;

    // The op has now run and may have had side effects (e.g. a git push). Even
    // if writing the response fails, we must remove the request file so the
    // next poll tick can't re-acquire and re-dispatch it — a lost response (the
    // caller times out waiting) is strictly safer than executing a
    // side-effectful op twice. The request stays only if the process crashes
    // before this point, which preserves at-least-once for un-dispatched ops.
    if let Err(e) = write_response_atomic(responses, stem, &resp) {
        tracing::error!(error = %e, id = %stem, "rpc: write response failed after dispatch; dropping request to avoid re-execution");
    }

    remove_request_file(path);

    effects
}

/// Request files currently being processed somewhere in this process. Guards
/// the dispatch of side-effectful ops against concurrent triggers; see the
/// claim site in `handle_request_file`.
fn in_flight() -> &'static Mutex<HashSet<PathBuf>> {
    static IN_FLIGHT: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new()))
}

/// RAII entry in `in_flight()`: released on drop, so every early return in
/// `handle_request_file` unclaims automatically.
struct InFlightClaim(PathBuf);

impl InFlightClaim {
    fn acquire(path: &Path) -> Option<Self> {
        in_flight()
            .lock()
            .insert(path.to_path_buf())
            .then(|| Self(path.to_path_buf()))
    }
}

impl Drop for InFlightClaim {
    fn drop(&mut self) {
        in_flight().lock().remove(&self.0);
    }
}

/// Delete a handled request file. NotFound is a no-op — a concurrent trigger
/// beat us to the cleanup, which is fine now that the request is answered.
fn remove_request_file(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(error = %e, file = %path.display(), "rpc: remove request failed");
        }
    }
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

    /// Like `MockDispatcher`, but counts dispatch calls so tests can assert an
    /// already-answered request is never re-executed.
    struct CountingDispatcher(std::sync::atomic::AtomicUsize);

    impl RpcDispatcher for CountingDispatcher {
        fn dispatch<'a>(
            &'a self,
            id: &'a str,
            _op: &'a str,
            _args: &'a Value,
        ) -> RpcFuture<'a, (Response, Vec<RpcEvent>)> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async move {
                (
                    Response::ok(id, 0, "pong".to_string(), String::new()),
                    Vec::new(),
                )
            })
        }
    }

    /// Counts dispatch entries and then parks until the test hands it a
    /// permit, so a test can hold a request in flight deterministically.
    struct BlockingDispatcher {
        calls: std::sync::atomic::AtomicUsize,
        release: tokio::sync::Semaphore,
    }

    impl RpcDispatcher for BlockingDispatcher {
        fn dispatch<'a>(
            &'a self,
            id: &'a str,
            _op: &'a str,
            _args: &'a Value,
        ) -> RpcFuture<'a, (Response, Vec<RpcEvent>)> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async move {
                let _permit = self.release.acquire().await.unwrap();
                (
                    Response::ok(id, 0, "pong".to_string(), String::new()),
                    Vec::new(),
                )
            })
        }
    }

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
        let dir = td.path().join(".fletch-rpc");
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
        let rpc_dir = td.path().join(".fletch-rpc");
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
        let rpc_dir = td.path().join(".fletch-rpc");
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
        let rpc_dir = td.path().join(".fletch-rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        write_request(&rpc_dir.join("requests"), "req-3.json", "{ not json");

        let dispatcher = MockDispatcher;
        process_pending(&rpc_dir, &dispatcher).await;

        assert!(rpc_dir.join("requests/req-3.json").exists());
        assert!(!rpc_dir.join("responses/req-3.json").exists());
    }

    #[tokio::test]
    async fn request_with_existing_response_is_not_redispatched() {
        let td = tempfile::tempdir().unwrap();
        let rpc_dir = td.path().join(".fletch-rpc");
        ensure_mailbox(&rpc_dir).unwrap();

        // A previous tick answered req-4 but its request file lingered
        // (remove failed or two triggers raced on the same scan).
        std::fs::write(
            rpc_dir.join("responses/req-4.json"),
            r#"{"id":"req-4","ok":true,"exit_code":0,"stdout":"first","stderr":""}"#,
        )
        .unwrap();
        write_request(
            &rpc_dir.join("requests"),
            "req-4.json",
            r#"{"id":"req-4","op":"ping"}"#,
        );

        let dispatcher = CountingDispatcher(std::sync::atomic::AtomicUsize::new(0));
        process_pending(&rpc_dir, &dispatcher).await;

        assert_eq!(
            dispatcher.0.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "an already-answered request must never be re-dispatched"
        );
        assert!(!rpc_dir.join("requests/req-4.json").exists());
        let body = std::fs::read_to_string(rpc_dir.join("responses/req-4.json")).unwrap();
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["stdout"], "first", "original response must be preserved");
    }

    /// A request that is being dispatched (no response written yet) must not
    /// be dispatched again by a concurrent trigger — e.g. an old watcher
    /// generation still draining a slow `git_push` while the respawned
    /// generation starts ticking, or an FS-event trigger racing the poll tick.
    #[tokio::test]
    async fn in_flight_request_is_not_redispatched_by_a_concurrent_trigger() {
        let td = tempfile::tempdir().unwrap();
        let rpc_dir = td.path().join(".fletch-rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        write_request(
            &rpc_dir.join("requests"),
            "req-7.json",
            r#"{"id":"req-7","op":"ping"}"#,
        );

        let dispatcher = std::sync::Arc::new(BlockingDispatcher {
            calls: std::sync::atomic::AtomicUsize::new(0),
            release: tokio::sync::Semaphore::new(0),
        });

        let dir = rpc_dir.clone();
        let d = dispatcher.clone();
        let first = tokio::spawn(async move { process_pending(&dir, d.as_ref()).await });

        // Wait until the first trigger is inside dispatch: claim held, no
        // response written yet — exactly the window the exists() check misses.
        while dispatcher.calls.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // A second trigger scans the same request mid-flight.
        process_pending(&rpc_dir, dispatcher.as_ref()).await;
        assert_eq!(
            dispatcher.calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "an in-flight request must not be re-dispatched"
        );

        dispatcher.release.add_permits(1);
        first.await.unwrap();

        assert_eq!(
            dispatcher.calls.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert!(!rpc_dir.join("requests/req-7.json").exists());
        let body = std::fs::read_to_string(rpc_dir.join("responses/req-7.json")).unwrap();
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], true);
    }

    /// A response-write failure *after* dispatch must not leave the request
    /// eligible for a second dispatch: the op already ran (possibly with side
    /// effects like a git push), so the request file is dropped even though no
    /// response was written. A later tick then finds nothing to re-execute.
    #[tokio::test]
    async fn write_failure_after_dispatch_does_not_redispatch() {
        let td = tempfile::tempdir().unwrap();
        let rpc_dir = td.path().join(".fletch-rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        write_request(
            &rpc_dir.join("requests"),
            "req-8.json",
            r#"{"id":"req-8","op":"ping"}"#,
        );

        // Force write_response_atomic to fail deterministically: it writes to
        // `responses/req-8.json.tmp` first, so a directory in that spot makes
        // the write error out on every platform.
        std::fs::create_dir(rpc_dir.join("responses/req-8.json.tmp")).unwrap();

        let dispatcher = CountingDispatcher(std::sync::atomic::AtomicUsize::new(0));
        process_pending(&rpc_dir, &dispatcher).await;

        assert_eq!(
            dispatcher.0.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the op runs once"
        );
        assert!(
            !rpc_dir.join("requests/req-8.json").exists(),
            "request must be removed even when the response write fails"
        );
        assert!(!rpc_dir.join("responses/req-8.json").exists());

        // A subsequent tick must not re-run the (side-effectful) op.
        process_pending(&rpc_dir, &dispatcher).await;
        assert_eq!(
            dispatcher.0.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a write failure must not cause the op to run a second time"
        );
    }

    #[tokio::test]
    async fn concurrently_deleted_request_is_tolerated() {
        let td = tempfile::tempdir().unwrap();
        let rpc_dir = td.path().join(".fletch-rpc");
        ensure_mailbox(&rpc_dir).unwrap();

        // The file vanished between the directory scan and the read.
        let gone = rpc_dir.join("requests/req-5.json");
        let effects = handle_request_file(&gone, &rpc_dir.join("responses"), &MockDispatcher).await;

        assert!(effects.is_empty());
        assert!(!rpc_dir.join("responses/req-5.json").exists());
    }

    /// With no FS-event mechanism anywhere (this transport has none — the
    /// watcher is a bare interval tick), a dropped request file is answered
    /// within 1s by the poll path alone. The tick here mirrors the 500ms
    /// fallback; production polls even faster (`RPC_TICK`).
    #[tokio::test]
    async fn poll_tick_answers_request_within_a_second_without_fs_events() {
        let td = tempfile::tempdir().unwrap();
        let rpc_dir = td.path().join(".fletch-rpc");
        ensure_mailbox(&rpc_dir).unwrap();

        let dir = rpc_dir.clone();
        let poller = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                process_pending(&dir, &MockDispatcher).await;
            }
        });

        write_request(
            &rpc_dir.join("requests"),
            "req-6.json",
            r#"{"id":"req-6","op":"ping"}"#,
        );

        let response = rpc_dir.join("responses/req-6.json");
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while !response.exists() && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        poller.abort();

        assert!(
            response.exists(),
            "poll fallback did not answer the request within 1s"
        );
        let v: Value = serde_json::from_str(&std::fs::read_to_string(response).unwrap()).unwrap();
        assert_eq!(v["ok"], true);
        assert!(!rpc_dir.join("requests/req-6.json").exists());
    }
}
