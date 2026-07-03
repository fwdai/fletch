//! Coordinator between Tauri IPC commands and the running agents.

use parking_lot::Mutex;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

use crate::activity::{Activity, ClaudeNativeActivity, ManagedActivity};
use crate::agent::{capabilities, injection_mode, per_turn_descriptor, Agent, PerTurnSpec, SpawnSpec};
use crate::error::{Error, Result};
use crate::git;
use crate::managed_session::ToolUseBehavior;
use crate::message_queue::{decide_delivery, Delivery, MessageQueue, PendingMsg};
use crate::pty_session::{PtySession, PtySpawn};
use crate::rpc;
use crate::run_session::{
    self, shell_args, user_shell, RunPhase, RunSession, RunStateSnapshot,
};
use crate::workspace::{
    agent_parent_dir, allocate_repo_subdir, is_per_turn_provider, new_agent_record,
    repo_worktree_path, AgentRecord, AgentStatus, AgentView, ArchiveMetadata, ArchivedRepoSnapshot,
    DiffStats, TrackedRepo, Workspace, WorkspaceManager,
};

#[derive(Clone, serde::Serialize)]
pub struct AgentOutputPayload {
    pub agent_id: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, serde::Serialize)]
pub struct AgentEventPayload {
    pub agent_id: String,
    pub event: Value,
}

#[derive(Clone, serde::Serialize)]
pub struct SessionRecordsAppendedPayload {
    pub agent_id: String,
}

/// Emitted when a turn flips to Running, carrying the backend's own start
/// timestamp (the same value persisted as the turn's `started_at`). The live
/// timer anchors to this rather than the event's client-receipt time, so it
/// shares the footer's clock and the two never disagree by the delivery latency.
#[derive(Clone, serde::Serialize)]
pub struct TurnStartedPayload {
    pub agent_id: String,
    pub started_at: i64,
}

#[derive(Clone, serde::Serialize)]
pub struct AgentStatusPayload {
    pub agent_id: String,
    pub status: AgentStatus,
    pub last_error: Option<String>,
}

#[derive(Clone, serde::Serialize)]
pub struct AgentViewPayload {
    pub agent_id: String,
    pub view: AgentView,
}

#[derive(Clone, serde::Serialize)]
pub struct AgentTaskPayload {
    pub agent_id: String,
    pub task: String,
}

#[derive(Clone, serde::Serialize)]
pub struct AgentBranchPayload {
    pub agent_id: String,
    pub subdir: String,
    pub branch: String,
}

#[derive(Clone, serde::Serialize)]
pub struct AgentRepoAddedPayload {
    pub agent_id: String,
    pub repo: TrackedRepo,
}

/// A successful, mutating git RPC op (`op`) the agent ran this turn — the
/// causal signal the delegation panel uses to confirm the agent did the work.
#[derive(Clone, serde::Serialize)]
pub struct AgentGitActionPayload {
    pub agent_id: String,
    pub op: String,
}


#[derive(Clone, serde::Serialize)]
pub struct ShellOutputPayload {
    pub agent_id: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, serde::Serialize)]
pub struct PrStateChangedPayload {
    pub agent_id: String,
    pub state: Option<crate::gh::PrState>,
}

#[derive(Clone, serde::Serialize)]
pub struct RunOutputPayload {
    pub agent_id: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, serde::Serialize)]
pub struct RunStatePayload {
    pub agent_id: String,
    pub phase: RunPhase,
    pub last_error: Option<String>,
}

const WATCHDOG_TICK: Duration = Duration::from_millis(500);
const SPAWN_TIMEOUT: Duration = Duration::from_secs(15);
/// How often the per-agent RPC watcher scans its mailbox for new requests.
const RPC_TICK: Duration = Duration::from_millis(100);

pub struct Supervisor {
    pub workspace: Arc<WorkspaceManager>,
    pub agents: Mutex<HashMap<String, Agent>>,
    pub generations: Mutex<HashMap<String, u64>>,
    pub activities: Mutex<HashMap<String, Box<dyn Activity>>>,
    /// In-memory source of truth for live runtime status
    /// (Spawning/Running/Idle). The DB only persists durable
    /// dispositions, so a resting record loaded from it derives `Idle`;
    /// this map carries the real current status while an agent is live.
    pub statuses: Mutex<HashMap<String, AgentStatus>>,
    pub native_input_lines: Mutex<HashMap<String, String>>,
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
}

/// Resolved, per-spawn inputs for `spawn_agent_process` — everything that
/// isn't already carried on the `AgentRecord` (paths, session id, and this
/// spawn's generation number).
struct ProcessLaunch {
    cwd: PathBuf,
    sandbox_root: PathBuf,
    rpc_dir: PathBuf,
    session_id: Option<String>,
    per_turn: bool,
    effective_fresh: bool,
    my_gen: u64,
}

/// Pick the turn-end detector for an agent by provider class and view, and
/// reset it when this spawn begins a fresh turn.
///
/// Per-turn agents carry their detector in the descriptor table — but only for
/// the Custom (exec/JSON) view. In the native view they run their interactive
/// TUI in a PTY with no JSON stream, so turn-end is detected by silence, the
/// same as claude's native view. Claude (no descriptor) picks by view too.
fn build_activity(record: &AgentRecord, effective_fresh: bool) -> Box<dyn Activity> {
    let mut activity: Box<dyn Activity> = match per_turn_descriptor(&record.provider) {
        Some(desc) => match record.view {
            AgentView::Native => Box::new(ClaudeNativeActivity::new()),
            AgentView::Custom => (desc.activity)(),
        },
        None => match record.view {
            AgentView::Native => Box::new(ClaudeNativeActivity::new()),
            AgentView::Custom => Box::new(ManagedActivity::claude()),
        },
    };
    if effective_fresh {
        activity.reset_for_new_turn();
    }
    activity
}

impl Supervisor {
    pub fn new(workspace: Arc<WorkspaceManager>) -> Self {
        Self {
            workspace,
            agents: Mutex::new(HashMap::new()),
            generations: Mutex::new(HashMap::new()),
            activities: Mutex::new(HashMap::new()),
            statuses: Mutex::new(HashMap::new()),
            native_input_lines: Mutex::new(HashMap::new()),
            shells: Mutex::new(HashMap::new()),
            runs: Mutex::new(HashMap::new()),
            respawn_pending: Mutex::new(HashSet::new()),
            message_queue: Mutex::new(MessageQueue::new()),
            interrupted: Mutex::new(HashSet::new()),
        }
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

        // Agent sessions: Agent::shutdown consumes and drops, killing the
        // managed/pty/per-turn child.
        let agents = std::mem::take(&mut *self.agents.lock());
        for (_, agent) in agents {
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
        // No-op when no turn is open (resting Idle at spawn, native turns), so
        // it's safe to run unconditionally on these transitions.
        if matches!(status, AgentStatus::Idle | AgentStatus::Error) {
            if let Err(e) = self.workspace.mark_user_turn_ended(agent_id) {
                tracing::warn!(error = %e, agent_id, "stamp user turn end failed");
            }
        }
        emit_status(app, agent_id, status, last_error);
    }

    /// The live (in-memory) runtime status, if the supervisor is tracking
    /// this agent. `None` once the agent is gone (exited / archived).
    fn live_status(&self, agent_id: &str) -> Option<AgentStatus> {
        self.statuses.lock().get(agent_id).cloned()
    }

    /// The status to report for an agent: the live in-memory value when
    /// present, otherwise the DB-derived at-rest status on the record.
    fn effective_status(&self, agent_id: &str, record: &AgentRecord) -> AgentStatus {
        self.live_status(agent_id)
            .unwrap_or_else(|| record.status.clone())
    }

    pub fn open_agent_shell(self: Arc<Self>, app: AppHandle, agent_id: &str) -> Result<()> {
        {
            let shells = self.shells.lock();
            if shells.contains_key(agent_id) {
                return Ok(());
            }
        }

        let record = self.workspace.agent(agent_id)?;
        let repo = record.repos.first()
            .ok_or_else(|| Error::Other("agent has no repos".into()))?;
        let worktree = repo_worktree_path(agent_id, &repo.subdir)?;

        let shell_str = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let shell_path = std::path::PathBuf::from(&shell_str);

        let sup_weak = Arc::downgrade(&self);
        let agent_id_out = agent_id.to_string();
        let agent_id_exit = agent_id.to_string();

        let session = PtySession::spawn(
            PtySpawn {
                program: &shell_path,
                args: &[],
                cwd: &worktree,
                env: &[],
                cols: 120,
                rows: 32,
            },
            move |bytes| {
                if let Err(e) = app.emit(
                    "shell:output",
                    ShellOutputPayload {
                        agent_id: agent_id_out.clone(),
                        bytes,
                    },
                ) {
                    tracing::warn!(error = %e, agent_id = %agent_id_out, "emit shell:output failed");
                }
            },
            move |exit| {
                tracing::info!(
                    success = exit.success,
                    message = %exit.message,
                    agent_id = %agent_id_exit,
                    "shell exited"
                );
                if let Some(sup) = sup_weak.upgrade() {
                    sup.shells.lock().remove(&agent_id_exit);
                }
            },
        )?;

        self.shells.lock().insert(agent_id.to_string(), session);
        Ok(())
    }

    pub fn close_agent_shell(&self, agent_id: &str) -> Result<()> {
        self.shells.lock().remove(agent_id); // Drop impl kills the PTY
        Ok(())
    }

    pub fn write_to_shell(&self, agent_id: &str, data: &[u8]) -> Result<()> {
        self.shells
            .lock()
            .get(agent_id)
            .ok_or_else(|| Error::Other("no shell for agent".into()))?
            .write(data)
    }

    pub fn resize_shell(&self, agent_id: &str, cols: u16, rows: u16) -> Result<()> {
        self.shells
            .lock()
            .get(agent_id)
            .ok_or_else(|| Error::Other("no shell for agent".into()))?
            .resize(cols, rows)
    }

    /// Start the Run-panel process for an agent.
    ///
    /// If the agent has never completed setup before, runs the setup
    /// command first; on exit 0 marks setup complete and chains into
    /// the run command. On setup failure → does NOT proceed to run.
    /// If setup is already complete, starts the run command directly.
    ///
    /// No-op if a run is already in progress for this agent.
    pub fn run_start(self: Arc<Self>, app: AppHandle, agent_id: &str) -> Result<()> {
        let record = self.workspace.agent(agent_id)?;
        if record.archive.is_some() {
            return Err(Error::Other("agent is archived".into()));
        }
        let primary = record
            .repos
            .first()
            .ok_or_else(|| Error::Other("agent has no repos".into()))?;
        let cwd = repo_worktree_path(agent_id, &primary.subdir)?;

        let (setup_cmd, run_cmd) = self.read_run_commands(&record.project_id, &cwd);
        let setup_done = self.workspace.is_setup_completed(agent_id)?;

        let session = {
            let mut runs = self.runs.lock();
            runs.entry(agent_id.to_string())
                .or_insert_with(|| Arc::new(RunSession::new()))
                .clone()
        };

        if session.is_active() {
            return Ok(()); // already running, idempotent
        }

        // Nothing to run (unrecognized ecosystem with no install/dev) —
        // leave the button Idle rather than spawning an empty command.
        let Some(plan) = plan_run_phases(setup_done, &setup_cmd, &run_cmd) else {
            return Ok(());
        };

        let gen = session.begin_phase(plan.first_phase);
        emit_run_state(&app, agent_id, plan.first_phase, None);
        write_header(&app, agent_id, &session, &plan.first_cmd);

        spawn_run_phase(
            self.clone(),
            app,
            agent_id.to_string(),
            session,
            gen,
            cwd,
            plan.first_phase,
            plan.first_cmd,
            plan.chained_run_cmd,
        )
    }

    /// Stop the Run-panel process for an agent. Idempotent.
    pub fn run_stop(&self, app: AppHandle, agent_id: &str) -> Result<()> {
        let session = {
            let runs = self.runs.lock();
            runs.get(agent_id).cloned()
        };
        let Some(session) = session else {
            return Ok(());
        };
        let prior = session.stop();
        if matches!(prior, RunPhase::Setup | RunPhase::Running) {
            emit_run_state(&app, agent_id, RunPhase::Stopped, None);
        }
        Ok(())
    }

    /// Snapshot of the current state and accumulated log for the
    /// panel to rehydrate on mount.
    pub fn run_state(&self, agent_id: &str) -> RunStateSnapshot {
        let session = {
            let runs = self.runs.lock();
            runs.get(agent_id).cloned()
        };
        match session {
            Some(s) => s.snapshot(),
            None => RunStateSnapshot {
                phase: RunPhase::Idle,
                last_error: None,
                log: Vec::new(),
            },
        }
    }

    /// Read the setup + run commands for an agent. The detector provides
    /// the baseline (same values the panel shows), and any persisted
    /// `run.install` / `run.dev` overrides in project_settings take
    /// precedence. One detector feeds both the panel and the runner, so
    /// there is no hardcoded default to keep in sync.
    fn read_run_commands(&self, project_id: &str, worktree: &Path) -> (String, String) {
        let configs = crate::run_detect::detect_all(worktree);
        let detected = |id: &str| -> String {
            configs
                .first()
                .and_then(|c| c.rows.iter().find(|r| r.id == id))
                .map(|r| r.value.clone())
                .unwrap_or_default()
        };
        let install_default = detected("install");
        let dev_default = detected("dev");
        if project_id.is_empty() {
            return (install_default, dev_default);
        }
        let conn = self.workspace.db_handle();
        let read = |key: &str| -> Option<String> {
            let conn = conn.lock();
            conn.query_row(
                "SELECT value FROM project_settings WHERE project_id = ?1 AND key = ?2",
                rusqlite::params![project_id, key],
                |row| row.get::<_, String>(0),
            )
            .ok()
        };
        (
            read("run.install").unwrap_or(install_default),
            read("run.dev").unwrap_or(dev_default),
        )
    }

    /// Detect the run configuration for an agent's primary repo,
    /// ranked by confidence. The panel renders the first (highest
    /// confidence) entry; the rest are returned for future
    /// multi-ecosystem selection.
    pub fn detect_run_config(&self, agent_id: &str) -> Result<Vec<crate::run_detect::DetectedConfig>> {
        let record = self.workspace.agent(agent_id)?;
        let primary = record
            .repos
            .first()
            .ok_or_else(|| Error::Other("agent has no repos".into()))?;
        let worktree = repo_worktree_path(agent_id, &primary.subdir)?;
        Ok(crate::run_detect::detect_all(&worktree))
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

    pub fn add_workspace_repo(&self, repo_path: PathBuf) -> Result<Workspace> {
        self.workspace.add_workspace_repo(repo_path)
    }

    pub fn remove_workspace_repo(&self, repo_path: PathBuf) -> Result<Workspace> {
        self.workspace.remove_workspace_repo(&repo_path)
    }

    pub async fn spawn_agent(
        self: Arc<Self>,
        app: AppHandle,
        view: AgentView,
        repo_path: PathBuf,
        provider: String,
        name: Option<String>,
        effort: Option<String>,
        model: Option<String>,
        instructions: Option<String>,
        custom_agent_id: Option<String>,
        // Explicit fork point (a commit-ish). When set — e.g. a workflow step
        // forking from the previous step's HEAD — the worktree is cut from this
        // commit instead of the parent branch's remote fork-point. None keeps the
        // default behavior.
        fork_base: Option<String>,
    ) -> Result<AgentRecord> {
        if !repo_path.join(".git").exists() {
            return Err(Error::InvalidPath(format!(
                "not a git repository: {}",
                repo_path.display()
            )));
        }

        // Only agents with a wired native (PTY/TUI) view can honor a Native
        // request; the rest fall back to the structured Custom view. Native
        // views are being rolled out per agent (see `AgentCapabilities`).
        let view = if capabilities(&provider).native_view {
            view
        } else {
            AgentView::Custom
        };

        // Use the name the draft already showed in the sidebar so it locks in
        // rather than being regenerated; only allocate a fresh one when the
        // caller didn't supply it (the draft-less spawn path).
        let agent_id = match name {
            Some(n) if !n.trim().is_empty() => n,
            _ => self.workspace.allocate_agent_id()?,
        };
        let name = agent_id.clone();

        // Parent_branch captured per-repo; primary's parent is the
        // branch the user was on when they hit Spawn.
        let parent_branch = git::current_branch(&repo_path).await.ok().flatten();
        let subdir = allocate_repo_subdir(&repo_path, &[]);
        // Cloned for the background fork task — `parent_branch`/`subdir` are
        // moved into `primary` below.
        let parent_for_fork = parent_branch.clone();
        let subdir_for_fork = subdir.clone();

        let primary = TrackedRepo {
            repo_path: repo_path.clone(),
            subdir: subdir.clone(),
            branch: None, // materialized at first push, named by the agent
            parent_branch,
            base_sha: None, // captured by the fork task once HEAD is known
            pr_number: None, // set when a PR is opened for this branch
        };

        let mut record = new_agent_record(
            agent_id.clone(),
            name,
            provider,
            primary,
            String::new(),
            view,
        );
        // Session-level effort (claude `--effort`); persisted so start_process
        // re-applies it on every spawn. Per-turn agents ignore it at spawn.
        record.effort = effort;
        // Session-level model selection. `None` preserves the provider CLI
        // default; selected values are reapplied on resume and view switches.
        record.model = model;
        // Custom agent identity + snapshotted brief. Both `None` for a plain
        // built-in spawn. The brief is re-injected on every spawn/resume.
        record.instructions = instructions;
        record.custom_agent_id = custom_agent_id;
        let parent_dir = agent_parent_dir(&agent_id)?;
        let primary_worktree = repo_worktree_path(&agent_id, &subdir)?;

        self.workspace.add_agent(&mut record)?;
        crate::telemetry::track(
            "agent_spawned",
            serde_json::json!({
                "provider": record.provider,
                "model": record.model,
                "effort": record.effort,
            }),
        );
        self.set_status(&app, &agent_id, AgentStatus::Spawning, None);
        arm_spawn_timeout(self.clone(), app.clone(), agent_id.clone());

        let sup = self.clone();
        let app_for_task = app.clone();
        let id_for_task = agent_id.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = tokio::fs::create_dir_all(&parent_dir).await {
                fail_spawn(&sup, &app_for_task, &id_for_task, e.to_string());
                return;
            }

            // Fork point: an explicit base (a workflow step forking from the
            // previous step's HEAD) wins; otherwise fork from the freshest remote
            // state of the parent branch so the agent never starts on stale local
            // refs. Best-effort: offline, no remote, or a local-only branch all
            // fall back to local HEAD.
            let base = match &fork_base {
                Some(sha) => Some(sha.clone()),
                None => match &parent_for_fork {
                    Some(b) => git::fetch_fork_point(&repo_path, b).await,
                    None => None,
                },
            };
            if let Err(e) =
                git::worktree_add_detached(&repo_path, &primary_worktree, base.as_deref()).await
            {
                fail_spawn(&sup, &app_for_task, &id_for_task, e.to_string());
                return;
            }

            // Record the fork point so diffs measure against the exact starting
            // commit rather than a branch name that can drift. Non-fatal: a
            // missing base_sha just falls back to the parent branch name.
            if let Ok(sha) = git::rev_parse(&primary_worktree, "HEAD").await {
                let _ = sup
                    .workspace
                    .set_repo_base_sha(&id_for_task, &subdir_for_fork, &sha);
            }

            tokio::time::sleep(Duration::from_millis(350)).await;

            if let Err(e) = sup.start_process(&app_for_task, &id_for_task, true).await {
                let _ = git::worktree_remove(&repo_path, &primary_worktree, true).await;
                let _ = tokio::fs::remove_dir_all(&parent_dir).await;
                fail_spawn(&sup, &app_for_task, &id_for_task, e.to_string());
            }
        });

        Ok(record)
    }

