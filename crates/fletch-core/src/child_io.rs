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
///
/// `closed_level` is the level for the terminal "stdout closed" line — pass
/// `INFO` for a long-lived process whose pipe closing is a lifecycle event
/// worth seeing in production, `DEBUG` for chatty per-turn readers. It can
/// precede the process exit (e.g. a crash before the reaper's `try_wait`
/// fires), so it isn't always redundant with the exit log.
pub fn spawn_json_reader<R>(
    stdout: R,
    name: &'static str,
    closed_level: tracing::Level,
    on_value: impl Fn(Value) + Send + 'static,
) where
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
        log_stdout_closed(name, closed_level);
    });
}

/// Emit the "stdout closed" line at a caller-chosen level. The `tracing` macros
/// bake the level in at compile time, so a runtime level needs this dispatch.
fn log_stdout_closed(name: &'static str, level: tracing::Level) {
    match level {
        tracing::Level::INFO => tracing::info!(name, "stdout closed"),
        _ => tracing::debug!(name, "stdout closed"),
    }
}

/// Drain a child's stdout to EOF without parsing — for agents whose stdout is
/// plaintext (their history comes from an on-disk transcript). Draining keeps
/// the pipe from filling and blocking the child. Runs on its own thread.
pub fn spawn_drain<R>(stdout: R, name: &'static str)
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if let Err(e) = line {
                tracing::warn!(name, error = %e, "stdout read error (drain)");
                break;
            }
        }
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

/// A session's child, in the one place it is ever reaped from: by the reaper
/// thread when it exits on its own ([`spawn_reaper`]), by a kill, by a
/// superseding turn ([`kill_and_reap`](Self::kill_and_reap)). Whichever path
/// takes the child out runs the step that must follow every reap — the
/// launch plan's `after_exit`, which a box uses to write the CLI's credential
/// changes back — exactly once per child, after the process is gone. A
/// caller cannot reap around it: the child is only reachable through here.
pub struct ChildSlot {
    child: Mutex<Option<Child>>,
    after_exit: Option<crate::sandbox::TurnPrep>,
}

impl ChildSlot {
    pub fn new(after_exit: Option<crate::sandbox::TurnPrep>) -> Arc<Self> {
        Arc::new(Self {
            child: Mutex::new(None),
            after_exit,
        })
    }

    /// Publish `child` as the occupant. The slot must be empty: a previous
    /// occupant is reaped first ([`kill_and_reap`](Self::kill_and_reap)), so
    /// no child is ever dropped unreaped.
    pub fn put(&self, child: Child) {
        let previous = self.child.lock().replace(child);
        debug_assert!(previous.is_none(), "a child was published over another");
    }

    /// The occupant's pid, to signal it.
    pub fn pid(&self) -> Option<u32> {
        self.child.lock().as_ref().map(Child::id)
    }

    /// Kill the occupant and reap it, running the after-exit step; nothing to
    /// do for an empty slot. Blocks until the process is gone.
    pub fn kill_and_reap(&self, name: &'static str) {
        let Some(mut child) = self.child.lock().take() else {
            return;
        };
        let _ = child.kill();
        let status = child.wait();
        tracing::debug!(name, status = ?status.as_ref().ok(), "child killed and reaped");
        self.after_reap(name);
    }

    /// Reap the occupant if it has exited, running the after-exit step: the
    /// reaper thread's poll. `None` while it runs or when the slot is empty.
    fn try_reap(&self, name: &'static str) -> Option<std::io::Result<ExitStatus>> {
        let outcome = {
            let mut guard = self.child.lock();
            let child = guard.as_mut()?;
            match child.try_wait() {
                Ok(Some(status)) => {
                    let _ = guard.take();
                    tracing::debug!(name, %status, "child exited");
                    Ok(status)
                }
                Ok(None) => return None,
                Err(e) => {
                    let _ = guard.take();
                    tracing::warn!(name, error = %e, "child wait failed");
                    Err(e)
                }
            }
        };
        self.after_reap(name);
        Some(outcome)
    }

    /// The step after a reap, outside the slot's lock: it may do I/O of its
    /// own (a box's ssh round trip).
    fn after_reap(&self, name: &'static str) {
        if let Some(step) = &self.after_exit {
            if let Err(e) = step() {
                tracing::warn!(name, error = %e, "after-exit step failed");
            }
        }
    }
}

