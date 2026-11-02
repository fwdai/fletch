//! File-mailbox RPC between a sandboxed agent and the app.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::credential_file::{Entry, PrivateDir};
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

/// Lay out `dir` as a mailbox, owner-only. Its parent is the host's (the
/// mailbox root); everything from `dir` down is the agent's to write too, so
/// it is made by handle and a link found at any of the three is replaced by
/// a real directory, never created through or chmod-ed through.
pub fn ensure_mailbox(dir: &Path) -> Result<()> {
    let fail =
        |e: std::io::Error| Error::Other(format!("create rpc mailbox {}: {e}", dir.display()));
    let (Some(parent), Some(name)) = (dir.parent(), dir.file_name().and_then(|n| n.to_str()))
    else {
        return Err(fail(std::io::ErrorKind::InvalidInput.into()));
    };
    std::fs::create_dir_all(parent).map_err(fail)?;
    // The host's own path, so a link in it (a relocated `~/.fletch`) is followed.
    let parent = std::fs::canonicalize(parent).map_err(fail)?;
    let root = PrivateDir::open(&parent)
        .and_then(|p| p.subdir(name))
        .map_err(fail)?;
    // 0700 on the root and both subdirs: responses can carry sensitive tool output.
    root.set_mode(0o700).map_err(fail)?;
    for sub in [MAILBOX_REQUESTS, MAILBOX_RESPONSES] {
        root.subdir(sub)
            .and_then(|d| d.set_mode(0o700))
            .map_err(fail)?;
    }
    Ok(())
}

const MAILBOX_REQUESTS: &str = "requests";
const MAILBOX_RESPONSES: &str = "responses";

/// A mailbox opened by handle. The agent can write everything in it, so
/// past [`open`](Self::open) nothing in it is resolved by path again: the
/// requests are listed, read and removed, and the responses checked and
/// written, relative to these two handles. A path-based walk let an agent
/// swap `requests` for a link and have the host chmod that directory and
/// read and delete the `.json` files in it.
struct Mailbox {
    requests: PrivateDir,
    responses: PrivateDir,
}

impl Mailbox {
    /// Refuses a mailbox, `requests` or `responses` that is a link (or
    /// missing): one the agent tampered with is not served, and the next
    /// [`ensure_mailbox`] puts real directories back.
    fn open(dir: &Path) -> std::io::Result<Self> {
        let root = PrivateDir::open(dir)?;
        Ok(Self {
            requests: root.walk(Path::new(MAILBOX_REQUESTS), false)?,
            responses: root.walk(Path::new(MAILBOX_RESPONSES), false)?,
        })
    }

    /// The host only ever writes a plain file there; a link the agent
    /// planted at the name is not an answer, and the write replaces it.
    fn answered(&self, name: &str) -> bool {
        matches!(self.responses.entry(name), Ok(Entry::Other))
    }

    /// Write `<key>.json` in one rename: a link at the name is replaced,
    /// never written through.
    fn respond(&self, key: &str, resp: &Response) -> Result<()> {
        let json = serde_json::to_vec_pretty(resp)
            .map_err(|e| Error::Other(format!("serialize rpc response: {e}")))?;
        self.responses
            .write_file(&format!("{key}.json"), &json, 0o600)
            .map_err(|e| Error::Other(format!("write rpc response: {e}")))
    }

    /// Answer a request that is not dispatched with `error`, and only then
    /// remove it. A refusal runs nothing, so when the answer can't be written
    /// the request stays and the next tick refuses it again: the file is the
    /// retry. Removing it first left the agent waiting on an answer that never
    /// came.
    fn refuse(&self, key: &str, name: &str, why: &str, error: String) {
        let path = self.requests.path().join(name);
        report(&path, &format!("{why}, answering with error"), &error);
        match self.respond(key, &Response::err(key, error)) {
            Ok(()) => self.remove_request(name),
            Err(e) => report(&path, "writing a refusal failed, retrying next tick", &e),
        }
    }

