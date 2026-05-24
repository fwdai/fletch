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
use crate::baker::{self, BakeSpec};
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
    /// Exclusive flag so only one base-image bake runs at a time. Bakes
    /// stomp on each other badly (the upstream image is cloned by name) so
    /// we serialize.
    pub baking: Mutex<bool>,
}

impl Supervisor {
    pub fn new(workspace: Arc<WorkspaceManager>, vm: Arc<Vm>, keys: KeyMaterial) -> Self {
        Self {
            workspace,
            vm,
            keys,
            agents: Mutex::new(HashMap::new()),
            baking: Mutex::new(false),
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

    /// Stop a live agent. Idempotent — if the agent isn't in the live map
    /// (e.g. it was already stopped, or it's a leftover record from an app
    /// restart), we still attempt to clean up the named VM and mark the
    /// state record as stopped so the UI is consistent.
    pub async fn stop_agent(&self, app: AppHandle, agent_id: &str) -> Result<()> {
        let live_agent = self.agents.lock().remove(agent_id);

        let (status, last_err) = if let Some(agent) = live_agent {
            match agent.shutdown(self.vm.clone()).await {
                Ok(_) => (AgentStatus::Stopped, None),
                Err(e) => (AgentStatus::Error, Some(e.to_string())),
            }
        } else {
            // No in-memory handle. The VM may still exist (e.g., app
            // restart left it running) — best-effort kill it so the user
            // doesn't have to drop to a terminal.
            let vm_name = format!("algiers-{}", agent_id);
            if self.vm.exists(&vm_name).await.unwrap_or(false) {
                let _ = self.vm.stop(&vm_name).await;
                let _ = self.vm.delete(&vm_name).await;
            }
            (AgentStatus::Stopped, None)
        };

        // The record may already have been removed (e.g., concurrent
        // discard). Tolerate that case by ignoring AgentNotFound errors
        // from the status update.
        match self
            .workspace
            .update_agent_status(agent_id, status.clone(), last_err.clone())
        {
            Ok(_) | Err(Error::AgentNotFound(_)) => {}
            Err(e) => return Err(e),
        }
        emit_status(&app, agent_id, status, last_err);
        Ok(())
    }

    /// Tear down everything associated with an agent: stop & delete its VM
    /// if still around, unregister and delete its worktree if still around,
    /// and ALWAYS remove the agent record from workspace state.
    ///
    /// Every step is best-effort and tolerated-on-failure. The whole point
    /// of this function is to be the "I don't care what state things are
    /// in, just make it go away" button — the previous version would bail
    /// on the first error and leave the agent stuck in the list forever.
    pub async fn discard_worktree(&self, agent_id: &str) -> Result<()> {
        let repo = self.workspace.repo_path()?;
        let worktree = self.workspace.worktree_path(agent_id)?;
        let vm_name = format!("algiers-{}", agent_id);
        // Capture the branch up-front — we still want to delete it even if
        // the agent record gets removed mid-cleanup somehow.
        let branch = self
            .workspace
            .current()
            .and_then(|ws| ws.agents.iter().find(|a| a.id == agent_id).map(|a| a.branch.clone()));

        // 1. Kill any live in-memory Agent handle. The PTY drop also kills
        //    the SSH session; the `tart run` child gets killed via
        //    `kill_on_drop` when we drop Agent. Best-effort.
        if let Some(agent) = self.agents.lock().remove(agent_id) {
            drop(agent);
        }

        // 2. Stop + delete the VM if it exists. Either step may legitimately
        //    fail (VM never created, already deleted, etc.).
        if self.vm.exists(&vm_name).await.unwrap_or(false) {
            if let Err(e) = self.vm.stop(&vm_name).await {
                tracing::warn!(%vm_name, error = %e, "discard: tart stop failed");
            }
            if let Err(e) = self.vm.delete(&vm_name).await {
                tracing::warn!(%vm_name, error = %e, "discard: tart delete failed");
            }
        }

        // 3. Worktree cleanup. First `git worktree prune` to clear out any
        //    internal refs that point at a missing directory, then attempt
        //    the registered removal, then fall back to removing the
        //    directory directly if it's still there.
        let _ = git::worktree_prune(&repo).await;
        if let Err(e) = git::worktree_remove(&repo, &worktree, true).await {
            tracing::warn!(path = %worktree.display(), error = %e, "discard: git worktree remove failed; trying fs fallback");
            if worktree.exists() {
                if let Err(e) = std::fs::remove_dir_all(&worktree) {
                    tracing::warn!(path = %worktree.display(), error = %e, "discard: fs remove failed too");
                }
            }
        }

        // 4. Delete the agent's branch (best-effort). The "Remove" action
        //    implies the user is done with this agent's work; leaving the
        //    branch around just clutters `git branch`. Recoverable via
        //    reflog for ~90 days if the user changes their mind.
        if let Some(branch) = branch {
            if let Err(e) = git::branch_delete(&repo, &branch).await {
                tracing::warn!(%branch, error = %e, "discard: branch delete failed");
            }
        }

        // 5. ALWAYS remove the agent record so the UI doesn't leave the
        //    user with stuck rows they can't get rid of.
        self.workspace.remove_agent(agent_id)?;
        Ok(())
    }

    /// Run the in-app base-image bake. Streams progress as `bake:progress`
    /// events on the supplied AppHandle. Errors are still emitted as a final
    /// progress event with `stage: Error` so the UI sees the message even if
    /// the Tauri command Result is mishandled.
    pub async fn bake_base_image(
        self: Arc<Self>,
        app: AppHandle,
        image_name: String,
    ) -> Result<()> {
        {
            let mut guard = self.baking.lock();
            if *guard {
                return Err(Error::Other(
                    "a base-image build is already in progress".into(),
                ));
            }
            *guard = true;
        }

        let res = baker::bake_base_image(
            self.vm.clone(),
            BakeSpec {
                image_name: &image_name,
                public_key_path: &self.keys.public_key,
            },
            move |progress| {
                let _ = app.emit("bake:progress", progress);
            },
        )
        .await;

        *self.baking.lock() = false;
        res
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
