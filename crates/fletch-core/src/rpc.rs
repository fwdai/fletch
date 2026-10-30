//! File-mailbox RPC between a sandboxed agent and the app.

use std::collections::HashSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::confined::Dir;
use crate::error::{Error, Result};

/// Grace window for a non-compliant agent caught mid-write; a compliant one
/// renames atomically. Past it the file is answered with an error and removed.
const STALE_REQUEST_AGE: Duration = Duration::from_secs(5);

#[path = "rpc/approval.rs"]
pub mod approval;
#[path = "rpc/caps.rs"]
pub mod caps;
#[path = "rpc/context/mod.rs"]
pub mod context;
#[path = "rpc/git/mod.rs"]
pub mod git;
#[path = "rpc/review.rs"]
pub mod review;
#[path = "rpc/roadmap/mod.rs"]
pub mod roadmap;

/// The Run panel's process and a notification, as mailbox ops.
pub mod run;

/// `id` comes from the filename, not the body; unknown body fields are ignored.
#[derive(Debug, Deserialize)]
pub struct Request {
    pub op: String,
    #[serde(default)]
    pub args: Value,
}

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

/// A nested Fletch launched as a Run process can't write the host's
/// `~/.fletch/rpc` — see `sandbox::nested_rpc_root`.
pub const RPC_ROOT_ENV: &str = "FLETCH_RPC_ROOT";

/// `<root>/<agent-id>/`; root is `rpc/` for release, `dev/rpc/` for debug.
pub fn mailbox_dir(agent_id: &str) -> Result<PathBuf> {
    Ok(rpc_root()?.join(agent_id))
}

pub(crate) fn rpc_root() -> Result<PathBuf> {
    let base = match std::env::var_os(RPC_ROOT_ENV).filter(|v| !v.is_empty()) {
        Some(root) => PathBuf::from(root),
        None => dirs::home_dir()
            .ok_or_else(|| Error::Other("HOME directory not available".into()))?
            .join(".fletch"),
    };
    Ok(rpc_root_in(&base))
}

/// Override and default paths share this step so neither can bypass the
/// per-build split.
fn rpc_root_in(base: &Path) -> PathBuf {
    base.join(crate::build_state_subpath("rpc"))
}

/// Create the mailbox at `dir` (`<root>/<agent-id>`) and its `requests` and
/// `responses`, owner-only. The agent's grant covers the mailbox from its own
/// entry down, so whatever it left at any of the three names — a link
/// included — is replaced by a real directory rather than followed.
pub fn ensure_mailbox(dir: &Path) -> Result<()> {
    let (Some(root), Some(name)) = (dir.parent(), dir.file_name()) else {
        return Err(Error::InvalidPath(dir.display().to_string()));
    };
    let failed = |what: &str, e: std::io::Error| {
        Error::Other(format!("{what} rpc mailbox {}: {e}", dir.display()))
    };
    // The root is the host's: no agent is granted more than its own mailbox.
    std::fs::create_dir_all(root).map_err(|e| failed("create", e))?;
    let mailbox = Dir::trusted(root)
        .and_then(|root| root.claim_dir(name))
        .map_err(|e| failed("create", e))?;
    // 0700 on the root and both subdirs: responses can carry sensitive tool output.
    mailbox.make_private().map_err(|e| failed("chmod", e))?;
    for sub in ["requests", "responses"] {
        let sub = mailbox.claim_dir(sub).map_err(|e| failed("create", e))?;
        sub.make_private().map_err(|e| failed("chmod", e))?;
    }
    Ok(())
}

/// A mailbox's two directories, held open: the agent can swap any of the
/// mailbox's names for a link at any moment, so nothing below is reached by
/// path again.
struct Mailbox {
    requests: Dir,
    responses: Dir,
}

impl Mailbox {
    fn open(dir: &Path) -> std::io::Result<Self> {
        let mailbox = Dir::open(dir, 1)?;
        Ok(Self {
            requests: mailbox.open_dir("requests")?,
            responses: mailbox.open_dir("responses")?,
        })
    }
}

/// Best-effort: an absent dir is success, and a surviving mailbox is wasted
/// disk, never a correctness problem.
pub fn remove_mailbox(agent_id: &str) -> Result<()> {
    let dir = mailbox_dir(agent_id)?;
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::Other(format!(
            "remove rpc mailbox {}: {e}",
            dir.display()
        ))),
    }
}

