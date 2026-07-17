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
    repo_checkout_path, AgentRecord, AgentView, DiffStats, ProjectDeleteResult, TrackedRepo,
    Workspace,
};

#[tauri::command]
pub fn get_workspace(supervisor: State<'_, Arc<Supervisor>>) -> Option<Workspace> {
    supervisor.current_workspace()
}

/// A workflow run's step agents (live + archived). Run-owned agents are hidden
/// from `get_workspace`, so the run monitor fetches them here to render each
/// attempt's preserved chat.
#[tauri::command]
pub fn wf_run_agents(run_id: String, supervisor: State<'_, Arc<Supervisor>>) -> Vec<AgentRecord> {
    supervisor.run_agents(&run_id)
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

/// Attach a repo to an existing project (multi-repo projects). Two phases,
/// deliberately ordered so a doomed attach can never mutate the picked
/// folder: the DB attach validates the project and commits first, and only
/// then is a non-git folder initialized (mirroring `add_workspace_repo`).
/// If that filesystem step fails, the DB attach is rolled back precisely.
#[tauri::command]
pub async fn attach_repo_to_project(
    supervisor: State<'_, Arc<Supervisor>>,
    project_id: String,
    repo_path: String,
) -> Result<Workspace> {
    let sup = supervisor.inner().clone();
    let path = PathBuf::from(repo_path);
    let outcome = sup.attach_repo_to_project(&project_id, &path)?;
    if let Err(e) = new_project::ensure_git_repo(&path).await {
        // Roll back the row(s) the DB phase wrote; best-effort — it re-applies
        // keys we wrote moments ago on a local connection, so a failure here
        // means the DB itself is gone, and the init error is the one to show.
        let _ = sup.undo_attach(&outcome);
        return Err(e);
    }
    Ok(sup.workspace.current().expect("workspace initialized"))
}

/// Detach a repo from a project. Rejects the project's last repo and any repo
/// still referenced by an agent checkout (live or archived).
#[tauri::command]
pub fn detach_repo_from_project(
    supervisor: State<'_, Arc<Supervisor>>,
    project_id: String,
    repo_path: String,
) -> Result<Workspace> {
    supervisor.detach_repo_from_project(&project_id, PathBuf::from(repo_path))
}

/// Set a repo's display label within its project ("Frontend", "Gateway").
/// Blank clears back to the folder-basename fallback.
#[tauri::command]
pub fn set_repo_label(
    supervisor: State<'_, Arc<Supervisor>>,
    repo_path: String,
    label: String,
) -> Result<Workspace> {
    supervisor.set_repo_label(PathBuf::from(repo_path), &label)
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

/// Delete a project and all of its workspaces, including non-archived ones.
/// The supervisor refuses while any project agent is actively running.
#[tauri::command]
pub async fn delete_project(
    supervisor: State<'_, Arc<Supervisor>>,
    workflows: State<'_, Arc<crate::workflow::scheduler::WorkflowService>>,
    project_id: String,
) -> Result<ProjectDeleteResult> {
    supervisor
        .inner()
        .clone()
        .delete_project(workflows.inner(), &project_id)
        .await
}

#[tauri::command]
pub fn project_has_running_agents(
    supervisor: State<'_, Arc<Supervisor>>,
    project_id: String,
) -> bool {
    supervisor.project_has_running_agents(&project_id)
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
            // Forked-conversation context is set only by the fork path.
            forked_context: None,
            custom_agent_id,
            skills: skills.unwrap_or_default(),
            mcp_servers: mcp_servers.unwrap_or_default(),
            fork_base,
            // User-initiated spawns fork from the source repo, not a run repo,
            // and are never run-owned; the scheduler sets both for a step spawn.
            run_repo: None,
            owner_run_id: None,
            // Carrying another workspace's working tree is a fork-only path.
            carry_from: None,
        },
    )
    .await
}

