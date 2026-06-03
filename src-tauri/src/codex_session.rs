//! Per-turn runner for Codex "custom view" agents.
//!
//! Unlike Claude's persistent stream-json pipe (`managed_session.rs`),
//! Codex's `exec` subcommand runs **one turn to completion and exits**.
//! So each user message spawns a fresh `codex exec [resume <id>]`
//! process; the process exiting is the *turn* boundary, not the agent
//! dying. Turn-end is signalled in-band by codex's `turn.completed`
//! event (see `CodexManagedActivity`), so the per-turn child exit is
//! purely internal cleanup and never tears the agent down.
//!
//! Codex assigns its own session ("thread") id, emitted as
//! `thread.started` on the first turn. We capture it, hand it to the
//! `on_session_id` callback (the supervisor persists it), and reuse it
//! via `codex exec resume <id>` on every later turn.

use parking_lot::Mutex;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::error::{Error, Result};

type EventCb = Arc<dyn Fn(Value) + Send + Sync>;
type SessionIdCb = Arc<dyn Fn(String) + Send + Sync>;
type ExitCb = Arc<dyn Fn(bool) + Send + Sync>;

pub struct CodexSpawn {
    /// Resolved path to the `codex` binary.
    pub codex: PathBuf,
    /// The agent's primary worktree — codex reads it as the workspace
    /// root (we set it as the child's cwd rather than passing `-C`,
    /// since `exec resume` doesn't accept `-C`).
    pub cwd: PathBuf,
    /// Codex thread id to resume, if we've already captured one.
    pub session_id: Option<String>,
}

pub struct CodexSession {
    codex: PathBuf,
    cwd: PathBuf,
    session_id: Arc<Mutex<Option<String>>>,
    child: Arc<Mutex<Option<Child>>>,
    /// Monotonic turn counter. A reap thread only reports its exit if its
    /// turn is still the latest — so a superseded turn's late exit can't
    /// flip the status of the turn that replaced it.
    turn_seq: Arc<AtomicU64>,
    on_event: EventCb,
    on_session_id: SessionIdCb,
    on_exit: ExitCb,
}

impl CodexSession {
    /// Build the session. Spawns **no** process — the first child is
    /// created when the first user message arrives.
    pub fn new<F, G, H>(spec: CodexSpawn, on_event: F, on_session_id: G, on_exit: H) -> Self
    where
        F: Fn(Value) + Send + Sync + 'static,
        G: Fn(String) + Send + Sync + 'static,
        H: Fn(bool) + Send + Sync + 'static,
    {
        Self {
            codex: spec.codex,
            cwd: spec.cwd,
            session_id: Arc::new(Mutex::new(spec.session_id)),
            child: Arc::new(Mutex::new(None)),
            turn_seq: Arc::new(AtomicU64::new(0)),
            on_event: Arc::new(on_event),
            on_session_id: Arc::new(on_session_id),
            on_exit: Arc::new(on_exit),
        }
    }

    fn build_args(&self, prompt: &str) -> Vec<String> {
        let mut args: Vec<String> = vec!["exec".into()];
        // Resume an existing thread once codex has handed us its id;
        // otherwise this is the first turn and codex starts fresh.
        if let Some(id) = self.session_id.lock().as_ref() {
            args.push("resume".into());
            args.push(id.clone());
        }
        args.push("--json".into());
        args.push("--skip-git-repo-check".into());
        // Approvals off + codex's own workspace-write sandbox on. Passed
        // as `-c` config (works on both `exec` and `exec resume`, unlike
        // the `-s`/`-a` flags which only exist on `exec`). Quorum does
        // not wrap codex in sandbox-exec; codex sandboxes itself.
        args.push("-c".into());
        args.push("approval_policy=\"never\"".into());
        args.push("-c".into());
        args.push("sandbox_mode=\"workspace-write\"".into());
        args.push(prompt.to_string());
        args
    }