/// Remove every mailbox under `root` that no agent in `live` owns.
///
/// Sound only because the root is per-build *and* owned by the engine `live`
/// came from: every mailbox under it belongs to that engine's database. Whether
/// this engine owns its root is the host's answer, not this module's — see
/// [`crate::host::StateRoots`], which is what decides whether `boot` calls this
/// at all. A nested Fletch's root belongs to the parent, which reclaims it in
/// `sandbox::cleanup_nested_rpc_roots`.
///
/// Takes the root as an argument rather than reading `RPC_ROOT_ENV` so tests
/// never mutate the process-global override.
pub(crate) fn sweep_orphan_mailboxes_in(root: &Path, live: &HashSet<String>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if live.contains(&name) || !entry.path().is_dir() {
            continue;
        }
        match std::fs::remove_dir_all(entry.path()) {
            Ok(()) => removed += 1,
            Err(e) => {
                tracing::warn!(agent_id = %name, error = %e, "orphan rpc mailbox removal failed")
            }
        }
    }
    if removed > 0 {
        tracing::info!(removed, live = live.len(), "swept orphan rpc mailboxes");
    }
}

/// One bad request file is logged and isolated, never stalls the rest.
pub async fn process_pending(rpc_dir: &Path, dispatcher: &dyn RpcDispatcher) -> Vec<RpcEvent> {
    let mailbox = match Mailbox::open(rpc_dir) {
        Ok(m) => m,
        Err(e) => {
            tracing::debug!(error = %e, dir = %rpc_dir.display(), "rpc: mailbox not openable");
            return Vec::new();
        }
    };
    let names = match mailbox.requests.entries() {
        Ok(names) => names,
        Err(_) => return Vec::new(),
    };

    let mut effects = Vec::new();
    for name in names {
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.ends_with(".json") {
            continue;
        }
        effects.extend(handle_request_file(&mailbox, name, dispatcher).await);
    }
    effects
}

async fn handle_request_file(
    mailbox: &Mailbox,
    name: &str,
    dispatcher: &dyn RpcDispatcher,
) -> Vec<RpcEvent> {
    let path = mailbox.requests.path().join(name);
    let Some(stem) = name.strip_suffix(".json") else {
        return Vec::new();
    };
    if !is_safe_key(stem) {
        tracing::warn!(file = %path.display(), "rpc: unsafe request filename, skipping");
        return Vec::new();
    }

    // Claim before the response-file check: two concurrent triggers could
    // otherwise both dispatch. In-memory suffices; after a crash the request file
    // survives for retry.
    let Some(_claim) = InFlightClaim::acquire(&path) else {
        tracing::debug!(file = %path.display(), "rpc: request already in flight, skipping");
        return Vec::new();
    };

    // Already answered by a tick that failed to remove the file, or a racing
    // trigger: never re-dispatch a side-effectful op.
    if mailbox
        .responses
        .contains(format!("{stem}.json"))
        .unwrap_or(false)
    {
        remove_request_file(mailbox, name);
        return Vec::new();
    }

    let raw = match mailbox.requests.read(name) {
        Ok(Some(b)) => b,
        // Not a regular file: a link (read through, it would hand the agent
        // any file of the user's) or a FIFO. Never readable, so answered and
        // dropped at once rather than left to age.
        Ok(None) if mailbox.requests.contains(name).unwrap_or(false) => {
            tracing::warn!(file = %path.display(), "rpc: request is not a regular file, refusing");
            let resp = Response::err(stem, "request is not a regular file");
            if write_response_atomic(&mailbox.responses, stem, &resp).is_ok() {
                remove_request_file(mailbox, name);
            }
            return Vec::new();
        }
        Ok(None) => {
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
            if file_age(&mailbox.requests, name).is_some_and(|age| age >= STALE_REQUEST_AGE) {
                tracing::warn!(error = %e, file = %path.display(), "rpc: malformed request, answering with error");
                let resp = Response::err(stem, format!("malformed request JSON: {e}"));
                if write_response_atomic(&mailbox.responses, stem, &resp).is_ok() {
                    remove_request_file(mailbox, name);
                }
            } else {
                tracing::debug!(error = %e, file = %path.display(), "rpc: unparseable request (will retry)");
            }
            return Vec::new();
        }
    };

    let (resp, effects) = dispatcher.dispatch(stem, &req.op, &req.args).await;

    // The op ran; remove the request even if writing the response fails, so it
    // can't be re-dispatched. A lost response is safer than a doubled push.
    if let Err(e) = write_response_atomic(&mailbox.responses, stem, &resp) {
        tracing::error!(error = %e, id = %stem, "rpc: write response failed after dispatch; dropping request to avoid re-execution");
    }

    remove_request_file(mailbox, name);

    effects
}

fn in_flight() -> &'static Mutex<HashSet<PathBuf>> {
    static IN_FLIGHT: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new()))
}

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

