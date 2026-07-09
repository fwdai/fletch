//! GitHub operations over the REST/GraphQL API with the app's own OAuth
//! token — the successor to shelling out to the `gh` CLI, so GitHub features
//! work without any extra tool installed or terminal auth dance.
//!
//! Same public surface (functions and types) the `gh` module exposed; the
//! HTTP/auth plumbing lives in [`client`]. Read ops degrade gracefully:
//! no token, a non-GitHub origin, or a missing PR all yield `Ok(None)` /
//! empty — matching how callers treated gh's "no pull requests found".
//! Mutating ops (create/merge/clone/publish) error loudly instead, telling
//! the user to connect GitHub.
//!
//! GraphQL is used for the read ops because that's what `gh` used under the
//! hood — the payload shapes (UPPERCASE enums, `statusCheckRollup` contexts,
//! review threads) carry over verbatim, and the pure parsers below are the
//! same ones that parsed gh's output.

pub mod client;

use std::path::Path;

use serde_json::{json, Value};

pub use client::{git_auth_env, set_token, TOKEN_SETTING};

use crate::error::{Error, Result};

// ---------------------------------------------------------------------------
// Public types (unchanged from the gh module — the IPC surface is identical)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrStatus {
    Open,
    Merged,
    Closed,
}

impl PrStatus {
    /// Stable lowercase form, matching the serde serialization. Used as the
    /// on-disk value in `worktrees.pr_state`.
    pub fn as_str(&self) -> &'static str {
        match self {
            PrStatus::Open => "open",
            PrStatus::Merged => "merged",
            PrStatus::Closed => "closed",
        }
    }

    /// Inverse of [`as_str`](Self::as_str), for rows written by this app.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "open" => Some(PrStatus::Open),
            "merged" => Some(PrStatus::Merged),
            "closed" => Some(PrStatus::Closed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PrState {
    pub number: u32,
    pub url: String,
    pub state: PrStatus,
    pub title: String,
    pub mergeable: bool,
    /// GitHub's createdAt / mergedAt as ms-epoch, when reported. Stamped onto
    /// `worktrees.pr_opened_at/pr_merged_at` by every PR-state fetch path so
    /// per-day PR history accrues locally (see `record_pr_times`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opened_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merged_at: Option<i64>,
}

/// Lightweight PR summary for the composer's "#" mention autocomplete —
/// just enough to list and reference a PR by number.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PrSummary {
    pub number: u32,
    pub title: String,
    pub state: PrStatus,
}

/// GitHub's combined merge gate (`mergeStateStatus`), normalized. This — not
/// `mergeable` — is what actually decides whether a PR can land (spec §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeState {
    Clean,
    Blocked,
    Unstable,
    Behind,
    Dirty,
    Draft,
    HasHooks,
    Unknown,
}

/// One CI check, normalized from the `statusCheckRollup` contexts (which mix
/// `CheckRun` and legacy `StatusContext` shapes).
#[derive(Debug, Clone, serde::Serialize)]
pub struct CheckRun {
    pub name: String,
    /// "queued" | "in_progress" | "completed"
    pub status: String,
    /// "success" | "failure" | "neutral" | "cancelled" | "skipped" |
    /// "timed_out" | "action_required" | "stale" — None until completed.
    pub conclusion: Option<String>,
    /// Branch-protection data needs an extra (often unauthorized) API call,
    /// so this is always `false` for now — the merge gate comes from
    /// `merge_state` instead (spec §6 fallback).
    pub required: bool,
    pub url: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

/// Rich PR merge-gate + per-check detail (spec §6). Heavier than `pr_view`
/// — callers poll it on a slow cadence.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PrChecks {
    pub merge_state: MergeState,
    /// "none" | "pending" | "passing" | "failing" — checks-only summary.
    pub rollup: String,
    pub total: u32,
    pub passed: u32,
    pub failed: u32,
    pub pending: u32,
    /// Names of failing checks. With `required` detection unavailable this
    /// lists ALL failing checks, not just protected ones.
    pub required_failing: Vec<String>,
    pub runs: Vec<CheckRun>,
}

/// One unresolved PR review thread, flattened to its root comment. Surfaced
/// in the Git panel so review feedback (Greptile, other bots, humans) is
/// visible without leaving the app, with a quick action to hand it to the
/// coding agent.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PrComment {
    /// Comment author's login.
    pub author: String,
    /// True when the author is a GitHub App / bot (`__typename == "Bot"`).
    /// Bots like Greptile already phrase their comments for an AI, so the UI
    /// inserts them as-is; human comments get a file/line context wrapper.
    pub is_bot: bool,
    pub body: String,
    /// File the thread is anchored to. `None` for an unanchored thread (e.g.
    /// the line was deleted).
    pub path: Option<String>,
    pub line: Option<u32>,
    /// Permalink to the thread on GitHub.
    pub url: String,
    /// Replies after the root comment (thread length − 1, clamped at 0).
    pub replies: u32,
}

/// Unresolved review threads for a PR. Heavier than `pr_view` — polled on the
/// same slow cadence as `pr_checks` while a PR is open.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PrComments {
    pub unresolved: Vec<PrComment>,
}

/// GitHub connection state. Drives the New Project UI and readiness rows.
/// `installed` is a legacy of the gh-CLI era kept for IPC compatibility —
/// there is no binary to install anymore, so it is always `true`; what
/// matters now is `authenticated` (a valid app token).
#[derive(Debug, Clone, serde::Serialize)]
pub struct GhStatus {
    pub installed: bool,
    pub authenticated: bool,
    pub login: Option<String>,
}

/// One repo for the New Project clone picker.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GhRepoSummary {
    pub name_with_owner: String,
    pub description: Option<String>,
    pub is_private: bool,
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// Repo / branch context
// ---------------------------------------------------------------------------

/// `owner`/`repo` of the checkout's `origin` remote, or `None` when there is
/// no origin or it isn't github.com. Read ops treat `None` as "no PR".
async fn repo_ref(checkout: &Path) -> Option<(String, String)> {
    let out = crate::git_dist::command(checkout)
        .args(["remote", "get-url", "origin"])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let web = crate::git_state::github_web_url(String::from_utf8_lossy(&out.stdout).trim())?;
    let mut parts = web.strip_prefix("https://github.com/")?.split('/');
    Some((parts.next()?.to_string(), parts.next()?.to_string()))
}

