//! Workspace state — the on-disk record of the sidebar's repo list and
//! the agents the user has spawned. Each agent is anchored to a primary
//! repo (`repos[0]`); the sidebar groups agents by that primary.

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

/// Default for AgentRecord.provider — assumes pre-existing records on
/// disk all came from the only backend that was wired before the
/// multi-agent refactor.
fn default_provider() -> String {
    "claude".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRecord {
    pub id: String,
    pub name: String,
    /// The project this agent belongs to. Populated by `add_agent`
    /// (looked up from the primary repo path) and re-read from the
    /// `agents.project_id` column.
    #[serde(default)]
    pub project_id: String,
    /// Which CLI backend powers this agent. Currently only "claude" has a
    /// Rust transport; other providers will land here as their backends
    /// ship. Missing in older workspace JSON → defaults to "claude".
    #[serde(default = "default_provider")]
    pub provider: String,
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

// ── Helpers ────────────────────────────────────────────────────────────────

fn status_to_str(s: &AgentStatus) -> &'static str {
    match s {
        AgentStatus::Spawning => "spawning",
        AgentStatus::Running => "running",
        AgentStatus::Idle => "idle",
        AgentStatus::Stopped => "stopped",
        AgentStatus::Error => "error",
    }
}

fn str_to_status(s: &str) -> AgentStatus {
    match s {
        "running" => AgentStatus::Running,
        "idle" => AgentStatus::Idle,
        "stopped" => AgentStatus::Stopped,
        "error" => AgentStatus::Error,
        _ => AgentStatus::Spawning,
    }
}

fn view_to_str(v: &AgentView) -> &'static str {
    match v {
        AgentView::Custom => "custom",
        AgentView::Native => "native",
    }
}

fn str_to_view(s: &str) -> AgentView {
    match s {
        "native" => AgentView::Native,
        _ => AgentView::Custom,
    }
}

fn millis_to_iso(millis: i64) -> String {
    DateTime::from_timestamp_millis(millis)
        .unwrap_or_default()
        .to_rfc3339()
}

fn now_millis() -> i64 {
    Utc::now().timestamp_millis()
}

// ── WorkspaceManager ──────────────────────────────────────────────────────

pub struct WorkspaceManager {
    db: Arc<Mutex<Connection>>,
}

impl WorkspaceManager {
    pub fn new(db: Arc<Mutex<Connection>>) -> Self {
        let mgr = Self { db };

        // Reconcile stale statuses left over from a crash / clean shutdown.
        // The in-memory Supervisor is fresh on every start, so any agent
        // marked Running / Idle / Spawning on disk has no live process.
        // We flip those back to Spawning so the supervisor's auto-resume
        // pass picks them up — agents the user explicitly Stopped (status
        // Stopped) stay stopped, available via the manual Resume button.
        // Only reconcile non-archived agents (archived_at IS NULL).
        let conn = mgr.db.lock();
        let _ = conn.execute(
            "UPDATE agents SET status = 'spawning'
             WHERE status IN ('running', 'idle', 'spawning')
               AND archived_at IS NULL",
            [],
        );
        drop(conn);

        mgr
    }

    /// Direct access to the connection — used by supervisor pieces
    /// (e.g. Run panel's project-settings lookup) that don't have a
    /// dedicated typed method on this manager yet.
    pub fn db_handle(&self) -> Arc<Mutex<Connection>> {
        self.db.clone()
    }

    pub fn current(&self) -> Option<Workspace> {
        let conn = self.db.lock();

        // Collect all unique repo paths.
        let repos = Self::query_all_repo_paths(&conn);

        // Collect all agents.
        let agents = Self::query_all_agents(&conn);

        Some(Workspace { repos, agents })
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

        let conn = self.db.lock();
        let path_str = repo_path.to_string_lossy().to_string();

        // Check if repo already exists.
        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM repos WHERE path = ?1",
                [&path_str],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);

