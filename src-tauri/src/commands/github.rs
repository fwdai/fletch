//! GitHub connection, repo clone/create/publish, and per-agent PR handlers
//! (state, checks, comments, issues) plus the app-wide PR polling sweeps.

use std::path::Path;
use std::sync::Arc;
use tauri::State;

use crate::error::{Error, Result};
use crate::github::{self as gh, GhRepoSummary, GhStatus, PrState};
use crate::new_project;
use crate::supervisor::Supervisor;
use crate::workspace::repo_checkout_path;

use super::files::{
    agent_repo_checkout, agent_repo_checkout_opt, expand_tilde, primary_repo, primary_repo_checkout,
};

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
) -> Result<crate::workspace::Workspace> {
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
) -> Result<crate::workspace::Workspace> {
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

/// List open GitHub issues for a repo by path, for the Home inbox. Like
/// `list_repo_prs`, this needs no agent — Home works over the workspace's
/// tracked repo paths. `Ok(None)` (not an error) when the repo has no token /
/// non-GitHub origin / a rate-limit pause is active, so the inbox degrades
/// quietly. Capped at 30 — the inbox shows a handful, newest-updated first.
#[tauri::command]
pub async fn list_repo_issues(repo_path: String) -> Result<Option<Vec<gh::IssueSummary>>> {
    gh::issue_list(&expand_tilde(&repo_path), 30).await
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
