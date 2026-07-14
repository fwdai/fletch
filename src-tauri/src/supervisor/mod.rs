//! Coordinator between Tauri IPC commands and the running agents.

mod disposition;
mod events;
mod fork;
mod lifecycle;
mod messaging;
mod rpc_watch;
pub(crate) mod run;
mod session_sync;
mod shell;

pub use fork::{ForkCode, ForkContext};
pub use lifecycle::SpawnRequest;
pub use run::ProjectRunConfig;
pub(crate) use session_sync::{persist_pr_snapshot, resolve_all_pr_states, resolve_pr_state};

use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::AppHandle;
use tokio::sync::broadcast;

use crate::activity::Activity;
use crate::agent::Agent;
use crate::error::{Error, Result};
use crate::message_queue::MessageQueue;
use crate::native_input::NativeInputTracker;
use crate::pty_session::PtySession;
use crate::run_session::RunSession;
use crate::workspace::{AgentRecord, AgentStatus, ClosedTurn, Workspace, WorkspaceManager};

use events::emit_status;
use messaging::{drain_message_queue, drain_pending_bin_respawn};

/// A runtime status transition, broadcast to in-process subscribers the moment
/// it is recorded. The workflow scheduler (`workflow::driver`) subscribes to
/// this channel so it can catch an arbitrarily fast `Running → Idle` flap
/// without polling: the Tauri `agent:status` event is the renderer's copy of
/// the same signal, but a broadcast receiver lets Rust code await transitions
/// deterministically. Distinct from the DB-persisted disposition — this is the
/// live in-memory value at the instant it changed.
#[derive(Debug, Clone)]
pub struct StatusEvent {
    pub agent_id: String,
    pub status: AgentStatus,
}

pub struct Supervisor {
    pub workspace: Arc<WorkspaceManager>,
    /// Live agent handles, keyed by agent id. INVARIANT: never hold this guard
    /// across any call into an `Agent` that touches its child — stdin/PTY
    /// writes, `interrupt`, `resize`, `shutdown`/`kill`. Those can block on a
    /// child that stopped draining its pipe, and this is one global lock, so
    /// blocking under it stalls every agent's I/O app-wide. Clone the `Arc` out
    /// via `live_agent` (or bind the value out of a `remove`) and drop the lock
    /// *first*; the sessions carry their own per-agent I/O mutexes, so the map
    /// lock only needs to protect membership. The `Arc` lets a clone outlive the
    /// guard so the actual I/O runs unlocked.
    pub agents: Mutex<HashMap<String, Arc<Agent>>>,
    pub generations: Mutex<HashMap<String, u64>>,
    pub activities: Mutex<HashMap<String, Box<dyn Activity>>>,
    /// In-memory source of truth for live runtime status
    /// (Spawning/Running/Idle). The DB only persists durable
    /// dispositions, so a resting record loaded from it derives `Idle`;
    /// this map carries the real current status while an agent is live.
    pub statuses: Mutex<HashMap<String, AgentStatus>>,
    /// Per-agent native-view input trackers, reconstructing submitted lines
    /// from raw keystroke bytes (see `native_input`).
    pub native_inputs: Mutex<HashMap<String, NativeInputTracker>>,
    pub shells: Mutex<HashMap<String, PtySession>>,
    /// Per-agent run-panel processes (dev server + setup). Reused
    /// across start/stop cycles so the log buffer survives.
    pub runs: Mutex<HashMap<String, Arc<RunSession>>>,
    /// Agent ids whose binary-path change couldn't be applied immediately
    /// because the agent was mid-turn. Drained at the next turn-end Idle
    /// transition (see `transition_active`), which respawns them onto the
    /// new binary. Empty in the common case.
    pub respawn_pending: Mutex<HashSet<String>>,
    /// Follow-up messages sent while a turn is in progress, awaiting delivery
    /// at the next turn boundary (per-turn agents, or claude paused on a tool
    /// gate). In-memory only — see `message_queue`.
    pub message_queue: Mutex<MessageQueue>,
    /// Agent ids whose current turn was stopped by the user. The dying process
    /// still converges on the Idle transition, so this flag lets the turn-end
    /// flush distinguish a natural completion (flush queued follow-ups) from a
    /// stop (keep them queued, don't auto-send — see `drain_message_queue`).
    pub interrupted: Mutex<HashSet<String>>,
    /// Last-emitted transcript-ingest health per agent, so `session:sync-health`
    /// fires on *change* only (real drift would otherwise spam an event every
    /// turn). Absent = healthy/never-degraded. In-memory only, event-driven —
    /// never persisted (see `session_sync`). Behind an `Arc` so the fire-and-
    /// forget sync task can hold it without borrowing `self`.
    pub sync_health: Arc<Mutex<HashMap<String, session_sync::SyncHealth>>>,
    /// Per-agent RPC dispatchers, so the mailbox can be drained on demand
    /// (`settle_agent_rpc`) in addition to the polling watcher — the scheduler
    /// drains a step agent's mailbox at turn end so a `wf_ask` is dispatched
    /// before it acts on the gate (§10.4). Overwritten on each (re)spawn; removed
    /// on teardown.
    pub rpc_dispatchers: Mutex<HashMap<String, Arc<dyn crate::rpc::RpcDispatcher>>>,
    /// Fan-out of every runtime status transition (see [`StatusEvent`]). Held
    /// as the sender; subscribers call [`Supervisor::subscribe_status`]. The
    /// supervisor never reads it, so a dropped-receiver `send` error is ignored.
    status_tx: broadcast::Sender<StatusEvent>,
}

