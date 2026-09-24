//! Per-agent git actions driven from the Git panel: push, commit, discard,
//! stash, abort-merge, branch listing/deletion, pull, and rebase.

use std::path::Path;
use std::sync::Arc;
use tauri::State;

use crate::error::Result;
use crate::git;
use crate::host::EngineCtx;
use crate::supervisor::Supervisor;

use super::files::{agent_repo_checkout, repo_branch};

/// Push the targeted repo's current branch to origin (primary by default).
#[tauri::command]
pub async fn push_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    ctx: State<'_, Arc<EngineCtx>>,
    agent_id: String,
    subdir: Option<String>,
) -> Result<String> {
    fletch_core::commands::push_agent_impl(
        supervisor.inner(),
        ctx.inner().clone(),
        agent_id,
        subdir.as_deref(),
    )
    .await
}

/// Stage all working-tree changes and commit them with the given message.
#[tauri::command]
pub async fn commit_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    message: String,
    subdir: Option<String>,
) -> Result<()> {
    fletch_core::commands::commit_agent_impl(&supervisor, &agent_id, &message, subdir.as_deref())
        .await
}

/// Discard every uncommitted change in the checkout (destructive).
#[tauri::command]
pub async fn discard_agent_changes(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    subdir: Option<String>,
) -> Result<()> {
    fletch_core::commands::discard_agent_changes_impl(&supervisor, &agent_id, subdir.as_deref())
        .await
}

/// Stash all working-tree changes including untracked files.
#[tauri::command]
pub async fn stash_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    subdir: Option<String>,
) -> Result<()> {
    fletch_core::commands::stash_agent_impl(&supervisor, &agent_id, subdir.as_deref()).await
}

/// Abort an in-progress merge in the agent's checkout.
#[tauri::command]
pub async fn abort_merge_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    subdir: Option<String>,
) -> Result<()> {
    fletch_core::commands::abort_merge_agent_impl(&supervisor, &agent_id, subdir.as_deref()).await
}

/// Remove the config keys that make Fletch refuse to run git in the checkout.
#[tauri::command]
pub async fn clear_checkout_config(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    subdir: Option<String>,
) -> Result<()> {
    fletch_core::commands::clear_checkout_config_impl(&supervisor, &agent_id, subdir.as_deref())
        .await
}

/// List all local branches in a repo. Used by the new-agent composer to
/// let the user pick the base branch before spawning.
#[tauri::command]
pub async fn list_repo_branches(repo_path: String) -> Result<Vec<String>> {
    git::list_local_branches(Path::new(&repo_path)).await
}

/// The repo's default branch — what the new-agent screen pre-selects as the
/// base, instead of assuming `"main"` (which silently forks the wrong branch on
/// a `master`/`develop` repo). Resolved from the repo's remote, never from the
/// branch the user currently has checked out; see `git::default_branch`.
/// Infallible by construction — falls back to `"main"` on its own.
#[tauri::command]
pub async fn repo_default_branch(repo_path: String) -> Result<String> {
    Ok(git::default_branch(Path::new(&repo_path)).await)
}

/// Force-delete the agent's local branch from its parent repository.
/// Used by the merged-state UI to clean up after a PR lands. Safe-noops
/// if the branch is already gone (matches `git::branch_delete` semantics).
///
/// Desktop-only on purpose: `repo.repo_path` is the user's real clone, not the
/// agent's checkout, so this is the one Git-panel action that writes outside the
/// workspace — and it writes with `branch -D`. It has no `_impl` because the
/// remote dispatcher must not be able to reach it (`docs/remote-protocol.md`).
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
    fletch_core::commands::pull_agent_impl(&supervisor, &agent_id, subdir.as_deref()).await
}

/// Rebase the agent's branch onto its parent (base) branch. Used by the
/// clean-state panel action to catch up when the base has advanced.
#[tauri::command]
pub async fn rebase_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    subdir: Option<String>,
) -> Result<()> {
    fletch_core::commands::rebase_agent_impl(&supervisor, &agent_id, subdir.as_deref()).await
}
