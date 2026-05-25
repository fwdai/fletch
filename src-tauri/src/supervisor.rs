//! Coordinator between Tauri IPC commands and the running agents.

use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

use crate::activity::{Activity, ClaudeManagedActivity, ClaudeNativeActivity};
use crate::agent::{Agent, SpawnSpec};
use crate::branding;
use crate::error::{Error, Result};
use crate::git;
use crate::workspace::{
    new_agent_record, AgentRecord, AgentStatus, AgentView, Workspace, WorkspaceManager,
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
pub struct AgentStatusPayload {
    pub agent_id: String,
    pub status: AgentStatus,
    pub last_error: Option<String>,
    #[serde(default)]
    pub status_message: Option<String>,
}

#[derive(Clone, serde::Serialize)]
pub struct AgentViewPayload {
    pub agent_id: String,
    pub view: AgentView,
}

/// Watchdog cadence — how often we poll each agent's Activity to ask
/// "has the turn ended?". The Activity's own threshold governs when
/// turn_ended() starts returning true; this is just polling cost.
const WATCHDOG_TICK: Duration = Duration::from_millis(500);

/// Upper bound on the Spawning state. If the worktree or claude
/// launch hangs longer than this, force the agent into Error so the
/// UI never wedges on a phantom "starting…" pill.
const SPAWN_TIMEOUT: Duration = Duration::from_secs(15);

pub struct Supervisor {
    pub workspace: Arc<WorkspaceManager>,
    pub agents: Mutex<HashMap<String, Agent>>,
    /// Per-agent spawn generation. Bumped on every `start_process` so
    /// exit callbacks from a torn-down (switched-away) process can be
    /// identified and ignored — without this, a stale exit would mark
    /// the freshly-spawned replacement as Stopped.
    pub generations: Mutex<HashMap<String, u64>>,
    /// Per-agent turn detector. The supervisor feeds it whatever
    /// signal the agent's output channel produces and the watchdog
    /// polls turn_ended() to drive Running → Idle.
    pub activities: Mutex<HashMap<String, Box<dyn Activity>>>,
}

impl Supervisor {
    pub fn new(workspace: Arc<WorkspaceManager>) -> Self {
        Self {
            workspace,
            agents: Mutex::new(HashMap::new()),
            generations: Mutex::new(HashMap::new()),
            activities: Mutex::new(HashMap::new()),
        }
    }

    pub fn current_workspace(&self) -> Option<Workspace> {
        self.workspace.current()
    }

    pub fn set_repo(&self, repo_path: PathBuf) -> Result<Workspace> {
        self.workspace.set_repo(repo_path)
    }

    pub async fn spawn_agent(
        self: Arc<Self>,
        app: AppHandle,
        task: String,
        view: AgentView,
    ) -> Result<AgentRecord> {
        let repo_path = self.workspace.repo_path()?;

        // The agent's display name comes from the auto-allocated place
        // id. The git branch is derived from the task itself — much
        // more useful in `git branch`, `git log`, and PR titles — and
        // namespaced under the app name via `branding::branch_for` so
        // a rename is a one-constant change. On collision with an
        // existing branch we append the place id for uniqueness.
        let agent_id = self.workspace.allocate_agent_id()?;
        let name = agent_id.clone();

        let slug = branding::slugify_task(&task);
        let slug = if slug.is_empty() { agent_id.clone() } else { slug };
        let mut branch = branding::branch_for(&slug);
        if git::branch_exists(&repo_path, &branch).await.unwrap_or(false) {
            branch = format!("{branch}-{agent_id}");
        }

        let record = new_agent_record(
            agent_id.clone(),
            name,
            branch.clone(),
            task.clone(),
            view,
        );
        let worktree = self.workspace.worktree_path(&agent_id)?;

        self.workspace.add_agent(record.clone())?;
        emit_status(&app, &agent_id, AgentStatus::Spawning, None);
        // Belt: if we somehow get stuck Spawning, this guarantees we
        // surface Error rather than wedge forever.
        arm_spawn_timeout(self.clone(), app.clone(), agent_id.clone());

        let sup = self.clone();
        let app_for_task = app.clone();
        let id_for_task = agent_id.clone();
        tauri::async_runtime::spawn(async move {
            emit_progress(&sup.workspace, &app_for_task, &id_for_task, "Creating git worktree...");

            if let Err(e) = std::fs::create_dir_all(git::worktrees_dir(&repo_path)) {
                fail_spawn(&sup, &app_for_task, &id_for_task, e.to_string());
                return;
            }

            if let Err(e) = git::worktree_add(&repo_path, &worktree, &branch).await {
                fail_spawn(&sup, &app_for_task, &id_for_task, e.to_string());
                return;
            }

            // Give React a tick to mount the view before claude starts.
            // PTY mode is sensitive to early terminal-negotiation bytes;
            // custom mode is less so but it costs nothing to wait.
            tokio::time::sleep(Duration::from_millis(350)).await;

            emit_progress(&sup.workspace, &app_for_task, &id_for_task, "Launching claude...");

            if let Err(e) = sup.start_process(&app_for_task, &id_for_task, true).await {
                let _ = git::worktree_remove(&repo_path, &worktree, true).await;
                fail_spawn(&sup, &app_for_task, &id_for_task, e.to_string());
            }
        });

        Ok(record)
    }

    /// Spawn the claude process matching the record's current view and
    /// register it in the agents map. Assumes the worktree already
    /// exists. `fresh=true` only on the very first spawn (uses
    /// --session-id); subsequent spawns (view switches / resume) use
    /// --resume.
    async fn start_process(
        self: &Arc<Self>,
        app: &AppHandle,
        agent_id: &str,
        fresh: bool,
    ) -> Result<()> {
        let record = self.workspace.agent(agent_id)?;
        let session_id = record
            .session_id
            .clone()
            .ok_or_else(|| Error::Other("agent record missing session_id".into()))?;
        let worktree = self.workspace.worktree_path(agent_id)?;

        let app = app.clone();
        let agent_id_str = agent_id.to_string();

        let my_gen = {
            let mut g = self.generations.lock();
            let entry = g.entry(agent_id_str.clone()).or_insert(0);
            *entry += 1;
            *entry
        };

        // Install a fresh Activity instance for this generation. Done
        // before spawn so the first output chunks can be observed.
        let activity: Box<dyn Activity> = match record.view {
            AgentView::Native => Box::new(ClaudeNativeActivity::new()),
            AgentView::Custom => Box::new(ClaudeManagedActivity::new()),
        };
        // A fresh spawn implies an in-flight turn (claude is
        // processing the initial task). A resume does not. We tell
        // the activity which one so its turn_ended() doesn't fire
        // prematurely on the silence that precedes the first event.
        let mut activity = activity;
        if fresh {
            activity.reset_for_new_turn();
        }
        self.activities.lock().insert(agent_id_str.clone(), activity);

        let spec = SpawnSpec {
            agent_id: &agent_id_str,
            worktree: worktree.clone(),
            session_id: &session_id,
            fresh,
            task: &record.task,
            cols: 120,
            rows: 32,
        };

        let agent = match record.view {
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
        };

        self.agents.lock().insert(agent_id_str.clone(), agent);

        // Initial status per the state machine:
        //   fresh  → Running  (initial task in flight)
        //   resume → Idle     (claude loaded session, no turn yet)
        let initial = if fresh {
            AgentStatus::Running
        } else {
            AgentStatus::Idle
        };
        let initial_for_emit = initial.clone();
        let changed = self.workspace.update_agent_status_if(
            &agent_id_str,
            initial,
            None,
            |status| matches!(status, AgentStatus::Spawning),
        );
        if matches!(changed, Ok(true)) {
            emit_status(&app, &agent_id_str, initial_for_emit, None);
        }

        spawn_turn_watchdog(self.clone(), app, agent_id_str, my_gen);

        Ok(())
    }

    pub fn resume_persisted_agents(self: Arc<Self>, app: AppHandle) {
        let agents = match self.workspace.current() {
            Some(ws) => ws.agents,
            None => return,
        };

        for record in agents {
            if !matches!(record.status, AgentStatus::Spawning) {
                continue;
            }
            if record.session_id.is_none() {
                let err = "Agent record has no session id (created before resume was supported). Remove and respawn.".to_string();
                let _ = self.workspace.update_agent_status(
                    &record.id,
                    AgentStatus::Error,
                    Some(err.clone()),
                );
                emit_status(&app, &record.id, AgentStatus::Error, Some(err));
                continue;
            }

            let sup = self.clone();
            let app = app.clone();
            let id = record.id.clone();
            arm_spawn_timeout(sup.clone(), app.clone(), id.clone());
            tauri::async_runtime::spawn(async move {
                emit_progress(&sup.workspace, &app, &id, "Resuming…");
                if let Err(e) = sup.start_process(&app, &id, false).await {
                    fail_spawn(&sup, &app, &id, e.to_string());
                }
            });
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
        if record.session_id.is_none() {
            return Err(Error::Other(
                "Agent has no session id; remove and respawn.".into(),
            ));
        }
        self.workspace
            .update_agent_status(agent_id, AgentStatus::Spawning, None)?;
        emit_progress(&self.workspace, &app, agent_id, "Resuming…");
        arm_spawn_timeout(self.clone(), app.clone(), agent_id.to_string());

        self.start_process(&app, agent_id, false).await
    }

    pub fn write_to_agent(&self, app: &AppHandle, agent_id: &str, bytes: &[u8]) -> Result<()> {
        {
            let agents = self.agents.lock();
            let agent = agents
                .get(agent_id)
                .ok_or_else(|| Error::AgentNotFound(agent_id.to_string()))?;
            agent.write_pty(bytes)?;
        }
        // In native mode the overlay-input box is the only writer
        // (we disabled xterm keystroke pass-through). So every write
        // is a user-initiated turn submission — mark Running and tell
        // the activity tracker a new turn has started.
        mark_user_turn_started(self, app, agent_id);
        Ok(())
    }

    pub fn send_user_message(
        &self,
        app: &AppHandle,
        agent_id: &str,
        text: &str,
    ) -> Result<()> {
        {
            let agents = self.agents.lock();
            let agent = agents
                .get(agent_id)
                .ok_or_else(|| Error::AgentNotFound(agent_id.to_string()))?;
            agent.send_user_message(text)?;
        }
        mark_user_turn_started(self, app, agent_id);
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

        if let Some(agent) = self.agents.lock().remove(agent_id) {
            let _ = agent.shutdown();
        }
        // Drop the old activity so the new spawn can't observe stale
        // state by accident.
        self.activities.lock().remove(agent_id);

        self.workspace.update_agent_view(agent_id, new_view)?;
        let _ = app.emit(
            "agent:view",
            AgentViewPayload {
                agent_id: agent_id.to_string(),
                view: new_view,
            },
        );
        let _ = self.workspace.update_agent_status(
            agent_id,
            AgentStatus::Spawning,
            None,
        );
        emit_progress(&self.workspace, &app, agent_id, "Switching view…");
        arm_spawn_timeout(self.clone(), app.clone(), agent_id.to_string());

        tokio::time::sleep(Duration::from_millis(150)).await;

        if let Err(e) = self.start_process(&app, agent_id, false).await {
            let err = e.to_string();
            let _ = self.workspace.update_agent_status(
                agent_id,
                AgentStatus::Error,
                Some(err.clone()),
            );
            emit_status(&app, agent_id, AgentStatus::Error, Some(err));
            return Err(e);
        }
        Ok(())
    }

    pub async fn stop_agent(self: Arc<Self>, app: AppHandle, agent_id: &str) -> Result<()> {
        let live = self.agents.lock().remove(agent_id);
        if let Some(agent) = live {
            let _ = agent.shutdown();
        }
        self.activities.lock().remove(agent_id);
        match self
            .workspace
            .update_agent_status(agent_id, AgentStatus::Stopped, None)
        {
            Ok(_) | Err(Error::AgentNotFound(_)) => {}
            Err(e) => return Err(e),
        }
        emit_status(&app, agent_id, AgentStatus::Stopped, None);
        Ok(())
    }

    pub async fn discard_agent(self: Arc<Self>, agent_id: &str) -> Result<()> {
        let repo = self.workspace.repo_path()?;
        let worktree = self.workspace.worktree_path(agent_id)?;
        let branch = self
            .workspace
            .current()
            .and_then(|ws| {
                ws.agents
                    .iter()
                    .find(|a| a.id == agent_id)
                    .map(|a| a.branch.clone())
            });

        if let Some(agent) = self.agents.lock().remove(agent_id) {
            let _ = agent.shutdown();
        }
        self.activities.lock().remove(agent_id);

        let _ = git::worktree_prune(&repo).await;
        if let Err(e) = git::worktree_remove(&repo, &worktree, true).await {
            tracing::warn!(error = %e, "discard: worktree remove failed; trying fs fallback");
            if worktree.exists() {
                let _ = std::fs::remove_dir_all(&worktree);
            }
        }

        if let Some(branch) = branch {
            if let Err(e) = git::branch_delete(&repo, &branch).await {
                tracing::warn!(%branch, error = %e, "discard: branch delete failed");
            }
        }

        self.workspace.remove_agent(agent_id)?;
        Ok(())
    }
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
            // Feed the activity tracker. We do NOT flip status here —
            // Running is set explicitly by the user's Send action and
            // demoted by the watchdog when activity.turn_ended() fires.
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
    let sup_for_exit = sup;
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

/// Mark "the user just started a turn." Flips status to Running and
/// tells the activity tracker to begin a new turn (so any prior
/// turn-end flag clears before claude starts emitting).
fn mark_user_turn_started(sup: &Supervisor, app: &AppHandle, agent_id: &str) {
    if let Some(activity) = sup.activities.lock().get_mut(agent_id) {
        activity.reset_for_new_turn();
    }
    transition_active(sup, app, agent_id, AgentStatus::Running);
}

/// Watchdog: poll the agent's activity tracker and demote Running →
/// Idle when turn_ended() fires. Exits when the agent's generation
/// moves (switch / resume / stop / discard).
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

/// If the agent is still in Spawning after SPAWN_TIMEOUT, force it to
/// Error. Prevents wedging in "starting…" forever when the worktree
/// or claude launch hangs.
fn arm_spawn_timeout(sup: Arc<Supervisor>, app: AppHandle, agent_id: String) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(SPAWN_TIMEOUT).await;
        let still_spawning = sup
            .workspace
            .agent(&agent_id)
            .map(|r| matches!(r.status, AgentStatus::Spawning))
            .unwrap_or(false);
        if !still_spawning {
            return;
        }
        let err = "Spawn timed out after 15s — process did not become ready.".to_string();
        let _ = sup.workspace.update_agent_status(
            &agent_id,
            AgentStatus::Error,
            Some(err.clone()),
        );
        emit_status(&app, &agent_id, AgentStatus::Error, Some(err));
        // Best-effort: tear down any half-spawned process so it
        // doesn't keep emitting events.
        if let Some(agent) = sup.agents.lock().remove(&agent_id) {
            let _ = agent.shutdown();
        }
        sup.activities.lock().remove(&agent_id);
    });
}