impl Supervisor {
    pub fn new(workspace: Arc<WorkspaceManager>) -> Self {
        Self {
            workspace,
            agents: Mutex::new(HashMap::new()),
            generations: Mutex::new(HashMap::new()),
            activities: Mutex::new(HashMap::new()),
            statuses: Mutex::new(HashMap::new()),
            native_inputs: Mutex::new(HashMap::new()),
            shells: Mutex::new(HashMap::new()),
            runs: Mutex::new(HashMap::new()),
            respawn_pending: Mutex::new(HashSet::new()),
            message_queue: Mutex::new(MessageQueue::new()),
            interrupted: Mutex::new(HashSet::new()),
            sync_health: Arc::new(Mutex::new(HashMap::new())),
            rpc_dispatchers: Mutex::new(HashMap::new()),
            // Capacity is generous: a lagging subscriber gets `Lagged` and
            // re-reads `status_of`, so overflow degrades to a resync, never a
            // lost terminal state. 1024 covers bursts across many live agents.
            status_tx: broadcast::channel(1024).0,
        }
    }

    /// Drain `agent_id`'s RPC mailbox once, synchronously, dispatching any
    /// requests it has already written (e.g. a `wf_ask` from the turn that just
    /// ended) before the caller acts on that turn's result. The per-agent watcher
    /// also processes on its tick; `process_pending` is idempotent and
    /// in-flight-guarded, so the two never double-dispatch. A no-op for an agent
    /// with no registered dispatcher.
    pub async fn settle_agent_rpc(self: &Arc<Self>, app: &tauri::AppHandle, agent_id: &str) {
        let dispatcher = self.rpc_dispatchers.lock().get(agent_id).cloned();
        let Some(dispatcher) = dispatcher else {
            return;
        };
        let Ok(rpc_dir) = crate::rpc::mailbox_dir(agent_id) else {
            return;
        };
        rpc_watch::process_agent_rpc_once(self, app, agent_id, dispatcher.as_ref(), &rpc_dir).await;
    }

    /// Subscribe to runtime status transitions across all agents. To avoid a
    /// race the caller MUST subscribe *first*, then read [`Supervisor::status_of`],
    /// then loop on `recv()` — the subscribe-before-read discipline is what makes
    /// a fast `Running → Idle` flap unlosable (see `workflow::attempt`).
    pub fn subscribe_status(&self) -> broadcast::Receiver<StatusEvent> {
        self.status_tx.subscribe()
    }

    /// The authoritative current status of an agent: the live in-memory value
    /// while it is tracked, else the DB-derived resting status on its record,
    /// else `None` once the agent no longer exists.
    pub fn status_of(&self, agent_id: &str) -> Option<AgentStatus> {
        self.live_status(agent_id)
            .or_else(|| self.workspace.agent(agent_id).ok().map(|r| r.status))
    }

    /// Ingest timestamp (ms) of this agent's most recent session record — the
    /// stall-detection clock. `None` before the first record lands.
    pub fn last_activity(&self, agent_id: &str) -> Option<i64> {
        self.workspace.last_activity(agent_id)
    }

    /// This agent's canonical session records (seq order) — the raw provider
    /// bodies as ingested. The workflow budget ledger reads token `usage` out of
    /// them (spec §11.2); a read error degrades to an empty slice (tokens then
    /// go uncounted, exactly as for a provider that exposes no usage).
    pub fn read_session_records(&self, agent_id: &str) -> Vec<crate::workspace::SessionRecord> {
        self.workspace
            .read_session_records(agent_id)
            .unwrap_or_default()
    }

