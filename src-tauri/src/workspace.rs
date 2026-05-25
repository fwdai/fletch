//! Workspace state — the on-disk record of which repo is active and
//! the agents that belong to it.

use chrono::Utc;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::git;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Spawning,
    Running,
    Idle,
    Stopped,
    Error,
}

/// Which view the user has open for an agent. Both views attach to the
/// same conversation (via claude's --session-id / --resume), but each
/// view requires a different process shape, so only one can be live at
/// a time per agent. The user can toggle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentView {
    /// Structured chat UI rendered from claude's stream-json events.
    Custom,
    /// Read-only xterm showing claude's native TUI, with our input box
    /// overlaid on top of the claude input prompt.
    Native,
}

impl Default for AgentView {
    fn default() -> Self {
        AgentView::Custom
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRecord {
    pub id: String,
    pub name: String,
    pub branch: String,
    pub task: String,
    pub status: AgentStatus,
    #[serde(default)]
    pub view: AgentView,
    /// UUID claude uses to persist this agent's conversation. Set on
    /// first spawn; used with --resume on subsequent process spawns
    /// (e.g. when the user switches views).
    #[serde(default)]
    pub session_id: Option<String>,
    pub created_at: String,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub status_message: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Workspace {
    pub repo_path: PathBuf,
    #[serde(default)]
    pub agents: Vec<AgentRecord>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PersistedState {
    #[serde(default)]
    current: Option<Workspace>,
}

pub struct WorkspaceManager {
    state_file: PathBuf,
    inner: RwLock<PersistedState>,
}

impl WorkspaceManager {
    pub fn new(app_data_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&app_data_dir)?;
        let state_file = app_data_dir.join("workspaces.json");
        let mut inner: PersistedState = if state_file.exists() {
            let raw = std::fs::read_to_string(&state_file)?;
            serde_json::from_str(&raw).unwrap_or_default()
        } else {
            PersistedState::default()
        };

        // Reconcile stale statuses left over from a crash / forced quit.
        // The in-memory Supervisor is fresh on every start, so any agent
        // marked Running or Spawning on disk is bogus — flip to Stopped.
        let mut dirty = false;
        if let Some(ws) = inner.current.as_mut() {
            for a in ws.agents.iter_mut() {
                if matches!(a.status, AgentStatus::Running | AgentStatus::Spawning) {
                    a.status = AgentStatus::Stopped;
                    if a.last_error.is_none() {
                        a.last_error = Some(
                            "App restarted while agent was running. Remove and re-spawn."
                                .into(),
                        );
                    }
                    a.status_message = None;
                    dirty = true;
                }
            }
        }

        let mgr = Self {
            state_file,
            inner: RwLock::new(inner),
        };
        if dirty {
            mgr.persist()?;
        }
        Ok(mgr)
    }

    pub fn current(&self) -> Option<Workspace> {
        self.inner.read().current.clone()
    }

    pub fn set_repo(&self, repo_path: PathBuf) -> Result<Workspace> {
        if !repo_path.join(".git").exists() {
            return Err(Error::InvalidPath(format!(
                "not a git repository: {}",
                repo_path.display()
            )));
        }
        let ws = Workspace {
            repo_path,
            agents: vec![],
        };
        self.inner.write().current = Some(ws.clone());
        self.persist()?;
        Ok(ws)
    }

    pub fn add_agent(&self, record: AgentRecord) -> Result<()> {
        {
            let mut g = self.inner.write();
            let ws = g.current.as_mut().ok_or(Error::WorkspaceNotLoaded)?;
            ws.agents.push(record);
        }
        self.persist()
    }

    pub fn update_agent_status(
        &self,
        id: &str,
        status: AgentStatus,
        last_error: Option<String>,
    ) -> Result<()> {
        {
            let mut g = self.inner.write();
            let ws = g.current.as_mut().ok_or(Error::WorkspaceNotLoaded)?;
            let a = ws
                .agents
                .iter_mut()
                .find(|a| a.id == id)
                .ok_or_else(|| Error::AgentNotFound(id.to_string()))?;
            a.status = status;
            if let Some(err) = last_error {
                a.last_error = Some(err);
            }
            if matches!(
                a.status,
                AgentStatus::Running | AgentStatus::Stopped | AgentStatus::Idle | AgentStatus::Error
            ) {
                a.status_message = None;
            }
        }
        self.persist()
    }

    pub fn update_agent_status_if<F>(
        &self,
        id: &str,
        status: AgentStatus,
        last_error: Option<String>,
        predicate: F,
    ) -> Result<bool>
    where
        F: FnOnce(&AgentStatus) -> bool,
    {
        let changed = {
            let mut g = self.inner.write();
            let ws = g.current.as_mut().ok_or(Error::WorkspaceNotLoaded)?;
            let a = ws
                .agents
                .iter_mut()
                .find(|a| a.id == id)
                .ok_or_else(|| Error::AgentNotFound(id.to_string()))?;
            if !predicate(&a.status) {
                return Ok(false);
            }
            a.status = status;
            if let Some(err) = last_error {
                a.last_error = Some(err);
            }
            if matches!(
                a.status,
                AgentStatus::Running | AgentStatus::Stopped | AgentStatus::Idle | AgentStatus::Error
            ) {
                a.status_message = None;
            }
            true
        };
        if changed {
            self.persist()?;
        }
        Ok(changed)
    }

    pub fn update_agent_view(&self, id: &str, view: AgentView) -> Result<()> {
        {
            let mut g = self.inner.write();
            let ws = g.current.as_mut().ok_or(Error::WorkspaceNotLoaded)?;
            let a = ws
                .agents
                .iter_mut()
                .find(|a| a.id == id)
                .ok_or_else(|| Error::AgentNotFound(id.to_string()))?;
            a.view = view;
        }
        self.persist()
    }

    pub fn agent(&self, id: &str) -> Result<AgentRecord> {
        let g = self.inner.read();
        let ws = g.current.as_ref().ok_or(Error::WorkspaceNotLoaded)?;
        ws.agents
            .iter()
            .find(|a| a.id == id)
            .cloned()
            .ok_or_else(|| Error::AgentNotFound(id.to_string()))
    }

    pub fn update_agent_status_message(&self, id: &str, message: Option<String>) -> Result<()> {
        {
            let mut g = self.inner.write();
            let ws = g.current.as_mut().ok_or(Error::WorkspaceNotLoaded)?;
            let a = ws
                .agents
                .iter_mut()
                .find(|a| a.id == id)
                .ok_or_else(|| Error::AgentNotFound(id.to_string()))?;
            a.status_message = message;
        }
        self.persist()
    }

    pub fn remove_agent(&self, id: &str) -> Result<()> {
        {
            let mut g = self.inner.write();
            let ws = g.current.as_mut().ok_or(Error::WorkspaceNotLoaded)?;
            ws.agents.retain(|a| a.id != id);
        }
        self.persist()
    }

    pub fn worktree_path(&self, agent_id: &str) -> Result<PathBuf> {
        let g = self.inner.read();
        let ws = g.current.as_ref().ok_or(Error::WorkspaceNotLoaded)?;
        Ok(git::worktrees_dir(&ws.repo_path).join(agent_id))
    }

    pub fn repo_path(&self) -> Result<PathBuf> {
        let g = self.inner.read();
        Ok(g.current
            .as_ref()
            .ok_or(Error::WorkspaceNotLoaded)?
            .repo_path
            .clone())
    }

    fn persist(&self) -> Result<()> {
        let snapshot = self.inner.read();
        let raw = serde_json::to_string_pretty(&*snapshot)?;
        atomic_write(&self.state_file, raw.as_bytes())
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn new_agent_record(
    name: String,
    branch: String,
    task: String,
    view: AgentView,
) -> AgentRecord {
    AgentRecord {
        id: uuid::Uuid::new_v4()
            .to_string()
            .split('-')
            .next()
            .unwrap_or("agent")
            .to_string(),
        name,
        branch,
        task,
        status: AgentStatus::Spawning,
        view,
        // Full UUID for claude's --session-id; reused on every respawn
        // (e.g. view switch) so the conversation persists.
        session_id: Some(uuid::Uuid::new_v4().to_string()),
        created_at: Utc::now().to_rfc3339(),
        last_error: None,
        status_message: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn init_repo(dir: &Path) -> PathBuf {
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        repo
    }

    #[test]
    fn persists_across_instances_and_reconciles_stale_running() {
        let td = tmpdir();
        let app_dir = td.path().to_path_buf();
        let repo = init_repo(td.path());

        {
            let wm = WorkspaceManager::new(app_dir.clone()).unwrap();
            wm.set_repo(repo.clone()).unwrap();
            let mut spawning = new_agent_record("a".into(), "b".into(), "c".into(), AgentView::Custom);
            spawning.status = AgentStatus::Spawning;
            wm.add_agent(spawning).unwrap();
        }

        let wm2 = WorkspaceManager::new(app_dir).unwrap();
        let cur = wm2.current().unwrap();
        assert_eq!(cur.repo_path, repo);
        assert_eq!(cur.agents.len(), 1);
        // Stale Spawning reconciled to Stopped on reload.
        assert_eq!(cur.agents[0].status, AgentStatus::Stopped);
    }

    #[test]
    fn rejects_non_repo_path() {
        let td = tmpdir();
        let wm = WorkspaceManager::new(td.path().to_path_buf()).unwrap();
        let err = wm.set_repo(td.path().join("nope")).unwrap_err();
        assert!(err.to_string().contains("not a git repository"));
    }

    #[test]
    fn agent_status_transitions() {
        let td = tmpdir();
        let repo = init_repo(td.path());
        let wm = WorkspaceManager::new(td.path().to_path_buf()).unwrap();
        wm.set_repo(repo).unwrap();
        let rec = new_agent_record("a".into(), "b".into(), "c".into(), AgentView::Custom);
        let id = rec.id.clone();
        wm.add_agent(rec).unwrap();

        wm.update_agent_status(&id, AgentStatus::Running, None)
            .unwrap();
        let cur = wm.current().unwrap();
        assert_eq!(cur.agents[0].status, AgentStatus::Running);
    }
}
