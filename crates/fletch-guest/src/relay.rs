//! `relay <mailbox dir>`: the file mailbox over a byte stream (spec §7.3).
//!
//! Requests appear as `<dir>/requests/*.json`; each goes out as one JSON line
//! and the file *stays* until the host answers `{"op":"ack","name"}`, which
//! means it has the request on its own disk. Only then is the file removed. A
//! flushed pipe proves nothing about the other end, so a session that drops
//! between the emit and the host's write leaves the file where the next
//! session (a fresh process, so an empty `emitted` set) re-emits it. Within one
//! process a name is emitted once; the set remembers what is awaiting an ack.
//!
//! Responses come back as JSON lines and land in `<dir>/responses/<name>`
//! through a temp file and rename, so the agent's own watcher never sees a
//! half-written response. Before acking, the guest drops a receipt marker
//! `/data/run/relay/delivered/<name>` (an empty file on the data disk, which
//! outlives relay processes and reboots alike); a response the host re-sends
//! because its ack was lost finds the marker and is acked without being
//! rewritten, so an answer the agent already consumed cannot come back.
//!
//! Markers are pruned by a session sync. A marker is only needed while the
//! host may still hold the file, and the host re-sends everything it still
//! holds at the start of each session, then says `{"op":"synced"}`; on that
//! the guest deletes every marker it was not shown this session. The guest
//! does the same for the host: after its first sweep of `requests/` it emits
//! `synced` so the host can prune its request markers. Markers made after the
//! sync stay until the next session's. Polling at 100 ms matches the host's
//! mailbox watcher; stdin EOF means the host is gone and the relay exits.

use std::collections::HashSet;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::protocol::{write_line, Event};
use crate::Result;

const POLL: Duration = Duration::from_millis(100);

/// Receipt markers for responses already written, one empty file per name.
/// Under `/data/run` (agent-owned, 0700) so it survives with the data disk.
pub const DELIVERED_DIR: &str = "/data/run/relay/delivered";

pub fn run(args: &[String]) -> Result<()> {
    let dir = PathBuf::from(args.first().ok_or("relay: missing <mailbox dir>")?);
    let mailbox = Arc::new(Mutex::new(Mailbox::new(&dir, Path::new(DELIVERED_DIR))));
    let reader = mailbox.clone();
    thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            let mut out = io::stdout().lock();
            if let Err(err) = lock(&reader).handle_host_line(&line, &mut out) {
                eprintln!("fletch-guest: relay: {err}");
            }
        }
        std::process::exit(0);
    });

    // The replay of whatever the last session left behind, then `synced`.
    {
        let mut out = io::stdout().lock();
        lock(&mailbox).emit_requests(&mut out)?;
        write_line(&mut out, &Event::Synced)?;
    }
    loop {
        thread::sleep(POLL);
        let mut out = io::stdout().lock();
        lock(&mailbox).emit_requests(&mut out)?;
    }
}

fn lock(mailbox: &Mutex<Mailbox>) -> std::sync::MutexGuard<'_, Mailbox> {
    mailbox.lock().unwrap_or_else(|e| e.into_inner())
}

/// The guest half of the mailbox: what has gone out and what has come in.
pub struct Mailbox {
    requests: PathBuf,
    responses: PathBuf,
    /// Receipt markers for responses written, see [`DELIVERED_DIR`].
    delivered: PathBuf,
    /// Request names emitted on this session and not yet acked by the host.
    /// Per process on purpose: a new session must re-emit what is still here.
    emitted: HashSet<String>,
    /// Response names the host has sent this session, until its `synced`;
    /// `None` afterwards. Markers for every other name go on `synced`.
    replayed: Option<HashSet<String>>,
}

impl Mailbox {
    pub fn new(dir: &Path, delivered: &Path) -> Self {
        Self {
            requests: dir.join("requests"),
            responses: dir.join("responses"),
            delivered: delivered.to_path_buf(),
            emitted: HashSet::new(),
            replayed: Some(HashSet::new()),
        }
    }

