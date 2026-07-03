//! Shared I/O plumbing for child-process agents.
//!
//! Both the persistent stream-json runner (`managed_session`) and the per-turn
//! runner (`exec_session`) drive their children the same way: read
//! newline-delimited JSON off stdout, log stderr, and reap the child through a
//! shared slot. The reap loop in particular carries a subtle race guard — a
//! newer owner can take the child out from under the reaper — so it lives here
//! and is exercised once.

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, ExitStatus};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::Value;

/// Read newline-delimited JSON off a child's stdout, calling `on_value` for
/// each parsed object. Blank lines are skipped, unparseable lines logged and
/// skipped, and a read error ends the loop. Runs on its own thread.
pub fn spawn_json_reader<R>(stdout: R, name: &'static str, on_value: impl Fn(Value) + Send + 'static)
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(l) if l.trim().is_empty() => continue,
                Ok(l) => match serde_json::from_str::<Value>(&l) {
                    Ok(v) => on_value(v),
                    Err(e) => tracing::warn!(name, error = %e, raw = %l, "bad json line"),
                },
                Err(e) => {
                    tracing::warn!(name, error = %e, "stdout read error");
                    break;
                }
            }
        }
        tracing::debug!(name, "stdout closed");
    });
}

/// Drain a child's stdout to EOF without parsing — for agents whose stdout is
/// plaintext (their history comes from an on-disk transcript). Draining keeps
/// the pipe from filling and blocking the child. Runs on its own thread.
pub fn spawn_drain<R>(stdout: R, name: &'static str)
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        for _ in BufReader::new(stdout).lines().map_while(Result::ok) {}
        tracing::debug!(name, "stdout closed");
    });
}

/// Log every stderr line as a warning and forward it to `sink`, followed by a
/// terminating `None` once the stream ends (EOF or read error). The `sink` lets
/// callers capture stderr — e.g. to fold into an exit message; pass a no-op to
/// only log. Runs on its own thread.
pub fn spawn_stderr_reader<R>(
    stderr: R,
    name: &'static str,
    sink: impl Fn(Option<String>) + Send + 'static,
) where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            tracing::warn!(name, stderr = %line, "stderr");
            sink(Some(line));
        }
        sink(None);
    });
}

/// Poll a shared child slot until the process exits, then hand the terminal
/// status to `on_exit` exactly once. Runs on its own thread.
///
/// The slot is shared with whatever spawns replacement children. If that code
/// takes the child out from under us — a kill+take for a superseded turn — we
/// observe `None` and return silently, leaving the new owner to report its own
/// lifecycle. On a genuine exit we `take()` the child so it is reaped once, then
/// fire `on_exit`; a handed-off slot never fires it.
pub fn spawn_reaper<F>(slot: Arc<Mutex<Option<Child>>>, name: &'static str, on_exit: F)
where
    F: FnOnce(std::io::Result<ExitStatus>) + Send + 'static,
{
    thread::spawn(move || loop {
        let outcome = {
            let mut guard = slot.lock();
            let Some(child) = guard.as_mut() else {
                return;
            };
            match child.try_wait() {
                Ok(Some(status)) => {
                    let _ = guard.take();
                    tracing::debug!(name, %status, "child exited");
                    Some(Ok(status))
                }
                Ok(None) => None,
                Err(e) => {
                    let _ = guard.take();
                    tracing::warn!(name, error = %e, "child wait failed");
                    Some(Err(e))
                }
            }
        };
        match outcome {
            Some(status) => return on_exit(status),
            None => thread::sleep(Duration::from_millis(50)),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Instant;

    fn script(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
        let path = dir.join("s.sh");
        std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn spawn(dir: &std::path::Path, body: &str) -> Child {
        Command::new(script(dir, body))
            .current_dir(dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }

    #[test]
    fn json_reader_forwards_parsed_values_and_skips_junk() {
        let dir = tempfile::tempdir().unwrap();
        let mut child = spawn(&dir.path(), "echo '{\"a\":1}'\necho ''\necho 'not json'\necho '{\"a\":2}'\n");
        let (tx, rx) = mpsc::channel();
        spawn_json_reader(child.stdout.take().unwrap(), "t", move |v| {
            let _ = tx.send(v);
        });
        let first = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let second = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(first["a"], 1);
        assert_eq!(second["a"], 2);
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        let _ = child.wait();
    }

    #[test]
    fn stderr_reader_forwards_lines_then_none_sentinel() {
        let dir = tempfile::tempdir().unwrap();
        let mut child = spawn(&dir.path(), "echo 'boom' >&2\n");
        let (tx, rx) = mpsc::channel();
        spawn_stderr_reader(child.stderr.take().unwrap(), "t", move |line| {
            let _ = tx.send(line);
        });
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Some("boom".into()));
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), None);
        let _ = child.wait();
    }

    #[test]
    fn reaper_reports_a_genuine_exit() {
        let dir = tempfile::tempdir().unwrap();
        let slot = Arc::new(Mutex::new(Some(spawn(&dir.path(), "exit 3\n"))));
        let (tx, rx) = mpsc::channel();
        spawn_reaper(slot, "t", move |status| {
            let _ = tx.send(status.unwrap().code());
        });
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Some(3));
    }

    /// The race guard: if a newer owner empties the slot before the reaper sees
    /// the exit, the reaper must stay silent — the new owner reports instead.
    #[test]
    fn reaper_stays_silent_when_the_slot_is_taken_away() {
        let dir = tempfile::tempdir().unwrap();
        let slot = Arc::new(Mutex::new(Some(spawn(&dir.path(), "sleep 30\n"))));
        let (tx, rx) = mpsc::channel();
        // Empty the slot (and reap the child) as the superseding owner would,
        // before starting the reaper so it observes `None` on its first poll.
        let mut taken = slot.lock().take().unwrap();
        let _ = taken.kill();
        let _ = taken.wait();
        spawn_reaper(slot, "t", move |_| {
            let _ = tx.send(());
        });
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            assert!(rx.try_recv().is_err(), "reaper fired on a handed-off slot");
            thread::sleep(Duration::from_millis(20));
        }
    }
}