async fn require_repo_ref(checkout: &Path) -> Result<(String, String)> {
    repo_ref(checkout).await.ok_or_else(|| {
        Error::Gh("this repository's `origin` remote is not a GitHub repository".into())
    })
}

/// Resolve `owner/repo` from the checkout's origin, falling back to the source
/// repo when the checkout is broken or gone (they share an origin). `None` when
/// neither resolves to a github.com remote. Shared by the by-number lookups
/// (single and batched) that must survive a checkout casualty.
pub(crate) async fn resolve_slug(checkout: &Path, source: Option<&Path>) -> Option<(String, String)> {
    match repo_ref(checkout).await {
        Some(slug) => Some(slug),
        None => match source {
            Some(src) => repo_ref(src).await,
            None => None,
        },
    }
}

async fn require_current_branch(checkout: &Path, what: &str) -> Result<String> {
    crate::git::current_branch(checkout)
        .await?
        .ok_or_else(|| Error::Gh(format!("{what}: HEAD is detached — no branch to look up")))
}

/// Shared query fields for a PR looked up by branch. Created-desc so
/// `pick_branch_pr` sees the newest first, mirroring gh's branch resolution.
fn branch_prs_query(inner_fields: &str) -> String {
    format!(
        r#"query($owner:String!,$repo:String!,$branch:String!){{
  repository(owner:$owner,name:$repo){{
    pullRequests(headRefName:$branch, states:[OPEN,CLOSED,MERGED], first:30,
                 orderBy:{{field:CREATED_AT,direction:DESC}}){{
      nodes{{ state {inner_fields} }}
    }}
  }}
}}"#
    )
}

/// The PR a branch "belongs to": the newest open PR, else the newest PR of
/// any state — the same preference gh used, so a branch whose PR just merged
/// still resolves to that merged PR instead of disappearing.
fn pick_branch_pr(nodes: &[Value]) -> Option<&Value> {
    nodes
        .iter()
        .find(|n| n["state"].as_str() == Some("OPEN"))
        .or_else(|| nodes.first())
}

/// Run a GraphQL query, mapping GitHub's "not found" errors to `Ok(None)` —
/// the same degradation the gh wrapper applied to its stderr ("could not
/// resolve to a PullRequest", "...not found").
async fn graphql_opt(query: &str, variables: Value) -> Result<Option<Value>> {
    // A rate-limit pause is in effect — skip the request so callers degrade to
    // the persisted snapshot instead of spending one that would likely 403.
    if client::is_backing_off() {
        return Ok(None);
    }
    let client = match client::Client::new() {
        Ok(c) => c,
        // Read paths poll in the background; not being connected is a normal
        // state there, not an error to surface on every tick.
        Err(_) => return Ok(None),
    };
    match client.graphql(query, variables).await {
        Ok(data) => Ok(Some(data)),
        Err(Error::Gh(msg)) => {
            let low = msg.to_lowercase();
            if low.contains("could not resolve") || low.contains("not found") {
                Ok(None)
            } else {
                Err(Error::Gh(msg))
            }
        }
        Err(e) => Err(e),
    }
}

fn branch_pr_nodes(data: &Value) -> Vec<Value> {
    data["repository"]["pullRequests"]["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// PR state
// ---------------------------------------------------------------------------

const PR_STATE_FIELDS: &str = "number url title mergeable createdAt mergedAt";

/// GitHub ISO-8601 timestamp → ms epoch. None for absent/null/unparseable.
fn gh_time_ms(node: &Value, field: &str) -> Option<i64> {
    let iso = node[field].as_str()?;
    chrono::DateTime::parse_from_rfc3339(iso)
        .ok()
        .map(|t| t.timestamp_millis())
}

fn parse_pr_state(node: &Value) -> PrState {
    PrState {
        number: node["number"].as_u64().unwrap_or_default() as u32,
        url: node["url"].as_str().unwrap_or_default().to_string(),
        state: match node["state"].as_str() {
            Some("MERGED") => PrStatus::Merged,
            Some("CLOSED") => PrStatus::Closed,
            _ => PrStatus::Open,
        },
        title: node["title"].as_str().unwrap_or_default().to_string(),
        mergeable: node["state"].as_str() == Some("OPEN")
            && node["mergeable"].as_str() == Some("MERGEABLE"),
        opened_at: gh_time_ms(node, "createdAt"),
        merged_at: gh_time_ms(node, "mergedAt"),
    }
}

/// Fetch the current PR state for the branch checked out in `checkout`.
/// `Ok(None)` when the branch has no PR (or no token / non-GitHub origin).
pub async fn pr_view(checkout: &Path) -> Result<Option<PrState>> {
    let Some((owner, repo)) = repo_ref(checkout).await else {
        return Ok(None);
    };
    let Some(branch) = crate::git::current_branch(checkout).await? else {
        return Ok(None);
    };
    let query = branch_prs_query(PR_STATE_FIELDS);
    let Some(data) = graphql_opt(
        &query,
        json!({ "owner": owner, "repo": repo, "branch": branch }),
    )
    .await?
    else {
        return Ok(None);
    };
    Ok(pick_branch_pr(&branch_pr_nodes(&data)).map(parse_pr_state))
}

/// Fetch PR state by explicit PR number, regardless of the branch currently
/// checked out in `checkout`. This is the lookup that doesn't rely on branch
/// identity: once we've recorded a PR number for an agent we fetch by it, so a
/// recycled workspace/branch name can't resolve to a different (e.g. a prior
/// agent's merged) PR. `Ok(None)` when the PR can't be found.
///
/// `owner/repo` resolves from the checkout's origin, falling back to
/// `source_repo` (the repo the agent was spawned against — it shares the same
/// origin) when the checkout is broken or gone. A checkout casualty — a moved
/// root, a pruned linked worktree — must not sever a by-number lookup that
/// never needed the checkout's git state in the first place.
pub async fn pr_view_number(
    checkout: &Path,
    source_repo: Option<&Path>,
    number: u32,
) -> Result<Option<PrState>> {
    let Some((owner, repo)) = resolve_slug(checkout, source_repo).await else {
        return Ok(None);
    };
    let query = format!(
        r#"query($owner:String!,$repo:String!,$number:Int!){{
  repository(owner:$owner,name:$repo){{ pullRequest(number:$number){{ state {PR_STATE_FIELDS} }} }}
}}"#
    );
    let Some(data) = graphql_opt(
        &query,
        json!({ "owner": owner, "repo": repo, "number": number }),
    )
    .await?
    else {
        return Ok(None);
    };
    let node = &data["repository"]["pullRequest"];
    if node.is_null() {
        return Ok(None);
    }
    Ok(Some(parse_pr_state(node)))
}

