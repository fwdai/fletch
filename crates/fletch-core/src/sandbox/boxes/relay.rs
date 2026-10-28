//! The two long-lived `fletch-guest` sessions per running box
//! (docs/boxes-spec.md §7.3), each a plain ssh child the registry supervises:
//!
//! - **Mailbox**: `fletch-guest relay <rpc dir>` emits every request file the
//!   agent writes in the guest as one JSON line; the host writes it into its
//!   own `rpc_dir/requests/` (atomically, so `supervisor::rpc_watch` picks it
//!   up unchanged) and then acks it, which is what lets the guest delete its
//!   copy. The host watches `rpc_dir/responses/` and sends each response back
//!   as a line, deleting the host copy only on the guest's ack. Either side
//!   re-sends what is still on its disk when a session reconnects, so the
//!   receiver dedupes by name: the guest keeps the responses it has written,
//!   the host keeps the request names it has delivered for the box's life
//!   (`rpc_watch` consumes the file, so a replayed request would otherwise run
//!   its op twice). Names are UUIDs chosen by the agent.
//! - **Transcript tail**: `fletch-guest tail <home>/.claude/projects` emits
//!   `create`/`append` lines for every `*.jsonl` under claude's projects dir;
//!   the host appends them under `<box>/claude-projects/`, and on reconnect
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
    Create {
        path: String,
    },
    Append {
        path: String,
        data: String,
    },
}

