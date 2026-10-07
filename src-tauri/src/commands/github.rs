//! GitHub connection, repo clone/create/publish, and per-agent PR handlers
//! (state, checks, comments, issues) plus the app-wide PR status read.

use std::sync::Arc;
use tauri::State;

use crate::error::{Error, Result};
use crate::github::{self as gh, GhRepoSummary, GhStatus, PrState};
use crate::host::EngineCtx;
use crate::new_project;
use crate::supervisor::Supervisor;

use super::files::{expand_tilde, primary_repo, primary_repo_checkout};

/// Whether the app has a working GitHub connection — drives the New Project
/// flow's gating (clone and create both need the API).
#[tauri::command]
pub async fn gh_status() -> Result<GhStatus> {
    fletch_core::commands::gh_status_impl().await
}

/// The authenticated user's GitHub repos, newest first, for the clone picker.
#[tauri::command]
pub async fn gh_repo_list() -> Result<Vec<GhRepoSummary>> {
    fletch_core::commands::gh_repo_list_impl().await
}

/// Clone a GitHub repo into `dest_parent/<repo-name>` and register it as a
/// workspace project.
#[tauri::command]
pub async fn clone_repo(
    ctx: State<'_, Arc<EngineCtx>>,
    supervisor: State<'_, Arc<Supervisor>>,
    spec: String,
    dest_parent: String,
) -> Result<crate::workspace::Workspace> {
    fletch_core::commands::announce_workspace(
        ctx.sink.as_ref(),
        fletch_core::commands::clone_repo_impl(&supervisor, &spec, &dest_parent).await,
    )
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
    fletch_core::commands::create_repo_impl(
        &supervisor,
        &name,
        &dest_parent,
        private,
        description.as_deref(),
        // Default true: an older frontend that doesn't pass the flag keeps
        // the original create-and-publish behavior.
        publish.unwrap_or(true),
    )
    .await
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
    fletch_core::commands::create_pr_impl(&supervisor, agent_id, &title, &body, subdir.as_deref())
        .await
}

/// Merge the open PR for the targeted repo's current branch.
#[tauri::command]
pub async fn merge_pr(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    subdir: Option<String>,
) -> Result<()> {
    fletch_core::commands::merge_pr_impl(&supervisor, &agent_id, subdir.as_deref()).await
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
    fletch_core::commands::get_pr_state_impl(&supervisor, &agent_id, subdir.as_deref()).await
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

/// Fetch the PR merge gate + per-check detail (spec §6). Best-effort: any
/// failure (no PR, gh missing, API error) returns `None` and the panel falls
/// back to `mergeable`-only behavior.
#[tauri::command]
pub async fn get_pr_checks(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    subdir: Option<String>,
) -> Result<Option<gh::PrChecks>> {
    fletch_core::commands::get_pr_checks_impl(&supervisor, &agent_id, subdir.as_deref()).await
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
/// `resolve_pr_state`'s own live lookup is now conditional REST too, so routing
/// through it costs nothing extra.
///
/// `Ok(None)` therefore means "this repo has no PR" — a real, renderable answer
/// — and is no longer conflated with "the lookup failed". The frontend relies on
/// that distinction to decide whether writing `null` is safe.
#[tauri::command]
pub async fn get_pr_live(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    subdir: Option<String>,
) -> Result<Option<gh::PrLive>> {
    fletch_core::commands::get_pr_live_impl(&supervisor, &agent_id, subdir.as_deref()).await
}

/// The Git panel's slow tick: unresolved review threads (Greptile / other bots
/// / humans), flattened to each thread's root comment.
///
/// Stays on GraphQL because thread resolution (`isResolved`/`isOutdated`) has
/// no REST equivalent — so unlike `get_pr_live` this one does spend points, 1
/// per call by PR number. Polled well below the state/CI cadence to match.
#[tauri::command]
pub async fn get_pr_threads(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    subdir: Option<String>,
) -> Result<Option<gh::PrComments>> {
    fletch_core::commands::get_pr_threads_impl(&supervisor, &agent_id, subdir.as_deref()).await
}

/// PR state + CI for every repo with a recorded PR across every agent, in one
/// batched round-trip: the sidebar's seed, read on launch, environment switch,
/// reconnect and window focus. Between those the host's PR watcher
/// (`supervisor::pr_watch`) sweeps open PRs once a minute, rechecks closed PRs
/// every five minutes, and emits `pr:state_changed` / `pr:checks_changed` on
/// change, so no client polls it.
/// The remote op of the same name answers the same thing (docs/remote-protocol.md).
///
/// Keyed by the frontend's `checkoutKey` convention: the agent's primary repo
/// under the plain agent id and each secondary repo under
/// `"{agent_id}::{subdir}"`. Each entry's `state`/`checks` are the checkout's
/// focused PR and `prs` its whole PR set. Only PRs with a known *number* are
/// read, by number (never branch), in one GraphQL query; merged PRs are served from the
/// persisted snapshot, closed ones too unless `reverify_closed` asks for a live
/// look (a closed PR can reopen), and everything degrades to the snapshot when
/// GitHub is unreachable or a rate-limit backoff is active. A repo that resolves
/// to nothing is *omitted*, so the frontend merge keeps its last-known badge.
#[tauri::command]
pub async fn get_all_pr_status(
    supervisor: State<'_, Arc<Supervisor>>,
    reverify_closed: Option<bool>,
) -> Result<std::collections::HashMap<String, crate::supervisor::AgentPrStatus>> {
    Ok(crate::supervisor::resolve_all_pr_status(
        &supervisor.workspace,
        reverify_closed.unwrap_or(false),
        false,
    )
    .await)
}

/// Focus one of a checkout's PRs (`number` must be in its set): the PR the
/// badge, panel header and focused reads follow. Emits `pr:state_changed` for
/// it at once. The remote op of the same name answers the same thing.
#[tauri::command]
pub fn set_focused_pr(
    ctx: State<'_, Arc<EngineCtx>>,
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    subdir: Option<String>,
    number: u32,
) -> Result<PrState> {
    fletch_core::commands::set_focused_pr_impl(
        &supervisor,
        &ctx,
        &agent_id,
        subdir.as_deref(),
        number,
    )
}
