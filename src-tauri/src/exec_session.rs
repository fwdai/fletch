//! Generic per-turn runner for agents that run one turn per process.
//!
//! Unlike Claude's persistent stream-json pipe (`managed_session.rs`),
//! some agents (codex, cursor-agent) run **one turn to completion and
//! exit**. Each user message spawns a fresh process; the process exiting
//! is the *turn* boundary, not the agent dying, so the per-turn child
//! exit is internal cleanup (reported via `on_exit`) and never tears the
//! agent down. The agent itself persists across turns.
//!
//! The two provider-specific bits are injected:
//!   - `build_args(prompt, session_id)` → the argv for one turn (fresh
//!     when `session_id` is `None`, resume otherwise).
//!   - `extract_session_id(event)` → the id from whichever event carries
//!     it (codex's `thread.started`, cursor's `system/init`). These
//!     agents assign their own session id rather than taking ours, so we
//!     capture it from the first turn and reuse it to resume.

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
type ArgsBuilder = Arc<dyn Fn(&str, Option<&str>, Option<&str>) -> Vec<String> + Send + Sync>;
type IdExtractor = Arc<dyn Fn(&Value) -> Option<String> + Send + Sync>;

pub struct ExecSpawn {
    /// Program to launch each turn. Normally `sandbox-exec`, with the real
    /// agent binary carried in `prefix_args`; tests pass the agent directly.
    pub program: PathBuf,
    /// Args inserted before the per-turn args on every spawn — e.g.
    /// `["-f", <profile>, <agent_bin>]` when `program` is `sandbox-exec`. Empty
    /// when the agent runs unwrapped.
    pub prefix_args: Vec<String>,
    /// Sandbox profile tempfile, held for the session's lifetime so the profile
    /// path embedded in `prefix_args` stays valid across the per-turn respawns.
    pub profile: Option<tempfile::NamedTempFile>,
    /// The agent's primary worktree — set as the child's cwd.
    pub cwd: PathBuf,
    /// Session id to resume, if one has been captured already.
    pub session_id: Option<String>,
    /// When false, the turn's stdout is **plaintext** — drained without JSON
    /// parsing (no events emitted). History for such agents comes from their
    /// on-disk transcript, and the session id from the filesystem.
    pub stdout_is_json: bool,
    /// Extra environment variables (e.g. `QUORUM_RPC_DIR`) set on every turn's
    /// child process, layered on top of the inherited environment.
    pub env: Vec<(String, String)>,
}

pub struct ExecSession {
    program: PathBuf,
    prefix_args: Vec<String>,
    /// Kept alive (not read) so the sandbox profile file outlives the session.
    _profile: Option<tempfile::NamedTempFile>,
    cwd: PathBuf,
    stdout_is_json: bool,
    env: Vec<(String, String)>,
    session_id: Arc<Mutex<Option<String>>>,
    child: Arc<Mutex<Option<Child>>>,
    /// Monotonic turn counter. A reap thread only reports its exit if its
    /// turn is still the latest — so a superseded turn's late exit can't
    /// flip the status of the turn that replaced it.
    turn_seq: Arc<AtomicU64>,
    build_args: ArgsBuilder,
    extract_session_id: IdExtractor,
    on_event: EventCb,
    on_session_id: SessionIdCb,
    on_exit: ExitCb,
}

pub struct ExecCallbacks<F, G, H> {
    pub on_event: F,
    pub on_session_id: G,
    pub on_exit: H,
}

