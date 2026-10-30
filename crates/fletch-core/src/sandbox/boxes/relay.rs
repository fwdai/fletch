//! The two long-lived `fletch-guest` sessions per running box, each a plain
//! ssh child the registry supervises:
//!
//! - **Mailbox**: `fletch-guest relay <rpc dir>` emits every request file the
//!   agent writes in the guest as one JSON line; the host writes it into its
//!   own `rpc_dir/requests/` (atomically, so `supervisor::rpc_watch` picks it
//!   up unchanged) and then acks it, which is what lets the guest delete its
//!   copy. The host watches `rpc_dir/responses/` and sends each response back
//!   as a line, deleting the host copy only on the guest's ack. Either side
//!   re-sends what is still on its disk when a session reconnects, so the
//!   receiver dedupes by name with a receipt marker on disk: the host's live
//!   in `<box dir>/relay/delivered/<name>`, which outlives idle stops and app
//!   restarts and goes with the box on discard (`rpc_watch` consumes the
//!   request file, so a replayed request would otherwise run its op twice);
//!   the guest's in `/data/run/relay/delivered/<name>` on the data disk.
//!   Publication is ordered so the marker exists before the watcher can see
//!   the file: body to `requests/.<name>.staging` (not `*.json`, so unlisted),
//!   marker, rename, ack. A crash between any two steps recovers from the
//!   files alone on replay or at the next session's start-up sweep
//!   (`deliver_request`, `finish_staged`), so no op runs twice. A
//!   marker is only needed while the sender may still hold the file, and a
//!   sender re-sends everything it still holds at session start and then says
//!   `synced`, so on `synced` the receiver deletes every marker it was not
//!   shown this session. Markers made after the sync wait for the next one.
//! - **Transcript tail**: `fletch-guest tail <sessions dir>` emits
//!   `create`/`append` lines for every `*.jsonl` under the dir the provider's
//!   CLI writes its sessions to (`super::home::ProviderHome::sessions_dir`:
//!   claude's `<home>/.claude/projects`, codex's `<CODEX_HOME>/sessions`);
//!   the host appends them under the provider's mirror in the box dir
//!   (`<box>/claude-projects/`, `<box>/codex-sessions/`), and on reconnect
//!   tells the guest how much it already holds (`--offsets`).
//!
//! Both reconnect with backoff while the VM runs; `Relays::stop` ends them.
//! The line handling is pure functions over a child's stdio so a scripted
//! child stands in for the guest in tests.

use std::collections::{BTreeMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use fletch_box::{guest, Channel};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Child;
use tokio::sync::watch;

/// The guest's mailbox watcher polls at 100 ms; the host side matches.
const POLL: Duration = Duration::from_millis(100);
const BACKOFF_START: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(5);

/// The JSON lines `fletch-guest relay` and `tail` exchange
/// (`crates/fletch-guest/src/protocol.rs`), spelled the same here.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Event {
    Request {
        name: String,
        body: String,
    },
    Response {
        name: String,
        body: String,
    },
    /// The receiver has durably written `name`; the sender may delete its copy.
    Ack {
        name: String,
    },
    /// The sender has re-sent everything it still held at session start; the
    /// receiver may drop receipt markers for every other name.
    Synced,
    Create {
        path: String,
    },
    Append {
        path: String,
        data: String,
    },
}

/// The relays of one running box: the owner's mailbox, the transcript tail,
/// and one more mailbox per tenant ([`Relays::add_mailbox`]).
pub struct Relays {
    stop: watch::Sender<bool>,
    tasks: parking_lot::Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// The mailboxes relayed, by rpc dir, so a tenant's relaunch adds no
    /// second relay for the same mailbox.
    mailboxes: parking_lot::Mutex<HashSet<PathBuf>>,
    /// The mirrors tailed into, so a tenant's relaunch adds no second tail
    /// for the same mirror.
    mirrors: parking_lot::Mutex<HashSet<PathBuf>>,
}

/// Where, under the box dir, the host keeps its receipt markers for the
/// owner's mailbox.
pub const DELIVERED_DIR: &str = "relay/delivered";

/// The receipt markers of a tenant's mailbox, kept apart from the owner's and
/// from every other tenant's: a session sync prunes every marker in its dir
/// that the sender did not re-send, so two mailboxes sharing one dir would
/// prune each other's.
pub fn tenant_delivered_dir(box_dir: &Path, tenant_agent_id: &str) -> PathBuf {
    box_dir
        .join("relay/tenants")
        .join(tenant_agent_id)
        .join("delivered")
}

