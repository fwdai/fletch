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

// Domain submodules. Each contributes an `impl WorkspaceManager` block; the
// re-exports below keep every `crate::workspace::X` path stable.
mod agents;
mod factory;
mod message_queue;
mod paths;
mod query;
mod repos;
mod sessions;
#[cfg(test)]
mod tests;
mod turns;

pub use factory::{is_per_turn_provider, new_agent_record};
pub use paths::{
    agent_parent_dir, allocate_repo_subdir, migrate_default_checkouts_root, projects_root,
    repo_checkout_path, tools_root, WORKSPACES_ROOT_ENV,
};

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
#[derive(Default)]
pub enum AgentView {
    /// Structured chat UI rendered from claude's stream-json events.
    #[default]
    Custom,
    /// Read-only xterm showing claude's native TUI, with our input box
    /// overlaid on top of the claude input prompt.
    Native,
}

/// One repo an agent has a checkout in.
///
/// At spawn time every agent gets `repos[0]` populated from the
/// repo the user spawned it against. The user can extend this list
/// mid-session via `add_repo_to_agent`, which creates a sibling
/// checkout under the same parent dir.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedRepo {
    pub repo_path: PathBuf,
    pub subdir: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub parent_branch: Option<String>,
    /// The immutable fork-point commit this checkout was created from,
    /// captured at spawn after a best-effort fetch. Used as the diff base so
    /// agent changes are measured against the exact starting commit rather
    /// than a branch name that may drift. `None` for pre-migration agents and
    /// when the fork SHA couldn't be resolved; readers fall back to
    /// `parent_branch`. Distinct from `parent_branch`, which names the branch
    /// for PR/merge bases.
    #[serde(default)]
    pub base_sha: Option<String>,
    /// The GitHub PR number this checkout's branch was opened as, once known.
    /// Set when a PR is created through the app or adopted from an OPEN
    /// out-of-band PR. PR state is fetched by this number, not by branch name,
    /// so a recycled workspace name can't resolve to a prior agent's PR.
    /// `None` until a PR exists.
    #[serde(default)]
    pub pr_number: Option<i64>,
    /// Last-known snapshot of the bound PR (url / title / open|merged|closed),
    /// stamped by every successful PR fetch. This is what the UI falls back to
    /// when GitHub can't be reached or the checkout is broken — database truth
    /// outlives git state. `None` until a fetch has succeeded.
    #[serde(default)]
    pub pr_url: Option<String>,
    #[serde(default)]
    pub pr_title: Option<String>,
    #[serde(default)]
    pub pr_state: Option<String>,
    /// The repo's display label within its project ("Frontend", "Gateway"),
    /// denormalized from `repos.label` when the record is read from the DB
    /// (constructors set `None`; `query_tracked_repos` fills it). `None` falls
    /// back to the folder basename in the UI and in agent-facing notes.
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiffStats {
    #[serde(default)]
    pub additions: u32,
    #[serde(default)]
    pub deletions: u32,
}

/// Snapshot of one tracked repo at archive time. Captures enough to
/// recreate the checkout and branch on restore.
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
    /// The repos this agent has checkouts in. Always non-empty;
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
    /// Prior-conversation digest injected into a forked agent's brief, kept in
    /// its own field so it is never co-mingled with (and heuristically parsed
    /// out of) the user brief above. Composed after `instructions` on every
    /// spawn. `None` for a non-fork session. A fork rebuilds this fresh from the
    /// parent's records and never inherits the parent's value.
    #[serde(default)]
    pub forked_context: Option<String>,
    /// The custom agent this session was spawned from, used to show its
    /// name/color in the sidebar. `None` for a plain built-in spawn.
    #[serde(default)]
    pub custom_agent_id: Option<String>,
    /// Skills snapshotted at spawn (by value, like `instructions`): materialized
    /// as files under the agent's writable root on every process spawn, with an
    /// index appended to the injected instructions. Empty for plain spawns.
    #[serde(default)]
    pub skills: Vec<crate::agent_profile::SkillSnapshot>,
    /// MCP servers snapshotted at spawn (by value): regenerated into provider
    /// config (claude `--mcp-config`, codex `-c mcp_servers.*`) on every process
    /// spawn. Empty for plain spawns.
    #[serde(default)]
    pub mcp_servers: Vec<crate::agent_profile::McpServerSnapshot>,
    /// Sandbox engine stamped at creation (an `EngineKind::as_setting`
    /// spelling) and reused on every process spawn, so a settings change never
    /// re-engines an existing agent. `None` = created before engine selection
    /// existed — such agents always ran (and keep running) under sandbox-exec.
    #[serde(default)]
    pub sandbox_engine: Option<String>,
    /// The workflow run that owns this agent, when it was spawned as a
    /// workflow step (see `workflow::scheduler`). Run-owned agents are hidden
    /// from the normal sidebar (they render under their run) and are cleaned up
    /// by `wf_delete_run` rather than by DB cascade. `None` for a normal,
    /// user-spawned agent.
    #[serde(default)]
    pub owner_run_id: Option<String>,
    /// The GitHub issue this workspace was started from (bare issue number as
    /// text), captured when the user hits "Start work" on a Home-inbox issue.
    /// `None` for a normal spawn. Drives the `Closes #<n>` trailer the agent's
    /// PR carries so merging it closes the originating issue.
    #[serde(default)]
    pub issue_ref: Option<String>,
    pub created_at: String,
    #[serde(default)]
    pub last_error: Option<String>,
    /// Some when the agent has been archived. Live agents have None.
    /// Archived agents have no checkout, no branch, and no live process —
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