impl ExecSession {
    /// Build the session. Spawns **no** process — the first child is
    /// created when the first user message arrives.
    pub fn new<A, I, F, G, H>(
        spec: ExecSpawn,
        build_args: A,
        extract_session_id: I,
        cb: ExecCallbacks<F, G, H>,
    ) -> Self
    where
        A: Fn(&str, Option<&str>, Option<&str>) -> Vec<String> + Send + Sync + 'static,
        I: Fn(&Value) -> Option<String> + Send + Sync + 'static,
        F: Fn(Value) + Send + Sync + 'static,
        G: Fn(String) + Send + Sync + 'static,
        H: Fn(bool) + Send + Sync + 'static,
    {
        Self {
            program: spec.program,
            prefix_args: spec.prefix_args,
            _profile: spec.profile,
            cwd: spec.cwd,
            stdout_is_json: spec.stdout_is_json,
            env: spec.env,
            session_id: Arc::new(Mutex::new(spec.session_id)),
            child: Arc::new(Mutex::new(None)),
            turn_seq: Arc::new(AtomicU64::new(0)),
            build_args: Arc::new(build_args),
            extract_session_id: Arc::new(extract_session_id),
            on_event: Arc::new(cb.on_event),
            on_session_id: Arc::new(cb.on_session_id),
            on_exit: Arc::new(cb.on_exit),
        }
    }

    pub fn send_user_message(&self, text: &str, attachments: &[String], thinking: Option<&str>) -> Result<()> {
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
        // file paths never pollute the user's prose; the agent reads each
        // path with its own file tools.
        let mut prompt = text.to_string();
        for path in attachments {
            if !prompt.is_empty() {
                prompt.push_str("\n\n");
            }
            prompt.push_str(&format!("Attached file: {path}"));
        }

        let args = {
            let id = self.session_id.lock();
            (self.build_args)(&prompt, id.as_deref(), thinking)
        };
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.prefix_args);
        cmd.args(&args);
        cmd.current_dir(&self.cwd);
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        tracing::info!(
            program = %self.program.display(),
            cwd = %self.cwd.display(),
            resume = self.session_id.lock().is_some(),
            "spawning per-turn agent process"
        );

        let mut child = cmd
            .spawn()
            .map_err(|e| Error::Other(format!("agent spawn: {e}")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Other("agent: child stdout missing".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::Other("agent: child stderr missing".into()))?;

        *self.child.lock() = Some(child);

        let on_event = self.on_event.clone();
        let on_session_id = self.on_session_id.clone();
        let extract_session_id = self.extract_session_id.clone();
        let session_id = self.session_id.clone();
        let stdout_is_json = self.stdout_is_json;
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(l) if l.trim().is_empty() => continue,
                    // Plaintext turn runner (e.g. agy): drain stdout without
                    // parsing — there are no events; history comes from the
                    // on-disk transcript ingested at turn-end.
                    Ok(_) if !stdout_is_json => continue,
                    Ok(l) => match serde_json::from_str::<Value>(&l) {
                        Ok(v) => {
                            maybe_capture_session_id(
                                &v,
                                &extract_session_id,
                                &session_id,
                                &on_session_id,
                            );
                            on_event(v);
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, raw = %l, "agent: bad json line");
                        }
                    },
                    Err(e) => {
                        tracing::warn!(error = %e, "agent: stdout read error");
                        break;
                    }
                }
            }
            tracing::debug!("agent: turn stdout closed");
        });

        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(std::result::Result::ok) {
                tracing::warn!(stderr = %line, "agent: stderr");
            }
        });

        // Reap the per-turn child when it exits, and report the exit so the
        // supervisor can end the turn — covering turns that exit without an
        // in-band turn-end event (interrupt, crash). The agent stays alive.
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
                        tracing::debug!(status = %status, "agent: turn exited");
                        Some(status.success())
                    }
                    Ok(None) => None,
                    Err(e) => {
                        let _ = guard.take();
                        tracing::warn!(error = %e, "agent: wait failed");
                        Some(false)
                    }
                }
            };
            if let Some(success) = exited {
                if turn_seq.load(Ordering::SeqCst) == seq {
                    on_exit(success);
                }
                return;
            }
            thread::sleep(Duration::from_millis(50));
        });

        Ok(())
    }

    /// Interrupt the in-flight turn (SIGINT). The agent stays alive for
    /// the next message.
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