    /// Bring a second (or third…) repo into a live agent. Creates a
    /// detached worktree at `~/.fletch/worktrees/<agent-id>/<subdir>/`
    /// and appends a TrackedRepo entry. The worktree stays detached until
    /// its first push, consistent with the primary repo.
    pub async fn add_repo_to_agent(
        self: Arc<Self>,
        app: AppHandle,
        agent_id: &str,
        repo_path: PathBuf,
    ) -> Result<TrackedRepo> {
        if !repo_path.join(".git").exists() {
            return Err(Error::InvalidPath(format!(
                "not a git repository: {}",
                repo_path.display()
            )));
        }
        let record = self.workspace.agent(agent_id)?;
        if record.repos.iter().any(|r| r.repo_path == repo_path) {
            return Err(Error::Other(
                "this repo is already tracked by the agent".into(),
            ));
        }
        let used: Vec<String> = record.repos.iter().map(|r| r.subdir.clone()).collect();
        let subdir = allocate_repo_subdir(&repo_path, &used);
        let worktree = repo_worktree_path(agent_id, &subdir)?;
        let parent_branch = git::current_branch(&repo_path).await.ok().flatten();

        // Fork from the freshest remote state of the parent branch (best-effort,
        // falls back to local HEAD), then record the fork point as the diff base.
        let base = match &parent_branch {
            Some(b) => git::fetch_fork_point(&repo_path, b).await,
            None => None,
        };
        git::worktree_add_detached(&repo_path, &worktree, base.as_deref()).await?;
        let base_sha = git::rev_parse(&worktree, "HEAD").await.ok();

        let repo = TrackedRepo {
            repo_path: repo_path.clone(),
            subdir: subdir.clone(),
            branch: None,
            parent_branch,
            base_sha,
            pr_number: None,
        };
        self.workspace
            .append_tracked_repo(agent_id, repo.clone())?;
        let _ = app.emit(
            "agent:repo_added",
            AgentRepoAddedPayload {
                agent_id: agent_id.to_string(),
                repo: repo.clone(),
            },
        );

        // No branch is created here — the new repo's worktree stays detached
        // until its first push, when the agent names its branch (same as the
        // primary repo).
        Ok(repo)
    }

    async fn start_process(
        self: &Arc<Self>,
        app: &AppHandle,
        agent_id: &str,
        fresh: bool,
    ) -> Result<()> {
        let record = self.workspace.agent(agent_id)?;
        let per_turn = is_per_turn_provider(&record.provider);
        // Claude carries a session id we generated at create time; per-turn
        // agents (codex, cursor) are assigned one by the CLI on their first
        // turn, so it may be None until then.
        let session_id = record.session_id.clone();
        if !per_turn && session_id.is_none() {
            return Err(Error::Other("agent record missing session_id".into()));
        }
        let primary = record
            .repos
            .first()
            .ok_or_else(|| Error::Other("agent has no tracked repos".into()))?;
        let cwd = repo_worktree_path(agent_id, &primary.subdir)?;
        // Sandbox writable root — the agent's parent dir. Every agent (claude
        // and per-turn alike) now runs under sandbox-exec rooted here.
        let sandbox_root = agent_parent_dir(agent_id)?;

        // The agent's file-mailbox RPC dir, created before spawn so the watcher
        // (and the agent's `QUORUM_RPC_DIR`) have a target from turn one.
        let rpc_dir = rpc::mailbox_dir(agent_id)?;
        rpc::ensure_mailbox(&rpc_dir)?;
        // Base branch for the git dispatcher — the branch the agent was
        // forked from, same default the manual PR action uses.
        let base_branch = primary
            .parent_branch
            .clone()
            .unwrap_or_else(|| "main".to_string());
        let rpc_dispatcher = Arc::new(rpc::git::GitDispatcher::new(cwd.clone(), base_branch));

        // Claude only writes a session file once the first turn lands.
        // If task is still empty (no first user message has ever been
        // sent) `--resume <uuid>` will 404. So we treat that case as
        // fresh — same UUID, no replay attempt — and the eventual
        // first message creates the session file. Once that's
        // happened, switch / resume can safely `--resume`.
        let no_messages_yet = record.task.trim().is_empty();
        let effective_fresh = fresh || no_messages_yet;

        let agent_id_str = agent_id.to_string();

        let my_gen = {
            let mut g = self.generations.lock();
            let entry = g.entry(agent_id_str.clone()).or_insert(0);
            *entry += 1;
            *entry
        };

        self.activities
            .lock()
            .insert(agent_id_str.clone(), build_activity(&record, effective_fresh));

        let agent = self.spawn_agent_process(
            app,
            &agent_id_str,
            &record,
            ProcessLaunch {
                cwd,
                sandbox_root,
                rpc_dir: rpc_dir.clone(),
                session_id,
                per_turn,
                effective_fresh,
                my_gen,
            },
        )?;

        self.agents.lock().insert(agent_id_str.clone(), agent);

        // Initial status is always Idle now — at process start there's
        // never an in-flight turn (we no longer pass a task as a spawn
        // arg). The user's first send flips it to Running. Promote out of
        // the live Spawning state atomically (a turn that already started
        // mustn't be clobbered). If the swap fails because the spawn already
        // timed out (status Error), the timeout fired before we inserted the
        // process above — so its shutdown was a no-op and we'd leak a live
        // process shown as failed. Tear down what we just started instead.
        let promoted = self.claim_spawn_outcome(app, &agent_id_str, AgentStatus::Idle, None);
        if !promoted && matches!(self.live_status(&agent_id_str), Some(AgentStatus::Error)) {
            self.bump_generation(&agent_id_str);
            if let Some(agent) = self.agents.lock().remove(&agent_id_str) {
                let _ = agent.shutdown();
            }
            self.activities.lock().remove(&agent_id_str);
            return Err(Error::Other(
                "spawn aborted: timed out before the process became ready".into(),
            ));
        }

        // A message sent before the process finished coming up was enqueued —
        // a Spawning agent counts as busy, so `send_user_message` routes the
        // first send as a follow-up (Enqueue for per-turn, a retried
        // AgentNotFound for claude). Turn-end Idle drains the queue via
        // `transition_active`, but this spawn-completion Idle doesn't go through
        // that path, so drain here too. Without it a queued first message sits
        // undelivered until the *next* message flushes it (the user sees their
        // bubble + a spinner that clears with no reply). No-op when the queue is
        // empty — the common case — and `drain_coalesced` makes it safe against a
        // concurrent FlushNow from a racing second send.
        if promoted {
            drain_message_queue(self, app, &agent_id_str);
        }

        spawn_turn_watchdog(self.clone(), app.clone(), agent_id_str.clone(), my_gen);

        // Watch this agent's RPC mailbox for the life of this generation,
        // executing allowlisted ops and writing responses back.
        spawn_rpc_watcher(
            self.clone(),
            app.clone(),
            agent_id_str,
            rpc_dispatcher,
            rpc_dir,
            my_gen,
        );

        Ok(())
    }