/// Nothing there is a no-op: a concurrent trigger beat us to it.
fn remove_request_file(mailbox: &Mailbox, name: &str) {
    if let Err(e) = mailbox.requests.remove(name) {
        tracing::warn!(error = %e, file = %mailbox.requests.path().join(name).display(), "rpc: remove request failed");
    }
}

/// A temp written in `responses` and renamed to `<key>.json`, so the agent
/// never sees a half-written response — and, the key being the agent's choice,
/// a link it planted at either name is replaced, never written through.
fn write_response_atomic(responses: &Dir, key: &str, resp: &Response) -> Result<()> {
    let json = serde_json::to_vec_pretty(resp)
        .map_err(|e| Error::Other(format!("serialize rpc response: {e}")))?;
    responses
        .write(format!("{key}.json"), &json)
        .map_err(|e| Error::Other(format!("write rpc response: {e}")))
}

fn is_safe_key(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn file_age(dir: &Dir, name: &str) -> Option<Duration> {
    let modified = dir.modified(name).ok()?;
    SystemTime::now().duration_since(modified).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_request(requests: &Path, name: &str, body: &str) {
        std::fs::write(requests.join(name), body).unwrap();
    }

    fn live(names: &[&str]) -> HashSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn mailbox_root_is_split_per_build() {
        let base = Path::new("/tmp/shared-base");
        assert_eq!(rpc_root_in(base), base.join("dev").join("rpc"));
    }

    #[test]
    fn sweep_cannot_reach_another_builds_live_mailboxes() {
        let td = tempfile::tempdir().unwrap();
        let ours = rpc_root_in(td.path());
        let theirs = td.path().join("rpc"); // the release install's, same base
        ensure_mailbox(&ours.join("bromo")).unwrap();
        ensure_mailbox(&theirs.join("pilbara")).unwrap();

        sweep_orphan_mailboxes_in(&ours, &live(&["bromo"]));

        assert!(
            theirs.join("pilbara").is_dir(),
            "another build's live mailbox was swept"
        );
        assert!(ours.join("bromo").is_dir());
    }

    #[test]
    fn sweep_removes_only_mailboxes_without_a_live_agent() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        for name in ["everest", "yellowstone", "gobi"] {
            ensure_mailbox(&root.join(name)).unwrap();
        }

        sweep_orphan_mailboxes_in(root, &live(&["everest"]));

        assert!(root.join("everest").is_dir(), "live mailbox was removed");
        assert!(!root.join("yellowstone").exists());
        assert!(!root.join("gobi").exists());
    }

    #[test]
    fn sweep_removes_a_mailbox_with_pending_traffic_in_it() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        let dir = root.join("gobi");
        ensure_mailbox(&dir).unwrap();
        write_request(&dir.join("requests"), "abc.json", r#"{"op":"echo"}"#);

        sweep_orphan_mailboxes_in(root, &HashSet::new());

        assert!(!dir.exists());
    }

    #[test]
    fn sweep_leaves_stray_files_and_a_missing_root_alone() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        std::fs::write(root.join("notes.txt"), "not a mailbox").unwrap();

        sweep_orphan_mailboxes_in(root, &HashSet::new());
        sweep_orphan_mailboxes_in(&root.join("does-not-exist"), &HashSet::new());

        assert!(
            root.join("notes.txt").is_file(),
            "non-dir entry was removed"
        );
    }

    #[test]
    fn sweep_is_idempotent() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        ensure_mailbox(&root.join("gobi")).unwrap();

        sweep_orphan_mailboxes_in(root, &HashSet::new());
        sweep_orphan_mailboxes_in(root, &HashSet::new());

        assert!(!root.join("gobi").exists());
    }

    struct MockDispatcher;

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
        std::fs::write(td.path().join("fresh"), b"x").unwrap();
        let dir = Dir::trusted(td.path()).unwrap();
        let age = file_age(&dir, "fresh").expect("fresh file has an age");
        assert!(age < STALE_REQUEST_AGE);
        assert!(file_age(&dir, "nope").is_none());
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

        while dispatcher.calls.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

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

        // The op deletes `responses` out from under the held handle, so the
        // response write fails on every platform: nothing can be created in a
        // removed directory.
        struct DeletingDispatcher(std::sync::atomic::AtomicUsize, PathBuf);
        impl RpcDispatcher for DeletingDispatcher {
            fn dispatch<'a>(
                &'a self,
                id: &'a str,
                _op: &'a str,
                _args: &'a Value,
            ) -> RpcFuture<'a, (Response, Vec<RpcEvent>)> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::fs::remove_dir(&self.1).unwrap();
                Box::pin(async move {
                    (
                        Response::ok(id, 0, "pong".into(), String::new()),
                        Vec::new(),
                    )
                })
            }
        }
        let responses = rpc_dir.join("responses");
        let dispatcher =
            DeletingDispatcher(std::sync::atomic::AtomicUsize::new(0), responses.clone());
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
        assert!(!responses.join("req-8.json").exists());

        std::fs::create_dir(&responses).unwrap();
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

        let mailbox = Mailbox::open(&rpc_dir).unwrap();
        let effects = handle_request_file(&mailbox, "req-5.json", &MockDispatcher).await;

        assert!(effects.is_empty());
        assert!(!rpc_dir.join("responses/req-5.json").exists());
    }

    #[tokio::test]
    async fn links_planted_at_response_names_are_never_written_through() {
        // The key is the agent's choice, so it can plant a link at the very
        // name the host is about to write — no race needed.
        let td = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let rpc_dir = td.path().join(".fletch-rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        let responses = rpc_dir.join("responses");
        let victim = outside.path().join("victim");
        std::fs::write(&victim, "user file").unwrap();

        // At the temp name the writer once used.
        std::os::unix::fs::symlink(&victim, responses.join("req-9.json.tmp")).unwrap();
        write_request(
            &rpc_dir.join("requests"),
            "req-9.json",
            r#"{"id":"req-9","op":"ping"}"#,
        );
        process_pending(&rpc_dir, &MockDispatcher).await;
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "user file");
        let body = std::fs::read_to_string(responses.join("req-9.json")).unwrap();
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], true);

        // At the final name: the request reads as answered and is dropped
        // without a write.
        std::os::unix::fs::symlink(&victim, responses.join("req-11.json")).unwrap();
        write_request(
            &rpc_dir.join("requests"),
            "req-11.json",
            r#"{"id":"req-11","op":"ping"}"#,
        );
        let dispatcher = CountingDispatcher(std::sync::atomic::AtomicUsize::new(0));
        process_pending(&rpc_dir, &dispatcher).await;
        assert_eq!(dispatcher.0.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "user file");
        assert!(!rpc_dir.join("requests/req-11.json").exists());
    }

    #[tokio::test]
    async fn a_linked_request_is_refused_and_its_target_left_alone() {
        // A request that links to a user file: read through, its contents
        // would reach the agent in the error; removed through, the file would go.
        let td = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let rpc_dir = td.path().join(".fletch-rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        let secret = outside.path().join("secret.json");
        std::fs::write(&secret, r#"{"op":"ping","token":"s3cret"}"#).unwrap();
        std::os::unix::fs::symlink(&secret, rpc_dir.join("requests/req-10.json")).unwrap();

        let dispatcher = CountingDispatcher(std::sync::atomic::AtomicUsize::new(0));
        process_pending(&rpc_dir, &dispatcher).await;

        assert_eq!(dispatcher.0.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(secret.exists(), "the link's target must survive");
        assert!(std::fs::symlink_metadata(rpc_dir.join("requests/req-10.json")).is_err());
        let body = std::fs::read_to_string(rpc_dir.join("responses/req-10.json")).unwrap();
        assert!(!body.contains("s3cret"), "{body}");
    }

    #[tokio::test]
    async fn a_linked_requests_dir_is_never_read_or_cleaned() {
        // `requests` swapped for a link to a user directory: the host must not
        // treat that directory's `*.json` files as requests (and delete them).
        let td = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let rpc_dir = td.path().join(".fletch-rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        let package = outside.path().join("package.json");
        std::fs::write(&package, "{ not a request").unwrap();
        std::fs::remove_dir(rpc_dir.join("requests")).unwrap();
        std::os::unix::fs::symlink(outside.path(), rpc_dir.join("requests")).unwrap();

        process_pending(&rpc_dir, &MockDispatcher).await;
        assert!(package.exists());

        // The next spawn's mailbox setup replaces the link with a real dir.
        ensure_mailbox(&rpc_dir).unwrap();
        assert!(!std::fs::symlink_metadata(rpc_dir.join("requests"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(package.exists());
    }

    #[test]
    fn a_linked_mailbox_is_replaced_and_its_target_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt;

        let td = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::set_permissions(outside.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let rpc_dir = td.path().join("agent-1");
        std::os::unix::fs::symlink(outside.path(), &rpc_dir).unwrap();

        ensure_mailbox(&rpc_dir).unwrap();

        assert!(rpc_dir.join("requests").is_dir());
        assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
        let mode = std::fs::metadata(outside.path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755, "chmod went through the link");
    }

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
