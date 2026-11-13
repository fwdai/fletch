//! Per-agent "Run" process: spawns the project's setup + dev command
//! inside the agent's checkout, streams output to the frontend, and
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
use crate::run_detect::DetectedConfig;
use crate::sandbox::ForwardedPort;

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
    /// The port on this host that reaches the dev server, once the run phase
    /// spawned: the port-safe one it was launched on, which may differ from
    /// the configured port, or for a box's guest the box's forward for
    /// `guest_port`, once the server listens there. What the panel's link
    /// opens and the sidebar shows, as the field has always meant
    /// (`run:port` carries the same value). `None` before the run phase,
    /// after a stop, and for a command with no port.
    pub port: Option<u16>,
    /// The port the server binds inside a box's guest, which the forward from
    /// `port` reaches: where the agent of a box workspace is, so what its
    /// `app_status` names. `None` for a server on the host, where `port` is
    /// that port. Set and cleared with the run's port.
    pub guest_port: Option<u16>,
    /// Every port something listens on in the box that holds the checkout,
    /// with the port on this host that reaches it: the box's, whatever
    /// started it, so kept across the run's starts and stops. Empty for a
    /// checkout on this host, where nothing needs forwarding.
    pub ports: Vec<ForwardedPort>,
    /// Which state this is: moved on by every change to the phase, the error
    /// or the ports, never back. `run:state` carries it too, so a client takes
    /// a snapshot and the events in one order — whichever of them arrives
    /// later but describes an older state is dropped — rather than let a
    /// snapshot taken while a box's forward was pending erase the port an
    /// event brought meanwhile. Every revision is at least the clock's
    /// milliseconds when the state was made — a session begins at the clock
    /// and each step is at least the clock ([`clock_revision`]) — so a host
    /// that restarts, its sessions begun anew or not there at all, still
    /// outranks what a client that stayed connected holds from before.
    pub revision: u64,
    /// Cumulative PTY output bytes (latest first dropped on overflow).
    /// Encoded as a byte array so the frontend can render or strip
    /// ANSI without re-decoding.
    pub log: Vec<u8>,
    /// Absolute end offset of `log` — the total number of bytes ever
    /// appended to this session (monotonic, uncapped, unaffected by ring
    /// eviction). Each `run:output` event carries the same running counter
    /// as its `seq`, so a panel that subscribes before fetching this snapshot
    /// can dedupe the overlap exactly at this boundary: any live chunk whose
    /// `seq <= log_seq` is already contained here.
    pub log_seq: u64,
}

/// [`RunStateSnapshot`] without the log: what `run:state` announces, read
/// under one lock so its fields agree with each other and with the snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunState {
    pub phase: RunPhase,
    pub last_error: Option<String>,
    pub port: Option<u16>,
    pub guest_port: Option<u16>,
    pub ports: Vec<ForwardedPort>,
    pub revision: u64,
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
    /// The port a server on this host binds.
    port: Option<u16>,
    guest_port: Option<u16>,
    ports: Vec<ForwardedPort>,
    log: LogBuffer,
    pty: Option<PtySession>,
    /// The run configs last detected in the checkout (`run_detect`), served
    /// to the panel while the checkout cannot be read without waking its
    /// sandbox (a stopped box). Survives stops like the log.
    detected: Option<Vec<DetectedConfig>>,
    /// Bumped on every `start` and `stop`. The PTY exit handler
    /// captures the generation it was spawned under; if that
    /// generation no longer matches, the exit is from a process the
    /// user already stopped or replaced and is ignored.
    generation: u64,
    /// Moved on by every change to the phase, the error or the ports
    /// ([`touched`](Self::touched), see [`RunStateSnapshot::revision`]); a
    /// refused mutation leaves it alone.
    revision: u64,
}

impl RunSessionInner {
    /// The state changed: move the revision on, past the clock, so a session
    /// begun after a host restart cannot hand out a revision a client already
    /// holds from the session before it.
    fn touched(&mut self) {
        self.revision = (self.revision + 1).max(clock_revision());
    }

