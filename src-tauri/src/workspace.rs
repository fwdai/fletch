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
    /// The immutable fork-point commit this worktree was created from,
    /// captured at spawn after a best-effort fetch. Used as the diff base so
    /// agent changes are measured against the exact starting commit rather
    /// than a branch name that may drift. `None` for pre-migration agents and
    /// when the fork SHA couldn't be resolved; readers fall back to
    /// `parent_branch`. Distinct from `parent_branch`, which names the branch
    /// for PR/merge bases.
    #[serde(default)]
    pub base_sha: Option<String>,
    /// The GitHub PR number this worktree's branch was opened as, once known.
    /// Set when a PR is created through the app or adopted from an OPEN
    /// out-of-band PR. PR state is fetched by this number, not by branch name,
    /// so a recycled workspace name can't resolve to a prior agent's PR.
    /// `None` until a PR exists.
    #[serde(default)]
    pub pr_number: Option<i64>,
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
    /// Claude's session-level reasoning effort (`--effort <level>`), chosen in
    /// the composer at session creation and applied on every process spawn
    /// (fresh, view-switch, resume). `None` = no selection; claude uses its
    /// own default. Only the persistent-runner agent (claude) consumes this;
    /// per-turn agents take effort per-turn instead.
    #[serde(default)]
    pub effort: Option<String>,
    /// Model selected at session creation. `None` means the provider CLI should
    /// use its configured/default model.
    #[serde(default)]
    pub model: Option<String>,
    /// A custom agent's standing instructions, snapshotted at spawn and
    /// re-injected on every process spawn/resume (after Fletch's global system
    /// prompt). `None` for a plain built-in spawn. Snapshotting (rather than
    /// re-resolving from `custom_agents`) keeps a running agent's brief stable
    /// even if the custom agent is later edited or deleted.
    #[serde(default)]
    pub instructions: Option<String>,
    /// The custom agent this session was spawned from, used to show its
    /// name/color in the sidebar. `None` for a plain built-in spawn.
    #[serde(default)]
    pub custom_agent_id: Option<String>,
    /// Sandbox engine stamped at creation (an `EngineKind::as_setting`
    /// spelling) and reused on every process spawn, so a settings change never
    /// re-engines an existing agent. `None` = created before engine selection
    /// existed — such agents always ran (and keep running) under sandbox-exec.
    #[serde(default)]
    pub sandbox_engine: Option<String>,
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
    /// Per-repo project metadata (custom display name + project id), parallel
    /// to `repos`. The sidebar shows `name` — a user-editable label that
    /// defaults to the folder basename but survives renames and relocations.
    #[serde(default)]
    pub projects: Vec<ProjectRef>,
    #[serde(default)]
    pub agents: Vec<AgentRecord>,
}

/// A pinned repo joined with its owning project, so the frontend can show a
/// custom name (independent of the folder basename) and address the project
/// for rename / relocate without a second round-trip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRef {
    pub path: PathBuf,
    pub name: String,
    pub project_id: String,
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

/// One Fletch-origin outgoing user message (the `session_user_turns` table).
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
    /// Wall-clock millis when the turn flipped to Running. `None` for turns
    /// that never started a run (native PTY turns, or rows from before timing
    /// existed).
    pub started_at: Option<i64>,
    /// Wall-clock millis when the turn reached a terminal state. `None` while
    /// in flight — the live-timer signal.
    pub ended_at: Option<i64>,
}

/// Stats for a turn that `mark_user_turn_ended` just closed, returned so the
/// caller can emit per-turn telemetry. `None` from that call means no open turn
/// existed (resting Idle at spawn, or a native turn with no timing row).
pub struct ClosedTurn {
    /// Wall-clock run duration (`ended_at - started_at`).
    pub duration_ms: i64,
    /// Transcript records ingested during the turn window — a proxy for how
    /// much the agent produced this turn.
    pub record_count: i64,
}

pub struct WorkspaceManager {
    db: Arc<Mutex<Connection>>,
}

/// Identity + task metadata live on `workspaces`; the provider run
/// (provider / view / session id / last_error / effort / model /
/// instructions / custom agent) lives on the single `sessions` row. Status
/// is derived, never selected. Callers append their own `ORDER BY` / `WHERE`.
const AGENT_SELECT: &str = "SELECT w.id, w.project_id, w.name, w.task, w.created_at,
            w.stopped_at, w.archived_at,
            s.provider, s.view, s.provider_session_id, s.last_error,
            s.effort, s.model, s.instructions, s.custom_agent_id,
            w.sandbox_engine
     FROM workspaces w
     LEFT JOIN sessions s ON s.workspace_id = w.id";

/// Raw column tuple decoded from an [`AGENT_SELECT`] row, in column order.
type AgentRow = (
    String,         // w.id
    String,         // w.project_id
    String,         // w.name
    String,         // w.task
    i64,            // w.created_at
    Option<i64>,    // w.stopped_at
    Option<i64>,    // w.archived_at
    Option<String>, // s.provider
    Option<String>, // s.view
    Option<String>, // s.provider_session_id
    Option<String>, // s.last_error
    Option<String>, // s.effort
    Option<String>, // s.model
    Option<String>, // s.instructions
    Option<String>, // s.custom_agent_id
    Option<String>, // w.sandbox_engine
);

