//! Git state surfaces shared with the remote dispatcher: one checkout's full
//! state, and the fleet-wide shortstats poll behind the sidebar badges.

use crate::error::Result;
use crate::git_state::{self, GitMeta, GitState, ShortStats};
use crate::supervisor::Supervisor;

use super::files::{agent_repo_checkout_opt, checkout_pending};

/// Git state for one of the agent's checkouts — the repo whose `subdir`
/// matches, or the primary when none is given.
///
/// Shared with the remote dispatcher.
pub async fn get_git_state_impl(
    supervisor: &Supervisor,
    agent_id: &str,
    subdir: Option<&str>,
) -> Result<Option<GitState>> {
    // Still cloning: no checkout to describe yet. `None` is what an unresolvable
    // agent already returns, and the panel renders it as Loading… rather than
    // as a status full of phantom deletions.
    if checkout_pending(supervisor, agent_id) {
        return Ok(None);
    }
    let Some((repo, checkout)) = agent_repo_checkout_opt(supervisor, agent_id, subdir)? else {
        return Ok(None);
    };
    // A refused checkout answers with the keys to remove rather than an error:
    // the panel polls this, and a failing poll just freezes it on stale state.
    // Checked before resolving the base, which runs git there too.
    let blocked = crate::git::hardening::steerable_config(&checkout).await?;
    if !blocked.is_empty() {
        let parent = repo.parent_branch.clone().unwrap_or_default();
        return Ok(Some(GitState::blocked(parent, blocked)));
    }
    let base = repo.resolve_base(&checkout).await;
    let state = git_state::query(&checkout, &base).await?;
    Ok(Some(state))
}

/// A compact shortstat (additions / deletions / file count) for every live
/// agent's primary repo, keyed by agent id. Used by the app-wide background
/// poll that powers per-agent shortstats in the sidebar and the right-rail
/// file-count badge. Archived agents and agents with no resolvable repo are
/// omitted; a git error degrades to zeroes.
///
/// Each agent's stats come from `git_state::shortstats`, which spawns just the
/// two git processes the badge reads (status + numstat) rather than the ~7 a
/// full `GitState` needs. Agents are queried in parallel, so total latency is
/// bounded by the slowest agent's git invocation, not the sum. The reply
/// carries only the three numbers per agent — no file list — to keep the IPC
/// payload flat as the agent count grows.
///
/// Shared with the remote dispatcher.
pub async fn get_all_shortstats_impl(
    supervisor: &Supervisor,
) -> Result<std::collections::HashMap<String, ShortStats>> {
    let workspace = match supervisor.workspace.current() {
        Some(w) => w,
        None => return Ok(Default::default()),
    };
    let mut set = tokio::task::JoinSet::new();
    for agent in workspace.agents {
        // Omitted while provisioning for the same reason as archived agents:
        // there is nothing to count yet, and counting a half-written clone
        // would flash a phantom file count on the badge.
        if agent.archive.is_some() || checkout_pending(supervisor, &agent.id) {
            continue;
        }
        // One shortstat per checkout; a multi-repo agent's badge shows the
        // sum across all of its repos (matching the archive metadata, which
        // also aggregates). Single-repo agents behave exactly as before.
        for repo in &agent.repos {
            let Ok(checkout) = repo.checkout_path(&agent.id) else {
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
/// always-visible "base moved · N behind" chips and the cross-agent
/// file-overlap hints.
///
/// Purely local git — no network. Each checkout's `behind` is measured against
/// its resolved base tip (`git::resolve_base`), preferring the SOURCE repo's
/// `refs/remotes/origin/<base>` that the slow `refresh_base_freshness` poll
/// advances, whenever that commit is readable from the checkout. Without a
/// GitHub connection the source ref never advances, so `behind` stays
/// unknown/zero and no chip shows — the intended silent degrade. File paths
/// always resolve (local status), so overlap hints work with or without GitHub.
///
/// Queried in parallel; a git error degrades that checkout to a bare
/// unknown-behind / empty-files entry rather than dropping it.
///
/// Shared with the remote dispatcher.
pub async fn get_all_git_meta_impl(
    supervisor: &Supervisor,
) -> Result<std::collections::HashMap<String, GitMeta>> {
    let workspace = match supervisor.workspace.current() {
        Some(w) => w,
        None => return Ok(Default::default()),
    };
    let mut set = tokio::task::JoinSet::new();
    for agent in workspace.agents {
        if agent.archive.is_some() || checkout_pending(supervisor, &agent.id) {
            continue;
        }
        for (i, repo) in agent.repos.iter().enumerate() {
            let Ok(checkout) = repo.checkout_path(&agent.id) else {
                continue;
            };
            let key = crate::supervisor::pr_map_key(&agent.id, &repo.subdir, i == 0);
            // Resolved inside the task, not in this loop: resolving a base
            // shells out to git, and doing that serially would make the poll's
            // latency scale with the fleet size instead of the slowest checkout.
            let repo = repo.clone();
            set.spawn(async move {
                let base = repo.resolve_base(&checkout).await;
                let meta = git_state::git_meta(&checkout, &base).await;
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
