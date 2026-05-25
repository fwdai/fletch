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
        emit_progress(&self.workspace, &app, &agent_id, "Creating git worktree…");

        // 1. Create the worktree on the new branch.
        std::fs::create_dir_all(git::worktrees_dir(&repo_path))?;
        if let Err(e) = git::worktree_add(&repo_path, &worktree, &branch).await {
            self.workspace.update_agent_status(
                &agent_id,
                AgentStatus::Error,
                Some(e.to_string()),
            )?;
            emit_status(&app, &agent_id, AgentStatus::Error, Some(e.to_string()));
            return Err(e);
        }

        // 2. Spawn the sandboxed claude inside the worktree.
        emit_progress(
            &self.workspace,
            &app,
            &agent_id,
            "Launching claude inside sandbox-exec…",
        );

        let app_for_output = app.clone();
        let id_for_output = agent_id.clone();
        let agent = Agent::spawn(
            SpawnSpec {
                agent_id: &agent_id,
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
        );

        match agent {
            Ok(agent) => {
                self.agents.lock().insert(agent_id.clone(), agent);
                self.workspace
                    .update_agent_status(&agent_id, AgentStatus::Running, None)?;
                emit_status(&app, &agent_id, AgentStatus::Running, None);
                let updated = find_record(&self.workspace.current(), &agent_id)
                    .unwrap_or(record);
                Ok(updated)
            }
            Err(e) => {
                let err = e.to_string();
                // Roll back the worktree so a retry isn't blocked.
                let _ = git::worktree_remove(&repo_path, &worktree, true).await;
                self.workspace
                    .update_agent_status(&agent_id, AgentStatus::Error, Some(err.clone()))?;
                emit_status(&app, &agent_id, AgentStatus::Error, Some(err));
                Err(e)
            }
        }
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

fn find_record(ws: &Option<Workspace>, agent_id: &str) -> Option<AgentRecord> {
    ws.as_ref()?
        .agents
        .iter()
        .find(|a| a.id == agent_id)
        .cloned()
}
