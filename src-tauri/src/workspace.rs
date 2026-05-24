//! Workspace state — the on-disk record of which repo is active, what its
//! base image is, and the agents that belong to it.
//!
//! State is persisted as JSON in the OS app-data directory. Reads and writes
//! are guarded by a single `RwLock`; concurrent access from Tauri command
//! handlers goes through here.

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRecord {
    pub id: String,
    pub name: String,
    pub branch: String,
    pub task: String,
    pub status: AgentStatus,
    pub created_at: String,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Workspace {
    pub repo_path: PathBuf,
    pub base_image: String,
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
        let inner = if state_file.exists() {
            let raw = std::fs::read_to_string(&state_file)?;
            serde_json::from_str(&raw).unwrap_or_default()
        } else {
            PersistedState::default()
        };
        Ok(Self {
            state_file,
            inner: RwLock::new(inner),
        })
    }

    pub fn current(&self) -> Option<Workspace> {
        self.inner.read().current.clone()
    }

    pub fn set_repo(&self, repo_path: PathBuf, base_image: String) -> Result<Workspace> {
        if !repo_path.join(".git").exists() {
            return Err(Error::InvalidPath(format!(
                "not a git repository: {}",
                repo_path.display()
            )));
        }
        let ws = Workspace {
            repo_path,
            base_image,
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

    pub fn base_image(&self) -> Result<String> {
        let g = self.inner.read();
        Ok(g.current
            .as_ref()
            .ok_or(Error::WorkspaceNotLoaded)?
            .base_image
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

pub fn new_agent_record(name: String, branch: String, task: String) -> AgentRecord {
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
        created_at: Utc::now().to_rfc3339(),
        last_error: None,
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
    fn persists_across_instances() {
        let td = tmpdir();
        let app_dir = td.path().to_path_buf();
        let repo = init_repo(td.path());

        {
            let wm = WorkspaceManager::new(app_dir.clone()).unwrap();
            wm.set_repo(repo.clone(), "base-dev".into()).unwrap();
            wm.add_agent(new_agent_record(
                "refactor".into(),
                "agent/abc".into(),
                "do thing".into(),
            ))
            .unwrap();
        }

        let wm2 = WorkspaceManager::new(app_dir).unwrap();
        let cur = wm2.current().unwrap();
        assert_eq!(cur.repo_path, repo);
        assert_eq!(cur.agents.len(), 1);
        assert_eq!(cur.agents[0].name, "refactor");
        assert_eq!(cur.agents[0].status, AgentStatus::Spawning);
    }

    #[test]
    fn rejects_non_repo_path() {
        let td = tmpdir();
        let wm = WorkspaceManager::new(td.path().to_path_buf()).unwrap();
        let err = wm
            .set_repo(td.path().join("nope"), "base".into())
            .unwrap_err();
        assert!(err.to_string().contains("not a git repository"));
    }

    #[test]
    fn agent_status_transitions() {
        let td = tmpdir();
        let repo = init_repo(td.path());
        let wm = WorkspaceManager::new(td.path().to_path_buf()).unwrap();
        wm.set_repo(repo, "base".into()).unwrap();
        let rec = new_agent_record("a".into(), "b".into(), "c".into());
        let id = rec.id.clone();
        wm.add_agent(rec).unwrap();

        wm.update_agent_status(&id, AgentStatus::Running, None)
            .unwrap();
        let cur = wm.current().unwrap();
        assert_eq!(cur.agents[0].status, AgentStatus::Running);

        wm.update_agent_status(&id, AgentStatus::Error, Some("oops".into()))
            .unwrap();
        let cur = wm.current().unwrap();
        assert_eq!(cur.agents[0].status, AgentStatus::Error);
        assert_eq!(cur.agents[0].last_error.as_deref(), Some("oops"));
    }
}