    /// Invalidate this agent's current spawn generation. Any gen-guarded
    /// background task (turn watchdog, RPC watcher) and any late process-exit
    /// handler captured the old number, so after this bump they see
    /// `current_gen != gen` and exit / no-op cleanly.
    ///
    /// `start_process` bumps on its own when it restarts the process; teardown
    /// paths that DON'T restart (archive, discard, spawn-timeout kill, spawn
    /// abort) must call this, or their loops spin for the app's lifetime and a
    /// late clean exit re-emits `Idle` for an already-gone agent (ghost entry).
    fn bump_generation(&self, agent_id: &str) {
        let mut g = self.generations.lock();
        *g.entry(agent_id.to_string()).or_insert(0) += 1;
    }

    /// Clone the live agent's handle under a short-lived `agents` lock.
    /// The only thing callers may do under the `agents` lock is this clone —
    /// all child I/O (stdin writes, PTY writes, interrupt, shutdown) must
    /// happen on the returned handle with the lock released (see the `agents`
    /// field doc). Returns `AgentNotFound` when the agent isn't live.
    pub(super) fn live_agent(&self, agent_id: &str) -> Result<Arc<Agent>> {
        self.agents
            .lock()
            .get(agent_id)
            .cloned()
            .ok_or_else(|| Error::AgentNotFound(agent_id.to_string()))
    }

    /// Kill and reap every live child process the supervisor is tracking:
    /// agent sessions (claude/codex/sandbox-exec), interactive shells, and
    /// run-panel dev servers (which hold ports). Called from the app's
    /// `ExitRequested` handler on quit.
    ///
    /// We can't rely on the per-session `Drop` impls firing here: the
    /// supervisor lives in tauri-managed state, which isn't reliably dropped
    /// when the macOS app terminates, so without this the children outlive
    /// the app. We take each map by value (releasing the lock immediately)
    /// and let the owned sessions drop, which runs their kill+reap and, for
    /// PTYs, closes the master fd (SIGHUP to the foreground group).
    pub fn shutdown(&self) {
        // Dev servers first — stopping them releases the ports they hold.
        let runs = std::mem::take(&mut *self.runs.lock());
        for run in runs.values() {
            run.stop();
        }
        drop(runs);

        // Interactive shells: PtySession::drop kills the child.
        drop(std::mem::take(&mut *self.shells.lock()));

        // Agent sessions: `Agent::shutdown` consumes and drops, killing the
        // managed/pty/per-turn child. Bump each generation *first* so the
        // kill-induced process exit is recognized as our own intentional
        // teardown and ignored by `apply_exit_if_current` (stale gen), rather
        // than recorded as a crash — the same guard the timeout/resume teardown
        // paths use. Without it a docker agent's `docker run` exits 143 (SIGTERM
        // from container teardown), which persists a `last_error` and makes the
        // agent derive to `Error` on the next launch, forcing a manual Resume;
        // seatbelt happens to exit 0 on stdin close and slips past. Bumping here
        // makes both engines resume silently.
        let agents = std::mem::take(&mut *self.agents.lock());
        for (id, agent) in agents {
            self.bump_generation(&id);
            let _ = agent.shutdown();
        }
    }

    /// Record + broadcast a runtime status transition. The in-memory map is
    /// the source of truth for live status (Spawning/Running/Idle); the DB
    /// only persists durable dispositions (last_error via update_agent_status).
    fn set_status(
        &self,
        app: &AppHandle,
        agent_id: &str,
        status: AgentStatus,
        last_error: Option<String>,
    ) {
        let prev = self
            .statuses
            .lock()
            .insert(agent_id.to_string(), status.clone());
        tracing::debug!(
            agent_id = %agent_id,
            from = ?prev,
            to = ?status,
            "agent status transition"
        );
        self.persist_and_emit_status(app, agent_id, status, last_error);
    }