    /// Spawn the agent's child process, dispatching on provider class
    /// (per-turn vs. claude) and view (Native PTY vs. Custom managed/exec).
    /// Returns the live `Agent` handle for the supervisor to track.
    fn spawn_agent_process(
        self: &Arc<Self>,
        app: &AppHandle,
        agent_id: &str,
        record: &AgentRecord,
        launch: ProcessLaunch,
    ) -> Result<Agent> {
        let ProcessLaunch {
            cwd,
            sandbox_root,
            rpc_dir,
            session_id,
            per_turn,
            effective_fresh,
            my_gen,
        } = launch;
        let agent_id_str = agent_id.to_string();

        if per_turn {
            match record.view {
                // Native view: launch the agent's interactive TUI in a PTY,
                // resuming the session the Custom view established. The
                // switch_view guard guarantees a session id is present before
                // we ever route a per-turn agent here.
                AgentView::Native => {
                    let session_id = session_id.as_deref().ok_or_else(|| {
                        Error::Other("native view requires an established session id".into())
                    })?;
                    let spec = SpawnSpec {
                        agent_id: &agent_id_str,
                        cwd,
                        sandbox_root,
                        session_id,
                        // Per-turn native always resumes (the agent built its
                        // session in the Custom view first).
                        fresh: false,
                        // Per-turn agents take effort per-turn (build-args),
                        // not at spawn.
                        effort: None,
                        model: record.model.as_deref(),
                        instructions: record.instructions.as_deref(),
                        rpc_dir,
                        cols: 120,
                        rows: 32,
                    };
                    spawn_pty_per_turn_agent(
                        spec,
                        record.provider.clone(),
                        app.clone(),
                        agent_id_str.clone(),
                        self.clone(),
                        my_gen,
                    )
                }
                // Custom view: per-turn runner — no process spawns until the
                // first user message. No sandbox profile: the agent sandboxes
                // itself rather than running under sandbox-exec.
                AgentView::Custom => spawn_per_turn_agent(
                    &record.provider,
                    PerTurnSpec {
                        cwd,
                        sandbox_root,
                        session_id,
                        model: record.model.clone(),
                        instructions: record.instructions.clone(),
                        rpc_dir,
                    },
                    app.clone(),
                    agent_id_str.clone(),
                    self.clone(),
                    my_gen,
                ),
            }
        } else {
            let session_id = session_id
                .as_deref()
                .expect("non-codex agents always have a session id");
            let spec = SpawnSpec {
                agent_id: &agent_id_str,
                cwd,
                sandbox_root,
                session_id,
                fresh: effective_fresh,
                // Claude's session-level effort, persisted on the record so it
                // re-applies on every spawn (fresh, view-switch, resume).
                effort: record.effort.as_deref(),
                model: record.model.as_deref(),
                instructions: record.instructions.as_deref(),
                rpc_dir,
                cols: 120,
                rows: 32,
            };
            match record.view {
                AgentView::Native => spawn_pty_agent(
                    spec,
                    app.clone(),
                    agent_id_str.clone(),
                    self.clone(),
                    my_gen,
                ),
                AgentView::Custom => spawn_managed_agent(
                    spec,
                    app.clone(),
                    agent_id_str.clone(),
                    self.clone(),
                    my_gen,
                ),
            }
        }
    }

    pub async fn resume_agent(
        self: Arc<Self>,
        app: AppHandle,
        agent_id: &str,
    ) -> Result<()> {
        let record = self.workspace.agent(agent_id)?;
        if self.agents.lock().contains_key(agent_id) {
            return Ok(());
        }
        // Per-turn agents are assigned a session id on their first turn, so
        // a missing one is only an error for providers that generate it up
        // front.
        if !is_per_turn_provider(&record.provider) && record.session_id.is_none() {
            return Err(Error::Other(
                "Agent has no session id; remove and respawn.".into(),
            ));
        }
        self.set_status(&app, agent_id, AgentStatus::Spawning, None);
        arm_spawn_timeout(self.clone(), app.clone(), agent_id.to_string());

        self.start_process(&app, agent_id, false).await?;
        Ok(())
    }

    pub fn write_to_agent(
        self: Arc<Self>,
        app: &AppHandle,
        agent_id: &str,
        bytes: &[u8],
    ) -> Result<()> {
        {
            let agents = self.agents.lock();
            let agent = agents
                .get(agent_id)
                .ok_or_else(|| Error::AgentNotFound(agent_id.to_string()))?;
            agent.write_pty(bytes)?;
        }
        for submitted in observe_native_input(&self, agent_id, bytes) {
            mark_user_turn_started(&self, app, agent_id, None);
            on_first_user_message(self.clone(), app.clone(), agent_id.to_string(), submitted);
        }
        Ok(())
    }

    /// Route a user message by the provider's injection mode and the agent's
    /// current state (see `message_queue::decide_delivery`):
    /// - idle, queue empty  → deliver now as a new turn (the original path),
    /// - idle, queue full    → flush the leftovers + this message, coalesced,
    /// - busy, claude live    → inject into the running turn over stdin,
    /// - busy, per-turn / tool-gated → queue for the next turn boundary.
    pub fn send_user_message(
        self: Arc<Self>,
        app: &AppHandle,
        agent_id: &str,
        turn_id: &str,
        text: &str,
        attachments: &[String],
        thinking: Option<&str>,
    ) -> Result<()> {
        let mode = injection_mode(&self.workspace.agent(agent_id)?.provider);
        let busy = matches!(
            self.live_status(agent_id),
            Some(AgentStatus::Spawning | AgentStatus::Running)
        );
        let tool_gated = self
            .agents
            .lock()
            .get(agent_id)
            .is_some_and(Agent::is_tool_gated);
        let queue_nonempty = !self.message_queue.lock().is_empty(agent_id);

        let msg = PendingMsg {
            turn_id: turn_id.to_string(),
            text: text.to_string(),
            attachments: attachments.to_vec(),
            thinking: thinking.map(str::to_string),
        };

        match decide_delivery(busy, mode, tool_gated, queue_nonempty) {
            Delivery::DeliverNow => deliver_as_turn(&self, app, agent_id, &msg)?,
            Delivery::FlushNow => {
                self.message_queue.lock().enqueue(agent_id, msg);
                flush_queued(&self, app, agent_id)?;
            }
            Delivery::WriteLive => {
                if let Err(e) = self.inject_live(agent_id, &msg) {
                    // The turn ended (or the pipe broke) in the race window
                    // between the busy check and the write. Deliver as a fresh
                    // turn *now* rather than only re-queueing: the turn-end Idle
                    // drain may already have run against an empty queue, so a
                    // bare re-enqueue would strand the follow-up until the next
                    // user message (CQ3-A).
                    tracing::warn!(error = %e, agent_id, "live inject failed; delivering as a new turn");
                    self.message_queue.lock().enqueue(agent_id, msg);
                    flush_queued(&self, app, agent_id)?;
                }
            }
            Delivery::Enqueue => self.message_queue.lock().enqueue(agent_id, msg),
        }
        Ok(())
    }

    /// Inject a message into the running turn over the managed agent's open
    /// stdin (claude). On success, persist its row so it matches the transcript
    /// record the live message produces (the matcher stays 1→1 per live
    /// message). Returns `Err` if the write fails — the turn ended or the pipe
    /// broke in the race window between the busy check and the write — leaving
    /// the message untouched so the caller can fall back without double-handling
    /// it.
    fn inject_live(&self, agent_id: &str, msg: &PendingMsg) -> Result<()> {
        self.agents
            .lock()
            .get(agent_id)
            .ok_or_else(|| Error::AgentNotFound(agent_id.to_string()))
            .and_then(|a| {
                a.send_user_message(&msg.text, &msg.attachments, msg.thinking.as_deref())
            })?;
        if let Err(e) =
            self.workspace
                .insert_user_turn(agent_id, &msg.turn_id, &msg.text, &msg.attachments)
        {
            tracing::warn!(error = %e, agent_id, "persist live-injected user turn failed");
        }
        Ok(())
    }

    /// Capture the outgoing user turn durably, then deliver it to the agent.
    ///
    /// Order matters: we persist the `session_user_turns` row *before* the agent
    /// send, idempotently on `turn_id`. So the message survives even if delivery
    /// fails (agent not yet spawned → `AgentNotFound`; the frontend resumes and
    /// retries via `sendWhenAgentReady`, reusing the same `turn_id` → one row).
    /// On reload a never-delivered turn renders standalone so the user can retry.
    ///
    /// This row carries Fletch-origin metadata (text + attachments) that the
    /// transcript can't; it lives outside `session_records`, which stays a pure
    /// 1:1 mirror of the agent's jsonl. At turn-end `sync_session_records`
    /// matches the row to its canonical transcript user-message and fills in
    /// `native_id`. It is never rendered as a message when matched (the
    /// transcript renders the turn; this only hangs attachments) — so no
    /// double-render with the optimistic live render.
    fn deliver_user_message(
        &self,
        agent_id: &str,
        turn_id: &str,
        text: &str,
        attachments: &[String],
        thinking: Option<&str>,
    ) -> Result<()> {
        // Durable capture first — independent of whether the agent accepts.
        if let Err(e) = self
            .workspace
            .insert_user_turn(agent_id, turn_id, text, attachments)
        {
            tracing::warn!(error = %e, agent_id, "persist outgoing user turn failed");
        }
        let agents = self.agents.lock();
        let agent = agents
            .get(agent_id)
            .ok_or_else(|| Error::AgentNotFound(agent_id.to_string()))?;
        agent.send_user_message(text, attachments, thinking)?;
        Ok(())
    }

    /// Deliver the user's answer to a held user-input prompt as a control
    /// response, unblocking the paused turn.
    pub fn answer_tool_use(
        &self,
        agent_id: &str,
        request_id: &str,
        updated_input: serde_json::Value,
        behavior: ToolUseBehavior,
        message: Option<String>,
    ) -> Result<()> {
        let agents = self.agents.lock();
        let agent = agents
            .get(agent_id)
            .ok_or_else(|| Error::AgentNotFound(agent_id.to_string()))?;
        agent.answer_tool_use(request_id, updated_input, behavior, message)
    }

    pub fn resize_agent(&self, agent_id: &str, cols: u16, rows: u16) -> Result<()> {
        let agents = self.agents.lock();
        let agent = agents
            .get(agent_id)
            .ok_or_else(|| Error::AgentNotFound(agent_id.to_string()))?;
        agent.resize(cols, rows)
    }

    pub async fn switch_view(
        self: Arc<Self>,
        app: AppHandle,
        agent_id: &str,
        new_view: AgentView,
    ) -> Result<()> {
        let record = self.workspace.agent(agent_id)?;
        if record.view == new_view {
            return Ok(());
        }
        // Reject switching to native for agents whose native view isn't
        // wired yet (rolling out per agent — see `AgentCapabilities`).
        if new_view == AgentView::Native && !capabilities(&record.provider).native_view {
            return Err(Error::Other(
                "The native view isn't available for this agent yet".into(),
            ));
        }

        // Per-turn agents assign their own session id on the first turn, and
        // the native TUI gives us no event stream to capture it. So we only
        // allow switching to native once that id exists — the TUI then
        // resumes the same session, and switching back to Custom can resume
        // it too. (claude generates its id up front, so this never blocks it.)
        if new_view == AgentView::Native
            && is_per_turn_provider(&record.provider)
            && record.session_id.is_none()
        {
            return Err(Error::Other(
                "Switch to the native view after the agent's first turn".into(),
            ));
        }

        if let Some(agent) = self.agents.lock().remove(agent_id) {
            let _ = agent.shutdown();
        }
        self.activities.lock().remove(agent_id);
        self.native_input_lines.lock().remove(agent_id);

        self.workspace.update_agent_view(agent_id, new_view)?;
        let _ = app.emit(
            "agent:view",
            AgentViewPayload {
                agent_id: agent_id.to_string(),
                view: new_view,
            },
        );
        self.set_status(&app, agent_id, AgentStatus::Spawning, None);
        arm_spawn_timeout(self.clone(), app.clone(), agent_id.to_string());

        tokio::time::sleep(Duration::from_millis(150)).await;

        if let Err(e) = self.start_process(&app, agent_id, false).await {
            let err = e.to_string();
            self.set_status(&app, agent_id, AgentStatus::Error, Some(err));
            return Err(e);
        }
        Ok(())
    }

    /// Respawn every live agent using `provider_id` so it picks up a freshly
    /// changed binary path. The binary is resolved only inside `start_process`
    /// (spawn / resume / view-switch), so a live agent keeps the old binary —
    /// baked into its running process (claude, persistent) or frozen spawn args
    /// (per-turn) — until torn down and restarted. Callers must refresh the
    /// `bin_resolve` override registry *before* calling this so the restarted
    /// processes resolve the new path.
    ///
    /// Only currently-live agents need this; anything not in the `agents` map
    /// will resolve the new binary on its next spawn anyway.
    pub async fn respawn_provider(self: &Arc<Self>, app: &AppHandle, provider_id: &str) {
        // Snapshot ids under a short-lived lock; never hold a guard across the
        // `start_process` await in `respawn_agent_for_bin` (parking_lot guards
        // aren't Send, and `start_process` re-locks these maps → deadlock).
        let ids: Vec<String> = self.agents.lock().keys().cloned().collect();
        for id in ids {
            match self.workspace.agent(&id) {
                Ok(r) if r.provider == provider_id => {}
                _ => continue, // wrong provider, or removed out from under us
            }
            self.respawn_agent_for_bin(app, &id).await;
        }
    }

