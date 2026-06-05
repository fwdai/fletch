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

/// Runtime status is not stored. Derive it from durable dispositions plus
/// whether a live process currently exists (supervisor-supplied). At the
/// storage layer `running` is always false (nothing is live mid-query), so a
/// resting, non-stopped, non-errored agent derives to `Idle`: it has no live
/// process and is not running a turn. A process is spawned lazily on the
/// user's next interaction (the frontend resumes on send), so agents no
/// longer auto-spawn at load.
pub fn derive_status(
    archived: bool,
    stopped: bool,
    running: bool,
    last_error: Option<&str>,
) -> AgentStatus {
    if running {
        return AgentStatus::Running;
    }
    if archived || stopped {
        return AgentStatus::Stopped;
    }
    if last_error.is_some() {
        return AgentStatus::Error;
    }
    AgentStatus::Idle
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

/// The workspace's current session id (the most recent session row). `None`
/// when the workspace has no session yet.
fn current_session_id(conn: &Connection, workspace_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT id FROM sessions WHERE workspace_id = ?1 ORDER BY created_at DESC LIMIT 1",
        [workspace_id],
        |r| r.get(0),
    )
    .ok()
}

// ── WorkspaceManager ──────────────────────────────────────────────────────

/// One canonical durable record from `session_records`, in the agent's own
/// verbatim shape. Normalized into ChatItems on read by the per-provider adapter.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionRecord {
    pub seq: i64,
    pub provider: String,
    pub source: String,
    pub native_id: String,
    pub agent_version: Option<String>,
    pub body: serde_json::Value,
}

/// One Quorum-origin outgoing user message (the `session_user_turns` table).
/// Carries the attachment metadata the transcript doesn't, plus a `native_id`
/// link to the canonical `session_records` row once matched at turn-end.
#[derive(Debug, Clone, serde::Serialize)]
pub struct UserTurn {
    pub turn_id: String,
    pub seq: i64,
    pub text: String,
    pub attachments: Vec<String>,
    /// Matched `session_records.native_id`; `None` = pending or failed turn.
    pub native_id: Option<String>,
}

pub struct WorkspaceManager {
    db: Arc<Mutex<Connection>>,
}

impl WorkspaceManager {
    pub fn new(db: Arc<Mutex<Connection>>) -> Self {
        // Status is derived, not stored, so there is nothing to reconcile at
        // load time: a resting, non-stopped, non-errored workspace derives to
        // `Idle` (no live process; resumed lazily on the next interaction),
        // while user-stopped workspaces keep their `stopped_at` and derive to
        // `Stopped` until the manual Resume button clears it.
        Self { db }
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
        let mut stmt = conn.prepare("SELECT id FROM workspaces")?;
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

        // The workspace is the durable work-area (identity + task metadata).
        conn.execute(
            "INSERT INTO workspaces (id, project_id, name, task, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                record.id,
                project_id,
                record.name,
                record.task,
                created_millis,
            ],
        )?;

        // Exactly one provider run per workspace today. The runtime status is
        // not persisted — it derives from the workspace/session dispositions.
        let session_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO sessions (id, workspace_id, provider, view, provider_session_id, last_error, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                session_id,
                record.id,
                record.provider,
                view_to_str(&record.view),
                record.session_id,
                record.last_error,
                created_millis,
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
        Self::apply_status(&conn, id, &status, last_error.as_deref())?;
        Ok(())
    }

    pub fn set_agent_task_if_empty(&self, id: &str, task: &str) -> Result<bool> {
        let conn = self.db.lock();
        let changed = conn.execute(
            "UPDATE workspaces SET task = ?1 WHERE id = ?2 AND (task = '' OR task IS NULL)",
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
            "UPDATE worktrees SET branch = ?1 WHERE workspace_id = ?2 AND subdir = ?3 AND branch IS NULL",
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
            "UPDATE sessions SET provider_session_id = ?1 WHERE workspace_id = ?2",
            rusqlite::params![session_id, id],
        )?;
        Ok(())
    }

