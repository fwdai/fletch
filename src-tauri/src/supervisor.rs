//! Coordinator between Tauri IPC commands and the running agents.

use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

use crate::activity::{Activity, ClaudeNativeActivity, ManagedActivity};
use crate::agent::{capabilities, per_turn_descriptor, Agent, PerTurnSpec, SpawnSpec};
use crate::branding;
use crate::error::{Error, Result};
use crate::git;
use crate::pty_session::{PtySession, PtySpawn};
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
        self.statuses
            .lock()
            .insert(agent_id.to_string(), status.clone());
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

        let (setup_cmd, run_cmd) = self.read_run_commands(&record.project_id);
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

        let needs_setup = !setup_done && !setup_cmd.trim().is_empty();
        let (first_phase, first_cmd, chains_to_run) = if needs_setup {
            (RunPhase::Setup, setup_cmd.clone(), true)
        } else {
            (RunPhase::Running, run_cmd.clone(), false)
        };

        let gen = session.begin_phase(first_phase);
        emit_run_state(&app, agent_id, first_phase, None);
        write_header(&app, agent_id, &session, &first_cmd);

        spawn_run_phase(
            self.clone(),
            app,
            agent_id.to_string(),
            session,
            gen,
            cwd,
            first_phase,
            first_cmd,
            if chains_to_run { Some(run_cmd) } else { None },
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

    /// Read the setup + run commands from project_settings, falling
    /// back to the same inferred defaults the panel UI shows. Keys
    /// match the RunPanel storage scheme (`run.install`, `run.dev`).
    fn read_run_commands(&self, project_id: &str) -> (String, String) {
        let conn = self.workspace.db_handle();
        let install_default = "pnpm install".to_string();
        let dev_default = "pnpm dev".to_string();
        if project_id.is_empty() {
            return (install_default, dev_default);
        }
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

        let primary = TrackedRepo {
            repo_path: repo_path.clone(),
            subdir: subdir.clone(),
            branch: None, // created later from first user message
            parent_branch,
        };

        let mut record = new_agent_record(
            agent_id.clone(),
            name,
            provider,
            primary,
            String::new(),
            view,
        );
        let parent_dir = agent_parent_dir(&agent_id)?;
        let primary_worktree = repo_worktree_path(&agent_id, &subdir)?;

        self.workspace.add_agent(&mut record)?;
        self.set_status(&app, &agent_id, AgentStatus::Spawning, None);
        arm_spawn_timeout(self.clone(), app.clone(), agent_id.clone());

        let sup = self.clone();
        let app_for_task = app.clone();
        let id_for_task = agent_id.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = std::fs::create_dir_all(&parent_dir) {
                fail_spawn(&sup, &app_for_task, &id_for_task, e.to_string());
                return;
            }

            if let Err(e) = git::worktree_add_detached(&repo_path, &primary_worktree).await {
                fail_spawn(&sup, &app_for_task, &id_for_task, e.to_string());
                return;
            }

            tokio::time::sleep(Duration::from_millis(350)).await;

            if let Err(e) = sup.start_process(&app_for_task, &id_for_task, true).await {
                let _ = git::worktree_remove(&repo_path, &primary_worktree, true).await;
                let _ = std::fs::remove_dir_all(&parent_dir);
                fail_spawn(&sup, &app_for_task, &id_for_task, e.to_string());
            }
        });

        Ok(record)
    }

    /// Bring a second (or third…) repo into a live agent. Creates a
    /// detached worktree at `~/.quorum/worktrees/<agent-id>/<subdir>/`
    /// and appends a TrackedRepo entry. If the agent already has a
    /// task set, a branch is created in the new repo immediately;
    /// otherwise we defer (consistent with the primary).
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

        git::worktree_add_detached(&repo_path, &worktree).await?;

        let repo = TrackedRepo {
            repo_path: repo_path.clone(),
            subdir: subdir.clone(),
            branch: None,
            parent_branch,
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

        // If the agent already has a task (slug), create the branch
        // in the freshly-added repo right away.
        let task = record.task.trim().to_string();
        if !task.is_empty() {
            create_branches_for_branchless_repos(
                self.clone(),
                app.clone(),
                agent_id.to_string(),
                task,
            );
        }

        Ok(repo)
    }

    async fn start_process(
        self: &Arc<Self>,
        app: &AppHandle,
        agent_id: &str,
        fresh: bool,
    ) -> Result<()> {
        let record = self.workspace.agent(agent_id)?;
        let provider = record.provider.clone();
        let per_turn = is_per_turn_provider(&provider);
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

        // Claude only writes a session file once the first turn lands.
        // If task is still empty (no first user message has ever been
        // sent) `--resume <uuid>` will 404. So we treat that case as
        // fresh — same UUID, no replay attempt — and the eventual
        // first message creates the session file. Once that's
        // happened, switch / resume can safely `--resume`.
        let no_messages_yet = record.task.trim().is_empty();
        let effective_fresh = fresh || no_messages_yet;

        let app = app.clone();
        let agent_id_str = agent_id.to_string();

        let my_gen = {
            let mut g = self.generations.lock();
            let entry = g.entry(agent_id_str.clone()).or_insert(0);
            *entry += 1;
            *entry
        };

        // Per-turn agents carry their turn-end detector in the descriptor
        // table — but only for the Custom (exec/JSON) view. In the native
        // view they run their interactive TUI in a PTY with no JSON stream,
        // so turn-end is detected by silence, the same as claude's native
        // view. Claude (no descriptor) picks its detector by view too.
        let mut activity: Box<dyn Activity> = match per_turn_descriptor(&provider) {
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
        self.activities.lock().insert(agent_id_str.clone(), activity);

        let agent = if per_turn {
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
                        sandbox_root: agent_parent_dir(agent_id)?,
                        session_id,
                        // Per-turn native always resumes (the agent built its
                        // session in the Custom view first).
                        fresh: false,
                        cols: 120,
                        rows: 32,
                    };
                    spawn_pty_per_turn_agent(
                        spec,
                        provider.clone(),
                        app.clone(),
                        agent_id_str.clone(),
                        self.clone(),
                        my_gen,
                    )?
                }
                // Custom view: per-turn runner — no process spawns until the
                // first user message. No sandbox profile: the agent sandboxes
                // itself rather than running under sandbox-exec.
                AgentView::Custom => spawn_per_turn_agent(
                    &provider,
                    cwd,
                    session_id.clone(),
                    app.clone(),
                    agent_id_str.clone(),
                    self.clone(),
                    my_gen,
                )?,
            }
        } else {
            let session_id = session_id
                .as_deref()
                .expect("non-codex agents always have a session id");
            let sandbox_root = agent_parent_dir(agent_id)?;
            let spec = SpawnSpec {
                agent_id: &agent_id_str,
                cwd,
                sandbox_root,
                session_id,
                fresh: effective_fresh,
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
                )?,
                AgentView::Custom => spawn_managed_agent(
                    spec,
                    app.clone(),
                    agent_id_str.clone(),
                    self.clone(),
                    my_gen,
                )?,
            }
        };

        self.agents.lock().insert(agent_id_str.clone(), agent);

        // Initial status is always Idle now — at process start there's
        // never an in-flight turn (we no longer pass a task as a spawn
        // arg). The user's first send flips it to Running. Only promote
        // out of the live Spawning state (a turn that already started
        // mustn't be clobbered).
        if matches!(self.live_status(&agent_id_str), Some(AgentStatus::Spawning)) {
            self.set_status(&app, &agent_id_str, AgentStatus::Idle, None);
        }

        spawn_turn_watchdog(self.clone(), app, agent_id_str, my_gen);

        Ok(())
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
            mark_user_turn_started(&self, app, agent_id);
            on_first_user_message(self.clone(), app.clone(), agent_id.to_string(), submitted);
        }
        Ok(())
    }

    pub fn send_user_message(
        self: Arc<Self>,
        app: &AppHandle,
        agent_id: &str,
        text: &str,
        attachments: &[String],
        thinking: Option<&str>,
    ) -> Result<()> {
        self.deliver_user_message(agent_id, text, attachments, thinking)?;
        mark_user_turn_started(&self, app, agent_id);
        on_first_user_message(self.clone(), app.clone(), agent_id.to_string(), text.to_string());
        Ok(())
    }

    /// Deliver a user turn to the running agent, then persist it as the
    /// provider-agnostic canonical `user_message` event.
    ///
    /// Order matters: we persist *after* the agent has accepted the message,
    /// never before. A freshly spawned agent isn't in the in-memory map yet,
    /// so a send aimed at it fails with `AgentNotFound`; the frontend retries
    /// until the agent is ready (`sendWhenAgentReady`). Persisting up front
    /// would record one duplicate `user_message` event per failed retry, and
    /// those surface as the same prompt rendered N times when the conversation
    /// is replayed from history on reopen.
    ///
    /// Persist-only: the frontend renders the user message optimistically on
    /// send and replays this event through the reducer on restore, so it must
    /// not be emitted live (that would double-render). It still lands ahead of
    /// the response — `send_user_message` only queues the write, so the agent's
    /// own events arrive (and persist) strictly later.
    fn deliver_user_message(
        &self,
        agent_id: &str,
        text: &str,
        attachments: &[String],
        thinking: Option<&str>,
    ) -> Result<()> {
        {
            let agents = self.agents.lock();
            let agent = agents
                .get(agent_id)
                .ok_or_else(|| Error::AgentNotFound(agent_id.to_string()))?;
            agent.send_user_message(text, attachments, thinking)?;
        }
        // The user's prompt is persisted via the agent's own transcript (ingested
        // into session_records at turn-end); the live view renders it
        // optimistically on send. No separate event log to write.
        Ok(())
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

    pub async fn stop_agent(self: Arc<Self>, app: AppHandle, agent_id: &str) -> Result<()> {
        // Interrupt the current turn. How it returns to Idle depends on
        // the runner: claude (managed) emits a `result` event and, if it
        // exits, `apply_exit_if_current` moves it to Idle; codex's
        // per-turn `exec` exits on SIGINT and its `on_turn_exit` handler
        // ends the turn (it emits no `turn.completed` when interrupted).
        let _ = app;
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

        if let Some(agent) = self.agents.lock().remove(agent_id) {
            let _ = agent.shutdown();
        }
        self.activities.lock().remove(agent_id);
        self.statuses.lock().remove(agent_id);
        self.native_input_lines.lock().remove(agent_id);
        self.shells.lock().remove(agent_id);
        if let Some(run) = self.runs.lock().remove(agent_id) {
            run.stop();
        }

        let mut snapshots: Vec<ArchivedRepoSnapshot> = Vec::with_capacity(record.repos.len());
        let mut total_adds: u32 = 0;
        let mut total_dels: u32 = 0;

        for repo in &record.repos {
            // Resolve SHAs first so we capture state before any
            // destructive step.
            let branch_tip_sha = if let Some(b) = &repo.branch {
                git::rev_parse(&repo.repo_path, b).await.ok()
            } else {
                None
            };
            let parent_branch_sha = if let Some(b) = &repo.parent_branch {
                git::rev_parse(&repo.repo_path, b).await.ok()
            } else {
                None
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

        // Tear down worktrees + branches (best-effort: a single failure
        // shouldn't block archive, since the user's intent is "get rid
        // of this", but we surface git errors via tracing).
        for repo in &record.repos {
            let worktree = match repo_worktree_path(agent_id, &repo.subdir) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(error = %e, subdir = %repo.subdir, "archive: worktree_path failed");
                    continue;
                }
            };
            let _ = git::worktree_prune(&repo.repo_path).await;
            if let Err(e) = git::worktree_remove(&repo.repo_path, &worktree, true).await {
                tracing::warn!(error = %e, subdir = %repo.subdir, "archive: worktree remove failed");
            }
            if let Some(branch) = &repo.branch {
                if let Err(e) = git::branch_delete(&repo.repo_path, branch).await {
                    tracing::warn!(%branch, error = %e, "archive: branch delete failed");
                }
            }
        }

        // Best-effort parent dir cleanup.
        if let Ok(parent) = agent_parent_dir(agent_id) {
            if parent.exists() {
                let _ = std::fs::remove_dir_all(&parent);
            }
        }

        let archive = ArchiveMetadata {
            archived_at: chrono::Utc::now().to_rfc3339(),
            repos: snapshots,
            diff_stats: DiffStats {
                additions: total_adds,
                deletions: total_dels,
            },
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
        std::fs::create_dir_all(&parent_dir)
            .map_err(|e| Error::Other(format!("create parent dir: {e}")))?;

        let mut restored: Vec<TrackedRepo> = Vec::with_capacity(archive.repos.len());
        for snap in &archive.repos {
            let tip_sha = snap.branch_tip_sha.as_deref().expect("checked above");
            let desired_name = snap
                .branch_name
                .clone()
                .unwrap_or_else(|| format!("quorum/{}-restored", agent_id));

            // Resolve branch name collisions by appending -restored / -restored-N.
            let mut chosen = desired_name.clone();
            let mut bumps = 0;
            loop {
                let exists = git::branch_exists(&snap.repo_path, &chosen).await.unwrap_or(false);
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

            let worktree = repo_worktree_path(agent_id, &snap.subdir)?;
            if let Some(parent) = worktree.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| Error::Other(format!("create worktree parent: {e}")))?;
            }
            git::worktree_add_branch(&snap.repo_path, &worktree, &chosen).await?;

            restored.push(TrackedRepo {
                repo_path: snap.repo_path.clone(),
                subdir: snap.subdir.clone(),
                branch: Some(chosen),
                parent_branch: snap.parent_branch.clone(),
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

    /// Read the persisted session JSONL for an archived (or live) agent
    /// and return its raw lines as a Vec<Value>. The frontend's
    /// per-provider adapter normalizes these into renderable events, so
    /// we don't need a parallel renderer here. Claude's file lives under
    /// `~/.claude/projects`; codex's rollout under `$CODEX_HOME/sessions`.
    ///
    /// Returns an empty vec if the file is missing (pruned, deleted, or
    /// the session never reached its first turn).
    pub fn read_session_transcript(&self, agent_id: &str) -> Result<Vec<Value>> {
        let record = self.workspace.agent(agent_id)?;

        // Agents whose native on-disk transcript isn't wired yet have
        // nothing to hand back here; re-attaching restores their history
        // from the provider-agnostic SQLite event log instead, and
        // `--resume`/`--session <id>` still continues the conversation.
        // (Per-agent rollout — see `AgentCapabilities::transcript_replay`.)
        if !capabilities(&record.provider).transcript_replay {
            return Ok(Vec::new());
        }

        let session_id = match record.session_id.as_deref() {
            Some(s) => s,
            // A per-turn agent's id is only assigned on the first turn;
            // before that there's nothing to replay.
            None if is_per_turn_provider(&record.provider) => return Ok(Vec::new()),
            None => return Err(Error::Other("agent has no session id".into())),
        };

        // Each provider persists its conversation in a different place and
        // format. The frontend's per-provider adapter (`normalizeTranscript`)
        // translates these raw lines into renderable events, so here we just
        // locate the file and hand back its JSONL.
        let path = if record.provider == "codex" {
            find_codex_rollout(session_id)
        } else {
            find_session_jsonl(session_id)
        };
        match path {
            Some(p) => read_jsonl_values(&p),
            None => Ok(Vec::new()),
        }
    }

    /// Synchronously ingest the agent's transcript into session_records (used
    /// for lazy backfill when a session is opened with no records yet). `None`
    /// if the provider has no transcript reader.
    pub fn sync_session(&self, agent_id: &str) -> Option<usize> {
        sync_session_records(&self.workspace, agent_id)
    }

    /// Fire-and-forget transcript ingest at turn-end. Retries with backoff to
    /// ride out the agent's flush lag; emits `session:records-appended` when new
    /// records land. WARNs once if a reader-backed agent ingests nothing after
    /// all retries — the early signal that its transcript path/format changed.
    /// Called from `transition_active` whenever any agent reaches Idle, so it
    /// covers managed, per-turn, and native turn-ends uniformly.
    pub fn trigger_session_sync(&self, app: AppHandle, agent_id: String) {
        let workspace = self.workspace.clone();
        tauri::async_runtime::spawn(async move {
            // Immediate attempt, then back off (ms) for flush lag.
            let backoffs = [0u64, 200, 400, 800, 1600];
            let mut had_reader = false;
            let mut inserted_any = false;
            for wait in backoffs {
                if wait > 0 {
                    tokio::time::sleep(Duration::from_millis(wait)).await;
                }
                match sync_session_records(&workspace, &agent_id) {
                    None => return, // no transcript reader — nothing to do
                    Some(0) => had_reader = true,
                    Some(_) => {
                        inserted_any = true;
                        break;
                    }
                }
            }
            if inserted_any {
                let _ = app.emit(
                    "session:records-appended",
                    SessionRecordsAppendedPayload { agent_id },
                );
            } else if had_reader {
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
            let worktree = match crate::workspace::repo_worktree_path(&agent_id, &repo.subdir) {
                Ok(p) => p,
                Err(_) => return,
            };
            let state = crate::gh::pr_view(&worktree).await.unwrap_or(None);
            let _ = app.emit(
                "pr:state_changed",
                PrStateChangedPayload { agent_id, state },
            );
        });
    }

    pub async fn discard_agent(self: Arc<Self>, agent_id: &str) -> Result<()> {
        let record = self.workspace.agent(agent_id).ok();
        let repos = record.as_ref().map(|r| r.repos.clone()).unwrap_or_default();
        let parent_dir = agent_parent_dir(agent_id).ok();

        if let Some(agent) = self.agents.lock().remove(agent_id) {
            let _ = agent.shutdown();
        }
        self.activities.lock().remove(agent_id);
        self.statuses.lock().remove(agent_id);
        self.native_input_lines.lock().remove(agent_id);
        self.shells.lock().remove(agent_id);
        if let Some(run) = self.runs.lock().remove(agent_id) {
            run.stop();
        }

        // Tear down each tracked repo's worktree + branch.
        for repo in &repos {
            let worktree = match repo_worktree_path(agent_id, &repo.subdir) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(error = %e, subdir = %repo.subdir, "discard: worktree_path failed");
                    continue;
                }
            };
            let _ = git::worktree_prune(&repo.repo_path).await;
            if let Err(e) = git::worktree_remove(&repo.repo_path, &worktree, true).await {
                tracing::warn!(error = %e, subdir = %repo.subdir, "discard: worktree remove failed");
            }
            if let Some(branch) = &repo.branch {
                if let Err(e) = git::branch_delete(&repo.repo_path, branch).await {
                    tracing::warn!(%branch, error = %e, "discard: branch delete failed");
                }
            }
        }

        // Remove the parent dir (may contain orphan files if any
        // worktree removal failed). Best-effort.
        if let Some(parent) = parent_dir {
            if parent.exists() {
                let _ = std::fs::remove_dir_all(&parent);
            }
        }

        self.workspace.remove_agent(agent_id)?;
        Ok(())
    }
}

/// Locate the claude session JSONL by scanning `~/.claude/projects/*/`
/// for `<session-id>.jsonl`. Claude's path-encoding scheme isn't part
/// of its public API, so we glob instead of recomputing the encoded
/// directory name from the worktree path.
/// Ingest the agent's on-disk transcript into `session_records`, idempotent per
/// `native_id`. `None` = no transcript reader for this provider (skip, don't
/// retry); `Some(n)` = reader ran, `n` new records inserted (`0` = nothing yet:
/// file not flushed, or its location/format changed).
fn sync_session_records(workspace: &WorkspaceManager, agent_id: &str) -> Option<usize> {
    let record = workspace.agent(agent_id).ok()?;
    let reader = crate::agent::transcript_reader(&record.provider)?;

    // A reader exists; from here any shortfall is "nothing yet" → Some(0).
    let Some(session_id) = record.session_id.as_deref() else {
        return Some(0);
    };
    let Some(repo) = record.repos.first() else {
        return Some(0);
    };
    let Ok(cwd) = repo_worktree_path(agent_id, &repo.subdir) else {
        return Some(0);
    };

    let paths = (reader.locate)(session_id, &cwd);
    let records = (reader.read)(&paths);

    // Version-frozen snapshot tag (memoized probe — at most one --version per
    // provider per process).
    let version = crate::agent::cached_provider_version(&record.provider);

    let mut inserted = 0usize;
    for rec in &records {
        match workspace.append_session_record(
            agent_id,
            &record.provider,
            "transcript",
            &rec.native_id,
            version.as_deref(),
            &rec.body,
        ) {
            Ok(true) => inserted += 1,
            Ok(false) => {}
            Err(e) => tracing::warn!(error = %e, agent_id, "append_session_record failed"),
        }
    }
    Some(inserted)
}

pub(crate) fn find_session_jsonl(session_id: &str) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let projects = home.join(".claude").join("projects");
    let entries = std::fs::read_dir(&projects).ok()?;
    let filename = format!("{session_id}.jsonl");
    for entry in entries.flatten() {
        let path = entry.path().join(&filename);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

/// Locate codex's rollout file for a thread id. Codex stores sessions at
/// `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<ts>-<id>.jsonl` (CODEX_HOME
/// defaults to `~/.codex`); the id suffix is the thread id we captured.
fn find_codex_rollout(session_id: &str) -> Option<PathBuf> {
    find_codex_rollouts(session_id).into_iter().next()
}

/// All of codex's rollout files for a thread id, ordered (filenames are
/// timestamp-prefixed, so lexical sort == chronological). Resume normally keeps
/// one file per session, but returning all is correct if it ever splits.
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

fn spawn_pty_agent(
    spec: SpawnSpec<'_>,
    app: AppHandle,
    agent_id: String,
    sup: Arc<Supervisor>,
    gen: u64,
) -> Result<Agent> {
    let app_for_output = app.clone();
    let id_for_output = agent_id.clone();
    let sup_for_output = sup.clone();
    let sup_for_exit = sup;
    let app_for_exit = app.clone();
    let id_for_exit = agent_id.clone();
    Agent::spawn_pty(
        spec,
        move |bytes| {
            if let Some(activity) = sup_for_output
                .activities
                .lock()
                .get_mut(&id_for_output)
            {
                activity.observe_bytes(&bytes);
            }

            if let Err(e) = app_for_output.emit(
                "agent:output",
                AgentOutputPayload {
                    agent_id: id_for_output.clone(),
                    bytes,
                },
            ) {
                tracing::warn!(error = %e, agent_id = %id_for_output, "emit agent:output failed");
            }
        },
        move |exit| {
            apply_exit_if_current(&sup_for_exit, &app_for_exit, &id_for_exit, gen, exit.success, exit.message);
        },
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
    let app_for_output = app.clone();
    let id_for_output = agent_id.clone();
    let sup_for_output = sup.clone();
    let sup_for_exit = sup;
    let app_for_exit = app.clone();
    let id_for_exit = agent_id.clone();
    Agent::spawn_pty_native(
        spec,
        &provider,
        move |bytes| {
            if let Some(activity) = sup_for_output.activities.lock().get_mut(&id_for_output) {
                activity.observe_bytes(&bytes);
            }

            if let Err(e) = app_for_output.emit(
                "agent:output",
                AgentOutputPayload {
                    agent_id: id_for_output.clone(),
                    bytes,
                },
            ) {
                tracing::warn!(error = %e, agent_id = %id_for_output, "emit agent:output failed");
            }
        },
        move |exit| {
            apply_exit_if_current(&sup_for_exit, &app_for_exit, &id_for_exit, gen, exit.success, exit.message);
        },
    )
}

fn spawn_managed_agent(
    spec: SpawnSpec<'_>,
    app: AppHandle,
    agent_id: String,
    sup: Arc<Supervisor>,
    gen: u64,
) -> Result<Agent> {
    let app_for_event = app.clone();
    let id_for_event = agent_id.clone();
    let sup_for_event = sup.clone();
    let sup_for_exit = sup.clone();
    let app_for_exit = app.clone();
    let id_for_exit = agent_id.clone();
    Agent::spawn_managed(
        spec,
        move |event| {
            if let Some(activity) = sup_for_event
                .activities
                .lock()
                .get_mut(&id_for_event)
            {
                activity.observe_event(&event);
            }

            if let Err(e) = app_for_event.emit(
                "agent:event",
                AgentEventPayload {
                    agent_id: id_for_event.clone(),
                    event,
                },
            ) {
                tracing::warn!(error = %e, agent_id = %id_for_event, "emit agent:event failed");
            }
        },
        move |exit| {
            apply_exit_if_current(&sup_for_exit, &app_for_exit, &id_for_exit, gen, exit.success, exit.message);
        },
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
    cwd: PathBuf,
    session_id: Option<String>,
    app: AppHandle,
    agent_id: String,
    sup: Arc<Supervisor>,
    gen: u64,
) -> Result<Agent> {
    let app_for_event = app.clone();
    let id_for_event = agent_id.clone();
    let sup_for_event = sup.clone();
    let id_for_sid = agent_id.clone();
    let sup_for_sid = sup.clone();
    let app_for_exit = app;
    let id_for_exit = agent_id;
    let sup_for_exit = sup;

    let on_event = move |event: Value| {
        if let Some(activity) = sup_for_event.activities.lock().get_mut(&id_for_event) {
            activity.observe_event(&event);
        }
        if let Err(e) = app_for_event.emit(
            "agent:event",
            AgentEventPayload {
                agent_id: id_for_event.clone(),
                event,
            },
        ) {
            tracing::warn!(error = %e, agent_id = %id_for_event, "emit agent:event failed");
        }
    };
    let on_session_id = move |sid: String| {
        if let Err(e) = sup_for_sid.workspace.set_agent_session_id(&id_for_sid, &sid) {
            tracing::warn!(error = %e, agent_id = %id_for_sid, "persist session id failed");
        }
    };
    let on_turn_exit = move |_success: bool| {
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
        // End the turn. Idempotent with the in-band turn-end watchdog path;
        // the win is the interrupt/crash case where no such event arrives.
        // We don't surface non-success as Error: a user-initiated Stop
        // (SIGINT) is also non-success, and real agent errors are reported
        // in-band as events.
        // Turn-end ingest fires from transition_active's Idle transition.
        transition_active(&sup_for_exit, &app_for_exit, &id_for_exit, AgentStatus::Idle);
    };

    let spec = PerTurnSpec { cwd, session_id };
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
/// as the agent's `task` and kicks branch creation for every
/// branchless tracked repo.
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

    create_branches_for_branchless_repos(sup, app, agent_id, trimmed);
}

/// For every tracked repo on the agent that doesn't have a branch yet,
/// derive a slug from the agent's task and create `quorum/<slug>` inside
/// that repo's worktree. Runs in a background task. Idempotent —
/// `set_repo_branch_if_empty` guards each write.
fn create_branches_for_branchless_repos(
    sup: Arc<Supervisor>,
    app: AppHandle,
    agent_id: String,
    task: String,
) {
    tauri::async_runtime::spawn(async move {
        let record = match sup.workspace.agent(&agent_id) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, agent_id = %agent_id, "branch creation: agent lookup failed");
                return;
            }
        };

        let slug_base = branding::slugify_task(&task);
        let slug = if slug_base.is_empty() {
            agent_id.clone()
        } else {
            slug_base
        };

        for repo in record.repos.iter() {
            if repo.branch.is_some() {
                continue;
            }
            let worktree = match repo_worktree_path(&agent_id, &repo.subdir) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(error = %e, subdir = %repo.subdir, "branch creation: worktree_path failed");
                    continue;
                }
            };
            let mut branch = branding::branch_for(&slug);
            if git::branch_exists(&repo.repo_path, &branch)
                .await
                .unwrap_or(false)
            {
                branch = format!("{branch}-{agent_id}");
            }
            if let Err(e) = git::checkout_new_branch(&worktree, &branch).await {
                tracing::warn!(
                    error = %e,
                    agent_id = %agent_id,
                    subdir = %repo.subdir,
                    branch = %branch,
                    "checkout_new_branch failed"
                );
                continue;
            }
            match sup
                .workspace
                .set_repo_branch_if_empty(&agent_id, &repo.subdir, &branch)
            {
                Ok(true) => {
                    let _ = app.emit(
                        "agent:branch",
                        AgentBranchPayload {
                            agent_id: agent_id.clone(),
                            subdir: repo.subdir.clone(),
                            branch: branch.clone(),
                        },
                    );
                }
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "set_repo_branch_if_empty failed");
                }
            }
        }
    });
}