    /// Atomically flip the live status out of `Spawning` to `to`, returning
    /// `true` iff this call performed the swap. The spawn task and the spawn
    /// timeout both race to finish a `Spawning` agent; whoever flips it first
    /// "wins" and owns the outcome. The check-and-swap happens under a single
    /// lock so the two can never both succeed — losing the race is precisely
    /// how each side learns the other already resolved the spawn.
    fn claim_spawn_outcome(
        &self,
        app: &AppHandle,
        agent_id: &str,
        to: AgentStatus,
        last_error: Option<String>,
    ) -> bool {
        {
            let mut statuses = self.statuses.lock();
            if !matches!(statuses.get(agent_id), Some(AgentStatus::Spawning)) {
                return false;
            }
            statuses.insert(agent_id.to_string(), to.clone());
        }
        self.persist_and_emit_status(app, agent_id, to, last_error);
        true
    }

    /// Durable side-effects of a status change: persist to the DB where the
    /// status warrants it, then emit to the frontend. Split out of
    /// `set_status` so `claim_spawn_outcome` can reuse it after writing the
    /// status map under the lock.
    fn persist_and_emit_status(
        &self,
        app: &AppHandle,
        agent_id: &str,
        status: AgentStatus,
        last_error: Option<String>,
    ) {
        // Persist durable side-effects: Error stores last_error; Spawning/Running
        // clear stale stopped/error; Idle persists nothing.
        match status {
            AgentStatus::Error => {
                let _ = self.workspace.update_agent_status(
                    agent_id,
                    AgentStatus::Error,
                    last_error.clone(),
                );
            }
            AgentStatus::Spawning | AgentStatus::Running => {
                let _ = self
                    .workspace
                    .update_agent_status(agent_id, status.clone(), None);
            }
            _ => {}
        }
        // Close the in-flight turn's timer on any terminal state. Idle covers
        // the in-band turn-end and clean process exit; Error covers a crash.
        // A closed turn (started, not native, not the resting Idle at spawn)
        // yields stats we report as `turn_completed`.
        if matches!(status, AgentStatus::Idle | AgentStatus::Error) {
            match self.workspace.mark_user_turn_ended(agent_id) {
                Ok(Some(turn)) => self.track_turn_completed(agent_id, &status, turn),
                Ok(None) => {}
                Err(e) => tracing::warn!(error = %e, agent_id, "stamp user turn end failed"),
            }
        }
        // Fan the transition out to in-process subscribers (the workflow
        // scheduler) before the Tauri emit. `send` errs only when there are no
        // receivers, which is the common case for user-spawned agents — ignore.
        let _ = self.status_tx.send(StatusEvent {
            agent_id: agent_id.to_string(),
            status: status.clone(),
        });
        emit_status(app, agent_id, status, last_error);
    }

    /// Report a completed turn to product telemetry: usage-weighted provider
    /// mix (unlike `agent_spawned`, which only counts starts) plus turn shape
    /// (duration, size, whether it errored). All properties are categorical or
    /// numeric aggregates — no prompt content. No-op unless consent is on.
    fn track_turn_completed(&self, agent_id: &str, status: &AgentStatus, turn: ClosedTurn) {
        let (provider, model) = self
            .workspace
            .agent(agent_id)
            .map(|r| (r.provider, r.model))
            .unwrap_or_else(|_| ("unknown".into(), None));
        crate::telemetry::track(
            "turn_completed",
            serde_json::json!({
                "provider": provider,
                "model": model,
                "duration_ms": turn.duration_ms,
                "record_count": turn.record_count,
                "errored": matches!(status, AgentStatus::Error),
            }),
        );
    }

    /// The live (in-memory) runtime status, if the supervisor is tracking
    /// this agent. `None` once the agent is gone (exited / archived).
    fn live_status(&self, agent_id: &str) -> Option<AgentStatus> {
        self.statuses.lock().get(agent_id).cloned()
    }

    /// Whether the agent is mid-turn (spawning or running), i.e. a new message
    /// can't start a fresh turn right now.
    fn is_busy(&self, agent_id: &str) -> bool {
        matches!(
            self.live_status(agent_id),
            Some(AgentStatus::Spawning | AgentStatus::Running)
        )
    }

    /// The status to report for an agent: the live in-memory value when
    /// present, otherwise the DB-derived at-rest status on the record.
    fn effective_status(&self, agent_id: &str, record: &AgentRecord) -> AgentStatus {
        self.live_status(agent_id)
            .unwrap_or_else(|| record.status.clone())
    }

    pub fn current_workspace(&self) -> Option<Workspace> {
        let mut ws = self.workspace.current()?;
        // The DB-derived `status` rests at `Idle`; overlay the supervisor's
        // in-memory runtime status so the snapshot reflects the real
        // Spawning/Running/Idle state for any agent that's currently live.
        for record in &mut ws.agents {
            record.status = self.effective_status(&record.id, record);
        }
        Some(ws)
    }

