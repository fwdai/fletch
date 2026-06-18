//! Tauri IPC command handlers — the thin frontend-facing surface.

use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, State};

use crate::agent::ProviderProbe;
use crate::error::{Error, Result};
use crate::gh::{self, GhRepoSummary, GhStatus, PrState};
use crate::git;
use crate::new_project;
use crate::git_state::{self, FileStatus, GitState, ShortStats, StatusKind};
use crate::names;
use crate::run_session::RunStateSnapshot;
use crate::supervisor::Supervisor;
use crate::workspace::{
    repo_worktree_path, AgentRecord, AgentView, DiffStats, TrackedRepo, Workspace,
};

#[tauri::command]
pub fn get_workspace(supervisor: State<'_, Arc<Supervisor>>) -> Option<Workspace> {
    supervisor.current_workspace()
}

#[tauri::command]
pub async fn get_agent_diff_stats(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<DiffStats> {
    let record = supervisor.workspace.agent(&agent_id)?;
    let mut stats = DiffStats::default();

    for repo in &record.repos {
        let worktree = repo_worktree_path(&agent_id, &repo.subdir)?;
        let base_ref = repo.parent_branch.as_deref().unwrap_or("HEAD");
        let diff = match git::worktree_diff_shortstat(&worktree, base_ref).await {
            Ok(diff) => diff,
            Err(err) if base_ref != "HEAD" => {
                tracing::warn!(
                    error = %err,
                    agent_id = %agent_id,
                    subdir = %repo.subdir,
                    base_ref = %base_ref,
                    "agent diff: falling back to HEAD"
                );
                git::worktree_diff_shortstat(&worktree, "HEAD").await?
            }
            Err(err) => return Err(err),
        };
        stats.additions = stats.additions.saturating_add(diff.0);
        stats.deletions = stats.deletions.saturating_add(diff.1);
    }

    Ok(stats)
}

/// Allocate a fresh name from the shared place pool for a draft agent.
/// Frontend passes the names already taken (real agents + other drafts) so
/// the picker avoids collisions.
#[tauri::command]
pub fn allocate_draft_name(used: Vec<String>) -> String {
    names::allocate(&used.into_iter().collect())
}

#[tauri::command]
pub fn add_workspace_repo(
    supervisor: State<'_, Arc<Supervisor>>,
    repo_path: String,
) -> Result<Workspace> {
    supervisor.add_workspace_repo(PathBuf::from(repo_path))
}

#[tauri::command]
pub fn remove_workspace_repo(
    supervisor: State<'_, Arc<Supervisor>>,
    repo_path: String,
) -> Result<Workspace> {
    supervisor.remove_workspace_repo(PathBuf::from(repo_path))
}

/// Whether the `gh` CLI is installed and authenticated — drives the New
/// Project flow's gating (clone and create both require `gh`).
#[tauri::command]
pub async fn gh_status() -> Result<GhStatus> {
    gh::auth_status().await
}

/// The authenticated user's GitHub repos, newest first, for the clone picker.
#[tauri::command]
pub async fn gh_repo_list() -> Result<Vec<GhRepoSummary>> {
    gh::repo_list(200).await
}

/// Clone a GitHub repo into `dest_parent/<repo-name>` and register it as a
/// workspace project.
#[tauri::command]
pub async fn clone_repo(
    supervisor: State<'_, Arc<Supervisor>>,
    spec: String,
    dest_parent: String,
) -> Result<Workspace> {
    let target = new_project::clone(&spec, Path::new(&dest_parent)).await?;
    supervisor.add_workspace_repo(target)
}

/// Create a fresh repo locally + on GitHub, then register it as a workspace
/// project.
#[tauri::command]
pub async fn create_repo(
    supervisor: State<'_, Arc<Supervisor>>,
    name: String,
    dest_parent: String,
    private: bool,
    description: Option<String>,
) -> Result<Workspace> {
    let target = new_project::create(
        &name,
        Path::new(&dest_parent),
        private,
        description.as_deref(),
    )
    .await?;
    supervisor.add_workspace_repo(target)
}

#[tauri::command]
pub async fn spawn_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    view: Option<AgentView>,
    repo_path: String,
    provider: Option<String>,
    name: Option<String>,
    effort: Option<String>,
) -> Result<AgentRecord> {
    let sup = supervisor.inner().clone();
    sup.spawn_agent(
        app,
        view.unwrap_or_default(),
        PathBuf::from(repo_path),
        provider.unwrap_or_else(|| "claude".to_string()),
        name,
        effort,
    )
    .await
}