impl WorkspaceManager {
    pub fn new(db: Arc<Mutex<Connection>>) -> Self {
        // Status is derived, not stored, so there is nothing to reconcile at
        // load time: a resting, non-stopped, non-errored workspace derives to
        // `Idle` (no live process; resumed lazily on the next interaction),
        // while user-stopped workspaces keep their `stopped_at` and derive to
        // `Stopped` until the manual Resume button clears it.
        Self { db }
    }

    /// Read one key from the key-value `settings` table (`None` when unset).
    /// Backend-side settings reads (e.g. the `workspace_mode` dev flag) go
    /// through here so callers never need the raw connection.
    pub fn setting(&self, key: &str) -> Option<String> {
        crate::database::get_setting(&self.db.lock(), key)
    }

    pub fn current(&self) -> Option<Workspace> {
        let conn = self.db.lock();

        // Collect all unique repo paths.
        let repos = Self::query_all_repo_paths(&conn);

        // Project metadata (custom name + id) for each pinned repo.
        let projects = Self::query_project_refs(&conn);

        // Collect all agents.
        let agents = Self::query_all_agents(&conn);

        Some(Workspace {
            repos,
            projects,
            agents,
        })
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

    /// Set a project's display name, decoupled from its folder basename. The
    /// name is trimmed; an empty name is rejected so a project always has a
    /// label. Does not touch the repo path or anything on disk.
    pub fn rename_project(&self, project_id: &str, name: &str) -> Result<Workspace> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(Error::Other("project name cannot be empty".into()));
        }