    /// Tear down and restart one live agent so it execs the freshly resolved
    /// binary, resuming its existing session (`fresh = false`) so the
    /// transcript/conversation is preserved.
    ///
    /// The idle-check and the `agents` removal happen atomically under a single
    /// `agents` lock: a concurrent send can flip an agent Idle→Running on
    /// another thread (`transition_active` touches only `statuses`), so a
    /// separate check-then-remove would risk shutting down an in-flight turn.
    /// If the agent is mid-turn (Spawning/Running) we leave it running and flag
    /// it in `respawn_pending`; the next turn-end Idle transition retries it
    /// (see `transition_active`). This is what keeps the "swap binary → keep
    /// going" flow working for an agent that's busy at swap time.
    async fn respawn_agent_for_bin(self: &Arc<Self>, app: &AppHandle, agent_id: &str) {
        let record = match self.workspace.agent(agent_id) {
            Ok(r) => r,
            Err(_) => {
                self.respawn_pending.lock().remove(agent_id);
                return;
            }
        };
        // Atomic idle-check + remove. `busy` distinguishes "left running" from
        // "already gone" when no agent is taken.
        let mut busy = false;
        let taken = {
            let mut agents = self.agents.lock();
            if !agents.contains_key(agent_id) {
                None // gone — next spawn resolves the new binary anyway
            } else if matches!(
                self.effective_status(agent_id, &record),
                AgentStatus::Spawning | AgentStatus::Running
            ) {
                busy = true;
                None
            } else {
                agents.remove(agent_id)
            }
        };
        let agent = match taken {
            Some(agent) => agent,
            None if busy => {
                self.respawn_pending.lock().insert(agent_id.to_string());
                tracing::info!(agent_id, "binary-swap respawn deferred: agent busy");
                return;
            }
            None => {
                self.respawn_pending.lock().remove(agent_id);
                return;
            }
        };
        self.respawn_pending.lock().remove(agent_id);
        let _ = agent.shutdown();
        self.activities.lock().remove(agent_id);
        self.native_input_lines.lock().remove(agent_id);

        self.set_status(app, agent_id, AgentStatus::Spawning, None);
        arm_spawn_timeout(self.clone(), app.clone(), agent_id.to_string());

        // Let the old process fully release its session before resuming it
        // (mirrors `switch_view`).
        tokio::time::sleep(Duration::from_millis(150)).await;

        if let Err(e) = self.start_process(app, agent_id, false).await {
            let err = e.to_string();
            tracing::warn!(agent_id, error = %err, "binary-swap respawn failed");
            self.set_status(app, agent_id, AgentStatus::Error, Some(err));
            return;
        }

        // This respawn passed through a turn-end Idle where the normal queue
        // drain deferred to us (see `drain_message_queue`). Now that the agent
        // is back, deliver any follow-ups queued during that turn — unless the
        // user stopped (A2-A), which we own the interrupt check for here.
        if !self.interrupted.lock().remove(agent_id) {
            if let Err(e) = flush_queued(self, app, agent_id) {
                tracing::warn!(agent_id, error = %e, "post-respawn queue flush failed");
            }
        }
    }

    pub async fn stop_agent(self: Arc<Self>, app: AppHandle, agent_id: &str) -> Result<()> {
        // Interrupt the current turn. How it returns to Idle depends on
        // the runner: claude (managed) emits a `result` event and, if it
        // exits, `apply_exit_if_current` moves it to Idle; codex's
        // per-turn `exec` exits on SIGINT and its `on_turn_exit` handler
        // ends the turn (it emits no `turn.completed` when interrupted).
        let _ = app;
        // Mark the stop so the turn-end Idle transition keeps the queue intact
        // instead of auto-flushing it (A2-A). Cleared when a new turn starts.
        self.interrupted.lock().insert(agent_id.to_string());
        let agents = self.agents.lock();
        if let Some(agent) = agents.get(agent_id) {
            agent.interrupt();
        }
        Ok(())
    }

    /// Move an agent into the History view: stop the process if any,
    /// snapshot each tracked repo's SHA + diff stats, then tear down
    /// the worktrees and branches. The claude session JSONL is left
    /// alone — that's what makes restore possible.
    ///
    /// Rejects while the agent is actively spawning or running a turn.
    /// Idle agents are safe to archive; we shut down the waiting
    /// process before taking repo snapshots.
    pub async fn archive_agent(self: Arc<Self>, app: AppHandle, agent_id: &str) -> Result<()> {
        let record = self.workspace.agent(agent_id)?;
        if record.archive.is_some() {
            return Err(Error::Other("agent is already archived".into()));
        }
        if matches!(
            self.effective_status(agent_id, &record),
            AgentStatus::Spawning | AgentStatus::Running
        ) {
            return Err(Error::Other(
                "agent must be idle, stopped, or in error before archiving".into(),
            ));
        }

        self.detach_runtime(agent_id);

        // Snapshot SHAs + diff stats before any destructive step, then tear
        // down the worktrees/branches (best-effort — a single git failure
        // shouldn't block archive, since the user's intent is "get rid of
        // this").
        let (snapshots, diff_stats) = capture_repo_snapshots(agent_id, &record.repos).await;
        teardown_agent_worktrees(agent_id, &record.repos, "archive").await;

        let archive = ArchiveMetadata {
            archived_at: chrono::Utc::now().to_rfc3339(),
            repos: snapshots,
            diff_stats,
        };

        self.workspace.archive_agent(agent_id, archive)?;
        // The frontend listens to `agent:status` to drive most UI;
        // archive is structurally a deeper change, so we re-emit the
        // workspace via a tiny event. Frontend already reloads on this
        // signal via `get_workspace`.
        let _ = app.emit("workspace:changed", ());
        Ok(())
    }

    /// Pull an archived agent back into the live sidebar: recreate
    /// branches and worktrees from snapshot SHAs, clear archive
    /// metadata, transition to Spawning so the supervisor's start path
    /// attaches to the existing claude session.
    pub async fn restore_agent(self: Arc<Self>, app: AppHandle, agent_id: &str) -> Result<()> {
        let record = self.workspace.agent(agent_id)?;
        let archive = record
            .archive
            .clone()
            .ok_or_else(|| Error::Other("agent is not archived".into()))?;
        if record.session_id.is_none() {
            return Err(Error::Other(
                "archived agent has no session id; cannot restore".into(),
            ));
        }

        // Pre-flight: every snapshot must have a tip SHA, and that SHA
        // must still be reachable. We do this before any mutation so
        // we don't leave a half-restored agent on failure.
        for snap in &archive.repos {
            let sha = snap.branch_tip_sha.as_deref().ok_or_else(|| {
                Error::Other(format!(
                    "snapshot for repo `{}` has no branch tip SHA",
                    snap.subdir
                ))
            })?;
            git::rev_parse(&snap.repo_path, sha).await.map_err(|e| {
                Error::Other(format!(
                    "branch tip {} no longer reachable in {}: {e}",
                    sha,
                    snap.repo_path.display()
                ))
            })?;
        }

        // Ensure the agent parent dir exists.
        let parent_dir = agent_parent_dir(agent_id)?;
        tokio::fs::create_dir_all(&parent_dir)
            .await
            .map_err(|e| Error::Other(format!("create parent dir: {e}")))?;

        let mut restored: Vec<TrackedRepo> = Vec::with_capacity(archive.repos.len());
        for snap in &archive.repos {
            let tip_sha = snap.branch_tip_sha.as_deref().expect("checked above");

            let worktree = repo_worktree_path(agent_id, &snap.subdir)?;
            if let Some(parent) = worktree.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| Error::Other(format!("create worktree parent: {e}")))?;
            }

            let branch = match &snap.branch_name {
                // The agent had pushed a branch → recreate it at the tip,
                // resolving name collisions with a -restored suffix.
                Some(desired_name) => {
                    let mut chosen = desired_name.clone();
                    let mut bumps = 0;
                    loop {
                        let exists =
                            git::branch_exists(&snap.repo_path, &chosen).await.unwrap_or(false);
                        if !exists {
                            break;
                        }
                        bumps += 1;
                        chosen = if bumps == 1 {
                            format!("{desired_name}-restored")
                        } else {
                            format!("{desired_name}-restored-{bumps}")
                        };
                    }
                    git::branch_create_at(&snap.repo_path, &chosen, tip_sha).await?;
                    git::worktree_add_branch(&snap.repo_path, &worktree, &chosen).await?;
                    Some(chosen)
                }
                // Branchless agent (never pushed) → restore detached at the
                // tip, ready to name its branch at the next push.
                None => {
                    git::worktree_add_detached(&snap.repo_path, &worktree, Some(tip_sha)).await?;
                    None
                }
            };

            restored.push(TrackedRepo {
                repo_path: snap.repo_path.clone(),
                subdir: snap.subdir.clone(),
                branch,
                parent_branch: snap.parent_branch.clone(),
                // The fork point persists in the worktrees row across
                // archive/restore (restore_agent doesn't clear base_sha), so
                // this literal value is never written back — None is a
                // placeholder to satisfy the struct.
                base_sha: None,
                // Likewise preserved in the worktrees row across restore;
                // placeholder to satisfy the struct.
                pr_number: None,
            });
        }

        self.workspace.restore_agent(agent_id, restored)?;
        self.set_status(&app, agent_id, AgentStatus::Spawning, None);
        let _ = app.emit("workspace:changed", ());

        // Restore is an explicit user action, so bring the process up now
        // (set_status(Spawning) above lets start_process promote to Idle).
        arm_spawn_timeout(self.clone(), app.clone(), agent_id.to_string());
        let sup = self.clone();
        let app_for_task = app.clone();
        let id_for_task = agent_id.to_string();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = sup.start_process(&app_for_task, &id_for_task, false).await {
                fail_spawn(&sup, &app_for_task, &id_for_task, e.to_string());
            }
        });

        Ok(())
    }

    /// Synchronously ingest the agent's transcript into session_records (used
    /// for lazy backfill when a session is opened with no records yet). `None`
    /// if the provider has no transcript reader.
    pub fn sync_session(&self, agent_id: &str) -> Option<usize> {
        sync_session_records(&self.workspace, agent_id)
    }

    /// Fire-and-forget transcript ingest at turn-end. Called from
    /// `transition_active` whenever any agent reaches Idle. Emits
    /// `session:records-appended` when new records land; WARNs once if a
    /// reader-backed agent ingests nothing.
    ///
    /// The polling shape depends on whether the agent's process persists across
    /// turns (see `SyncPoll`):
    /// - **Per-turn agents** (custom view) have *exited* by turn-end, so the
    ///   file is complete and quiescent — we just ride out any flush lag and stop
    ///   at the first non-empty read.
    /// - **Claude / native-view agents** keep the transcript file open, so the
    ///   final line can still be flushing. We poll until the file settles (two
    ///   consecutive reads add nothing) before trusting the turn is fully on disk.
    pub fn trigger_session_sync(&self, app: AppHandle, agent_id: String) {
        let workspace = self.workspace.clone();
        let persistent = workspace
            .agent(&agent_id)
            .map(|r| is_persistent_runner(&r))
            .unwrap_or(true);
        tauri::async_runtime::spawn(async move {
            // Immediate attempt, then fine-grained backoff (ms) to ride out flush
            // lag / detect settle. Reads are incremental (O(new)), so polling is
            // cheap even on long transcripts.
            let backoffs = [0u64, 150, 150, 150, 200, 300, 400, 600];
            let mut poll = SyncPoll::new(persistent);
            for wait in backoffs {
                if wait > 0 {
                    tokio::time::sleep(Duration::from_millis(wait)).await;
                }
                let result = sync_session_records(&workspace, &agent_id);
                if matches!(poll.observe(result), PollControl::Stop) {
                    break;
                }
            }
            if poll.should_emit() {
                let _ = app.emit(
                    "session:records-appended",
                    SessionRecordsAppendedPayload { agent_id },
                );
            } else if poll.reader_ingested_nothing() {
                tracing::warn!(
                    agent_id,
                    "session sync ingested 0 records after retries (transcript not found or unchanged)"
                );
            }
        });
    }

    /// Fetch the current PR state for an agent's primary repo and emit
    /// a `pr:state_changed` event. Runs as a background task — never blocks the caller.
    pub fn fetch_and_emit_pr_state(&self, app: AppHandle, agent_id: String) {
        let workspace = self.workspace.clone();
        tauri::async_runtime::spawn(async move {
            let record = match workspace.agent(&agent_id) {
                Ok(r) => r,
                Err(_) => return,
            };
            let repo = match record.repos.first() {
                Some(r) => r,
                None => return,
            };
            // Only fetch if there's a branch (agent may still be on detached HEAD)
            if repo.branch.is_none() {
                return;
            }
            let subdir = repo.subdir.clone();
            let worktree = match crate::workspace::repo_worktree_path(&agent_id, &subdir) {
                Ok(p) => p,
                Err(_) => return,
            };
            let state = if let Some(number) = repo.pr_number {
                // Known PR: fetch by number, never by branch. This is what keeps
                // PR identity bound to the agent rather than the recyclable
                // branch name.
                crate::gh::pr_view_number(&worktree, number as u32)
                    .await
                    .unwrap_or(None)
            } else {
                // No PR recorded yet. Discover one created out-of-band (agent ran
                // `gh pr create`, or it was opened on github.com), but only adopt
                // it if it's OPEN — a stale merged/closed PR sitting on a recycled
                // branch must not be claimed as this agent's. Once adopted we
                // persist the number so all later lookups go by number.
                match crate::gh::pr_view(&worktree).await.unwrap_or(None) {
                    Some(pr) if matches!(pr.state, crate::gh::PrStatus::Open) => {
                        if let Err(e) =
                            workspace.set_repo_pr_number(&agent_id, &subdir, pr.number as i64)
                        {
                            tracing::warn!(
                                error = %e,
                                agent_id = %agent_id,
                                pr = pr.number,
                                "pr discovery: failed to persist PR number"
                            );
                        }
                        Some(pr)
                    }
                    _ => None,
                }
            };
            let _ = app.emit(
                "pr:state_changed",
                PrStateChangedPayload { agent_id, state },
            );
        });
    }

    pub async fn discard_agent(self: Arc<Self>, agent_id: &str) -> Result<()> {
        let record = self.workspace.agent(agent_id).ok();
        let repos = record.as_ref().map(|r| r.repos.clone()).unwrap_or_default();

        self.detach_runtime(agent_id);
        teardown_agent_worktrees(agent_id, &repos, "discard").await;

        self.workspace.remove_agent(agent_id)?;
        Ok(())
    }

    /// Detach an agent's live runtime: shut down its process and drop its
    /// in-memory state (activity detector, status, native input buffer, shell,
    /// and run-panel session). Shared by archive and discard.
    fn detach_runtime(&self, agent_id: &str) {
        // Bump first: invalidates the watchdog/RPC-watcher loops and the
        // process-exit handler before `shutdown()` triggers the latter, so the
        // exit can't re-emit `Idle` for the agent we're tearing down.
        self.bump_generation(agent_id);
        if let Some(agent) = self.agents.lock().remove(agent_id) {
            let _ = agent.shutdown();
        }
        self.activities.lock().remove(agent_id);
        self.statuses.lock().remove(agent_id);
        self.native_input_lines.lock().remove(agent_id);
        self.message_queue.lock().clear(agent_id);
        self.interrupted.lock().remove(agent_id);
        self.shells.lock().remove(agent_id);
        if let Some(run) = self.runs.lock().remove(agent_id) {
            run.stop();
        }
    }
}

