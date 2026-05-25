//! Coordinator between Tauri IPC commands and the running agents.

use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

use crate::agent::{Agent, SpawnSpec};
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

pub struct Supervisor {
    pub workspace: Arc<WorkspaceManager>,
    pub agents: Mutex<HashMap<String, Agent>>,
    /// Per-agent spawn generation. Bumped on every `start_process` so
    /// exit callbacks from a torn-down (switched-away) process can be
    /// identified and ignored — without this, a stale exit would mark
    /// the freshly-spawned replacement as Stopped.
    pub generations: Mutex<HashMap<String, u64>>,
}

impl Supervisor {
    pub fn new(workspace: Arc<WorkspaceManager>) -> Self {
        Self {
            workspace,
            agents: Mutex::new(HashMap::new()),
            generations: Mutex::new(HashMap::new()),
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
        name: String,
        branch: String,
        task: String,
        view: AgentView,
    ) -> Result<AgentRecord> {
        let repo_path = self.workspace.repo_path()?;

        let record = new_agent_record(name, branch.clone(), task.clone(), view);
        let agent_id = record.id.clone();
        let worktree = self.workspace.worktree_path(&agent_id)?;

        self.workspace.add_agent(record.clone())?;
        emit_status(&app, &agent_id, AgentStatus::Spawning, None);

        let sup = self.clone();
        let app_for_task = app.clone();
        let id_for_task = agent_id.clone();
        tauri::async_runtime::spawn(async move {
            emit_progress(&sup.workspace, &app_for_task, &id_for_task, "Creating git worktree...");

            if let Err(e) = std::fs::create_dir_all(git::worktrees_dir(&repo_path)) {
                let err = e.to_string();
                let _ = sup.workspace.update_agent_status(
                    &id_for_task,
                    AgentStatus::Error,
                    Some(err.clone()),
                );
                emit_status(&app_for_task, &id_for_task, AgentStatus::Error, Some(err));
                return;
            }

            if let Err(e) = git::worktree_add(&repo_path, &worktree, &branch).await {
                let err = e.to_string();
                let _ = sup.workspace.update_agent_status(
                    &id_for_task,
                    AgentStatus::Error,
                    Some(err.clone()),
                );
                emit_status(&app_for_task, &id_for_task, AgentStatus::Error, Some(err));
                return;
            }

            // Give React a tick to mount the view before claude starts.
            // PTY mode is sensitive to early terminal-negotiation bytes;
            // custom mode is less so but it costs nothing to wait.
            tokio::time::sleep(std::time::Duration::from_millis(350)).await;

            emit_progress(&sup.workspace, &app_for_task, &id_for_task, "Launching claude...");

            match sup.start_process(&app_for_task, &id_for_task, true).await {
                Ok(()) => {}
                Err(e) => {
                    let err = e.to_string();
                    let _ = git::worktree_remove(&repo_path, &worktree, true).await;
                    let _ = sup.workspace.update_agent_status(
                        &id_for_task,
                        AgentStatus::Error,
                        Some(err.clone()),
                    );
                    emit_status(&app_for_task, &id_for_task, AgentStatus::Error, Some(err));
                }
            }
        });

        Ok(record)
    }

    /// Spawn the claude process matching the record's current view and
    /// register it in the agents map. Assumes the worktree already
    /// exists. `fresh=true` only on the very first spawn (uses
    /// --session-id); subsequent spawns (view switches) use --resume.
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

        let changed = self.workspace.update_agent_status_if(
            &agent_id_str,
            AgentStatus::Running,
            None,
            |status| matches!(status, AgentStatus::Spawning),
        );
        if matches!(changed, Ok(true)) {
            emit_status(&app, &agent_id_str, AgentStatus::Running, None);
        }
        Ok(())
    }

    pub fn write_to_agent(&self, agent_id: &str, bytes: &[u8]) -> Result<()> {
        let agents = self.agents.lock();
        let agent = agents
            .get(agent_id)
            .ok_or_else(|| Error::AgentNotFound(agent_id.to_string()))?;
        agent.write_pty(bytes)
    }

    pub fn send_user_message(&self, agent_id: &str, text: &str) -> Result<()> {
        let agents = self.agents.lock();
        let agent = agents
            .get(agent_id)
            .ok_or_else(|| Error::AgentNotFound(agent_id.to_string()))?;
        agent.send_user_message(text)
    }

    pub fn resize_agent(&self, agent_id: &str, cols: u16, rows: u16) -> Result<()> {
        let agents = self.agents.lock();
        let agent = agents
            .get(agent_id)
            .ok_or_else(|| Error::AgentNotFound(agent_id.to_string()))?;
        agent.resize(cols, rows)
    }

    /// Switch the agent's view. Kills the current process, updates the
    /// record, and respawns the other variant against the same
    /// session_id via --resume.
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

        // 1. Tear down the current process. The exit callback may fire
        //    asynchronously; we don't wait for it — the new spawn will
        //    overwrite the agents-map entry once it's ready.
        if let Some(agent) = self.agents.lock().remove(agent_id) {
            let _ = agent.shutdown();
        }

        // 2. Mark spawning + update view on the record + notify frontend.
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

        // 3. Tiny gap so the frontend can swap the view component before
        //    the new process starts emitting output / events.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        // 4. Spawn the new variant with --resume.
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
    let sup_for_exit = sup;
    let app_for_exit = app.clone();
    let id_for_exit = agent_id.clone();
    Agent::spawn_pty(
        spec,
        move |bytes| {
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
    let sup_for_exit = sup;
    let app_for_exit = app.clone();
    let id_for_exit = agent_id.clone();
    Agent::spawn_managed(
        spec,
        move |event| {
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

/// Apply an exit callback only if this generation is still the
/// "current" one for the agent. A stale generation means we already
/// replaced this process (via switch_view) and the new spawn owns the
/// status; touching it here would clobber the live process's state.
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

    // Process this generation owns died on its own. Remove from the map
    // (if still present) and mark the record.
    sup.agents.lock().remove(agent_id);

    if success {
        let changed = sup.workspace.update_agent_status_if(
            agent_id,
            AgentStatus::Stopped,
            None,
            |status| matches!(status, AgentStatus::Running | AgentStatus::Spawning),
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
            |status| matches!(status, AgentStatus::Running | AgentStatus::Spawning),
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