#[derive(Debug, Clone, Serialize)]
pub struct ProjectDeleteResult {
    pub workspace: Workspace,
    pub deleted_agent_ids: Vec<String>,
    pub deleted_run_ids: Vec<String>,
}

/// A pinned repo joined with its owning project, so the frontend can show a
/// custom name (independent of the folder basename) and address the project
/// for rename / relocate without a second round-trip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRef {
    pub path: PathBuf,
    pub name: String,
    pub project_id: String,
    /// Per-repo display label ("Frontend", "Gateway"); `None` falls back to
    /// the folder basename. Distinct from `name`, which labels the project.
    pub label: Option<String>,
}

/// What the DB phase of an attach changed, so the command layer can undo it
/// precisely if the follow-up filesystem step (`ensure_git_repo`) fails. The
/// filesystem is only mutated after the DB phase commits — see
/// `commands::attach_repo_to_project`.
#[derive(Debug, Clone)]
pub enum AttachOutcome {
    /// The path already belonged to the target project — nothing changed.
    AlreadyAttached,
    /// A fresh repos row was inserted.
    Inserted { repo_id: String },
    /// An existing pinned repo was moved out of an empty source project.
    Moved {
        repo_id: String,
        source_project_id: String,
        /// Set when the emptied source project row was dropped; kept so undo
        /// can re-create it verbatim.
        dropped_source: Option<DroppedProject>,
    },
}

/// Fields of a project row dropped during an attach-move, for undo.
#[derive(Debug, Clone)]
pub struct DroppedProject {
    pub name: String,
    pub created_at: i64,
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
            s.effort, s.model, s.instructions, s.forked_context, s.custom_agent_id,
            s.skills, s.mcp_servers,
            w.sandbox_engine, w.owner_run_id, w.issue_ref
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
    Option<String>, // s.forked_context
    Option<String>, // s.custom_agent_id
    Option<String>, // s.skills (JSON array of SkillSnapshot)
    Option<String>, // s.mcp_servers (JSON array of McpServerSnapshot)
    Option<String>, // w.sandbox_engine
    Option<String>, // w.owner_run_id
    Option<String>, // w.issue_ref
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

    /// A workflow run's step agents (by `owner_run_id`), including archived
    /// ones — the run monitor's source for per-attempt chat records, which the
    /// sidebar snapshot deliberately omits.
    pub fn agents_for_run(&self, run_id: &str) -> Vec<AgentRecord> {
        let conn = self.db.lock();
        Self::query_agents_for_run(&conn, run_id)
    }

    /// Every agent owned by a project, including workflow-owned and archived
    /// agents that are omitted from the normal sidebar snapshot.
    pub fn agents_for_project(&self, project_id: &str) -> Vec<AgentRecord> {
        let conn = self.db.lock();
        Self::query_agents_for_project(&conn, project_id)
    }
}

/// Decode a nullable JSON-array session column into a Vec, treating NULL,
/// blank, or malformed JSON as empty — a snapshot that can't be read shouldn't
/// keep the whole workspace from loading.
fn decode_json_vec<T: serde::de::DeserializeOwned>(json: Option<&str>) -> Vec<T> {
    json.filter(|s| !s.trim().is_empty())
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default()
}

/// Encode a snapshot Vec for its session column: `None` (SQL NULL) when empty,
/// so plain built-in spawns keep NULL columns like they do for `instructions`.
fn encode_json_vec<T: serde::Serialize>(items: &[T]) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    serde_json::to_string(items).ok()
}