    pub fn send_user_message(&self, text: &str, attachments: &[String]) -> Result<()> {
        // Claim this turn's sequence number first, so a superseded turn's
        // reap thread sees it's no longer current and stays quiet.
        let seq = self.turn_seq.fetch_add(1, Ordering::SeqCst) + 1;

        // A new turn supersedes any still-running one (e.g. the user
        // sent again before the prior turn finished). Kill + reap it.
        if let Some(mut prev) = self.child.lock().take() {
            let _ = prev.kill();
            let _ = prev.wait();
        }

        // The typed message plus one reference line per attachment, so
        // file paths never pollute the user's prose. Mirrors the Claude
        // managed path; codex reads each path with its own file tools.
        let mut prompt = text.to_string();
        for path in attachments {
            if !prompt.is_empty() {
                prompt.push_str("\n\n");
            }
            prompt.push_str(&format!("Attached file: {path}"));
        }

        let args = self.build_args(&prompt);
        let mut cmd = Command::new(&self.codex);
        cmd.args(&args);
        cmd.current_dir(&self.cwd);
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        tracing::info!(
            codex = %self.codex.display(),
            cwd = %self.cwd.display(),
            resume = self.session_id.lock().is_some(),
            "spawning codex exec turn"
        );

        let mut child = cmd
            .spawn()
            .map_err(|e| Error::Other(format!("codex spawn: {e}")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Other("codex: child stdout missing".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::Other("codex: child stderr missing".into()))?;

        *self.child.lock() = Some(child);

        let on_event = self.on_event.clone();
        let on_session_id = self.on_session_id.clone();
        let session_id = self.session_id.clone();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(l) if l.trim().is_empty() => continue,
                    Ok(l) => match serde_json::from_str::<Value>(&l) {
                        Ok(v) => {
                            maybe_capture_session_id(&v, &session_id, &on_session_id);
                            on_event(v);
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, raw = %l, "codex: bad json line");
                        }
                    },
                    Err(e) => {
                        tracing::warn!(error = %e, "codex: stdout read error");
                        break;
                    }
                }
            }
            tracing::debug!("codex: turn stdout closed");
        });

        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(std::result::Result::ok) {
                tracing::warn!(stderr = %line, "codex: stderr");
            }
        });

        // Reap the per-turn child when it exits so it doesn't linger as a
        // zombie, and report the exit so the supervisor can end the turn.
        // This is the per-turn analogue of a turn-end signal: a turn that
        // exits without emitting `turn.completed` (interrupt, crash, error)
        // still leaves the agent promptly instead of waiting for the
        // silence backstop. The agent itself stays alive across turns.
        let child_for_wait = self.child.clone();
        let turn_seq = self.turn_seq.clone();
        let on_exit = self.on_exit.clone();
        thread::spawn(move || loop {
            let exited = {
                let mut guard = child_for_wait.lock();
                let Some(c) = guard.as_mut() else {
                    // Slot emptied by a newer turn (kill+take) — that turn
                    // owns the lifecycle now; stay quiet.
                    return;
                };
                match c.try_wait() {
                    Ok(Some(status)) => {
                        let _ = guard.take();
                        tracing::debug!(status = %status, "codex: turn exited");
                        Some(status.success())
                    }
                    Ok(None) => None,
                    Err(e) => {
                        let _ = guard.take();
                        tracing::warn!(error = %e, "codex: wait failed");
                        Some(false)
                    }
                }
            };
            if let Some(success) = exited {
                // Only the current turn reports — a superseded turn's exit
                // must not flip the status of the turn that replaced it.
                if turn_seq.load(Ordering::SeqCst) == seq {
                    on_exit(success);
                }
                return;
            }
            thread::sleep(Duration::from_millis(50));
        });

        Ok(())
    }

    /// Interrupt the in-flight turn (SIGINT). Codex exits the current
    /// `exec` process; the agent stays alive for the next message.
    pub fn interrupt(&self) {
        #[cfg(unix)]
        {
            use nix::sys::signal::{kill, Signal};
            use nix::unistd::Pid;
            if let Some(child) = self.child.lock().as_ref() {
                let _ = kill(Pid::from_raw(child.id() as i32), Signal::SIGINT);
            }
        }
    }

    pub fn kill(&self) -> Result<()> {
        if let Some(mut child) = self.child.lock().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(())
    }
}

