//! Tauri IPC command handlers — the thin frontend-facing surface.

use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, State};

use crate::agent::{BinValidation, ProviderProbe, ToolStatus};
use crate::error::{Error, Result};
use crate::git;
use crate::git_state::{self, FileStatus, GitState, ShortStats, StatusKind};
use crate::github::{self as gh, GhRepoSummary, GhStatus, PrState};
use crate::managed_session::ToolUseBehavior;
use crate::names;
use crate::new_project;
use crate::run_session::RunStateSnapshot;
use crate::supervisor::{SpawnRequest, Supervisor};
use crate::workspace::{
    repo_checkout_path, AgentRecord, AgentView, DiffStats, TrackedRepo, Workspace,
};

#[tauri::command]
pub fn get_workspace(supervisor: State<'_, Arc<Supervisor>>) -> Option<Workspace> {
    supervisor.current_workspace()
}

/// Reveal Fletch's log folder in the OS file manager so a user can attach
/// logs to a bug report. Creates the folder if no session has written to it
/// yet. Fletch ships macOS-only (sandbox-exec), but the CI build runs on
/// Linux, so the opener binary is chosen per-platform rather than hard-coding
/// `open`.
#[tauri::command]
pub fn reveal_logs() -> Result<()> {
    let dir = crate::logs_dir();
    std::fs::create_dir_all(&dir).map_err(|e| Error::Other(format!("create log dir: {e}")))?;
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(&dir)
        .spawn()
        .map_err(|e| Error::Other(format!("open log dir: {e}")))?;
    Ok(())
}

/// Launch Docker Desktop — the "Start Docker Desktop" action on a docker
/// agent's daemon-down error state. macOS-only, like the rest of the sandbox
/// feature (`open -a Docker`); other platforms error so the UI can
/// report it rather than silently no-op. The daemon then takes a few seconds to
/// answer, which the settings pane's probe-retry loop already covers.
#[tauri::command]
pub fn start_docker_desktop() -> Result<()> {
    if cfg!(target_os = "macos") {
        std::process::Command::new("open")
            .args(["-a", "Docker"])
            .spawn()
            .map_err(|e| Error::Other(format!("open Docker Desktop: {e}")))?;
        Ok(())
    } else {
        Err(Error::Other(
            "Starting Docker Desktop from Fletch is only supported on macOS.".into(),
        ))
    }
}

/// The code editors installed on this machine, for the title-bar
/// "Open in editor" launcher. Detected live (see `editors::detect`).
#[tauri::command]
pub fn detect_editors() -> Vec<crate::editors::DetectedEditor> {
    crate::editors::detect()
}

/// Open an agent's primary checkout in the chosen editor.
#[tauri::command]
pub fn open_in_editor(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    editor_id: String,
) -> Result<()> {
    let (_, checkout) = primary_repo_checkout(&supervisor, &agent_id)?;
    crate::editors::open(&editor_id, &checkout)
}

/// The ref a checkout's *committed* changes are diffed against: the immutable
/// fork-point SHA captured at spawn when known, else the parent branch name
/// (pre-migration agents), which may have drifted from the actual fork point.
/// PR/merge/rebase bases and ahead/behind use `parent_branch` directly instead,
/// since those need a live branch name, not a commit.
fn diff_base(repo: &TrackedRepo) -> Option<String> {
    repo.base_sha.clone().or_else(|| repo.parent_branch.clone())
}

