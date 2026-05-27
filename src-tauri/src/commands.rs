//! Tauri IPC command handlers — the thin frontend-facing surface.

use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, State};

use crate::error::Result;
use crate::gh::{self, PrState};
use crate::git;
use crate::git_state::{self, GitState};
use crate::names;
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

#[tauri::command]
pub async fn spawn_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    view: Option<AgentView>,
    repo_path: String,
    provider: Option<String>,
) -> Result<AgentRecord> {
    let sup = supervisor.inner().clone();
    sup.spawn_agent(
        app,
        view.unwrap_or_default(),
        PathBuf::from(repo_path),
        provider.unwrap_or_else(|| "claude".to_string()),
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
    text: String,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.send_user_message(&app, &agent_id, &text)
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
pub fn read_session_transcript(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<Vec<Value>> {
    supervisor.read_session_transcript(&agent_id)
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
    agent_id: String,
) -> Result<()> {
    let record = supervisor.workspace.agent(&agent_id)?;
    let repo = record.repos.first()
        .ok_or_else(|| crate::error::Error::Other("agent has no repos".into()))?;
    let worktree = repo_worktree_path(&agent_id, &repo.subdir)?;
    let branch = repo.branch.as_deref()
        .ok_or_else(|| crate::error::Error::Other("agent has no branch yet".into()))?;
    git::push(&worktree, branch).await
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
