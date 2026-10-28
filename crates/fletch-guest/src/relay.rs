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
//! half-written response; the guest then acks, and the host deletes its copy.
//! A response the host re-sends (its ack was lost) is acked again but not
//! rewritten. Polling at 100 ms matches the host's mailbox watcher; stdin EOF
//! means the host is gone and the relay exits.

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

pub fn run(args: &[String]) -> Result<()> {
    let dir = PathBuf::from(args.first().ok_or("relay: missing <mailbox dir>")?);
    let mailbox = Arc::new(Mutex::new(Mailbox::new(&dir)));
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

    loop {
        {
            let mut out = io::stdout().lock();
            lock(&mailbox).emit_requests(&mut out)?;
        }
        thread::sleep(POLL);
    }
}

fn lock(mailbox: &Mutex<Mailbox>) -> std::sync::MutexGuard<'_, Mailbox> {
    mailbox.lock().unwrap_or_else(|e| e.into_inner())
}

/// The guest half of the mailbox: what has gone out and what has come in.
pub struct Mailbox {
    requests: PathBuf,
    responses: PathBuf,
    /// Request names emitted on this session and not yet acked by the host.
    emitted: HashSet<String>,
    /// Response names already written, so a re-sent one is acked, not rewritten.
    written: HashSet<String>,
}

impl Mailbox {
    pub fn new(dir: &Path) -> Self {
        Self {
            requests: dir.join("requests"),
            responses: dir.join("responses"),
            emitted: HashSet::new(),
            written: HashSet::new(),
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
    /// `responses` (once) and acked; an `ack` deletes the request it names.
    pub fn handle_host_line(&mut self, line: &str, out: &mut impl Write) -> Result<()> {
        match serde_json::from_str(line)? {
            Event::Response { name, body } => {
                check_name(&name)?;
                if !self.written.contains(&name) {
                    fs::create_dir_all(&self.responses)?;
                    let tmp = self.responses.join(format!(".{name}.tmp"));
                    fs::write(&tmp, body)?;
                    fs::rename(&tmp, self.responses.join(&name))?;
                    self.written.insert(name.clone());
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
            _ => Err("expected {\"op\":\"response\",...} or {\"op\":\"ack\",...}".into()),
        }
    }
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

    /// A request goes out as a line and stays on disk until the host acks it.
    #[test]
    fn requests_go_out_once_and_survive_until_acked() {
        let dir = tempfile::tempdir().unwrap();
        let requests = dir.path().join("requests");
        fs::create_dir(&requests).unwrap();
        fs::write(requests.join("b.json"), r#"{"n":2}"#).unwrap();
        fs::write(requests.join("a.json"), r#"{"n":1}"#).unwrap();
        fs::write(requests.join("notes.txt"), "ignored").unwrap();

        let mut mailbox = Mailbox::new(dir.path());
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
        let requests = dir.path().join("requests");
        fs::create_dir(&requests).unwrap();
        fs::write(requests.join("a.json"), "1").unwrap();
        fs::write(requests.join("b.json"), "2").unwrap();

        let mut first = Mailbox::new(dir.path());
        let mut out = Vec::new();
        assert_eq!(first.emit_requests(&mut out).unwrap(), 2);
        first.handle_host_line(&ack("a.json"), &mut out).unwrap();
        drop(first);

        let mut second = Mailbox::new(dir.path());
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
        assert_eq!(Mailbox::new(dir.path()).emit_requests(&mut out).unwrap(), 0);
    }

    /// A response lands atomically and is acked; the same response again is
    /// acked but not rewritten.
    #[test]
    fn responses_are_written_atomically_once_and_acked() {
        let dir = tempfile::tempdir().unwrap();
        let responses = dir.path().join("responses");
        let line = serde_json::to_string(&Event::Response {
            name: "a.json".into(),
            body: r#"{"ok":true}"#.into(),
        })
        .unwrap();
        let mut mailbox = Mailbox::new(dir.path());
        let mut out = Vec::new();
        mailbox.handle_host_line(&line, &mut out).unwrap();
        assert_eq!(
            fs::read_to_string(responses.join("a.json")).unwrap(),
            r#"{"ok":true}"#
        );
        assert!(!responses.join(".a.json.tmp").exists());
        assert_eq!(
            events(&out),
            vec![Event::Ack {
                name: "a.json".into()
            }]
        );

        // The agent may have consumed it by the time the host re-sends.
        fs::remove_file(responses.join("a.json")).unwrap();
        let mut again = Vec::new();
        mailbox.handle_host_line(&line, &mut again).unwrap();
        assert!(!responses.join("a.json").exists());
        assert_eq!(
            events(&again),
            vec![Event::Ack {
                name: "a.json".into()
            }]
        );
    }

    #[test]
    fn rejects_other_ops_and_path_names() {
        let dir = tempfile::tempdir().unwrap();
        let mut mailbox = Mailbox::new(dir.path());
        let mut out = Vec::new();
        let request = r#"{"op":"request","name":"a.json","body":""}"#;
        assert!(mailbox.handle_host_line(request, &mut out).is_err());
        let escape = r#"{"op":"response","name":"../x","body":""}"#;
        assert!(mailbox.handle_host_line(escape, &mut out).is_err());
        let escape_ack = r#"{"op":"ack","name":"../x"}"#;
        assert!(mailbox.handle_host_line(escape_ack, &mut out).is_err());
        assert!(mailbox.handle_host_line("not json", &mut out).is_err());
        assert!(!dir.path().join("x").exists());
        assert!(out.is_empty());
    }
}