    /// The answer to a dispatched request, whose file is already gone or about
    /// to be (the op must never run twice): written now, or held in
    /// [`undelivered`] for the next ticks to write.
    fn deliver(&self, key: &str, resp: Response) {
        if let Err(e) = self.respond(key, &resp) {
            let path = self.responses.path().join(format!("{key}.json"));
            report(
                &path,
                "writing a response failed, holding it for the next tick",
                &e,
            );
            undelivered().lock().insert(path, (key.to_string(), resp));
        }
    }

    /// Write this mailbox's held answers again. Each is taken out while it is
    /// written, so a concurrent tick never writes it twice, and put back if
    /// the write fails again.
    fn deliver_held(&self) {
        let held: Vec<(PathBuf, (String, Response))> = {
            let mut all = undelivered().lock();
            let mine: Vec<PathBuf> = all
                .keys()
                .filter(|path| path.parent() == Some(self.responses.path()))
                .cloned()
                .collect();
            mine.into_iter()
                .filter_map(|path| all.remove_entry(&path))
                .collect()
        };
        for (path, (key, resp)) in held {
            match self.respond(&key, &resp) {
                Ok(()) => {
                    reported().lock().remove(&path);
                    tracing::info!(file = %path.display(), "rpc: held response delivered");
                }
                Err(e) => {
                    report(
                        &path,
                        "writing a held response failed, retrying next tick",
                        &e,
                    );
                    undelivered().lock().insert(path, (key, resp));
                }
            }
        }
    }

    /// Removes the entry itself, a link included, never what it points at.
    /// Gone already is fine: a concurrent trigger beat us to it. The request
    /// is settled, so a later failure under its name is reported afresh.
    fn remove_request(&self, name: &str) {
        let path = self.requests.path().join(name);
        match self.requests.remove(name) {
            Ok(()) => {
                reported().lock().remove(&path);
            }
            Err(e) => report(&path, "remove request failed", &e),
        }
    }

    fn request_age(&self, name: &str) -> Option<Duration> {
        let modified = self.requests.modified(name).ok()?;
        SystemTime::now().duration_since(modified).ok()
    }
}

/// Best-effort: an absent dir is success, and a surviving mailbox is wasted
/// disk, never a correctness problem.
pub fn remove_mailbox(agent_id: &str) -> Result<()> {
    let dir = mailbox_dir(agent_id)?;
    forget_mailbox(&dir);
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
///
/// Every request ends with exactly one answer at `responses/<id>.json`, and
/// no op runs twice. A request that is not dispatched leaves the mailbox only
/// once its refusal is written ([`Mailbox::refuse`]); a dispatched one leaves
/// at once, and its answer is held until it is written
/// ([`Mailbox::deliver`]). Whatever keeps failing is retried every tick and
/// reported once ([`report`]), not every tick.
pub async fn process_pending(rpc_dir: &Path, dispatcher: &dyn RpcDispatcher) -> Vec<RpcEvent> {
    // Polled ten times a second, so a tampered mailbox is logged quietly.
    let mailbox = match Mailbox::open(rpc_dir) {
        Ok(m) => m,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::debug!(error = %e, dir = %rpc_dir.display(), "rpc: mailbox not served");
            }
            return Vec::new();
        }
    };
    mailbox.deliver_held();
    let Ok(names) = mailbox.requests.list() else {
        return Vec::new();
    };
    // A stuck request the agent removed itself is settled too.
    reported().lock().retain(|path| {
        path.parent() != Some(mailbox.requests.path())
            || path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| names.iter().any(|name| name == n))
    });

    let mut effects = Vec::new();
    for name in names {
        let Some(key) = name.strip_suffix(".json") else {
            continue;
        };
        effects.extend(handle_request(&mailbox, key, dispatcher).await);
    }
    effects
}