        if !exists {
            // Look up or create a project named after the repo dir basename.
            let project_name = repo_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            let project_id = Self::find_or_create_project(&conn, &project_name)?;

            let repo_id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO repos (id, project_id, path, created_at) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![repo_id, project_id, path_str, now_millis()],
            )?;
        }

        drop(conn);
        Ok(self.current().expect("workspace initialized"))
    }

    /// Remove a repo from the sidebar's pinned list. Does NOT touch
    /// agents — agents whose primary points at the removed repo keep
    /// working and continue to show in the sidebar under that repo
    /// (the sidebar takes the union of pinned + agent-primary repos).
    pub fn remove_workspace_repo(&self, repo_path: &Path) -> Result<Workspace> {
        let conn = self.db.lock();
        let path_str = repo_path.to_string_lossy().to_string();
        conn.execute("DELETE FROM repos WHERE path = ?1", [&path_str])?;
        drop(conn);
        Ok(self.current().expect("workspace initialized"))
    }

    pub fn allocate_agent_id(&self) -> Result<String> {
        let conn = self.db.lock();
        let mut stmt = conn.prepare("SELECT id FROM agents")?;
        let used: HashSet<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(names::allocate(&used))
    }

    pub fn add_agent(&self, record: &mut AgentRecord) -> Result<()> {
        let conn = self.db.lock();

        // Look up project_id from the primary repo path.
        let project_id = if let Some(primary) = record.repos.first() {
            let path_str = primary.repo_path.to_string_lossy().to_string();
            Self::project_id_for_repo_path(&conn, &path_str)?
        } else {
            return Err(Error::Other("agent must have at least one repo".into()));
        };
        record.project_id = project_id.clone();

        // Parse created_at ISO string to millis.
        let created_millis = chrono::DateTime::parse_from_rfc3339(&record.created_at)
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|_| now_millis());

        conn.execute(
            "INSERT INTO agents (id, project_id, name, provider, task, status, view, session_id, created_at, last_error, archived_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                record.id,
                project_id,
                record.name,
                record.provider,
                record.task,
                status_to_str(&record.status),
                view_to_str(&record.view),
                record.session_id,
                created_millis,
                record.last_error,
                rusqlite::types::Null,
            ],
        )?;

        // Insert worktree records for each TrackedRepo.
        for repo in &record.repos {
            Self::insert_worktree(&conn, &record.id, repo)?;
        }

        Ok(())
    }

    pub fn update_agent_status(
        &self,
        id: &str,
        status: AgentStatus,
        last_error: Option<String>,
    ) -> Result<()> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;

        if let Some(ref err) = last_error {
            conn.execute(
                "UPDATE agents SET status = ?1, last_error = ?2 WHERE id = ?3",
                rusqlite::params![status_to_str(&status), err, id],
            )?;
        } else {
            conn.execute(
                "UPDATE agents SET status = ?1 WHERE id = ?2",
                rusqlite::params![status_to_str(&status), id],
            )?;
        }
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
        let conn = self.db.lock();

        // Read current status.
        let current_str: String = conn
            .query_row("SELECT status FROM agents WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .map_err(|_| Error::AgentNotFound(id.to_string()))?;

        let current = str_to_status(&current_str);
        if !predicate(&current) {
            return Ok(false);
        }

        if let Some(ref err) = last_error {
            conn.execute(
                "UPDATE agents SET status = ?1, last_error = ?2 WHERE id = ?3",
                rusqlite::params![status_to_str(&status), err, id],
            )?;
        } else {
            conn.execute(
                "UPDATE agents SET status = ?1 WHERE id = ?2",
                rusqlite::params![status_to_str(&status), id],
            )?;
        }
        Ok(true)
    }

    pub fn set_agent_task_if_empty(&self, id: &str, task: &str) -> Result<bool> {
        let conn = self.db.lock();
        let changed = conn.execute(
            "UPDATE agents SET task = ?1 WHERE id = ?2 AND (task = '' OR task IS NULL)",
            rusqlite::params![task, id],
        )?;
        Ok(changed > 0)
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
        let conn = self.db.lock();
        let changed = conn.execute(
            "UPDATE worktrees SET branch = ?1 WHERE agent_id = ?2 AND subdir = ?3 AND branch IS NULL",
            rusqlite::params![branch, agent_id, subdir],
        )?;
        Ok(changed > 0)
    }

    pub fn append_tracked_repo(&self, agent_id: &str, repo: TrackedRepo) -> Result<()> {
        let conn = self.db.lock();
        Self::insert_worktree(&conn, agent_id, &repo)?;
        Ok(())
    }

    /// Persist the agent's session id. Used for Codex, whose thread id
    /// is assigned by the CLI and captured from its first turn's events
    /// (Claude's id is generated up front, so it never changes here).
    pub fn set_agent_session_id(&self, id: &str, session_id: &str) -> Result<()> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;
        conn.execute(
            "UPDATE agents SET session_id = ?1 WHERE id = ?2",
            rusqlite::params![session_id, id],
        )?;
        Ok(())
    }

    pub fn update_agent_view(&self, id: &str, view: AgentView) -> Result<()> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;
        conn.execute(
            "UPDATE agents SET view = ?1 WHERE id = ?2",
            rusqlite::params![view_to_str(&view), id],
        )?;
        Ok(())
    }

    pub fn agent(&self, id: &str) -> Result<AgentRecord> {
        let conn = self.db.lock();
        Self::load_agent(&conn, id)
    }

    /// Mark an agent as archived. Stamps `archived_at`, stores the
    /// snapshot of every tracked repo, and clears `repos` so the
    /// frontend doesn't treat the (now-deleted) worktrees as live.
    /// Status moves to `Stopped` so resume-on-launch ignores it.
    pub fn archive_agent(&self, id: &str, archive: ArchiveMetadata) -> Result<()> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;

        // Parse archived_at string to millis for storage.
        let archived_millis = chrono::DateTime::parse_from_rfc3339(&archive.archived_at)
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|_| now_millis());

        // Clear setup_completed_at too — restore recreates the worktree
        // from scratch, so node_modules etc. won't be there.
        conn.execute(
            "UPDATE agents SET archived_at = ?1, status = 'stopped',
                    setup_completed_at = NULL WHERE id = ?2",
            rusqlite::params![archived_millis, id],
        )?;

        // Update worktree rows with snapshot data from ArchiveMetadata.repos.
        for snap in &archive.repos {
            conn.execute(
                "UPDATE worktrees SET branch_tip_sha = ?1, parent_branch_sha = ?2,
                        diff_additions = ?3, diff_deletions = ?4
                 WHERE agent_id = ?5 AND subdir = ?6",
                rusqlite::params![
                    snap.branch_tip_sha,
                    snap.parent_branch_sha,
                    snap.diff_stats.additions,
                    snap.diff_stats.deletions,
                    id,
                    snap.subdir,
                ],
            )?;
        }

        Ok(())
    }

    /// Clear archive metadata and re-seed `repos`. Status moves to
    /// `Spawning` so the supervisor's resume path picks the agent up.
    pub fn restore_agent(&self, id: &str, repos: Vec<TrackedRepo>) -> Result<()> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;

        conn.execute(
            "UPDATE agents SET archived_at = NULL, status = 'spawning' WHERE id = ?1",
            [id],
        )?;

        // Update worktree records with new branch info and clear snapshot fields.
        for repo in &repos {
            conn.execute(
                "UPDATE worktrees SET branch = ?1, parent_branch = ?2,
                        branch_tip_sha = NULL, parent_branch_sha = NULL,
                        diff_additions = 0, diff_deletions = 0
                 WHERE agent_id = ?3 AND subdir = ?4",
                rusqlite::params![repo.branch, repo.parent_branch, id, repo.subdir],
            )?;
        }

        Ok(())
    }

    /// Has the Run panel's setup command ever succeeded for this agent?
    /// Cleared on archive so a restored agent re-runs setup against the
    /// freshly-recreated worktree.
    pub fn is_setup_completed(&self, id: &str) -> Result<bool> {
        let conn = self.db.lock();
        let value: Option<i64> = conn
            .query_row(
                "SELECT setup_completed_at FROM agents WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .map_err(|_| Error::AgentNotFound(id.to_string()))?;
        Ok(value.is_some())
    }

    /// Stamp the setup command as having succeeded. Idempotent.
    pub fn mark_setup_completed(&self, id: &str) -> Result<()> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;
        conn.execute(
            "UPDATE agents SET setup_completed_at = ?1 WHERE id = ?2",
            rusqlite::params![now_millis(), id],
        )?;
        Ok(())
    }

    pub fn remove_agent(&self, id: &str) -> Result<()> {
        let conn = self.db.lock();
        conn.execute("DELETE FROM agents WHERE id = ?1", [id])?;
        Ok(())
    }

    // ── Internal helpers ──────────────────────────────────────────────────

    fn query_all_repo_paths(conn: &Connection) -> Vec<PathBuf> {
        let mut stmt = conn
            .prepare("SELECT path FROM repos ORDER BY created_at")
            .unwrap_or_else(|_| conn.prepare("SELECT 1 WHERE 0").unwrap());
        stmt.query_map([], |row| {
            let p: String = row.get(0)?;
            Ok(PathBuf::from(p))
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    fn query_all_agents(conn: &Connection) -> Vec<AgentRecord> {
        let mut stmt = match conn.prepare(
            "SELECT id, project_id, name, provider, task, status, view, session_id,
                    created_at, last_error, archived_at
             FROM agents ORDER BY created_at",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let agents: Vec<AgentRecord> = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let project_id: String = row.get(1)?;
                let name: String = row.get(2)?;
                let provider: String = row.get(3)?;
                let task: String = row.get(4)?;
                let status_str: String = row.get(5)?;
                let view_str: String = row.get(6)?;
                let session_id: Option<String> = row.get(7)?;
                let created_millis: i64 = row.get(8)?;
                let last_error: Option<String> = row.get(9)?;
                let archived_millis: Option<i64> = row.get(10)?;

                Ok((
                    id,
                    project_id,
                    name,
                    provider,
                    task,
                    status_str,
                    view_str,
                    session_id,
                    created_millis,
                    last_error,
                    archived_millis,
                ))
            })
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .map(
                |(
                    id,
                    project_id,
                    name,
                    provider,
                    task,
                    status_str,
                    view_str,
                    session_id,
                    created_millis,
                    last_error,
                    archived_millis,
                )| {
                    let is_archived = archived_millis.is_some();

                    let (repos, archive) = if is_archived {
                        // Build ArchiveMetadata from worktree snapshot fields.
                        let archive_meta =
                            Self::build_archive_metadata(conn, &id, archived_millis.unwrap());
                        (Vec::new(), Some(archive_meta))
                    } else {
                        // Build TrackedRepo vec from worktrees+repos join.
                        let tracked = Self::query_tracked_repos(conn, &id);
                        (tracked, None)
                    };

                    AgentRecord {
                        id,
                        project_id,
                        name,
                        provider,
                        repos,
                        task,
                        status: str_to_status(&status_str),
                        view: str_to_view(&view_str),
                        session_id,
                        created_at: millis_to_iso(created_millis),
                        last_error,
                        archive,
                    }
                },
            )
            .collect();

        agents
    }

    fn query_tracked_repos(conn: &Connection, agent_id: &str) -> Vec<TrackedRepo> {
        let mut stmt = match conn.prepare(
            "SELECT r.path, w.subdir, w.branch, w.parent_branch
             FROM worktrees w
             JOIN repos r ON r.id = w.repo_id
             WHERE w.agent_id = ?1
             ORDER BY w.created_at",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map([agent_id], |row| {
            let path: String = row.get(0)?;
            let subdir: String = row.get(1)?;
            let branch: Option<String> = row.get(2)?;
            let parent_branch: Option<String> = row.get(3)?;
            Ok(TrackedRepo {
                repo_path: PathBuf::from(path),
                subdir,
                branch,
                parent_branch,
            })
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    fn build_archive_metadata(
        conn: &Connection,
        agent_id: &str,
        archived_millis: i64,
    ) -> ArchiveMetadata {
        let mut stmt = match conn.prepare(
            "SELECT r.path, w.subdir, w.branch, w.branch_tip_sha,
                    w.parent_branch, w.parent_branch_sha,
                    w.diff_additions, w.diff_deletions
             FROM worktrees w
             JOIN repos r ON r.id = w.repo_id
             WHERE w.agent_id = ?1
             ORDER BY w.created_at",
        ) {
            Ok(s) => s,
            Err(_) => {
                return ArchiveMetadata {
                    archived_at: millis_to_iso(archived_millis),
                    repos: Vec::new(),
                    diff_stats: DiffStats::default(),
                }
            }
        };

        let snapshots: Vec<ArchivedRepoSnapshot> = stmt
            .query_map([agent_id], |row| {
                let path: String = row.get(0)?;
                let subdir: String = row.get(1)?;
                let branch: Option<String> = row.get(2)?;
                let branch_tip_sha: Option<String> = row.get(3)?;
                let parent_branch: Option<String> = row.get(4)?;
                let parent_branch_sha: Option<String> = row.get(5)?;
                let additions: u32 = row.get(6)?;
                let deletions: u32 = row.get(7)?;
                Ok(ArchivedRepoSnapshot {
                    repo_path: PathBuf::from(path),
                    subdir,
                    branch_name: branch,
                    branch_tip_sha,
                    parent_branch,
                    parent_branch_sha,
                    diff_stats: DiffStats {
                        additions,
                        deletions,
                    },
                })
            })
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        // Aggregate diff stats across all repos.
        let total_additions: u32 = snapshots.iter().map(|s| s.diff_stats.additions).sum();
        let total_deletions: u32 = snapshots.iter().map(|s| s.diff_stats.deletions).sum();

        ArchiveMetadata {
            archived_at: millis_to_iso(archived_millis),
            repos: snapshots,
            diff_stats: DiffStats {
                additions: total_additions,
                deletions: total_deletions,
            },
        }
    }

    fn find_or_create_project(conn: &Connection, name: &str) -> Result<String> {
        // Try to find an existing project by name.
        let existing: Option<String> = conn
            .query_row(
                "SELECT id FROM projects WHERE name = ?1",
                [name],
                |row| row.get(0),
            )
            .ok();

        if let Some(id) = existing {
            return Ok(id);
        }

        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, name, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, name, now_millis()],
        )?;
        Ok(id)
    }

    /// Look up the project_id for a repo path. If the repo doesn't exist
    /// yet, create both the project and repo record.
    fn project_id_for_repo_path(conn: &Connection, path_str: &str) -> Result<String> {
        // Try to find existing repo.
        let existing: Option<String> = conn
            .query_row(
                "SELECT project_id FROM repos WHERE path = ?1",
                [path_str],
                |row| row.get(0),
            )
            .ok();

        if let Some(pid) = existing {
            return Ok(pid);
        }

        // Create project + repo.
        let repo_path = Path::new(path_str);
        let project_name = repo_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        let project_id = Self::find_or_create_project(conn, &project_name)?;

        let repo_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO repos (id, project_id, path, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![repo_id, project_id, path_str, now_millis()],
        )?;

        Ok(project_id)
    }

    fn insert_worktree(conn: &Connection, agent_id: &str, repo: &TrackedRepo) -> Result<()> {
        let path_str = repo.repo_path.to_string_lossy().to_string();

        // Look up repo_id from repos table.
        let repo_id: String = conn
            .query_row(
                "SELECT id FROM repos WHERE path = ?1",
                [&path_str],
                |row| row.get(0),
            )
            .map_err(|_| {
                Error::Other(format!("repo not found in database: {path_str}"))
            })?;

        let wt_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO worktrees (id, agent_id, repo_id, subdir, branch, parent_branch, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                wt_id,
                agent_id,
                repo_id,
                repo.subdir,
                repo.branch,
                repo.parent_branch,
                now_millis(),
            ],
        )?;
        Ok(())
    }

    fn ensure_agent_exists(conn: &Connection, id: &str) -> Result<()> {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM agents WHERE id = ?1",
            [id],
            |row| row.get(0),
        )?;
        if count == 0 {
            return Err(Error::AgentNotFound(id.to_string()));
        }
        Ok(())
    }

    fn load_agent(conn: &Connection, id: &str) -> Result<AgentRecord> {
        let row = conn
            .query_row(
                "SELECT id, project_id, name, provider, task, status, view, session_id,
                        created_at, last_error, archived_at
                 FROM agents WHERE id = ?1",
                [id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, i64>(8)?,
                        row.get::<_, Option<String>>(9)?,
                        row.get::<_, Option<i64>>(10)?,
                    ))
                },
            )
            .map_err(|_| Error::AgentNotFound(id.to_string()))?;

        let (
            agent_id,
            project_id,
            name,
            provider,
            task,
            status_str,
            view_str,
            session_id,
            created_millis,
            last_error,
            archived_millis,
        ) = row;

        let is_archived = archived_millis.is_some();

        let (repos, archive) = if is_archived {
            let archive_meta =
                Self::build_archive_metadata(conn, &agent_id, archived_millis.unwrap());
            (Vec::new(), Some(archive_meta))
        } else {
            let tracked = Self::query_tracked_repos(conn, &agent_id);
            (tracked, None)
        };

        Ok(AgentRecord {
            id: agent_id,
            project_id,
            name,
            provider,
            repos,
            task,
            status: str_to_status(&status_str),
            view: str_to_view(&view_str),
            session_id,
            created_at: millis_to_iso(created_millis),
            last_error,
            archive,
        })
    }
}