    /// Emits every `*.json` in `requests` (oldest name first) that this
    /// session has not emitted yet. Nothing is deleted here; see `handle_host_line`.
    pub fn emit_requests(&mut self, out: &mut impl Write) -> io::Result<usize> {
        let entries = match fs::read_dir(&self.requests) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(err) => return Err(err),
        };
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.ends_with(".json") && !self.emitted.contains(n))
            .collect();
        names.sort();

        let mut sent = 0;
        for name in names {
            let body = match fs::read(self.requests.join(&name)) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                // Taken by someone else between listing and reading.
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => return Err(err),
            };
            write_line(
                out,
                &Event::Request {
                    name: name.clone(),
                    body,
                },
            )?;
            self.emitted.insert(name);
            sent += 1;
        }
        Ok(sent)
    }

    /// One line from the host: a `response` is written atomically into
    /// `responses` unless its receipt marker exists, the marker is created,
    /// and it is acked; an `ack` deletes the request it names; the session's
    /// first `synced` prunes the markers the host did not re-send.
    pub fn handle_host_line(&mut self, line: &str, out: &mut impl Write) -> Result<()> {
        match serde_json::from_str(line)? {
            Event::Response { name, body } => {
                check_name(&name)?;
                let marker = self.delivered.join(&name);
                if !marker.exists() {
                    fs::create_dir_all(&self.responses)?;
                    let tmp = self.responses.join(format!(".{name}.tmp"));
                    fs::write(&tmp, body)?;
                    fs::rename(&tmp, self.responses.join(&name))?;
                    fs::create_dir_all(&self.delivered)?;
                    fs::File::create(&marker)?;
                }
                if let Some(replayed) = &mut self.replayed {
                    replayed.insert(name.clone());
                }
                write_line(out, &Event::Ack { name })?;
                Ok(())
            }
            Event::Ack { name } => {
                check_name(&name)?;
                match fs::remove_file(self.requests.join(&name)) {
                    Ok(()) => {}
                    Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                    Err(err) => return Err(err.into()),
                }
                self.emitted.remove(&name);
                Ok(())
            }
            Event::Synced => {
                // Only the first one counts: markers made after it belong to
                // responses the host may still hold.
                if let Some(keep) = self.replayed.take() {
                    prune_markers(&self.delivered, &keep)?;
                }
                Ok(())
            }
            _ => Err("expected a response, ack or synced line".into()),
        }
    }
}

/// Deletes every marker in `dir` whose name is not in `keep`. Returns how
/// many went. A missing dir is an empty one.
pub fn prune_markers(dir: &Path, keep: &HashSet<String>) -> io::Result<usize> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err),
    };
    let mut gone = 0;
    for entry in entries.filter_map(|e| e.ok()) {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !keep.contains(&name) {
            match fs::remove_file(entry.path()) {
                Ok(()) => gone += 1,
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
        }
    }
    Ok(gone)
}