/// List open PRs for the repo at `checkout` (newest first), for the
/// composer's "#" mention autocomplete. Empty when not connected.
pub async fn pr_list(checkout: &Path, limit: u32) -> Result<Vec<PrSummary>> {
    let Some((owner, repo)) = repo_ref(checkout).await else {
        return Ok(Vec::new());
    };
    let query = r#"query($owner:String!,$repo:String!,$limit:Int!){
  repository(owner:$owner,name:$repo){
    pullRequests(states:[OPEN], first:$limit, orderBy:{field:CREATED_AT,direction:DESC}){
      nodes{ number title state }
    }
  }
}"#;
    let Some(data) = graphql_opt(
        query,
        json!({ "owner": owner, "repo": repo, "limit": limit.min(100) }),
    )
    .await?
    else {
        return Ok(Vec::new());
    };
    Ok(branch_pr_nodes(&data)
        .iter()
        .map(|n| PrSummary {
            number: n["number"].as_u64().unwrap_or_default() as u32,
            title: n["title"].as_str().unwrap_or_default().to_string(),
            state: match n["state"].as_str() {
                Some("MERGED") => PrStatus::Merged,
                Some("CLOSED") => PrStatus::Closed,
                _ => PrStatus::Open,
            },
        })
        .collect())
}

// ---------------------------------------------------------------------------
// PR checks
// ---------------------------------------------------------------------------

/// GraphQL selection for the merge gate + per-check rollup on a PR node.
/// `startedAt: createdAt` aliases StatusContext's field to the name the parser
/// (shared with the CheckRun arm) expects. Reused by the branch lookup
/// (`pr_checks`) and the by-number batch (`pr_checks_batch`).
const PR_CHECKS_FIELDS: &str = r#"mergeStateStatus
           commits(last:1){nodes{commit{statusCheckRollup{contexts(first:100){nodes{
             __typename
             ... on CheckRun { name status conclusion detailsUrl startedAt completedAt }
             ... on StatusContext { context state targetUrl startedAt: createdAt }
           }}}}}}"#;

