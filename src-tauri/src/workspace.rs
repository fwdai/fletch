//! Workspace state — the on-disk record of the sidebar's repo list and
//! the agents the user has spawned. Each agent is anchored to a primary
//! repo (`repos[0]`); the sidebar groups agents by that primary.

use chrono::Utc;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
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

/// One repo an agent has a worktree in.
///
/// At spawn time every agent gets `repos[0]` populated from the
/// repo the user spawned it against. The user can extend this list
/// mid-session via `add_repo_to_agent`, which creates a sibling
/// worktree under the same parent dir.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedRepo {
    pub repo_path: PathBuf,
    pub subdir: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub parent_branch: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiffStats {
    #[serde(default)]
    pub additions: u32,
    #[serde(default)]
    pub deletions: u32,
}

/// Snapshot of one tracked repo at archive time. Captures enough to
/// recreate the worktree and branch on restore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchivedRepoSnapshot {
    pub repo_path: PathBuf,
    pub subdir: String,
    #[serde(default)]
    pub branch_name: Option<String>,
    #[serde(default)]
    pub branch_tip_sha: Option<String>,
    #[serde(default)]
    pub parent_branch: Option<String>,
    #[serde(default)]
    pub parent_branch_sha: Option<String>,
    #[serde(default)]
    pub diff_stats: DiffStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveMetadata {
    pub archived_at: String,
    pub repos: Vec<ArchivedRepoSnapshot>,
    #[serde(default)]
    pub diff_stats: DiffStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRecord {
    pub id: String,
    pub name: String,
    /// The repos this agent has worktrees in. Always non-empty;
    /// `repos[0]` is the primary (the repo the user spawned against).
    #[serde(default)]
    pub repos: Vec<TrackedRepo>,
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
    /// Some when the agent has been archived. Live agents have None.
    /// Archived agents have no worktree, no branch, and no live process —
    /// only a record (and claude's session JSONL on disk).
    #[serde(default)]
    pub archive: Option<ArchiveMetadata>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Workspace {
    /// Repos pinned in the sidebar. Empty on first launch — the user
    /// adds one or more via "+ Repo". Agents spawn against one of
    /// these; the sidebar groups agents by primary repo.
    #[serde(default)]
    pub repos: Vec<PathBuf>,
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

        // Always have a workspace — empty repos / empty agents on first
        // launch. Avoids None-handling sprawl downstream.
        if inner.current.is_none() {
            inner.current = Some(Workspace::default());
        }

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

    /// Append a repo to the sidebar's pinned list. Idempotent — adding
    /// a path that's already pinned is a no-op (returns Ok).
    pub fn add_workspace_repo(&self, repo_path: PathBuf) -> Result<Workspace> {
        if !repo_path.join(".git").exists() {
            return Err(Error::InvalidPath(format!(
                "not a git repository: {}",
                repo_path.display()
            )));
        }
        {
            let mut g = self.inner.write();
            let ws = g.current.as_mut().ok_or(Error::WorkspaceNotLoaded)?;
            if !ws.repos.iter().any(|p| p == &repo_path) {
                ws.repos.push(repo_path);
            }
        }
        self.persist()?;
        Ok(self.current().expect("workspace initialized"))
    }

    /// Remove a repo from the sidebar's pinned list. Does NOT touch
    /// agents — agents whose primary points at the removed repo keep
    /// working and continue to show in the sidebar under that repo
    /// (the sidebar takes the union of pinned + agent-primary repos).
    pub fn remove_workspace_repo(&self, repo_path: &Path) -> Result<Workspace> {
        {
            let mut g = self.inner.write();
            let ws = g.current.as_mut().ok_or(Error::WorkspaceNotLoaded)?;
            ws.repos.retain(|p| p != repo_path);
        }
        self.persist()?;
        Ok(self.current().expect("workspace initialized"))
    }

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

    pub fn set_agent_task_if_empty(&self, id: &str, task: &str) -> Result<bool> {
        self.mutate_agent(id, |a| {
            if !a.task.trim().is_empty() {
                return false;
            }
            a.task = task.to_string();
            true
        })
    }

    /// Set the branch on a specific tracked repo within an agent — but
    /// only if it isn't set yet. Identified by subdir (unique per
    /// agent). Returns true iff it actually wrote.
    pub fn set_repo_branch_if_empty(
        &self,
        agent_id: &str,
        subdir: &str,
        branch: &str,
    ) -> Result<bool> {
        self.mutate_agent(agent_id, |a| {
            let repo = match a.repos.iter_mut().find(|r| r.subdir == subdir) {
                Some(r) => r,
                None => return false,
            };
            if repo.branch.is_some() {
                return false;
            }
            repo.branch = Some(branch.to_string());
            true
        })
    }

    pub fn append_tracked_repo(&self, agent_id: &str, repo: TrackedRepo) -> Result<()> {
        self.mutate_agent(agent_id, |a| {
            a.repos.push(repo);
            true
        })?;
        Ok(())
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

    /// Mark an agent as archived. Stamps `archived_at`, stores the
    /// snapshot of every tracked repo, and clears `repos` so the
    /// frontend doesn't treat the (now-deleted) worktrees as live.
    /// Status moves to `Stopped` so resume-on-launch ignores it.
    pub fn archive_agent(&self, id: &str, archive: ArchiveMetadata) -> Result<()> {
        self.mutate_agent(id, |a| {
            a.archive = Some(archive);
            a.repos = Vec::new();
            a.status = AgentStatus::Stopped;
            true
        })?;
        Ok(())
    }

    /// Clear archive metadata and re-seed `repos`. Status moves to
    /// `Spawning` so the supervisor's resume path picks the agent up.
    pub fn restore_agent(&self, id: &str, repos: Vec<TrackedRepo>) -> Result<()> {
        self.mutate_agent(id, |a| {
            a.archive = None;
            a.repos = repos;
            a.status = AgentStatus::Spawning;
            true
        })?;
        Ok(())
    }

    pub fn remove_agent(&self, id: &str) -> Result<()> {
        {
            let mut g = self.inner.write();
            let ws = g.current.as_mut().ok_or(Error::WorkspaceNotLoaded)?;
            ws.agents.retain(|a| a.id != id);
        }
        self.persist()
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

/// Build a fresh AgentRecord with one primary tracked repo.
pub fn new_agent_record(
    id: String,
    name: String,
    primary: TrackedRepo,
    task: String,
    view: AgentView,
) -> AgentRecord {
    AgentRecord {
        id,
        name,
        repos: vec![primary],
        task,
        status: AgentStatus::Spawning,
        view,
        session_id: Some(uuid::Uuid::new_v4().to_string()),
        created_at: Utc::now().to_rfc3339(),
        last_error: None,
        archive: None,
    }
}

/// Compute a unique subdir name for a new tracked repo. Basename of
/// the repo path, with `-2`, `-3`, … suffix appended on collision with
/// an existing subdir in the same agent.
pub fn allocate_repo_subdir(repo_path: &Path, used: &[String]) -> String {
    let base = repo_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo")
        .to_string();
    if !used.iter().any(|u| u == &base) {
        return base;
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base}-{n}");
        if !used.iter().any(|u| u == &candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Absolute path to the dir holding all of one agent's worktrees:
/// `~/.quorum/worktrees/<agent-id>/`.
pub fn agent_parent_dir(agent_id: &str) -> Result<PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| Error::Other("HOME directory not available".into()))?;
    Ok(home.join(".quorum").join("worktrees").join(agent_id))
}

/// Absolute path to one tracked repo's worktree:
/// `~/.quorum/worktrees/<agent-id>/<subdir>/`.
pub fn repo_worktree_path(agent_id: &str, subdir: &str) -> Result<PathBuf> {
    Ok(agent_parent_dir(agent_id)?.join(subdir))
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

    fn mk_repo(path: &str) -> TrackedRepo {
        TrackedRepo {
            repo_path: PathBuf::from(path),
            subdir: "repo".into(),
            branch: None,
            parent_branch: None,
        }
    }

    #[test]
    fn persists_across_instances_and_reconciles_to_spawning_for_resume() {
        let td = tmpdir();
        let app_dir = td.path().to_path_buf();
        let repo = init_repo(td.path());

        {
            let wm = WorkspaceManager::new(app_dir.clone()).unwrap();
            wm.add_workspace_repo(repo.clone()).unwrap();
            let mut running = new_agent_record(
                "yosemite".into(),
                "a".into(),
                mk_repo("/r"),
                "c".into(),
                AgentView::Custom,
            );
            running.status = AgentStatus::Running;
            wm.add_agent(running).unwrap();

            let mut stopped = new_agent_record(
                "dolomites".into(),
                "s".into(),
                mk_repo("/r2"),
                "sc".into(),
                AgentView::Custom,
            );
            stopped.status = AgentStatus::Stopped;
            wm.add_agent(stopped).unwrap();
        }

        let wm2 = WorkspaceManager::new(app_dir).unwrap();
        let cur = wm2.current().unwrap();
        assert_eq!(cur.repos, vec![repo]);
        assert_eq!(cur.agents.len(), 2);
        assert_eq!(cur.agents[0].status, AgentStatus::Spawning);
        assert_eq!(cur.agents[1].status, AgentStatus::Stopped);
    }

    #[test]
    fn rejects_non_repo_path() {
        let td = tmpdir();
        let wm = WorkspaceManager::new(td.path().to_path_buf()).unwrap();
        let err = wm.add_workspace_repo(td.path().join("nope")).unwrap_err();
        assert!(err.to_string().contains("not a git repository"));
    }

    #[test]
    fn add_repo_idempotent() {
        let td = tmpdir();
        let repo = init_repo(td.path());
        let wm = WorkspaceManager::new(td.path().to_path_buf()).unwrap();
        wm.add_workspace_repo(repo.clone()).unwrap();
        wm.add_workspace_repo(repo.clone()).unwrap();
        let cur = wm.current().unwrap();
        assert_eq!(cur.repos, vec![repo]);
    }

    #[test]
    fn remove_repo_leaves_agents_alone() {
        let td = tmpdir();
        let repo = init_repo(td.path());
        let wm = WorkspaceManager::new(td.path().to_path_buf()).unwrap();
        wm.add_workspace_repo(repo.clone()).unwrap();
        wm.add_agent(new_agent_record(
            "yosemite".into(),
            "a".into(),
            mk_repo(repo.to_str().unwrap()),
            "".into(),
            AgentView::Custom,
        ))
        .unwrap();
        wm.remove_workspace_repo(&repo).unwrap();
        let cur = wm.current().unwrap();
        assert!(cur.repos.is_empty());
        assert_eq!(cur.agents.len(), 1);
    }

    #[test]
    fn agent_status_transitions() {
        let td = tmpdir();
        let repo = init_repo(td.path());
        let wm = WorkspaceManager::new(td.path().to_path_buf()).unwrap();
        wm.add_workspace_repo(repo).unwrap();
        let rec = new_agent_record(
            "test-id".into(),
            "a".into(),
            mk_repo("/r"),
            "c".into(),
            AgentView::Custom,
        );
        let id = rec.id.clone();
        wm.add_agent(rec).unwrap();

        wm.update_agent_status(&id, AgentStatus::Running, None)
            .unwrap();
        let cur = wm.current().unwrap();
        assert_eq!(cur.agents[0].status, AgentStatus::Running);
    }

    #[test]
    fn archive_then_restore_roundtrip() {
        let td = tmpdir();
        let wm = WorkspaceManager::new(td.path().to_path_buf()).unwrap();
        let rec = new_agent_record(
            "yosemite".into(),
            "yosemite".into(),
            mk_repo("/some/repo"),
            "do the thing".into(),
            AgentView::Custom,
        );
        let id = rec.id.clone();
        wm.add_agent(rec).unwrap();

        let archive = ArchiveMetadata {
            archived_at: "2026-05-26T12:00:00Z".into(),
            repos: vec![ArchivedRepoSnapshot {
                repo_path: PathBuf::from("/some/repo"),
                subdir: "repo".into(),
                branch_name: Some("quorum/do-the-thing".into()),
                branch_tip_sha: Some("deadbeef".into()),
                parent_branch: Some("main".into()),
                parent_branch_sha: Some("cafebabe".into()),
                diff_stats: DiffStats {
                    additions: 12,
                    deletions: 3,
                },
            }],
            diff_stats: DiffStats {
                additions: 12,
                deletions: 3,
            },
        };
        wm.archive_agent(&id, archive).unwrap();

        let cur = wm.current().unwrap();
        let a = &cur.agents[0];
        assert!(a.archive.is_some());
        assert!(a.repos.is_empty());
        assert_eq!(a.status, AgentStatus::Stopped);
        // session_id preserved so restore can re-attach claude
        assert!(a.session_id.is_some());

        // Restore puts repos back and flips to Spawning
        let restored = vec![TrackedRepo {
            repo_path: PathBuf::from("/some/repo"),
            subdir: "repo".into(),
            branch: Some("quorum/do-the-thing".into()),
            parent_branch: Some("main".into()),
        }];
        wm.restore_agent(&id, restored).unwrap();
        let cur = wm.current().unwrap();
        let a = &cur.agents[0];
        assert!(a.archive.is_none());
        assert_eq!(a.repos.len(), 1);
        assert_eq!(a.status, AgentStatus::Spawning);
    }

    #[test]
    fn archived_agents_survive_reload_without_reconcile() {
        let td = tmpdir();
        let app_dir = td.path().to_path_buf();
        {
            let wm = WorkspaceManager::new(app_dir.clone()).unwrap();
            let rec = new_agent_record(
                "yosemite".into(),
                "yosemite".into(),
                mk_repo("/r"),
                "".into(),
                AgentView::Custom,
            );
            let id = rec.id.clone();
            wm.add_agent(rec).unwrap();
            wm.archive_agent(
                &id,
                ArchiveMetadata {
                    archived_at: "2026-05-26T12:00:00Z".into(),
                    repos: vec![],
                    diff_stats: DiffStats::default(),
                },
            )
            .unwrap();
        }
        let wm2 = WorkspaceManager::new(app_dir).unwrap();
        let cur = wm2.current().unwrap();
        assert_eq!(cur.agents.len(), 1);
        // Archived agent stays archived; reconcile shouldn't flip Stopped → Spawning
        assert!(cur.agents[0].archive.is_some());
        assert_eq!(cur.agents[0].status, AgentStatus::Stopped);
    }

    #[test]
    fn allocate_subdir_handles_collision() {
        let used = vec!["luxembourg".to_string()];
        assert_eq!(
            allocate_repo_subdir(Path::new("/foo/luxembourg"), &used),
            "luxembourg-2"
        );
        let used2 = vec!["luxembourg".to_string(), "luxembourg-2".to_string()];
        assert_eq!(
            allocate_repo_subdir(Path::new("/bar/luxembourg"), &used2),
            "luxembourg-3"
        );
        assert_eq!(
            allocate_repo_subdir(Path::new("/foo/fresh"), &used),
            "fresh"
        );
    }
}