/// A mailbox file name: one component, no traversal.
fn check_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        return Err(format!("bad mailbox name {name:?}").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(out: &[u8]) -> Vec<Event> {
        String::from_utf8(out.to_vec())
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn ack(name: &str) -> String {
        serde_json::to_string(&Event::Ack { name: name.into() }).unwrap()
    }

    fn response(name: &str, body: &str) -> String {
        serde_json::to_string(&Event::Response {
            name: name.into(),
            body: body.into(),
        })
        .unwrap()
    }

    fn synced() -> String {
        serde_json::to_string(&Event::Synced).unwrap()
    }

    /// A mailbox over `<tmp>/rpc` with markers in `<tmp>/delivered`.
    fn mailbox(dir: &Path) -> Mailbox {
        Mailbox::new(&dir.join("rpc"), &dir.join("delivered"))
    }

    /// A request goes out as a line and stays on disk until the host acks it.
    #[test]
    fn requests_go_out_once_and_survive_until_acked() {
        let dir = tempfile::tempdir().unwrap();
        let requests = dir.path().join("rpc/requests");
        fs::create_dir_all(&requests).unwrap();
        fs::write(requests.join("b.json"), r#"{"n":2}"#).unwrap();
        fs::write(requests.join("a.json"), r#"{"n":1}"#).unwrap();
        fs::write(requests.join("notes.txt"), "ignored").unwrap();

        let mut mailbox = mailbox(dir.path());
        let mut out = Vec::new();
        assert_eq!(mailbox.emit_requests(&mut out).unwrap(), 2);
        assert_eq!(
            events(&out),
            vec![
                Event::Request {
                    name: "a.json".into(),
                    body: r#"{"n":1}"#.into()
                },
                Event::Request {
                    name: "b.json".into(),
                    body: r#"{"n":2}"#.into()
                },
            ]
        );
        // Still there: the host has not said it has them.
        assert!(requests.join("a.json").exists());
        assert!(requests.join("b.json").exists());

        // And not emitted twice while waiting.
        let mut again = Vec::new();
        assert_eq!(mailbox.emit_requests(&mut again).unwrap(), 0);
        assert!(again.is_empty());

        // The ack is what deletes.
        let mut acked = Vec::new();
        mailbox
            .handle_host_line(&ack("a.json"), &mut acked)
            .unwrap();
        assert!(acked.is_empty());
        assert!(!requests.join("a.json").exists());
        assert!(requests.join("b.json").exists());
        assert!(requests.join("notes.txt").exists());
        // An ack for something already gone is not an error.
        mailbox
            .handle_host_line(&ack("a.json"), &mut acked)
            .unwrap();
    }

    /// A new process (the reconnect) re-emits every unacked file, once.
    #[test]
    fn a_restart_re_emits_unacked_requests_once() {
        let dir = tempfile::tempdir().unwrap();
        let requests = dir.path().join("rpc/requests");
        fs::create_dir_all(&requests).unwrap();
        fs::write(requests.join("a.json"), "1").unwrap();
        fs::write(requests.join("b.json"), "2").unwrap();

        let mut first = mailbox(dir.path());
        let mut out = Vec::new();
        assert_eq!(first.emit_requests(&mut out).unwrap(), 2);
        first.handle_host_line(&ack("a.json"), &mut out).unwrap();
        drop(first);

        let mut second = mailbox(dir.path());
        let mut replay = Vec::new();
        assert_eq!(second.emit_requests(&mut replay).unwrap(), 1);
        assert_eq!(
            events(&replay),
            vec![Event::Request {
                name: "b.json".into(),
                body: "2".into()
            }]
        );
        assert_eq!(second.emit_requests(&mut replay).unwrap(), 0);
    }

    #[test]
    fn missing_requests_dir_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut out = Vec::new();
        assert_eq!(mailbox(dir.path()).emit_requests(&mut out).unwrap(), 0);
    }

    /// A response lands atomically, leaves a receipt marker, and is acked.
    #[test]
    fn responses_are_written_atomically_with_a_marker_and_acked() {
        let dir = tempfile::tempdir().unwrap();
        let responses = dir.path().join("rpc/responses");
        let mut mailbox = mailbox(dir.path());
        let mut out = Vec::new();
        mailbox
            .handle_host_line(&response("a.json", r#"{"ok":true}"#), &mut out)
            .unwrap();
        assert_eq!(
            fs::read_to_string(responses.join("a.json")).unwrap(),
            r#"{"ok":true}"#
        );
        assert!(!responses.join(".a.json.tmp").exists());
        assert!(dir.path().join("delivered/a.json").exists());
        assert_eq!(
            events(&out),
            vec![Event::Ack {
                name: "a.json".into()
            }]
        );
    }

    /// The host re-sends a response after its ack was lost and the relay
    /// process (or the whole VM) has restarted in between: the marker, not
    /// any in-memory state, says it was delivered, so an answer the agent
    /// already consumed is acked again and not resurrected.
    #[test]
    fn a_fresh_mailbox_acks_a_replayed_response_without_rewriting() {
        let dir = tempfile::tempdir().unwrap();
        let answer = dir.path().join("rpc/responses/a.json");
        let line = response("a.json", r#"{"ok":true}"#);

        let mut first = mailbox(dir.path());
        first.handle_host_line(&line, &mut Vec::new()).unwrap();
        assert!(answer.exists());
        drop(first);
        // The agent consumed its answer.
        fs::remove_file(&answer).unwrap();

        let mut second = mailbox(dir.path());
        let mut again = Vec::new();
        second.handle_host_line(&line, &mut again).unwrap();
        assert!(!answer.exists(), "a consumed response must not reappear");
        assert_eq!(
            events(&again),
            vec![Event::Ack {
                name: "a.json".into()
            }]
        );
    }

    /// Session one took A and B. Session two's host still holds A (its ack
    /// was lost) and re-sends it before `synced`: B's marker goes, A's stays
    /// and keeps A from being rewritten; a marker made after the sync is kept
    /// through a stray second `synced`.
    #[test]
    fn synced_prunes_markers_for_responses_the_host_no_longer_holds() {
        let dir = tempfile::tempdir().unwrap();
        let markers = dir.path().join("delivered");
        let answers = dir.path().join("rpc/responses");
        let a = response("a.json", "A");
        let b = response("b.json", "B");
        let mut sink = Vec::new();

        let mut first = mailbox(dir.path());
        first.handle_host_line(&a, &mut sink).unwrap();
        first.handle_host_line(&b, &mut sink).unwrap();
        drop(first);
        fs::remove_file(answers.join("a.json")).unwrap();
        fs::remove_file(answers.join("b.json")).unwrap();
        assert!(markers.join("a.json").exists());
        assert!(markers.join("b.json").exists());

        let mut second = mailbox(dir.path());
        second.handle_host_line(&a, &mut sink).unwrap();
        second.handle_host_line(&synced(), &mut sink).unwrap();
        assert!(markers.join("a.json").exists(), "A may still be replayed");
        assert!(!markers.join("b.json").exists(), "nothing can replay B");
        assert!(!answers.join("a.json").exists());

        // A again, still deduped.
        let mut out = Vec::new();
        second.handle_host_line(&a, &mut out).unwrap();
        assert!(!answers.join("a.json").exists());
        assert_eq!(
            events(&out),
            vec![Event::Ack {
                name: "a.json".into()
            }]
        );

        // New after the sync: its marker outlives a second `synced`.
        second
            .handle_host_line(&response("c.json", "C"), &mut sink)
            .unwrap();
        second.handle_host_line(&synced(), &mut sink).unwrap();
        assert!(markers.join("c.json").exists());
        assert!(markers.join("a.json").exists());
    }

    #[test]
    fn rejects_other_ops_and_path_names() {
        let dir = tempfile::tempdir().unwrap();
        let mut mailbox = mailbox(dir.path());
        let mut out = Vec::new();
        let request = r#"{"op":"request","name":"a.json","body":""}"#;
        assert!(mailbox.handle_host_line(request, &mut out).is_err());
        let escape = r#"{"op":"response","name":"../x","body":""}"#;
        assert!(mailbox.handle_host_line(escape, &mut out).is_err());
        let escape_ack = r#"{"op":"ack","name":"../x"}"#;
        assert!(mailbox.handle_host_line(escape_ack, &mut out).is_err());
        assert!(mailbox.handle_host_line("not json", &mut out).is_err());
        assert!(!dir.path().join("x").exists());
        assert!(!dir.path().join("rpc/x").exists());
        assert!(out.is_empty());
    }
}