    pub fn update_agent_view(&self, id: &str, view: AgentView) -> Result<()> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;
        conn.execute(
            "UPDATE sessions SET view = ?1 WHERE workspace_id = ?2",
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
        // from scratch, so node_modules etc. won't be there. Stamping
        // archived_at is enough to derive `Stopped`; there is no status
        // column to flip.
        conn.execute(
            "UPDATE workspaces SET archived_at = ?1,
                    setup_completed_at = NULL WHERE id = ?2",
            rusqlite::params![archived_millis, id],
        )?;

        // Update worktree rows with snapshot data from ArchiveMetadata.repos.
        for snap in &archive.repos {
            conn.execute(
                "UPDATE worktrees SET branch_tip_sha = ?1, parent_branch_sha = ?2,
                        diff_additions = ?3, diff_deletions = ?4
                 WHERE workspace_id = ?5 AND subdir = ?6",
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

    /// Clear archive metadata and re-seed `repos`. Clearing `archived_at`
    /// (with no `stopped_at`/error) makes the workspace derive back to
    /// `Idle`; the supervisor's restore path drives the live spawn explicitly.
    pub fn restore_agent(&self, id: &str, repos: Vec<TrackedRepo>) -> Result<()> {
        let conn = self.db.lock();
        Self::ensure_agent_exists(&conn, id)?;

        // Clearing both dispositions (archived + user-stopped) returns the
        // record to its resting `Idle` state — a restored agent should be
        // live-able again, not stuck Stopped.
        conn.execute(
            "UPDATE workspaces SET archived_at = NULL, stopped_at = NULL WHERE id = ?1",
            [id],
        )?;

        // Update worktree records with new branch info and clear snapshot fields.
        for repo in &repos {
            conn.execute(
                "UPDATE worktrees SET branch = ?1, parent_branch = ?2,
                        branch_tip_sha = NULL, parent_branch_sha = NULL,
                        diff_additions = 0, diff_deletions = 0
                 WHERE workspace_id = ?3 AND subdir = ?4",
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
                "SELECT setup_completed_at FROM workspaces WHERE id = ?1",
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
            "UPDATE workspaces SET setup_completed_at = ?1 WHERE id = ?2",
            rusqlite::params![now_millis(), id],
        )?;
        Ok(())
    }

    pub fn remove_agent(&self, id: &str) -> Result<()> {
        let conn = self.db.lock();
        // Cascades to the workspace's sessions, worktrees, and session_records.
        conn.execute("DELETE FROM workspaces WHERE id = ?1", [id])?;
        Ok(())
    }

    // ── Session event log ─────────────────────────────────────────────────

    /// Append a canonical record to the workspace's current session. Idempotent
    /// on `(session_id, native_id)`: a duplicate native_id is ignored and the
    /// original row's body is retained. Returns `true` if a new row was
    /// inserted, `false` if it was a duplicate or the workspace has no session.
    /// Append many transcript records in a single transaction. Same idempotency
    /// as `append_session_record` (ignored on a `(session_id, native_id)`
    /// conflict), but one commit for the whole batch instead of one per record —
    /// so turn-end ingest is O(batch) commits, not O(conversation). `seq` stays
    /// contiguous: an ignored duplicate doesn't burn a number. Returns how many
    /// rows were actually inserted.
    pub fn append_session_records(
        &self,
        workspace_id: &str,
        provider: &str,
        source: &str,
        agent_version: Option<&str>,
        records: &[(&str, &serde_json::Value)],
    ) -> Result<usize> {
        if records.is_empty() {
            return Ok(0);
        }
        let conn = self.db.lock();

        let sid: Option<String> = conn
            .query_row(
                "SELECT id FROM sessions WHERE workspace_id = ?1 ORDER BY created_at DESC LIMIT 1",
                [workspace_id],
                |r| r.get(0),
            )
            .ok();
        let Some(sid) = sid else {
            return Ok(0);
        };

        let now = now_millis();
        let tx = conn.unchecked_transaction()?;
        let mut seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM session_records WHERE session_id = ?1",
            [&sid],
            |r| r.get(0),
        )?;
        let mut inserted = 0usize;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO session_records
                    (session_id, seq, provider, source, native_id, agent_version, body, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for (native_id, body) in records {
                let body_json = serde_json::to_string(body)
                    .map_err(|e| Error::Other(format!("serialize record body: {e}")))?;
                let next = seq + 1;
                let n = stmt.execute(rusqlite::params![
                    sid, next, provider, source, native_id, agent_version, body_json, now
                ])?;
                if n > 0 {
                    seq = next; // consumed only on a real insert; dups keep seq dense
                    inserted += 1;
                }
            }
        }
        tx.commit()?;

        Ok(inserted)
    }

    /// Byte offset into the current session's transcript up to which records have
    /// been ingested — the resume point for an incremental tail read. 0 if there
    /// is no session yet or nothing has been ingested.
    pub fn session_ingest_offset(&self, workspace_id: &str) -> Result<u64> {
        let conn = self.db.lock();
        let offset: Option<i64> = conn
            .query_row(
                "SELECT ingest_offset FROM sessions WHERE workspace_id = ?1 ORDER BY created_at DESC LIMIT 1",
                [workspace_id],
                |r| r.get(0),
            )
            .ok();
        Ok(offset.unwrap_or(0).max(0) as u64)
    }

    /// Persist the tail offset for the current session after an incremental read.
    pub fn set_session_ingest_offset(&self, workspace_id: &str, offset: u64) -> Result<()> {
        let conn = self.db.lock();
        conn.execute(
            "UPDATE sessions SET ingest_offset = ?2
             WHERE id = (SELECT id FROM sessions WHERE workspace_id = ?1 ORDER BY created_at DESC LIMIT 1)",
            rusqlite::params![workspace_id, offset as i64],
        )?;
        Ok(())
    }

    /// Count of records already ingested for the current session (= MAX(seq)) —
    /// the starting index for positional `ln:{i}` native ids on the next read.
    pub fn session_record_count(&self, workspace_id: &str) -> Result<usize> {
        let conn = self.db.lock();
        let sid: Option<String> = conn
            .query_row(
                "SELECT id FROM sessions WHERE workspace_id = ?1 ORDER BY created_at DESC LIMIT 1",
                [workspace_id],
                |r| r.get(0),
            )
            .ok();
        let Some(sid) = sid else {
            return Ok(0);
        };
        let count: i64 = conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM session_records WHERE session_id = ?1",
            [&sid],
            |r| r.get(0),
        )?;
        Ok(count.max(0) as usize)
    }

    /// All canonical records for the workspace's current session, in seq order.
    pub fn read_session_records(&self, workspace_id: &str) -> Result<Vec<SessionRecord>> {
        let conn = self.db.lock();

        let sid: Option<String> = conn
            .query_row(
                "SELECT id FROM sessions WHERE workspace_id = ?1 ORDER BY created_at DESC LIMIT 1",
                [workspace_id],
                |r| r.get(0),
            )
            .ok();

        let Some(sid) = sid else {
            return Ok(vec![]);
        };

        let mut stmt = conn.prepare(
            "SELECT seq, provider, source, native_id, agent_version, body
             FROM session_records WHERE session_id = ?1 ORDER BY seq ASC",
        )?;

        let rows: Vec<(i64, String, String, String, Option<String>, String)> = stmt
            .query_map([&sid], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })?
            .collect::<std::result::Result<_, rusqlite::Error>>()?;

        rows.into_iter()
            .map(|(seq, provider, source, native_id, agent_version, body_text)| {
                let body = serde_json::from_str(&body_text)
                    .map_err(|e| Error::Other(format!("deserialize record body: {e}")))?;
                Ok(SessionRecord {
                    seq,
                    provider,
                    source,
                    native_id,
                    agent_version,
                    body,
                })
            })
            .collect()
    }

    // ── Outgoing user turns (session_user_turns) ──────────────────────────

    /// Insert an outgoing user message for the workspace's current session.
    /// Idempotent on `turn_id` (send auto-retries reuse the same id). Returns
    /// `true` if a new row was inserted, `false` on duplicate / no session.
    pub fn insert_user_turn(
        &self,
        workspace_id: &str,
        turn_id: &str,
        text: &str,
        attachments: &[String],
    ) -> Result<bool> {
        let conn = self.db.lock();
        let Some(sid) = current_session_id(&conn, workspace_id) else {
            return Ok(false);
        };
        let attachments_json = serde_json::to_string(attachments)
            .map_err(|e| Error::Other(format!("serialize attachments: {e}")))?;

        let tx = conn.unchecked_transaction()?;
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM session_user_turns WHERE session_id = ?1",
            [&sid],
            |r| r.get(0),
        )?;
        let n = tx.execute(
            "INSERT OR IGNORE INTO session_user_turns
                (turn_id, session_id, seq, text, attachments, native_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6)",
            rusqlite::params![turn_id, sid, seq, text, attachments_json, now_millis()],
        )?;
        tx.commit()?;
        Ok(n > 0)
    }