/// Fork an existing workspace into a new one, seeding its worktree (`code`) and
/// conversation (`context`) independently. `context = up_to_message` carries the
/// parent conversation through the navigable prompt at a 0-based ordinal (the
/// same ordinal the chat's turn list uses; git-action turns excluded).
///
/// `context_digest` is the frontend-rendered prose for the carried range — built
/// there so it renders uniformly across every provider's chat adapter and always
/// matches the history the child shows. `null`/empty when nothing is carried.
///
/// `snapshot_max_seq` is the highest `session_records.seq` the frontend saw when
/// it built the digest; the copy is capped at it so a sync that appends to the
/// parent between the two reads can't seed the child with turns the brief omitted.
#[tauri::command]
pub async fn fork_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    parent_id: String,
    code: crate::supervisor::ForkCode,
    context: crate::supervisor::ForkContext,
    context_digest: Option<String>,
    snapshot_max_seq: Option<i64>,
) -> Result<AgentRecord> {
    let sup = supervisor.inner().clone();
    sup.fork_agent(
        app,
        &parent_id,
        code,
        context,
        context_digest,
        snapshot_max_seq,
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

/// Push the targeted repo's current branch to origin (primary by default).
#[tauri::command]
pub async fn push_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    agent_id: String,
    subdir: Option<String>,
) -> Result<String> {
    let (repo, checkout) = agent_repo_checkout(&supervisor, &agent_id, subdir.as_deref())?;
    let branch = repo_branch(&repo)?.to_string();
    let summary = git::push(&checkout, &branch, false).await?;
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
    subdir: Option<String>,
) -> Result<()> {
    let (_repo, checkout) = agent_repo_checkout(&supervisor, &agent_id, subdir.as_deref())?;
    git::commit(&checkout, &message).await
}

/// Discard every uncommitted change in the checkout (destructive).
#[tauri::command]
pub async fn discard_agent_changes(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    subdir: Option<String>,
) -> Result<()> {
    let (_repo, checkout) = agent_repo_checkout(&supervisor, &agent_id, subdir.as_deref())?;
    git::discard_all(&checkout).await
}

/// Stash all working-tree changes including untracked files.
#[tauri::command]
pub async fn stash_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    subdir: Option<String>,
) -> Result<()> {
    let (_repo, checkout) = agent_repo_checkout(&supervisor, &agent_id, subdir.as_deref())?;
    git::stash_push(&checkout).await
}

/// Abort an in-progress merge in the agent's checkout.
#[tauri::command]
pub async fn abort_merge_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    subdir: Option<String>,
) -> Result<()> {
    let (_repo, checkout) = agent_repo_checkout(&supervisor, &agent_id, subdir.as_deref())?;
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
    subdir: Option<String>,
) -> Result<()> {
    let (repo, _checkout) = agent_repo_checkout(&supervisor, &agent_id, subdir.as_deref())?;
    let branch = repo_branch(&repo)?;
    git::branch_delete(&repo.repo_path, branch).await
}

/// Pull latest into the targeted repo's checkout (primary by default).
#[tauri::command]
pub async fn pull_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    subdir: Option<String>,
) -> Result<()> {
    let (_repo, checkout) = agent_repo_checkout(&supervisor, &agent_id, subdir.as_deref())?;
    git::pull(&checkout).await
}

/// Rebase the agent's branch onto its parent (base) branch. Used by the
/// clean-state panel action to catch up when the base has advanced.
#[tauri::command]
pub async fn rebase_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    subdir: Option<String>,
) -> Result<()> {
    let (repo, checkout) = agent_repo_checkout(&supervisor, &agent_id, subdir.as_deref())?;
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
    subdir: Option<String>,
) -> Result<PrState> {
    let (repo, checkout) = agent_repo_checkout(&supervisor, &agent_id, subdir.as_deref())?;
    let base = repo.parent_branch.as_deref().unwrap_or("main");
    let pr = gh::pr_create(&checkout, &title, &body, base).await?;
    crate::telemetry::track("pr_opened", serde_json::json!({ "source": "manual" }));
    // Bind the PR to this agent (number + state snapshot) so later lookups
    // don't rely on the (recyclable) branch name. A failure here isn't fatal —
    // the next idle/push poll re-binds it via guarded discovery once the PR
    // shows OPEN — but the helper logs it so the gap is observable, not silent.
    crate::supervisor::persist_pr_snapshot(&supervisor.workspace, &agent_id, &repo.subdir, &pr);
    // If the agent now has PRs in two or more repos, cross-link the whole set
    // in each PR's body (best-effort, off the command's critical path).
    let workspace = supervisor.workspace.clone();
    tauri::async_runtime::spawn(async move {
        crate::supervisor::sync_pr_set_links(&workspace, &agent_id).await;
    });
    Ok(pr)
}