fn fail_spawn(sup: &Supervisor, app: &AppHandle, agent_id: &str, err: String) {
    let _ = sup
        .workspace
        .update_agent_status(agent_id, AgentStatus::Error, Some(err.clone()));
    emit_status(app, agent_id, AgentStatus::Error, Some(err));
}

/// Flip an agent between Running and Idle. Allowed transitions:
/// Spawning|Running|Idle → target. Refuses to overwrite Stopped/Error
/// (so a late watchdog tick can't resurrect a dead agent) and skips
/// persistence when target equals current.
fn transition_active(
    sup: &Supervisor,
    app: &AppHandle,
    agent_id: &str,
    new: AgentStatus,
) {
    let for_predicate = new.clone();
    let for_emit = new.clone();
    let changed = sup.workspace.update_agent_status_if(
        agent_id,
        new,
        None,
        move |cur| {
            matches!(
                cur,
                AgentStatus::Spawning | AgentStatus::Running | AgentStatus::Idle
            ) && *cur != for_predicate
        },
    );
    if matches!(changed, Ok(true)) {
        emit_status(app, agent_id, for_emit, None);
    }
}

/// Apply an exit callback only if this generation is still the
/// "current" one for the agent. A stale generation means we already
/// replaced this process (via switch_view) and the new spawn owns the
/// status; touching it here would clobber the live process.
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

    if success {
        let changed = sup.workspace.update_agent_status_if(
            agent_id,
            AgentStatus::Stopped,
            None,
            |status| {
                matches!(
                    status,
                    AgentStatus::Running | AgentStatus::Idle | AgentStatus::Spawning
                )
            },
        );
        if matches!(changed, Ok(true)) {
            emit_status(app, agent_id, AgentStatus::Stopped, None);
        }
    } else {
        let err = format!("Agent process exited: {message}");
        let changed = sup.workspace.update_agent_status_if(
            agent_id,
            AgentStatus::Error,
            Some(err.clone()),
            |status| {
                matches!(
                    status,
                    AgentStatus::Running | AgentStatus::Idle | AgentStatus::Spawning
                )
            },
        );
        if matches!(changed, Ok(true)) {
            emit_status(app, agent_id, AgentStatus::Error, Some(err));
        }
    }
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
            status_message: None,
        },
    );
}

fn emit_progress(
    workspace: &WorkspaceManager,
    app: &AppHandle,
    agent_id: &str,
    message: &str,
) {
    let _ = workspace.update_agent_status_message(agent_id, Some(message.into()));
    let _ = app.emit(
        "agent:status",
        AgentStatusPayload {
            agent_id: agent_id.to_string(),
            status: AgentStatus::Spawning,
            last_error: None,
            status_message: Some(message.into()),
        },
    );
}
