//! Per-agent git actions shared with the remote dispatcher: push and commit.

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