/// The relays of one running box.
pub struct Relays {
    stop: watch::Sender<bool>,
    tasks: parking_lot::Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl Relays {
    pub fn start(
        channel: Arc<Channel>,
        rpc_dir: &Path,
        guest_projects: &Path,
        mirror: &Path,
    ) -> Self {
        let (stop, rx) = watch::channel(false);
        let mailbox = {
            let (channel, rpc_dir, rx) = (channel.clone(), rpc_dir.to_path_buf(), rx.clone());
            // Outlives every session: a request the guest replays after a
            // reconnect must be recognised, or its op runs twice.
            let delivered = Delivered::default();
            crate::host::spawn(async move {
                let rpc = rpc_dir.to_string_lossy().into_owned();
                run_with_reconnect(
                    rx,
                    || channel.spawn_relay(&[guest::RELAY, &rpc]),
                    |child, stop| pump_mailbox(child, rpc_dir.clone(), delivered.clone(), stop),
                )
                .await;
            })
        };
        let tail = {
            let (guest_projects, mirror) = (guest_projects.to_path_buf(), mirror.to_path_buf());
            crate::host::spawn(async move {
                let root = guest_projects.to_string_lossy().into_owned();
                run_with_reconnect(
                    rx,
                    || {
                        // What the host already holds, so the guest resends
                        // only what is new; a damaged mirror file starts over.
                        let offsets = serde_json::to_string(&offsets(&guest_projects, &mirror))
                            .unwrap_or_else(|_| "{}".into());
                        channel.spawn_relay(&[guest::TAIL, &root, "--offsets", &offsets])
                    },
                    |child, stop| pump_tail(child, guest_projects.clone(), mirror.clone(), stop),
                )
                .await;
            })
        };
        Self {
            stop,
            tasks: parking_lot::Mutex::new(vec![mailbox, tail]),
        }
    }

    /// Tell both loops to end and wait briefly for them; whatever is still
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

/// The request names one box's host side has written into its mailbox, kept
/// for the box's life and shared by every mailbox session of that box.
pub type Delivered = Arc<parking_lot::Mutex<HashSet<String>>>;

/// Mirror requests out of `child` into the host mailbox and responses back,
/// until the child ends or `stop` says so.
///
/// Each request line is written, then acked; a name already in `delivered` is
/// acked without being written again. Each response file is sent once per
/// session and deleted when the guest acks it, so a session that drops before
/// the ack leaves the file for the next session to re-send.
pub async fn pump_mailbox(
    mut child: Child,
    rpc_dir: PathBuf,
    delivered: Delivered,
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
    // Responses sent on this session and awaiting the guest's ack.
    let mut in_flight: HashSet<String> = HashSet::new();
    loop {
        tokio::select! {
            line = lines.next_line() => match line? {
                Some(line) => match handle_guest_line(&rpc_dir, &delivered, &line) {
                    Ok(Reply::Ack(name)) => {
                        stdin.write_all(ack_line(&name).as_bytes()).await?;
                        stdin.flush().await?;
                    }
                    Ok(Reply::Acked(name)) => {
                        // The guest has it: the host copy was only insurance.
                        let _ = std::fs::remove_file(responses.join(&name));
                        in_flight.remove(&name);
                    }
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
}

/// One line from the guest. A `request` goes to `rpc_dir/requests/<name>`
/// through a temp file and a rename so the watcher never reads half of it —
/// unless `delivered` already has the name (a replay), in which case it is
/// only acked. An `ack` names a response the guest has written.
pub fn handle_guest_line(rpc_dir: &Path, delivered: &Delivered, line: &str) -> io::Result<Reply> {
    match serde_json::from_str(line)? {
        Event::Request { name, body } => {
            if !safe_name(&name) {
                return Err(io::Error::other(format!("bad request name {name:?}")));
            }
            if delivered.lock().contains(&name) {
                return Ok(Reply::Ack(name));
            }
            let requests = rpc_dir.join("requests");
            std::fs::create_dir_all(&requests)?;
            let tmp = requests.join(format!(".{name}.tmp"));
            std::fs::write(&tmp, body)?;
            std::fs::rename(&tmp, requests.join(&name))?;
            delivered.lock().insert(name.clone());
            Ok(Reply::Ack(name))
        }
        Event::Ack { name } => {
            if !safe_name(&name) {
                return Err(io::Error::other(format!("bad ack name {name:?}")));
            }
            Ok(Reply::Acked(name))
        }
        _ => Err(io::Error::other("expected a request or ack line")),
    }
}

pub fn ack_line(name: &str) -> String {
    let mut line = serde_json::to_string(&Event::Ack {
        name: name.to_string(),
    })
    .expect("an ack serializes");
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

    /// A request line lands in the host mailbox atomically and is acked; a
    /// response the host writes goes back as a line and the host copy goes
    /// once the guest acks it.
    #[tokio::test]
    async fn mailbox_acks_a_request_and_deletes_an_acked_response() {
        let dir = tempfile::tempdir().unwrap();
        let rpc = dir.path().join("rpc");
        let got_ack = dir.path().join("got_ack");
        let got_response = dir.path().join("got_response");
        let script = format!(
            "printf '%s\\n' '{}'; IFS= read -r line; printf '%s' \"$line\" > '{}'; \
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
            Delivered::default(),
            rx,
        ));

        let request_file = rpc.join("requests/r1.json");
        wait_for(&request_file).await;
        assert_eq!(
            std::fs::read_to_string(&request_file).unwrap(),
            r#"{"id":"r1","op":"noop"}"#
        );
        assert!(!rpc.join("requests/.r1.json.tmp").exists());
        wait_for(&got_ack).await;
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

    /// The guest replays a request after a reconnect (its ack was lost): the
    /// host acks again but does not write the file a second time, since
    /// `rpc_watch` already consumed and ran the first one.
    #[tokio::test]
    async fn a_replayed_request_is_acked_but_not_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let rpc = dir.path().join("rpc");
        let delivered = Delivered::default();
        let request = request_line("r1.json", r#"{"id":"r1","op":"noop"}"#);
        let (_stop, rx) = watch::channel(false);

        // Session one: delivered and acked, then the session ends.
        let script = format!("printf '%s\\n' '{request}'; IFS= read -r line");
        finished(tokio::spawn(pump_mailbox(
            scripted(&script),
            rpc.clone(),
            delivered.clone(),
            rx.clone(),
        )))
        .await;
        let request_file = rpc.join("requests/r1.json");
        assert!(request_file.exists());
        // `rpc_watch` takes it.
        std::fs::remove_file(&request_file).unwrap();

        // Session two: the same request again.
        let got = dir.path().join("got");
        let script = format!(
            "printf '%s\\n' '{request}'; IFS= read -r line; printf '%s' \"$line\" > '{}'",
            got.display()
        );
        finished(tokio::spawn(pump_mailbox(
            scripted(&script),
            rpc.clone(),
            delivered,
            rx,
        )))
        .await;
        assert_eq!(
            std::fs::read_to_string(&got).unwrap().trim(),
            ack_line("r1.json").trim()
        );
        assert!(!request_file.exists());
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
            Delivered::default(),
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
            Delivered::default(),
            rx,
        )))
        .await;
        assert_eq!(
            std::fs::read_to_string(&got_again).unwrap().trim(),
            response_line("r1.json", r#"{"id":"r1","ok":true}"#).trim()
        );
        assert!(!response_file.exists());
    }

    #[test]
    fn guest_lines_are_validated() {
        let dir = tempfile::tempdir().unwrap();
        let delivered = Delivered::default();
        let escape = r#"{"op":"request","name":"../x","body":""}"#;
        assert!(handle_guest_line(dir.path(), &delivered, escape).is_err());
        let escape_ack = r#"{"op":"ack","name":"../x"}"#;
        assert!(handle_guest_line(dir.path(), &delivered, escape_ack).is_err());
        let wrong_op = r#"{"op":"response","name":"a.json","body":""}"#;
        assert!(handle_guest_line(dir.path(), &delivered, wrong_op).is_err());
        assert!(handle_guest_line(dir.path(), &delivered, "nonsense").is_err());
        assert!(!dir.path().join("requests").join("x").exists());
        assert!(delivered.lock().is_empty());

        let ack = r#"{"op":"ack","name":"a.json"}"#;
        assert_eq!(
            handle_guest_line(dir.path(), &delivered, ack).unwrap(),
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
