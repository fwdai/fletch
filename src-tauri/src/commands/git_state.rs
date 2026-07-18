//! Git state surfaces: a single checkout's full state, and the fleet-wide
//! shortstats / git-meta / base-freshness polls the sidebar reads.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;

use crate::error::Result;
use crate::git;
use crate::git_state::{self, GitState, ShortStats};
use crate::github as gh;
use crate::supervisor::Supervisor;
use crate::workspace::repo_checkout_path;

use super::files::agent_repo_checkout_opt;

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