async fn handle_request(
    mailbox: &Mailbox,
    key: &str,
    dispatcher: &dyn RpcDispatcher,
) -> Vec<RpcEvent> {
    let name = format!("{key}.json");
    // Never dispatched, only refused: an empty id (`ID=$(uuidgen)` where there
    // is no uuidgen) was once skipped on every tick, and the agent waited on
    // `.json` for an answer that never came. The name is still one component,
    // which is all the response write and the removal need.
    if !is_safe_key(key) {
        mailbox.refuse(
            key,
            &name,
            "unsafe request filename",
            format!(
                "invalid request id {key:?}: use only letters, digits, '-' and '_' \
                 (an empty id usually means the command that made it, like uuidgen, \
                 is missing)"
            ),
        );
        return Vec::new();
    }

    // Claim before the response-file check: two concurrent triggers could
    // otherwise both dispatch. In-memory suffices; after a crash the request file
    // survives for retry.
    let Some(_claim) = InFlightClaim::acquire(&mailbox.requests.path().join(&name)) else {
        tracing::debug!(file = %name, "rpc: request already in flight, skipping");
        return Vec::new();
    };

    // Already answered by a tick that failed to remove the file, or a racing
    // trigger: never re-dispatch a side-effectful op.
    if mailbox.answered(&name) {
        mailbox.remove_request(&name);
        return Vec::new();
    }

    let raw = match mailbox.requests.read_file(&name) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(file = %name, "rpc: request vanished before read");
            return Vec::new();
        }
        // A link, a directory or a FIFO at the name: never a request an agent
        // wrote by the protocol, so refused rather than read.
        Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {
            mailbox.refuse(
                key,
                &name,
                "request is not a regular file",
                "request is not a regular file".into(),
            );
            return Vec::new();
        }
        Err(e) => {
            report(
                &mailbox.requests.path().join(&name),
                "read request failed",
                &e,
            );
            return Vec::new();
        }
    };

    let req: Request = match serde_json::from_slice(&raw) {
        Ok(r) => r,
        Err(e) => {
            if mailbox
                .request_age(&name)
                .is_some_and(|age| age >= STALE_REQUEST_AGE)
            {
                mailbox.refuse(
                    key,
                    &name,
                    "malformed request",
                    format!("malformed request JSON: {e}"),
                );
            } else {
                tracing::debug!(error = %e, file = %name, "rpc: unparseable request (will retry)");
            }
            return Vec::new();
        }
    };

    let (resp, effects) = dispatcher.dispatch(key, &req.op, &req.args).await;

    // The op ran; the request goes even if its answer can't be written yet,
    // so it is never dispatched again. The answer is held and written by a
    // later tick instead of lost.
    mailbox.deliver(key, resp);
    mailbox.remove_request(&name);

    effects
}

/// Answers to dispatched requests that could not be written yet, by response
/// path, with the request id they answer. The request is gone (its op must
/// never run twice), so this is all that still owes the agent its answer:
/// every tick of the mailbox writes them again ([`Mailbox::deliver_held`])
/// until one lands. In memory only: a restart ends the turn that was waiting.
fn undelivered() -> &'static Mutex<HashMap<PathBuf, (String, Response)>> {
    static UNDELIVERED: OnceLock<Mutex<HashMap<PathBuf, (String, Response)>>> = OnceLock::new();
    UNDELIVERED.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The requests and held answers a failure has been reported for and that
/// have not settled since. Everything stuck is retried every tick (100 ms),
/// and is worth one warning, not ten a second.
fn reported() -> &'static Mutex<HashSet<PathBuf>> {
    static REPORTED: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    REPORTED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Log `what` about `path`: a warning the first time since it last settled,
/// debug on every retry after that.
fn report(path: &Path, what: &str, error: &dyn std::fmt::Display) {
    if reported().lock().insert(path.to_path_buf()) {
        tracing::warn!(file = %path.display(), error = %error, "rpc: {what}");
    } else {
        tracing::debug!(file = %path.display(), error = %error, "rpc: {what}");
    }
}