    /// The port on this host that reaches the run's server: its own on this
    /// host, or the box's forward for the one it binds in the guest.
    fn host_port(&self) -> Option<u16> {
        match self.guest_port {
            Some(guest_port) => self
                .ports
                .iter()
                .find(|forwarded| forwarded.guest_port == guest_port)
                .map(|forwarded| forwarded.port),
            None => self.port,
        }
    }
}

/// The clock's milliseconds since the epoch: the revision of a state made
/// now by a host with nothing older to go on — a new session, or the Idle
/// state of an agent with no session, as after a restart — which outranks
/// every state an earlier incarnation of the host handed out, since those
/// were at least the clock of their own time.
pub fn clock_revision() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Default for RunSession {
    fn default() -> Self {
        Self {
            inner: Mutex::new(RunSessionInner {
                phase: RunPhase::Idle,
                last_error: None,
                port: None,
                guest_port: None,
                ports: Vec::new(),
                log: LogBuffer::new(LOG_BUFFER_CAP),
                pty: None,
                detected: None,
                generation: 0,
                revision: clock_revision(),
            }),
        }
    }
}

impl RunSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> RunStateSnapshot {
        let inner = self.inner.lock();
        RunStateSnapshot {
            phase: inner.phase,
            last_error: inner.last_error.clone(),
            port: inner.host_port(),
            guest_port: inner.guest_port,
            ports: inner.ports.clone(),
            revision: inner.revision,
            log: inner.log.bytes().to_vec(),
            log_seq: inner.log.total(),
        }
    }

    /// The session's state, as [`snapshot`](Self::snapshot) has it, without
    /// copying the log.
    pub fn state(&self) -> RunState {
        let inner = self.inner.lock();
        RunState {
            phase: inner.phase,
            last_error: inner.last_error.clone(),
            port: inner.host_port(),
            guest_port: inner.guest_port,
            ports: inner.ports.clone(),
            revision: inner.revision,
        }
    }

    pub fn phase(&self) -> RunPhase {
        self.inner.lock().phase
    }

    pub fn is_active(&self) -> bool {
        matches!(self.phase(), RunPhase::Setup | RunPhase::Running)
    }

    /// Keep `configs` as the checkout's last detection.
    pub fn remember_detected(&self, configs: &[DetectedConfig]) {
        self.inner.lock().detected = Some(configs.to_vec());
    }

    /// The checkout's last detection, if one was made.
    pub fn last_detected(&self) -> Option<Vec<DetectedConfig>> {
        self.inner.lock().detected.clone()
    }

    /// The port the run phase's server binds, recorded as the phase spawns
    /// so the agent's `app_status` can name it: `port` on this host, or
    /// `guest_port` in a box's guest, which this host reaches through the
    /// box's forward for it (see [`RunStateSnapshot`]); the panel hears the
    /// host's port on `run:port` and in each `run:state` that follows.
    /// Cleared by every start and stop. Refused, as `false`, when `gen` is no
    /// longer the current generation: a stop came while the start was still
    /// settling the port (a box boots and probes the guest before its command
    /// runs), and a stopped session must not advertise one.
    pub fn set_ports(&self, gen: u64, port: Option<u16>, guest_port: Option<u16>) -> bool {
        let mut inner = self.inner.lock();
        if inner.generation != gen {
            return false;
        }
        inner.port = port;
        inner.guest_port = guest_port;
        inner.touched();
        true
    }

    /// The ports the box that holds the checkout forwards to this host now
    /// (`sandbox::on_forwarded_ports`). `false`, and nothing moves, when they
    /// are the ones the session holds.
    pub fn set_forwarded(&self, ports: &[ForwardedPort]) -> bool {
        let mut inner = self.inner.lock();
        if inner.ports == ports {
            return false;
        }
        inner.ports = ports.to_vec();
        inner.touched();
        true
    }

    /// Append PTY output to the rolling buffer. Called from the per-spawn
    /// output callback. Returns the new absolute end offset (total bytes ever
    /// appended) so the caller can stamp the emitted `run:output` event with a
    /// `seq` the panel dedupes against `RunStateSnapshot::log_seq`.
    pub fn append_log(&self, bytes: &[u8]) -> u64 {
        let mut inner = self.inner.lock();
        inner.log.append(bytes);
        inner.log.total()
    }

    /// Kill the active PTY (if any), bump generation so any in-flight
    /// exit callback bails, and transition to Stopped. The on_exit
    /// from the killed PTY is ignored due to the generation bump.
    /// Returns the prior phase so callers can decide whether to emit
    /// state change.
    pub fn stop(&self) -> RunPhase {
        // Take the PTY and update state under the lock, but run the (blocking)
        // group kill after releasing it — a stubborn child mustn't stall
        // snapshot()/phase() reads from the Run panel.
        let (prior, pty) = {
            let mut inner = self.inner.lock();
            let prior = inner.phase;
            let pty = inner.pty.take();
            inner.generation = inner.generation.wrapping_add(1);
            inner.phase = RunPhase::Stopped;
            inner.last_error = None;
            inner.port = None;
            inner.guest_port = None;
            inner.touched();
            (prior, pty)
        };
        if let Some(pty) = pty {
            let _ = pty.kill();
        }
        prior
    }

    /// Begin a fresh start sequence: claim the session for the start that
    /// observed generation `expected` ([`generation`](Self::generation)).
    /// Under one lock, the claim is refused — `None`, and nothing changes —
    /// when another start claimed it since (the generation moved on, or a
    /// phase is active) or a stop came in between: two ▶ clicks while a
    /// stopped box boots both reach here, and only the first may spawn, or
    /// the second's PTY would replace and kill the first's command. Granted,
    /// it bumps the generation (so any in-flight callbacks from a previous
    /// start are now stale), sets the initial phase, clears `last_error` and
    /// the ports, and returns the new generation. Use `transition_phase` for
    /// chained transitions inside the same sequence (setup → run) — those
    /// keep the generation stable.
    pub fn begin_phase(&self, expected: u64, phase: RunPhase) -> Option<u64> {
        let mut inner = self.inner.lock();
        if inner.generation != expected
            || matches!(inner.phase, RunPhase::Setup | RunPhase::Running)
        {
            return None;
        }
        inner.generation = inner.generation.wrapping_add(1);
        inner.phase = phase;
        inner.last_error = None;
        inner.port = None;
        inner.guest_port = None;
        inner.touched();
        Some(inner.generation)
    }

    /// The current generation, whatever the phase.
    pub fn generation(&self) -> u64 {
        self.inner.lock().generation
    }

    /// The generation of an idle session — `None` while a phase is active —
    /// as one reading under one lock: what a start observes before
    /// [`begin_phase`](Self::begin_phase) renews it. Read apart, "idle" and
    /// the generation could straddle another run: one that began and ended
    /// on its own in between leaves the session idle at a newer generation,
    /// which a claim made with it would take, running the command twice.
    /// The same token lets a failure on the way to the claim (the checkout
    /// unreadable, no free port) tell whether a newer start has begun.
    pub fn idle_generation(&self) -> Option<u64> {
        let inner = self.inner.lock();
        (!matches!(inner.phase, RunPhase::Setup | RunPhase::Running)).then_some(inner.generation)
    }

    /// Like begin_phase but keeps the same generation. Used when one
    /// phase ends successfully and we chain into the next (setup→run).
    /// Refused, as `false`, when `gen` is no longer current: the user
    /// stopped or restarted the run while the first phase was ending.
    pub fn transition_phase(&self, gen: u64, phase: RunPhase) -> bool {
        let mut inner = self.inner.lock();
        if inner.generation != gen {
            return false;
        }
        inner.phase = phase;
        inner.last_error = None;
        inner.touched();
        true
    }

    /// Attach a freshly spawned PTY, if `gen` is still the current generation
    /// and the phase still active; else kill it and answer `false`. The start
    /// that spawned it runs off the UI thread, so a stop can land while a box
    /// boots, probes its port and launches the command: `stop` has then
    /// bumped the generation and found no PTY to kill, so the one arriving
    /// here is ended in its place — its exit hook sees the stale generation
    /// and changes nothing. One lock covers the check and the attach, so a
    /// stop either finds the PTY or is seen by this. The `sandbox-exec`
    /// profile it was launched under travels in argv, so there is no profile
    /// resource to park here.
    pub fn attach_pty(&self, pty: PtySession, gen: u64) -> bool {
        {
            let mut inner = self.inner.lock();
            if inner.generation == gen && matches!(inner.phase, RunPhase::Setup | RunPhase::Running)
            {
                inner.pty = Some(pty);
                return true;
            }
        }
        // Killed outside the lock, as `stop` does: a box's kill ends the
        // command in the guest, which is slow.
        let _ = pty.kill();
        false
    }

    /// Returns true if the given generation is still the current one,
    /// meaning this PTY exit corresponds to the live phase.
    pub fn is_current_generation(&self, gen: u64) -> bool {
        self.inner.lock().generation == gen
    }

    /// Mark the current phase as Stopped with an error message. Drops
    /// the PTY handle. Called on natural exits and start failures. Refused,
    /// as `false`, when `gen` is no longer current: the run this outcome
    /// belongs to was stopped, and a newer one may be under way — its state,
    /// port and PTY are not the old run's to take down.
    pub fn mark_stopped(&self, gen: u64, error: Option<String>) -> bool {
        let mut inner = self.inner.lock();
        if inner.generation != gen {
            return false;
        }
        inner.pty = None;
        inner.phase = RunPhase::Stopped;
        inner.last_error = error;
        inner.port = None;
        inner.guest_port = None;
        inner.touched();
        true
    }
}

