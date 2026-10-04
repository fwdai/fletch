//! Git state surfaces: a single checkout's full state, and the fleet-wide
//! shortstats / git-meta polls the sidebar reads, and an on-demand
//! base-freshness fetch.

use std::sync::Arc;
use tauri::State;

use crate::error::Result;
use crate::git_state::{self, GitState, ShortStats};
use crate::supervisor::Supervisor;

/// Returns git state for one of the agent's checkouts — the repo whose
/// `subdir` matches, or the primary when none is given.
#[tauri::command]
pub async fn get_git_state(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    subdir: Option<String>,
) -> Result<Option<GitState>> {
    fletch_core::commands::get_git_state_impl(&supervisor, &agent_id, subdir.as_deref()).await
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
    fletch_core::commands::get_all_shortstats_impl(&supervisor).await
}

/// Advisory fleet-wide git metadata for every live agent's checkouts, keyed by
/// the frontend's `gitKey` convention (plain agent id for the primary repo,
/// `"{agent_id}::{subdir}"` for secondaries — same as the PR maps). Feeds the
/// always-visible "base moved · N behind" chips and the cross-agent file-overlap
/// hints.
///
/// Purely local git — no network. Each checkout's `behind` is measured against
/// its resolved base tip (`git::resolve_base`), preferring the SOURCE repo's
/// `refs/remotes/origin/<base>` that the host's base-freshness loop
/// advances, whenever that commit is readable from the checkout. Without a
/// GitHub connection the source ref never advances, so `behind` stays
/// unknown/zero and no chip shows — the intended silent degrade. File paths always resolve (local status), so overlap
/// hints work with or without GitHub.
///
/// Queried in parallel; a git error degrades that checkout to a bare
/// unknown-behind / empty-files entry rather than dropping it.
#[tauri::command]
pub async fn get_all_git_meta(
    supervisor: State<'_, Arc<Supervisor>>,
) -> Result<std::collections::HashMap<String, git_state::GitMeta>> {
    fletch_core::commands::get_all_git_meta_impl(&supervisor).await
}

/// Fetch each project's base branch on its SOURCE repo now, so
/// `get_all_git_meta` can measure staleness against a base that moved on
/// GitHub. The host already runs this pass every five minutes on its own
/// (`supervisor::base_freshness`); this is the same pass on demand, for a
/// manual refresh. Silent by contract: no GitHub credential, a rate-limit
/// backoff or a failed fetch all answer `Ok`.
#[tauri::command]
pub async fn refresh_base_freshness(supervisor: State<'_, Arc<Supervisor>>) -> Result<()> {
    fletch_core::commands::refresh_base_freshness_impl(&supervisor).await;
    Ok(())
}