impl Relays {
    /// `box_dir` is the box's own directory, which outlives the VM: the
    /// request receipt markers live under it.
    ///
    /// `tail` is the guest sessions dir to follow and the host mirror to
    /// append it to; `None` for a provider whose sessions the host pulls
    /// instead (`super::home::Transport`).
    pub fn start(
        channel: Arc<Channel>,
        box_dir: &Path,
        rpc_dir: &Path,
        tail: Option<(&Path, &Path)>,
    ) -> Self {
        let (stop, rx) = watch::channel(false);
        let mailbox = spawn_mailbox(
            channel.clone(),
            rpc_dir.to_path_buf(),
            box_dir.join(DELIVERED_DIR),
            rx.clone(),
        );
        let mut tasks = vec![mailbox];
        let mut mirrors = HashSet::new();
        if let Some((guest_sessions, mirror)) = tail {
            tasks.push(spawn_tail(channel, guest_sessions, mirror, rx));
            mirrors.insert(mirror.to_path_buf());
        }
        Self {
            stop,
            tasks: parking_lot::Mutex::new(tasks),
            mailboxes: parking_lot::Mutex::new(HashSet::from([rpc_dir.to_path_buf()])),
            mirrors: parking_lot::Mutex::new(mirrors),
        }
    }

    /// Tail one more sessions dir out of this box: a tenant's, into the
    /// tenant's own mirror (a tenant of the owner's provider whose sessions
    /// dir is the owner's — claude's `~/.claude/projects` — is tailed twice,
    /// once per mirror, so each agent's mirror is whole). Idempotent per
    /// mirror; ends and comes back with the box like a tenant's mailbox.
    pub fn add_tail(&self, channel: Arc<Channel>, guest_sessions: &Path, mirror: &Path) {
        if !self.mirrors.lock().insert(mirror.to_path_buf()) {
            return;
        }
        let task = spawn_tail(channel, guest_sessions, mirror, self.stop.subscribe());
        self.tasks.lock().push(task);
    }

    /// Relay one more mailbox out of this box: a tenant's — an agent that
    /// runs in this box because it adopted a tree the box holds (the local
    /// reviewer of the box's owner) — with its own rpc dir and its own
    /// receipt markers. Idempotent per rpc dir; the relay ends with the
    /// box's others ([`Self::stop`]), and a later boot of the box adds it
    /// again at the tenant's next launch.
    pub fn add_mailbox(
        &self,
        channel: Arc<Channel>,
        box_dir: &Path,
        rpc_dir: &Path,
        tenant_agent_id: &str,
    ) {
        if !self.mailboxes.lock().insert(rpc_dir.to_path_buf()) {
            return;
        }
        let task = spawn_mailbox(
            channel,
            rpc_dir.to_path_buf(),
            tenant_delivered_dir(box_dir, tenant_agent_id),
            self.stop.subscribe(),
        );
        self.tasks.lock().push(task);
    }

    /// Tell every loop to end and wait briefly for them; whatever is still
    /// running is aborted (its ssh child is `kill_on_drop`).
    pub async fn stop(&self) {
        let _ = self.stop.send(true);
        let tasks = std::mem::take(&mut *self.tasks.lock());
        for task in tasks {
            let handle = task.abort_handle();
            if tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .is_err()
            {
                handle.abort();
            }
        }
    }
}

/// One transcript tail, kept alive until `stop`: `fletch-guest tail
/// <sessions>` in the guest, appended into `mirror` on the host.
fn spawn_tail(
    channel: Arc<Channel>,
    guest_sessions: &Path,
    mirror: &Path,
    stop: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    let (guest_sessions, mirror) = (guest_sessions.to_path_buf(), mirror.to_path_buf());
    crate::host::spawn(async move {
        let root = guest_sessions.to_string_lossy().into_owned();
        run_with_reconnect(
            stop,
            || {
                // What the host already holds, so the guest resends only
                // what is new; a damaged mirror file starts over.
                let offsets = serde_json::to_string(&offsets(&guest_sessions, &mirror))
                    .unwrap_or_else(|_| "{}".into());
                channel.spawn_relay(&[guest::TAIL, &root, "--offsets", &offsets])
            },
            |child, stop| pump_tail(child, guest_sessions.clone(), mirror.clone(), stop),
        )
        .await;
    })
}

/// One mailbox relay, kept alive until `stop`: `fletch-guest relay <rpc dir>`
/// in the guest, pumped into the host's `rpc_dir` with its receipt markers
/// in `delivered`.
fn spawn_mailbox(
    channel: Arc<Channel>,
    rpc_dir: PathBuf,
    delivered: PathBuf,
    stop: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    crate::host::spawn(async move {
        let rpc = rpc_dir.to_string_lossy().into_owned();
        run_with_reconnect(
            stop,
            || channel.spawn_relay(&[guest::RELAY, &rpc]),
            |child, stop| pump_mailbox(child, rpc_dir.clone(), delivered.clone(), stop),
        )
        .await;
    })
}