#[tauri::command]
pub fn write_to_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    agent_id: String,
    data: String,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.write_to_agent(&app, &agent_id, data.as_bytes())
}

#[tauri::command]
pub fn send_user_message(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    agent_id: String,
    turn_id: String,
    text: String,
    attachments: Vec<String>,
    thinking: Option<String>,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.send_user_message(
        &app,
        &agent_id,
        &turn_id,
        &text,
        &attachments,
        thinking.as_deref(),
    )
}

#[tauri::command]
pub fn answer_tool_use(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    request_id: String,
    updated_input: serde_json::Value,
) -> Result<()> {
    supervisor
        .inner()
        .answer_tool_use(&agent_id, &request_id, updated_input)
}

#[tauri::command]
pub fn resize_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    cols: u16,
    rows: u16,
) -> Result<()> {
    supervisor.resize_agent(&agent_id, cols, rows)
}

#[tauri::command]
pub async fn resume_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    agent_id: String,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.resume_agent(app, &agent_id).await
}

#[tauri::command]
pub async fn switch_view(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    agent_id: String,
    view: AgentView,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.switch_view(app, &agent_id, view).await
}

#[tauri::command]
pub async fn stop_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    agent_id: String,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.stop_agent(app, &agent_id).await
}

#[tauri::command]
pub async fn discard_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.discard_agent(&agent_id).await
}

#[tauri::command]
pub async fn archive_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    agent_id: String,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.archive_agent(app, &agent_id).await
}

#[tauri::command]
pub async fn restore_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    agent_id: String,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.restore_agent(app, &agent_id).await
}

#[tauri::command]
pub fn read_session_records(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<Vec<crate::workspace::SessionRecord>> {
    supervisor.workspace.read_session_records(&agent_id)
}

#[tauri::command]
pub fn read_user_turns(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<Vec<crate::workspace::UserTurn>> {
    supervisor.workspace.read_user_turns(&agent_id)
}

/// Ingest the agent's on-disk transcript into session_records now (lazy
/// backfill when a session is opened with no records yet). Idempotent.
#[tauri::command]
pub fn sync_session(supervisor: State<'_, Arc<Supervisor>>, agent_id: String) -> Result<()> {
    supervisor.sync_session(&agent_id);
    Ok(())
}

#[tauri::command]
pub async fn add_repo_to_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    agent_id: String,
    repo_path: String,
) -> Result<TrackedRepo> {
    let sup = supervisor.inner().clone();
    sup.add_repo_to_agent(app, &agent_id, PathBuf::from(repo_path))
        .await
}

/// Push the primary repo's current branch to origin.
#[tauri::command]
pub async fn push_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    agent_id: String,
) -> Result<String> {
    let record = supervisor.workspace.agent(&agent_id)?;
    let repo = record.repos.first()
        .ok_or_else(|| crate::error::Error::Other("agent has no repos".into()))?;
    let worktree = repo_worktree_path(&agent_id, &repo.subdir)?;
    let branch = repo.branch.as_deref()
        .ok_or_else(|| crate::error::Error::Other("agent has no branch yet".into()))?
        .to_string();
    let summary = git::push(&worktree, &branch).await?;
    // After successful push, fetch PR state in background
    supervisor.inner().fetch_and_emit_pr_state(app, agent_id);
    Ok(summary)
}

