//! Per-agent git actions shared with the remote dispatcher: commit, push, and
//! the Git panel's working-tree moves (pull, rebase, stash, discard, abort),
//! and clearing the config keys that block a checkout.
//!
//! Every one of them acts inside the agent's own checkout and nowhere else,
//! which is the reach `commit_agent_impl` has always had. `delete_branch_agent`
//! is deliberately not here: it force-deletes a ref in the *parent* repository
//! (the user's real clone), outside any checkout, so it stays a desktop-only
//! command — see `docs/remote-protocol.md`.

use std::sync::Arc;

use crate::error::Result;
use crate::git;
use crate::host::EngineCtx;
use crate::supervisor::Supervisor;

use super::files::{agent_repo_checkout, repo_branch};

/// Push the targeted repo's current branch to origin (primary by default).
///
/// Shared with the remote dispatcher, so a push from the phone triggers the
/// same background PR-state fetch the desktop push does.
pub async fn push_agent_impl(
    supervisor: &Arc<Supervisor>,
    ctx: Arc<EngineCtx>,
    agent_id: String,
    subdir: Option<&str>,
) -> Result<String> {
    let (repo, checkout) = agent_repo_checkout(supervisor, &agent_id, subdir)?;
    let branch = repo_branch(&repo)?.to_string();
    let summary = git::push(&checkout, &branch, false).await?;
    // After successful push, fetch PR state in background
    supervisor.fetch_and_emit_pr_state(ctx, agent_id);
    Ok(summary)
}

/// Stage all working-tree changes and commit them with the given message.
/// Shared with the remote dispatcher.
pub async fn commit_agent_impl(
    supervisor: &Supervisor,
    agent_id: &str,
    message: &str,
    subdir: Option<&str>,
) -> Result<()> {
    let (_repo, checkout) = agent_repo_checkout(supervisor, agent_id, subdir)?;
    git::commit(&checkout, message).await
}

/// Pull latest into the targeted repo's checkout (primary by default).
/// Shared with the remote dispatcher.
pub async fn pull_agent_impl(
    supervisor: &Supervisor,
    agent_id: &str,
    subdir: Option<&str>,
) -> Result<()> {
    let (_repo, checkout) = agent_repo_checkout(supervisor, agent_id, subdir)?;
    git::pull(&checkout).await
}

/// Rebase the agent's branch onto its parent (base) branch — the clean-state
/// panel action for catching up when the base has advanced.
/// Shared with the remote dispatcher.
pub async fn rebase_agent_impl(
    supervisor: &Supervisor,
    agent_id: &str,
    subdir: Option<&str>,
) -> Result<()> {
    let (repo, checkout) = agent_repo_checkout(supervisor, agent_id, subdir)?;
    // Onto the base's resolved tip, not its name: the clone's local
    // `refs/heads/<base>` is a stale snapshot from clone time.
    let base = repo.resolve_base(&checkout).await;
    git::rebase_onto(&checkout, &base).await
}

/// Stash all working-tree changes in the checkout, including untracked files.
/// Shared with the remote dispatcher.
pub async fn stash_agent_impl(
    supervisor: &Supervisor,
    agent_id: &str,
    subdir: Option<&str>,
) -> Result<()> {
    let (_repo, checkout) = agent_repo_checkout(supervisor, agent_id, subdir)?;
    git::stash_push(&checkout).await
}

/// Discard every uncommitted change in the checkout (destructive).
/// Shared with the remote dispatcher.
pub async fn discard_agent_changes_impl(
    supervisor: &Supervisor,
    agent_id: &str,
    subdir: Option<&str>,
) -> Result<()> {
    let (_repo, checkout) = agent_repo_checkout(supervisor, agent_id, subdir)?;
    git::discard_all(&checkout).await
}

/// Abort an in-progress merge in the agent's checkout, restoring the pre-merge
/// working tree. Shared with the remote dispatcher.
pub async fn abort_merge_agent_impl(
    supervisor: &Supervisor,
    agent_id: &str,
    subdir: Option<&str>,
) -> Result<()> {
    let (_repo, checkout) = agent_repo_checkout(supervisor, agent_id, subdir)?;
    git::merge_abort(&checkout).await
}

/// Remove the config keys that make Fletch refuse to run git in the checkout
/// (`GitState.blocked_config`) — the user's way out of that refusal. Writes only
/// the checkout's own `.git/config`. Shared with the remote dispatcher.
pub async fn clear_checkout_config_impl(
    supervisor: &Supervisor,
    agent_id: &str,
    subdir: Option<&str>,
) -> Result<()> {
    let (_repo, checkout) = agent_repo_checkout(supervisor, agent_id, subdir)?;
    git::hardening::remove_steerable_config(&checkout).await
}
