//! Coordinator between Tauri IPC commands and the running agents.
//!
//! Owns the live `Agent` map plus the `WorkspaceManager` and `Vm`. All
//! frontend-initiated mutations funnel through here.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

use crate::agent::{Agent, SpawnSpec};
use crate::error::{Error, Result};
use crate::git;
use crate::vm::Vm;
use crate::workspace::{new_agent_record, AgentRecord, AgentStatus, Workspace, WorkspaceManager};

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
}

/// Path to the SSH key pair the host uses to authenticate to guests.
/// Generated lazily on first spawn if absent.
pub struct KeyMaterial {
    pub private_key: PathBuf,
    pub public_key: PathBuf,
}

pub struct Supervisor {
    pub workspace: Arc<WorkspaceManager>,
    pub vm: Arc<Vm>,
    pub keys: KeyMaterial,
    /// Live agents keyed by agent id. Wrapped in a sync mutex because agents
    /// also need to be torn down from inside async command handlers; we hold
    /// the lock only across cheap map ops, never across awaits.
    pub agents: Mutex<HashMap<String, Agent>>,
}

impl Supervisor {
    pub fn new(workspace: Arc<WorkspaceManager>, vm: Arc<Vm>, keys: KeyMaterial) -> Self {
        Self {
            workspace,
            vm,
            keys,
            agents: Mutex::new(HashMap::new()),
        }
    }

    pub fn current_workspace(&self) -> Option<Workspace> {
        self.workspace.current()
    }

    pub fn set_repo(&self, repo_path: PathBuf, base_image: String) -> Result<Workspace> {
        self.workspace.set_repo(repo_path, base_image)
    }

    pub async fn spawn_agent(
        &self,
        app: AppHandle,
        name: String,
        branch: String,
        task: String,
    ) -> Result<AgentRecord> {
        let repo_path = self.workspace.repo_path()?;
        let base_image = self.workspace.base_image()?;

        let record = new_agent_record(name, branch.clone(), task.clone());
        let agent_id = record.id.clone();
        let vm_name = format!("algiers-{}", agent_id);
        let worktree = self.workspace.worktree_path(&agent_id)?;

        // Persist as Spawning before we do anything destructive.
        self.workspace.add_agent(record.clone())?;
        emit_status(&app, &agent_id, AgentStatus::Spawning, None);

        // Run the full spawn flow; on error we roll back the worktree and
        // surface the failure as Error status.
        let spawn_result = self
            .do_spawn(&app, &repo_path, &branch, &agent_id, &vm_name, &worktree, &base_image, &task)
            .await;

        match spawn_result {
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
                let err_str = e.to_string();
                let _ = git::worktree_remove(&repo_path, &worktree, true).await;
                self.workspace
                    .update_agent_status(&agent_id, AgentStatus::Error, Some(err_str.clone()))?;
                emit_status(&app, &agent_id, AgentStatus::Error, Some(err_str));
                Err(e)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn do_spawn(
        &self,
        app: &AppHandle,
        repo_path: &std::path::Path,
        branch: &str,
        agent_id: &str,
        vm_name: &str,
        worktree: &std::path::Path,
        base_image: &str,
        task: &str,
    ) -> Result<Agent> {
        // 1. Worktree first — fails fast on conflict, doesn't touch the VM.
        std::fs::create_dir_all(git::worktrees_dir(repo_path))?;
        git::worktree_add(repo_path, worktree, branch).await?;

        // 2. Hand off to Agent::spawn for VM + SSH PTY.
        let app2 = app.clone();
        let id_owned = agent_id.to_string();
        let agent = Agent::spawn(
            self.vm.clone(),
            SpawnSpec {
                agent_id,
                vm_name,
                base_image,
                worktree: worktree.to_path_buf(),
                task,
                key_path: self.keys.private_key.clone(),
                cols: 120,
                rows: 32,
            },
            move |bytes| {
                let _ = app2.emit(
                    "agent:output",
                    AgentOutputPayload {
                        agent_id: id_owned.clone(),
                        bytes,
                    },
                );
            },
        )
        .await?;
        Ok(agent)
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

    pub async fn stop_agent(&self, app: AppHandle, agent_id: &str) -> Result<()> {
        // Pop the agent out of the map without holding the lock across the
        // async shutdown.
        let agent = self
            .agents
            .lock()
            .remove(agent_id)
            .ok_or_else(|| Error::AgentNotFound(agent_id.to_string()))?;
        let res = agent.shutdown(self.vm.clone()).await;
        let (status, last_err) = match &res {
            Ok(_) => (AgentStatus::Stopped, None),
            Err(e) => (AgentStatus::Error, Some(e.to_string())),
        };
        self.workspace
            .update_agent_status(agent_id, status.clone(), last_err.clone())?;
        emit_status(&app, agent_id, status, last_err);
        res
    }

    pub async fn discard_worktree(&self, agent_id: &str) -> Result<()> {
        let repo = self.workspace.repo_path()?;
        let worktree = self.workspace.worktree_path(agent_id)?;
        git::worktree_remove(&repo, &worktree, true).await?;
        self.workspace.remove_agent(agent_id)?;
        Ok(())
    }
}

fn emit_status(app: &AppHandle, agent_id: &str, status: AgentStatus, last_error: Option<String>) {
    let _ = app.emit(
        "agent:status",
        AgentStatusPayload {
            agent_id: agent_id.to_string(),
            status,
            last_error,
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