/// Drop what is held or reported for the mailbox at `dir`: it is being
/// removed, and with it the agent that was owed the answers.
fn forget_mailbox(dir: &Path) {
    undelivered()
        .lock()
        .retain(|path, _| !path.starts_with(dir));
    reported().lock().retain(|path| !path.starts_with(dir));
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

fn is_safe_key(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
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
    fn a_fresh_request_is_young() {
        let td = tempfile::tempdir().unwrap();
        let dir = td.path().join(".fletch-rpc");
        ensure_mailbox(&dir).unwrap();
        write_request(&dir.join("requests"), "fresh.json", "{}");
        let mailbox = Mailbox::open(&dir).unwrap();
        let age = mailbox
            .request_age("fresh.json")
            .expect("fresh file has an age");
        assert!(age < STALE_REQUEST_AGE);
        assert!(mailbox.request_age("nope.json").is_none());
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::symlink_metadata(path)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777
    }

    #[cfg(unix)]
    fn is_link(path: &Path) -> bool {
        std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
    }

    /// A directory outside the mailbox the user can reach, holding a file
    /// that reads as a request: what an agent would aim a link at.
    #[cfg(unix)]
    fn elsewhere_with_precious(td: &Path) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let elsewhere = td.join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o755)).unwrap();
        let precious = elsewhere.join("precious.json");
        std::fs::write(&precious, r#"{"op":"ping"}"#).unwrap();
        (elsewhere, precious)
    }

    /// The agent can write its whole mailbox, so it can swap `requests` for
    /// a link: the host must neither serve nor delete what is behind it, nor
    /// chmod it when the mailbox is laid out again at the next launch.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_requests_dir_linked_elsewhere_is_never_read_deleted_or_chmod_ed() {
        let td = tempfile::tempdir().unwrap();
        let rpc_dir = td.path().join(".fletch-rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        let (elsewhere, precious) = elsewhere_with_precious(td.path());
        let requests = rpc_dir.join("requests");
        std::fs::remove_dir(&requests).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &requests).unwrap();

        let dispatcher = CountingDispatcher(std::sync::atomic::AtomicUsize::new(0));
        process_pending(&rpc_dir, &dispatcher).await;

        assert_eq!(dispatcher.0.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(
            std::fs::read_to_string(&precious).unwrap(),
            r#"{"op":"ping"}"#
        );
        assert!(!rpc_dir.join("responses/precious.json").exists());

        ensure_mailbox(&rpc_dir).unwrap();

        assert_eq!(mode(&elsewhere), 0o755, "the link's target was chmod-ed");
        assert!(precious.exists());
        assert!(!is_link(&requests) && requests.is_dir());
        assert_eq!(mode(&requests), 0o700);
    }

    /// The mailbox itself is inside the agent's grant: one swapped for a
    /// link is not served, and is replaced, not chmod-ed through.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_mailbox_linked_elsewhere_is_not_served_and_is_replaced() {
        let td = tempfile::tempdir().unwrap();
        let rpc_dir = td.path().join(".fletch-rpc");
        let (elsewhere, precious) = elsewhere_with_precious(td.path());
        std::fs::create_dir(elsewhere.join("requests")).unwrap();
        std::fs::create_dir(elsewhere.join("responses")).unwrap();
        std::fs::copy(&precious, elsewhere.join("requests/precious.json")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &rpc_dir).unwrap();

        let dispatcher = CountingDispatcher(std::sync::atomic::AtomicUsize::new(0));
        process_pending(&rpc_dir, &dispatcher).await;
        assert_eq!(dispatcher.0.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(elsewhere.join("requests/precious.json").exists());

        ensure_mailbox(&rpc_dir).unwrap();
        assert_eq!(mode(&elsewhere), 0o755);
        assert!(!is_link(&rpc_dir) && rpc_dir.join("requests").is_dir());
        assert!(elsewhere.join("requests/precious.json").exists());
    }

    /// A request that is a link is answered with an error and the link
    /// removed; what it points at is never read or deleted.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_request_linked_elsewhere_is_refused_and_only_the_link_removed() {
        let td = tempfile::tempdir().unwrap();
        let rpc_dir = td.path().join(".fletch-rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        let (_elsewhere, precious) = elsewhere_with_precious(td.path());
        let request = rpc_dir.join("requests/req-10.json");
        std::os::unix::fs::symlink(&precious, &request).unwrap();

        let dispatcher = CountingDispatcher(std::sync::atomic::AtomicUsize::new(0));
        process_pending(&rpc_dir, &dispatcher).await;

        assert_eq!(dispatcher.0.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(!is_link(&request) && !request.exists());
        assert_eq!(
            std::fs::read_to_string(&precious).unwrap(),
            r#"{"op":"ping"}"#
        );
        let body = std::fs::read_to_string(rpc_dir.join("responses/req-10.json")).unwrap();
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().contains("not a regular file"));
    }

    #[test]
    fn is_safe_key_rejects_traversal() {
        assert!(is_safe_key("abc-123_DEF"));
        assert!(!is_safe_key(""));
        assert!(!is_safe_key("../escape"));
        assert!(!is_safe_key("a/b"));
        assert!(!is_safe_key("with.dot"));
    }

    /// A request whose id is empty (`ID=$(uuidgen)` with no uuidgen) or
    /// otherwise unsafe is answered with an error at the name the agent polls
    /// and removed, never dispatched and never left to be skipped every tick.
    #[tokio::test]
    async fn an_unsafe_request_id_is_answered_and_removed() {
        let td = tempfile::tempdir().unwrap();
        let rpc_dir = td.path().join(".fletch-rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        for name in [".json", "with.dot.json"] {
            write_request(&rpc_dir.join("requests"), name, r#"{"op":"ping"}"#);
        }

        let dispatcher = CountingDispatcher(std::sync::atomic::AtomicUsize::new(0));
        process_pending(&rpc_dir, &dispatcher).await;

        assert_eq!(dispatcher.0.load(std::sync::atomic::Ordering::SeqCst), 0);
        for name in [".json", "with.dot.json"] {
            assert!(!rpc_dir.join("requests").join(name).exists(), "{name} left");
            let body = std::fs::read_to_string(rpc_dir.join("responses").join(name)).unwrap();
            let v: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(v["ok"], false);
            assert!(v["error"].as_str().unwrap().contains("invalid request id"));
        }
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
        // Nothing but the response: no temp file left beside it.
        let left: Vec<_> = std::fs::read_dir(rpc_dir.join("responses"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, ["req-1.json"]);
    }

    /// The agent names the request, so it can plant a link where the host
    /// would put the response, or its temp file, ahead of asking: the
    /// response must replace the link, never land in what it points at.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_response_never_follows_a_link_planted_in_the_mailbox() {
        let td = tempfile::tempdir().unwrap();
        let rpc_dir = td.path().join(".fletch-rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        let precious = td.path().join("precious");
        std::fs::write(&precious, "keep").unwrap();
        let responses = rpc_dir.join("responses");
        // The temp name the host once wrote through, and a dangling link at the
        // response's own name (an existing target would read as answered).
        std::os::unix::fs::symlink(&precious, responses.join("req-9.json.tmp")).unwrap();
        let unborn = td.path().join("unborn");
        std::os::unix::fs::symlink(&unborn, responses.join("req-9.json")).unwrap();
        write_request(
            &rpc_dir.join("requests"),
            "req-9.json",
            r#"{"id":"req-9","op":"ping"}"#,
        );

        process_pending(&rpc_dir, &MockDispatcher).await;

        assert_eq!(std::fs::read_to_string(&precious).unwrap(), "keep");
        assert!(!unborn.exists());
        let response = responses.join("req-9.json");
        assert!(!std::fs::symlink_metadata(&response)
            .unwrap()
            .file_type()
            .is_symlink());
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&response).unwrap()).unwrap();
        assert_eq!(v["stdout"], "pong");
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

    #[cfg(unix)]
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

        // A directory at the response's name is no answer, and no file can be
        // renamed over it, so the write fails (for root too, unlike a
        // read-only directory).
        let response = rpc_dir.join("responses/req-8.json");
        std::fs::create_dir(&response).unwrap();

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
        assert!(response.is_dir(), "no response was written");

        process_pending(&rpc_dir, &dispatcher).await;
        assert_eq!(
            dispatcher.0.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a write failure must not cause the op to run a second time"
        );
        assert!(response.is_dir(), "the write still fails");

        // Once the write can land, the held answer does: the agent is never
        // left waiting on an op that ran.
        std::fs::remove_dir(&response).unwrap();
        process_pending(&rpc_dir, &dispatcher).await;
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&response).unwrap()).unwrap();
        assert_eq!(v["id"], "req-8");
        assert_eq!(v["stdout"], "pong");
        assert_eq!(
            dispatcher.0.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "delivering the held answer runs nothing"
        );
        assert!(
            !reported().lock().contains(&response),
            "settled, not reported"
        );
    }

    /// A refusal runs nothing, so a request whose refusal can't be written
    /// stays for the next tick rather than being dropped unanswered: the agent
    /// polling for it would otherwise wait forever.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_refusal_that_cannot_be_written_is_retried_until_it_lands() {
        let td = tempfile::tempdir().unwrap();
        let rpc_dir = td.path().join(".fletch-rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        let request = rpc_dir.join("requests/.json");
        write_request(&rpc_dir.join("requests"), ".json", r#"{"op":"ping"}"#);
        let response = rpc_dir.join("responses/.json");
        std::fs::create_dir(&response).unwrap();

        let dispatcher = CountingDispatcher(std::sync::atomic::AtomicUsize::new(0));
        for _ in 0..2 {
            process_pending(&rpc_dir, &dispatcher).await;
            assert!(
                request.exists(),
                "an unanswered refusal must keep its request"
            );
            assert!(response.is_dir());
            assert!(reported().lock().contains(&request), "reported once, kept");
        }

        std::fs::remove_dir(&response).unwrap();
        process_pending(&rpc_dir, &dispatcher).await;

        assert_eq!(dispatcher.0.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(!request.exists());
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&response).unwrap()).unwrap();
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().contains("invalid request id"));
        assert!(
            !reported().lock().contains(&request),
            "settled, not reported"
        );
    }

    /// Removing a mailbox drops what was held for it: nobody is left to read
    /// the answers, and nothing may linger for a later mailbox at that path.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_removed_mailbox_forgets_its_held_answers() {
        let td = tempfile::tempdir().unwrap();
        let rpc_dir = td.path().join(".fletch-rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        write_request(&rpc_dir.join("requests"), "req-11.json", r#"{"op":"ping"}"#);
        let response = rpc_dir.join("responses/req-11.json");
        std::fs::create_dir(&response).unwrap();

        process_pending(&rpc_dir, &MockDispatcher).await;
        assert!(undelivered().lock().contains_key(&response));

        forget_mailbox(&rpc_dir);
        assert!(!undelivered().lock().contains_key(&response));
        assert!(!reported().lock().contains(&response));
    }

    #[tokio::test]
    async fn concurrently_deleted_request_is_tolerated() {
        let td = tempfile::tempdir().unwrap();
        let rpc_dir = td.path().join(".fletch-rpc");
        ensure_mailbox(&rpc_dir).unwrap();

        let mailbox = Mailbox::open(&rpc_dir).unwrap();
        let effects = handle_request(&mailbox, "req-5", &MockDispatcher).await;

        assert!(effects.is_empty());
        assert!(!rpc_dir.join("responses/req-5.json").exists());
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