/// Per-turn agents run one process per turn, assign their own session id
/// (captured from their first turn's events rather than generated up
/// front), and render only in the structured (Custom) view for now.
pub fn is_per_turn_provider(provider: &str) -> bool {
    matches!(provider, "codex" | "cursor" | "opencode" | "pi")
}

/// Build a fresh AgentRecord with one primary tracked repo.
pub fn new_agent_record(
    id: String,
    name: String,
    provider: String,
    primary: TrackedRepo,
    task: String,
    view: AgentView,
) -> AgentRecord {
    // Claude attaches to a session id we generate up front (passed as
    // `--session-id`). Per-turn agents (codex, cursor) assign their own id
    // on the first turn (captured from their events), so they start with
    // none.
    let session_id = if is_per_turn_provider(&provider) {
        None
    } else {
        Some(uuid::Uuid::new_v4().to_string())
    };
    AgentRecord {
        id,
        // Populated by WorkspaceManager::add_agent (looked up from the
        // primary repo path) — empty here because callers don't know it.
        project_id: String::new(),
        name,
        provider,
        repos: vec![primary],
        task,
        status: AgentStatus::Spawning,
        view,
        session_id,
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

    fn test_db() -> Arc<Mutex<Connection>> {
        let dir = tempfile::tempdir().unwrap();
        crate::database::init(dir.path()).unwrap()
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

    /// Helper: ensure the repo path exists in the repos table so add_agent can find it.
    fn seed_repo(db: &Arc<Mutex<Connection>>, repo_path: &str) {
        let conn = db.lock();
        let path = Path::new(repo_path);
        let project_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        let project_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT OR IGNORE INTO projects (id, name, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![project_id, project_name, now_millis()],
        )
        .unwrap();
        // Re-read the project_id in case it already existed.
        let pid: String = conn
            .query_row(
                "SELECT id FROM projects WHERE name = ?1",
                [project_name],
                |row| row.get(0),
            )
            .unwrap();
        let repo_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT OR IGNORE INTO repos (id, project_id, path, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![repo_id, pid, repo_path, now_millis()],
        )
        .unwrap();
    }

    #[test]
    fn persists_across_instances_and_reconciles_to_spawning_for_resume() {
        let db = test_db();

        // Seed repo paths so add_agent can look them up.
        let td = tempfile::tempdir().unwrap();
        let repo = init_repo(td.path());
        seed_repo(&db, "/r");
        seed_repo(&db, "/r2");

        {
            let wm = WorkspaceManager::new(db.clone());
            wm.add_workspace_repo(repo.clone()).unwrap();
            let mut running = new_agent_record(
                "yosemite".into(),
                "a".into(),
                "claude".into(),
                mk_repo("/r"),
                "c".into(),
                AgentView::Custom,
            );
            running.status = AgentStatus::Running;
            wm.add_agent(&mut running).unwrap();

            let mut stopped = new_agent_record(
                "dolomites".into(),
                "s".into(),
                "claude".into(),
                mk_repo("/r2"),
                "sc".into(),
                AgentView::Custom,
            );
            stopped.status = AgentStatus::Stopped;
            wm.add_agent(&mut stopped).unwrap();
        }

        // Second instance — reconciliation should flip Running → Spawning.
        let wm2 = WorkspaceManager::new(db);
        let cur = wm2.current().unwrap();
        assert!(cur.repos.iter().any(|p| p == &repo));
        assert_eq!(cur.agents.len(), 2);

        let yosemite = cur.agents.iter().find(|a| a.id == "yosemite").unwrap();
        let dolomites = cur.agents.iter().find(|a| a.id == "dolomites").unwrap();
        assert_eq!(yosemite.status, AgentStatus::Spawning);
        assert_eq!(dolomites.status, AgentStatus::Stopped);
    }

    #[test]
    fn rejects_non_repo_path() {
        let db = test_db();
        let td = tempfile::tempdir().unwrap();
        let wm = WorkspaceManager::new(db);
        let err = wm.add_workspace_repo(td.path().join("nope")).unwrap_err();
        assert!(err.to_string().contains("not a git repository"));
    }

    #[test]
    fn add_repo_idempotent() {
        let db = test_db();
        let td = tempfile::tempdir().unwrap();
        let repo = init_repo(td.path());
        let wm = WorkspaceManager::new(db);
        wm.add_workspace_repo(repo.clone()).unwrap();
        wm.add_workspace_repo(repo.clone()).unwrap();
        let cur = wm.current().unwrap();
        assert_eq!(cur.repos.iter().filter(|p| **p == repo).count(), 1);
    }

    #[test]
    fn remove_repo_leaves_agents_alone() {
        let db = test_db();
        let td = tempfile::tempdir().unwrap();
        let repo = init_repo(td.path());
        let wm = WorkspaceManager::new(db.clone());
        wm.add_workspace_repo(repo.clone()).unwrap();

        let repo_str = repo.to_str().unwrap();
        let mut rec = new_agent_record(
            "yosemite".into(),
            "a".into(),
            "claude".into(),
            mk_repo(repo_str),
            "".into(),
            AgentView::Custom,
        );
        wm.add_agent(&mut rec).unwrap();
        wm.remove_workspace_repo(&repo).unwrap();
        let cur = wm.current().unwrap();
        // The repo record is deleted, but the agent remains (its worktree
        // may reference a now-deleted repo — that's fine, the sidebar
        // union logic handles it).
        assert_eq!(cur.agents.len(), 1);
    }

    #[test]
    fn agent_status_transitions() {
        let db = test_db();
        let td = tempfile::tempdir().unwrap();
        let repo = init_repo(td.path());
        let wm = WorkspaceManager::new(db.clone());
        wm.add_workspace_repo(repo).unwrap();

        seed_repo(&db, "/r");
        let mut rec = new_agent_record(
            "test-id".into(),
            "a".into(),
            "claude".into(),
            mk_repo("/r"),
            "c".into(),
            AgentView::Custom,
        );
        let id = rec.id.clone();
        wm.add_agent(&mut rec).unwrap();

        wm.update_agent_status(&id, AgentStatus::Running, None)
            .unwrap();
        let a = wm.agent(&id).unwrap();
        assert_eq!(a.status, AgentStatus::Running);
    }

    #[test]
    fn archive_then_restore_roundtrip() {
        let db = test_db();
        seed_repo(&db, "/some/repo");
        let wm = WorkspaceManager::new(db);

        let mut rec = new_agent_record(
            "yosemite".into(),
            "yosemite".into(),
            "claude".into(),
            mk_repo("/some/repo"),
            "do the thing".into(),
            AgentView::Custom,
        );
        let id = rec.id.clone();
        wm.add_agent(&mut rec).unwrap();

        let archive = ArchiveMetadata {
            archived_at: "2026-05-26T12:00:00+00:00".into(),
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

        let a = wm.agent(&id).unwrap();
        assert!(a.archive.is_some());
        assert!(a.repos.is_empty());
        assert_eq!(a.status, AgentStatus::Stopped);
        // session_id preserved so restore can re-attach claude
        assert!(a.session_id.is_some());

        let arch = a.archive.unwrap();
        assert_eq!(arch.repos.len(), 1);
        assert_eq!(arch.repos[0].branch_tip_sha.as_deref(), Some("deadbeef"));
        assert_eq!(arch.repos[0].diff_stats.additions, 12);
        assert_eq!(arch.repos[0].diff_stats.deletions, 3);

        // Restore puts repos back and flips to Spawning
        let restored = vec![TrackedRepo {
            repo_path: PathBuf::from("/some/repo"),
            subdir: "repo".into(),
            branch: Some("quorum/do-the-thing".into()),
            parent_branch: Some("main".into()),
        }];
        wm.restore_agent(&id, restored).unwrap();
        let a = wm.agent(&id).unwrap();
        assert!(a.archive.is_none());
        assert_eq!(a.repos.len(), 1);
        assert_eq!(a.status, AgentStatus::Spawning);
    }

    #[test]
    fn archived_agents_survive_reload_without_reconcile() {
        let db = test_db();
        seed_repo(&db, "/r");

        {
            let wm = WorkspaceManager::new(db.clone());
            let mut rec = new_agent_record(
                "yosemite".into(),
                "yosemite".into(),
                "claude".into(),
                mk_repo("/r"),
                "".into(),
                AgentView::Custom,
            );
            let id = rec.id.clone();
            wm.add_agent(&mut rec).unwrap();
            wm.archive_agent(
                &id,
                ArchiveMetadata {
                    archived_at: "2026-05-26T12:00:00+00:00".into(),
                    repos: vec![],
                    diff_stats: DiffStats::default(),
                },
            )
            .unwrap();
        }

        // Second instance — archived agent should stay archived.
        let wm2 = WorkspaceManager::new(db);
        let cur = wm2.current().unwrap();
        assert_eq!(cur.agents.len(), 1);
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
