//! Thin wrapper around the `gh` CLI for GitHub PR operations.
//!
//! Follows the same subprocess pattern as `git.rs` — each function
//! shells out to `gh` and maps exit-code / stderr to typed errors.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tokio::process::Command;

use crate::error::{Error, Result};

/// A `Command` for the `gh` CLI, resolved to an absolute path.
///
/// A GUI app launched from Finder/Dock inherits launchd's minimal PATH, which
/// omits Homebrew (`/opt/homebrew/bin`) where `gh` is installed — so a bare
/// `Command::new("gh")` fails with ENOENT ("No such file or directory"). We
/// resolve the real path once (it may spawn a login shell) and cache it. If
/// `gh` genuinely isn't installed anywhere we fall back to the bare name, so
/// the same not-found error still surfaces (e.g. `auth_status` reports it as
/// not installed).
fn gh_command() -> Command {
    static GH_PATH: OnceLock<String> = OnceLock::new();
    let path = GH_PATH.get_or_init(|| {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        crate::bin_resolve::resolve_bin("gh", &home).unwrap_or_else(|| "gh".to_string())
    });
    let mut cmd = Command::new(path);
    if let Some(env) = crate::bin_resolve::login_shell_env() {
        for (k, v) in env {
            cmd.env(k, v);
        }
    }
    cmd
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrStatus {
    Open,
    Merged,
    Closed,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PrState {
    pub number: u32,
    pub url: String,
    pub state: PrStatus,
    pub title: String,
    pub mergeable: bool,
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

/// One CI check, normalized from gh's `statusCheckRollup` (which mixes
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

// ---------------------------------------------------------------------------
// Internal deserialization helpers
// ---------------------------------------------------------------------------

/// Raw shape returned by `gh pr view --json ...`. `gh` uses uppercase
/// strings for both `state` and `mergeable`.
#[derive(serde::Deserialize)]
struct GhPrRaw {
    number: u32,
    url: String,
    state: String,     // "OPEN" | "MERGED" | "CLOSED"
    title: String,
    mergeable: String, // "MERGEABLE" | "CONFLICTING" | "UNKNOWN"
}

impl From<GhPrRaw> for PrState {
    fn from(raw: GhPrRaw) -> Self {
        PrState {
            number: raw.number,
            url: raw.url,
            state: match raw.state.as_str() {
                "MERGED" => PrStatus::Merged,
                "CLOSED" => PrStatus::Closed,
                _ => PrStatus::Open,
            },
            title: raw.title,
            mergeable: raw.mergeable == "MERGEABLE",
        }
    }
}

// ---------------------------------------------------------------------------
// Public async functions
// ---------------------------------------------------------------------------

/// Fetch the current PR state for the branch checked out in `worktree`.
///
/// Returns `Ok(None)` when `gh` exits non-zero with "no pull requests found"
/// in stderr (i.e. the branch simply has no open PR yet).
pub async fn pr_view(worktree: &Path) -> Result<Option<PrState>> {
    let out = gh_command()
        .current_dir(worktree)
        .args(["pr", "view", "--json", "number,url,state,title,mergeable"])
        .output()
        .await?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.to_lowercase().contains("no pull requests found") {
            return Ok(None);
        }
        return Err(Error::Gh(stderr.trim().to_string()));
    }

    let raw: GhPrRaw = serde_json::from_slice(&out.stdout)?;
    Ok(Some(raw.into()))
}

/// Fetch PR state by explicit PR number, regardless of the branch currently
/// checked out in `worktree`. This is the lookup that doesn't rely on branch
/// identity: once we've recorded a PR number for an agent we fetch by it, so a
/// recycled workspace/branch name can't resolve to a different (e.g. a prior
/// agent's merged) PR.
///
/// Returns `Ok(None)` when the PR can't be found (e.g. it was deleted) so the
/// caller can treat it the same as "no PR".
pub async fn pr_view_number(worktree: &Path, number: u32) -> Result<Option<PrState>> {
    let num = number.to_string();
    let out = gh_command()
        .current_dir(worktree)
        .args(["pr", "view", &num, "--json", "number,url,state,title,mergeable"])
        .output()
        .await?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let low = stderr.to_lowercase();
        // gh's "missing PR" wording varies by lookup and version: branch lookups
        // say "no pull requests found", a bad number says "pull request not
        // found" or "could not resolve to a PullRequest". `contains("not found")`
        // covers the first two; keep the resolve case explicit. All map to the
        // documented `Ok(None)` so callers treat it as "no PR".
        if low.contains("not found") || low.contains("could not resolve") {
            return Ok(None);
        }
        return Err(Error::Gh(stderr.trim().to_string()));
    }

    let raw: GhPrRaw = serde_json::from_slice(&out.stdout)?;
    Ok(Some(raw.into()))
}

/// List open PRs for the repo at `worktree` (most-recent first), for the
/// composer's "#" mention autocomplete.
pub async fn pr_list(worktree: &Path, limit: u32) -> Result<Vec<PrSummary>> {
    let out = gh_command()
        .current_dir(worktree)
        .args([
            "pr",
            "list",
            "--state",
            "open",
            "--json",
            "number,title,state",
            "--limit",
            &limit.to_string(),
        ])
        .output()
        .await?;

    if !out.status.success() {
        return Err(Error::Gh(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }

    #[derive(serde::Deserialize)]
    struct Raw {
        number: u32,
        title: String,
        state: String,
    }
    let raw: Vec<Raw> = serde_json::from_slice(&out.stdout)?;
    Ok(raw
        .into_iter()
        .map(|r| PrSummary {
            number: r.number,
            title: r.title,
            state: match r.state.as_str() {
                "MERGED" => PrStatus::Merged,
                "CLOSED" => PrStatus::Closed,
                _ => PrStatus::Open,
            },
        })
        .collect())
}

/// Fetch the merge gate + per-check detail for the current branch's PR.
/// One `gh pr view` call. Returns `Ok(None)` when there is no PR; other gh
/// failures surface as `Err` — the command layer treats both as "checks
/// unavailable" and the panel degrades to `mergeable`-only behavior.
pub async fn pr_checks(worktree: &Path) -> Result<Option<PrChecks>> {
    let out = gh_command()
        .current_dir(worktree)
        .args(["pr", "view", "--json", "mergeStateStatus,statusCheckRollup"])
        .output()
        .await?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.to_lowercase().contains("no pull requests found") {
            return Ok(None);
        }
        return Err(Error::Gh(stderr.trim().to_string()));
    }

    let raw: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let merge_state = raw["mergeStateStatus"].as_str().unwrap_or("UNKNOWN").to_string();
    let rollup: Vec<serde_json::Value> = raw["statusCheckRollup"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Ok(Some(parse_pr_checks(&merge_state, &rollup)))
}

/// Normalize gh's UPPERCASE payload into the spec §6 shape. Pure — unit
/// tested against captured fixtures.
fn parse_pr_checks(merge_state_status: &str, rollup: &[serde_json::Value]) -> PrChecks {
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

    let str_of = |v: &serde_json::Value, key: &str| -> Option<String> {
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
    // Computed directly, not by subtraction: gh can report a failure
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

/// GraphQL query fetching a PR's review threads with resolution state. REST's
/// `/pulls/{n}/comments` does not expose `isResolved`/`isOutdated`, which we
/// need to keep only the actionable (unresolved) threads — so we use GraphQL.
const REVIEW_THREADS_QUERY: &str = r#"
query($owner:String!,$repo:String!,$number:Int!){
  repository(owner:$owner,name:$repo){
    pullRequest(number:$number){
      reviewThreads(first:100){
        nodes{
          isResolved
          isOutdated
          comments(first:1){
            totalCount
            nodes{ author{ login __typename } body path line url }
          }
        }
      }
    }
  }
}
"#;

/// Owner + name parsed from a PR's HTML URL
/// (`https://github.com/OWNER/REPO/pull/N`). The PR lives in the base repo
/// the URL points at, so this is correct for forked-branch PRs too.
fn parse_repo_from_url(url: &str) -> Option<(String, String)> {
    let rest = url.split("github.com/").nth(1)?;
    let mut parts = rest.split('/');
    let owner = parts.next().filter(|s| !s.is_empty())?;
    let repo = parts.next().filter(|s| !s.is_empty())?;
    Some((owner.to_string(), repo.to_string()))
}

/// Fetch the unresolved review threads for the current branch's PR.
///
/// Two `gh` calls: `gh pr view` to resolve the PR's repo + number from the
/// worktree, then one `gh api graphql` for the threads. Returns `Ok(None)`
/// when there is no PR; other gh failures surface as `Err` — the command
/// layer maps both to "comments unavailable" and the panel simply omits the
/// section.
pub async fn pr_comments(worktree: &Path) -> Result<Option<PrComments>> {
    let out = gh_command()
        .current_dir(worktree)
        .args(["pr", "view", "--json", "url,number"])
        .output()
        .await?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.to_lowercase().contains("no pull requests found") {
            return Ok(None);
        }
        return Err(Error::Gh(stderr.trim().to_string()));
    }

    let raw: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let url = raw["url"].as_str().unwrap_or_default();
    let number = raw["number"].as_u64().unwrap_or_default();
    let Some((owner, repo)) = parse_repo_from_url(url) else {
        return Ok(None);
    };

    let gql = gh_command()
        .current_dir(worktree)
        .args([
            "api",
            "graphql",
            "-f",
            &format!("query={REVIEW_THREADS_QUERY}"),
            "-F",
            &format!("owner={owner}"),
            "-F",
            &format!("repo={repo}"),
            "-F",
            &format!("number={number}"),
        ])
        .output()
        .await?;
    if !gql.status.success() {
        return Err(Error::Gh(
            String::from_utf8_lossy(&gql.stderr).trim().to_string(),
        ));
    }

    let data: serde_json::Value = serde_json::from_slice(&gql.stdout)?;
    let nodes = data["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Ok(Some(PrComments {
        unresolved: parse_review_threads(&nodes),
    }))
}

/// Flatten review-thread nodes into the root comment of each *unresolved,
/// non-outdated* thread. Pure — unit tested against captured fixtures.
fn parse_review_threads(nodes: &[serde_json::Value]) -> Vec<PrComment> {
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

/// Create a PR for the branch checked out in `worktree`.
///
/// If `title` is empty the `--fill` flag is used so `gh` auto-fills the
/// title and body from the commit log. Otherwise `--title` / `--body` are
/// passed explicitly.
///
/// `gh pr create` does not support `--json`, so we run it for its side-effect
/// (creating the PR) and then call `pr_view` to fetch the full `PrState`.
/// Whether a `gh pr create` failure means a PR for this branch already exists
/// (gh's message is "a pull request for branch ... already exists"). Used to
/// make `pr_create` idempotent across retries.
fn pr_already_exists(stderr: &str) -> bool {
    stderr.to_lowercase().contains("already exists")
}

pub async fn pr_create(worktree: &Path, title: &str, body: &str, base: &str) -> Result<PrState> {
    let mut args = vec!["pr", "create", "--base", base];
    if title.is_empty() {
        args.push("--fill");
    } else {
        args.extend_from_slice(&["--title", title, "--body", body]);
    }

    let mut cmd = gh_command();
    cmd.current_dir(worktree).args(&args);
    let out = crate::git::output_timed(&mut cmd, "gh pr create").await?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        // Idempotency: a prior attempt may have created the PR but failed
        // before we could fetch it (a transient `pr_view` error returns
        // `Err` even though the PR exists). On retry `gh pr create` reports
        // the branch already has a PR — treat that as success by returning the
        // existing one, so the caller isn't stuck erroring forever over a PR
        // that's actually there.
        if pr_already_exists(&stderr) {
            if let Some(pr) = pr_view(worktree).await? {
                return Ok(pr);
            }
        }
        return Err(Error::Gh(stderr.trim().to_string()));
    }

    // `gh pr create` only prints the PR URL on success; fetch full state.
    pr_view(worktree).await?.ok_or_else(|| {
        Error::Gh("PR was created but could not be fetched".into())
    })
}

/// Merge the open PR for the branch checked out in `worktree` using a merge
/// commit and the `--auto` flag (merges as soon as all checks pass).
pub async fn pr_merge(worktree: &Path) -> Result<()> {
    let mut cmd = gh_command();
    cmd.current_dir(worktree).args(["pr", "merge", "--merge", "--auto"]);
    let out = crate::git::output_timed(&mut cmd, "gh pr merge").await?;

    if !out.status.success() {
        return Err(Error::Gh(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Account / discovery (used by the New Project flow)
// ---------------------------------------------------------------------------

/// Whether `gh` is installed and authenticated. Drives the New Project UI:
/// clone and create both go through `gh`, so we surface a clear prompt up
/// front instead of letting an operation fail half-way.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GhStatus {
    pub installed: bool,
    pub authenticated: bool,
    pub login: Option<String>,
}

/// One repo as returned by `gh repo list --json`. `gh` emits camelCase keys;
/// we re-expose the snake_case shape the rest of the IPC surface uses.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GhRepoSummary {
    pub name_with_owner: String,
    pub description: Option<String>,
    pub is_private: bool,
    pub updated_at: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhRepoRaw {
    name_with_owner: String,
    description: Option<String>,
    is_private: bool,
    updated_at: String,
}

impl From<GhRepoRaw> for GhRepoSummary {
    fn from(r: GhRepoRaw) -> Self {
        GhRepoSummary {
            name_with_owner: r.name_with_owner,
            // `gh` returns "" rather than null for an empty description.
            description: r.description.filter(|d| !d.is_empty()),
            is_private: r.is_private,
            updated_at: r.updated_at,
        }
    }
}

/// Probe `gh` availability and auth. Never errors — a missing binary or a
/// logged-out state are reported as fields, not failures.
pub async fn auth_status() -> Result<GhStatus> {
    let out = match gh_command().args(["auth", "status"]).output().await {
        Ok(out) => out,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Ok(GhStatus { installed: false, authenticated: false, login: None });
        }
        Err(e) => return Err(e.into()),
    };

    if !out.status.success() {
        // `gh` is installed but no account is logged in.
        return Ok(GhStatus { installed: true, authenticated: false, login: None });
    }

    // Best-effort login name; never fail the whole probe on it.
    let login = gh_command()
        .args(["api", "user", "--jq", ".login"])
        .output()
        .await
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());

    Ok(GhStatus { installed: true, authenticated: true, login })
}

/// List the authenticated user's repos (most-recently-updated first). The
/// New Project picker filters this list client-side.
pub async fn repo_list(limit: u32) -> Result<Vec<GhRepoSummary>> {
    let out = gh_command()
        .args([
            "repo",
            "list",
            "--json",
            "nameWithOwner,description,isPrivate,updatedAt",
            "--limit",
            &limit.to_string(),
        ])
        .output()
        .await?;

    if !out.status.success() {
        return Err(Error::Gh(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }

    let raw: Vec<GhRepoRaw> = serde_json::from_slice(&out.stdout)?;
    Ok(raw.into_iter().map(Into::into).collect())
}

/// Clone `spec` (an `owner/repo`, an https URL, or an ssh URL — `gh` accepts
/// all three) into `target`.
pub async fn repo_clone(spec: &str, target: &Path) -> Result<()> {
    let target = target.to_str().ok_or_else(|| {
        Error::InvalidPath(target.display().to_string())
    })?;
    let out = gh_command()
        .args(["repo", "clone", spec, target])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Gh(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(())
}

/// Create a GitHub repo from the existing git repo at `target` and push the
/// initial commit. `target` must already be a git repo with at least one
/// commit (see `new_project::create`).
pub async fn repo_create_and_push(
    target: &Path,
    name: &str,
    private: bool,
    description: Option<&str>,
) -> Result<()> {
    let mut args = vec![
        "repo".to_string(),
        "create".to_string(),
        name.to_string(),
        if private { "--private".to_string() } else { "--public".to_string() },
        "--source=.".to_string(),
        "--remote=origin".to_string(),
        "--push".to_string(),
    ];
    if let Some(desc) = description.filter(|d| !d.is_empty()) {
        args.push("--description".to_string());
        args.push(desc.to_string());
    }

    let out = gh_command()
        .current_dir(target)
        .args(&args)
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Gh(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_raw_open_mergeable() {
        let raw = GhPrRaw {
            number: 42,
            url: "https://github.com/owner/repo/pull/42".into(),
            state: "OPEN".into(),
            title: "My PR".into(),
            mergeable: "MERGEABLE".into(),
        };
        let pr = PrState::from(raw);
        assert!(matches!(pr.state, PrStatus::Open));
        assert!(pr.mergeable);
        assert_eq!(pr.number, 42);
    }

    #[test]
    fn detects_already_exists_failure() {
        // gh's real message for a duplicate PR.
        assert!(pr_already_exists(
            "a pull request for branch \"feat\" into branch \"main\" already exists:\nhttps://github.com/o/r/pull/7"
        ));
        // An unrelated failure must not be mistaken for it.
        assert!(!pr_already_exists("fatal: could not read from remote repository"));
    }

    #[test]
    fn pr_raw_merged_conflicting() {
        let raw = GhPrRaw {
            number: 1,
            url: "u".into(),
            state: "MERGED".into(),
            title: "t".into(),
            mergeable: "CONFLICTING".into(),
        };
        let pr = PrState::from(raw);
        assert!(matches!(pr.state, PrStatus::Merged));
        assert!(!pr.mergeable);
    }

    #[test]
    fn pr_raw_closed_unknown() {
        let raw = GhPrRaw {
            number: 2,
            url: "u".into(),
            state: "CLOSED".into(),
            title: "t".into(),
            mergeable: "UNKNOWN".into(),
        };
        let pr = PrState::from(raw);
        assert!(matches!(pr.state, PrStatus::Closed));
        assert!(!pr.mergeable);
    }

    fn rollup_fixture() -> Vec<serde_json::Value> {
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
        let passing: Vec<serde_json::Value> = serde_json::from_str(
            r#"[{"__typename":"CheckRun","name":"build","status":"COMPLETED","conclusion":"SUCCESS"}]"#,
        )
        .unwrap();
        assert_eq!(parse_pr_checks("CLEAN", &passing).rollup, "passing");
        // Pending (no failures yet).
        let pending: Vec<serde_json::Value> = serde_json::from_str(
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
        let rollup: Vec<serde_json::Value> = serde_json::from_str(
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

    fn review_threads_fixture() -> Vec<serde_json::Value> {
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

    #[test]
    fn parses_repo_from_pr_url() {
        assert_eq!(
            parse_repo_from_url("https://github.com/fwdai/quorum/pull/158"),
            Some(("fwdai".into(), "quorum".into())),
        );
        // Enterprise / trailing path tolerated; bad input → None.
        assert_eq!(parse_repo_from_url("not a url"), None);
    }

    #[test]
    fn pr_raw_unknown_state_defaults_to_open() {
        let raw = GhPrRaw {
            number: 3,
            url: "u".into(),
            state: "SOMETHING_NEW".into(),
            title: "t".into(),
            mergeable: "MERGEABLE".into(),
        };
        let pr = PrState::from(raw);
        assert!(matches!(pr.state, PrStatus::Open));
    }
}
