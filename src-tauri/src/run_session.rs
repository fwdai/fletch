//! Per-agent "Run" process: spawns the project's setup + dev command
//! inside the agent's worktree, streams output to the frontend, and
//! tracks state across panel mounts.
//!
//! Separate from the agent's claude PTY (`agent.rs`) and from the
//! user-facing shell PTY (`open_agent_shell` in `supervisor.rs`).
//! The Run panel owns this session — clicking ▶ starts it, ■ kills
//! it. Setup runs at most once per agent lifetime (until archive).

use parking_lot::Mutex;
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::pty_session::{PtyExit, PtySession, PtySpawn};

/// How much PTY output we keep around for panel rehydration.
/// 5 MB is enough for ~50k lines of average-width terminal output and
/// caps memory growth for long-lived dev servers.
const LOG_BUFFER_CAP: usize = 5 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RunPhase {
    /// No process. Default state. Play button is enabled.
    Idle,
    /// Setup command in progress (e.g. `pnpm install`).
    Setup,
    /// Dev command in progress (e.g. `pnpm dev`).
    Running,
    /// Last process exited (or was killed). Holds last_error.
    Stopped,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunStateSnapshot {
    pub phase: RunPhase,
    pub last_error: Option<String>,
    /// Cumulative PTY output bytes (latest first dropped on overflow).
    /// Encoded as a byte array so the frontend can render or strip
    /// ANSI without re-decoding.
    pub log: Vec<u8>,
}

/// State for one agent's run process. Reused across start/stop cycles
/// (the log buffer survives stops so the panel can show what happened
/// the previous time you ran it).
pub struct RunSession {
    inner: Mutex<RunSessionInner>,
}

struct RunSessionInner {
    phase: RunPhase,
    last_error: Option<String>,
    log: LogBuffer,
    pty: Option<PtySession>,
    /// The `sandbox-exec` profile file the live PTY was launched under. The
    /// kernel compiles the SBPL into the child at `exec` and never consults the
    /// file again, so it only needs to exist until the child has spawned. We
    /// nonetheless hold it for the PTY's lifetime — a conservative simplification
    /// that ties cleanup to the process (dropped on stop/replace) rather than
    /// racing to unlink right after spawn.
    profile: Option<tempfile::NamedTempFile>,
    /// Bumped on every `start` and `stop`. The PTY exit handler
    /// captures the generation it was spawned under; if that
    /// generation no longer matches, the exit is from a process the
    /// user already stopped or replaced and is ignored.
    generation: u64,
}

impl RunSession {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RunSessionInner {
                phase: RunPhase::Idle,
                last_error: None,
                log: LogBuffer::new(LOG_BUFFER_CAP),
                pty: None,
                profile: None,
                generation: 0,
            }),
        }
    }

    pub fn snapshot(&self) -> RunStateSnapshot {
        let inner = self.inner.lock();
        RunStateSnapshot {
            phase: inner.phase,
            last_error: inner.last_error.clone(),
            log: inner.log.bytes().to_vec(),
        }
    }

    pub fn phase(&self) -> RunPhase {
        self.inner.lock().phase
    }

    pub fn is_active(&self) -> bool {
        matches!(self.phase(), RunPhase::Setup | RunPhase::Running)
    }

    /// Append PTY output to the rolling buffer. Called from the
    /// per-spawn output callback.
    pub fn append_log(&self, bytes: &[u8]) {
        self.inner.lock().log.append(bytes);
    }

    /// Kill the active PTY (if any), bump generation so any in-flight
    /// exit callback bails, and transition to Stopped. The on_exit
    /// from the killed PTY is ignored due to the generation bump.
    /// Returns the prior phase so callers can decide whether to emit
    /// state change.
    pub fn stop(&self) -> RunPhase {
        let mut inner = self.inner.lock();
        let prior = inner.phase;
        if let Some(pty) = inner.pty.take() {
            let _ = pty.kill();
        }
        inner.profile = None;
        inner.generation = inner.generation.wrapping_add(1);
        inner.phase = RunPhase::Stopped;
        inner.last_error = None;
        prior
    }

    /// Begin a fresh start sequence. Bumps the generation token (so
    /// any in-flight callbacks from a previous start are now stale),
    /// sets the initial phase, and clears `last_error`. Use
    /// `transition_phase` for chained transitions inside the same
    /// sequence (setup → run) — those keep the generation stable.
    pub fn begin_phase(&self, phase: RunPhase) -> u64 {
        let mut inner = self.inner.lock();
        inner.generation = inner.generation.wrapping_add(1);
        inner.phase = phase;
        inner.last_error = None;
        inner.generation
    }

    /// Like begin_phase but keeps the same generation. Used when one
    /// phase ends successfully and we chain into the next (setup→run).
    pub fn transition_phase(&self, phase: RunPhase) -> u64 {
        let mut inner = self.inner.lock();
        inner.phase = phase;
        inner.last_error = None;
        inner.generation
    }

    /// Attach a freshly spawned PTY together with the `sandbox-exec` profile
    /// file it was launched under. The profile only needs to exist through the
    /// child's `exec`, but we let it ride on the session alongside the PTY so
    /// its cleanup is tied to the process lifecycle (see the field comment).
    pub fn attach_pty(&self, pty: PtySession, profile: tempfile::NamedTempFile) {
        let mut inner = self.inner.lock();
        // Replace and drop any stale handles. (Shouldn't happen — the
        // supervisor's stop() drops them first — but defensive.)
        inner.pty = Some(pty);
        inner.profile = Some(profile);
    }

    /// Returns true if the given generation is still the current one,
    /// meaning this PTY exit corresponds to the live phase.
    pub fn is_current_generation(&self, gen: u64) -> bool {
        self.inner.lock().generation == gen
    }

    /// Mark the current phase as Stopped with an error message. Drops
    /// the PTY handle. Called on natural exits.
    pub fn mark_stopped(&self, error: Option<String>) {
        let mut inner = self.inner.lock();
        inner.pty = None;
        inner.profile = None;
        inner.phase = RunPhase::Stopped;
        inner.last_error = error;
    }
}