    /// A workflow run's step agents (live + archived), with the supervisor's
    /// in-memory runtime status overlaid — the run monitor renders each
    /// attempt's chat from these records.
    pub fn run_agents(&self, run_id: &str) -> Vec<AgentRecord> {
        let mut agents = self.workspace.agents_for_run(run_id);
        for record in &mut agents {
            record.status = self.effective_status(&record.id, record);
        }
        agents
    }

    pub fn add_workspace_repo(&self, repo_path: PathBuf) -> Result<Workspace> {
        self.workspace.add_workspace_repo(repo_path)
    }

    pub fn remove_workspace_repo(&self, repo_path: PathBuf) -> Result<Workspace> {
        self.workspace.remove_workspace_repo(&repo_path)
    }

    pub fn rename_project(&self, project_id: &str, name: &str) -> Result<Workspace> {
        self.workspace.rename_project(project_id, name)
    }

    pub fn relocate_repo(&self, old_path: PathBuf, new_path: PathBuf) -> Result<Workspace> {
        self.workspace.relocate_repo(&old_path, &new_path)
    }
}

fn transition_active(sup: &Supervisor, app: &AppHandle, agent_id: &str, new: AgentStatus) {
    // Operate on the live in-memory status. A live agent with no entry yet
    // is treated as Spawning (the at-rest derivation).
    let cur = sup.live_status(agent_id).unwrap_or(AgentStatus::Spawning);
    let should_change = matches!(
        cur,
        AgentStatus::Spawning | AgentStatus::Running | AgentStatus::Idle
    ) && cur != new;
    if should_change {
        sup.set_status(app, agent_id, new.clone(), None);
        if matches!(new, AgentStatus::Idle) {
            sup.fetch_and_emit_pr_state(app.clone(), agent_id.to_string());
            // Turn ended (managed in-band, per-turn exit, or native silence all
            // converge here). Ingest the just-written transcript into
            // session_records. Idempotent + reader-gated, so it's a cheap no-op
            // for agents without a reader.
            sup.trigger_session_sync(app.clone(), agent_id.to_string());
            drain_pending_bin_respawn(sup, app, agent_id);
            drain_message_queue(sup, app, agent_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty_session::PtySpawn;
    use crate::sandbox::KillHandle;
    use crate::workspace::{new_agent_record, AgentView, TrackedRepo};
    use std::path::Path;
    use std::time::Duration;

    pub(super) fn test_supervisor() -> Supervisor {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::database::init(dir.path()).unwrap();
        Supervisor::new(Arc::new(WorkspaceManager::new(db)))
    }

    /// Manual/local check (macOS-only, `#[ignore]`d so it's off the Linux CI
    /// path). Reproduces the real spawn topology — PTY child is `sandbox-exec`,
    /// which runs a process that itself spawns a child — and proves
    /// `shutdown()` takes down the *whole* tree, not just the leader, so
    /// quitting can't orphan a grandchild (e.g. a bash/MCP process claude
    /// spawned). Run with:
    ///   cargo test --lib shutdown_kills_sandbox_exec_grandchild -- --ignored --nocapture
    #[test]
    #[ignore]
    #[cfg(target_os = "macos")]
    fn shutdown_kills_sandbox_exec_grandchild() {
        use std::time::Instant;

        fn alive(pid: i32) -> bool {
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
        }

        let sup = test_supervisor();
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pids");

        // sandbox-exec execs the shell in-place (so $$ is the PTY's direct
        // child), and the shell backgrounds a `sleep` in the same process
        // group — the stand-in for a child claude spawns. We record both pids.
        let script = format!(
            "echo leader=$$ > '{pf}'; sleep 1000 & echo child=$! >> '{pf}'; wait",
            pf = pidfile.display()
        );
        let pty = PtySession::spawn(
            PtySpawn {
                program: Path::new("/usr/bin/sandbox-exec"),
                args: &[
                    "-p".to_string(),
                    "(version 1)(allow default)".to_string(),
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    script,
                ],
                cwd: dir.path(),
                env: &[],
                cols: 80,
                rows: 24,
                kill_plan: KillHandle::ProcessGroup,
            },
            |_| {},
            |_| {},
        )
        .unwrap();
        sup.shells.lock().insert("agent".to_string(), pty);

        // Wait for both pids to be recorded by the spawned tree.
        let (mut leader, mut child) = (0i32, 0i32);
        let start = Instant::now();
        while (leader == 0 || child == 0) && start.elapsed() < Duration::from_secs(5) {
            if let Ok(s) = std::fs::read_to_string(&pidfile) {
                for line in s.lines() {
                    if let Some(p) = line.strip_prefix("leader=") {
                        leader = p.trim().parse().unwrap_or(0);
                    }
                    if let Some(p) = line.strip_prefix("child=") {
                        child = p.trim().parse().unwrap_or(0);
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(leader != 0 && child != 0, "failed to capture pids");
        assert!(
            alive(leader) && alive(child),
            "both processes should be running before shutdown"
        );
        eprintln!("before shutdown: leader={leader} alive, child={child} alive");

        sup.shutdown();

        let start = Instant::now();
        while (alive(leader) || alive(child)) && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
        eprintln!(
            "after shutdown:  leader={leader} {}, child={child} {}",
            if alive(leader) { "ALIVE" } else { "dead" },
            if alive(child) { "ALIVE" } else { "dead" },
        );
        assert!(
            !alive(leader),
            "sandbox-exec leader survived shutdown (pid {leader})"
        );
        assert!(
            !alive(child),
            "grandchild survived shutdown (pid {child}) — orphaned!"
        );
    }

    #[test]
    fn shutdown_kills_live_children_and_drains_maps() {
        use std::sync::mpsc;

        let sup = test_supervisor();
        let dir = tempfile::tempdir().unwrap();
        let (exit_tx, exit_rx) = mpsc::channel();

        // A long-lived child parked in the shells map. Its on_exit callback
        // fires only when the process actually dies (the waiter thread's
        // child.wait() returns), so receiving on exit_rx proves shutdown
        // killed it rather than leaving it orphaned.
        let pty = PtySession::spawn(
            PtySpawn {
                program: Path::new("/bin/sh"),
                args: &["-c".to_string(), "while :; do sleep 0.1; done".to_string()],
                cwd: dir.path(),
                env: &[],
                cols: 80,
                rows: 24,
                kill_plan: KillHandle::ProcessGroup,
            },
            |_| {},
            move |_exit| {
                let _ = exit_tx.send(());
            },
        )
        .unwrap();
        sup.shells.lock().insert("agent".to_string(), pty);

        sup.shutdown();

        // The child must have been killed (its waiter reports the exit)...
        exit_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("shutdown should kill the live shell child");
        // ...and every live-process map must be drained.
        assert!(sup.shells.lock().is_empty());
        assert!(sup.agents.lock().is_empty());
        assert!(sup.runs.lock().is_empty());
    }

    pub(super) fn record_with_status(id: &str, status: AgentStatus) -> AgentRecord {
        let mut record = new_agent_record(
            id.to_string(),
            id.to_string(),
            "claude".to_string(),
            TrackedRepo {
                repo_path: PathBuf::from("/r"),
                subdir: "repo".to_string(),
                branch: None,
                parent_branch: None,
                base_sha: None,
                pr_number: None,
                pr_url: None,
                pr_title: None,
                pr_state: None,
            },
            String::new(),
            AgentView::Custom,
        );
        record.status = status;
        record
    }

    #[test]
    fn effective_status_falls_back_to_record_when_absent() {
        let sup = test_supervisor();
        // No in-memory entry → use the record's (DB-derived) status.
        let record = record_with_status("yosemite", AgentStatus::Spawning);
        assert_eq!(
            sup.effective_status("yosemite", &record),
            AgentStatus::Spawning
        );

        let stopped = record_with_status("dolomites", AgentStatus::Stopped);
        assert_eq!(
            sup.effective_status("dolomites", &stopped),
            AgentStatus::Stopped
        );
    }

    #[test]
    fn effective_status_prefers_in_memory_value() {
        let sup = test_supervisor();
        sup.statuses
            .lock()
            .insert("yosemite".to_string(), AgentStatus::Running);
        // Record derives Spawning, but the live map says Running — the
        // in-memory value wins.
        let record = record_with_status("yosemite", AgentStatus::Spawning);
        assert_eq!(
            sup.effective_status("yosemite", &record),
            AgentStatus::Running
        );
    }

    #[test]
    fn live_status_reflects_inserted_value() {
        let sup = test_supervisor();
        assert_eq!(sup.live_status("yosemite"), None);
        sup.statuses
            .lock()
            .insert("yosemite".to_string(), AgentStatus::Running);
        assert_eq!(sup.live_status("yosemite"), Some(AgentStatus::Running));
    }
}
