//! Git state surfaces: a single checkout's full state, the fleet-wide
//! shortstats poll the sidebar reads, and an on-demand base-freshness fetch.

use std::sync::Arc;
use tauri::State;

use crate::error::Result;
use crate::git_state::{GitState, ShortStats};
use crate::supervisor::Supervisor;

/// Returns git state for one of the agent's checkouts — the repo whose
/// `subdir` matches, or the primary when none is given. `poll` is the Git
/// panel's interval tick, which answers `None` rather than wake a sleeping
/// box; any other read boots it.
#[tauri::command]
pub async fn get_git_state(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    subdir: Option<String>,
    poll: Option<bool>,
) -> Result<Option<GitState>> {
    let subdir = subdir.as_deref();
    if poll.unwrap_or(false) {
        fletch_core::commands::poll_git_state_impl(&supervisor, &agent_id, subdir).await
    } else {
        fletch_core::commands::get_git_state_impl(&supervisor, &agent_id, subdir).await
    }
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

/// Fetch each project's base branch on its SOURCE repo now, so a checkout's
/// base (`git::resolve_base`) is the one that moved on GitHub. The host
/// already runs this pass every five minutes on its own
/// (`supervisor::base_freshness`); this is the same pass on demand, for a
/// manual refresh. Silent by contract: no GitHub credential, a rate-limit
/// backoff or a failed fetch all answer `Ok`.
#[tauri::command]
pub async fn refresh_base_freshness(supervisor: State<'_, Arc<Supervisor>>) -> Result<()> {
    fletch_core::commands::refresh_base_freshness_impl(&supervisor).await;
    Ok(())
}