        let conn = self.db.lock();
        let changed = conn.execute(
            "UPDATE projects SET name = ?1 WHERE id = ?2",
            rusqlite::params![trimmed, project_id],
        )?;
        if changed == 0 {
            return Err(Error::Other(format!("project not found: {project_id}")));
        }
        drop(conn);
        Ok(self.current().expect("workspace initialized"))
    }

    /// Repoint a pinned repo at a new location on disk. The user has already
    /// moved the folder; this only updates the stored reference so future
    /// agents spawn from the right place. Validates the new path is a git repo
    /// and isn't already pinned. Existing agents' worktrees are NOT relinked —
    /// they were forked from the old location and keep pointing there.
    pub fn relocate_repo(&self, old_path: &Path, new_path: &Path) -> Result<Workspace> {
        if !new_path.join(".git").exists() {
            return Err(Error::InvalidPath(format!(
                "not a git repository: {}",
                new_path.display()
            )));
        }

        let conn = self.db.lock();
        let old_str = old_path.to_string_lossy().to_string();
        let new_str = new_path.to_string_lossy().to_string();

        if old_str == new_str {
            drop(conn);
            return Ok(self.current().expect("workspace initialized"));
        }

        // `repos.path` is UNIQUE — refuse to collide with a repo already pinned
        // at the destination rather than fail with an opaque constraint error.
        let taken: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM repos WHERE path = ?1",
                [&new_str],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);
        if taken {
            return Err(Error::Other(format!(
                "a project is already pinned at {new_str}"
            )));
        }

        let changed = conn.execute(
            "UPDATE repos SET path = ?1 WHERE path = ?2",
            rusqlite::params![new_str, old_str],
        )?;
        if changed == 0 {
            return Err(Error::Other(format!("repo not found: {old_str}")));
        }
        drop(conn);
        Ok(self.current().expect("workspace initialized"))
    }

    pub fn allocate_agent_id(&self) -> Result<String> {
        let conn = self.db.lock();
        // Only *live* (non-archived) agents reserve their name. Once an agent
        // is archived its worktree is torn down, so the name is free to reuse —
        // unless a directory still lingers on disk (cleanup failed, or it
        // belongs to another running instance such as a dev build, which shares
        // this same worktrees root). The on-disk listing closes that gap: it's
        // the only namespace shared across every Fletch process on the machine,
        // so a collision there is what actually breaks `git worktree add`.
        let mut stmt =
            conn.prepare("SELECT id FROM workspaces WHERE archived_at IS NULL")?;
        let mut used: HashSet<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        used.extend(occupied_worktree_dirs());
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

        // Evicting the recycled archived row and writing its replacement must be
        // atomic. Without a transaction the DELETE auto-commits immediately, so
        // any later failure (a failed INSERT, disk-full, UUID collision) would
        // leave the archived agent — and its cascaded sessions/worktrees —
        // permanently gone with nothing in its place. The transaction rolls the
        // DELETE back on any error before `commit`.
        let tx = conn.unchecked_transaction()?;

        // Recycling a freed name: the allocator only hands back ids held by
        // *archived* agents (live ones and on-disk worktrees are excluded), but
        // the archived row still owns this primary key. Evict it so the INSERT
        // below doesn't trip the PK constraint. Cascades clear its sessions,
        // worktrees, and session records. A *live* row with this id would be a
        // genuine bug, so we deliberately don't touch those — the INSERT will
        // surface the conflict instead of silently clobbering a running agent.
        let recycled = tx.execute(
            "DELETE FROM workspaces WHERE id = ?1 AND archived_at IS NOT NULL",
            rusqlite::params![record.id],
        )?;
        if recycled > 0 {
            tracing::info!(
                agent_id = %record.id,
                "reusing archived agent name; evicted its archived record",
            );
        }

        // The workspace is the durable work-area (identity + task metadata).
        tx.execute(
            "INSERT INTO workspaces (id, project_id, name, task, created_at, sandbox_engine)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                record.id,
                project_id,
                record.name,
                record.task,
                created_millis,
                record.sandbox_engine,
            ],
        )?;

        // Exactly one provider run per workspace today. The runtime status is
        // not persisted — it derives from the workspace/session dispositions.
        let session_id = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO sessions (id, workspace_id, provider, view, provider_session_id, last_error, effort, model, instructions, custom_agent_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                session_id,
                record.id,
                record.provider,
                view_to_str(&record.view),
                record.session_id,
                record.last_error,
                record.effort,
                record.model,
                record.instructions,
                record.custom_agent_id,
                created_millis,
            ],
        )?;

        // Insert worktree records for each TrackedRepo.
        for repo in &record.repos {
            Self::insert_worktree(&tx, &record.id, repo)?;
        }

        tx.commit()?;
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
    /// Record the branch a tracked repo's worktree is on, identified by subdir.
    /// Written when the agent materializes its branch at first push (see
    /// `open_pr`/`git_push`). Overwrites unconditionally — a second PR cuts a
    /// fresh branch in the same worktree, so the recorded name can change.
    pub fn set_repo_branch(&self, agent_id: &str, subdir: &str, branch: &str) -> Result<()> {
        let conn = self.db.lock();
        conn.execute(
            "UPDATE worktrees SET branch = ?1 WHERE workspace_id = ?2 AND subdir = ?3",
            rusqlite::params![branch, agent_id, subdir],
        )?;
        Ok(())
    }

    /// Record the fork-point SHA for a tracked repo, identified by subdir.
    /// Written once the spawn task has created the worktree and resolved its
    /// HEAD. Overwrites unconditionally — the fork point is fixed for the
    /// worktree's life, so a re-write only ever sets the same value.
    pub fn set_repo_base_sha(
        &self,
        agent_id: &str,
        subdir: &str,
        base_sha: &str,
    ) -> Result<()> {
        let conn = self.db.lock();
        conn.execute(
            "UPDATE worktrees SET base_sha = ?1 WHERE workspace_id = ?2 AND subdir = ?3",
            rusqlite::params![base_sha, agent_id, subdir],
        )?;
        Ok(())
    }

    /// Record the GitHub PR number for a tracked repo, identified by subdir.
    /// Written when a PR is created through the app or adopted from an OPEN
    /// out-of-band PR. Overwrites unconditionally — the latest PR opened for
    /// the branch is the one we track.
    pub fn set_repo_pr_number(
        &self,
        agent_id: &str,
        subdir: &str,
        pr_number: i64,
    ) -> Result<()> {
        let conn = self.db.lock();
        conn.execute(
            "UPDATE worktrees SET pr_number = ?1 WHERE workspace_id = ?2 AND subdir = ?3",
            rusqlite::params![pr_number, agent_id, subdir],
        )?;
        Ok(())
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

    /// A single project-scoped setting value (e.g. the Run panel's
    /// `run.install` / `run.dev` overrides). `None` when unset.
    pub fn project_setting(&self, project_id: &str, key: &str) -> Option<String> {
        let conn = self.db.lock();
        conn.query_row(
            "SELECT value FROM project_settings WHERE project_id = ?1 AND key = ?2",
            rusqlite::params![project_id, key],
            |row| row.get::<_, String>(0),
        )
        .ok()
    }

    /// Resolve the project_id for a repo path (creating the project/repo
    /// record if it doesn't exist yet — idempotent). The sidebar keys its
    /// project groups by repo path, so the Project Settings surface uses
    /// this to reach the `project_settings` rows, which are keyed by
    /// project_id.
    pub fn project_id_for_repo(&self, repo_path: &str) -> Result<String> {
        let conn = self.db.lock();
        Self::project_id_for_repo_path(&conn, repo_path)
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

    /// Stamp a turn's run start when it flips to Running, with the caller's
    /// timestamp so the same value reaches the live timer (via the `turn:started`
    /// event) and the persisted duration. Guarded on `started_at IS NULL` so a
    /// delivery retry (same `turn_id`) never resets the clock. No-op when the row
    /// doesn't exist (native PTY turns carry no timing row).
    pub fn mark_user_turn_started(&self, turn_id: &str, started_at: i64) -> Result<()> {
        let conn = self.db.lock();
        conn.execute(
            "UPDATE session_user_turns SET started_at = ?1
             WHERE turn_id = ?2 AND started_at IS NULL",
            rusqlite::params![started_at, turn_id],
        )?;
        Ok(())
    }

    /// Close the in-flight turn at turn end by stamping `ended_at` on the open
    /// turn (started, not yet ended) of the workspace's current session, and
    /// return its stats for telemetry. `None` when none is open — e.g. the
    /// resting Idle emitted at spawn, or a native turn with no timing row. At
    /// most one turn is ever open per session (each end closes the open turn
    /// before the next one starts), but the `WHERE` would safely close all open
    /// turns if one were ever stranded; duration then anchors on the earliest.
    pub fn mark_user_turn_ended(&self, workspace_id: &str) -> Result<Option<ClosedTurn>> {
        let conn = self.db.lock();
        let Some(sid) = current_session_id(&conn, workspace_id) else {
            return Ok(None);
        };
        let started_at: Option<i64> = conn.query_row(
            "SELECT MIN(started_at) FROM session_user_turns
             WHERE session_id = ?1 AND started_at IS NOT NULL AND ended_at IS NULL",
            [&sid],
            |r| r.get(0),
        )?;
        let Some(started_at) = started_at else {
            return Ok(None);
        };
        let now = now_millis();
        conn.execute(
            "UPDATE session_user_turns SET ended_at = ?1
             WHERE session_id = ?2 AND started_at IS NOT NULL AND ended_at IS NULL",
            rusqlite::params![now, sid],
        )?;
        // Records land before the terminal event that trips turn-end detection,
        // so the window is complete by the time we get here.
        let record_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM session_records
             WHERE session_id = ?1 AND created_at BETWEEN ?2 AND ?3",
            rusqlite::params![sid, started_at, now],
            |r| r.get(0),
        )?;
        Ok(Some(ClosedTurn {
            duration_ms: now - started_at,
            record_count,
        }))
    }

    /// All outgoing user turns for the workspace's current session, in seq order.
    pub fn read_user_turns(&self, workspace_id: &str) -> Result<Vec<UserTurn>> {
        let conn = self.db.lock();
        let Some(sid) = current_session_id(&conn, workspace_id) else {
            return Ok(vec![]);
        };
        let mut stmt = conn.prepare(
            "SELECT turn_id, seq, text, attachments, native_id, started_at, ended_at
             FROM session_user_turns WHERE session_id = ?1 ORDER BY seq ASC",
        )?;
        let rows: Vec<(String, i64, String, String, Option<String>, Option<i64>, Option<i64>)> =
            stmt.query_map([&sid], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            })?
            .collect::<std::result::Result<_, rusqlite::Error>>()?;
        rows.into_iter()
            .map(
                |(turn_id, seq, text, attachments_text, native_id, started_at, ended_at)| {
                    let attachments = serde_json::from_str(&attachments_text)
                        .map_err(|e| Error::Other(format!("deserialize attachments: {e}")))?;
                    Ok(UserTurn {
                        turn_id,
                        seq,
                        text,
                        attachments,
                        native_id,
                        started_at,
                        ended_at,
                    })
                },
            )
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
            // The body is stored as serde_json::to_string(value), so characters
            // like newlines appear JSON-escaped (\n) in the stored string. Escape
            // the needle the same way so the substring match works for multi-line
            // messages. serde_json::to_string wraps in quotes; strip them.
            let needle_escaped = serde_json::to_string(&needle)
                .map(|s| s[1..s.len() - 1].to_string())
                .unwrap_or(needle.clone());
            let hit = records
                .iter()
                .find(|(nid, body)| !claimed.contains(nid) && body.contains(&needle_escaped));
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

    /// One `ProjectRef` per repo, keyed by path (the frontend looks these up by
    /// path, not by index). A LEFT JOIN keeps a repo even if its project row is
    /// somehow missing — the name then falls back to the folder basename rather
    /// than the repo silently vanishing from the sidebar.
    fn query_project_refs(conn: &Connection) -> Vec<ProjectRef> {
        let mut stmt = match conn.prepare(
            "SELECT r.path, p.name, p.id
             FROM repos r LEFT JOIN projects p ON p.id = r.project_id
             ORDER BY r.created_at",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([], |row| {
            let path: String = row.get(0)?;
            let name: Option<String> = row.get(1)?;
            let project_id: Option<String> = row.get(2)?;
            let name = name.unwrap_or_else(|| {
                Path::new(&path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&path)
                    .to_string()
            });
            Ok(ProjectRef {
                path: PathBuf::from(path),
                name,
                project_id: project_id.unwrap_or_default(),
            })
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
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
        let mut stmt = match conn.prepare(&format!("{AGENT_SELECT} ORDER BY w.created_at")) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        // Collect the raw rows first, then build records: building a record
        // issues further queries on `conn`, which can't run while `stmt`
        // still borrows it.
        stmt.query_map([], Self::map_agent_row)
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .map(|row| Self::build_agent_record(conn, row))
            .collect()
    }

    fn query_tracked_repos(conn: &Connection, agent_id: &str) -> Vec<TrackedRepo> {
        let mut stmt = match conn.prepare(
            "SELECT r.path, w.subdir, w.branch, w.parent_branch, w.base_sha, w.pr_number
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
            let base_sha: Option<String> = row.get(4)?;
            let pr_number: Option<i64> = row.get(5)?;
            Ok(TrackedRepo {
                repo_path: PathBuf::from(path),
                subdir,
                branch,
                parent_branch,
                base_sha,
                pr_number,
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
            "INSERT INTO worktrees (id, workspace_id, repo_id, subdir, branch, parent_branch, base_sha, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                wt_id,
                agent_id,
                repo_id,
                repo.subdir,
                repo.branch,
                repo.parent_branch,
                repo.base_sha,
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
        let row = conn
            .query_row(
                &format!("{AGENT_SELECT} WHERE w.id = ?1"),
                [id],
                Self::map_agent_row,
            )
            .map_err(|_| Error::AgentNotFound(id.to_string()))?;

        Ok(Self::build_agent_record(conn, row))
    }

    /// Map a row from an [`AGENT_SELECT`] query into the raw column tuple.
    /// Shared by `query_all_agents` and `load_agent` so the 16-column layout
    /// is decoded in exactly one place.
    fn map_agent_row(row: &rusqlite::Row) -> rusqlite::Result<AgentRow> {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
            row.get(8)?,
            row.get(9)?,
            row.get(10)?,
            row.get(11)?,
            row.get(12)?,
            row.get(13)?,
            row.get(14)?,
            row.get(15)?,
        ))
    }

    /// Build an [`AgentRecord`] from a raw [`AGENT_SELECT`] row, issuing the
    /// follow-up queries for tracked repos or archive metadata. Status is
    /// derived from durable dispositions, never selected.
    fn build_agent_record(conn: &Connection, row: AgentRow) -> AgentRecord {
        let (
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
            effort,
            model,
            instructions,
            custom_agent_id,
            sandbox_engine,
        ) = row;

        let is_archived = archived_millis.is_some();

        let (repos, archive) = if is_archived {
            // Build ArchiveMetadata from worktree snapshot fields.
            let archive_meta = Self::build_archive_metadata(conn, &id, archived_millis.unwrap());
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
            effort,
            model,
            instructions,
            custom_agent_id,
            sandbox_engine,
            created_at: millis_to_iso(created_millis),
            last_error,
            archive,
        }
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
        effort: None,
        model: None,
        instructions: None,
        custom_agent_id: None,
        // Stamped by the spawn path (`supervisor::lifecycle::spawn_agent`)
        // from the live setting — callers building records directly (tests)
        // default to the pre-selection NULL, which spawns under sandbox-exec.
        sandbox_engine: None,
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

/// Env var overriding the worktrees root (default `~/.fletch/worktrees`). The
/// Run sandbox forbids writes to the host's `~/.fletch/worktrees`, so a nested
/// Fletch launched as a Run process (dogfooding: Fletch running Fletch) is
/// pointed at a sandbox-writable root instead — see
/// `sandbox::nested_worktrees_root`. Mirrors `rpc::RPC_ROOT_ENV`.
pub const WORKTREES_ROOT_ENV: &str = "FLETCH_WORKTREES_ROOT";

/// Absolute path to the root holding every agent's worktrees:
/// `~/.fletch/worktrees/`. Shared by *all* Fletch processes on the machine
/// (release and dev builds alike — only the database is namespaced per build),
/// which is why name allocation has to consult it directly. `$FLETCH_WORKTREES_ROOT`
/// overrides it when set and non-empty (nested-Fletch Run redirect).
pub fn worktrees_root() -> Result<PathBuf> {
    if let Some(root) = std::env::var_os(WORKTREES_ROOT_ENV).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(root));
    }
    let home = dirs::home_dir()
        .ok_or_else(|| Error::Other("HOME directory not available".into()))?;
    Ok(home.join(".fletch").join("worktrees"))
}

/// The set of agent-id directories that physically exist under the worktrees
/// root. These are off-limits as new agent ids regardless of what any single
/// database knows: the directory is the resource `git worktree add` collides
/// on. Best-effort — a missing or unreadable root just yields an empty set.
pub fn occupied_worktree_dirs() -> HashSet<String> {
    match worktrees_root() {
        Ok(root) => occupied_worktree_dirs_in(&root),
        Err(_) => HashSet::new(),
    }
}

fn occupied_worktree_dirs_in(root: &Path) -> HashSet<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return HashSet::new();
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect()
}

/// Absolute path to the dir holding all of one agent's worktrees:
/// `~/.fletch/worktrees/<agent-id>/`.
pub fn agent_parent_dir(agent_id: &str) -> Result<PathBuf> {
    Ok(worktrees_root()?.join(agent_id))
}

/// Absolute path to one tracked repo's worktree:
/// `~/.fletch/worktrees/<agent-id>/<subdir>/`.
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
            base_sha: None,
            pr_number: None,
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
    fn sandbox_engine_stamp_round_trips() {
        let db = test_db();
        seed_repo(&db, "/r");
        seed_repo(&db, "/r2");
        let wm = WorkspaceManager::new(db);

        // A stamped engine persists verbatim and comes back on load — the
        // stickiness contract: spawn paths reuse this, never the live setting.
        let mut stamped = new_agent_record(
            "yosemite".into(),
            "a".into(),
            "claude".into(),
            mk_repo("/r"),
            "t".into(),
            AgentView::Custom,
        );
        stamped.sandbox_engine = Some("docker".into());
        wm.add_agent(&mut stamped).unwrap();
        assert_eq!(
            wm.agent("yosemite").unwrap().sandbox_engine.as_deref(),
            Some("docker")
        );

        // An unstamped record (pre-selection agents) stays NULL, which spawn
        // paths treat as sandbox-exec.
        let mut legacy = new_agent_record(
            "dolomites".into(),
            "b".into(),
            "claude".into(),
            mk_repo("/r2"),
            "t".into(),
            AgentView::Custom,
        );
        wm.add_agent(&mut legacy).unwrap();
        assert_eq!(wm.agent("dolomites").unwrap().sandbox_engine, None);
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
    fn rename_project_updates_display_name() {
        let db = test_db();
        let td = tempfile::tempdir().unwrap();
        let repo = init_repo(td.path());
        let wm = WorkspaceManager::new(db);
        wm.add_workspace_repo(repo.clone()).unwrap();

        let pid = wm.current().unwrap().projects[0].project_id.clone();
        let ws = wm.rename_project(&pid, "  My Project  ").unwrap();

        // Name is trimmed and decoupled from the folder path, which is untouched.
        assert_eq!(ws.projects[0].name, "My Project");
        assert_eq!(ws.projects[0].path, repo);
        assert!(ws.repos.contains(&repo));
    }

    #[test]
    fn rename_project_rejects_empty_name() {
        let db = test_db();
        let td = tempfile::tempdir().unwrap();
        let repo = init_repo(td.path());
        let wm = WorkspaceManager::new(db);
        wm.add_workspace_repo(repo).unwrap();
        let pid = wm.current().unwrap().projects[0].project_id.clone();

        let err = wm.rename_project(&pid, "   ").unwrap_err();
        assert!(err.to_string().contains("cannot be empty"));
    }

    #[test]
    fn relocate_repo_repoints_path() {
        let db = test_db();
        let td = tempfile::tempdir().unwrap();
        let old = init_repo(td.path());
        let new = td.path().join("moved");
        std::fs::create_dir_all(new.join(".git")).unwrap();

        let wm = WorkspaceManager::new(db);
        wm.add_workspace_repo(old.clone()).unwrap();
        let pid = wm.current().unwrap().projects[0].project_id.clone();

        let ws = wm.relocate_repo(&old, &new).unwrap();
        assert!(ws.repos.contains(&new));
        assert!(!ws.repos.contains(&old));
        // Same project — relocate keeps the id (and any per-project settings).
        assert_eq!(ws.projects[0].project_id, pid);
    }

    #[test]
    fn relocate_repo_rejects_non_git_dest() {
        let db = test_db();
        let td = tempfile::tempdir().unwrap();
        let old = init_repo(td.path());
        let wm = WorkspaceManager::new(db);
        wm.add_workspace_repo(old.clone()).unwrap();

        let err = wm.relocate_repo(&old, &td.path().join("nope")).unwrap_err();
        assert!(err.to_string().contains("not a git repository"));
    }

    #[test]
    fn relocate_repo_rejects_pinned_collision() {
        let db = test_db();
        let td = tempfile::tempdir().unwrap();
        let a = init_repo(td.path());
        let b = td.path().join("b");
        std::fs::create_dir_all(b.join(".git")).unwrap();

        let wm = WorkspaceManager::new(db);
        wm.add_workspace_repo(a.clone()).unwrap();
        wm.add_workspace_repo(b.clone()).unwrap();

        // Moving `a` onto `b`'s already-pinned path is refused, not silently merged.
        let err = wm.relocate_repo(&a, &b).unwrap_err();
        assert!(err.to_string().contains("already pinned"));
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
    fn custom_agent_instructions_and_id_round_trip() {
        let db = test_db();
        let wm = WorkspaceManager::new(db.clone());
        seed_repo(&db, "/r");

        let mut rec = new_agent_record(
            "shasta".into(),
            "a".into(),
            "claude".into(),
            mk_repo("/r"),
            "task".into(),
            AgentView::Custom,
        );
        let id = rec.id.clone();
        rec.instructions = Some("You are the Reviewer. Be terse.".into());
        rec.custom_agent_id = Some("ca-reviewer".into());
        wm.add_agent(&mut rec).unwrap();

        // Read back via load_agent (single-row path)…
        let loaded = wm.agent(&id).unwrap();
        assert_eq!(loaded.instructions.as_deref(), Some("You are the Reviewer. Be terse."));
        assert_eq!(loaded.custom_agent_id.as_deref(), Some("ca-reviewer"));

        // …and via the full list (query_all_agents path).
        let listed = wm
            .current()
            .unwrap()
            .agents
            .into_iter()
            .find(|a| a.id == id)
            .unwrap();
        assert_eq!(listed.custom_agent_id.as_deref(), Some("ca-reviewer"));

        // A plain built-in spawn leaves both columns null.
        let mut plain = new_agent_record(
            "tahoe".into(),
            "b".into(),
            "claude".into(),
            mk_repo("/r"),
            "task".into(),
            AgentView::Custom,
        );
        let plain_id = plain.id.clone();
        wm.add_agent(&mut plain).unwrap();
        let plain_loaded = wm.agent(&plain_id).unwrap();
        assert_eq!(plain_loaded.instructions, None);
        assert_eq!(plain_loaded.custom_agent_id, None);
    }

    #[test]
    fn pr_number_persists_and_resets_on_name_reuse() {
        let db = test_db();
        let wm = WorkspaceManager::new(db.clone());
        seed_repo(&db, "/r");

        // Spawn an agent named "denali" and record a PR number for it.
        let mut rec = new_agent_record(
            "denali".into(),
            "a".into(),
            "claude".into(),
            mk_repo("/r"),
            "task".into(),
            AgentView::Custom,
        );
        let id = rec.id.clone();
        wm.add_agent(&mut rec).unwrap();
        let subdir = wm.agent(&id).unwrap().repos[0].subdir.clone();

        // No PR until one is recorded.
        assert_eq!(wm.agent(&id).unwrap().repos[0].pr_number, None);
        wm.set_repo_pr_number(&id, &subdir, 42).unwrap();
        assert_eq!(wm.agent(&id).unwrap().repos[0].pr_number, Some(42));

        // Deleting the agent drops its worktree row. A future agent that reuses
        // the same name (and therefore the same branch) starts with no PR — so
        // it can't resolve to the deleted agent's now-merged PR. This is the
        // crux of binding PR identity to the worktree row, not the branch name.
        wm.remove_agent(&id).unwrap();
        let mut reused = new_agent_record(
            "denali".into(),
            "a".into(),
            "claude".into(),
            mk_repo("/r"),
            "task".into(),
            AgentView::Custom,
        );
        let reused_id = reused.id.clone();
        wm.add_agent(&mut reused).unwrap();
        assert_eq!(wm.agent(&reused_id).unwrap().repos[0].pr_number, None);
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
                branch_name: Some("feat/do-the-thing".into()),
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
            branch: Some("feat/do-the-thing".into()),
            parent_branch: Some("main".into()),
            base_sha: None,
            pr_number: None,
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
                base_sha: None,
                pr_number: None,
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
        // Timing is unset until the turn starts/ends.
        assert_eq!(turns[0].started_at, None);
        assert_eq!(turns[0].ended_at, None);
    }

    #[test]
    fn user_turn_timing_start_then_end() {
        let db = test_db();
        let (ws_id, wm) = make_workspace_with_session(&db);
        wm.insert_user_turn(&ws_id, "turn-1", "hello", &[]).unwrap();

        // Start stamps started_at; end stamps ended_at on the open turn.
        wm.mark_user_turn_started("turn-1", 1000).unwrap();
        let started = wm.read_user_turns(&ws_id).unwrap()[0].started_at;
        assert_eq!(started, Some(1000));
        assert_eq!(wm.read_user_turns(&ws_id).unwrap()[0].ended_at, None);

        let closed = wm.mark_user_turn_ended(&ws_id).unwrap().expect("open turn closed");
        let turn = wm.read_user_turns(&ws_id).unwrap().remove(0);
        assert_eq!(turn.started_at, started, "start clock not reset by end");
        assert!(turn.ended_at >= turn.started_at, "ended_at after started_at");
        assert_eq!(
            closed.duration_ms,
            turn.ended_at.unwrap() - turn.started_at.unwrap(),
            "duration_ms matches stored ended_at − started_at"
        );
        assert_eq!(closed.record_count, 0, "no records ingested in this test");
    }

    #[test]
    fn mark_user_turn_started_is_idempotent() {
        let db = test_db();
        let (ws_id, wm) = make_workspace_with_session(&db);
        wm.insert_user_turn(&ws_id, "turn-1", "hello", &[]).unwrap();

        wm.mark_user_turn_started("turn-1", 1000).unwrap();
        let first = wm.read_user_turns(&ws_id).unwrap()[0].started_at;
        // A delivery retry re-stamps — but the guard keeps the original clock.
        wm.mark_user_turn_started("turn-1", 2000).unwrap();
        assert_eq!(wm.read_user_turns(&ws_id).unwrap()[0].started_at, first);
    }

    #[test]
    fn mark_user_turn_ended_skips_turns_that_never_started() {
        let db = test_db();
        let (ws_id, wm) = make_workspace_with_session(&db);
        // A row with no started_at (e.g. a never-delivered turn, or the resting
        // Idle emitted at spawn) must not get an ended_at.
        wm.insert_user_turn(&ws_id, "turn-1", "hello", &[]).unwrap();
        assert!(
            wm.mark_user_turn_ended(&ws_id).unwrap().is_none(),
            "no open turn to close"
        );
        assert_eq!(wm.read_user_turns(&ws_id).unwrap()[0].ended_at, None);
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
    fn associate_matches_multiline_text() {
        let db = test_db();
        let (ws_id, wm) = make_workspace_with_session(&db);

        // Multi-line message — the transcript stores it JSON-escaped (\n → \\n).
        let text = "Please do this:\n- step one\n- step two";
        wm.insert_user_turn(&ws_id, "t1", text, &[]).unwrap();

        let rec_body = serde_json::json!({"role": "user", "text": text});
        wm.append_session_records(
            &ws_id,
            "claude",
            "transcript",
            None,
            &[("rec-1", &rec_body)],
        )
        .unwrap();

        let n = wm.associate_pending_user_turns(&ws_id).unwrap();
        assert_eq!(n, 1);
        let turns = wm.read_user_turns(&ws_id).unwrap();
        assert_eq!(turns[0].native_id.as_deref(), Some("rec-1"));
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

    // ── mid-turn follow-up messages (coalesced delivery + live injection) ──

    #[test]
    fn coalesced_follow_ups_persist_one_row_that_matches_one_record() {
        // Per-turn flush (A5-A): N queued follow-ups coalesce into ONE prompt,
        // delivered as one turn → one transcript record → one user_turn row
        // that matches 1:1. No orphans.
        let db = test_db();
        let (ws_id, wm) = make_workspace_with_session(&db);

        wm.insert_user_turn(&ws_id, "t-coalesced", "first\n\nsecond", &[])
            .unwrap();

        let rec = serde_json::json!({"role": "user", "text": "first\n\nsecond"});
        wm.append_session_records(&ws_id, "codex", "transcript", None, &[("rec-1", &rec)])
            .unwrap();

        assert_eq!(wm.associate_pending_user_turns(&ws_id).unwrap(), 1);
        let turns = wm.read_user_turns(&ws_id).unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].native_id.as_deref(), Some("rec-1"));
    }

    #[test]
    fn live_injected_follow_ups_each_match_their_own_record() {
        // Claude live: each injected message is its own transcript user record,
        // so two follow-ups inside one turn window match N→N (no coalescing).
        let db = test_db();
        let (ws_id, wm) = make_workspace_with_session(&db);

        wm.insert_user_turn(&ws_id, "t1", "original", &[]).unwrap();
        wm.insert_user_turn(&ws_id, "t2", "actually also do X", &[])
            .unwrap();

        let rec_a = serde_json::json!({"role": "user", "text": "original"});
        let rec_b = serde_json::json!({"role": "user", "text": "actually also do X"});
        wm.append_session_records(
            &ws_id,
            "claude",
            "transcript",
            None,
            &[("rec-A", &rec_a), ("rec-B", &rec_b)],
        )
        .unwrap();

        assert_eq!(wm.associate_pending_user_turns(&ws_id).unwrap(), 2);
        let turns = wm.read_user_turns(&ws_id).unwrap();
        assert_eq!(turns[0].native_id.as_deref(), Some("rec-A"));
        assert_eq!(turns[1].native_id.as_deref(), Some("rec-B"));
    }

    #[test]
    fn per_message_rows_orphan_against_a_coalesced_record() {
        // Guards the A5-A decision: if a coalesced delivery (one merged record)
        // were persisted as N separate rows instead of one, the claim-set lets
        // only the first row match and the rest orphan forever. This is the bug
        // we avoid by persisting a single coalesced row — documented here so a
        // future change back to per-message rows fails loudly.
        let db = test_db();
        let (ws_id, wm) = make_workspace_with_session(&db);

        wm.insert_user_turn(&ws_id, "t1", "first", &[]).unwrap();
        wm.insert_user_turn(&ws_id, "t2", "second", &[]).unwrap();

        let rec = serde_json::json!({"role": "user", "text": "first\n\nsecond"});
        wm.append_session_records(&ws_id, "codex", "transcript", None, &[("rec-1", &rec)])
            .unwrap();

        // Only one row can claim the single record; the other stays pending.
        assert_eq!(wm.associate_pending_user_turns(&ws_id).unwrap(), 1);
        let pending = wm
            .read_user_turns(&ws_id)
            .unwrap()
            .into_iter()
            .filter(|t| t.native_id.is_none())
            .count();
        assert_eq!(pending, 1, "the unclaimed row orphans — hence we coalesce to one row");
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

    #[test]
    fn occupied_worktree_dirs_lists_only_subdirs() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("kilimanjaro")).unwrap();
        std::fs::create_dir_all(root.path().join("seychelles")).unwrap();
        // A stray file (not a dir) must not be reported as an occupied name.
        std::fs::write(root.path().join("notes.txt"), b"x").unwrap();

        let found = occupied_worktree_dirs_in(root.path());
        assert_eq!(found.len(), 2);
        assert!(found.contains("kilimanjaro"));
        assert!(found.contains("seychelles"));
        assert!(!found.contains("notes.txt"));
    }

    #[test]
    fn occupied_worktree_dirs_empty_when_root_missing() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("does-not-exist");
        assert!(occupied_worktree_dirs_in(&missing).is_empty());
    }

    /// Mark a workspace archived directly (tests don't go through the full
    /// archive flow, which needs live worktrees on disk).
    fn mark_archived(db: &Arc<Mutex<Connection>>, id: &str) {
        let conn = db.lock();
        conn.execute(
            "UPDATE workspaces SET archived_at = ?1 WHERE id = ?2",
            rusqlite::params![now_millis(), id],
        )
        .unwrap();
    }

    #[test]
    fn add_agent_reuses_archived_name() {
        let db = test_db();
        seed_repo(&db, "/r");
        let wm = WorkspaceManager::new(db.clone());

        let mut first = new_agent_record(
            "kilimanjaro".into(),
            "first".into(),
            "claude".into(),
            mk_repo("/r"),
            String::new(),
            AgentView::Custom,
        );
        wm.add_agent(&mut first).unwrap();
        mark_archived(&db, "kilimanjaro");

        // Recycling the freed name must succeed — the archived row is evicted
        // rather than tripping the primary-key constraint.
        let mut second = new_agent_record(
            "kilimanjaro".into(),
            "second".into(),
            "claude".into(),
            mk_repo("/r"),
            String::new(),
            AgentView::Custom,
        );
        wm.add_agent(&mut second).unwrap();

        let conn = db.lock();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM workspaces WHERE id = 'kilimanjaro'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "exactly one row should remain");
        let (name, archived): (String, Option<i64>) = conn
            .query_row(
                "SELECT name, archived_at FROM workspaces WHERE id = 'kilimanjaro'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(name, "second", "the live agent should have replaced it");
        assert!(archived.is_none(), "the recycled agent must be live");
    }

    #[test]
    fn add_agent_does_not_evict_a_live_name_clash() {
        let db = test_db();
        seed_repo(&db, "/r");
        let wm = WorkspaceManager::new(db.clone());

        let mut first = new_agent_record(
            "kilimanjaro".into(),
            "first".into(),
            "claude".into(),
            mk_repo("/r"),
            String::new(),
            AgentView::Custom,
        );
        wm.add_agent(&mut first).unwrap();

        // A *live* id clash is a real bug: the INSERT must fail loudly rather
        // than clobber the running agent.
        let mut clash = new_agent_record(
            "kilimanjaro".into(),
            "second".into(),
            "claude".into(),
            mk_repo("/r"),
            String::new(),
            AgentView::Custom,
        );
        assert!(wm.add_agent(&mut clash).is_err());

        let conn = db.lock();
        let name: String = conn
            .query_row(
                "SELECT name FROM workspaces WHERE id = 'kilimanjaro'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(name, "first", "the original live agent must survive");
    }

    #[test]
    fn allocate_agent_id_excludes_archived_from_reservation() {
        let db = test_db();
        seed_repo(&db, "/r");
        let wm = WorkspaceManager::new(db.clone());

        // Fill the whole pool with archived agents, then one live agent.
        for place in names::PLACES {
            let mut rec = new_agent_record(
                (*place).into(),
                (*place).into(),
                "claude".into(),
                mk_repo("/r"),
                String::new(),
                AgentView::Custom,
            );
            wm.add_agent(&mut rec).unwrap();
            mark_archived(&db, place);
        }

        // Every pool name is archived (so all are reusable) — the allocator
        // should hand back a bare pool name, never a "-N" exhaustion suffix.
        let id = wm.allocate_agent_id().unwrap();
        assert!(
            names::PLACES.contains(&id.as_str()),
            "expected a reusable pool name, got {id}"
        );
    }
}
