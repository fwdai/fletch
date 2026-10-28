//! `relay <mailbox dir>`: the file mailbox over a byte stream (spec §7.3).
//!
//! Requests appear as `<dir>/requests/*.json`; each goes out as one JSON line
//! and the file is removed. Responses come back as JSON lines and land in
//! `<dir>/responses/<name>` through a temp file and rename, so the agent's own
//! watcher never sees a half-written response. Polling at 100 ms matches the
//! host's mailbox watcher; stdin EOF means the host is gone and the relay exits.

use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use crate::protocol::{write_line, Event};
use crate::Result;

const POLL: Duration = Duration::from_millis(100);

pub fn run(args: &[String]) -> Result<()> {
    let dir = PathBuf::from(args.first().ok_or("relay: missing <mailbox dir>")?);
    let responses = dir.join("responses");
    thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if let Err(err) = handle_response_line(&responses, &line) {
                eprintln!("fletch-guest: relay: response: {err}");
            }
        }
        std::process::exit(0);
    });

    let requests = dir.join("requests");
    let mut out = io::stdout().lock();
    loop {
        drain_requests(&requests, &mut out)?;
        thread::sleep(POLL);
    }
}

/// Emits every `*.json` in `requests` (oldest name first) and deletes it.
pub fn drain_requests(requests: &Path, out: &mut impl Write) -> io::Result<usize> {
    let entries = match fs::read_dir(requests) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err),
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(".json"))
        .collect();
    names.sort();

    let mut sent = 0;
    for name in names {
        let path = requests.join(&name);
        let body = match fs::read(&path) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            // Taken by someone else between listing and reading.
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };
        write_line(out, &Event::Request { name, body })?;
        fs::remove_file(&path)?;
        sent += 1;
    }
    Ok(sent)
}

/// Parses one `response` line and writes it atomically into `responses`.
pub fn handle_response_line(responses: &Path, line: &str) -> Result<()> {
    let Event::Response { name, body } = serde_json::from_str(line)? else {
        return Err("expected {\"op\":\"response\",...}".into());
    };
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return Err(format!("bad response name {name:?}").into());
    }
    fs::create_dir_all(responses)?;
    let tmp = responses.join(format!(".{name}.tmp"));
    fs::write(&tmp, body)?;
    fs::rename(&tmp, responses.join(&name))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_go_out_as_lines_and_are_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let requests = dir.path().join("requests");
        fs::create_dir(&requests).unwrap();
        fs::write(requests.join("b.json"), r#"{"n":2}"#).unwrap();
        fs::write(requests.join("a.json"), r#"{"n":1}"#).unwrap();
        fs::write(requests.join("notes.txt"), "ignored").unwrap();

        let mut out = Vec::new();
        assert_eq!(drain_requests(&requests, &mut out).unwrap(), 2);
        let lines: Vec<Event> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(
            lines,
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
        assert!(!requests.join("a.json").exists());
        assert!(!requests.join("b.json").exists());
        assert!(requests.join("notes.txt").exists());

        let mut again = Vec::new();
        assert_eq!(drain_requests(&requests, &mut again).unwrap(), 0);
        assert!(again.is_empty());
    }

    #[test]
    fn missing_requests_dir_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut out = Vec::new();
        assert_eq!(
            drain_requests(&dir.path().join("requests"), &mut out).unwrap(),
            0
        );
    }

    #[test]
    fn responses_are_written_atomically_into_responses() {
        let dir = tempfile::tempdir().unwrap();
        let responses = dir.path().join("responses");
        let line = serde_json::to_string(&Event::Response {
            name: "a.json".into(),
            body: r#"{"ok":true}"#.into(),
        })
        .unwrap();
        handle_response_line(&responses, &line).unwrap();
        assert_eq!(
            fs::read_to_string(responses.join("a.json")).unwrap(),
            r#"{"ok":true}"#
        );
        assert!(!responses.join(".a.json.tmp").exists());
    }

    #[test]
    fn rejects_other_ops_and_path_names() {
        let dir = tempfile::tempdir().unwrap();
        let responses = dir.path().join("responses");
        let request = r#"{"op":"request","name":"a.json","body":""}"#;
        assert!(handle_response_line(&responses, request).is_err());
        let escape = r#"{"op":"response","name":"../x","body":""}"#;
        assert!(handle_response_line(&responses, escape).is_err());
        assert!(handle_response_line(&responses, "not json").is_err());
        assert!(!dir.path().join("x").exists());
    }
}