fn mark_user_turn_started(sup: &Supervisor, app: &AppHandle, agent_id: &str) {
    if let Some(activity) = sup.activities.lock().get_mut(agent_id) {
        activity.reset_for_new_turn();
    }
    transition_active(sup, app, agent_id, AgentStatus::Running);
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

fn arm_spawn_timeout(sup: Arc<Supervisor>, app: AppHandle, agent_id: String) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(SPAWN_TIMEOUT).await;
        // Only time out an agent still in the live Spawning state. One that
        // has progressed to Running/Idle has a non-Spawning in-memory entry
        // and must not be killed.
        let still_spawning = matches!(sup.live_status(&agent_id), Some(AgentStatus::Spawning));
        if !still_spawning {
            return;
        }
        let err = "Spawn timed out after 15s — process did not become ready.".to_string();
        sup.set_status(&app, &agent_id, AgentStatus::Error, Some(err));
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
        }
    }
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
    let shell = user_shell();
    let args = shell_args(&cmd);

    let session_out = session.clone();
    let app_out = app.clone();
    let id_out = agent_id.clone();

    let sup_exit = sup.clone();
    let app_exit = app.clone();
    let id_exit = agent_id.clone();
    let session_exit = session.clone();
    let cwd_exit = cwd.clone();

    let pty = run_session::spawn_command(
        &shell,
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

    session.attach_pty(pty);
    Ok(())
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

    fn test_supervisor() -> Supervisor {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::database::init(dir.path()).unwrap();
        Supervisor::new(Arc::new(WorkspaceManager::new(db)))
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
    fn delivery_to_unready_agent_persists_nothing() {
        // A freshly spawned agent has a session row but isn't in the live agents
        // map yet (the frontend retries the send until it's ready). A failed
        // delivery must not write anything to the canonical store.
        let sup = test_supervisor();
        let mut record = record_with_status("yosemite", AgentStatus::Spawning);
        sup.workspace.add_agent(&mut record).unwrap();

        let err = sup
            .deliver_user_message("yosemite", "hello", &[], None)
            .unwrap_err();
        assert!(matches!(err, Error::AgentNotFound(_)));

        let records = sup.workspace.read_session_records("yosemite").unwrap();
        assert!(
            records.is_empty(),
            "failed delivery must persist nothing, got {records:?}",
        );
    }
}