/// Snapshot each tracked repo's tip SHA + diff stats against its fork point,
/// returning the per-repo snapshots plus the aggregate add/delete totals.
///
/// Resolves SHAs without mutating anything, so callers can capture state before
/// any destructive teardown. The tip is the worktree's HEAD — works whether the
/// agent is on a branch or still detached (never pushed), so both restore from
/// the exact committed tip.
async fn capture_repo_snapshots(
    agent_id: &str,
    repos: &[TrackedRepo],
) -> (Vec<ArchivedRepoSnapshot>, DiffStats) {
    let mut snapshots: Vec<ArchivedRepoSnapshot> = Vec::with_capacity(repos.len());
    let mut total_adds: u32 = 0;
    let mut total_dels: u32 = 0;

    for repo in repos {
        let branch_tip_sha = match repo_worktree_path(agent_id, &repo.subdir) {
            Ok(wt) => git::rev_parse(&wt, "HEAD").await.ok(),
            Err(_) => None,
        };
        // Prefer the immutable fork point; only fall back to resolving the
        // parent branch name (which may have drifted) for pre-migration
        // agents that never captured a base_sha.
        let parent_branch_sha = match &repo.base_sha {
            Some(sha) => Some(sha.clone()),
            None => match &repo.parent_branch {
                Some(b) => git::rev_parse(&repo.repo_path, b).await.ok(),
                None => None,
            },
        };

        let mut adds = 0u32;
        let mut dels = 0u32;
        if let (Some(from), Some(to)) = (&parent_branch_sha, &branch_tip_sha) {
            if from != to {
                if let Ok((a, d)) = git::diff_shortstat(&repo.repo_path, from, to).await {
                    adds = a;
                    dels = d;
                }
            }
        }
        total_adds = total_adds.saturating_add(adds);
        total_dels = total_dels.saturating_add(dels);

        snapshots.push(ArchivedRepoSnapshot {
            repo_path: repo.repo_path.clone(),
            subdir: repo.subdir.clone(),
            branch_name: repo.branch.clone(),
            branch_tip_sha,
            parent_branch: repo.parent_branch.clone(),
            parent_branch_sha,
            diff_stats: DiffStats {
                additions: adds,
                deletions: dels,
            },
        });
    }

    (
        snapshots,
        DiffStats {
            additions: total_adds,
            deletions: total_dels,
        },
    )
}

/// Best-effort teardown of every tracked repo's worktree + branch, plus the
/// agent's parent dir. Failures are logged (tagged with `op` for context) but
/// never abort the sweep — the caller's intent is to get rid of the agent.
/// Shared by archive and discard.
async fn teardown_agent_worktrees(agent_id: &str, repos: &[TrackedRepo], op: &str) {
    for repo in repos {
        let worktree = match repo_worktree_path(agent_id, &repo.subdir) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, subdir = %repo.subdir, op, "worktree_path failed");
                continue;
            }
        };
        let _ = git::worktree_prune(&repo.repo_path).await;
        if let Err(e) = git::worktree_remove(&repo.repo_path, &worktree, true).await {
            tracing::warn!(error = %e, subdir = %repo.subdir, op, "worktree remove failed");
        }
        if let Some(branch) = &repo.branch {
            if let Err(e) = git::branch_delete(&repo.repo_path, branch).await {
                tracing::warn!(%branch, error = %e, op, "branch delete failed");
            }
        }
    }

    // Remove the parent dir (may still hold orphan files if any worktree
    // removal failed). Best-effort.
    if let Ok(parent) = agent_parent_dir(agent_id) {
        if parent.exists() {
            let _ = tokio::fs::remove_dir_all(&parent).await;
        }
    }
}

/// Does this agent keep its transcript file open across turns? Per-turn agents
/// in the custom view *exit* at each turn-end, so the file is complete and
/// quiescent the moment we sync. Everything else — claude, and any agent in the
/// native (PTY/TUI) view — holds the file open, so the final line may still be
/// flushing and we must poll until it settles.
fn is_persistent_runner(record: &AgentRecord) -> bool {
    let per_turn = per_turn_descriptor(&record.provider).is_some();
    !(per_turn && record.view == AgentView::Custom)
}

/// Whether the turn-end transcript poll should keep going.
#[derive(Debug, PartialEq, Eq)]
enum PollControl {
    Continue,
    Stop,
}

/// Decision logic for the turn-end transcript sync, split out from
/// `trigger_session_sync` so it's unit-testable without timers or the
/// filesystem. Fed each `sync_session_records` result (`None` = no reader,
/// `Some(n)` = n new records this pass).
///
/// The stop condition depends on whether the runner persists:
/// - **Non-persistent (per-turn, exited):** the file is complete, so stop at the
///   first non-empty read — earlier empty reads just ride out flush lag.
/// - **Persistent (claude / native):** the final line may still be flushing,
///   possibly after a gap, so keep polling until the file *settles* — two
///   consecutive reads that add nothing once we've started ingesting. A later
///   batch resets the counter, so a multi-phase flush (tool-result, then the
///   answer) is still captured this turn.
struct SyncPoll {
    persistent: bool,
    had_reader: bool,
    inserted: usize,
    stable_polls: u32,
}

impl SyncPoll {
    fn new(persistent: bool) -> Self {
        Self {
            persistent,
            had_reader: false,
            inserted: 0,
            stable_polls: 0,
        }
    }

    fn observe(&mut self, result: Option<usize>) -> PollControl {
        match result {
            None => PollControl::Stop, // no reader — nothing to wait for
            Some(0) => {
                self.had_reader = true;
                if self.inserted == 0 {
                    return PollControl::Continue; // not flushed yet — keep waiting
                }
                if !self.persistent {
                    return PollControl::Stop; // exited → first batch was the whole turn
                }
                self.stable_polls += 1;
                if self.stable_polls >= 2 {
                    PollControl::Stop // file quiet for two polls → settled
                } else {
                    PollControl::Continue
                }
            }
            Some(n) => {
                self.had_reader = true;
                self.inserted += n;
                self.stable_polls = 0; // new content → not settled
                if self.persistent {
                    PollControl::Continue
                } else {
                    PollControl::Stop // exited → the batch is complete
                }
            }
        }
    }

    fn should_emit(&self) -> bool {
        self.inserted > 0
    }

    fn reader_ingested_nothing(&self) -> bool {
        self.had_reader && self.inserted == 0
    }
}

/// Locate the claude session JSONL by scanning the candidate `projects/*/`
/// dirs (see [`claude_projects_dirs`]) for `<session-id>.jsonl`. Claude's
/// path-encoding scheme isn't part of its public API, so we glob instead of
/// recomputing the encoded directory name from the worktree path.
/// Ingest the agent's on-disk transcript into `session_records`, idempotent per
/// `native_id`. `None` = no transcript reader for this provider (skip, don't
/// retry); `Some(n)` = reader ran, `n` new records inserted (`0` = nothing yet:
/// file not flushed, or its location/format changed).
fn sync_session_records(workspace: &WorkspaceManager, agent_id: &str) -> Option<usize> {
    let record = workspace.agent(agent_id).ok()?;
    let reader = crate::agent::transcript_reader(&record.provider)?;

    // A reader exists; from here any shortfall is "nothing yet" → Some(0).
    let Some(repo) = record.repos.first() else {
        return Some(0);
    };
    let Ok(cwd) = repo_worktree_path(agent_id, &repo.subdir) else {
        return Some(0);
    };

    // Resolve the session id. Event-stream agents have it on the record already;
    // plaintext agents (agy) read it from the filesystem at turn-end — persist
    // it here so the next turn can resume.
    let session_id = match record.session_id.clone() {
        Some(id) => id,
        None => {
            let captured = per_turn_descriptor(&record.provider)
                .and_then(|d| d.session_id_from_cwd)
                .and_then(|f| f(&cwd));
            match captured {
                Some(id) => {
                    let _ = workspace.set_agent_session_id(agent_id, &id);
                    id
                }
                None => return Some(0),
            }
        }
    };

    let paths = (reader.locate)(&session_id, &cwd);

    // Version-frozen snapshot tag (memoized probe — at most one --version per
    // provider per process).
    let version = crate::agent::cached_provider_version(&record.provider);

    // Read only what's new. Single-file JSONL readers tail from the stored byte
    // offset (O(new), not O(conversation) — the key win for long claude/image
    // sessions); multi-file / blob-dir readers fall back to a full read whose
    // already-stored rows are idempotently skipped. Either way the batch lands
    // in one transaction.
    // Per-turn agents in Custom view have exited by turn-end, so their final
    // line is complete even without a trailing newline (cursor/pi write it that
    // way) — consume it. Persistent writers (claude) keep the file open, so a
    // trailing line may be mid-write; hold it until it's newline-terminated.
    let consume_trailing = !is_persistent_runner(&record);
    let (records, new_offset) = match (reader.tail, paths.as_slice()) {
        (Some(tail), [path]) => {
            let offset = workspace.session_ingest_offset(agent_id).unwrap_or(0);
            let start_index = workspace.session_record_count(agent_id).unwrap_or(0);
            let (recs, next) = crate::agent::read_jsonl_tail(
                path,
                offset,
                start_index,
                tail.id_field,
                consume_trailing,
            );
            (recs, Some(next))
        }
        _ => ((reader.read)(&paths), None),
    };

    let batch: Vec<(&str, &serde_json::Value)> =
        records.iter().map(|r| (r.native_id.as_str(), &r.body)).collect();
    let inserted = match workspace.append_session_records(
        agent_id,
        &record.provider,
        "transcript",
        version.as_deref(),
        &batch,
    ) {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(error = %e, agent_id, "append_session_records failed");
            0
        }
    };

    // Advance the tail cursor past the complete lines we just consumed (only for
    // the single-file readers; `None` leaves it untouched).
    if let Some(next) = new_offset {
        if let Err(e) = workspace.set_session_ingest_offset(agent_id, next) {
            tracing::warn!(error = %e, agent_id, "persist ingest offset failed");
        }
    }

    // Link any pending outgoing user turns to the canonical transcript
    // user-message rows just ingested (fills in their `native_id`).
    if let Err(e) = workspace.associate_pending_user_turns(agent_id) {
        tracing::warn!(error = %e, agent_id, "associate user turns failed");
    }

    Some(inserted)
}

pub(crate) fn find_session_jsonl(session_id: &str) -> Option<PathBuf> {
    find_session_jsonl_in(&claude_projects_dirs(), session_id)
}

/// Candidate `projects` directories Claude may have written transcripts to.
/// Honors `CLAUDE_CONFIG_DIR` (Claude CLI's own config-dir override) and always
/// also includes the default `~/.claude`, so a transcript is located regardless
/// of which config dir was active when the agent was spawned. Mirrors the way
/// `find_codex_rollouts` honors `CODEX_HOME` — without this, an agent spawned
/// with `CLAUDE_CONFIG_DIR` set wrote its transcript somewhere we never scanned,
/// so it was never ingested into `session_records` and was lost when that dir
/// moved.
fn claude_projects_dirs() -> Vec<PathBuf> {
    projects_dirs_from(
        std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from),
        dirs::home_dir(),
    )
}

/// Pure core of [`claude_projects_dirs`]: configured dir first (if any), then
/// the default `~/.claude`, deduped so an explicit `CLAUDE_CONFIG_DIR=~/.claude`
/// doesn't double-scan.
fn projects_dirs_from(config_dir: Option<PathBuf>, home: Option<PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |base: PathBuf| {
        let p = base.join("projects");
        if !out.contains(&p) {
            out.push(p);
        }
    };
    if let Some(cfg) = config_dir {
        push(cfg);
    }
    if let Some(home) = home {
        push(home.join(".claude"));
    }
    out
}

/// Scan the given `projects` dirs for `<session-id>.jsonl`, returning the first
/// match. A missing/unreadable candidate dir is skipped, not fatal, so a later
/// candidate is still searched.
fn find_session_jsonl_in(projects_dirs: &[PathBuf], session_id: &str) -> Option<PathBuf> {
    let filename = format!("{session_id}.jsonl");
    for projects in projects_dirs {
        let Ok(entries) = std::fs::read_dir(projects) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path().join(&filename);
            if path.exists() {
                return Some(path);
            }
        }
    }
    None
}