/// Spawn a single phase's PTY. The caller is responsible for
/// `stop()` / `begin_phase` / `transition_phase` bookkeeping and for
/// wiring up the on_output / on_exit closures (which need supervisor
/// access to chain phases). `kill` is how the child is ended beyond its
/// own process group: the sandbox's own teardown for a command running in
/// a box's guest, nothing more for one on the host.
pub fn spawn_command<F, G>(
    program: &Path,
    args: &[String],
    cwd: &Path,
    env: &[(String, String)],
    kill: crate::sandbox::KillHandle,
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
            env,
            cols: 120,
            rows: 32,
            env_remove: &[],
            kill_plan: kill,
            after_exit: None,
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
///
/// `set +m` disables job control. Interactive shells otherwise run each job
/// in its own process group, so the dev command and anything it backgrounds
/// escape the shell's group — and stop() could only reap the group, leaving
/// them orphaned on the port. With job control off the whole tree stays in
/// the shell's (session-leader) group, so one killpg takes it all down.
pub fn shell_args(cmd: &str) -> Vec<String> {
    vec!["-lic".to_string(), format!("set +m; {cmd}")]
}

// ── Log buffer ───────────────────────────────────────────────────────────────

struct LogBuffer {
    bytes: Vec<u8>,
    cap: usize,
    /// Monotonic count of every byte ever appended, independent of ring
    /// eviction. Serves as the absolute offset frontends align against.
    total: u64,
}

impl LogBuffer {
    fn new(cap: usize) -> Self {
        Self {
            bytes: Vec::new(),
            cap,
            total: 0,
        }
    }

    fn total(&self) -> u64 {
        self.total
    }

    fn append(&mut self, b: &[u8]) {
        self.total += b.len() as u64;
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
    fn log_buffer_total_counts_all_bytes_past_eviction() {
        let mut buf = LogBuffer::new(100);
        assert_eq!(buf.total(), 0);
        for _ in 0..20 {
            buf.append(&[b'x'; 50]);
        }
        // total is monotonic and uncapped even though the ring evicts: it is
        // the absolute offset the panel dedupes against.
        assert_eq!(buf.total(), 20 * 50);
    }

    #[test]
    fn append_log_returns_running_offset_and_snapshot_matches() {
        let s = RunSession::new();
        assert_eq!(s.append_log(b"hello"), 5);
        assert_eq!(s.append_log(b" world"), 11);
        let snap = s.snapshot();
        // The snapshot's end offset equals the last append's returned seq, so a
        // `run:output` event stamped with that seq is recognized as contained.
        assert_eq!(snap.log_seq, 11);
        assert_eq!(snap.log, b"hello world");
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
        let g1 = s.begin_phase(s.generation(), RunPhase::Running).unwrap();
        s.stop();
        assert_eq!(s.phase(), RunPhase::Stopped);
        assert!(!s.is_current_generation(g1));
    }

    fn forwarded(pairs: &[(u16, u16)]) -> Vec<ForwardedPort> {
        pairs
            .iter()
            .map(|&(guest_port, port)| ForwardedPort { guest_port, port })
            .collect()
    }

    #[test]
    fn the_port_lives_with_the_running_phase() {
        let s = RunSession::new();
        assert_eq!(s.snapshot().port, None);
        let gen = s.begin_phase(s.generation(), RunPhase::Running).unwrap();
        assert!(s.set_ports(gen, None, Some(3000)));
        assert!(s.set_forwarded(&forwarded(&[(3000, 3001)])));
        assert_eq!(s.snapshot().port, Some(3001));
        assert_eq!(s.snapshot().guest_port, Some(3000));
        s.stop();
        assert_eq!(s.snapshot().port, None, "a stopped run binds no port");
        assert_eq!(s.snapshot().guest_port, None);
        // A start still settling its port when the stop came is refused.
        assert!(!s.set_ports(gen, None, Some(3000)));
        assert_eq!(s.snapshot().port, None, "a stale start sets no port");
        let gen = s.begin_phase(s.generation(), RunPhase::Setup).unwrap();
        assert!(s.set_ports(gen, Some(3001), None));
        assert!(s.mark_stopped(gen, Some("crashed".into())));
        assert_eq!(s.snapshot().port, None);
        assert_eq!(s.snapshot().guest_port, None);
        s.begin_phase(s.generation(), RunPhase::Running).unwrap();
        assert_eq!(
            s.snapshot().port,
            None,
            "a fresh start forgets the old port"
        );
        assert_eq!(s.snapshot().guest_port, None);
    }

    /// The ports a box forwards are the box's, whatever listens on them: each
    /// change moves the revision on and a repeat does not, the run's starts
    /// and stops leave them, and the run's own port on this host is the
    /// forward for the port it binds in the guest, once the server listens.
    #[test]
    fn the_boxs_forwarded_ports_ride_the_run_state() {
        let s = RunSession::new();
        let fresh = s.state().revision;
        assert!(s.set_forwarded(&forwarded(&[(5432, 5432)])));
        let with_db = s.state().revision;
        assert!(with_db > fresh);
        assert!(!s.set_forwarded(&forwarded(&[(5432, 5432)])));
        assert_eq!(s.state().revision, with_db, "a repeat moves nothing");

        let gen = s.begin_phase(s.generation(), RunPhase::Running).unwrap();
        assert!(s.set_ports(gen, None, Some(3000)));
        let state = s.state();
        assert_eq!(state.port, None, "the server does not listen yet");
        assert_eq!(state.guest_port, Some(3000));
        assert_eq!(
            state.ports,
            forwarded(&[(5432, 5432)]),
            "a start keeps them"
        );
        assert!(s.set_forwarded(&forwarded(&[(3000, 3001), (5432, 5432)])));
        let snap = s.snapshot();
        assert_eq!(snap.port, Some(3001));
        assert_eq!(snap.ports, forwarded(&[(3000, 3001), (5432, 5432)]));
        assert!(snap.revision > state.revision);

        s.stop();
        let state = s.state();
        assert_eq!(state.port, None);
        assert_eq!(state.ports.len(), 2, "the box's ports outlive the run");
        assert!(s.set_forwarded(&[]), "the box stopped");
        assert!(s.snapshot().ports.is_empty());
    }

    /// A stop that lands while a start is still launching its command finds
    /// no PTY to kill; the PTY arriving afterwards is killed on attach, and
    /// its exit hook sees the stale generation.
    #[test]
    fn a_pty_attached_after_a_stop_is_killed() {
        use std::sync::mpsc;
        use std::time::Duration;
        let td = tempfile::tempdir().unwrap();
        let s = RunSession::new();
        let gen = s.begin_phase(s.generation(), RunPhase::Running).unwrap();
        let (exit_tx, exit_rx) = mpsc::channel();
        let pty = spawn_command(
            Path::new("/bin/sh"),
            &["-c".to_string(), "sleep 30".to_string()],
            td.path(),
            &[],
            crate::sandbox::KillHandle::ProcessGroup,
            |_| {},
            move |exit| {
                let _ = exit_tx.send(exit);
            },
        )
        .unwrap();
        assert_eq!(s.stop(), RunPhase::Running, "nothing was attached yet");
        assert!(!s.attach_pty(pty, gen), "the stop came first");
        assert!(!s.is_current_generation(gen));
        exit_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the late PTY is killed on attach");
        assert_eq!(s.snapshot().phase, RunPhase::Stopped);

        // The ordinary case: current generation, the PTY is kept.
        let gen = s.begin_phase(s.generation(), RunPhase::Running).unwrap();
        let pty = spawn_command(
            Path::new("/bin/sh"),
            &["-c".to_string(), "sleep 30".to_string()],
            td.path(),
            &[],
            crate::sandbox::KillHandle::ProcessGroup,
            |_| {},
            |_| {},
        )
        .unwrap();
        assert!(s.attach_pty(pty, gen));
        s.stop();
    }

    #[test]
    fn transition_phase_keeps_generation() {
        let s = RunSession::new();
        let g1 = s.begin_phase(s.generation(), RunPhase::Setup).unwrap();
        assert!(s.transition_phase(g1, RunPhase::Running));
        assert_eq!(s.phase(), RunPhase::Running);
        assert!(s.is_current_generation(g1));
        s.stop();
        assert!(
            !s.transition_phase(g1, RunPhase::Running),
            "a chain from a stopped run goes nowhere"
        );
        assert_eq!(s.phase(), RunPhase::Stopped);
    }

    /// Two ▶ clicks while a stopped box boots: both starts read the session
    /// idle and the same generation, both reach the claim, and only the
    /// first gets it. A Stop between the read and the claim refuses the
    /// start too, and a fresh read claims again.
    #[test]
    fn only_one_of_two_concurrent_starts_claims_the_run() {
        let s = RunSession::new();
        let observed = s.generation();
        let first = s.begin_phase(observed, RunPhase::Setup).unwrap();
        assert_eq!(
            s.begin_phase(observed, RunPhase::Setup),
            None,
            "the second start saw the same idle session"
        );
        assert!(
            s.is_current_generation(first),
            "the first start keeps the run"
        );
        assert_eq!(s.phase(), RunPhase::Setup);
        // Even a start that read the session after the claim is refused
        // while a phase is active.
        assert_eq!(s.begin_phase(s.generation(), RunPhase::Running), None);

        s.stop();
        let observed = s.generation();
        s.stop();
        assert_eq!(
            s.begin_phase(observed, RunPhase::Running),
            None,
            "a stop between the read and the claim wins"
        );
        assert!(s.begin_phase(s.generation(), RunPhase::Running).is_some());
    }

    /// Every change to what `run:state` announces moves the revision on, and
    /// a refused change does not: a state read before a later change is the
    /// older of the two by its revision, however late it is delivered.
    #[test]
    fn the_revision_orders_every_state_change() {
        let before = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let s = RunSession::new();
        let fresh = s.snapshot().revision;
        assert!(
            fresh >= before,
            "a new session begins at the clock: {fresh} < {before}"
        );
        let gen = s.begin_phase(0, RunPhase::Setup).unwrap();
        let begun = s.state().revision;
        assert!(begun > fresh);
        assert!(s.transition_phase(gen, RunPhase::Running));
        let transitioned = s.state().revision;
        assert!(transitioned > begun);
        assert!(s.set_ports(gen, None, Some(3000)));
        let with_port = s.state().revision;
        assert!(with_port > transitioned);
        assert_eq!(
            s.snapshot().revision,
            with_port,
            "the snapshot says the same"
        );
        // The stop that lands after a start read its state: the start's
        // reading is older by its revision, so a client that already saw the
        // stop drops it, port and all.
        s.stop();
        let stopped = s.state().revision;
        assert!(stopped > with_port);
        assert!(!s.set_ports(gen, Some(3001), None), "stale, refused");
        assert!(!s.mark_stopped(gen, None), "stale, refused");
        assert_eq!(
            s.state().revision,
            stopped,
            "a refused change moves nothing"
        );
        let gen = s
            .begin_phase(s.idle_generation().unwrap(), RunPhase::Running)
            .unwrap();
        assert!(s.mark_stopped(gen, Some("Run exited: 1".into())));
        assert!(s.state().revision > stopped);
        // A session begun anew (a host restart) starts past what the old one
        // handed out, since each step is at least the clock.
        let fresh = RunSession::new();
        fresh.begin_phase(0, RunPhase::Running).unwrap();
        assert!(fresh.state().revision >= s.state().revision - 6);
    }

    /// A start that observed the session idle, then lost its turn while
    /// another run began and ended on its own, finds the session idle again
    /// at a newer generation: its claim is refused all the same, or two
    /// clicks would run a short command twice.
    #[test]
    fn a_run_that_came_and_went_in_between_refuses_the_older_observation() {
        let s = RunSession::new();
        let second = s.idle_generation().unwrap();
        let first = s.begin_phase(second, RunPhase::Setup).unwrap();
        assert_eq!(s.idle_generation(), None, "an active session is not idle");
        assert!(s.mark_stopped(first, None), "the first run ends on its own");
        assert_eq!(s.idle_generation(), Some(first), "idle again, newer");
        assert_eq!(
            s.begin_phase(second, RunPhase::Setup),
            None,
            "the observation predates the run that came and went"
        );
        assert_eq!(s.phase(), RunPhase::Stopped);
        assert!(s
            .begin_phase(s.idle_generation().unwrap(), RunPhase::Setup)
            .is_some());
    }

    /// Stop, then Start again at once: the first start is still under way
    /// off the UI thread, and when it fails later its outcome belongs to the
    /// run that was stopped, not to the one now running.
    #[test]
    fn a_stale_starts_failure_does_not_stop_the_newer_run() {
        let s = RunSession::new();
        let old = s.begin_phase(s.generation(), RunPhase::Running).unwrap();
        s.stop();
        let new = s.begin_phase(s.generation(), RunPhase::Running).unwrap();
        assert!(s.set_ports(new, Some(3000), None));
        assert!(!s.mark_stopped(old, Some("Failed to start command: box gone".into())));
        let snap = s.snapshot();
        assert_eq!(snap.phase, RunPhase::Running);
        assert_eq!(snap.last_error, None);
        assert_eq!(snap.port, Some(3000));
        // A failure observed before `begin_phase` is refused the same way.
        assert!(!s.mark_stopped(s.generation().wrapping_sub(1), None));
        assert!(s.mark_stopped(new, Some("Run exited: 1".into())));
        assert_eq!(s.snapshot().phase, RunPhase::Stopped);
    }
}
