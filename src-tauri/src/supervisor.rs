//! Coordinator between Tauri IPC commands and the running agents.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

use crate::agent::{Agent, SpawnSpec};
use crate::error::{Error, Result};
use crate::git;
use crate::workspace::{
    new_agent_record, AgentRecord, AgentStatus, Workspace, WorkspaceManager,
};

#[derive(Clone, serde::Serialize)]
pub struct AgentOutputPayload {
    pub agent_id: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, serde::Serialize)]
pub struct AgentStatusPayload {
    pub agent_id: String,
    pub status: AgentStatus,
    pub last_error: Option<String>,
    #[serde(default)]
    pub status_message: Option<String>,
}

pub struct Supervisor {
    pub workspace: Arc<WorkspaceManager>,
    pub agents: Mutex<HashMap<String, Agent>>,
}

impl Supervisor {
    pub fn new(workspace: Arc<WorkspaceManager>) -> Self {
        Self {
            workspace,
            agents: Mutex::new(HashMap::new()),
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
    ) -> Result<AgentRecord> {
        let repo_path = self.workspace.repo_path()?;

        let record = new_agent_record(name, branch.clone(), task.clone());
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

            // Give React a tick to mount xterm before Claude starts terminal
            // negotiation. Starting the child before a terminal is attached can
            // drop startup control sequences and leave Claude waiting silently.
            tokio::time::sleep(std::time::Duration::from_millis(350)).await;

            emit_progress(&sup.workspace, &app_for_task, &id_for_task, "Launching claude...");

            let app_for_output = app_for_task.clone();
            let id_for_output = id_for_task.clone();
            let sup_for_exit = sup.clone();
            let app_for_exit = app_for_task.clone();
            let id_for_exit = id_for_task.clone();
            let agent = Agent::spawn(
                SpawnSpec {
                    agent_id: &id_for_task,
                    worktree: worktree.clone(),
                    task: &task,
                    cols: 120,
                    rows: 32,
                },
                move |bytes| {
                    let len = bytes.len();
                    if let Err(e) = app_for_output.emit(
                        "agent:output",
                        AgentOutputPayload {
                            agent_id: id_for_output.clone(),
                            bytes,
                        },
                    ) {
                        tracing::warn!(error = %e, agent_id = %id_for_output, "emit agent:output failed");
                    } else {
                        tracing::debug!(
                            agent_id = %id_for_output,
                            bytes = len,
                            "emitted agent:output"
                        );
                    }
                },
                move |exit| {
                    sup_for_exit.agents.lock().remove(&id_for_exit);

                    if exit.success {
                        let changed = sup_for_exit.workspace.update_agent_status_if(
                            &id_for_exit,
                            AgentStatus::Stopped,
                            None,
                            |status| matches!(status, AgentStatus::Running | AgentStatus::Spawning),
                        );
                        if matches!(changed, Ok(true)) {
                            emit_status(&app_for_exit, &id_for_exit, AgentStatus::Stopped, None);
                        }
                    } else {
                        let err = format!("Agent process exited: {}", exit.message);
                        let changed = sup_for_exit.workspace.update_agent_status_if(
                            &id_for_exit,
                            AgentStatus::Error,
                            Some(err.clone()),
                            |status| matches!(status, AgentStatus::Running | AgentStatus::Spawning),
                        );
                        if matches!(changed, Ok(true)) {
                            emit_status(&app_for_exit, &id_for_exit, AgentStatus::Error, Some(err));
                        }
                    }
                },
            );

            match agent {
                Ok(agent) => {
                    sup.agents.lock().insert(id_for_task.clone(), agent);
                    let changed = sup.workspace.update_agent_status_if(
                        &id_for_task,
                        AgentStatus::Running,
                        None,
                        |status| matches!(status, AgentStatus::Spawning),
                    );
                    if matches!(changed, Ok(true)) {
                        emit_status(&app_for_task, &id_for_task, AgentStatus::Running, None);
                    } else {
                        sup.agents.lock().remove(&id_for_task);
                    }
                }
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

    pub fn write_to_agent(&self, agent_id: &str, bytes: &[u8]) -> Result<()> {
        let agents = self.agents.lock();
        let agent = agents
            .get(agent_id)
            .ok_or_else(|| Error::AgentNotFound(agent_id.to_string()))?;
        agent.write(bytes)
    }

    pub fn resize_agent(&self, agent_id: &str, cols: u16, rows: u16) -> Result<()> {
        let agents = self.agents.lock();
        let agent = agents
            .get(agent_id)
            .ok_or_else(|| Error::AgentNotFound(agent_id.to_string()))?;
        agent.resize(cols, rows)
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

    /// Universal "make this agent go away" — kill the process, remove
    /// the worktree, delete the branch, drop the record.
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

        // 1. Kill the live process if any.
        if let Some(agent) = self.agents.lock().remove(agent_id) {
            let _ = agent.shutdown();
        }

        // 2. Worktree cleanup (idempotent across all the failure shapes).
        let _ = git::worktree_prune(&repo).await;
        if let Err(e) = git::worktree_remove(&repo, &worktree, true).await {
            tracing::warn!(error = %e, "discard: worktree remove failed; trying fs fallback");
            if worktree.exists() {
                let _ = std::fs::remove_dir_all(&worktree);
            }
        }

        // 3. Branch cleanup.
        if let Some(branch) = branch {
            if let Err(e) = git::branch_delete(&repo, &branch).await {
                tracing::warn!(%branch, error = %e, "discard: branch delete failed");
            }
        }

        // 4. Always drop the record.
        self.workspace.remove_agent(agent_id)?;
        Ok(())
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