/// All of codex's rollout files for a thread id, ordered (filenames are
/// timestamp-prefixed, so lexical sort == chronological). Codex stores sessions
/// at `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<ts>-<id>.jsonl` (CODEX_HOME
/// defaults to `~/.codex`); the id suffix is the thread id we captured. Resume
/// normally keeps one file per session, but returning all is correct if it splits.
pub(crate) fn find_codex_rollouts(session_id: &str) -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".codex")))
    else {
        return Vec::new();
    };
    // Anchor on the `-<id>.jsonl` boundary (filenames are
    // `rollout-<ts>-<id>.jsonl`) so one thread id can't match another whose
    // name merely ends with the same characters.
    let suffix = format!("-{session_id}.jsonl");
    // Walk the YYYY/MM/DD tree (three dir levels) and match the suffix.
    fn dirs_in(p: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(p)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect()
    }
    let sessions = home.join("sessions");
    let mut out = Vec::new();
    for year in dirs_in(&sessions) {
        for month in dirs_in(&year) {
            for day in dirs_in(&month) {
                for entry in std::fs::read_dir(&day).into_iter().flatten().flatten() {
                    let path = entry.path();
                    if path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.ends_with(&suffix))
                    {
                        out.push(path);
                    }
                }
            }
        }
    }
    out.sort();
    out
}

/// Read a JSONL file into a vec of parsed values, skipping blank or
/// unparseable lines.
pub(crate) fn read_jsonl_values(path: &Path) -> Result<Vec<Value>> {
    use std::io::BufRead;
    let file = std::fs::File::open(path)
        .map_err(|e| Error::Other(format!("open transcript: {e}")))?;
    let reader = std::io::BufReader::new(file);
    let mut out = Vec::new();
    for line in reader.lines().map_while(std::result::Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            out.push(v);
        }
    }
    Ok(out)
}

/// Process-exit outcomes that feed `apply_exit_if_current`. `PtyExit` and
/// `ManagedExit` are distinct types but carry the same fields, so this trait
/// lets `make_exit_handler` cover both spawners with one closure.
trait ExitOutcome {
    fn into_parts(self) -> (bool, String);
}

impl ExitOutcome for crate::pty_session::PtyExit {
    fn into_parts(self) -> (bool, String) {
        (self.success, self.message)
    }
}

impl ExitOutcome for crate::managed_session::ManagedExit {
    fn into_parts(self) -> (bool, String) {
        (self.success, self.message)
    }
}

/// Raw-byte output callback shared by the PTY spawners: record activity, then
/// emit `agent:output`.
fn make_output_handler(
    sup: Arc<Supervisor>,
    app: AppHandle,
    agent_id: String,
) -> impl Fn(Vec<u8>) + Send + Sync + 'static {
    move |bytes: Vec<u8>| {
        if let Some(activity) = sup.activities.lock().get_mut(&agent_id) {
            activity.observe_bytes(&bytes);
        }

        if let Err(e) = app.emit(
            "agent:output",
            AgentOutputPayload {
                agent_id: agent_id.clone(),
                bytes,
            },
        ) {
            tracing::warn!(error = %e, agent_id = %agent_id, "emit agent:output failed");
        }
    }
}

/// Parsed-JSON event callback shared by the managed + per-turn spawners:
/// record activity, then emit `agent:event`.
fn make_event_handler(
    sup: Arc<Supervisor>,
    app: AppHandle,
    agent_id: String,
) -> impl Fn(Value) + Send + Sync + 'static {
    move |event: Value| {
        if let Some(activity) = sup.activities.lock().get_mut(&agent_id) {
            activity.observe_event(&event);
        }

        if let Err(e) = app.emit(
            "agent:event",
            AgentEventPayload {
                agent_id: agent_id.clone(),
                event,
            },
        ) {
            tracing::warn!(error = %e, agent_id = %agent_id, "emit agent:event failed");
        }
    }
}

/// Process-exit callback shared by the pty/managed spawners: hand the outcome
/// to `apply_exit_if_current`, which ignores exits from a stale generation.
fn make_exit_handler<E: ExitOutcome>(
    sup: Arc<Supervisor>,
    app: AppHandle,
    agent_id: String,
    gen: u64,
) -> impl Fn(E) + Send + Sync + 'static {
    move |exit: E| {
        let (success, message) = exit.into_parts();
        apply_exit_if_current(&sup, &app, &agent_id, gen, success, message);
    }
}

fn spawn_pty_agent(
    spec: SpawnSpec<'_>,
    app: AppHandle,
    agent_id: String,
    sup: Arc<Supervisor>,
    gen: u64,
) -> Result<Agent> {
    Agent::spawn_pty(
        spec,
        make_output_handler(sup.clone(), app.clone(), agent_id.clone()),
        make_exit_handler(sup, app, agent_id, gen),
    )
}

/// Native (PTY/TUI) view for a per-turn agent. Same byte/exit wiring as
/// `spawn_pty_agent` (claude), but launches the agent's own binary via
/// `Agent::spawn_pty_native` rather than running claude under sandbox-exec.
fn spawn_pty_per_turn_agent(
    spec: SpawnSpec<'_>,
    provider: String,
    app: AppHandle,
    agent_id: String,
    sup: Arc<Supervisor>,
    gen: u64,
) -> Result<Agent> {
    Agent::spawn_pty_native(
        spec,
        &provider,
        make_output_handler(sup.clone(), app.clone(), agent_id.clone()),
        make_exit_handler(sup, app, agent_id, gen),
    )
}

fn spawn_managed_agent(
    spec: SpawnSpec<'_>,
    app: AppHandle,
    agent_id: String,
    sup: Arc<Supervisor>,
    gen: u64,
) -> Result<Agent> {
    Agent::spawn_managed(
        spec,
        make_event_handler(sup.clone(), app.clone(), agent_id.clone()),
        make_exit_handler(sup, app, agent_id, gen),
    )
}

/// Build a per-turn agent (codex, cursor). Their process exits at the end
/// of *every* turn — that's normal, not the agent dying — so unlike the
/// pty/managed spawners we don't wire `apply_exit_if_current` (which would
/// remove the agent from the map). Instead the per-turn exit is reported
/// via `on_turn_exit`, which ends the turn (Idle) without tearing the
/// agent down. This covers turns that exit without an in-band turn-end
/// event (interrupt, crash) so the agent doesn't sit Running until the
/// silence backstop. The session-id callback persists the id the agent
/// assigns on its first turn so later turns (and re-attach after restart)
/// resume it.
fn spawn_per_turn_agent(
    provider: &str,
    spec: PerTurnSpec,
    app: AppHandle,
    agent_id: String,
    sup: Arc<Supervisor>,
    gen: u64,
) -> Result<Agent> {
    let id_for_sid = agent_id.clone();
    let sup_for_sid = sup.clone();
    let app_for_exit = app.clone();
    let id_for_exit = agent_id.clone();
    let sup_for_exit = sup.clone();

    let on_event = make_event_handler(sup, app, agent_id);
    let on_session_id = move |sid: String| {
        if let Err(e) = sup_for_sid.workspace.set_agent_session_id(&id_for_sid, &sid) {
            tracing::warn!(error = %e, agent_id = %id_for_sid, "persist session id failed");
        }
    };
    let on_turn_exit = move |exit: crate::exec_session::ExecExit| {
        // The turn's process exited. Ignore if a respawn/teardown has since
        // bumped the generation (e.g. the session was dropped).
        let current = sup_for_exit
            .generations
            .lock()
            .get(&id_for_exit)
            .copied()
            .unwrap_or(0);
        if current != gen {
            return;
        }
        // End the turn. Idempotent with the in-band turn-end watchdog path.
        // User Stop is an expected non-zero exit; a non-interrupted failure
        // before the CLI emits JSON is a real crash and must be surfaced.
        if exit.success || exit.interrupted {
            transition_active(&sup_for_exit, &app_for_exit, &id_for_exit, AgentStatus::Idle);
        } else {
            sup_for_exit.agents.lock().remove(&id_for_exit);
            sup_for_exit.activities.lock().remove(&id_for_exit);
            sup_for_exit.native_input_lines.lock().remove(&id_for_exit);
            sup_for_exit.trigger_session_sync(app_for_exit.clone(), id_for_exit.clone());
            sup_for_exit.set_status(
                &app_for_exit,
                &id_for_exit,
                AgentStatus::Error,
                Some(format!("Agent process exited: {}", exit.message)),
            );
        }
    };

    let desc = per_turn_descriptor(provider).ok_or_else(|| {
        Error::Other(format!("unknown per-turn agent provider: {provider}"))
    })?;
    Agent::spawn_per_turn(desc, spec, on_event, on_session_id, on_turn_exit)
}

fn observe_native_input(sup: &Supervisor, agent_id: &str, bytes: &[u8]) -> Vec<String> {
    let mut submitted = Vec::new();
    let mut lines = sup.native_input_lines.lock();
    let line = lines.entry(agent_id.to_string()).or_default();
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'\r' | b'\n' => {
                let trimmed = line.trim().to_string();
                line.clear();
                if !trimmed.is_empty() {
                    submitted.push(trimmed);
                }
                i += 1;
            }
            0x7f | 0x08 => {
                line.pop();
                i += 1;
            }
            0x03 | 0x15 => {
                line.clear();
                i += 1;
            }
            0x1b => {
                i = skip_escape_sequence(bytes, i);
            }
            b if b < 0x20 => {
                i += 1;
            }
            _ => match std::str::from_utf8(&bytes[i..]) {
                Ok(rest) => {
                    if let Some(ch) = rest.chars().next() {
                        line.push(ch);
                        i += ch.len_utf8();
                    } else {
                        break;
                    }
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    if valid > 0 {
                        if let Ok(s) = std::str::from_utf8(&bytes[i..i + valid]) {
                            if let Some(ch) = s.chars().next() {
                                line.push(ch);
                                i += ch.len_utf8();
                            } else {
                                i += valid;
                            }
                        } else {
                            i += valid;
                        }
                    } else {
                        i += 1;
                    }
                }
            },
        }
    }

    submitted
}

fn skip_escape_sequence(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    if i < bytes.len() && bytes[i] == b'[' {
        i += 1;
        while i < bytes.len() {
            let b = bytes[i];
            i += 1;
            if (0x40..=0x7e).contains(&b) {
                break;
            }
        }
        return i;
    }
    if i < bytes.len() {
        i + 1
    } else {
        i
    }
}

/// Fire-and-forget handler for the user's first message: persists it
/// as the agent's `task`. No branch is created here — the worktree stays
/// detached until the first push, when the agent names its branch (see
/// `open_pr`/`git_push`).
fn on_first_user_message(
    sup: Arc<Supervisor>,
    app: AppHandle,
    agent_id: String,
    text: String,
) {
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        return;
    }
    if trimmed.starts_with('/') {
        return;
    }

    match sup.workspace.set_agent_task_if_empty(&agent_id, &trimmed) {
        Ok(true) => {
            let _ = app.emit(
                "agent:task",
                AgentTaskPayload {
                    agent_id: agent_id.clone(),
                    task: trimmed.clone(),
                },
            );
        }
        Ok(false) => {} // task already set
        Err(e) => {
            tracing::warn!(error = %e, agent_id = %agent_id, "set_agent_task_if_empty failed");
        }
    }
}

fn mark_user_turn_started(
    sup: &Supervisor,
    app: &AppHandle,
    agent_id: &str,
    turn_id: Option<&str>,
) {
    // A new turn is starting, so any prior stop is moot: clear the interrupt
    // flag so this turn's natural completion flushes queued follow-ups.
    sup.interrupted.lock().remove(agent_id);
    if let Some(activity) = sup.activities.lock().get_mut(agent_id) {
        activity.reset_for_new_turn();
    }
    // Stamp the turn's run start with a single timestamp shared by the persisted
    // row and the `turn:started` event, so the live timer and the footer measure
    // from the identical instant. Native PTY turns have no fletch-origin row (no
    // turn_id), so they carry no persisted timing — but still emit the event so
    // their live timer has an anchor.
    let started_at = chrono::Utc::now().timestamp_millis();
    if let Some(turn_id) = turn_id {
        if let Err(e) = sup.workspace.mark_user_turn_started(turn_id, started_at) {
            tracing::warn!(error = %e, agent_id, "stamp user turn start failed");
        }
    }
    let _ = app.emit(
        "turn:started",
        TurnStartedPayload {
            agent_id: agent_id.to_string(),
            started_at,
        },
    );
    transition_active(sup, app, agent_id, AgentStatus::Running);
}

/// Deliver a single message as a fresh turn: persist it durably, hand it to the
/// agent, and mark the turn started. The pre-existing send path, now shared by
/// the direct-send and queue-flush routes.
fn deliver_as_turn(
    sup: &Arc<Supervisor>,
    app: &AppHandle,
    agent_id: &str,
    msg: &PendingMsg,
) -> Result<()> {
    sup.deliver_user_message(
        agent_id,
        &msg.turn_id,
        &msg.text,
        &msg.attachments,
        msg.thinking.as_deref(),
    )?;
    mark_user_turn_started(sup, app, agent_id, Some(&msg.turn_id));
    on_first_user_message(sup.clone(), app.clone(), agent_id.to_string(), msg.text.clone());
    Ok(())
}

/// Coalesce every queued follow-up for an agent into one prompt and deliver it
/// as the next turn. No-op if the queue is empty. Persists a single
/// `session_user_turns` row (the coalesced message's `turn_id`), so the matcher
/// stays 1→1 with the one transcript record the turn produces.
fn flush_queued(sup: &Arc<Supervisor>, app: &AppHandle, agent_id: &str) -> Result<()> {
    let count = sup.message_queue.lock().len(agent_id);
    let Some(coalesced) = sup.message_queue.lock().drain_coalesced(agent_id) else {
        return Ok(());
    };
    if count > 1 {
        tracing::debug!(agent_id, count, "flushing coalesced follow-up messages as one turn");
    }
    if let Err(e) = deliver_as_turn(sup, app, agent_id, &coalesced) {
        // Delivery raced with teardown/respawn (e.g. AgentNotFound). Put the
        // follow-ups back rather than dropping them; a later boundary or the
        // post-respawn flush retries. Re-queue at the front to preserve order.
        tracing::warn!(error = %e, agent_id, "flush delivery failed; re-queueing follow-ups");
        sup.message_queue.lock().requeue_front(agent_id, coalesced);
    }
    Ok(())
}

