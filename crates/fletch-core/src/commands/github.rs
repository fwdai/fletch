//! GitHub reads and writes shared with the remote dispatcher: connection
//! status, repo clone, and the per-agent PR surfaces (create, state, checks,
//! the fast live tick).

use std::path::Path;

use crate::error::Result;
use crate::github::{self as gh, GhRepoSummary, GhStatus, PrState};
use crate::new_project;
use crate::supervisor::Supervisor;

use super::files::{agent_repo_checkout, agent_repo_checkout_opt};

/// Whether the app has a working GitHub connection — drives the New Project
/// flow's gating (clone and create both need the API).
pub async fn gh_status_impl() -> Result<GhStatus> {
    gh::auth_status().await
}

/// The authenticated user's GitHub repos, newest first, for the clone picker.
pub async fn gh_repo_list_impl() -> Result<Vec<GhRepoSummary>> {
    gh::repo_list(200).await
}

/// Clone a GitHub repo into `dest_parent/<repo-name>` and register it as a
/// workspace project.
///
/// Shared with the remote dispatcher, so a phone clones and pins through the
/// same path the desktop's New Project dialog uses.
pub async fn clone_repo_impl(
    supervisor: &Supervisor,
    spec: &str,
    dest_parent: &str,
) -> Result<crate::workspace::Workspace> {
    let target = new_project::clone(spec, Path::new(dest_parent)).await?;
    supervisor.add_workspace_repo(target)
}

/// Create a PR for the agent's current branch. Pass empty title/body to
/// auto-fill from commits.
///
/// Shared with the remote dispatcher, so a PR opened from the phone is bound,
/// snapshotted and cross-linked exactly like a desktop one.
pub async fn create_pr_impl(
    supervisor: &Supervisor,
    agent_id: String,
    title: &str,
    body: &str,
    subdir: Option<&str>,
) -> Result<PrState> {
    let (repo, checkout) = agent_repo_checkout(supervisor, &agent_id, subdir)?;
    let base = repo.base_branch().await;
    let pr = gh::pr_create(&checkout, title, body, &base).await?;
    crate::telemetry::track("pr_opened", serde_json::json!({ "source": "manual" }));
    // Bind the PR to this agent (number + state snapshot) so later lookups
    // don't rely on the (recyclable) branch name. A failure here isn't fatal —
    // the next idle/push poll re-binds it via guarded discovery once the PR
    // shows OPEN — but the helper logs it so the gap is observable, not silent.
    crate::supervisor::persist_pr_snapshot(&supervisor.workspace, &agent_id, &repo.subdir, &pr);
    // If the agent now has PRs in two or more repos, cross-link the whole set
    // in each PR's body (best-effort, off the command's critical path).
    let workspace = supervisor.workspace.clone();
    crate::host::spawn(async move {
        crate::supervisor::sync_pr_set_links(&workspace, &agent_id).await;
    });
    Ok(pr)
}

/// Fetch and return the current PR state for the agent's primary repo: by
/// bound number when one is recorded (with the persisted snapshot as the
/// fallback when GitHub is unreachable), else discovered by branch. Unbound
/// merged/closed PRs on a recycled branch are included here for panel
/// display, though the app-wide paths never claim them as the agent's.
///
/// Shared with the remote dispatcher.
pub async fn get_pr_state_impl(
    supervisor: &Supervisor,
    agent_id: &str,
    subdir: Option<&str>,
) -> Result<Option<PrState>> {
    Ok(crate::supervisor::resolve_pr_state(
        &supervisor.workspace,
        agent_id,
        subdir,
        // Called after a user action (create/merge/push, a delegated git op), so
        // a new PR is plausible right now — worth the branch scan immediately.
        crate::supervisor::Discovery::Forced,
    )
    .await
    .map(|(pr, _bound)| pr))
}

/// Fetch the PR merge gate + per-check detail (spec §6). Best-effort: any
/// failure (no PR, gh missing, API error) returns `None` and the panel falls
/// back to `mergeable`-only behavior.
///
/// Shared with the remote dispatcher.
pub async fn get_pr_checks_impl(
    supervisor: &Supervisor,
    agent_id: &str,
    subdir: Option<&str>,
) -> Result<Option<gh::PrChecks>> {
    let Some((repo, checkout)) = agent_repo_checkout_opt(supervisor, agent_id, subdir)? else {
        return Ok(None);
    };
    if repo.branch.is_none() {
        return Ok(None);
    }
    Ok(gh::pr_checks(&checkout).await.unwrap_or(None))
}

/// The Git panel's fast tick: PR state + CI, both over ETag-conditional REST.
///
/// GitHub does not count a `304 Not Modified` against the primary rate limit, so
/// this poll is free whenever nothing has changed — which lets the panel run a
/// tight cadence without spending the GraphQL points budget that the review
/// threads (`get_pr_threads`) still need.
///
/// State comes from the shared `resolve_pr_state`, not a bespoke read, so this
/// path inherits its policy rather than restating it: **merged** served from the
/// database with no network, a failed fetch **degrading to the last persisted
/// snapshot** instead of erasing a badge GitHub already confirmed, and a
/// discovered OPEN PR **adopted** (bound) so later ticks stop re-discovering it.
///
/// `Ok(None)` therefore means "this repo has no PR" — a real, renderable answer
/// — and is no longer conflated with "the lookup failed". The frontend relies on
/// that distinction to decide whether writing `null` is safe.
///
/// Shared with the remote dispatcher.
pub async fn get_pr_live_impl(
    supervisor: &Supervisor,
    agent_id: &str,
    subdir: Option<&str>,
) -> Result<Option<gh::PrLive>> {
    let Some((state, _bound)) = crate::supervisor::resolve_pr_state(
        &supervisor.workspace,
        agent_id,
        subdir,
        // A background poll: an unbound repo scans on an interval rather than
        // paying a point every tick for the same "still no PR".
        crate::supervisor::Discovery::Throttled,
    )
    .await
    else {
        return Ok(None);
    };
    // CI only matters while the PR is open: the panel renders it for no other
    // state, and a settled PR's checks can't change.
    if !matches!(state.state, gh::PrStatus::Open) {
        return Ok(Some(gh::PrLive {
            state,
            checks: None,
        }));
    }
    let Some((repo, checkout)) = agent_repo_checkout_opt(supervisor, agent_id, subdir)? else {
        return Ok(Some(gh::PrLive {
            state,
            checks: None,
        }));
    };
    let checks = gh::pr_checks_live(&checkout, Some(&repo.repo_path), state.number)
        .await
        .unwrap_or(None);
    Ok(Some(gh::PrLive { state, checks }))
}