/// Keep one relay alive: spawn, pump until the child ends, back off, again —
/// until `stop` says so. A spawn failure backs off like a dropped session.
async fn run_with_reconnect<S, P, Fut>(mut stop: watch::Receiver<bool>, spawn: S, pump: P)
where
    S: Fn() -> fletch_box::Result<Child>,
    P: Fn(Child, watch::Receiver<bool>) -> Fut,
    Fut: std::future::Future<Output = io::Result<()>>,
{
    let mut backoff = BACKOFF_START;
    loop {
        if *stop.borrow() {
            return;
        }
        let started = std::time::Instant::now();
        match spawn() {
            Ok(child) => {
                if let Err(e) = pump(child, stop.clone()).await {
                    tracing::info!(error = %e, "box relay ended");
                }
            }
            Err(e) => tracing::info!(error = %e, "box relay spawn failed"),
        }
        if *stop.borrow() {
            return;
        }
        // A session that held for a while earned a fresh backoff.
        if started.elapsed() > BACKOFF_MAX {
            backoff = BACKOFF_START;
        }
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = stop.changed() => return,
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// Mirror requests out of `child` into the host mailbox and responses back,
/// until the child ends or `stop` says so.
///
/// Each request line is written, marked in `delivered`, then acked; a name
/// with a marker already is acked without being written again. Each response
/// file is sent once per session and deleted when the guest acks it, so a
/// session that drops before the ack leaves the file for the next session to
/// re-send: that re-send is the first thing a session does, followed by
/// `synced`. The guest's own `synced` closes its replay, and every marker in
/// `delivered` the replay did not name is deleted then.
pub async fn pump_mailbox(
    mut child: Child,
    rpc_dir: PathBuf,
    delivered: PathBuf,
    mut stop: watch::Receiver<bool>,
) -> io::Result<()> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("relay has no stdout"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("relay has no stdin"))?;
    let mut lines = BufReader::new(stdout).lines();
    let mut tick = tokio::time::interval(POLL);
    let responses = rpc_dir.join("responses");
    std::fs::create_dir_all(&responses)?;
    // A publication the last session left half-done must not wait for a
    // replay that may never come.
    finish_staged(&rpc_dir.join("requests"), &delivered)?;
    // Responses sent on this session and awaiting the guest's ack.
    let mut in_flight: HashSet<String> = HashSet::new();
    // Request names the guest has replayed this session, until its `synced`.
    let mut replayed: Option<HashSet<String>> = Some(HashSet::new());

    // Whatever the last session left unacked goes first, then `synced`.
    for (name, body) in pending_responses(&responses)? {
        stdin
            .write_all(response_line(&name, &body).as_bytes())
            .await?;
        in_flight.insert(name);
    }
    stdin
        .write_all(event_line(&Event::Synced).as_bytes())
        .await?;
    stdin.flush().await?;

    loop {
        tokio::select! {
            line = lines.next_line() => match line? {
                Some(line) => match handle_guest_line(&rpc_dir, &delivered, &line) {
                    Ok(Reply::Ack(name)) => {
                        if let Some(replayed) = &mut replayed {
                            replayed.insert(name.clone());
                        }
                        stdin.write_all(ack_line(&name).as_bytes()).await?;
                        stdin.flush().await?;
                    }
                    Ok(Reply::Acked(name)) => {
                        // The guest has it: the host copy was only insurance.
                        let _ = std::fs::remove_file(responses.join(&name));
                        in_flight.remove(&name);
                    }
                    // Only the first one counts: markers made after it belong
                    // to requests the guest may still hold.
                    Ok(Reply::Synced) => if let Some(keep) = replayed.take() {
                        prune_markers(&delivered, &keep)?;
                    },
                    Err(e) => tracing::warn!(error = %e, "box relay: bad guest line"),
                },
                // The guest closed the stream: the session is over.
                None => return Ok(()),
            },
            _ = tick.tick() => {
                for (name, body) in pending_responses(&responses)? {
                    if in_flight.contains(&name) {
                        continue;
                    }
                    let line = response_line(&name, &body);
                    stdin.write_all(line.as_bytes()).await?;
                    stdin.flush().await?;
                    in_flight.insert(name);
                }
            }
            _ = stop.changed() => {
                let _ = child.start_kill();
                return Ok(());
            }
        }
    }
}

/// What a guest line calls for on the wire.
#[derive(Debug, PartialEq, Eq)]
pub enum Reply {
    /// Send `{"op":"ack","name"}`: the request is on the host's disk.
    Ack(String),
    /// The guest acked this response; nothing to send, delete the host copy.
    Acked(String),
    /// The guest's replay is complete; prune the markers it did not name.
    Synced,
}

/// Deletes every marker in `dir` whose name is not in `keep`. Returns how
/// many went. A missing dir is an empty one.
pub fn prune_markers(dir: &Path, keep: &HashSet<String>) -> io::Result<usize> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let mut gone = 0;
    for entry in entries.filter_map(|e| e.ok()) {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !keep.contains(&name) {
            match std::fs::remove_file(entry.path()) {
                Ok(()) => gone += 1,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
    }
    Ok(gone)
}

/// One line from the guest. A `request` is published into
/// `rpc_dir/requests/<name>` (see `deliver_request`) and acked; a replay of
/// one already published is only acked. An `ack` names a response the guest
/// has written.
pub fn handle_guest_line(rpc_dir: &Path, delivered: &Path, line: &str) -> io::Result<Reply> {
    match serde_json::from_str(line)? {
        Event::Request { name, body } => {
            if !safe_name(&name) {
                return Err(io::Error::other(format!("bad request name {name:?}")));
            }
            deliver_request(&rpc_dir.join("requests"), delivered, &name, &body)?;
            Ok(Reply::Ack(name))
        }
        Event::Ack { name } => {
            if !safe_name(&name) {
                return Err(io::Error::other(format!("bad ack name {name:?}")));
            }
            Ok(Reply::Acked(name))
        }
        Event::Synced => Ok(Reply::Synced),
        _ => Err(io::Error::other("expected a request, ack or synced line")),
    }
}

/// Bring `requests/<name>` to published, from whatever state a crash left:
/// no marker → stage, mark, rename; marker → just the rename, which is a
/// no-op when staging is gone (published already, maybe consumed by
/// `rpc_watch`). The marker thus exists before the watcher can ever see the
/// file, and a replay never publishes a second copy.
pub fn deliver_request(
    requests: &Path,
    delivered: &Path,
    name: &str,
    body: &str,
) -> io::Result<()> {
    if !delivered.join(name).exists() {
        stage_request(requests, name, body)?;
        mark_delivered(delivered, name)?;
    }
    publish_request(requests, name)
}

/// Step 1: the body, where only the relay looks.
pub fn stage_request(requests: &Path, name: &str, body: &str) -> io::Result<()> {
    std::fs::create_dir_all(requests)?;
    std::fs::write(staging_path(requests, name), body)
}

/// Step 2: the receipt, before anything is visible.
pub fn mark_delivered(delivered: &Path, name: &str) -> io::Result<()> {
    std::fs::create_dir_all(delivered)?;
    std::fs::File::create(delivered.join(name)).map(drop)
}

/// Step 3: the one atomic step `rpc_watch` sees. Nothing staged, nothing to
/// do — the earlier publication stands.
pub fn publish_request(requests: &Path, name: &str) -> io::Result<()> {
    match std::fs::rename(staging_path(requests, name), requests.join(name)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Finish what a crash interrupted: staged requests with a marker are
/// published, those without are dropped (the guest still holds them and will
/// replay).
pub fn finish_staged(requests: &Path, delivered: &Path) -> io::Result<()> {
    let entries = match std::fs::read_dir(requests) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let staged: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|f| {
            f.strip_prefix('.')
                .and_then(|f| f.strip_suffix(".staging"))
                .map(str::to_string)
        })
        .collect();
    for name in staged {
        if delivered.join(&name).exists() {
            publish_request(requests, &name)?;
        } else {
            std::fs::remove_file(staging_path(requests, &name))?;
        }
    }
    Ok(())
}

/// `dir/.<name>.staging`: next to the target, never `*.json`, so neither
/// `rpc_watch` (extension `json`) nor this relay's own listings see it.
pub fn staging_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!(".{name}.staging"))
}

pub fn ack_line(name: &str) -> String {
    event_line(&Event::Ack {
        name: name.to_string(),
    })
}

/// One event as the newline-terminated JSON line it goes out as.
pub fn event_line(event: &Event) -> String {
    let mut line = serde_json::to_string(event).expect("an event serializes");
    line.push('\n');
    line
}

/// Every answered request in `responses`, by file name, oldest name first.
fn pending_responses(responses: &Path) -> io::Result<Vec<(String, String)>> {
    let mut names: Vec<String> = match std::fs::read_dir(responses) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.ends_with(".json"))
            .collect(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    names.sort();
    let mut out = Vec::new();
    for name in names {
        match std::fs::read_to_string(responses.join(&name)) {
            Ok(body) => out.push((name, body)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}

pub fn response_line(name: &str, body: &str) -> String {
    let mut line = serde_json::to_string(&Event::Response {
        name: name.to_string(),
        body: body.to_string(),
    })
    .expect("a response serializes");
    line.push('\n');
    line
}

/// A mailbox file name: one component, no traversal.
fn safe_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\0')
}

/// Mirror transcript bytes out of `child` into `mirror` until it ends.
pub async fn pump_tail(
    mut child: Child,
    guest_root: PathBuf,
    mirror: PathBuf,
    mut stop: watch::Receiver<bool>,
) -> io::Result<()> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("tail has no stdout"))?;
    let mut lines = BufReader::new(stdout).lines();
    loop {
        tokio::select! {
            line = lines.next_line() => match line? {
                Some(line) => {
                    if let Err(e) = apply_tail_line(&guest_root, &mirror, &line) {
                        tracing::warn!(error = %e, "box relay: bad tail line");
                    }
                }
                None => return Ok(()),
            },
            _ = stop.changed() => {
                let _ = child.start_kill();
                return Ok(());
            }
        }
    }
}

/// One `create`/`append` line → the mirror file for the guest path, which
/// must lie under `guest_root` (anything else is refused, not mirrored).
pub fn apply_tail_line(guest_root: &Path, mirror: &Path, line: &str) -> io::Result<()> {
    let event: Event = serde_json::from_str(line)?;
    let (path, data) = match event {
        Event::Create { path } => (path, None),
        Event::Append { path, data } => (path, Some(data)),
        _ => return Err(io::Error::other("expected a tail line")),
    };
    let target = mirror_path(guest_root, mirror, Path::new(&path)).ok_or_else(|| {
        io::Error::other(format!("tail path {path:?} is outside the tailed root"))
    })?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match data {
        // A new (or restarted) file: whatever the mirror had is stale.
        None => std::fs::write(&target, b"").map(drop),
        Some(data) => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|e| io::Error::other(format!("bad base64: {e}")))?;
            use std::io::Write;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&target)?
                .write_all(&bytes)
        }
    }
}