/// On the first turn codex emits `{"type":"thread.started","thread_id":…}`.
/// Capture the id once for resume, and notify the caller so it can be
/// persisted to the agent record.
fn maybe_capture_session_id(
    event: &Value,
    session_id: &Arc<Mutex<Option<String>>>,
    on_session_id: &SessionIdCb,
) {
    if event.get("type").and_then(|t| t.as_str()) != Some("thread.started") {
        return;
    }
    let Some(tid) = event.get("thread_id").and_then(|t| t.as_str()) else {
        return;
    };
    let mut guard = session_id.lock();
    if guard.as_deref() == Some(tid) {
        return;
    }
    *guard = Some(tid.to_string());
    drop(guard);
    on_session_id(tid.to_string());
}

impl Drop for CodexSession {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// Write an executable shell script that ignores its args and prints
    /// canned JSONL — a stand-in for `codex exec` so we can exercise the
    /// per-turn spawn + event plumbing without the real binary or API.
    fn fake_codex(dir: &std::path::Path, body: &str) -> PathBuf {
        let script = dir.join("fakecodex.sh");
        std::fs::write(&script, format!("#!/bin/sh\ncat <<'EOF'\n{body}\nEOF\n")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    #[test]
    fn spawns_a_turn_forwards_events_and_captures_session_id() {
        let dir = tempfile::tempdir().unwrap();
        let script = fake_codex(
            dir.path(),
            r#"{"type":"thread.started","thread_id":"abc-123"}
{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"hi"}}
{"type":"turn.completed","usage":{}}"#,
        );

        let (etx, erx) = mpsc::channel();
        let (stx, srx) = mpsc::channel();
        let (xtx, xrx) = mpsc::channel();
        let session = CodexSession::new(
            CodexSpawn {
                codex: script,
                cwd: dir.path().to_path_buf(),
                session_id: None,
            },
            move |ev| {
                let _ = etx.send(ev);
            },
            move |sid| {
                let _ = stx.send(sid);
            },
            move |success| {
                let _ = xtx.send(success);
            },
        );

        session.send_user_message("hello", &[]).unwrap();

        let mut events = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(ev) = erx.recv_timeout(Duration::from_millis(100)) {
                let done = ev.get("type").and_then(|t| t.as_str()) == Some("turn.completed");
                events.push(ev);
                if done {
                    break;
                }
            }
        }

        // The thread id was reported to the callback and stored internally
        // so the next turn resumes it.
        assert_eq!(srx.recv_timeout(Duration::from_secs(1)).unwrap(), "abc-123");
        assert_eq!(session.session_id.lock().as_deref(), Some("abc-123"));

        assert_eq!(events.len(), 3);
        assert_eq!(events[0].get("type").unwrap(), "thread.started");
        assert_eq!(events[2].get("type").unwrap(), "turn.completed");

        // The per-turn process exit is reported so the supervisor can end
        // the turn even if `turn.completed` were absent.
        assert_eq!(xrx.recv_timeout(Duration::from_secs(2)).unwrap(), true);
    }

    #[test]
    fn resume_turn_passes_the_session_id_as_an_arg() {
        let dir = tempfile::tempdir().unwrap();
        // This fake records its own argv to a file (avoiding JSON-escaping
        // the quoted `-c` args) and emits one valid event so the turn
        // completes. CodexSession runs it with cwd = dir, so argv.txt
        // lands there.
        let script = dir.path().join("argecho.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf '%s' \"$*\" > argv.txt\necho '{\"type\":\"turn.completed\"}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let (etx, erx) = mpsc::channel();
        let session = CodexSession::new(
            CodexSpawn {
                codex: script,
                cwd: dir.path().to_path_buf(),
                session_id: Some("prev-thread".into()),
            },
            move |ev| {
                let _ = etx.send(ev);
            },
            |_sid| {},
            |_success| {},
        );

        session.send_user_message("again", &[]).unwrap();

        // Wait for the turn to finish so argv.txt is fully written.
        erx.recv_timeout(Duration::from_secs(5)).unwrap();
        let args = std::fs::read_to_string(dir.path().join("argv.txt")).unwrap();
        assert!(args.contains("exec resume prev-thread"), "argv was: {args}");
        assert!(args.contains("--json"));
        assert!(args.contains("sandbox_mode=\"workspace-write\""), "argv was: {args}");
    }
}