/// Merge the open PR for the targeted repo's current branch.
#[tauri::command]
pub async fn merge_pr(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    subdir: Option<String>,
) -> Result<()> {
    let (_repo, checkout) = agent_repo_checkout(&supervisor, &agent_id, subdir.as_deref())?;
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
    subdir: Option<String>,
) -> Result<Option<PrState>> {
    Ok(
        crate::supervisor::resolve_pr_state(&supervisor.workspace, &agent_id, subdir.as_deref())
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

/// List the open PRs for a repo by path, for the draft (new-workspace)
/// composer's "#" mention autocomplete. Unlike `list_prs`, this needs no agent
/// — a draft has no checkout yet — so it queries the base repo directly.
/// Capped at 50 to match `list_prs`.
#[tauri::command]
pub async fn list_repo_prs(repo_path: String) -> Result<Vec<gh::PrSummary>> {
    gh::pr_list(&expand_tilde(&repo_path), 50).await
}

/// Fetch the PR merge gate + per-check detail (spec §6). Best-effort: any
/// failure (no PR, gh missing, API error) returns `None` and the panel falls
/// back to `mergeable`-only behavior.
#[tauri::command]
pub async fn get_pr_checks(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    subdir: Option<String>,
) -> Result<Option<gh::PrChecks>> {
    let Some((repo, checkout)) =
        agent_repo_checkout_opt(&supervisor, &agent_id, subdir.as_deref())?
    else {
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
    subdir: Option<String>,
) -> Result<Option<gh::PrComments>> {
    let Some((repo, checkout)) =
        agent_repo_checkout_opt(&supervisor, &agent_id, subdir.as_deref())?
    else {
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

/// Default wall-clock budget for an ad-hoc verification run's checks, matching
/// the workflow tests gate's `DEFAULT_TESTS_TIMEOUT_SECS` (15 min). Ad-hoc
/// checkouts have no step budget to draw from.
const VERIFY_TIMEOUT_SECS: u64 = 900;

/// Run the project's deterministic checks — install → test → lint — in an
/// agent's checkout and return a [`crate::verify::VerificationReport`]. Resolves
/// the target repo via `subdir` (primary when `None`) and layers the project's
/// `run.test` / `run.install` / `run.lint` overrides over detection, the same
/// layering the workflow tests gate uses.
#[tauri::command]
pub async fn run_verification(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    subdir: Option<String>,
) -> Result<crate::verify::VerificationReport> {
    let (_repo, checkout) = agent_repo_checkout(&supervisor, &agent_id, subdir.as_deref())?;
    // Project-scoped command overrides (mirrors the tests gate's `run.test` /
    // `run.install`, plus `run.lint`). Empty project_id → detection only.
    let project_id = supervisor
        .workspace
        .agent(&agent_id)
        .map(|r| r.project_id)
        .unwrap_or_default();
    let setting = |key: &str| -> Option<String> {
        if project_id.is_empty() {
            None
        } else {
            supervisor.workspace.project_setting(&project_id, key)
        }
    };
    let verifier = crate::verify::Verifier::new(
        setting("run.test"),
        setting("run.install"),
        setting("run.lint"),
        VERIFY_TIMEOUT_SECS,
    )?;
    let report = verifier.verify(&checkout).await;
    tracing::info!(
        agent_id = %agent_id,
        passed = report.passed(),
        checks = report.checks.len(),
        "ran ad-hoc verification"
    );
    Ok(report)
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

/// Discover the `KEY=value` pairs in a project's `.env` (in the *source* repo,
/// where gitignored env files live), for the Run & Environment settings list.
/// Missing/unreadable `.env` → empty. Values are returned so the UI can show
/// them masked and flag overrides that differ; it never writes them anywhere.
#[tauri::command]
pub fn read_env_file_keys(repo_path: String) -> Result<Vec<crate::run_env::EnvEntry>> {
    Ok(crate::run_env::read_env_file(&expand_tilde(&repo_path)))
}

/// Read a project variable's override value (keychain-backed) so the settings
/// UI can pre-fill the edit field. `None` when no override is set.
#[tauri::command]
pub fn get_env_override(project_id: String, key: String) -> Option<String> {
    crate::run_env::override_get(&crate::run_env::override_secret_key(&project_id, &key))
}

/// Store a project variable's override value in the override store (OS keychain
/// on release macOS; in-memory session store on dev / non-macOS) so a
/// user-chosen value (e.g. a disposable per-agent DB URL) can diverge from
/// `.env` without ever being written to the database.
#[tauri::command]
pub fn set_env_override(project_id: String, key: String, value: String) -> Result<()> {
    crate::run_env::override_set(
        &crate::run_env::override_secret_key(&project_id, &key),
        &value,
    )
}

/// Remove a project variable's override; resolution falls back to the `.env`
/// value (mirror).
#[tauri::command]
pub fn clear_env_override(project_id: String, key: String) -> Result<()> {
    crate::run_env::override_delete(&crate::run_env::override_secret_key(&project_id, &key))
}

/// Returns git state for one of the agent's checkouts — the repo whose
/// `subdir` matches, or the primary when none is given.
#[tauri::command]
pub async fn get_git_state(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    subdir: Option<String>,
) -> Result<Option<GitState>> {
    let Some((repo, checkout)) =
        agent_repo_checkout_opt(&supervisor, &agent_id, subdir.as_deref())?
    else {
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
        // One shortstat per checkout; a multi-repo agent's badge shows the
        // sum across all of its repos (matching the archive metadata, which
        // also aggregates). Single-repo agents behave exactly as before.
        for repo in &agent.repos {
            let Ok(checkout) = repo_checkout_path(&agent.id, &repo.subdir) else {
                continue;
            };
            let agent_id = agent.id.clone();
            set.spawn(async move { (agent_id, git_state::shortstats(&checkout).await) });
        }
    }
    let mut out: std::collections::HashMap<String, ShortStats> = std::collections::HashMap::new();
    while let Some(res) = set.join_next().await {
        if let Ok((id, stats)) = res {
            let entry = out.entry(id).or_insert(ShortStats {
                additions: 0,
                deletions: 0,
                file_count: 0,
            });
            entry.additions += stats.additions;
            entry.deletions += stats.deletions;
            entry.file_count += stats.file_count;
        }
    }
    Ok(out)
}

/// Advisory fleet-wide git metadata for every live agent's checkouts, keyed by
/// the frontend's `gitKey` convention (plain agent id for the primary repo,
/// `"{agent_id}::{subdir}"` for secondaries — same as the PR maps). Feeds the
/// always-visible "base moved · N behind" chips and the cross-agent file-overlap
/// hints.
///
/// Purely local git — no network. Each checkout's `behind` is measured against
/// the base tip resolved from its SOURCE repo's `refs/remotes/origin/<base>`,
/// which the slow `refresh_base_freshness` poll advances; the clone shares the
/// source's object store, so the moved base is reachable there without the clone
/// fetching (see `git_state::git_meta`). Without a GitHub connection the source
/// ref never advances, so `behind` stays unknown/zero and no chip shows — the
/// intended silent degrade. File paths always resolve (local status), so overlap
/// hints work with or without GitHub.
///
/// Queried in parallel; a git error degrades that checkout to a bare
/// unknown-behind / empty-files entry rather than dropping it.
#[tauri::command]
pub async fn get_all_git_meta(
    supervisor: State<'_, Arc<Supervisor>>,
) -> Result<std::collections::HashMap<String, git_state::GitMeta>> {
    let workspace = match supervisor.workspace.current() {
        Some(w) => w,
        None => return Ok(Default::default()),
    };
    let mut set = tokio::task::JoinSet::new();
    for agent in workspace.agents {
        if agent.archive.is_some() {
            continue;
        }
        for (i, repo) in agent.repos.iter().enumerate() {
            let Ok(checkout) = repo_checkout_path(&agent.id, &repo.subdir) else {
                continue;
            };
            let base = repo.parent_branch.clone().unwrap_or_else(|| "main".into());
            let key = crate::supervisor::pr_map_key(&agent.id, &repo.subdir, i == 0);
            let source = repo.repo_path.clone();
            set.spawn(async move {
                let base_sha = git::remote_base_sha(&source, &base).await;
                let meta = git_state::git_meta(&checkout, &base, base_sha.as_deref()).await;
                (key, meta)
            });
        }
    }
    let mut out = std::collections::HashMap::new();
    while let Some(res) = set.join_next().await {
        if let Ok((key, meta)) = res {
            out.insert(key, meta);
        }
    }
    Ok(out)
}

/// Slow-cadence background fetch that advances each project's base branch on its
/// SOURCE repo, so `get_all_git_meta` can measure staleness against a base that
/// moved on GitHub (a sibling's PR merging, a teammate's push). One fetch per
/// distinct `(source repo, base)` — deduped so a multi-agent project pays a
/// single fetch, and the shared object store propagates it to every clone.
///
/// Best-effort and silent by contract: a paused rate-limit backoff skips the
/// whole sweep, and each fetch failure is logged and stepped over — a background
/// fetch must never raise a user-facing error. Returns nothing; the next
/// `get_all_git_meta` tick reflects whatever landed.
#[tauri::command]
pub async fn refresh_base_freshness(supervisor: State<'_, Arc<Supervisor>>) -> Result<()> {
    // Paused → touch no network; the last-fetched base tips still serve.
    if gh::client::is_backing_off() {
        return Ok(());
    }
    let Some(workspace) = supervisor.workspace.current() else {
        return Ok(());
    };
    // Distinct (source repo, base) across every live agent's repos — one fetch
    // covers all clones that share that source's objects.
    let mut seen: BTreeSet<(PathBuf, String)> = BTreeSet::new();
    for agent in &workspace.agents {
        if agent.archive.is_some() {
            continue;
        }
        for repo in &agent.repos {
            let base = repo.parent_branch.clone().unwrap_or_else(|| "main".into());
            seen.insert((repo.repo_path.clone(), base));
        }
    }
    for (source, base) in seen {
        if let Err(e) = git::fetch_base(&source, &base).await {
            tracing::debug!(error = %e, source = %source.display(), base, "base freshness fetch skipped");
        }
    }
    Ok(())
}

/// App-wide background poll that refreshes PR state for every repo with a
/// recorded PR across every agent, so the sidebar badge (and any open Git
/// panel) reflects merges / closes / mergeability changes that happen on
/// GitHub — without the user having to open the panel. Returns a map keyed by
/// the frontend's `gitKey` convention: the agent's primary repo under the
/// plain agent id (what every existing consumer reads) and each secondary
/// repo under `"{agent_id}::{subdir}"` — so a multi-repo agent's PR on a
/// secondary repo reaches the sidebar too.
///
/// Unlike the per-trigger `fetch_and_emit_pr_state` path (which emits an event),
/// this returns the states directly so the caller folds them into the store
/// synchronously. That avoids a startup race: `usePoll` fires immediately, and
/// routing through `pr:state_changed` would drop results emitted before the
/// store's listener finishes attaching during `init()`.
///
/// Only repos with a known PR *number* are polled: discovery of a brand-new PR
/// still rides the existing turn-end / push / git-action triggers, so this poll
/// never fans a `gh` call out to a repo that has no PR. Resolution goes
/// through `resolve_all_pr_states`, which collapses every live lookup into a
/// single batched GraphQL query rather than a per-repo fan-out: by number
/// (never branch), served straight from the persisted snapshot for merged PRs
/// (and closed ones except on the slow re-verify tick), and degrading to that
/// snapshot when GitHub is unreachable or a rate-limit backoff is active. A
/// repo that resolves to nothing is *omitted* from the map — not written as
/// null — so the frontend merge keeps its last-known badge instead of wiping it
/// (same contract as `refresh_all_pr_checks`).
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

/// Refresh CI checks for every repo with an open PR across every agent, so the
/// sidebar can tint each PR pill pass/fail without opening the Git panel.
/// Mirror of `refresh_all_pr_states`, including its key shape (`gitKey`: plain
/// agent id for the primary repo, `"{agent_id}::{subdir}"` for secondaries):
/// skips archived agents and any repo without a PR, and collapses the lookups
/// into a single batched GraphQL query rather than a per-repo fan-out.
/// Best-effort: only a resolved rollup lands in the map (including "no checks
/// configured"); a not-found/partial-error alias, a whole-batch failure, or an
/// active rate-limit backoff omits the repo, so the frontend's merge keeps its
/// last-known tint instead of wiping it — matching `fetchPrAux`'s contract.
#[tauri::command]
pub async fn refresh_all_pr_checks(
    supervisor: State<'_, Arc<Supervisor>>,
) -> Result<std::collections::HashMap<String, Option<gh::PrChecks>>> {
    let Some(workspace) = supervisor.workspace.current() else {
        return Ok(Default::default());
    };
    // Paused for rate-limit backoff → return nothing so the frontend merge keeps
    // every repo's last-known tint instead of wiping it.
    if gh::client::is_backing_off() {
        return Ok(Default::default());
    }

    // Gather one (key, PR ref) per repo with a branch + PR number, resolving
    // the slug via local git (the network cost is deferred to the single batched
    // query below, not fanned out per repo).
    let mut keys: Vec<String> = Vec::new();
    let mut refs: Vec<gh::PrRef> = Vec::new();
    for agent in workspace.agents {
        if agent.archive.is_some() {
            continue;
        }
        for (i, repo) in agent.repos.iter().enumerate() {
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
            keys.push(crate::supervisor::pr_map_key(
                &agent.id,
                &repo.subdir,
                i == 0,
            ));
            refs.push(gh::PrRef {
                owner,
                repo: repo_name,
                number: number as u32,
            });
        }
    }

    let mut out = std::collections::HashMap::new();
    // A whole-batch failure leaves the map empty (all repos keep last-known);
    // per-alias `None` (PR not found / partial error) is dropped for the same
    // reason. Only a resolved rollup — including "no checks" — is recorded.
    if let Ok(results) = gh::pr_checks_batch(&refs).await {
        for (key, checks) in keys.into_iter().zip(results) {
            if let Some(checks) = checks {
                out.insert(key, Some(checks));
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

/// The tracked repo a panel command targets: the one whose `subdir` matches,
/// or the primary when no subdir is given — which keeps every single-repo
/// caller (and old frontends that don't pass the arg) byte-identical.
fn agent_repo_checkout(
    supervisor: &Supervisor,
    agent_id: &str,
    subdir: Option<&str>,
) -> Result<(TrackedRepo, PathBuf)> {
    let Some(s) = subdir else {
        return primary_repo_checkout(supervisor, agent_id);
    };
    let record = supervisor.workspace.agent(agent_id)?;
    let repo = record
        .repos
        .into_iter()
        .find(|r| r.subdir == s)
        .ok_or_else(|| Error::Other(format!("agent has no tracked repo {s:?}")))?;
    let checkout = repo_checkout_path(agent_id, &repo.subdir)?;
    Ok((repo, checkout))
}

/// Best-effort variant for read-only lookups (git / PR state): returns `None`
/// instead of an error when the agent or its repo can't be resolved, so callers
/// can degrade gracefully rather than surfacing a failure.
fn agent_repo_checkout_opt(
    supervisor: &Supervisor,
    agent_id: &str,
    subdir: Option<&str>,
) -> Result<Option<(TrackedRepo, PathBuf)>> {
    let Ok(record) = supervisor.workspace.agent(agent_id) else {
        return Ok(None);
    };
    let repo = match subdir {
        None => record.repos.into_iter().next(),
        Some(s) => record.repos.into_iter().find(|r| r.subdir == s),
    };
    let Some(repo) = repo else {
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

/// Split a repo-prefixed Code-tab path (`"<subdir>/<rel>"`) into its tracked
/// repo and the checkout-relative remainder. Only ever called for multi-repo
/// agents — a single-repo agent's paths are never prefixed (all prefix logic
/// is gated on `repos.len() > 1`), so a real top-level directory that happens
/// to share a repo's name can't be misrouted here.
fn split_repo_path<'a>(repos: &'a [TrackedRepo], path: &str) -> Result<(&'a TrackedRepo, String)> {
    let (first, rest) = path.split_once('/').unwrap_or((path, ""));
    let repo = repos.iter().find(|r| r.subdir == first).ok_or_else(|| {
        let known: Vec<&str> = repos.iter().map(|r| r.subdir.as_str()).collect();
        Error::InvalidPath(format!(
            "{path:?} must start with one of the agent's repo folders: {}",
            known.join(", ")
        ))
    })?;
    if rest.is_empty() {
        return Err(Error::InvalidPath(format!(
            "{path:?} names a repo folder itself, not a path inside it"
        )));
    }
    Ok((repo, rest.to_string()))
}

/// Resolve a Code-tab path to the checkout it lives in: `(checkout root,
/// parent ref, checkout-relative path)`. Single-repo agents use the primary
/// checkout with the path unchanged — the exact legacy behavior. For a
/// multi-repo agent every tree path is prefixed with the repo's `subdir`
/// (see `list_checkout_tree`), so the first segment picks the checkout.
fn checkout_scope_for_path(
    supervisor: &Supervisor,
    agent_id: &str,
    path: &str,
) -> Result<(PathBuf, String, String)> {
    let record = supervisor.workspace.agent(agent_id)?;
    if record.repos.len() <= 1 {
        let (checkout, parent) = primary_checkout(supervisor, agent_id)?;
        return Ok((checkout, parent, path.to_string()));
    }
    let (repo, rel) = split_repo_path(&record.repos, path)?;
    let checkout = repo_checkout_path(agent_id, &repo.subdir)?;
    let parent = diff_base(repo).unwrap_or_else(|| "main".to_string());
    Ok((checkout, parent, rel))
}

/// One checkout's file list (tracked + untracked, deleted dropped), each
/// tagged with its git status vs `parent`. `prefix` (a repo's subdir) is
/// prepended to every path for multi-repo agents' virtual roots.
async fn checkout_tree_files(
    checkout: &Path,
    parent: &str,
    prefix: Option<&str>,
) -> Vec<CheckoutFile> {
    let state = git_state::query(checkout, parent).await.ok();
    let status_for = |path: &str| -> Option<&FileStatus> {
        state.as_ref()?.files.iter().find(|f| f.path == path)
    };

    let mut paths: BTreeSet<String> = git::list_files(checkout)
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

    paths
        .into_iter()
        .map(|path| {
            let st = status_for(&path);
            CheckoutFile {
                status: st.map(|f| status_code(&f.kind).to_string()),
                additions: st.map(|f| f.additions).unwrap_or(0),
                deletions: st.map(|f| f.deletions).unwrap_or(0),
                path: match prefix {
                    Some(p) => format!("{p}/{path}"),
                    None => path,
                },
            }
        })
        .collect()
}

/// List the agent's checkout files (tracked + untracked), each tagged with
/// its git status vs the parent branch. This mirrors what's actually on disk
/// — like a regular file explorer — so files the agent deleted are dropped
/// rather than lingering as struck-through entries.
///
/// Single-repo agents get today's un-prefixed listing of the primary checkout.
/// A multi-repo agent gets one virtual root per repo: every path is prefixed
/// with the checkout's `subdir`, each repo's status computed against its own
/// fork point — the tree component nests on `/`, so the repos render as
/// top-level folders. The file read/write commands resolve the same prefix
/// back to the owning checkout (`checkout_scope_for_path`).
#[tauri::command]
pub async fn list_checkout_tree(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<Vec<CheckoutFile>> {
    let record = supervisor.workspace.agent(&agent_id)?;
    if record.repos.len() <= 1 {
        let (checkout, parent) = primary_checkout(&supervisor, &agent_id)?;
        return Ok(checkout_tree_files(&checkout, &parent, None).await);
    }
    let mut out = Vec::new();
    for repo in &record.repos {
        // One broken checkout shouldn't blank the whole tree — skip it and
        // keep listing the others.
        let Ok(checkout) = repo_checkout_path(&agent_id, &repo.subdir) else {
            continue;
        };
        let parent = diff_base(repo).unwrap_or_else(|| "main".to_string());
        out.extend(checkout_tree_files(&checkout, &parent, Some(&repo.subdir)).await);
    }
    Ok(out)
}

/// List a repo's files by path (tracked + non-ignored untracked), for the
/// draft (new-workspace) composer's "@" mention autocomplete. Unlike
/// `list_checkout_tree`, this needs no agent — a draft has no checkout yet — so
/// it reads the base repo directly and returns plain paths (no diff status,
/// since there's no fork point to diff against).
#[tauri::command]
pub async fn list_repo_tree(repo_path: String) -> Result<Vec<String>> {
    git::list_files(&expand_tilde(&repo_path)).await
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

/// Discover the user- and project-level slash commands a provider exposes on
/// disk (e.g. Claude's `~/.claude/commands` + `<project>/.claude/commands`),
/// for the composer's `/` autocomplete. `project_dir` is the agent's project
/// root, or None for the new-agent composer before a project is chosen. Empty
/// (never an error) for providers without command discovery or when the dirs
/// are absent.
#[tauri::command]
pub async fn discover_slash_commands(
    provider: String,
    project_dir: Option<String>,
) -> Result<Vec<crate::slash_commands::DiscoveredCommand>> {
    let project = project_dir.as_deref().map(expand_tilde);
    Ok(crate::slash_commands::discover(
        &provider,
        project.as_deref(),
    ))
}

/// Captured output of a one-shot `claude <args>` invocation, run for a local
/// slash command the stream-json session can't service (e.g. `/doctor` →
/// `claude doctor`). Rendered into the chat as a notice; `success` is the exit
/// status so the UI can flag failures.
#[derive(Serialize)]
pub struct ClaudeCommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

/// Run a read-only `claude` subcommand (e.g. `doctor`, `mcp list`) in the
/// agent's checkout and capture its output. Runs unsandboxed like the
/// model-list probes (a read-only CLI query), honoring a per-agent binary
/// override before PATH discovery. `args` is a fixed command vocabulary chosen
/// by the frontend dispatcher, never free user input.
#[tauri::command]
pub async fn run_claude_command(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    args: Vec<String>,
) -> Result<ClaudeCommandOutput> {
    let (_, checkout) = primary_repo_checkout(&supervisor, &agent_id)?;
    let home = dirs::home_dir().ok_or_else(|| Error::Other("no home directory".into()))?;
    let bin = match crate::bin_resolve::resolve_agent_override(&agent_id, &home) {
        Some(Ok(path)) => path,
        _ => crate::bin_resolve::resolve_bin("claude", &home)
            .ok_or_else(|| Error::Other("claude binary not found".into()))?,
    };

    let mut cmd = tokio::process::Command::new(&bin);
    cmd.args(&args).current_dir(&checkout).kill_on_drop(true);
    if let Some(env) = crate::bin_resolve::login_shell_env() {
        for (k, v) in env {
            cmd.env(k, v);
        }
    }

    const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
    let out = tokio::time::timeout(TIMEOUT, cmd.output())
        .await
        .map_err(|_| Error::Other(format!("claude {} timed out", args.join(" "))))?
        .map_err(|e| Error::Other(format!("run claude {}: {e}", args.join(" "))))?;

    Ok(ClaudeCommandOutput {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        success: out.status.success(),
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
    let (checkout, parent, path) = checkout_scope_for_path(&supervisor, &agent_id, &path)?;
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
    let (checkout, parent, path) = checkout_scope_for_path(&supervisor, &agent_id, &path)?;
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
    let (checkout, _parent, path) = checkout_scope_for_path(&supervisor, &agent_id, &path)?;
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
/// Source and destination resolve their repo scope independently, so a move
/// between a multi-repo agent's checkouts (sibling directories on the same
/// volume) works like any other rename.
#[tauri::command]
pub async fn rename_checkout_path(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    from: String,
    to: String,
) -> Result<()> {
    let (checkout_from, _parent, from) = checkout_scope_for_path(&supervisor, &agent_id, &from)?;
    let (checkout_to, _parent, to) = checkout_scope_for_path(&supervisor, &agent_id, &to)?;
    let src = safe_join(&checkout_from, &from)?;
    let dst = resolve_new_path(&checkout_to, &to)?;
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
    let (checkout, _parent, path) = checkout_scope_for_path(&supervisor, &agent_id, &path)?;
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
    let (checkout, _parent, path) = checkout_scope_for_path(&supervisor, &agent_id, &path)?;
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
    let (checkout, _parent, path) = checkout_scope_for_path(&supervisor, &agent_id, &path)?;
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
    let (checkout_from, _parent, from) = checkout_scope_for_path(&supervisor, &agent_id, &from)?;
    let (checkout_to, _parent, to) = checkout_scope_for_path(&supervisor, &agent_id, &to)?;
    let src = safe_join(&checkout_from, &from)?;
    let dst = resolve_new_path(&checkout_to, &to)?;
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
mod split_repo_path_tests {
    use super::split_repo_path;
    use crate::workspace::TrackedRepo;

    fn repo(subdir: &str) -> TrackedRepo {
        TrackedRepo {
            repo_path: std::path::PathBuf::from(format!("/src/{subdir}")),
            subdir: subdir.into(),
            branch: None,
            parent_branch: None,
            base_sha: None,
            pr_number: None,
            pr_url: None,
            pr_title: None,
            pr_state: None,
            label: None,
        }
    }

    #[test]
    fn routes_first_segment_to_the_matching_repo() {
        let repos = [repo("frontend"), repo("backend")];
        let (r, rel) = split_repo_path(&repos, "backend/src/main.rs").unwrap();
        assert_eq!(r.subdir, "backend");
        assert_eq!(rel, "src/main.rs");
    }

    #[test]
    fn rejects_unknown_prefix_listing_tracked_folders() {
        let repos = [repo("frontend"), repo("backend")];
        let err = split_repo_path(&repos, "shared/util.ts").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("frontend"),
            "should list tracked folders: {msg}"
        );
        assert!(
            msg.contains("backend"),
            "should list tracked folders: {msg}"
        );
    }

    #[test]
    fn rejects_a_bare_repo_root() {
        // "frontend" alone names the checkout itself — never a file operation
        // target (renaming/deleting a repo root must not be possible).
        let repos = [repo("frontend"), repo("backend")];
        assert!(split_repo_path(&repos, "frontend").is_err());
        assert!(split_repo_path(&repos, "frontend/").is_err());
    }
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