/// Extract [`PrChecks`] from a PR node carrying [`PR_CHECKS_FIELDS`].
fn pr_checks_from_node(pr: &Value) -> PrChecks {
    let merge_state = pr["mergeStateStatus"].as_str().unwrap_or("UNKNOWN").to_string();
    let rollup = pr["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    parse_pr_checks(&merge_state, &rollup)
}

/// Fetch the merge gate + per-check detail for the current branch's PR in one
/// GraphQL call. `Ok(None)` when there is no PR; other failures surface as
/// `Err` — the command layer treats both as "checks unavailable" and the
/// panel degrades to `mergeable`-only behavior.
pub async fn pr_checks(checkout: &Path) -> Result<Option<PrChecks>> {
    let Some((owner, repo)) = repo_ref(checkout).await else {
        return Ok(None);
    };
    let Some(branch) = crate::git::current_branch(checkout).await? else {
        return Ok(None);
    };
    let query = branch_prs_query(PR_CHECKS_FIELDS);
    let Some(data) = graphql_opt(
        &query,
        json!({ "owner": owner, "repo": repo, "branch": branch }),
    )
    .await?
    else {
        return Ok(None);
    };
    let nodes = branch_pr_nodes(&data);
    let Some(pr) = pick_branch_pr(&nodes) else {
        return Ok(None);
    };
    Ok(Some(pr_checks_from_node(pr)))
}

/// Normalize the UPPERCASE rollup payload into the spec §6 shape. Pure — unit
/// tested against captured fixtures.
fn parse_pr_checks(merge_state_status: &str, rollup: &[Value]) -> PrChecks {
    let merge_state = match merge_state_status {
        "CLEAN" => MergeState::Clean,
        "BLOCKED" => MergeState::Blocked,
        "UNSTABLE" => MergeState::Unstable,
        "BEHIND" => MergeState::Behind,
        "DIRTY" => MergeState::Dirty,
        "DRAFT" => MergeState::Draft,
        "HAS_HOOKS" => MergeState::HasHooks,
        _ => MergeState::Unknown,
    };

    let str_of = |v: &Value, key: &str| -> Option<String> {
        v.get(key)
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    };

    let runs: Vec<CheckRun> = rollup
        .iter()
        .map(|item| {
            if item["__typename"].as_str() == Some("StatusContext") {
                // Legacy commit status: a single `state` covers both status
                // and conclusion.
                let state = item["state"].as_str().unwrap_or("");
                let (status, conclusion) = match state {
                    "SUCCESS" => ("completed", Some("success")),
                    "FAILURE" | "ERROR" => ("completed", Some("failure")),
                    "EXPECTED" => ("queued", None),
                    _ => ("in_progress", None), // PENDING
                };
                CheckRun {
                    name: str_of(item, "context").unwrap_or_else(|| "status".into()),
                    status: status.to_string(),
                    conclusion: conclusion.map(|s| s.to_string()),
                    required: false,
                    url: str_of(item, "targetUrl"),
                    started_at: str_of(item, "startedAt"),
                    completed_at: None,
                }
            } else {
                CheckRun {
                    name: str_of(item, "name").unwrap_or_else(|| "check".into()),
                    status: item["status"].as_str().unwrap_or("QUEUED").to_lowercase(),
                    conclusion: str_of(item, "conclusion").map(|c| c.to_lowercase()),
                    required: false,
                    url: str_of(item, "detailsUrl"),
                    started_at: str_of(item, "startedAt"),
                    completed_at: str_of(item, "completedAt"),
                }
            }
        })
        .collect();

    let is_failing = |r: &CheckRun| {
        matches!(
            r.conclusion.as_deref(),
            Some("failure")
                | Some("timed_out")
                | Some("cancelled")
                | Some("action_required")
                | Some("startup_failure")
        )
    };
    let total = runs.len() as u32;
    let pending = runs.iter().filter(|r| r.status != "completed").count() as u32;
    let failed = runs.iter().filter(|r| is_failing(r)).count() as u32;
    // Computed directly, not by subtraction: the API can report a failure
    // conclusion on a not-yet-completed run (e.g. cancelled mid-run), which
    // would double-count into both `pending` and `failed` and underflow.
    let passed = runs.iter().filter(|r| r.status == "completed" && !is_failing(r)).count() as u32;
    let rollup_summary = if total == 0 {
        "none"
    } else if failed > 0 {
        "failing"
    } else if pending > 0 {
        "pending"
    } else {
        "passing"
    };
    let required_failing = runs.iter().filter(|r| is_failing(r)).map(|r| r.name.clone()).collect();

    PrChecks {
        merge_state,
        rollup: rollup_summary.to_string(),
        total,
        passed,
        failed,
        pending,
        required_failing,
        runs,
    }
}

// ---------------------------------------------------------------------------
// Batched multi-PR queries
//
// The app-wide polls (`refresh_all_pr_states`/`refresh_all_pr_checks`) used to
// fan one GraphQL request out per agent, concurrently — the burst that trips
// GitHub's secondary rate limit. Instead we collapse them into a single aliased
// query (`a0: repository(...){pullRequest(number:…){…}} a1: …`), chunked, so N
// agents cost ⌈N/50⌉ *sequential* requests rather than N concurrent ones.
// ---------------------------------------------------------------------------

/// One PR to look up by number in a batched query.
#[derive(Debug, Clone)]
pub(crate) struct PrRef {
    pub owner: String,
    pub repo: String,
    pub number: u32,
}

/// Max PRs per batched request — each adds a top-level `repository` alias, so
/// chunking keeps the query under GitHub's node/complexity limits.
const BATCH_CHUNK: usize = 50;

/// Build an aliased multi-PR query with a trailing `rateLimit` probe. Values
/// ride in as variables (`$oN/$rN/$nN`) so nothing user-derived is interpolated
/// into the query text.
fn build_batch_query(chunk: &[PrRef], inner_fields: &str) -> (String, Value) {
    let mut decls = Vec::with_capacity(chunk.len());
    let mut aliases = Vec::with_capacity(chunk.len());
    let mut vars = serde_json::Map::new();
    for (i, r) in chunk.iter().enumerate() {
        decls.push(format!("$o{i}:String!,$r{i}:String!,$n{i}:Int!"));
        aliases.push(format!(
            "a{i}:repository(owner:$o{i},name:$r{i}){{pullRequest(number:$n{i}){{{inner_fields}}}}}"
        ));
        vars.insert(format!("o{i}"), json!(r.owner));
        vars.insert(format!("r{i}"), json!(r.repo));
        vars.insert(format!("n{i}"), json!(r.number));
    }
    let query = format!(
        "query({}){{{} rateLimit{{cost remaining resetAt}}}}",
        decls.join(","),
        aliases.join(" "),
    );
    (query, Value::Object(vars))
}

/// Feed the queried `rateLimit` budget into the client's backoff gate.
fn note_budget(data: &Value) {
    let rl = &data["rateLimit"];
    if let Some(remaining) = rl["remaining"].as_i64() {
        let reset = rl["resetAt"]
            .as_str()
            .and_then(|iso| chrono::DateTime::parse_from_rfc3339(iso).ok())
            .map(|t| t.timestamp_millis());
        client::note_rate_budget(remaining, reset);
    }
}

/// Run `refs` through one or more batched queries, mapping each alias's
/// `pullRequest` node with `parse`. Results align 1:1 with `refs`; a
/// missing/inaccessible PR yields `None` for its slot (partial-error tolerant).
/// `Ok(vec![])` for empty input; an active backoff short-circuits to all-`None`
/// so callers fall back to the persisted snapshot without spending a request.
async fn pr_batch<T>(
    refs: &[PrRef],
    inner_fields: &str,
    parse: impl Fn(&Value) -> T,
) -> Result<Vec<Option<T>>> {
    if refs.is_empty() {
        return Ok(Vec::new());
    }
    if client::is_backing_off() {
        return Ok(refs.iter().map(|_| None).collect());
    }
    let client = client::Client::new()?;
    let mut out = Vec::with_capacity(refs.len());
    for chunk in refs.chunks(BATCH_CHUNK) {
        let (query, vars) = build_batch_query(chunk, inner_fields);
        let data = client.graphql_partial(&query, vars).await?;
        note_budget(&data);
        for i in 0..chunk.len() {
            let node = &data[format!("a{i}")]["pullRequest"];
            out.push((!node.is_null()).then(|| parse(node)));
        }
    }
    Ok(out)
}

/// Fetch PR state for many PRs by number in one (chunked) round-trip.
pub(crate) async fn pr_states_batch(refs: &[PrRef]) -> Result<Vec<Option<PrState>>> {
    pr_batch(refs, &format!("state {PR_STATE_FIELDS}"), parse_pr_state).await
}

/// Fetch the merge gate + checks for many PRs by number in one round-trip.
pub(crate) async fn pr_checks_batch(refs: &[PrRef]) -> Result<Vec<Option<PrChecks>>> {
    pr_batch(refs, PR_CHECKS_FIELDS, pr_checks_from_node).await
}

// ---------------------------------------------------------------------------
// PR review comments
// ---------------------------------------------------------------------------

/// Fetch the unresolved review threads for the current branch's PR — one
/// GraphQL call (threads inline with the branch-PR lookup; the gh version
/// needed two). `Ok(None)` when there is no PR; the command layer maps both
/// `None` and `Err` to "comments unavailable".
pub async fn pr_comments(checkout: &Path) -> Result<Option<PrComments>> {
    let Some((owner, repo)) = repo_ref(checkout).await else {
        return Ok(None);
    };
    let Some(branch) = crate::git::current_branch(checkout).await? else {
        return Ok(None);
    };
    let query = branch_prs_query(
        r#"reviewThreads(first:100){
             nodes{
               isResolved
               isOutdated
               comments(first:1){
                 totalCount
                 nodes{ author{ login __typename } body path line url }
               }
             }
           }"#,
    );
    let Some(data) = graphql_opt(
        &query,
        json!({ "owner": owner, "repo": repo, "branch": branch }),
    )
    .await?
    else {
        return Ok(None);
    };
    let nodes = branch_pr_nodes(&data);
    let Some(pr) = pick_branch_pr(&nodes) else {
        return Ok(None);
    };
    let threads = pr["reviewThreads"]["nodes"].as_array().cloned().unwrap_or_default();
    Ok(Some(PrComments {
        unresolved: parse_review_threads(&threads),
    }))
}