/// Stage all working-tree changes and commit them with the given message.
#[tauri::command]
pub async fn commit_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    message: String,
) -> Result<()> {
    let record = supervisor.workspace.agent(&agent_id)?;
    let repo = record.repos.first()
        .ok_or_else(|| crate::error::Error::Other("agent has no repos".into()))?;
    let worktree = repo_worktree_path(&agent_id, &repo.subdir)?;
    git::commit(&worktree, &message).await
}

/// Discard every uncommitted change in the worktree (destructive).
#[tauri::command]
pub async fn discard_agent_changes(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<()> {
    let record = supervisor.workspace.agent(&agent_id)?;
    let repo = record.repos.first()
        .ok_or_else(|| crate::error::Error::Other("agent has no repos".into()))?;
    let worktree = repo_worktree_path(&agent_id, &repo.subdir)?;
    git::discard_all(&worktree).await
}

/// Stash all working-tree changes including untracked files.
#[tauri::command]
pub async fn stash_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<()> {
    let record = supervisor.workspace.agent(&agent_id)?;
    let repo = record.repos.first()
        .ok_or_else(|| crate::error::Error::Other("agent has no repos".into()))?;
    let worktree = repo_worktree_path(&agent_id, &repo.subdir)?;
    git::stash_push(&worktree).await
}

/// Abort an in-progress merge in the agent's worktree.
#[tauri::command]
pub async fn abort_merge_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<()> {
    let record = supervisor.workspace.agent(&agent_id)?;
    let repo = record.repos.first()
        .ok_or_else(|| crate::error::Error::Other("agent has no repos".into()))?;
    let worktree = repo_worktree_path(&agent_id, &repo.subdir)?;
    git::merge_abort(&worktree).await
}

/// List all local branches in a repo. Used by the new-agent composer to
/// let the user pick the base branch before spawning.
#[tauri::command]
pub async fn list_repo_branches(repo_path: String) -> Result<Vec<String>> {
    git::list_local_branches(Path::new(&repo_path)).await
}

/// Force-delete the agent's local branch from its parent repository.
/// Used by the merged-state UI to clean up after a PR lands. Safe-noops
/// if the branch is already gone (matches `git::branch_delete` semantics).
#[tauri::command]
pub async fn delete_branch_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<()> {
    let record = supervisor.workspace.agent(&agent_id)?;
    let repo = record.repos.first()
        .ok_or_else(|| crate::error::Error::Other("agent has no repos".into()))?;
    let branch = repo.branch.as_deref()
        .ok_or_else(|| crate::error::Error::Other("agent has no branch yet".into()))?;
    git::branch_delete(&repo.repo_path, branch).await
}

/// Pull latest into the primary repo's worktree.
#[tauri::command]
pub async fn pull_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<()> {
    let record = supervisor.workspace.agent(&agent_id)?;
    let repo = record.repos.first()
        .ok_or_else(|| crate::error::Error::Other("agent has no repos".into()))?;
    let worktree = repo_worktree_path(&agent_id, &repo.subdir)?;
    git::pull(&worktree).await
}

/// Rebase the agent's branch onto its parent (base) branch. Used by the
/// clean-state panel action to catch up when the base has advanced.
#[tauri::command]
pub async fn rebase_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<()> {
    let record = supervisor.workspace.agent(&agent_id)?;
    let repo = record.repos.first()
        .ok_or_else(|| crate::error::Error::Other("agent has no repos".into()))?;
    let worktree = repo_worktree_path(&agent_id, &repo.subdir)?;
    let base = repo.parent_branch.as_deref().unwrap_or("main");
    git::rebase_onto(&worktree, base).await
}

/// Create a PR for the agent's current branch.
/// Pass empty title/body to auto-fill from commits.
#[tauri::command]
pub async fn create_pr(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    title: String,
    body: String,
) -> Result<PrState> {
    let record = supervisor.workspace.agent(&agent_id)?;
    let repo = record.repos.first()
        .ok_or_else(|| crate::error::Error::Other("agent has no repos".into()))?;
    let worktree = repo_worktree_path(&agent_id, &repo.subdir)?;
    let base = repo.parent_branch.as_deref().unwrap_or("main");
    gh::pr_create(&worktree, &title, &body, base).await
}

/// Merge the open PR for the agent's current branch.
#[tauri::command]
pub async fn merge_pr(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<()> {
    let record = supervisor.workspace.agent(&agent_id)?;
    let repo = record.repos.first()
        .ok_or_else(|| crate::error::Error::Other("agent has no repos".into()))?;
    let worktree = repo_worktree_path(&agent_id, &repo.subdir)?;
    gh::pr_merge(&worktree).await
}

/// Fetch and return the current PR state for the agent's branch.
#[tauri::command]
pub async fn get_pr_state(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<Option<PrState>> {
    let record = supervisor.workspace.agent(&agent_id)?;
    let repo = record.repos.first()
        .ok_or_else(|| crate::error::Error::Other("agent has no repos".into()))?;
    // Only fetch if the agent has a branch
    if repo.branch.is_none() {
        return Ok(None);
    }
    let worktree = repo_worktree_path(&agent_id, &repo.subdir)?;
    gh::pr_view(&worktree).await
}

/// List the open PRs for the agent's repo, for the composer's "#" mention
/// autocomplete. Capped at 50 — the picker filters and shows a handful.
#[tauri::command]
pub async fn list_prs(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<Vec<gh::PrSummary>> {
    let record = supervisor.workspace.agent(&agent_id)?;
    let repo = record
        .repos
        .first()
        .ok_or_else(|| crate::error::Error::Other("agent has no repos".into()))?;
    let worktree = repo_worktree_path(&agent_id, &repo.subdir)?;
    gh::pr_list(&worktree, 50).await
}

/// Fetch the PR merge gate + per-check detail (spec §6). Best-effort: any
/// failure (no PR, gh missing, API error) returns `None` and the panel falls
/// back to `mergeable`-only behavior.
#[tauri::command]
pub async fn get_pr_checks(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<Option<gh::PrChecks>> {
    let record = match supervisor.workspace.agent(&agent_id) {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };
    let repo = match record.repos.first() {
        Some(r) => r,
        None => return Ok(None),
    };
    if repo.branch.is_none() {
        return Ok(None);
    }
    let worktree = repo_worktree_path(&agent_id, &repo.subdir)?;
    Ok(gh::pr_checks(&worktree).await.unwrap_or(None))
}

/// Open an interactive shell PTY in the agent's primary worktree.
/// Idempotent: if a shell is already running for this agent, does nothing.
#[tauri::command]
pub fn open_agent_shell(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    agent_id: String,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.open_agent_shell(app, &agent_id)
}

/// Kill the shell PTY for an agent.
/// Idempotent: if no shell is running, does nothing.
#[tauri::command]
pub fn close_agent_shell(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<()> {
    supervisor.close_agent_shell(&agent_id)
}

/// Write bytes to the agent's shell PTY stdin.
#[tauri::command]
pub fn write_to_shell(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    data: String,
) -> Result<()> {
    supervisor.write_to_shell(&agent_id, data.as_bytes())
}

/// Resize the agent's shell PTY.
#[tauri::command]
pub fn resize_shell(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    cols: u16,
    rows: u16,
) -> Result<()> {
    supervisor.resize_shell(&agent_id, cols, rows)
}

/// Start the Run-panel process for an agent.
/// Runs setup-then-run on first start, then run only on subsequent.
#[tauri::command]
pub fn run_start(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    agent_id: String,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.run_start(app, &agent_id)
}

/// Stop the Run-panel process for an agent. Idempotent.
#[tauri::command]
pub fn run_stop(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    agent_id: String,
) -> Result<()> {
    supervisor.run_stop(app, &agent_id)
}

/// Snapshot of the Run-panel state and accumulated log buffer for
/// rehydrating the panel on mount.
#[tauri::command]
pub fn run_state(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<RunStateSnapshot> {
    Ok(supervisor.run_state(&agent_id))
}

/// Detect the run configuration for an agent's primary repo, ranked by
/// confidence. The panel renders the first entry and layers persisted
/// overrides on top.
#[tauri::command]
pub fn detect_run_config(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<Vec<crate::run_detect::DetectedConfig>> {
    supervisor.detect_run_config(&agent_id)
}

/// Returns git state for the agent's primary repo.
/// For multi-repo agents only the first repo's state is returned.
#[tauri::command]
pub async fn get_git_state(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<Option<GitState>> {
    let record = match supervisor.workspace.agent(&agent_id) {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };
    let repo = match record.repos.first() {
        Some(r) => r,
        None => return Ok(None),
    };
    let worktree = repo_worktree_path(&agent_id, &repo.subdir)?;
    let parent = repo.parent_branch.as_deref().unwrap_or("main");
    let state = git_state::query(&worktree, parent).await?;
    Ok(Some(state))
}

/// Returns a compact shortstat (additions / deletions / file count) for
/// every live agent's primary repo, keyed by agent id. Used by the
/// app-wide background poll that powers per-agent shortstats in the
/// sidebar and the right-rail file-count badge. The focused panel calls
/// `get_git_state` separately for its own full state. Agents whose
/// state can't be queried (missing repo, archived, git error) are omitted.
///
/// Queries run in parallel so total latency is bounded by the slowest
/// agent's git invocation, not the sum. The reply contains only the
/// three numbers per agent — no file list — to keep the IPC payload
/// flat as the agent count grows.
#[tauri::command]
pub async fn get_all_shortstats(
    supervisor: State<'_, Arc<Supervisor>>,
) -> Result<std::collections::HashMap<String, ShortStats>> {
    let workspace = match supervisor.workspace.current() {
        Some(w) => w,
        None => return Ok(Default::default()),
    };
    let mut set = tokio::task::JoinSet::new();
    for agent in workspace.agents {
        if agent.archive.is_some() {
            continue;
        }
        let Some(repo) = agent.repos.first() else { continue };
        let Ok(worktree) = repo_worktree_path(&agent.id, &repo.subdir) else { continue };
        let parent = repo.parent_branch.clone().unwrap_or_else(|| "main".to_string());
        let agent_id = agent.id.clone();
        set.spawn(async move {
            let state = git_state::query(&worktree, &parent).await.ok()?;
            Some((
                agent_id,
                ShortStats {
                    additions: state.additions,
                    deletions: state.deletions,
                    file_count: state.files.len() as u32,
                },
            ))
        });
    }
    let mut out = std::collections::HashMap::new();
    while let Some(res) = set.join_next().await {
        if let Ok(Some((id, stats))) = res {
            out.insert(id, stats);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// File panel — browse the worktree, view & edit file contents.
// ---------------------------------------------------------------------------

/// Largest file the viewer will load. Bigger files report `too_large` so
/// the UI shows a "no preview" notice instead of choking the editor.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// One entry in an arbitrary directory listing (for the composer's `@`
/// file-mention autocomplete when the user types a filesystem path).
#[derive(Serialize)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
}

/// A directory listing plus the absolute path that was listed, so the
/// caller can build absolute attachment paths from entry names.
#[derive(Serialize)]
pub struct DirListing {
    /// Absolute, tilde-expanded directory that was read.
    pub base: String,
    pub entries: Vec<DirEntry>,
}

/// One entry in the worktree file list. Directories are derived on the
/// frontend from the path segments; only files are sent over IPC.
#[derive(Serialize)]
pub struct WorktreeFile {
    pub path: String,
    /// Git status vs the parent branch: "M" | "A" | "D" | "R" (None = clean).
    pub status: Option<String>,
    pub additions: u32,
    pub deletions: u32,
}

/// A single file's contents plus the metadata the editor needs.
#[derive(Serialize)]
pub struct WorktreeFileContents {
    pub text: String,
    /// File-extension hint (e.g. "ts", "rs", "py"); "" when unknown.
    pub lang: String,
    pub status: Option<String>,
    /// 1-indexed line numbers the agent added / modified (change gutter).
    pub chg_add: Vec<u32>,
    pub chg_mod: Vec<u32>,
    pub binary: bool,
    pub too_large: bool,
}

/// Collapse a rich git status into the single-letter code the panel renders.
/// Untracked reads as added; conflicted reads as modified.
fn status_code(kind: &StatusKind) -> &'static str {
    match kind {
        StatusKind::Modified | StatusKind::Conflicted => "M",
        StatusKind::Added | StatusKind::Untracked => "A",
        StatusKind::Deleted => "D",
        StatusKind::Renamed => "R",
    }
}

/// Map a path's extension to a language hint for the highlighter.
fn lang_for(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Join a caller-supplied relative path onto the worktree root, rejecting
/// anything that could escape it (absolute paths, `..`, drive prefixes).
fn safe_join(worktree: &Path, rel: &str) -> Result<PathBuf> {
    let p = Path::new(rel);
    let escapes = p.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    });
    if p.is_absolute() || escapes || rel.is_empty() {
        return Err(Error::InvalidPath(rel.to_string()));
    }
    Ok(worktree.join(p))
}

/// Resolve the agent's primary worktree and its parent ref.
fn primary_worktree(
    supervisor: &Supervisor,
    agent_id: &str,
) -> Result<(PathBuf, String)> {
    let record = supervisor.workspace.agent(agent_id)?;
    let repo = record
        .repos
        .first()
        .ok_or_else(|| Error::Other("agent has no repos".into()))?;
    let worktree = repo_worktree_path(agent_id, &repo.subdir)?;
    let parent = repo.parent_branch.clone().unwrap_or_else(|| "main".to_string());
    Ok((worktree, parent))
}

/// List the agent's worktree files (tracked + untracked), each tagged with
/// its git status vs the parent branch. This mirrors what's actually on disk
/// — like a regular file explorer — so files the agent deleted are dropped
/// rather than lingering as struck-through entries.
#[tauri::command]
pub async fn list_worktree_tree(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<Vec<WorktreeFile>> {
    let (worktree, parent) = primary_worktree(&supervisor, &agent_id)?;

    let state = git_state::query(&worktree, &parent).await.ok();
    let status_for = |path: &str| -> Option<&FileStatus> {
        state.as_ref()?.files.iter().find(|f| f.path == path)
    };

    let mut paths: BTreeSet<String> =
        git::list_files(&worktree).await.unwrap_or_default().into_iter().collect();
    if let Some(s) = &state {
        for f in &s.files {
            // A deleted file is gone from disk, so a file tree shouldn't show
            // it — and `ls-files --cached` still lists it (it's in the index),
            // so we must actively remove it. Everything else (untracked adds,
            // modifications) belongs in the tree.
            if matches!(f.kind, StatusKind::Deleted) {
                paths.remove(&f.path);
            } else {
                paths.insert(f.path.clone());
            }
        }
    }

    Ok(paths
        .into_iter()
        .map(|path| {
            let st = status_for(&path);
            WorktreeFile {
                status: st.map(|f| status_code(&f.kind).to_string()),
                additions: st.map(|f| f.additions).unwrap_or(0),
                deletions: st.map(|f| f.deletions).unwrap_or(0),
                path,
            }
        })
        .collect())
}

/// Expand a leading `~` (or `~/…`) to the user's home directory. Any other
/// path is returned unchanged. Used to resolve filesystem paths the user
/// types into the composer's `@` mention.
fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix('~') {
        if rest.is_empty() || rest.starts_with('/') {
            if let Some(home) = dirs::home_dir() {
                return home.join(rest.strip_prefix('/').unwrap_or(rest));
            }
        }
    }
    PathBuf::from(path)
}

/// List the entries of an arbitrary directory for the composer's `@`
/// mention autocomplete (e.g. `@~/Downloads/`). The path may start with
/// `~`; the resolved absolute directory comes back as `base` so the caller
/// can attach files by absolute path.
#[tauri::command]
pub async fn list_dir(path: String) -> Result<DirListing> {
    // Stop reading well above what the picker shows (the frontend filters and
    // caps display at 10) so a huge directory like /usr/lib or node_modules
    // can't stall the read or bloat the IPC payload. Hidden entries are kept
    // so typing a leading "." can still reveal dotfiles.
    const MAX_ENTRIES: usize = 1000;

    let dir = expand_tilde(&path);
    let read = std::fs::read_dir(&dir)
        .map_err(|e| Error::Other(format!("read_dir {}: {e}", dir.display())))?;

    let mut entries = Vec::new();
    for entry in read.flatten().take(MAX_ENTRIES) {
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        entries.push(DirEntry { name, is_dir });
    }

    Ok(DirListing {
        base: dir.to_string_lossy().to_string(),
        entries,
    })
}

/// Read a worktree file for the viewer/editor: contents, language hint,
/// git status, and the changed-line numbers driving the gutter.
#[tauri::command]
pub async fn read_worktree_file(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    path: String,
) -> Result<WorktreeFileContents> {
    let (worktree, parent) = primary_worktree(&supervisor, &agent_id)?;
    let abs = safe_join(&worktree, &path)?;
    let lang = lang_for(&path);

    let state = git_state::query(&worktree, &parent).await.ok();
    let status = state
        .as_ref()
        .and_then(|s| s.files.iter().find(|f| f.path == path))
        .map(|f| status_code(&f.kind).to_string());

    let empty = |text: String, binary: bool, too_large: bool| WorktreeFileContents {
        text,
        lang: lang.clone(),
        status: status.clone(),
        chg_add: vec![],
        chg_mod: vec![],
        binary,
        too_large,
    };

    // Deleted by the agent: the file is gone from disk, so show its prior
    // contents from the parent ref (the design lets you re-create it).
    if status.as_deref() == Some("D") {
        let text = git::show_file(&worktree, &parent, &path).await.unwrap_or_default();
        return Ok(empty(text, false, false));
    }

    if !abs.is_file() {
        return Ok(empty(String::new(), false, false));
    }
    if std::fs::metadata(&abs)?.len() > MAX_FILE_BYTES {
        return Ok(empty(String::new(), false, true));
    }
    let bytes = std::fs::read(&abs)?;
    if bytes.contains(&0) {
        return Ok(empty(String::new(), true, false));
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();

    let (chg_add, chg_mod) = if matches!(status.as_deref(), Some("M") | Some("R")) {
        git::file_changed_lines(&worktree, &parent, &path)
            .await
            .unwrap_or_default()
    } else {
        (vec![], vec![])
    };

    Ok(WorktreeFileContents {
        text,
        lang,
        status,
        chg_add,
        chg_mod,
        binary: false,
        too_large: false,
    })
}

/// Full unified diff of one worktree file versus the parent branch — the data
/// behind the Code panel's Live view. Returns "" when the file is unchanged.
#[tauri::command]
pub async fn get_file_diff(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    path: String,
) -> Result<String> {
    let (worktree, parent) = primary_worktree(&supervisor, &agent_id)?;
    git::file_diff(&worktree, &parent, &path).await
}

/// Overwrite a worktree file with new contents (the editor's Save / Revert).
#[tauri::command]
pub async fn write_worktree_file(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    path: String,
    contents: String,
) -> Result<()> {
    let (worktree, _parent) = primary_worktree(&supervisor, &agent_id)?;
    let abs = safe_join(&worktree, &path)?;
    if let Some(dir) = abs.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&abs, contents)?;
    Ok(())
}

/// Resolve a not-yet-existing destination inside the worktree: reject path
/// traversal, refuse to clobber an existing entry, and create its parent
/// directory. The create / rename / copy commands all share this so the
/// no-clobber + path-safety contract lives in exactly one place.
fn resolve_new_path(worktree: &Path, rel: &str) -> Result<PathBuf> {
    let abs = safe_join(worktree, rel)?;
    if abs.exists() {
        return Err(Error::Other(format!("\"{rel}\" already exists")));
    }
    if let Some(dir) = abs.parent() {
        std::fs::create_dir_all(dir)?;
    }
    Ok(abs)
}

/// Rename/move a worktree path (file or directory). Refuses to clobber an
/// existing destination so a rename can never silently overwrite a sibling.
#[tauri::command]
pub async fn rename_worktree_path(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    from: String,
    to: String,
) -> Result<()> {
    let (worktree, _parent) = primary_worktree(&supervisor, &agent_id)?;
    let src = safe_join(&worktree, &from)?;
    let dst = resolve_new_path(&worktree, &to)?;
    std::fs::rename(&src, &dst)?;
    Ok(())
}

/// Delete a worktree path. Files are removed directly; directories are
/// removed recursively (the UI guards this behind a confirm step). Deleting a
/// path that's already gone is a no-op, so concurrent deletes don't error.
#[tauri::command]
pub async fn delete_worktree_path(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    path: String,
) -> Result<()> {
    let (worktree, _parent) = primary_worktree(&supervisor, &agent_id)?;
    let abs = safe_join(&worktree, &path)?;
    if abs.is_dir() {
        std::fs::remove_dir_all(&abs)?;
    } else if abs.exists() {
        std::fs::remove_file(&abs)?;
    }
    Ok(())
}

/// Create a new empty file, making parent directories as needed. Refuses to
/// overwrite an existing path.
#[tauri::command]
pub async fn create_worktree_file(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    path: String,
) -> Result<()> {
    let (worktree, _parent) = primary_worktree(&supervisor, &agent_id)?;
    let abs = resolve_new_path(&worktree, &path)?;
    std::fs::write(&abs, "")?;
    Ok(())
}

/// Create a new directory. Refuses to clobber an existing path.
#[tauri::command]
pub async fn create_worktree_dir(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    path: String,
) -> Result<()> {
    let (worktree, _parent) = primary_worktree(&supervisor, &agent_id)?;
    let abs = resolve_new_path(&worktree, &path)?;
    std::fs::create_dir_all(&abs)?;
    Ok(())
}

/// Copy a worktree file to a new path (the explorer's "Duplicate"). Refuses
/// to overwrite an existing destination.
#[tauri::command]
pub async fn copy_worktree_file(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    from: String,
    to: String,
) -> Result<()> {
    let (worktree, _parent) = primary_worktree(&supervisor, &agent_id)?;
    let src = safe_join(&worktree, &from)?;
    let dst = resolve_new_path(&worktree, &to)?;
    std::fs::copy(&src, &dst)?;
    Ok(())
}

/// Probe every known provider's CLI binary: resolve its path, run `--version`,
/// and return what was found. Missing or uninstalled providers return `None`
/// for both fields; the frontend falls back to hardcoded defaults.
#[tauri::command]
pub async fn probe_provider_versions() -> Vec<ProviderProbe> {
    crate::agent::probe_all_providers().await
}

#[cfg(test)]
mod safe_join_tests {
    use super::safe_join;
    use std::path::Path;

    #[test]
    fn accepts_nested_relative_path() {
        let wt = Path::new("/tmp/wt");
        assert_eq!(
            safe_join(wt, "src/server/checkout.ts").unwrap(),
            wt.join("src/server/checkout.ts")
        );
    }

    #[test]
    fn rejects_parent_traversal() {
        let wt = Path::new("/tmp/wt");
        assert!(safe_join(wt, "../secrets").is_err());
        assert!(safe_join(wt, "src/../../etc/passwd").is_err());
    }

    #[test]
    fn rejects_absolute_and_empty() {
        let wt = Path::new("/tmp/wt");
        assert!(safe_join(wt, "/etc/passwd").is_err());
        assert!(safe_join(wt, "").is_err());
    }
}
