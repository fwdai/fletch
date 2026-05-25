//! Workspace state — the on-disk record of which repo is active and
//! the agents that belong to it.

use chrono::Utc;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::git;
use crate::names;

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
    /// `None` until the user sends their first message — we defer
    /// branch creation so spawned-but-unused agents don't pollute
    /// `git branch`. Set to `amux/<slug>` once the first user message
    /// gives us something to name it after.
    #[serde(default)]
    pub branch: Option<String>,
    /// The branch the agent's worktree was created from. Captured at
    /// spawn time so the UI can show "→ target: main" and so future
    /// "make a PR" flows know the merge target. `None` for agents
    /// spawned in detached HEAD state or before this field existed.
    #[serde(default)]
    pub parent_branch: Option<String>,
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

        // Reconcile stale statuses left over from a crash / clean shutdown.
        // The in-memory Supervisor is fresh on every start, so any agent
        // marked Running / Idle / Spawning on disk has no live process.
        // We flip those back to Spawning so the supervisor's auto-resume
        // pass picks them up — agents the user explicitly Stopped (status
        // Stopped) stay stopped, available via the manual Resume button.
        let mut dirty = false;
        if let Some(ws) = inner.current.as_mut() {
            for a in ws.agents.iter_mut() {
                if matches!(
                    a.status,
                    AgentStatus::Running | AgentStatus::Spawning | AgentStatus::Idle
                ) {
                    a.status = AgentStatus::Spawning;
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

    /// Pick a memorable, unused id for a new agent. Reads the current
    /// agents under the workspace lock so two consecutive calls
    /// observe each other's writes; if you need full atomicity
    /// against `add_agent`, do them back-to-back without anything
    /// else mutating the agents list in between.
    pub fn allocate_agent_id(&self) -> Result<String> {
        let g = self.inner.read();
        let ws = g.current.as_ref().ok_or(Error::WorkspaceNotLoaded)?;
        let used: HashSet<String> = ws.agents.iter().map(|a| a.id.clone()).collect();
        Ok(names::allocate(&used))
    }

    pub fn add_agent(&self, record: AgentRecord) -> Result<()> {
        {
            let mut g = self.inner.write();
            let ws = g.current.as_mut().ok_or(Error::WorkspaceNotLoaded)?;
            ws.agents.push(record);
        }
        self.persist()
    }

    /// Mutate one agent under the workspace write lock and persist iff
    /// the closure returns true. The single primitive every callable
    /// update_/set_ helper builds on.
    fn mutate_agent<F>(&self, id: &str, f: F) -> Result<bool>
    where
        F: FnOnce(&mut AgentRecord) -> bool,
    {
        let changed = {
            let mut g = self.inner.write();
            let ws = g.current.as_mut().ok_or(Error::WorkspaceNotLoaded)?;
            let a = ws
                .agents
                .iter_mut()
                .find(|a| a.id == id)
                .ok_or_else(|| Error::AgentNotFound(id.to_string()))?;
            f(a)
        };
        if changed {
            self.persist()?;
        }
        Ok(changed)
    }

    pub fn update_agent_status(
        &self,
        id: &str,
        status: AgentStatus,
        last_error: Option<String>,
    ) -> Result<()> {
        self.mutate_agent(id, |a| {
            a.status = status;
            if last_error.is_some() {
                a.last_error = last_error;
            }
            true
        })?;
        Ok(())
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
        self.mutate_agent(id, |a| {
            if !predicate(&a.status) {
                return false;
            }
            a.status = status;
            if last_error.is_some() {
                a.last_error = last_error;
            }
            true
        })
    }

    /// Record the first user message as the agent's task — but only
    /// if the task hasn't been set yet. Returns true if it actually
    /// wrote (so callers can decide whether to emit an event).
    pub fn set_agent_task_if_empty(&self, id: &str, task: &str) -> Result<bool> {
        self.mutate_agent(id, |a| {
            if !a.task.trim().is_empty() {
                return false;
            }
            a.task = task.to_string();
            true
        })
    }

    /// Set the agent's branch — but only if it isn't set yet. Used
    /// when the first user message triggers branch creation. Returns
    /// true iff it actually wrote.
    pub fn set_agent_branch_if_empty(&self, id: &str, branch: &str) -> Result<bool> {
        self.mutate_agent(id, |a| {
            if a.branch.is_some() {
                return false;
            }
            a.branch = Some(branch.to_string());
            true
        })
    }

    pub fn update_agent_view(&self, id: &str, view: AgentView) -> Result<()> {
        self.mutate_agent(id, |a| {
            a.view = view;
            true
        })?;
        Ok(())
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
    id: String,
    name: String,
    branch: Option<String>,
    parent_branch: Option<String>,
    task: String,
    view: AgentView,
) -> AgentRecord {
    AgentRecord {
        id,
        name,
        branch,
        parent_branch,
        task,
        status: AgentStatus::Spawning,
        view,
        // Full UUID for claude's --session-id; reused on every respawn
        // (e.g. view switch) so the conversation persists.
        session_id: Some(uuid::Uuid::new_v4().to_string()),
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
    fn persists_across_instances_and_reconciles_to_spawning_for_resume() {
        let td = tmpdir();
        let app_dir = td.path().to_path_buf();
        let repo = init_repo(td.path());

        {
            let wm = WorkspaceManager::new(app_dir.clone()).unwrap();
            wm.set_repo(repo.clone()).unwrap();
            let mut running = new_agent_record("yosemite".into(), "a".into(), Some("b".into()), None, "c".into(), AgentView::Custom);
            running.status = AgentStatus::Running;
            wm.add_agent(running).unwrap();

            let mut stopped = new_agent_record("dolomites".into(), "s".into(), Some("sb".into()), None, "sc".into(), AgentView::Custom);
            stopped.status = AgentStatus::Stopped;
            wm.add_agent(stopped).unwrap();
        }

        let wm2 = WorkspaceManager::new(app_dir).unwrap();
        let cur = wm2.current().unwrap();
        assert_eq!(cur.repo_path, repo);
        assert_eq!(cur.agents.len(), 2);
        // Previously-running agent flagged for auto-resume.
        assert_eq!(cur.agents[0].status, AgentStatus::Spawning);
        // Previously-stopped agent stays stopped (manual Resume required).
        assert_eq!(cur.agents[1].status, AgentStatus::Stopped);
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
        let rec = new_agent_record("test-id".into(), "a".into(), Some("b".into()), None, "c".into(), AgentView::Custom);
        let id = rec.id.clone();
        wm.add_agent(rec).unwrap();

        wm.update_agent_status(&id, AgentStatus::Running, None)
            .unwrap();
        let cur = wm.current().unwrap();
        assert_eq!(cur.agents[0].status, AgentStatus::Running);
    }
}