/// Flatten review-thread nodes into the root comment of each *unresolved,
/// non-outdated* thread. Pure — unit tested against captured fixtures.
fn parse_review_threads(nodes: &[Value]) -> Vec<PrComment> {
    nodes
        .iter()
        .filter(|t| {
            !t["isResolved"].as_bool().unwrap_or(false)
                && !t["isOutdated"].as_bool().unwrap_or(false)
        })
        .filter_map(|t| {
            let comments = &t["comments"];
            let root = comments["nodes"].get(0)?;
            let total = comments["totalCount"].as_u64().unwrap_or(1);
            Some(PrComment {
                author: root["author"]["login"].as_str().unwrap_or("unknown").to_string(),
                is_bot: root["author"]["__typename"].as_str() == Some("Bot"),
                body: root["body"].as_str().unwrap_or_default().to_string(),
                path: root["path"].as_str().filter(|s| !s.is_empty()).map(|s| s.to_string()),
                line: root["line"].as_u64().map(|n| n as u32),
                url: root["url"].as_str().unwrap_or_default().to_string(),
                replies: total.saturating_sub(1) as u32,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// PR create / merge
// ---------------------------------------------------------------------------

/// Whether a create failure means a PR for this branch already exists (the
/// REST 422 message is "A pull request already exists for owner:branch.").
/// Used to make `pr_create` idempotent across retries.
fn pr_already_exists(message: &str) -> bool {
    message.to_lowercase().contains("already exists")
}

/// Create a PR for the branch checked out in `checkout`. When `title` is
/// empty, the last commit's subject/body fill in (what gh's `--fill` did).
pub async fn pr_create(checkout: &Path, title: &str, body: &str, base: &str) -> Result<PrState> {
    let (owner, repo) = require_repo_ref(checkout).await?;
    let branch = require_current_branch(checkout, "pr create").await?;

    let (title, body) = if title.is_empty() {
        crate::git::last_commit_message(checkout).await?
    } else {
        (title.to_string(), body.to_string())
    };

    let client = client::Client::new()?;
    let (status, resp) = client
        .rest(
            reqwest::Method::POST,
            &format!("/repos/{owner}/{repo}/pulls"),
            Some(&json!({ "title": title, "body": body, "head": branch, "base": base })),
        )
        .await?;

    if !status.is_success() {
        let message = detailed_rest_error(&resp);
        // Idempotency: a prior attempt may have created the PR but failed
        // before we could fetch it. On retry GitHub reports the branch
        // already has a PR — treat that as success by returning the existing
        // one, so the caller isn't stuck erroring forever over a PR that's
        // actually there.
        if pr_already_exists(&message) {
            if let Some(pr) = pr_view(checkout).await? {
                return Ok(pr);
            }
        }
        return Err(Error::Gh(format!("pr create failed: {message}")));
    }

    // The create response carries no `mergeable` verdict yet (GitHub computes
    // it async) — fetch the same shape every other path returns.
    let number = resp["number"].as_u64().unwrap_or_default() as u32;
    match pr_view_number(checkout, None, number).await? {
        Some(pr) => Ok(pr),
        None => Err(Error::Gh("PR was created but could not be fetched".into())),
    }
}

/// REST error bodies often carry the actionable detail in `errors[]`, not
/// `message` (e.g. create's "A pull request already exists…"). Join both.
fn detailed_rest_error(body: &Value) -> String {
    let mut parts = vec![client::rest_error_message(body)];
    if let Some(errors) = body.get("errors").and_then(Value::as_array) {
        parts.extend(errors.iter().filter_map(|e| {
            e.get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
        }));
    }
    parts.retain(|p| !p.is_empty());
    parts.join("; ")
}

/// Merge the current branch's open PR: enable auto-merge (merge commit) so it
/// lands when checks pass — or merge immediately when GitHub says the PR is
/// already clean (the "clean status" refusal), matching `gh pr merge --auto`.
pub async fn pr_merge(checkout: &Path) -> Result<()> {
    let (owner, repo) = require_repo_ref(checkout).await?;
    let branch = require_current_branch(checkout, "pr merge").await?;

    let client = client::Client::new()?;
    let query = branch_prs_query("id");
    let data = client
        .graphql(&query, json!({ "owner": owner, "repo": repo, "branch": branch }))
        .await?;
    let nodes = branch_pr_nodes(&data);
    let id = pick_branch_pr(&nodes)
        .filter(|n| n["state"].as_str() == Some("OPEN"))
        .and_then(|n| n["id"].as_str())
        .ok_or_else(|| Error::Gh("no open PR for this branch".into()))?
        .to_string();

    let auto = client
        .graphql(
            r#"mutation($id:ID!){
  enablePullRequestAutoMerge(input:{pullRequestId:$id, mergeMethod:MERGE}){ clientMutationId }
}"#,
            json!({ "id": id }),
        )
        .await;
    match auto {
        Ok(_) => Ok(()),
        // "Pull request is in clean status" = nothing to wait for; merge now.
        Err(Error::Gh(msg)) if msg.to_lowercase().contains("clean status") => {
            client
                .graphql(
                    r#"mutation($id:ID!){
  mergePullRequest(input:{pullRequestId:$id, mergeMethod:MERGE}){ clientMutationId }
}"#,
                    json!({ "id": id }),
                )
                .await?;
            Ok(())
        }
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Account / discovery (used by the New Project flow)
// ---------------------------------------------------------------------------

/// GitHub connection probe. Never errors — no token, an invalid token, and a
/// network failure all report as not-authenticated fields, not failures.
pub async fn auth_status() -> Result<GhStatus> {
    let Ok(client) = client::Client::new() else {
        return Ok(GhStatus { installed: true, authenticated: false, login: None });
    };
    match client.graphql("query{viewer{login}}", json!({})).await {
        Ok(data) => {
            let login = data["viewer"]["login"].as_str().map(str::to_string);
            Ok(GhStatus { installed: true, authenticated: login.is_some(), login })
        }
        Err(_) => Ok(GhStatus { installed: true, authenticated: false, login: None }),
    }
}

/// List repos the user can clone (most-recently-updated first): owned,
/// collaborator, and org repos. The New Project picker filters client-side.
pub async fn repo_list(limit: u32) -> Result<Vec<GhRepoSummary>> {
    let client = client::Client::new()?;
    let query = r#"query($first:Int!,$after:String){
  viewer{
    repositories(first:$first, after:$after,
                 orderBy:{field:UPDATED_AT,direction:DESC},
                 affiliations:[OWNER,COLLABORATOR,ORGANIZATION_MEMBER]){
      nodes{ nameWithOwner description isPrivate updatedAt }
      pageInfo{ hasNextPage endCursor }
    }
  }
}"#;
    let mut repos = Vec::new();
    let mut cursor: Option<String> = None;
    while (repos.len() as u32) < limit {
        let first = (limit - repos.len() as u32).min(100);
        let data = client
            .graphql(query, json!({ "first": first, "after": cursor }))
            .await?;
        let page = &data["viewer"]["repositories"];
        for n in page["nodes"].as_array().cloned().unwrap_or_default() {
            repos.push(GhRepoSummary {
                name_with_owner: n["nameWithOwner"].as_str().unwrap_or_default().to_string(),
                description: n["description"]
                    .as_str()
                    .filter(|d| !d.is_empty())
                    .map(str::to_string),
                is_private: n["isPrivate"].as_bool().unwrap_or(false),
                updated_at: n["updatedAt"].as_str().unwrap_or_default().to_string(),
            });
        }
        if !page["pageInfo"]["hasNextPage"].as_bool().unwrap_or(false) {
            break;
        }
        cursor = page["pageInfo"]["endCursor"].as_str().map(str::to_string);
    }
    Ok(repos)
}

/// The URL `git clone` should use for a repo spec: `owner/repo` becomes an
/// https github.com URL (authenticated via `git_auth_env`); full https/ssh
/// URLs pass through untouched (ssh uses the user's own keys).
fn clone_url(spec: &str) -> String {
    if spec.contains("://") || spec.starts_with("git@") {
        spec.to_string()
    } else {
        format!("https://github.com/{}.git", spec.trim_end_matches(".git"))
    }
}

/// Clone `spec` (an `owner/repo`, an https URL, or an ssh URL) into `target`.
/// No timeout — a large repo can legitimately take minutes; the caller
/// (`new_project::clone`) self-heals a wedged partial clone by removing it.
pub async fn repo_clone(spec: &str, target: &Path) -> Result<()> {
    let target = target
        .to_str()
        .ok_or_else(|| Error::InvalidPath(target.display().to_string()))?;
    let mut cmd = crate::git_dist::bare_command();
    cmd.args(["clone", &clone_url(spec), target]);
    for (k, v) in client::git_auth_env() {
        cmd.env(k, v);
    }
    let out = cmd.output().await?;
    if !out.status.success() {
        return Err(Error::Gh(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(())
}

/// Create a GitHub repo from the existing git repo at `target` and push its
/// current branch. `target` must already be a git repo with at least one
/// commit. Serves both the New Project create flow and the git panel's
/// "Publish to GitHub" for a local-only project. Returns the repo's web URL.
pub async fn repo_create_and_push(
    target: &Path,
    name: &str,
    private: bool,
    description: Option<&str>,
) -> Result<String> {
    let client = client::Client::new()?;
    let mut body = json!({ "name": name, "private": private });
    if let Some(desc) = description.filter(|d| !d.is_empty()) {
        body["description"] = json!(desc);
    }
    let (status, resp) = client
        .rest(reqwest::Method::POST, "/user/repos", Some(&body))
        .await?;
    if !status.is_success() {
        return Err(Error::Gh(format!(
            "repo create failed: {}",
            detailed_rest_error(&resp),
        )));
    }
    let full_name = resp["full_name"]
        .as_str()
        .ok_or_else(|| Error::Gh("repo created but response had no full_name".into()))?;

    crate::git::remote_add(target, "origin", &format!("https://github.com/{full_name}.git"))
        .await?;
    let branch = require_current_branch(target, "publish").await?;
    crate::git::push(target, &branch).await?;
    Ok(format!("https://github.com/{full_name}"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn pr_node(state: &str, number: u32, mergeable: &str) -> Value {
        json!({
            "number": number,
            "url": format!("https://github.com/o/r/pull/{number}"),
            "state": state,
            "title": format!("PR {number}"),
            "mergeable": mergeable,
        })
    }

    #[test]
    fn pr_state_open_mergeable() {
        let pr = parse_pr_state(&pr_node("OPEN", 42, "MERGEABLE"));
        assert!(matches!(pr.state, PrStatus::Open));
        assert!(pr.mergeable);
        assert_eq!(pr.number, 42);
    }

    #[test]
    fn pr_state_merged_and_closed_are_never_mergeable() {
        let merged = parse_pr_state(&pr_node("MERGED", 1, "MERGEABLE"));
        assert!(matches!(merged.state, PrStatus::Merged));
        assert!(!merged.mergeable);
        let closed = parse_pr_state(&pr_node("CLOSED", 2, "UNKNOWN"));
        assert!(matches!(closed.state, PrStatus::Closed));
        assert!(!closed.mergeable);
    }

    #[test]
    fn pr_state_unknown_state_defaults_to_open() {
        let pr = parse_pr_state(&pr_node("SOMETHING_NEW", 3, "MERGEABLE"));
        assert!(matches!(pr.state, PrStatus::Open));
    }

    /// The branch→PR pick prefers the newest open PR, falling back to the
    /// newest of any state — a merged PR must still resolve (session adoption
    /// depends on seeing MERGED, not "no PR").
    #[test]
    fn picks_open_pr_over_newer_closed_and_falls_back_to_newest() {
        let nodes = vec![
            pr_node("CLOSED", 9, "UNKNOWN"),
            pr_node("OPEN", 7, "MERGEABLE"),
            pr_node("MERGED", 5, "UNKNOWN"),
        ];
        let picked = pick_branch_pr(&nodes).unwrap();
        assert_eq!(picked["number"].as_u64(), Some(7));

        let no_open = vec![pr_node("MERGED", 9, "UNKNOWN"), pr_node("CLOSED", 5, "UNKNOWN")];
        assert_eq!(pick_branch_pr(&no_open).unwrap()["number"].as_u64(), Some(9));
        assert!(pick_branch_pr(&[]).is_none());
    }

    #[test]
    fn batch_query_builds_aliases_and_variables() {
        let refs = vec![
            PrRef { owner: "acme".into(), repo: "web".into(), number: 7 },
            PrRef { owner: "acme".into(), repo: "api".into(), number: 12 },
        ];
        let (query, vars) = build_batch_query(&refs, "state number");
        // One aliased repository/pullRequest per ref, values via variables.
        assert!(query.contains("a0:repository(owner:$o0,name:$r0)"), "{query}");
        assert!(query.contains("a1:repository(owner:$o1,name:$r1)"), "{query}");
        assert!(query.contains("pullRequest(number:$n1)"), "{query}");
        // The budget probe rides along on every batch.
        assert!(query.contains("rateLimit"), "{query}");
        assert_eq!(vars["o0"], json!("acme"));
        assert_eq!(vars["r1"], json!("api"));
        assert_eq!(vars["n1"], json!(12));
    }

    #[test]
    fn detects_already_exists_failure() {
        // GitHub's real 422 message for a duplicate PR.
        assert!(pr_already_exists("A pull request already exists for fwdai:feat."));
        // An unrelated failure must not be mistaken for it.
        assert!(!pr_already_exists("Validation Failed"));
    }

    #[test]
    fn clone_url_forms() {
        assert_eq!(clone_url("fwdai/fletch"), "https://github.com/fwdai/fletch.git");
        assert_eq!(
            clone_url("https://github.com/fwdai/fletch.git"),
            "https://github.com/fwdai/fletch.git",
        );
        assert_eq!(
            clone_url("git@github.com:fwdai/fletch.git"),
            "git@github.com:fwdai/fletch.git",
        );
    }

    fn rollup_fixture() -> Vec<Value> {
        serde_json::from_str(
            r#"[
              {"__typename":"CheckRun","name":"build","status":"COMPLETED","conclusion":"SUCCESS",
               "detailsUrl":"https://ci/build","startedAt":"2026-06-10T00:00:00Z","completedAt":"2026-06-10T00:05:00Z"},
              {"__typename":"CheckRun","name":"test","status":"COMPLETED","conclusion":"FAILURE",
               "detailsUrl":"https://ci/test","startedAt":"2026-06-10T00:00:00Z","completedAt":"2026-06-10T00:07:00Z"},
              {"__typename":"CheckRun","name":"lint","status":"IN_PROGRESS","conclusion":null,
               "detailsUrl":null,"startedAt":"2026-06-10T00:00:00Z","completedAt":null},
              {"__typename":"StatusContext","context":"ci/legacy","state":"SUCCESS","targetUrl":"https://ci/legacy"}
            ]"#,
        )
        .unwrap()
    }

    #[test]
    fn pr_checks_normalizes_runs_and_counts() {
        let checks = parse_pr_checks("BLOCKED", &rollup_fixture());
        assert!(matches!(checks.merge_state, MergeState::Blocked));
        assert_eq!(checks.total, 4);
        assert_eq!(checks.passed, 2); // build + legacy status context
        assert_eq!(checks.failed, 1); // test
        assert_eq!(checks.pending, 1); // lint
        assert_eq!(checks.rollup, "failing");
        assert_eq!(checks.required_failing, vec!["test".to_string()]);
        let lint = checks.runs.iter().find(|r| r.name == "lint").unwrap();
        assert_eq!(lint.status, "in_progress");
        assert_eq!(lint.conclusion, None);
        let legacy = checks.runs.iter().find(|r| r.name == "ci/legacy").unwrap();
        assert_eq!(legacy.status, "completed");
        assert_eq!(legacy.conclusion.as_deref(), Some("success"));
        assert_eq!(legacy.url.as_deref(), Some("https://ci/legacy"));
    }

    #[test]
    fn pr_checks_rollup_states() {
        // No checks at all.
        let none = parse_pr_checks("CLEAN", &[]);
        assert_eq!(none.rollup, "none");
        assert!(matches!(none.merge_state, MergeState::Clean));
        // All passing.
        let passing: Vec<Value> = serde_json::from_str(
            r#"[{"__typename":"CheckRun","name":"build","status":"COMPLETED","conclusion":"SUCCESS"}]"#,
        )
        .unwrap();
        assert_eq!(parse_pr_checks("CLEAN", &passing).rollup, "passing");
        // Pending (no failures yet).
        let pending: Vec<Value> = serde_json::from_str(
            r#"[{"__typename":"CheckRun","name":"build","status":"QUEUED","conclusion":null}]"#,
        )
        .unwrap();
        assert_eq!(parse_pr_checks("UNKNOWN", &pending).rollup, "pending");
    }

    #[test]
    fn pr_checks_tolerates_failing_conclusion_on_incomplete_run() {
        // A cancelled-while-running check can surface as IN_PROGRESS with a
        // failure conclusion. It must count as failed (and pending) without
        // `passed` underflowing.
        let rollup: Vec<Value> = serde_json::from_str(
            r#"[{"__typename":"CheckRun","name":"build","status":"IN_PROGRESS","conclusion":"CANCELLED"}]"#,
        )
        .unwrap();
        let checks = parse_pr_checks("UNKNOWN", &rollup);
        assert_eq!(checks.total, 1);
        assert_eq!(checks.failed, 1);
        assert_eq!(checks.pending, 1);
        assert_eq!(checks.passed, 0);
        assert_eq!(checks.rollup, "failing");
    }

    #[test]
    fn pr_checks_merge_state_mapping() {
        for (raw, want) in [
            ("CLEAN", MergeState::Clean),
            ("BLOCKED", MergeState::Blocked),
            ("UNSTABLE", MergeState::Unstable),
            ("BEHIND", MergeState::Behind),
            ("DIRTY", MergeState::Dirty),
            ("DRAFT", MergeState::Draft),
            ("HAS_HOOKS", MergeState::HasHooks),
            ("UNKNOWN", MergeState::Unknown),
            ("SOMETHING_NEW", MergeState::Unknown),
        ] {
            let got = parse_pr_checks(raw, &[]).merge_state;
            assert_eq!(got, want, "for {raw}");
        }
    }

    fn review_threads_fixture() -> Vec<Value> {
        serde_json::from_str(
            r#"[
              {"isResolved":false,"isOutdated":false,"comments":{"totalCount":1,"nodes":[
                {"author":{"login":"greptileai","__typename":"Bot"},
                 "body":"Consider handling the null case here.",
                 "path":"src/foo.rs","line":42,
                 "url":"https://github.com/o/r/pull/1#discussion_r1"}]}},
              {"isResolved":false,"isOutdated":false,"comments":{"totalCount":3,"nodes":[
                {"author":{"login":"alice","__typename":"User"},
                 "body":"Can we rename this?",
                 "path":"src/bar.rs","line":7,
                 "url":"https://github.com/o/r/pull/1#discussion_r2"}]}},
              {"isResolved":true,"isOutdated":false,"comments":{"totalCount":1,"nodes":[
                {"author":{"login":"bob","__typename":"User"},"body":"resolved one",
                 "path":"src/baz.rs","line":1,"url":"u3"}]}},
              {"isResolved":false,"isOutdated":true,"comments":{"totalCount":1,"nodes":[
                {"author":{"login":"carol","__typename":"User"},"body":"stale one",
                 "path":"src/qux.rs","line":1,"url":"u4"}]}},
              {"isResolved":false,"isOutdated":false,"comments":{"totalCount":1,"nodes":[
                {"author":{"login":"dave","__typename":"User"},"body":"unanchored",
                 "path":null,"line":null,"url":"u5"}]}}
            ]"#,
        )
        .unwrap()
    }

    #[test]
    fn review_threads_keep_only_unresolved_active() {
        let comments = parse_review_threads(&review_threads_fixture());
        // Resolved + outdated dropped; 3 remain (greptile, alice, dave).
        assert_eq!(comments.len(), 3);
        assert!(comments.iter().all(|c| c.author != "bob" && c.author != "carol"));
    }

    #[test]
    fn review_threads_flag_bots_and_count_replies() {
        let comments = parse_review_threads(&review_threads_fixture());
        let greptile = comments.iter().find(|c| c.author == "greptileai").unwrap();
        assert!(greptile.is_bot);
        assert_eq!(greptile.replies, 0);
        assert_eq!(greptile.path.as_deref(), Some("src/foo.rs"));
        assert_eq!(greptile.line, Some(42));

        let alice = comments.iter().find(|c| c.author == "alice").unwrap();
        assert!(!alice.is_bot);
        assert_eq!(alice.replies, 2); // totalCount 3 − root
    }

    #[test]
    fn review_threads_tolerate_null_anchor() {
        let comments = parse_review_threads(&review_threads_fixture());
        let dave = comments.iter().find(|c| c.author == "dave").unwrap();
        assert_eq!(dave.path, None);
        assert_eq!(dave.line, None);
    }

    /// Live read-path check against the real GitHub API, using this repo's
    /// own checkout as the fixture. Read-only — never creates or merges
    /// anything. Ignored by default (needs a token + network); run with:
    ///
    ///   FLETCH_GITHUB_TOKEN=$(gh auth token) cargo test github_live -- --ignored
    #[tokio::test]
    #[ignore]
    // The guard must span the awaits — that's its purpose (no other test may
    // touch the token registry while live calls run). Single test, no
    // re-entrancy, so the lint's deadlock scenario can't occur.
    #[allow(clippy::await_holding_lock)]
    async fn github_live_read_ops() {
        let _guard = client::test_token_lock();
        let token =
            std::env::var("FLETCH_GITHUB_TOKEN").expect("set FLETCH_GITHUB_TOKEN to a token");
        client::set_token(Some(token));
        // cargo test runs in src-tauri; the repo root is one up.
        let repo = std::env::current_dir().unwrap().parent().unwrap().to_path_buf();

        let status = auth_status().await.unwrap();
        assert!(status.authenticated, "token must authenticate");
        assert!(status.login.is_some(), "viewer login must resolve");

        let repos = repo_list(3).await.unwrap();
        assert!(!repos.is_empty(), "repo list must return something");
        assert!(repos.iter().all(|r| r.name_with_owner.contains('/')));

        // Branch-PR resolution against whatever branch is checked out: a PR
        // (if one exists) must parse into a coherent state, and the heavier
        // checks/comments lookups for the same branch must not error.
        if let Some(pr) = pr_view(&repo).await.unwrap() {
            assert!(pr.number > 0);
            assert!(pr.url.starts_with("https://github.com/"));
            let by_number = pr_view_number(&repo, None, pr.number).await.unwrap().unwrap();
            assert_eq!(by_number.number, pr.number);
            let checks = pr_checks(&repo).await.unwrap();
            assert!(checks.is_some(), "checks must resolve for an existing PR");
            let _ = pr_comments(&repo).await.unwrap();
        }

        // A PR number that can't exist maps to None, not an error.
        assert!(pr_view_number(&repo, None, 999_999_999).await.unwrap().is_none());

        let prs = pr_list(&repo, 5).await.unwrap();
        assert!(prs.len() <= 5);

        client::set_token(None);
    }
}