/// Spawn a single phase's PTY. The caller is responsible for
/// `stop()` / `begin_phase` / `transition_phase` bookkeeping and for
/// wiring up the on_output / on_exit closures (which need supervisor
/// access to chain phases).
pub fn spawn_command<F, G>(
    program: &Path,
    args: &[String],
    cwd: &Path,
    on_output: F,
    on_exit: G,
) -> Result<PtySession>
where
    F: Fn(Vec<u8>) + Send + 'static,
    G: Fn(PtyExit) + Send + 'static,
{
    PtySession::spawn(
        PtySpawn {
            program,
            args,
            cwd,
            env: &[],
            cols: 120,
            rows: 32,
        },
        on_output,
        on_exit,
    )
}

/// Resolve the shell binary to use for `-lc <cmd>` invocations.
/// Falls back to /bin/zsh which is the default on every macOS the app
/// targets.
pub fn user_shell() -> PathBuf {
    PathBuf::from(std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string()))
}

/// Build the args for running `cmd` through the user's shell as a
/// login + interactive shell. Login pulls .zprofile / .bash_profile
/// (PATH); interactive pulls .zshrc (where most users put pnpm /
/// nvm shims). Single `-c` argument means the shell exits as soon
/// as the command completes.
pub fn shell_args(cmd: &str) -> Vec<String> {
    vec!["-lic".to_string(), cmd.to_string()]
}

// ── Log buffer ───────────────────────────────────────────────────────────────

struct LogBuffer {
    bytes: Vec<u8>,
    cap: usize,
}

impl LogBuffer {
    fn new(cap: usize) -> Self {
        Self {
            bytes: Vec::new(),
            cap,
        }
    }

    fn append(&mut self, b: &[u8]) {
        self.bytes.extend_from_slice(b);
        // Compact only when we've drifted ≥50% past cap so per-byte
        // amortized work stays O(1).
        if self.bytes.len() > self.cap + self.cap / 2 {
            let drop = self.bytes.len() - self.cap;
            self.bytes.drain(..drop);
        }
    }

    fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_buffer_evicts_oldest_bytes() {
        let mut buf = LogBuffer::new(100);
        for _ in 0..20 {
            buf.append(&[b'x'; 50]);
        }
        // After many appends past cap, len stays within [cap, 1.5*cap]
        // and the most recent bytes are still present.
        assert!(buf.bytes().len() >= 100);
        assert!(buf.bytes().len() <= 150);
        assert!(buf.bytes().iter().all(|&b| b == b'x'));
    }

    #[test]
    fn phase_starts_idle() {
        let s = RunSession::new();
        assert_eq!(s.phase(), RunPhase::Idle);
        assert!(!s.is_active());
    }

    #[test]
    fn stop_bumps_generation_and_marks_stopped() {
        let s = RunSession::new();
        let g1 = s.begin_phase(RunPhase::Running);
        s.stop();
        assert_eq!(s.phase(), RunPhase::Stopped);
        assert!(!s.is_current_generation(g1));
    }

    #[test]
    fn transition_phase_keeps_generation() {
        let s = RunSession::new();
        let g1 = s.begin_phase(RunPhase::Setup);
        let g2 = s.transition_phase(RunPhase::Running);
        assert_eq!(g1, g2);
        assert!(s.is_current_generation(g1));
    }
}