/// Poll a child slot until the process exits, then hand the terminal status
/// to `on_exit` exactly once. Runs on its own thread.
///
/// If another path reaps the child first — a kill, a superseding turn — the
/// slot reads empty and the thread returns silently, leaving that path's
/// owner to report its own lifecycle; the after-exit step ran there. On a
/// genuine exit the slot reaps (and runs the step) before `on_exit` fires.
pub fn spawn_reaper<F>(slot: Arc<ChildSlot>, name: &'static str, on_exit: F)
where
    F: FnOnce(std::io::Result<ExitStatus>) + Send + 'static,
{
    thread::spawn(move || loop {
        if slot.child.lock().is_none() {
            return;
        }
        match slot.try_reap(name) {
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
        let path = script(dir, body);
        // Retry on ETXTBSY (errno 26): a sibling test thread's fork can briefly
        // inherit a write-fd to a just-written executable, making our execve
        // race. The window is tiny, so a few short retries clears it.
        for attempt in 0.. {
            match Command::new(&path)
                .current_dir(dir)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
            {
                Ok(child) => return child,
                Err(e) if e.raw_os_error() == Some(26) && attempt < 10 => {
                    thread::sleep(Duration::from_millis(20));
                }
                Err(e) => panic!("spawn failed: {e}"),
            }
        }
        unreachable!()
    }

    /// A counting after-exit step, and the slot it belongs to.
    fn counted_slot() -> (Arc<ChildSlot>, Arc<std::sync::atomic::AtomicUsize>) {
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let step = {
            let count = count.clone();
            Arc::new(move || {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }) as crate::sandbox::TurnPrep
        };
        (ChildSlot::new(Some(step)), count)
    }

    /// The reaper thread reaps a child that exits on its own: the after-exit
    /// step runs once, before the exit is reported; a later kill finds the
    /// slot empty and runs nothing.
    #[test]
    fn after_exit_runs_once_when_the_child_exits_on_its_own() {
        use std::sync::atomic::Ordering;
        let dir = tempfile::tempdir().unwrap();
        let (slot, count) = counted_slot();
        slot.put(spawn(dir.path(), "exit 0\n"));
        let (tx, rx) = mpsc::channel();
        let seen = count.clone();
        spawn_reaper(slot.clone(), "t", move |status| {
            let _ = tx.send((status.map(|s| s.success()), seen.load(Ordering::SeqCst)));
        });
        let (status, count_at_report) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(status.unwrap(), true);
        assert_eq!(
            count_at_report, 1,
            "the step runs before the exit is reported"
        );
        slot.kill_and_reap("t");
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    /// A kill reaps a running child itself: the after-exit step runs once
    /// there, and the reaper thread, finding the slot empty, reports nothing.
    #[test]
    fn after_exit_runs_once_when_a_kill_reaps_the_child() {
        use std::sync::atomic::Ordering;
        let dir = tempfile::tempdir().unwrap();
        let (slot, count) = counted_slot();
        slot.put(spawn(dir.path(), "sleep 30\n"));
        let (tx, rx) = mpsc::channel();
        spawn_reaper(slot.clone(), "t", move |status| {
            let _ = tx.send(status.map(|s| s.success()));
        });
        slot.kill_and_reap("t");
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(slot.pid().is_none());
        assert!(
            rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "a child reaped by the kill is not reported as an exit"
        );
        slot.kill_and_reap("t");
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "an empty slot runs nothing"
        );
    }

    #[test]
    fn json_reader_forwards_parsed_values_and_skips_junk() {
        let dir = tempfile::tempdir().unwrap();
        let mut child = spawn(
            dir.path(),
            "echo '{\"a\":1}'\necho ''\necho 'not json'\necho '{\"a\":2}'\n",
        );
        let (tx, rx) = mpsc::channel();
        spawn_json_reader(
            child.stdout.take().unwrap(),
            "t",
            tracing::Level::DEBUG,
            move |v| {
                let _ = tx.send(v);
            },
        );
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
        let mut child = spawn(dir.path(), "echo 'boom' >&2\n");
        let (tx, rx) = mpsc::channel();
        spawn_stderr_reader(child.stderr.take().unwrap(), "t", move |line| {
            let _ = tx.send(line);
        });
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            Some("boom".into())
        );
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), None);
        let _ = child.wait();
    }

    #[test]
    fn reaper_reports_a_genuine_exit() {
        let dir = tempfile::tempdir().unwrap();
        let slot = ChildSlot::new(None);
        slot.put(spawn(dir.path(), "exit 3\n"));
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
        let slot = ChildSlot::new(None);
        slot.put(spawn(dir.path(), "sleep 30\n"));
        let (tx, rx) = mpsc::channel();
        // Empty the slot (and reap the child) as the superseding owner would,
        // before starting the reaper so it observes `None` on its first poll.
        slot.kill_and_reap("t");
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