#[tauri::command]
pub async fn get_agent_diff_stats(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<DiffStats> {
    let record = supervisor.workspace.agent(&agent_id)?;
    let mut stats = DiffStats::default();

    for repo in &record.repos {
        let checkout = repo_checkout_path(&agent_id, &repo.subdir)?;
        let base = diff_base(repo);
        let base_ref = base.as_deref().unwrap_or("HEAD");
        let diff = match git::checkout_diff_shortstat(&checkout, base_ref).await {
            Ok(diff) => diff,
            Err(err) if base_ref != "HEAD" => {
                tracing::warn!(
                    error = %err,
                    agent_id = %agent_id,
                    subdir = %repo.subdir,
                    base_ref = %base_ref,
                    "agent diff: falling back to HEAD"
                );
                git::checkout_diff_shortstat(&checkout, "HEAD").await?
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
    // Fold in the names already on disk (other instances, stale checkouts) so a
    // draft never previews a name that `git worktree add` would later reject.
    let mut reserved: std::collections::HashSet<String> = used.into_iter().collect();
    reserved.extend(crate::workspace::occupied_checkout_dirs());
    names::allocate(&reserved)
}

/// Pin a folder as a workspace project. A folder that isn't a git repository
/// yet is initialized (with an initial commit) first, so users who've never
/// heard of git can still point the app at any project folder and get working
/// agents, checkouts, and history.
#[tauri::command]
pub async fn add_workspace_repo(
    supervisor: State<'_, Arc<Supervisor>>,
    repo_path: String,
) -> Result<Workspace> {
    let sup = supervisor.inner().clone();
    let path = PathBuf::from(repo_path);
    new_project::ensure_git_repo(&path).await?;
    sup.add_workspace_repo(path)
}

#[tauri::command]
pub fn remove_workspace_repo(
    supervisor: State<'_, Arc<Supervisor>>,
    repo_path: String,
) -> Result<Workspace> {
    supervisor.remove_workspace_repo(PathBuf::from(repo_path))
}

/// Rename a project — a custom display label independent of its folder name.
/// The sidebar and Project Settings header show this instead of the basename.
#[tauri::command]
pub fn rename_project(
    supervisor: State<'_, Arc<Supervisor>>,
    project_id: String,
    name: String,
) -> Result<Workspace> {
    supervisor.rename_project(&project_id, &name)
}

/// Repoint a pinned repo at a folder the user has moved on disk. Validates the
/// destination is a git repo; existing agents' worktrees are not relinked.
#[tauri::command]
pub fn relocate_repo(
    supervisor: State<'_, Arc<Supervisor>>,
    old_path: String,
    new_path: String,
) -> Result<Workspace> {
    supervisor.relocate_repo(PathBuf::from(old_path), PathBuf::from(new_path))
}

/// Whether the app has a working GitHub connection — drives the New Project
/// flow's gating (clone and create both need the API).
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
    publish: Option<bool>,
) -> Result<Workspace> {
    let target = new_project::create(
        &name,
        Path::new(&dest_parent),
        private,
        description.as_deref(),
        // Default true: an older frontend that doesn't pass the flag keeps
        // the original create-and-publish behavior.
        publish.unwrap_or(true),
    )
    .await?;
    supervisor.add_workspace_repo(target)
}

/// Publish a local-only project to GitHub: create the remote repo from the
/// project's *root* (so its default branch — e.g. `main` — becomes the GitHub
/// default, not the agent's working branch), wire `origin`, and push. The
/// checkout shares the new remote, so the agent can push its branch afterward.
/// The repo name is the project directory's basename. Returns the web URL.
#[tauri::command]
pub async fn publish_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    private: bool,
) -> Result<String> {
    let repo = primary_repo(&supervisor, &agent_id)?;
    let name = repo
        .repo_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| Error::InvalidPath("project folder has no name".into()))?
        .to_string();
    new_project::validate_new_name(&name)?;
    gh::repo_create_and_push(&repo.repo_path, &name, private, None).await
}

/// Drop the stored GitHub token — the app returns to local-only mode.
#[tauri::command]
pub fn github_disconnect(
    db: State<'_, Arc<parking_lot::Mutex<rusqlite::Connection>>>,
) -> Result<()> {
    crate::secrets::delete(&db.lock(), gh::TOKEN_SETTING)?;
    gh::set_token(None);
    Ok(())
}

// Args mirror the frontend `invoke("spawn_agent", ...)` payload one-to-one;
// they're the IPC wire surface, not collapsible into a struct here.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn spawn_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    view: Option<AgentView>,
    repo_path: String,
    provider: Option<String>,
    name: Option<String>,
    effort: Option<String>,
    model: Option<String>,
    instructions: Option<String>,
    custom_agent_id: Option<String>,
    skills: Option<Vec<crate::agent_profile::SkillSnapshot>>,
    mcp_servers: Option<Vec<crate::agent_profile::McpServerSnapshot>>,
    fork_base: Option<String>,
) -> Result<AgentRecord> {
    let sup = supervisor.inner().clone();
    sup.spawn_agent(
        app,
        SpawnRequest {
            view: view.unwrap_or_default(),
            repo_path: PathBuf::from(repo_path),
            provider: provider.unwrap_or_else(|| "claude".to_string()),
            name,
            effort,
            model,
            instructions,
            custom_agent_id,
            skills: skills.unwrap_or_default(),
            mcp_servers: mcp_servers.unwrap_or_default(),
            fork_base,
            // User-initiated spawns are never run-owned; the workflow scheduler
            // sets this when it spawns a step agent.
            owner_run_id: None,
        },
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

/// Returns `true` when the follow-up was enqueued for a later turn boundary
/// rather than delivered now (see `Supervisor::send_user_message`).
#[tauri::command]
pub fn send_user_message(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    agent_id: String,
    turn_id: String,
    text: String,
    attachments: Vec<String>,
    thinking: Option<String>,
) -> Result<bool> {
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
    behavior: ToolUseBehavior,
    message: Option<String>,
) -> Result<()> {
    supervisor
        .inner()
        .answer_tool_use(&agent_id, &request_id, updated_input, behavior, message)
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
pub async fn discard_agent(supervisor: State<'_, Arc<Supervisor>>, agent_id: String) -> Result<()> {
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

/// Persist a runtime-compiled record (`source = 'live_compiled'`) the frontend
/// holds but the on-disk transcript lacks — currently cursor's per-turn token
/// usage, which it emits only on its live `result` event. Idempotent on
/// `native_id` (use the event's `request_id`), so re-sending a turn is a no-op.
/// Returns whether a new row was inserted.
#[tauri::command]
pub fn append_live_record(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    provider: String,
    native_id: String,
    body: serde_json::Value,
) -> Result<bool> {
    let inserted = supervisor.workspace.append_session_records(
        &agent_id,
        &provider,
        "live_compiled",
        None,
        &[(native_id.as_str(), &body)],
    )?;
    Ok(inserted > 0)
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
    let (repo, checkout) = primary_repo_checkout(&supervisor, &agent_id)?;
    let branch = repo_branch(&repo)?.to_string();
    let summary = git::push(&checkout, &branch).await?;
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
    let (_repo, checkout) = primary_repo_checkout(&supervisor, &agent_id)?;
    git::commit(&checkout, &message).await
}

/// Discard every uncommitted change in the checkout (destructive).
#[tauri::command]
pub async fn discard_agent_changes(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<()> {
    let (_repo, checkout) = primary_repo_checkout(&supervisor, &agent_id)?;
    git::discard_all(&checkout).await
}

/// Stash all working-tree changes including untracked files.
#[tauri::command]
pub async fn stash_agent(supervisor: State<'_, Arc<Supervisor>>, agent_id: String) -> Result<()> {
    let (_repo, checkout) = primary_repo_checkout(&supervisor, &agent_id)?;
    git::stash_push(&checkout).await
}

/// Abort an in-progress merge in the agent's checkout.
#[tauri::command]
pub async fn abort_merge_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<()> {
    let (_repo, checkout) = primary_repo_checkout(&supervisor, &agent_id)?;
    git::merge_abort(&checkout).await
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
    let repo = primary_repo(&supervisor, &agent_id)?;
    let branch = repo_branch(&repo)?;
    git::branch_delete(&repo.repo_path, branch).await
}

/// Pull latest into the primary repo's checkout.
#[tauri::command]
pub async fn pull_agent(supervisor: State<'_, Arc<Supervisor>>, agent_id: String) -> Result<()> {
    let (_repo, checkout) = primary_repo_checkout(&supervisor, &agent_id)?;
    git::pull(&checkout).await
}

/// Rebase the agent's branch onto its parent (base) branch. Used by the
/// clean-state panel action to catch up when the base has advanced.
#[tauri::command]
pub async fn rebase_agent(supervisor: State<'_, Arc<Supervisor>>, agent_id: String) -> Result<()> {
    let (repo, checkout) = primary_repo_checkout(&supervisor, &agent_id)?;
    let base = repo.parent_branch.as_deref().unwrap_or("main");
    git::rebase_onto(&checkout, base).await
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
    let (repo, checkout) = primary_repo_checkout(&supervisor, &agent_id)?;
    let base = repo.parent_branch.as_deref().unwrap_or("main");
    let pr = gh::pr_create(&checkout, &title, &body, base).await?;
    crate::telemetry::track("pr_opened", serde_json::json!({ "source": "manual" }));
    // Bind the PR to this agent (number + state snapshot) so later lookups
    // don't rely on the (recyclable) branch name. A failure here isn't fatal —
    // the next idle/push poll re-binds it via guarded discovery once the PR
    // shows OPEN — but the helper logs it so the gap is observable, not silent.
    crate::supervisor::persist_pr_snapshot(&supervisor.workspace, &agent_id, &repo.subdir, &pr);
    Ok(pr)
}

/// Merge the open PR for the agent's current branch.
#[tauri::command]
pub async fn merge_pr(supervisor: State<'_, Arc<Supervisor>>, agent_id: String) -> Result<()> {
    let (_repo, checkout) = primary_repo_checkout(&supervisor, &agent_id)?;
    gh::pr_merge(&checkout).await
}

/// Fetch and return the current PR state for the agent's primary repo: by
/// bound number when one is recorded (with the persisted snapshot as the
/// fallback when GitHub is unreachable), else discovered by branch. Unbound
/// merged/closed PRs on a recycled branch are included here for panel
/// display, though the app-wide paths never claim them as the agent's.
#[tauri::command]
pub async fn get_pr_state(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<Option<PrState>> {
    Ok(
        crate::supervisor::resolve_pr_state(&supervisor.workspace, &agent_id)
            .await
            .map(|(pr, _bound)| pr),
    )
}

/// List the open PRs for the agent's repo, for the composer's "#" mention
/// autocomplete. Capped at 50 — the picker filters and shows a handful.
#[tauri::command]
pub async fn list_prs(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<Vec<gh::PrSummary>> {
    let (_repo, checkout) = primary_repo_checkout(&supervisor, &agent_id)?;
    gh::pr_list(&checkout, 50).await
}

/// Fetch the PR merge gate + per-check detail (spec §6). Best-effort: any
/// failure (no PR, gh missing, API error) returns `None` and the panel falls
/// back to `mergeable`-only behavior.
#[tauri::command]
pub async fn get_pr_checks(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<Option<gh::PrChecks>> {
    let Some((repo, checkout)) = primary_repo_checkout_opt(&supervisor, &agent_id)? else {
        return Ok(None);
    };
    if repo.branch.is_none() {
        return Ok(None);
    }
    Ok(gh::pr_checks(&checkout).await.unwrap_or(None))
}

/// Fetch the unresolved PR review threads (Greptile / other bots / humans),
/// flattened to each thread's root comment. Best-effort: any failure (no PR,
/// gh missing, API error) returns `None` and the panel omits the section.
#[tauri::command]
pub async fn get_pr_comments(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<Option<gh::PrComments>> {
    let Some((repo, checkout)) = primary_repo_checkout_opt(&supervisor, &agent_id)? else {
        return Ok(None);
    };
    if repo.branch.is_none() {
        return Ok(None);
    }
    Ok(gh::pr_comments(&checkout).await.unwrap_or(None))
}

/// Open an interactive shell PTY in the agent's primary checkout.
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
pub fn close_agent_shell(supervisor: State<'_, Arc<Supervisor>>, agent_id: String) -> Result<()> {
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

/// Detect the run configuration for a project by repo path (as the sidebar
/// keys its groups), bundled with the resolved project_id. Powers the
/// Project Settings surface, which can open for a repo that has no live agent.
#[tauri::command]
pub fn project_run_config(
    supervisor: State<'_, Arc<Supervisor>>,
    repo_path: String,
) -> Result<crate::supervisor::ProjectRunConfig> {
    supervisor.project_run_config(&repo_path)
}

/// Returns git state for the agent's primary repo.
/// For multi-repo agents only the first repo's state is returned.
#[tauri::command]
pub async fn get_git_state(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<Option<GitState>> {
    let Some((repo, checkout)) = primary_repo_checkout_opt(&supervisor, &agent_id)? else {
        return Ok(None);
    };
    let parent = repo.parent_branch.as_deref().unwrap_or("main");
    let state = git_state::query(&checkout, parent).await?;
    Ok(Some(state))
}

/// Returns a compact shortstat (additions / deletions / file count) for
/// every live agent's primary repo, keyed by agent id. Used by the
/// app-wide background poll that powers per-agent shortstats in the
/// sidebar and the right-rail file-count badge. The focused panel calls
/// `get_git_state` separately for its own full state. Archived agents and
/// agents with no resolvable repo are omitted; a git error degrades to zeroes.
///
/// Each agent's stats come from `git_state::shortstats`, which spawns just the
/// two git processes the badge reads (status + numstat) rather than the ~7 a
/// full `GitState` needs. Agents are queried in parallel, so total latency is
/// bounded by the slowest agent's git invocation, not the sum. The reply
/// carries only the three numbers per agent — no file list — to keep the IPC
/// payload flat as the agent count grows.
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
        let Some(repo) = agent.repos.first() else {
            continue;
        };
        let Ok(checkout) = repo_checkout_path(&agent.id, &repo.subdir) else {
            continue;
        };
        let agent_id = agent.id.clone();
        set.spawn(async move { (agent_id, git_state::shortstats(&checkout).await) });
    }
    let mut out = std::collections::HashMap::new();
    while let Some(res) = set.join_next().await {
        if let Ok((id, stats)) = res {
            out.insert(id, stats);
        }
    }
    Ok(out)
}

/// App-wide background poll that refreshes PR state for every agent with a
/// recorded PR, so the sidebar badge (and any open Git panel) reflects merges /
/// closes / mergeability changes that happen on GitHub — without the user
/// having to open the panel. Returns an `agent_id -> PrState | null` map.
///
/// Unlike the per-trigger `fetch_and_emit_pr_state` path (which emits an event),
/// this returns the states directly so the caller folds them into the store
/// synchronously. That avoids a startup race: `usePoll` fires immediately, and
/// routing through `pr:state_changed` would drop results emitted before the
/// store's listener finishes attaching during `init()`.
///
/// Only agents with a known PR *number* are polled: discovery of a brand-new PR
/// still rides the existing turn-end / push / git-action triggers, so this poll
/// never fans a `gh` call out to an agent that has no PR. Resolution goes
/// through `resolve_all_pr_states`, which collapses every live lookup into a
/// single batched GraphQL query rather than a per-agent fan-out: by number
/// (never branch), served straight from the persisted snapshot for merged PRs
/// (and closed ones except on the slow re-verify tick), and degrading to that
/// snapshot when GitHub is unreachable or a rate-limit backoff is active. An
/// agent that resolves to nothing is *omitted* from the map — not written as
/// null — so the frontend merge keeps its last-known badge instead of wiping it
/// (same contract as `refresh_all_pr_checks`).
///
/// Each agent's *first* repo is used, matching the rest of the PR subsystem
/// (`get_pr_state`, `fetch_and_emit_pr_state`) and the one-PR-per-agent shape of
/// the store's `prStates` map; multi-repo PR tracking is out of scope here.
#[tauri::command]
pub async fn refresh_all_pr_states(
    supervisor: State<'_, Arc<Supervisor>>,
) -> Result<std::collections::HashMap<String, Option<PrState>>> {
    // Closed PRs are served from the DB snapshot most cycles and only re-verified
    // live on every Nth tick (they can reopen) — cheap coverage of a rare event.
    // Tick 0 (first poll after launch) re-verifies so freshly-adopted state is
    // confirmed right away.
    let tick = PR_STATE_TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let reverify_closed = tick % CLOSED_REVERIFY_EVERY == 0;
    let states =
        crate::supervisor::resolve_all_pr_states(&supervisor.workspace, reverify_closed).await;
    // Only present states land in the map — an omitted agent keeps its
    // last-known badge on the frontend merge (never wiped to null).
    Ok(states.into_iter().map(|(id, pr)| (id, Some(pr))).collect())
}

/// Monotonic tick for `refresh_all_pr_states`, driving the slow closed-PR
/// re-verify cadence.
static PR_STATE_TICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Re-verify closed PRs live on every Nth `refresh_all_pr_states` tick.
const CLOSED_REVERIFY_EVERY: u64 = 6;

/// Refresh CI checks for every agent with an open PR, so the sidebar can tint
/// each PR pill pass/fail without opening the Git panel. Mirror of
/// `refresh_all_pr_states`: skips archived agents and any without a PR, and
/// collapses the lookups into a single batched GraphQL query rather than a
/// per-agent fan-out. Best-effort: only a resolved rollup lands in the map
/// (including "no checks configured"); a not-found/partial-error alias, a
/// whole-batch failure, or an active rate-limit backoff omits the agent, so the
/// frontend's merge keeps its last-known tint instead of wiping it — matching
/// `fetchPrAux`'s contract.
#[tauri::command]
pub async fn refresh_all_pr_checks(
    supervisor: State<'_, Arc<Supervisor>>,
) -> Result<std::collections::HashMap<String, Option<gh::PrChecks>>> {
    let Some(workspace) = supervisor.workspace.current() else {
        return Ok(Default::default());
    };
    // Paused for rate-limit backoff → return nothing so the frontend merge keeps
    // every agent's last-known tint instead of wiping it.
    if gh::client::is_backing_off() {
        return Ok(Default::default());
    }

    // Gather one (agent, PR ref) per agent with a branch + PR number, resolving
    // the slug via local git (the network cost is deferred to the single batched
    // query below, not fanned out per agent).
    let mut agent_ids: Vec<String> = Vec::new();
    let mut refs: Vec<gh::PrRef> = Vec::new();
    for agent in workspace.agents {
        if agent.archive.is_some() {
            continue;
        }
        let Some(repo) = agent.repos.first() else {
            continue;
        };
        let (Some(_branch), Some(number)) = (repo.branch.as_ref(), repo.pr_number) else {
            continue;
        };
        let Ok(checkout) = repo_checkout_path(&agent.id, &repo.subdir) else {
            continue;
        };
        let Some((owner, repo_name)) = gh::resolve_slug(&checkout, Some(&repo.repo_path)).await
        else {
            continue;
        };
        agent_ids.push(agent.id.clone());
        refs.push(gh::PrRef {
            owner,
            repo: repo_name,
            number: number as u32,
        });
    }

    let mut out = std::collections::HashMap::new();
    // A whole-batch failure leaves the map empty (all agents keep last-known);
    // per-alias `None` (PR not found / partial error) is dropped for the same
    // reason. Only a resolved rollup — including "no checks" — is recorded.
    if let Ok(results) = gh::pr_checks_batch(&refs).await {
        for (id, checks) in agent_ids.into_iter().zip(results) {
            if let Some(checks) = checks {
                out.insert(id, Some(checks));
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// File panel — browse the checkout, view & edit file contents.
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

/// One entry in the checkout file list. Directories are derived on the
/// frontend from the path segments; only files are sent over IPC.
#[derive(Serialize)]
pub struct CheckoutFile {
    pub path: String,
    /// Git status vs the parent branch: "M" | "A" | "D" | "R" (None = clean).
    pub status: Option<String>,
    pub additions: u32,
    pub deletions: u32,
}

/// A single file's contents plus the metadata the editor needs.
#[derive(Serialize)]
pub struct CheckoutFileContents {
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

/// Join a caller-supplied relative path onto the checkout root, rejecting
/// anything that could escape it (absolute paths, `..`, drive prefixes).
fn safe_join(checkout: &Path, rel: &str) -> Result<PathBuf> {
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
    Ok(checkout.join(p))
}

// ── Agent → repo → checkout resolution ────────────────────────────
// Nearly every git/PR command operates on the agent's *primary* (first) repo.
// These helpers centralize that resolution — and its error strings — so the
// command bodies stay focused on the git/gh call they actually make.

/// The agent's primary (first) repo, or an error if the agent has no repos.
fn primary_repo(supervisor: &Supervisor, agent_id: &str) -> Result<TrackedRepo> {
    supervisor
        .workspace
        .agent(agent_id)?
        .repos
        .into_iter()
        .next()
        .ok_or_else(|| Error::Other("agent has no repos".into()))
}

/// The agent's primary repo paired with its checkout path.
fn primary_repo_checkout(
    supervisor: &Supervisor,
    agent_id: &str,
) -> Result<(TrackedRepo, PathBuf)> {
    let repo = primary_repo(supervisor, agent_id)?;
    let checkout = repo_checkout_path(agent_id, &repo.subdir)?;
    Ok((repo, checkout))
}

/// Best-effort variant for read-only lookups (git / PR state): returns `None`
/// instead of an error when the agent or its repo can't be resolved, so callers
/// can degrade gracefully rather than surfacing a failure.
fn primary_repo_checkout_opt(
    supervisor: &Supervisor,
    agent_id: &str,
) -> Result<Option<(TrackedRepo, PathBuf)>> {
    let Ok(record) = supervisor.workspace.agent(agent_id) else {
        return Ok(None);
    };
    let Some(repo) = record.repos.into_iter().next() else {
        return Ok(None);
    };
    let checkout = repo_checkout_path(agent_id, &repo.subdir)?;
    Ok(Some((repo, checkout)))
}

/// The agent's branch name, or an error if the checkout has no branch yet.
fn repo_branch(repo: &TrackedRepo) -> Result<&str> {
    repo.branch
        .as_deref()
        .ok_or_else(|| Error::Other("agent has no branch yet".into()))
}

/// Resolve the agent's primary checkout and its parent ref (the fork point
/// used for file-tree / per-file diffs).
fn primary_checkout(supervisor: &Supervisor, agent_id: &str) -> Result<(PathBuf, String)> {
    let (repo, checkout) = primary_repo_checkout(supervisor, agent_id)?;
    // File tree / per-file diffs compare committed work against the fork point.
    let parent = diff_base(&repo).unwrap_or_else(|| "main".to_string());
    Ok((checkout, parent))
}

/// List the agent's checkout files (tracked + untracked), each tagged with
/// its git status vs the parent branch. This mirrors what's actually on disk
/// — like a regular file explorer — so files the agent deleted are dropped
/// rather than lingering as struck-through entries.
#[tauri::command]
pub async fn list_checkout_tree(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<Vec<CheckoutFile>> {
    let (checkout, parent) = primary_checkout(&supervisor, &agent_id)?;

    let state = git_state::query(&checkout, &parent).await.ok();
    let status_for = |path: &str| -> Option<&FileStatus> {
        state.as_ref()?.files.iter().find(|f| f.path == path)
    };

    let mut paths: BTreeSet<String> = git::list_files(&checkout)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();
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
            CheckoutFile {
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

/// Read a checkout file for the viewer/editor: contents, language hint,
/// git status, and the changed-line numbers driving the gutter.
#[tauri::command]
pub async fn read_checkout_file(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    path: String,
) -> Result<CheckoutFileContents> {
    let (checkout, parent) = primary_checkout(&supervisor, &agent_id)?;
    let abs = safe_join(&checkout, &path)?;
    let lang = lang_for(&path);

    let state = git_state::query(&checkout, &parent).await.ok();
    let status = state
        .as_ref()
        .and_then(|s| s.files.iter().find(|f| f.path == path))
        .map(|f| status_code(&f.kind).to_string());

    let empty = |text: String, binary: bool, too_large: bool| CheckoutFileContents {
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
        let text = git::show_file(&checkout, &parent, &path)
            .await
            .unwrap_or_default();
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
        git::file_changed_lines(&checkout, &parent, &path)
            .await
            .unwrap_or_default()
    } else {
        (vec![], vec![])
    };

    Ok(CheckoutFileContents {
        text,
        lang,
        status,
        chg_add,
        chg_mod,
        binary: false,
        too_large: false,
    })
}

/// Full unified diff of one checkout file versus the parent branch — the data
/// behind the Code panel's Live view. Returns "" when the file is unchanged.
#[tauri::command]
pub async fn get_file_diff(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    path: String,
) -> Result<String> {
    let (checkout, parent) = primary_checkout(&supervisor, &agent_id)?;
    git::file_diff(&checkout, &parent, &path).await
}

/// Overwrite a checkout file with new contents (the editor's Save / Revert).
#[tauri::command]
pub async fn write_checkout_file(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    path: String,
    contents: String,
) -> Result<()> {
    let (checkout, _parent) = primary_checkout(&supervisor, &agent_id)?;
    let abs = safe_join(&checkout, &path)?;
    if let Some(dir) = abs.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&abs, contents)?;
    Ok(())
}

/// Resolve a not-yet-existing destination inside the checkout: reject path
/// traversal, refuse to clobber an existing entry, and create its parent
/// directory. The create / rename / copy commands all share this so the
/// no-clobber + path-safety contract lives in exactly one place.
fn resolve_new_path(checkout: &Path, rel: &str) -> Result<PathBuf> {
    let abs = safe_join(checkout, rel)?;
    if abs.exists() {
        return Err(Error::Other(format!("\"{rel}\" already exists")));
    }
    if let Some(dir) = abs.parent() {
        std::fs::create_dir_all(dir)?;
    }
    Ok(abs)
}

/// Rename/move a checkout path (file or directory). Refuses to clobber an
/// existing destination so a rename can never silently overwrite a sibling.
#[tauri::command]
pub async fn rename_checkout_path(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    from: String,
    to: String,
) -> Result<()> {
    let (checkout, _parent) = primary_checkout(&supervisor, &agent_id)?;
    let src = safe_join(&checkout, &from)?;
    let dst = resolve_new_path(&checkout, &to)?;
    std::fs::rename(&src, &dst)?;
    Ok(())
}

/// Delete a checkout path. Files are removed directly; directories are
/// removed recursively (the UI guards this behind a confirm step). Deleting a
/// path that's already gone is a no-op, so concurrent deletes don't error.
#[tauri::command]
pub async fn delete_checkout_path(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    path: String,
) -> Result<()> {
    let (checkout, _parent) = primary_checkout(&supervisor, &agent_id)?;
    let abs = safe_join(&checkout, &path)?;
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
pub async fn create_checkout_file(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    path: String,
) -> Result<()> {
    let (checkout, _parent) = primary_checkout(&supervisor, &agent_id)?;
    let abs = resolve_new_path(&checkout, &path)?;
    std::fs::write(&abs, "")?;
    Ok(())
}

/// Create a new directory. Refuses to clobber an existing path.
#[tauri::command]
pub async fn create_checkout_dir(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    path: String,
) -> Result<()> {
    let (checkout, _parent) = primary_checkout(&supervisor, &agent_id)?;
    let abs = resolve_new_path(&checkout, &path)?;
    std::fs::create_dir_all(&abs)?;
    Ok(())
}

/// Copy a checkout file to a new path (the explorer's "Duplicate"). Refuses
/// to overwrite an existing destination.
#[tauri::command]
pub async fn copy_checkout_file(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    from: String,
    to: String,
) -> Result<()> {
    let (checkout, _parent) = primary_checkout(&supervisor, &agent_id)?;
    let src = safe_join(&checkout, &from)?;
    let dst = resolve_new_path(&checkout, &to)?;
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

/// Resolve a required CLI and probe its `--version`. Drives the first-run
/// readiness check. For `git` this reflects unified resolution (system or the
/// portable dist — see `git_dist`); other tools are presence-only.
#[tauri::command]
pub async fn check_cli(name: String) -> ToolStatus {
    tokio::task::spawn_blocking(move || crate::agent::check_cli(&name))
        .await
        .unwrap_or(ToolStatus {
            installed: false,
            version: None,
            path: None,
            source: None,
        })
}

/// Manually (re)trigger portable-git resolution/installation. The startup
/// bootstrap may have failed (offline first launch, blocked network) — this
/// gives the readiness UI a retry that doesn't require an app restart. Emits
/// the same `git-dist:state` events as the startup path, so existing
/// listeners render its progress unchanged.
#[tauri::command]
pub async fn git_dist_install(app: AppHandle) -> Result<()> {
    use tauri::Emitter;
    crate::git_dist::resolve_or_install(move |payload| {
        let _ = app.emit("git-dist:state", payload);
    })
    .await
    .map_err(Error::Other)
}

/// Run the pinned official installer for an agent CLI (see `agent_install`),
/// streaming progress via `agent-install:state` events. Resolves when the
/// installer exits; the frontend re-probes providers afterwards to confirm
/// the binary is now detectable.
#[tauri::command]
pub async fn install_agent(app: AppHandle, id: String) -> Result<()> {
    use tauri::Emitter;
    crate::agent_install::install(id, move |payload| {
        let _ = app.emit("agent-install:state", payload);
    })
    .await
    .map_err(Error::Other)
}

/// Validate a candidate custom agent binary path: is it an executable file,
/// and what `--version` does it report? The providers settings UI calls this
/// before saving a path override so it can show immediate inline feedback
/// (green version on success, error on failure) and block a broken save.
#[tauri::command]
pub async fn validate_agent_bin(path: String) -> BinValidation {
    tokio::task::spawn_blocking(move || crate::agent::validate_bin(&path))
        .await
        .unwrap_or(BinValidation {
            executable: false,
            version: None,
        })
}

/// Discover the models each agent CLI reports it supports (raw ids + any cheap
/// metadata the CLI provides). The frontend enriches these against models.dev
/// to build the unified catalog. Never errors — an absent/broken CLI just
/// contributes no models.
#[tauri::command]
pub async fn discover_supported_models() -> Vec<crate::model_catalog::AgentModels> {
    crate::model_catalog::discover_supported_models().await
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