/// At a turn-end Idle transition, flush any queued follow-up messages as the
/// next turn — but only on a *natural* completion. Order of the guards matters:
///
/// 1. A pending binary-swap respawn owns the flush (and the interrupt check):
///    it tears down and restarts the agent, then flushes once it's ready (see
///    `respawn_agent_for_bin`). Flushing here would race that teardown and
///    `AgentNotFound` could drop the queue. The flag is still set at this point
///    — `transition_active` calls us synchronously right after
///    `drain_pending_bin_respawn`, before its spawned task clears it.
/// 2. A user stop converges on this same Idle (the dying process emits its
///    result), so when the interrupt flag is set we clear it and keep the queue
///    intact (A2-A: stop never auto-sends).
///
/// Spawns the flush because `transition_active` holds only `&Supervisor`, and
/// the delivery needs an owned `Arc` (recovered from Tauri state, like
/// `drain_pending_bin_respawn`).
fn drain_message_queue(sup: &Supervisor, app: &AppHandle, agent_id: &str) {
    if sup.respawn_pending.lock().contains(agent_id) {
        return;
    }
    if sup.interrupted.lock().remove(agent_id) {
        return;
    }
    if sup.message_queue.lock().is_empty(agent_id) {
        return;
    }
    let Some(sup_arc) = app.try_state::<Arc<Supervisor>>().map(|s| s.inner().clone()) else {
        return;
    };
    let app = app.clone();
    let agent_id = agent_id.to_string();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = flush_queued(&sup_arc, &app, &agent_id) {
            tracing::warn!(error = %e, agent_id, "flush queued follow-up messages failed");
        }
    });
}

fn spawn_turn_watchdog(
    sup: Arc<Supervisor>,
    app: AppHandle,
    agent_id: String,
    gen: u64,
) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(WATCHDOG_TICK).await;

            let current_gen = sup
                .generations
                .lock()
                .get(&agent_id)
                .copied()
                .unwrap_or(0);
            if current_gen != gen {
                return;
            }

            let ended = sup
                .activities
                .lock()
                .get(&agent_id)
                .map(|a| a.turn_ended())
                .unwrap_or(false);

            if ended {
                transition_active(&sup, &app, &agent_id, AgentStatus::Idle);
            }
        }
    });
}

/// Drive the agent's file-mailbox RPC for the life of this generation: each
/// tick, execute any pending requests and write responses. Gen-guarded like
/// `spawn_turn_watchdog`, so it exits cleanly when the agent is respawned or
/// torn down (no explicit handle to track). Polling (no `notify` crate) mirrors
/// the transcript-sync style already used elsewhere.
fn spawn_rpc_watcher(
    sup: Arc<Supervisor>,
    app: AppHandle,
    agent_id: String,
    dispatcher: Arc<dyn rpc::RpcDispatcher>,
    rpc_dir: PathBuf,
    gen: u64,
) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(RPC_TICK).await;

            let current_gen = sup
                .generations
                .lock()
                .get(&agent_id)
                .copied()
                .unwrap_or(0);
            if current_gen != gen {
                return;
            }

            let events = rpc::process_pending(&rpc_dir, dispatcher.as_ref()).await;
            for event in events {
                match event {
                    rpc::RpcEvent::Named { name, payload }
                        if name == rpc::git::EVENT_BRANCH_CREATED =>
                    {
                        let Some(branch) = payload.get("branch").and_then(|v| v.as_str()) else {
                            tracing::warn!(
                                event = %name,
                                payload = %payload,
                                "git dispatcher emitted branch event without branch"
                            );
                            continue;
                        };
                        if let Ok(record) = sup.workspace.agent(&agent_id) {
                            if let Some(repo) = record.repos.first() {
                                if let Err(e) = sup.workspace.set_repo_branch(
                                    &agent_id,
                                    &repo.subdir,
                                    branch,
                                ) {
                                    tracing::warn!(
                                        error = %e,
                                        agent_id = %agent_id,
                                        branch = %branch,
                                        "git_push/open_pr: failed to persist branch name"
                                    );
                                } else {
                                    let _ = app.emit(
                                        "agent:branch",
                                        AgentBranchPayload {
                                            agent_id: agent_id.clone(),
                                            subdir: repo.subdir.clone(),
                                            branch: branch.to_string(),
                                        },
                                    );
                                }
                            }
                        }
                    }
                    rpc::RpcEvent::Named { name, payload }
                        if name == rpc::git::EVENT_PR_OPENED =>
                    {
                        let Some(number) = payload.get("number").and_then(|v| v.as_u64()) else {
                            tracing::warn!(
                                event = %name,
                                payload = %payload,
                                "git dispatcher emitted PR event without number"
                            );
                            continue;
                        };
                        if let Ok(record) = sup.workspace.agent(&agent_id) {
                            if let Some(repo) = record.repos.first() {
                                if let Err(e) = sup.workspace.set_repo_pr_number(
                                    &agent_id,
                                    &repo.subdir,
                                    number as i64,
                                ) {
                                    tracing::warn!(
                                        error = %e,
                                        agent_id = %agent_id,
                                        pr = number,
                                        "open_pr: failed to persist PR number"
                                    );
                                }
                            }
                        }
                        sup.fetch_and_emit_pr_state(app.clone(), agent_id.clone());
                    }
                    rpc::RpcEvent::Named { name, payload }
                        if name == rpc::git::EVENT_ACTION_DONE =>
                    {
                        // Authoritative "the agent performed a git mutation this
                        // turn" signal. Forward it so the panel can attribute a
                        // git/PR transition to the turn rather than guessing from
                        // a polled snapshot.
                        let op = payload
                            .get("op")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        if let Err(e) = app.emit(
                            "agent:git-action",
                            AgentGitActionPayload {
                                agent_id: agent_id.clone(),
                                op,
                            },
                        ) {
                            tracing::warn!(error = %e, agent_id = %agent_id, "emit agent:git-action failed");
                        }
                    }
                    rpc::RpcEvent::Named { name, payload } => {
                        tracing::debug!(event = %name, payload = %payload, "rpc: unhandled event");
                    }
                }
            }
        }
    });
}

fn arm_spawn_timeout(sup: Arc<Supervisor>, app: AppHandle, agent_id: String) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(SPAWN_TIMEOUT).await;
        // Atomically claim the timeout outcome. Only an agent still in the
        // live Spawning state may be timed out; if the swap fails the spawn
        // already left Spawning (completed, or failed on its own) and must
        // not be killed. The compare-and-swap also closes the race with
        // start_process: if the spawn task inserts its process concurrently,
        // exactly one of us flips the status, and the loser tears down.
        let err = "Spawn timed out after 15s — process did not become ready.".to_string();
        if !sup.claim_spawn_outcome(&app, &agent_id, AgentStatus::Error, Some(err)) {
            return;
        }
        // Invalidate any gen-guarded loop / exit handler from this spawn before
        // killing the process (same reason as `detach_runtime`).
        sup.bump_generation(&agent_id);
        if let Some(agent) = sup.agents.lock().remove(&agent_id) {
            let _ = agent.shutdown();
        }
        sup.activities.lock().remove(&agent_id);
    });
}

fn fail_spawn(sup: &Supervisor, app: &AppHandle, agent_id: &str, err: String) {
    sup.set_status(app, agent_id, AgentStatus::Error, Some(err));
}