/// `<mirror>/<path relative to guest_root>`, `None` when `path` is not under it
/// or climbs.
fn mirror_path(guest_root: &Path, mirror: &Path, path: &Path) -> Option<PathBuf> {
    let rel = path.strip_prefix(guest_root).ok()?;
    if rel
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return None;
    }
    Some(mirror.join(rel))
}

/// What the mirror holds, keyed by guest path: the `--offsets` the tail
/// resumes from.
pub fn offsets(guest_root: &Path, mirror: &Path) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    collect(mirror, mirror, guest_root, &mut out);
    out
}

fn collect(dir: &Path, mirror: &Path, guest_root: &Path, out: &mut BTreeMap<String, u64>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, mirror, guest_root, out);
        } else if path.extension().is_some_and(|e| e == "jsonl") {
            if let (Ok(rel), Ok(meta)) = (path.strip_prefix(mirror), path.metadata()) {
                out.insert(
                    guest_root.join(rel).to_string_lossy().into_owned(),
                    meta.len(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    /// A `sh` child with piped stdio standing in for the guest session.
    fn scripted(script: &str) -> Child {
        tokio::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap()
    }

    fn request_line(name: &str, body: &str) -> String {
        serde_json::to_string(&Event::Request {
            name: name.into(),
            body: body.into(),
        })
        .unwrap()
    }

    async fn wait_for(path: &Path) {
        for _ in 0..250 {
            if path.exists() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("{} never appeared", path.display());
    }

    async fn finished(pump: tokio::task::JoinHandle<io::Result<()>>) {
        tokio::time::timeout(Duration::from_secs(5), pump)
            .await
            .expect("the pump ends with the child")
            .unwrap()
            .unwrap();
    }

    /// The marker dir of a box whose dir is `<tmp>/box`.
    fn delivered(dir: &Path) -> PathBuf {
        dir.join("box").join(DELIVERED_DIR)
    }

    /// A request line lands in the host mailbox atomically, leaves its
    /// receipt marker and is acked; a response the host writes goes back as
    /// a line and the host copy goes once the guest acks it.
    #[tokio::test]
    async fn mailbox_acks_a_request_and_deletes_an_acked_response() {
        let dir = tempfile::tempdir().unwrap();
        let rpc = dir.path().join("rpc");
        let got_ack = dir.path().join("got_ack");
        let got_response = dir.path().join("got_response");
        // On stdin: the host's `synced` (nothing pending), the ack, then the
        // response.
        let script = format!(
            "printf '%s\\n' '{}'; IFS= read -r line; \
             IFS= read -r line; printf '%s' \"$line\" > '{}'; \
             IFS= read -r line; printf '%s' \"$line\" > '{}'; printf '%s\\n' '{}'",
            request_line("r1.json", r#"{"id":"r1","op":"noop"}"#),
            got_ack.display(),
            got_response.display(),
            ack_line("r1.json").trim(),
        );
        let (_stop, rx) = watch::channel(false);
        let pump = tokio::spawn(pump_mailbox(
            scripted(&script),
            rpc.clone(),
            delivered(dir.path()),
            rx,
        ));

        let request_file = rpc.join("requests/r1.json");
        wait_for(&request_file).await;
        assert_eq!(
            std::fs::read_to_string(&request_file).unwrap(),
            r#"{"id":"r1","op":"noop"}"#
        );
        assert!(!staging_path(&rpc.join("requests"), "r1.json").exists());
        wait_for(&got_ack).await;
        assert!(delivered(dir.path()).join("r1.json").exists());
        assert_eq!(
            std::fs::read_to_string(&got_ack).unwrap().trim(),
            ack_line("r1.json").trim()
        );

        // What `rpc_watch` does once it answered.
        std::fs::write(rpc.join("responses/r1.json"), r#"{"id":"r1","ok":true}"#).unwrap();
        finished(pump).await;
        assert_eq!(
            std::fs::read_to_string(&got_response).unwrap().trim(),
            response_line("r1.json", r#"{"id":"r1","ok":true}"#).trim()
        );
        assert!(!rpc.join("responses/r1.json").exists());
    }

    /// The guest's session drops before it sees the host's ack, and by the
    /// time it replays the request the host has restarted (an idle stop, a
    /// reboot, the app): a fresh pump with no in-memory state over the same
    /// box dir acks again but does not write the file a second time, since
    /// `rpc_watch` already consumed and ran the first one.
    #[tokio::test]
    async fn a_replayed_request_is_acked_but_not_rewritten_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let rpc = dir.path().join("rpc");
        let request = request_line("r1.json", r#"{"id":"r1","op":"noop"}"#);
        let (_stop, rx) = watch::channel(false);

        // Session one: the guest emits, sees the host's `synced`, and is gone
        // before the ack reaches it.
        let script = format!("printf '%s\\n' '{request}'; IFS= read -r line");
        let pump = tokio::spawn(pump_mailbox(
            scripted(&script),
            rpc.clone(),
            delivered(dir.path()),
            rx.clone(),
        ));
        // The ack may hit a closed pipe; either way the session is over.
        let _ = tokio::time::timeout(Duration::from_secs(5), pump)
            .await
            .expect("the pump ends with the child");
        let request_file = rpc.join("requests/r1.json");
        assert!(request_file.exists());
        assert!(delivered(dir.path()).join("r1.json").exists());
        // `rpc_watch` takes it.
        std::fs::remove_file(&request_file).unwrap();

        // Session two, a new `Relays` life: the same request again. On stdin
        // `synced` first, then the ack.
        let got = dir.path().join("got");
        let script = format!(
            "printf '%s\\n' '{request}'; IFS= read -r line; \
             IFS= read -r line; printf '%s' \"$line\" > '{}'",
            got.display()
        );
        finished(tokio::spawn(pump_mailbox(
            scripted(&script),
            rpc.clone(),
            delivered(dir.path()),
            rx,
        )))
        .await;
        assert_eq!(
            std::fs::read_to_string(&got).unwrap().trim(),
            ack_line("r1.json").trim()
        );
        assert!(
            !request_file.exists(),
            "a consumed request must not reappear"
        );
    }

    /// The session drops after the response went out but before the guest
    /// acked it: the host copy stays, and the next session re-sends it.
    #[tokio::test]
    async fn a_response_unacked_at_disconnect_stays_for_the_next_session() {
        let dir = tempfile::tempdir().unwrap();
        let rpc = dir.path().join("rpc");
        let response_file = rpc.join("responses/r1.json");
        std::fs::create_dir_all(rpc.join("responses")).unwrap();
        std::fs::write(&response_file, r#"{"id":"r1","ok":true}"#).unwrap();

        // Session one reads the response and hangs without acking.
        let got = dir.path().join("got");
        let script = format!(
            "IFS= read -r line; printf '%s' \"$line\" > '{}'; exec sleep 30",
            got.display()
        );
        let (stop, rx) = watch::channel(false);
        let pump = tokio::spawn(pump_mailbox(
            scripted(&script),
            rpc.clone(),
            delivered(dir.path()),
            rx,
        ));
        wait_for(&got).await;
        assert_eq!(
            std::fs::read_to_string(&got).unwrap().trim(),
            response_line("r1.json", r#"{"id":"r1","ok":true}"#).trim()
        );
        stop.send(true).unwrap();
        finished(pump).await;
        assert!(response_file.exists(), "unacked response must survive");

        // Session two gets it again and acks; now it goes.
        let got_again = dir.path().join("got_again");
        let script = format!(
            "IFS= read -r line; printf '%s' \"$line\" > '{}'; printf '%s\\n' '{}'; \
             while [ -e '{}' ]; do sleep 0.02; done",
            got_again.display(),
            ack_line("r1.json").trim(),
            response_file.display(),
        );
        let (_stop, rx) = watch::channel(false);
        finished(tokio::spawn(pump_mailbox(
            scripted(&script),
            rpc.clone(),
            delivered(dir.path()),
            rx,
        )))
        .await;
        assert_eq!(
            std::fs::read_to_string(&got_again).unwrap().trim(),
            response_line("r1.json", r#"{"id":"r1","ok":true}"#).trim()
        );
        assert!(!response_file.exists());
    }

    /// Session one delivered A and B (and the guest, acked, deleted both).
    /// Session two starts with the guest still holding A — its ack was lost
    /// — so it replays A and says `synced`: B's marker goes, A's stays and a
    /// later replay of A is still acked without writing. A marker made after
    /// the sync survives a stray second `synced`.
    #[tokio::test]
    async fn a_session_sync_prunes_markers_for_requests_the_guest_no_longer_holds() {
        let dir = tempfile::tempdir().unwrap();
        let rpc = dir.path().join("rpc");
        let markers = delivered(dir.path());
        let a = request_line("a.json", "A");
        let b = request_line("b.json", "B");
        let (_stop, rx) = watch::channel(false);

        // Session one: A and B, replayed-then-synced, acked. On stdin the
        // host's `synced` and two acks.
        let script = format!(
            "printf '%s\\n' '{a}' '{b}' '{}'; IFS= read -r l; IFS= read -r l; IFS= read -r l",
            event_line(&Event::Synced).trim()
        );
        finished(tokio::spawn(pump_mailbox(
            scripted(&script),
            rpc.clone(),
            markers.clone(),
            rx.clone(),
        )))
        .await;
        assert!(markers.join("a.json").exists());
        assert!(markers.join("b.json").exists());
        // `rpc_watch` takes both.
        std::fs::remove_file(rpc.join("requests/a.json")).unwrap();
        std::fs::remove_file(rpc.join("requests/b.json")).unwrap();

        // Session two: only A comes back before `synced`.
        let script = format!(
            "printf '%s\\n' '{a}' '{}'; IFS= read -r l; IFS= read -r l",
            event_line(&Event::Synced).trim()
        );
        finished(tokio::spawn(pump_mailbox(
            scripted(&script),
            rpc.clone(),
            markers.clone(),
            rx,
        )))
        .await;
        assert!(markers.join("a.json").exists(), "A may still be replayed");
        assert!(!markers.join("b.json").exists(), "nothing can replay B");
        assert!(!rpc.join("requests/a.json").exists());
        assert!(!rpc.join("requests/b.json").exists());

        // A again, still deduped.
        assert_eq!(
            handle_guest_line(&rpc, &markers, &a).unwrap(),
            Reply::Ack("a.json".into())
        );
        assert!(!rpc.join("requests/a.json").exists());

        // After the sync a new request's marker is not touched by a second
        // `synced` in the same session.
        let keep = HashSet::new();
        assert_eq!(
            handle_guest_line(&rpc, &markers, &request_line("c.json", "C")).unwrap(),
            Reply::Ack("c.json".into())
        );
        assert!(markers.join("c.json").exists());
        // What `pump_mailbox` does on the first `synced` only; a pump that
        // already synced leaves `replayed` as `None` and never calls this.
        assert_eq!(prune_markers(&markers, &keep).unwrap(), 2);
        assert!(!markers.exists() || std::fs::read_dir(&markers).unwrap().next().is_none());
    }

    /// Fault injection: the app dies after each publication step and the
    /// next session (a guest replay, or the start-up sweep when none comes)
    /// ends with the request visible to `rpc_watch` exactly once, and never
    /// again after the watcher consumed it.
    #[test]
    fn a_crash_at_any_publication_step_recovers_to_one_publication() {
        let dir = tempfile::tempdir().unwrap();
        let requests = dir.path().join("rpc/requests");
        let markers = delivered(dir.path());
        let published = |name: &str| requests.join(name).exists();
        let staged = |name: &str| staging_path(&requests, name).exists();
        // What `rpc_watch` lists: `*.json` only, so a staged file is unseen.
        let visible_to_watcher = |name: &str| {
            staging_path(&requests, name)
                .extension()
                .is_some_and(|e| e == "json")
        };
        assert!(!visible_to_watcher("a.json"));

        // Crash after step 1 (staged, no marker): the replay publishes once.
        stage_request(&requests, "a.json", "A").unwrap();
        assert!(staged("a.json") && !published("a.json"));
        deliver_request(&requests, &markers, "a.json", "A").unwrap();
        assert_eq!(
            std::fs::read_to_string(requests.join("a.json")).unwrap(),
            "A"
        );
        assert!(!staged("a.json") && markers.join("a.json").exists());
        // `rpc_watch` ran it; the replay must not bring it back.
        std::fs::remove_file(requests.join("a.json")).unwrap();
        deliver_request(&requests, &markers, "a.json", "A").unwrap();
        assert!(!published("a.json"), "consumed A must not come back");

        // Crash after step 2 (staged and marked, not visible): the replay
        // finishes the rename.
        stage_request(&requests, "b.json", "B").unwrap();
        mark_delivered(&markers, "b.json").unwrap();
        assert!(staged("b.json") && !published("b.json"));
        deliver_request(&requests, &markers, "b.json", "B").unwrap();
        assert_eq!(
            std::fs::read_to_string(requests.join("b.json")).unwrap(),
            "B"
        );
        assert!(!staged("b.json"));
        std::fs::remove_file(requests.join("b.json")).unwrap();
        deliver_request(&requests, &markers, "b.json", "B").unwrap();
        assert!(!published("b.json"));

        // Same crash, no replay: the session-start sweep publishes it. A
        // staged file without a marker (crash after step 1) is dropped —
        // the guest still holds it.
        stage_request(&requests, "c.json", "C").unwrap();
        mark_delivered(&markers, "c.json").unwrap();
        stage_request(&requests, "d.json", "D").unwrap();
        finish_staged(&requests, &markers).unwrap();
        assert_eq!(
            std::fs::read_to_string(requests.join("c.json")).unwrap(),
            "C"
        );
        assert!(!staged("c.json"));
        assert!(!staged("d.json") && !published("d.json"));
        assert!(!markers.join("d.json").exists());
        std::fs::remove_file(requests.join("c.json")).unwrap();
        deliver_request(&requests, &markers, "c.json", "C").unwrap();
        assert!(!published("c.json"));

        // Crash after step 3 (published, ack never sent): the replay only
        // acks, and nothing reappears once consumed.
        stage_request(&requests, "e.json", "E").unwrap();
        mark_delivered(&markers, "e.json").unwrap();
        publish_request(&requests, "e.json").unwrap();
        std::fs::remove_file(requests.join("e.json")).unwrap();
        let rpc = dir.path().join("rpc");
        assert_eq!(
            handle_guest_line(&rpc, &markers, &request_line("e.json", "E")).unwrap(),
            Reply::Ack("e.json".into())
        );
        assert!(!published("e.json") && !staged("e.json"));
    }

    /// The sweep runs when a session opens, so a publication interrupted
    /// after its marker becomes visible without waiting on the guest.
    #[tokio::test]
    async fn a_session_start_finishes_a_staged_and_marked_request() {
        let dir = tempfile::tempdir().unwrap();
        let rpc = dir.path().join("rpc");
        let requests = rpc.join("requests");
        let markers = delivered(dir.path());
        stage_request(&requests, "r1.json", "R").unwrap();
        mark_delivered(&markers, "r1.json").unwrap();

        // A guest that says nothing and leaves once it has read `synced`.
        let (_stop, rx) = watch::channel(false);
        finished(tokio::spawn(pump_mailbox(
            scripted("IFS= read -r line"),
            rpc.clone(),
            markers,
            rx,
        )))
        .await;
        assert_eq!(
            std::fs::read_to_string(requests.join("r1.json")).unwrap(),
            "R"
        );
        assert!(!staging_path(&requests, "r1.json").exists());
    }

    #[test]
    fn guest_lines_are_validated() {
        let dir = tempfile::tempdir().unwrap();
        let rpc = dir.path().join("rpc");
        let delivered = delivered(dir.path());
        assert_eq!(
            handle_guest_line(&rpc, &delivered, r#"{"op":"synced"}"#).unwrap(),
            Reply::Synced
        );
        let escape = r#"{"op":"request","name":"../x","body":""}"#;
        assert!(handle_guest_line(&rpc, &delivered, escape).is_err());
        let escape_ack = r#"{"op":"ack","name":"../x"}"#;
        assert!(handle_guest_line(&rpc, &delivered, escape_ack).is_err());
        let wrong_op = r#"{"op":"response","name":"a.json","body":""}"#;
        assert!(handle_guest_line(&rpc, &delivered, wrong_op).is_err());
        assert!(handle_guest_line(&rpc, &delivered, "nonsense").is_err());
        assert!(!rpc.join("requests").join("x").exists());
        assert!(!dir.path().join("x").exists());
        assert!(!delivered.exists());

        let ack = r#"{"op":"ack","name":"a.json"}"#;
        assert_eq!(
            handle_guest_line(&rpc, &delivered, ack).unwrap(),
            Reply::Acked("a.json".into())
        );
    }

    /// `create` starts a mirror file, `append` grows it, and the offsets the
    /// next connection reports are the mirror's sizes under guest paths.
    #[tokio::test]
    async fn tail_lines_build_the_mirror_and_its_offsets() {
        let dir = tempfile::tempdir().unwrap();
        let mirror = dir.path().join("claude-projects");
        let root = PathBuf::from("/Users/u/.claude/projects");
        let file = "/Users/u/.claude/projects/-Users-u-ws/s1.jsonl";
        let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
        let lines = [
            serde_json::to_string(&Event::Create { path: file.into() }).unwrap(),
            serde_json::to_string(&Event::Append {
                path: file.into(),
                data: b64("one\n"),
            })
            .unwrap(),
            serde_json::to_string(&Event::Append {
                path: file.into(),
                data: b64("two\n"),
            })
            .unwrap(),
            // Outside the root: refused, nothing written.
            serde_json::to_string(&Event::Create {
                path: "/etc/passwd.jsonl".into(),
            })
            .unwrap(),
        ];
        let script = lines
            .iter()
            .map(|l| format!("printf '%s\\n' '{l}'"))
            .collect::<Vec<_>>()
            .join("; ");
        let (_stop, rx) = watch::channel(false);
        pump_tail(scripted(&script), root.clone(), mirror.clone(), rx)
            .await
            .unwrap();

        let mirrored = mirror.join("-Users-u-ws/s1.jsonl");
        assert_eq!(std::fs::read_to_string(&mirrored).unwrap(), "one\ntwo\n");
        assert_eq!(
            offsets(&root, &mirror),
            BTreeMap::from([(file.to_string(), 8u64)])
        );
        assert!(!dir.path().join("etc").exists());

        // A `create` for a known file starts it over (the guest saw it shrink).
        apply_tail_line(
            &root,
            &mirror,
            &serde_json::to_string(&Event::Create { path: file.into() }).unwrap(),
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(&mirrored).unwrap(), "");
    }
}