/// Capture the agent-assigned session id the first time it appears, and
/// notify the caller so it can be persisted for resume.
fn maybe_capture_session_id(
    event: &Value,
    extract: &IdExtractor,
    session_id: &Arc<Mutex<Option<String>>>,
    on_session_id: &SessionIdCb,
) {
    let Some(id) = extract(event) else {
        return;
    };
    let mut guard = session_id.lock();
    if guard.as_deref() == Some(id.as_str()) {
        return;
    }
    *guard = Some(id.clone());
    drop(guard);
    on_session_id(id);
}

impl Drop for ExecSession {
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

    fn fake_agent(dir: &std::path::Path, body: &str) -> PathBuf {
        let script = dir.join("fakeagent.sh");
        std::fs::write(&script, format!("#!/bin/sh\ncat <<'EOF'\n{body}\nEOF\n")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    // A codex-style config: id from `thread.started`, end on `turn.completed`.
    fn codex_args(prompt: &str, session_id: Option<&str>, _thinking: Option<&str>) -> Vec<String> {
        let mut a = vec!["exec".to_string()];
        if let Some(id) = session_id {
            a.push("resume".into());
            a.push(id.to_string());
        }
        a.push("--json".into());
        a.push(prompt.to_string());
        a
    }
    fn codex_id(ev: &Value) -> Option<String> {
        if ev.get("type").and_then(|t| t.as_str()) == Some("thread.started") {
            ev.get("thread_id")
                .and_then(|t| t.as_str())
                .map(str::to_string)
        } else {
            None
        }
    }

    #[test]
    fn spawns_a_turn_forwards_events_captures_id_and_reports_exit() {
        let dir = tempfile::tempdir().unwrap();
        let script = fake_agent(
            dir.path(),
            r#"{"type":"thread.started","thread_id":"abc-123"}
{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"hi"}}
{"type":"turn.completed","usage":{}}"#,
        );

        let (etx, erx) = mpsc::channel();
        let (stx, srx) = mpsc::channel();
        let (xtx, xrx) = mpsc::channel();
        let session = ExecSession::new(
            ExecSpawn {
                program: script,
                prefix_args: vec![],
                profile: None,
                cwd: dir.path().to_path_buf(),
                session_id: None,
                stdout_is_json: true,
                env: vec![],
            },
            codex_args,
            codex_id,
            ExecCallbacks {
                on_event: move |ev| {
                    let _ = etx.send(ev);
                },
                on_session_id: move |sid| {
                    let _ = stx.send(sid);
                },
                on_exit: move |success| {
                    let _ = xtx.send(success);
                },
            },
        );

        session.send_user_message("hello", &[], None).unwrap();

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

        assert_eq!(srx.recv_timeout(Duration::from_secs(1)).unwrap(), "abc-123");
        assert_eq!(session.session_id.lock().as_deref(), Some("abc-123"));
        assert_eq!(events.len(), 3);
        assert_eq!(xrx.recv_timeout(Duration::from_secs(2)).unwrap(), true);
    }

    #[test]
    fn resume_turn_passes_the_session_id_via_the_args_builder() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("argecho.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf '%s' \"$*\" > argv.txt\necho '{\"type\":\"turn.completed\"}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let (etx, erx) = mpsc::channel();
        let session = ExecSession::new(
            ExecSpawn {
                program: script,
                prefix_args: vec![],
                profile: None,
                cwd: dir.path().to_path_buf(),
                session_id: Some("prev-thread".into()),
                stdout_is_json: true,
                env: vec![],
            },
            codex_args,
            codex_id,
            ExecCallbacks {
                on_event: move |ev| {
                    let _ = etx.send(ev);
                },
                on_session_id: |_sid| {},
                on_exit: |_success| {},
            },
        );

        session.send_user_message("again", &[], None).unwrap();
        erx.recv_timeout(Duration::from_secs(5)).unwrap();
        let args = std::fs::read_to_string(dir.path().join("argv.txt")).unwrap();
        assert!(args.contains("exec resume prev-thread"), "argv was: {args}");
        assert!(args.contains("--json"));
    }
}