fn transition_active(
    sup: &Supervisor,
    app: &AppHandle,
    agent_id: &str,
    new: AgentStatus,
) {
    // Operate on the live in-memory status. A live agent with no entry yet
    // is treated as Spawning (the at-rest derivation).
    let cur = sup
        .live_status(agent_id)
        .unwrap_or(AgentStatus::Spawning);
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

/// If a binary-path change was deferred for this agent because it was
/// mid-turn (see `respawn_agent_for_bin`), now that it's Idle restart it onto
/// the new binary. No-op unless the agent is flagged. We recover the managed
/// `Arc<Supervisor>` from Tauri state because `transition_active` only holds
/// `&Supervisor`, and the respawn needs an owned `Arc` for its spawned task.
fn drain_pending_bin_respawn(sup: &Supervisor, app: &AppHandle, agent_id: &str) {
    if !sup.respawn_pending.lock().contains(agent_id) {
        return;
    }
    let Some(sup_arc) = app.try_state::<Arc<Supervisor>>().map(|s| s.inner().clone()) else {
        return;
    };
    let app = app.clone();
    let agent_id = agent_id.to_string();
    tauri::async_runtime::spawn(async move {
        sup_arc.respawn_agent_for_bin(&app, &agent_id).await;
    });
}

fn apply_exit_if_current(
    sup: &Supervisor,
    app: &AppHandle,
    agent_id: &str,
    gen: u64,
    success: bool,
    message: String,
) {
    let current = sup
        .generations
        .lock()
        .get(agent_id)
        .copied()
        .unwrap_or(0);
    if current != gen {
        tracing::debug!(
            agent_id = %agent_id,
            stale_gen = gen,
            current_gen = current,
            "ignoring exit from prior generation"
        );
        return;
    }

    sup.agents.lock().remove(agent_id);
    sup.activities.lock().remove(agent_id);
    sup.native_input_lines.lock().remove(agent_id);

    let (status, err) = if success {
        // Clean exit means the agent is resumable — keep it Idle so the
        // user can send follow-up messages without a manual Resume step.
        // The Idle entry stays in the `statuses` map (the agent is
        // resumable for the life of the session).
        (AgentStatus::Idle, None)
    } else {
        (AgentStatus::Error, Some(format!("Agent process exited: {message}")))
    };
    // Only apply the exit transition if the agent was still live (not
    // already moved to a terminal disposition by another path).
    let was_live = matches!(
        sup.live_status(agent_id).unwrap_or(AgentStatus::Spawning),
        AgentStatus::Running | AgentStatus::Idle | AgentStatus::Spawning
    );
    if was_live {
        sup.set_status(app, agent_id, status.clone(), err);
        if matches!(status, AgentStatus::Idle) {
            sup.fetch_and_emit_pr_state(app.clone(), agent_id.to_string());
        }
    }
}

fn emit_run_state(
    app: &AppHandle,
    agent_id: &str,
    phase: RunPhase,
    last_error: Option<String>,
) {
    let _ = app.emit(
        "run:state",
        RunStatePayload {
            agent_id: agent_id.to_string(),
            phase,
            last_error,
        },
    );
}

fn emit_run_output(app: &AppHandle, agent_id: &str, bytes: Vec<u8>) {
    let _ = app.emit(
        "run:output",
        RunOutputPayload {
            agent_id: agent_id.to_string(),
            bytes,
        },
    );
}

/// Inject a "$ <cmd>" header line into the log so each phase has a
/// visible boundary, then emit it like any other PTY output.
fn write_header(app: &AppHandle, agent_id: &str, session: &Arc<RunSession>, cmd: &str) {
    // Dim ANSI for the prompt — the frontend strips ANSI for v1,
    // so the line still reads fine without color support.
    let line = format!("\x1b[2m$ {cmd}\x1b[0m\r\n");
    let bytes = line.into_bytes();
    session.append_log(&bytes);
    emit_run_output(app, agent_id, bytes);
}

/// The phases to spawn for a single `run_start`, derived from the
/// resolved commands and whether setup has already completed.
#[derive(Debug)]
struct RunPlan {
    first_phase: RunPhase,
    first_cmd: String,
    /// Run command to chain after a successful setup phase. `None` when
    /// the first phase is already the run, or when there is no run
    /// command to chain (so we never spawn an empty command).
    chained_run_cmd: Option<String>,
}

/// Decide what to spawn. Returns `None` when there is nothing to run —
/// neither a setup nor a run command — so the caller can leave the
/// button Idle instead of spawning an empty command that would exit 0
/// and flash the panel to Stopped with no explanation.
fn plan_run_phases(setup_done: bool, setup_cmd: &str, run_cmd: &str) -> Option<RunPlan> {
    let needs_setup = !setup_done && !setup_cmd.trim().is_empty();
    let has_run_cmd = !run_cmd.trim().is_empty();
    if needs_setup {
        Some(RunPlan {
            first_phase: RunPhase::Setup,
            first_cmd: setup_cmd.to_string(),
            chained_run_cmd: has_run_cmd.then(|| run_cmd.to_string()),
        })
    } else if has_run_cmd {
        Some(RunPlan {
            first_phase: RunPhase::Running,
            first_cmd: run_cmd.to_string(),
            chained_run_cmd: None,
        })
    } else {
        None
    }
}

/// Spawn one phase's PTY (setup or run). Wires up output streaming
/// and the exit handler that chains setup→run or transitions to
/// Stopped on natural exit. Out-of-band stops are handled via the
/// generation check.
fn spawn_run_phase(
    sup: Arc<Supervisor>,
    app: AppHandle,
    agent_id: String,
    session: Arc<RunSession>,
    gen: u64,
    cwd: std::path::PathBuf,
    phase: RunPhase,
    cmd: String,
    chain_run_cmd: Option<String>,
) -> Result<()> {
    // Confine the run command to the worktree + toolchain caches. The command
    // string is repo-derived (package.json scripts, postinstall, dev-server
    // config), so a malicious agent could otherwise plant a script that runs
    // unsandboxed with full user privilege the moment the user clicks ▶. Reads
    // and network stay open (dev servers need them); only writes are fenced.
    let (program, args, profile_file) = sandboxed_run_command(&cwd, &cmd)?;

    let session_out = session.clone();
    let app_out = app.clone();
    let id_out = agent_id.clone();

    let sup_exit = sup.clone();
    let app_exit = app.clone();
    let id_exit = agent_id.clone();
    let session_exit = session.clone();
    let cwd_exit = cwd.clone();

    let pty = run_session::spawn_command(
        &program,
        &args,
        &cwd,
        move |bytes| {
            session_out.append_log(&bytes);
            emit_run_output(&app_out, &id_out, bytes);
        },
        move |exit| {
            handle_run_phase_exit(
                sup_exit.clone(),
                app_exit.clone(),
                id_exit.clone(),
                session_exit.clone(),
                gen,
                phase,
                exit,
                cwd_exit.clone(),
                chain_run_cmd.clone(),
            );
        },
    )?;

    session.attach_pty(pty, profile_file);
    Ok(())
}

/// Build the `sandbox-exec`-wrapped invocation for a Run-panel command:
/// `sandbox-exec -f <profile> <shell> -lic <cmd>`. Returns the program, argv,
/// and the profile tempfile. `sandbox-exec` reads the profile once, at the
/// child's `exec`, so the tempfile must survive until then; the caller parks it
/// on the `RunSession` (via `attach_pty`), which conservatively keeps it for the
/// process's lifetime.
fn sandboxed_run_command(
    cwd: &Path,
    cmd: &str,
) -> Result<(PathBuf, Vec<String>, tempfile::NamedTempFile)> {
    let home =
        dirs::home_dir().ok_or_else(|| Error::Other("HOME directory not available".into()))?;
    let profile_text = crate::sandbox::build_run_profile(cwd, &home)?;
    let profile_file = crate::sandbox::profile_tempfile(&profile_text)?;
    let profile_path = profile_file
        .path()
        .to_str()
        .ok_or_else(|| Error::Other("profile path not utf-8".into()))?
        .to_string();
    let shell = user_shell();
    let shell_str = shell
        .to_str()
        .ok_or_else(|| Error::Other("shell path not utf-8".into()))?
        .to_string();
    // sandbox-exec -f <profile> <shell> -lic <cmd>
    let mut args = vec!["-f".to_string(), profile_path, shell_str];
    args.extend(shell_args(cmd));
    Ok((
        PathBuf::from(crate::sandbox::SANDBOX_EXEC),
        args,
        profile_file,
    ))
}

fn handle_run_phase_exit(
    sup: Arc<Supervisor>,
    app: AppHandle,
    agent_id: String,
    session: Arc<RunSession>,
    gen: u64,
    phase: RunPhase,
    exit: crate::pty_session::PtyExit,
    cwd: std::path::PathBuf,
    chain_run_cmd: Option<String>,
) {
    // If the user clicked Stop (or started a fresh run), our
    // generation is stale — just drop this event.
    if !session.is_current_generation(gen) {
        tracing::debug!(
            agent_id = %agent_id,
            phase = ?phase,
            "ignoring stale run-phase exit"
        );
        return;
    }

    if matches!(phase, RunPhase::Setup) && exit.success {
        // Setup finished cleanly — persist the flag and chain into
        // the run command (if we have one).
        if let Err(e) = sup.workspace.mark_setup_completed(&agent_id) {
            tracing::warn!(error = %e, agent_id = %agent_id, "mark_setup_completed failed");
        }
        if let Some(run_cmd) = chain_run_cmd {
            session.transition_phase(RunPhase::Running);
            emit_run_state(&app, &agent_id, RunPhase::Running, None);
            write_header(&app, &agent_id, &session, &run_cmd);
            if let Err(e) = spawn_run_phase(
                sup,
                app.clone(),
                agent_id.clone(),
                session.clone(),
                gen,
                cwd,
                RunPhase::Running,
                run_cmd,
                None,
            ) {
                let msg = format!("Failed to start run command: {e}");
                session.mark_stopped(Some(msg.clone()));
                emit_run_state(&app, &agent_id, RunPhase::Stopped, Some(msg));
            }
            return;
        }
        // No run command to chain into — treat as clean stop.
        session.mark_stopped(None);
        emit_run_state(&app, &agent_id, RunPhase::Stopped, None);
        return;
    }

    // Setup failed → do NOT proceed to run. Surface the error.
    if matches!(phase, RunPhase::Setup) && !exit.success {
        let msg = format!("Setup failed: {}", exit.message);
        session.mark_stopped(Some(msg.clone()));
        emit_run_state(&app, &agent_id, RunPhase::Stopped, Some(msg));
        return;
    }

    // Run-phase exit — natural end or crash. Either way → Stopped.
    let err = if exit.success {
        None
    } else {
        Some(format!("Run exited: {}", exit.message))
    };
    session.mark_stopped(err.clone());
    emit_run_state(&app, &agent_id, RunPhase::Stopped, err);
}

fn emit_status(
    app: &AppHandle,
    agent_id: &str,
    status: AgentStatus,
    last_error: Option<String>,
) {
    let _ = app.emit(
        "agent:status",
        AgentStatusPayload {
            agent_id: agent_id.to_string(),
            status,
            last_error,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── plan_run_phases ───────────────────────────────────────────────────

    #[test]
    fn plan_runs_dev_directly_when_setup_done() {
        let plan = plan_run_phases(true, "pnpm install", "pnpm dev").unwrap();
        assert_eq!(plan.first_phase, RunPhase::Running);
        assert_eq!(plan.first_cmd, "pnpm dev");
        assert_eq!(plan.chained_run_cmd, None);
    }

    #[test]
    fn plan_runs_setup_then_chains_dev() {
        let plan = plan_run_phases(false, "pnpm install", "pnpm dev").unwrap();
        assert_eq!(plan.first_phase, RunPhase::Setup);
        assert_eq!(plan.first_cmd, "pnpm install");
        assert_eq!(plan.chained_run_cmd.as_deref(), Some("pnpm dev"));
    }

    #[test]
    fn plan_does_not_chain_into_empty_run_cmd() {
        // Setup needed but no dev command (e.g. a plain Python project with
        // an install but no recognized run). Setup runs alone — no empty
        // command chained after it.
        let plan = plan_run_phases(false, "pip install -r requirements.txt", "").unwrap();
        assert_eq!(plan.first_phase, RunPhase::Setup);
        assert_eq!(plan.chained_run_cmd, None);
    }

    #[test]
    fn plan_is_none_when_nothing_to_run() {
        // Wholly unrecognized ecosystem: no setup, no run. Nothing should
        // be spawned — the button stays Idle instead of flashing Stopped.
        assert!(plan_run_phases(true, "", "").is_none());
        assert!(plan_run_phases(false, "", "").is_none());
        assert!(plan_run_phases(false, "   ", "  ").is_none());
    }

    #[test]
    fn plan_skips_completed_setup_even_if_run_empty() {
        // Setup already done and no run command → nothing to do.
        assert!(plan_run_phases(true, "pnpm install", "").is_none());
    }

    #[test]
    fn plan_runs_only_run_cmd_when_no_setup_needed() {
        let plan = plan_run_phases(true, "", "cargo run").unwrap();
        assert_eq!(plan.first_phase, RunPhase::Running);
        assert_eq!(plan.first_cmd, "cargo run");
        assert_eq!(plan.chained_run_cmd, None);
    }

    // ── claude transcript location (CLAUDE_CONFIG_DIR) ────────────────────────

    #[test]
    fn projects_dirs_prefers_config_dir_then_default_home() {
        let dirs = projects_dirs_from(
            Some(PathBuf::from("/home/u/.claude-eve")),
            Some(PathBuf::from("/home/u")),
        );
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/home/u/.claude-eve/projects"),
                PathBuf::from("/home/u/.claude/projects"),
            ],
        );
    }

    #[test]
    fn projects_dirs_dedups_when_config_is_default() {
        // CLAUDE_CONFIG_DIR explicitly set to ~/.claude must not double-scan.
        let dirs = projects_dirs_from(
            Some(PathBuf::from("/home/u/.claude")),
            Some(PathBuf::from("/home/u")),
        );
        assert_eq!(dirs, vec![PathBuf::from("/home/u/.claude/projects")]);
    }

    #[test]
    fn projects_dirs_falls_back_to_home_when_config_unset() {
        let dirs = projects_dirs_from(None, Some(PathBuf::from("/home/u")));
        assert_eq!(dirs, vec![PathBuf::from("/home/u/.claude/projects")]);
    }

    #[test]
    fn find_session_jsonl_locates_transcript_in_relocated_config_dir() {
        // Regression: the locator used to hardcode `~/.claude/projects`, so an
        // agent spawned with CLAUDE_CONFIG_DIR pointing elsewhere wrote its
        // transcript to a dir we never scanned — it was never ingested and was
        // lost when that dir moved. The transcript must be found wherever the
        // configured projects dir is.
        let cfg = tempfile::tempdir().unwrap();
        let projects = cfg.path().join("projects");
        let slug = projects.join("-Users-alex--fletch-worktrees-transylvania-fletch");
        std::fs::create_dir_all(&slug).unwrap();
        let sid = "f90f9c57-6dd1-45a0-9b69-5b5963979d5b";
        let jsonl = slug.join(format!("{sid}.jsonl"));
        std::fs::write(&jsonl, b"{}\n").unwrap();

        let found = find_session_jsonl_in(&[projects], sid);
        assert_eq!(found.as_deref(), Some(jsonl.as_path()));
    }

    #[test]
    fn find_session_jsonl_skips_missing_dir_and_scans_the_next() {
        // A non-existent candidate dir (e.g. the default ~/.claude when only the
        // relocated config dir has the file) must not short-circuit the scan.
        let cfg = tempfile::tempdir().unwrap();
        let projects = cfg.path().join("projects");
        let slug = projects.join("slug");
        std::fs::create_dir_all(&slug).unwrap();
        let sid = "abc";
        let jsonl = slug.join(format!("{sid}.jsonl"));
        std::fs::write(&jsonl, b"{}\n").unwrap();

        let missing = cfg.path().join("does-not-exist");
        let found = find_session_jsonl_in(&[missing, projects], sid);
        assert_eq!(found.as_deref(), Some(jsonl.as_path()));
    }

    fn test_supervisor() -> Supervisor {
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

    fn record_with_status(id: &str, status: AgentStatus) -> AgentRecord {
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

    #[test]
    fn delivery_to_unready_agent_leaves_canonical_store_clean_but_captures_turn() {
        // A freshly spawned agent has a session row but isn't in the live agents
        // map yet (the frontend retries the send until it's ready). A failed
        // delivery must not touch the canonical transcript store — but the
        // outgoing user turn IS captured durably so it isn't lost and can be
        // retried.
        let sup = test_supervisor();
        let mut record = record_with_status("yosemite", AgentStatus::Spawning);
        sup.workspace.add_agent(&mut record).unwrap();

        let err = sup
            .deliver_user_message("yosemite", "turn-1", "hello", &[], None)
            .unwrap_err();
        assert!(matches!(err, Error::AgentNotFound(_)));

        // Canonical store untouched.
        let records = sup.workspace.read_session_records("yosemite").unwrap();
        assert!(
            records.is_empty(),
            "failed delivery must not write the canonical store, got {records:?}",
        );

        // Outgoing turn captured, pending (no transcript yet) → renders standalone.
        let turns = sup.workspace.read_user_turns("yosemite").unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].turn_id, "turn-1");
        assert_eq!(turns[0].text, "hello");
        assert_eq!(turns[0].native_id, None);
    }

    // ── SyncPoll: per-turn stops on first batch; persistent waits for settle ──

    #[test]
    fn sync_poll_per_turn_stops_at_first_non_empty_read() {
        // A per-turn agent has exited, so the file is complete: the first batch
        // is the whole turn. Empty reads before it just ride out flush lag.
        let mut poll = SyncPoll::new(false);
        assert_eq!(poll.observe(Some(0)), PollControl::Continue); // not flushed yet
        assert_eq!(poll.observe(Some(6)), PollControl::Stop); // complete — done
        assert!(poll.should_emit());
    }

    #[test]
    fn sync_poll_persistent_settles_after_two_quiet_polls() {
        // Claude keeps the file open; only stop once it's been quiet for two
        // consecutive reads after we started ingesting.
        let mut poll = SyncPoll::new(true);
        assert_eq!(poll.observe(Some(5)), PollControl::Continue);
        assert_eq!(poll.observe(Some(0)), PollControl::Continue); // quiet 1
        assert_eq!(poll.observe(Some(0)), PollControl::Stop); // quiet 2 → settled
        assert!(poll.should_emit());
    }

    #[test]
    fn sync_poll_persistent_captures_a_late_answer_across_a_gap() {
        // The live-evidence case: tool-result + bookkeeping flush first, then the
        // final answer a phase later (an empty read in between). A new batch
        // resets the settle counter, so the answer is still ingested this turn.
        let mut poll = SyncPoll::new(true);
        assert_eq!(poll.observe(Some(7)), PollControl::Continue);
        assert_eq!(poll.observe(Some(0)), PollControl::Continue); // gap, quiet 1
        assert_eq!(poll.observe(Some(2)), PollControl::Continue); // answer lands → reset
        assert_eq!(poll.observe(Some(0)), PollControl::Continue); // quiet 1
        assert_eq!(poll.observe(Some(0)), PollControl::Stop); // quiet 2 → settled
        assert!(poll.should_emit());
    }

    #[test]
    fn sync_poll_no_reader_stops_immediately() {
        let mut poll = SyncPoll::new(true);
        assert_eq!(poll.observe(None), PollControl::Stop);
        assert!(!poll.should_emit());
        assert!(!poll.reader_ingested_nothing());
    }

    #[test]
    fn sync_poll_reader_but_nothing_ingested_warns() {
        let mut poll = SyncPoll::new(true);
        for _ in 0..5 {
            assert_eq!(poll.observe(Some(0)), PollControl::Continue);
        }
        assert!(!poll.should_emit());
        assert!(poll.reader_ingested_nothing());
    }
}