    /// All outgoing user turns for the workspace's current session, in seq order.
    pub fn read_user_turns(&self, workspace_id: &str) -> Result<Vec<UserTurn>> {
        let conn = self.db.lock();
        let Some(sid) = current_session_id(&conn, workspace_id) else {
            return Ok(vec![]);
        };
        let mut stmt = conn.prepare(
            "SELECT turn_id, seq, text, attachments, native_id
             FROM session_user_turns WHERE session_id = ?1 ORDER BY seq ASC",
        )?;
        let rows: Vec<(String, i64, String, String, Option<String>)> = stmt
            .query_map([&sid], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<std::result::Result<_, rusqlite::Error>>()?;
        rows.into_iter()
            .map(|(turn_id, seq, text, attachments_text, native_id)| {
                let attachments = serde_json::from_str(&attachments_text)
                    .map_err(|e| Error::Other(format!("deserialize attachments: {e}")))?;
                Ok(UserTurn {
                    turn_id,
                    seq,
                    text,
                    attachments,
                    native_id,
                })
            })
            .collect()
    }

    /// Match pending (`native_id IS NULL`) user turns to their canonical
    /// `session_records` user-message rows and fill in `native_id`. Run at
    /// turn-end after transcript ingest. Matching: for each pending turn (seq
    /// order) find the lowest-seq transcript record not already claimed whose
    /// body contains the turn's distinctive marker — the first attachment path
    /// (injected by the runner as `Attached file: <path>`) when present, else
    /// the prompt text. Returns the number newly associated.
    pub fn associate_pending_user_turns(&self, workspace_id: &str) -> Result<usize> {
        let conn = self.db.lock();
        let Some(sid) = current_session_id(&conn, workspace_id) else {
            return Ok(0);
        };

        // Pending turns, oldest first.
        let pending: Vec<(String, String, String)> = {
            let mut stmt = conn.prepare(
                "SELECT turn_id, text, attachments FROM session_user_turns
                 WHERE session_id = ?1 AND native_id IS NULL ORDER BY seq ASC",
            )?;
            let v = stmt
                .query_map([&sid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<std::result::Result<_, rusqlite::Error>>()?;
            v
        };
        if pending.is_empty() {
            return Ok(0);
        }

        // Transcript records, oldest first.
        let records: Vec<(String, String)> = {
            let mut stmt = conn.prepare(
                "SELECT native_id, body FROM session_records
                 WHERE session_id = ?1 AND source = 'transcript' ORDER BY seq ASC",
            )?;
            let v = stmt
                .query_map([&sid], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<std::result::Result<_, rusqlite::Error>>()?;
            v
        };

        // native_ids already claimed by any user turn for this session.
        let mut claimed: std::collections::HashSet<String> = {
            let mut stmt = conn.prepare(
                "SELECT native_id FROM session_user_turns
                 WHERE session_id = ?1 AND native_id IS NOT NULL",
            )?;
            let v = stmt
                .query_map([&sid], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<_, rusqlite::Error>>()?;
            v
        };

        let tx = conn.unchecked_transaction()?;
        let mut associated = 0usize;
        for (turn_id, text, attachments_text) in pending {
            let attachments: Vec<String> =
                serde_json::from_str(&attachments_text).unwrap_or_default();
            // Distinctive needle: an attachment path beats the prompt text
            // (paths are unique; text can be empty or duplicated).
            let needle = attachments.first().cloned().unwrap_or(text);
            if needle.is_empty() {
                continue;
            }
            let hit = records
                .iter()
                .find(|(nid, body)| !claimed.contains(nid) && body.contains(&needle));
            if let Some((nid, _)) = hit {
                tx.execute(
                    "UPDATE session_user_turns SET native_id = ?1 WHERE turn_id = ?2",
                    rusqlite::params![nid, turn_id],
                )?;
                claimed.insert(nid.clone());
                associated += 1;
            }
        }
        tx.commit()?;
        Ok(associated)
    }

    // ── Internal helpers ──────────────────────────────────────────────────

    /// Translate a requested runtime status into durable disposition writes.
    /// There is no status column — only the workspace's `stopped_at` and the
    /// session's `last_error` are persisted; everything else is derived.
    fn apply_status(
        conn: &Connection,
        id: &str,
        status: &AgentStatus,
        last_error: Option<&str>,
    ) -> Result<()> {
        match status {
            // User-stopped — stamp it once (don't clobber an earlier stop time).
            AgentStatus::Stopped => {
                conn.execute(
                    "UPDATE workspaces SET stopped_at = ?1
                     WHERE id = ?2 AND stopped_at IS NULL",
                    rusqlite::params![now_millis(), id],
                )?;
            }
            // Resuming or active — clear the stop disposition and any stale error.
            AgentStatus::Spawning | AgentStatus::Running => {
                conn.execute(
                    "UPDATE workspaces SET stopped_at = NULL WHERE id = ?1",
                    [id],
                )?;
                conn.execute(
                    "UPDATE sessions SET last_error = NULL WHERE workspace_id = ?1",
                    [id],
                )?;
            }
            // Record the failure on the session row.
            AgentStatus::Error => {
                conn.execute(
                    "UPDATE sessions SET last_error = ?1 WHERE workspace_id = ?2",
                    rusqlite::params![last_error, id],
                )?;
            }
            // Idle is a pure runtime state with no durable representation.
            AgentStatus::Idle => {}
        }
        Ok(())
    }

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
        // Identity + task metadata live on `workspaces`; the provider run
        // (provider / view / session id / last_error) lives on the single
        // `sessions` row. Status is derived, never selected.
        let mut stmt = match conn.prepare(
            "SELECT w.id, w.project_id, w.name, w.task, w.created_at,
                    w.stopped_at, w.archived_at,
                    s.provider, s.view, s.provider_session_id, s.last_error
             FROM workspaces w
             LEFT JOIN sessions s ON s.workspace_id = w.id
             ORDER BY w.created_at",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let agents: Vec<AgentRecord> = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let project_id: String = row.get(1)?;
                let name: String = row.get(2)?;
                let task: String = row.get(3)?;
                let created_millis: i64 = row.get(4)?;
                let stopped_millis: Option<i64> = row.get(5)?;
                let archived_millis: Option<i64> = row.get(6)?;
                let provider: Option<String> = row.get(7)?;
                let view_str: Option<String> = row.get(8)?;
                let session_id: Option<String> = row.get(9)?;
                let last_error: Option<String> = row.get(10)?;

                Ok((
                    id,
                    project_id,
                    name,
                    task,
                    created_millis,
                    stopped_millis,
                    archived_millis,
                    provider,
                    view_str,
                    session_id,
                    last_error,
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
                    task,
                    created_millis,
                    stopped_millis,
                    archived_millis,
                    provider,
                    view_str,
                    session_id,
                    last_error,
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

                    let status = derive_status(
                        is_archived,
                        stopped_millis.is_some(),
                        false,
                        last_error.as_deref(),
                    );

                    AgentRecord {
                        id,
                        project_id,
                        name,
                        provider: provider.unwrap_or_else(default_provider),
                        repos,
                        task,
                        status,
                        view: str_to_view(view_str.as_deref().unwrap_or("custom")),
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
             WHERE w.workspace_id = ?1
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
             WHERE w.workspace_id = ?1
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
            "INSERT INTO worktrees (id, workspace_id, repo_id, subdir, branch, parent_branch, created_at)
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
            "SELECT COUNT(*) FROM workspaces WHERE id = ?1",
            [id],
            |row| row.get(0),
        )?;
        if count == 0 {
            return Err(Error::AgentNotFound(id.to_string()));
        }
        Ok(())
    }

    fn load_agent(conn: &Connection, id: &str) -> Result<AgentRecord> {
        // Identity/task from `workspaces`; provider run from the single
        // `sessions` row. Status is derived from durable dispositions.
        let row = conn
            .query_row(
                "SELECT w.id, w.project_id, w.name, w.task, w.created_at,
                        w.stopped_at, w.archived_at,
                        s.provider, s.view, s.provider_session_id, s.last_error
                 FROM workspaces w
                 LEFT JOIN sessions s ON s.workspace_id = w.id
                 WHERE w.id = ?1",
                [id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, Option<String>>(8)?,
                        row.get::<_, Option<String>>(9)?,
                        row.get::<_, Option<String>>(10)?,
                    ))
                },
            )
            .map_err(|_| Error::AgentNotFound(id.to_string()))?;

        let (
            agent_id,
            project_id,
            name,
            task,
            created_millis,
            stopped_millis,
            archived_millis,
            provider,
            view_str,
            session_id,
            last_error,
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

        let status = derive_status(
            is_archived,
            stopped_millis.is_some(),
            false,
            last_error.as_deref(),
        );

        Ok(AgentRecord {
            id: agent_id,
            project_id,
            name,
            provider: provider.unwrap_or_else(default_provider),
            repos,
            task,
            status,
            view: str_to_view(view_str.as_deref().unwrap_or("custom")),
            session_id,
            created_at: millis_to_iso(created_millis),
            last_error,
            archive,
        })
    }
}

/// Per-turn agents run one process per turn, assign their own session id
/// (captured from their first turn's events rather than generated up
/// front), and render only in the structured (Custom) view for now. The
/// canonical set is the `agent::PER_TURN_AGENTS` descriptor table.
pub fn is_per_turn_provider(provider: &str) -> bool {
    crate::agent::per_turn_descriptor(provider).is_some()
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
    fn persists_across_instances_and_rests_at_idle() {
        let db = test_db();

        // Seed repo paths so add_agent can look them up.
        let td = tempfile::tempdir().unwrap();
        let repo = init_repo(td.path());
        seed_repo(&db, "/r");
        seed_repo(&db, "/r2");

        {
            let wm = WorkspaceManager::new(db.clone());
            wm.add_workspace_repo(repo.clone()).unwrap();
            // A resting agent has no durable disposition — it derives to
            // `Idle` (no live process, not running a turn).
            let mut running = new_agent_record(
                "yosemite".into(),
                "a".into(),
                "claude".into(),
                mk_repo("/r"),
                "c".into(),
                AgentView::Custom,
            );
            wm.add_agent(&mut running).unwrap();

            // An agent the user explicitly Stopped stamps `stopped_at`, so it
            // stays Stopped across reloads (available via manual Resume).
            let mut stopped = new_agent_record(
                "dolomites".into(),
                "s".into(),
                "claude".into(),
                mk_repo("/r2"),
                "sc".into(),
                AgentView::Custom,
            );
            wm.add_agent(&mut stopped).unwrap();
            wm.update_agent_status("dolomites", AgentStatus::Stopped, None)
                .unwrap();
        }

        // Second instance — status is derived, so the resting agent comes
        // back as Idle and the stopped one stays Stopped.
        let wm2 = WorkspaceManager::new(db);
        let cur = wm2.current().unwrap();
        assert!(cur.repos.iter().any(|p| p == &repo));
        assert_eq!(cur.agents.len(), 2);

        let yosemite = cur.agents.iter().find(|a| a.id == "yosemite").unwrap();
        let dolomites = cur.agents.iter().find(|a| a.id == "dolomites").unwrap();
        assert_eq!(yosemite.status, AgentStatus::Idle);
        assert_eq!(dolomites.status, AgentStatus::Stopped);
    }

    #[test]
    fn status_derivation() {
        // Archived workspaces are stopped regardless of error/run state.
        assert_eq!(
            derive_status(true, false, false, None),
            AgentStatus::Stopped
        );
        // User-stopped workspaces are stopped.
        assert_eq!(
            derive_status(false, true, false, None),
            AgentStatus::Stopped
        );
        // A recorded error with no live process surfaces as Error.
        assert_eq!(
            derive_status(false, false, false, Some("boom")),
            AgentStatus::Error
        );
        // A live process wins over a stale error.
        assert_eq!(
            derive_status(false, false, true, Some("boom")),
            AgentStatus::Running
        );
        // A resting, clean workspace derives to Idle (lazy resume on send).
        assert_eq!(
            derive_status(false, false, false, None),
            AgentStatus::Idle
        );
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

        // Fresh agent derives Idle (no durable disposition, no live process).
        assert_eq!(wm.agent(&id).unwrap().status, AgentStatus::Idle);

        // Stopping stamps a durable disposition → derives Stopped.
        wm.update_agent_status(&id, AgentStatus::Stopped, None)
            .unwrap();
        assert_eq!(wm.agent(&id).unwrap().status, AgentStatus::Stopped);

        // Resuming clears the stop disposition → back to Idle.
        wm.update_agent_status(&id, AgentStatus::Running, None)
            .unwrap();
        assert_eq!(wm.agent(&id).unwrap().status, AgentStatus::Idle);

        // Recording an error surfaces as Error (no live process at rest).
        wm.update_agent_status(&id, AgentStatus::Error, Some("boom".into()))
            .unwrap();
        let a = wm.agent(&id).unwrap();
        assert_eq!(a.status, AgentStatus::Error);
        assert_eq!(a.last_error.as_deref(), Some("boom"));

        // Resuming again clears the error → back to Idle.
        wm.update_agent_status(&id, AgentStatus::Spawning, None)
            .unwrap();
        let a = wm.agent(&id).unwrap();
        assert_eq!(a.status, AgentStatus::Idle);
        assert!(a.last_error.is_none());
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

        // Restore puts repos back and clears the archived/stopped
        // disposition, so the record derives Idle. (The supervisor's
        // restore path then drives the live spawn separately.)
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
        assert_eq!(a.status, AgentStatus::Idle);
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

    // ── session event log ─────────────────────────────────────────────────

    /// Seed a minimal workspace+session row and return (workspace_id, wm).
    fn make_workspace_with_session(db: &Arc<Mutex<Connection>>) -> (String, WorkspaceManager) {
        let td = tempfile::tempdir().unwrap();
        let repo = init_repo(td.path());
        let repo_str = repo.to_str().unwrap().to_string();
        let wm = WorkspaceManager::new(db.clone());
        wm.add_workspace_repo(repo.clone()).unwrap();

        let mut rec = new_agent_record(
            uuid::Uuid::new_v4().to_string(),
            "evt-test".into(),
            "claude".into(),
            TrackedRepo {
                repo_path: repo,
                subdir: "repo".into(),
                branch: None,
                parent_branch: None,
            },
            "task".into(),
            AgentView::Custom,
        );
        // add_agent needs the repo pre-seeded in repos; add_workspace_repo
        // handles that above. But we also need the repo in the repos table
        // for the worktree join — seed it explicitly so the lookup succeeds.
        let _ = seed_repo_path(db, &repo_str);
        wm.add_agent(&mut rec).unwrap();
        let id = rec.id.clone();
        (id, wm)
    }

    fn seed_repo_path(db: &Arc<Mutex<Connection>>, repo_path: &str) {
        // No-op if already there; used to guarantee the row exists for the
        // worktree FK before add_agent runs the lookup.
        let conn = db.lock();
        let path = std::path::Path::new(repo_path);
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

    // ── session record store (canonical) ──────────────────────────────────

    #[test]
    fn append_and_read_session_records_roundtrip() {
        let db = test_db();
        let (ws_id, wm) = make_workspace_with_session(&db);

        let body = serde_json::json!({"role": "user", "content": "hello"});
        let inserted = wm
            .append_session_records(&ws_id, "claude", "transcript", Some("1.2.3"), &[("uuid-1", &body)])
            .unwrap();
        assert_eq!(inserted, 1);

        let records = wm.read_session_records(&ws_id).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].provider, "claude");
        assert_eq!(records[0].source, "transcript");
        assert_eq!(records[0].native_id, "uuid-1");
        assert_eq!(records[0].agent_version.as_deref(), Some("1.2.3"));
        assert_eq!(records[0].body, body);
        assert_eq!(records[0].seq, 1);
    }

    #[test]
    fn append_session_record_is_idempotent_on_native_id() {
        let db = test_db();
        let (ws_id, wm) = make_workspace_with_session(&db);

        let first = serde_json::json!({"n": 1});
        let dup = serde_json::json!({"n": 2});
        assert_eq!(
            wm.append_session_records(&ws_id, "pi", "transcript", None, &[("id-a", &first)])
                .unwrap(),
            1
        );
        // Same (session, native_id) — must be ignored, original body retained.
        assert_eq!(
            wm.append_session_records(&ws_id, "pi", "transcript", None, &[("id-a", &dup)])
                .unwrap(),
            0
        );

        let records = wm.read_session_records(&ws_id).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].body, first);
        assert_eq!(records[0].agent_version, None);
    }

    #[test]
    fn append_session_records_batches_in_one_pass_and_is_idempotent() {
        let db = test_db();
        let (ws_id, wm) = make_workspace_with_session(&db);

        let a = serde_json::json!({"n": 1});
        let b = serde_json::json!({"n": 2});
        let c = serde_json::json!({"n": 3});

        // First batch: all three land, seq contiguous in order.
        let inserted = wm
            .append_session_records(
                &ws_id,
                "claude",
                "transcript",
                None,
                &[("id-a", &a), ("id-b", &b), ("id-c", &c)],
            )
            .unwrap();
        assert_eq!(inserted, 3);

        let records = wm.read_session_records(&ws_id).unwrap();
        assert_eq!(records.iter().map(|r| r.seq).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(
            records.iter().map(|r| r.native_id.as_str()).collect::<Vec<_>>(),
            vec!["id-a", "id-b", "id-c"],
        );

        // Re-running with two already-stored + one new inserts only the new one,
        // and seq stays contiguous (ignored dups don't burn a seq).
        let d = serde_json::json!({"n": 4});
        let inserted = wm
            .append_session_records(
                &ws_id,
                "claude",
                "transcript",
                None,
                &[("id-b", &b), ("id-c", &c), ("id-d", &d)],
            )
            .unwrap();
        assert_eq!(inserted, 1);

        let records = wm.read_session_records(&ws_id).unwrap();
        assert_eq!(records.iter().map(|r| r.seq).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
        assert_eq!(records[3].native_id, "id-d");
        assert_eq!(records[3].body, d);
    }

    #[test]
    fn ingest_offset_and_record_count_roundtrip() {
        let db = test_db();
        let (ws_id, wm) = make_workspace_with_session(&db);

        // Defaults before anything is ingested.
        assert_eq!(wm.session_ingest_offset(&ws_id).unwrap(), 0);
        assert_eq!(wm.session_record_count(&ws_id).unwrap(), 0);

        let a = serde_json::json!({"n": 1});
        let b = serde_json::json!({"n": 2});
        wm.append_session_records(&ws_id, "claude", "transcript", None, &[("x", &a), ("y", &b)])
            .unwrap();
        // record_count tracks MAX(seq) — the start index for the next tail read.
        assert_eq!(wm.session_record_count(&ws_id).unwrap(), 2);

        wm.set_session_ingest_offset(&ws_id, 4096).unwrap();
        assert_eq!(wm.session_ingest_offset(&ws_id).unwrap(), 4096);
    }

    #[test]
    fn session_records_seq_increments_in_order() {
        let db = test_db();
        let (ws_id, wm) = make_workspace_with_session(&db);

        let a = serde_json::json!({"a": 1});
        let b = serde_json::json!({"a": 2});
        wm.append_session_records(&ws_id, "pi", "transcript", None, &[("ln:0", &a)])
            .unwrap();
        wm.append_session_records(&ws_id, "pi", "transcript", None, &[("ln:1", &b)])
            .unwrap();

        let records = wm.read_session_records(&ws_id).unwrap();
        let seqs: Vec<i64> = records.iter().map(|r| r.seq).collect();
        assert_eq!(seqs, vec![1, 2]);
        assert_eq!(records[0].native_id, "ln:0");
        assert_eq!(records[1].native_id, "ln:1");
    }

    #[test]
    fn read_session_records_empty_when_none() {
        let db = test_db();
        let (ws_id, wm) = make_workspace_with_session(&db);
        assert!(wm.read_session_records(&ws_id).unwrap().is_empty());
    }

    #[test]
    fn append_session_record_to_workspace_with_no_session_is_noop() {
        let db = test_db();
        make_workspace_with_session(&db);
        let wm = WorkspaceManager::new(db.clone());
        // Unknown workspace id → no session → nothing inserted, read empty.
        let body = serde_json::json!({});
        let inserted = wm
            .append_session_records("no-such-ws", "claude", "transcript", None, &[("x", &body)])
            .unwrap();
        assert_eq!(inserted, 0);
        assert!(wm.read_session_records("no-such-ws").unwrap().is_empty());
    }

    #[test]
    fn insert_and_read_user_turns_roundtrip() {
        let db = test_db();
        let (ws_id, wm) = make_workspace_with_session(&db);

        assert!(wm
            .insert_user_turn(&ws_id, "turn-1", "hello", &["/tmp/a.png".into()])
            .unwrap());

        let turns = wm.read_user_turns(&ws_id).unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].turn_id, "turn-1");
        assert_eq!(turns[0].seq, 1);
        assert_eq!(turns[0].text, "hello");
        assert_eq!(turns[0].attachments, vec!["/tmp/a.png".to_string()]);
        assert_eq!(turns[0].native_id, None);
    }

    #[test]
    fn insert_user_turn_is_idempotent_on_turn_id() {
        let db = test_db();
        let (ws_id, wm) = make_workspace_with_session(&db);

        assert!(wm.insert_user_turn(&ws_id, "turn-1", "first", &[]).unwrap());
        // Same turn_id (a send retry) — ignored, original retained, no new row.
        assert!(!wm.insert_user_turn(&ws_id, "turn-1", "second", &[]).unwrap());

        let turns = wm.read_user_turns(&ws_id).unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].text, "first");
    }

    #[test]
    fn associate_pending_user_turns_matches_attachment_path_then_text() {
        let db = test_db();
        let (ws_id, wm) = make_workspace_with_session(&db);

        // Two outgoing turns: one with an attachment, one plain.
        wm.insert_user_turn(&ws_id, "t1", "look at this", &["/tmp/diagram.png".into()])
            .unwrap();
        wm.insert_user_turn(&ws_id, "t2", "now refactor", &[]).unwrap();

        // Transcript user-message records as the agent logged them (attachment
        // turn carries the injected reference line; plain turn is just text).
        let rec_a = serde_json::json!({"role": "user", "text": "look at this\nAttached file: /tmp/diagram.png"});
        let rec_b = serde_json::json!({"role": "user", "text": "now refactor"});
        wm.append_session_records(
            &ws_id,
            "claude",
            "transcript",
            None,
            &[("rec-A", &rec_a), ("rec-B", &rec_b)],
        )
        .unwrap();

        let n = wm.associate_pending_user_turns(&ws_id).unwrap();
        assert_eq!(n, 2);

        let turns = wm.read_user_turns(&ws_id).unwrap();
        assert_eq!(turns[0].native_id.as_deref(), Some("rec-A"));
        assert_eq!(turns[1].native_id.as_deref(), Some("rec-B"));

        // Idempotent: re-running associates nothing new.
        assert_eq!(wm.associate_pending_user_turns(&ws_id).unwrap(), 0);
    }

    #[test]
    fn associate_leaves_unmatched_turn_pending() {
        let db = test_db();
        let (ws_id, wm) = make_workspace_with_session(&db);

        // Sent, but the agent never logged it (call failed) — no transcript row.
        wm.insert_user_turn(&ws_id, "t1", "never delivered", &[])
            .unwrap();
        assert_eq!(wm.associate_pending_user_turns(&ws_id).unwrap(), 0);

        let turns = wm.read_user_turns(&ws_id).unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].native_id, None); // still pending → renders standalone
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
