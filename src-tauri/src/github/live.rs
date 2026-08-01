//! The Git panel's fast tick: PR state + CI, read over ETag-conditional REST.
//!
//! Why REST here, when every other read op is GraphQL. GraphQL bills a points
//! budget and offers no conditional requests — every poll costs, whether or not
//! anything moved. REST answers a request carrying a matching `If-None-Match`
//! with `304 Not Modified`, and GitHub does not count a 304 against the primary
//! rate limit. For the panel's hottest signals — "is it green yet", "did it
//! merge" — that is the difference between paying per glance and paying per
//! *change*, so this tick can run fast and stay near-free.
//!
//! GraphQL keeps the reads it alone can serve: review-thread resolution
//! (`isResolved`/`isOutdated`) has no REST equivalent, so it stays on a slower
//! GraphQL tick in `comments.rs`. That is the whole split.
//!
//! Three conditional GETs make up a tick: the PR object (state, mergeability,
//! head sha), the head commit's check-runs, and its legacy commit statuses.
//! GraphQL's `statusCheckRollup` merges the last two; REST keeps them apart, so
//! we fetch both and concatenate. Both are conditional, so the extra
//! round-trip is free in rate-limit terms.

use std::path::Path;

use serde_json::Value;

use crate::error::Result;

use super::checks::{parse_merge_state, rollup_checks};
use super::client;
use super::query::{gh_time_ms, resolve_slug};
use super::types::*;

/// Parse the REST pull-request object into [`PrState`].
///
/// Must agree field-for-field with `pr::parse_pr_state` (the GraphQL parser):
/// the app-wide sweep writes PR state from GraphQL and this writes it from
/// REST, into the same store slice, so a disagreement would show up as the
/// badge flickering between two answers. The shapes differ in two places —
/// REST splits GraphQL's single `MERGED` state into `state:"closed"` plus a
/// `merged` boolean, and reports mergeability as a tri-state bool rather than
/// an enum.
pub(crate) fn parse_pr_state_rest(pr: &Value) -> PrState {
    let merged = pr["merged"].as_bool().unwrap_or(false)
        || pr["merged_at"].as_str().is_some_and(|s| !s.is_empty());
    let closed = pr["state"].as_str() == Some("closed");
    let state = if merged {
        PrStatus::Merged
    } else if closed {
        PrStatus::Closed
    } else {
        PrStatus::Open
    };
    PrState {
        number: pr["number"].as_u64().unwrap_or_default() as u32,
        url: pr["html_url"].as_str().unwrap_or_default().to_string(),
        title: pr["title"].as_str().unwrap_or_default().to_string(),
        // Only an OPEN PR has a meaningful mergeability, and `null` means
        // "GitHub hasn't computed it yet" — distinct from a real conflict.
        mergeable: if matches!(state, PrStatus::Open) {
            match pr["mergeable"].as_bool() {
                Some(true) => MergeableState::Mergeable,
                Some(false) => MergeableState::Conflicting,
                None => MergeableState::Unknown,
            }
        } else {
            MergeableState::Unknown
        },
        state,
        opened_at: gh_time_ms(pr, "created_at"),
        merged_at: gh_time_ms(pr, "merged_at"),
    }
}

/// One REST check-run node → [`CheckRun`]. `status`/`conclusion` already arrive
/// lowercase here, matching what the GraphQL parser lowercases them to.
fn parse_check_run(node: &Value) -> CheckRun {
    let str_of = |key: &str| -> Option<String> {
        node.get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    CheckRun {
        name: str_of("name").unwrap_or_else(|| "check".into()),
        status: str_of("status").unwrap_or_else(|| "queued".into()),
        conclusion: str_of("conclusion"),
        required: false,
        url: str_of("details_url"),
        started_at: str_of("started_at"),
        completed_at: str_of("completed_at"),
    }
}

/// One REST combined-status context → [`CheckRun`]. The legacy commit-status
/// API has a single `state` covering both status and conclusion — the same
/// collapse the GraphQL `StatusContext` arm handles.
fn parse_status_context(node: &Value) -> CheckRun {
    let str_of = |key: &str| -> Option<String> {
        node.get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let (status, conclusion) = match node["state"].as_str().unwrap_or("") {
        "success" => ("completed", Some("success")),
        "failure" | "error" => ("completed", Some("failure")),
        _ => ("in_progress", None), // pending
    };
    CheckRun {
        name: str_of("context").unwrap_or_else(|| "status".into()),
        status: status.to_string(),
        conclusion: conclusion.map(str::to_string),
        required: false,
        url: str_of("target_url"),
        started_at: str_of("created_at"),
        completed_at: None,
    }
}

/// Every check on a commit: App check-runs plus legacy commit statuses,
/// concatenated the way GraphQL's `statusCheckRollup` presents them.
fn parse_commit_checks(check_runs: &[Value], statuses: &[Value]) -> Vec<CheckRun> {
    check_runs
        .iter()
        .map(parse_check_run)
        .chain(statuses.iter().map(parse_status_context))
        .collect()
}

/// Page size for the commit-check reads — GitHub's maximum, so the common case
/// is one request.
const CHECKS_PAGE_SIZE: usize = 100;
/// Pages pulled per endpoint before giving up. 500 checks on one commit is well
/// past the point where the rollup means anything; the cap only stops a
/// pathological repo from turning one poll into unbounded work.
const CHECKS_MAX_PAGES: u32 = 5;

/// Fetch every page of a paginated commit-check endpoint, concatenating the
/// array at `field`.
///
/// These endpoints paginate at **30 items by default**, so requesting them
/// unpaginated silently truncates: a commit with more checks than one page would
/// roll up over a subset, under-reporting `total`/`pending`/`failed` and dropping
/// names from `required_failing` — a green pill over a failing build. `per_page`
/// is maxed and `total_count` drives the loop, so the full set is always read.
///
/// Each page is a distinct path, so conditional caching still applies per page
/// and an unchanged commit's re-poll stays free.
async fn fetch_all_checks(
    client: &client::Client,
    base_path: &str,
    field: &str,
) -> Result<Option<Vec<Value>>> {
    let mut items: Vec<Value> = Vec::new();
    for page in 1..=CHECKS_MAX_PAGES {
        let (status, body) = client
            .rest_get_conditional(&format!(
                "{base_path}?per_page={CHECKS_PAGE_SIZE}&page={page}"
            ))
            .await?;
        if !status.is_success() {
            return Ok(None);
        }
        let batch = body[field].as_array().cloned().unwrap_or_default();
        let batch_len = batch.len();
        // Absent `total_count` (shouldn't happen on these endpoints) degrades to
        // "what we have", which the short-page check below then terminates on.
        let total = body["total_count"]
            .as_u64()
            .unwrap_or((items.len() + batch_len) as u64);
        items.extend(batch);

        // A short page means the server had nothing more to give, whatever
        // `total_count` claimed.
        if batch_len < CHECKS_PAGE_SIZE || items.len() as u64 >= total {
            return Ok(Some(items));
        }
        if page == CHECKS_MAX_PAGES {
            tracing::debug!(
                field,
                fetched = items.len(),
                total,
                "commit checks truncated at the page cap"
            );
        }
    }
    Ok(Some(items))
}

/// Fetch one PR's state by number over conditional REST, resolving the repo
/// from any checkout of it.
///
/// The state-only counterpart to [`pr_checks_live`], for background pollers
/// that need "did it merge?" and nothing else. Conditional, so a PR that hasn't
/// moved since the last read answers `304` and is not billed against the rate
/// limit — which is what makes it affordable to ask every couple of minutes for
/// as long as a review takes.
///
/// `Ok(None)` means "no answer this round": a rate-limit backoff, no token, a
/// non-GitHub remote, or a PR the API won't serve. Callers must treat that as
/// *unchanged*, never as a state — see `roadmap::merge_sweep`.
pub async fn pr_state_live(checkout: &Path, number: u32) -> Result<Option<PrState>> {
    if client::is_backing_off() {
        return Ok(None);
    }
    let Some((owner, repo)) = resolve_slug(checkout, None).await else {
        return Ok(None);
    };
    // Background poll path: not being connected is a normal state, not an error.
    let Ok(client) = client::Client::new() else {
        return Ok(None);
    };
    let (status, pr) = client
        .rest_get_conditional(&format!("/repos/{owner}/{repo}/pulls/{number}"))
        .await?;
    if !status.is_success() {
        return Ok(None);
    }
    Ok(Some(parse_pr_state_rest(&pr)))
}

/// Fetch the merge gate + CI rollup for one **open** PR by number, over
/// conditional REST.
///
/// Deliberately CI-only. PR *state* is resolved by `supervisor::resolve_pr_state`
/// instead, so this path inherits that resolver's policy rather than restating
/// it: merged served from the database, a failed fetch degrading to the last
/// persisted snapshot (never erasing a confirmed badge), and a discovered OPEN
/// PR getting adopted. An earlier revision returned state here too and silently
/// dropped all three — blanking the panel's PR card during a rate-limit pause,
/// of all moments.
///
/// `Ok(None)` means "no rollup this round" — backoff, unresolvable slug, gone
/// PR, or a failed commit read. Callers keep their last-known tint rather than
/// rendering a false "no checks".
pub async fn pr_checks_live(
    checkout: &Path,
    source_repo: Option<&Path>,
    number: u32,
) -> Result<Option<PrChecks>> {
    if client::is_backing_off() {
        return Ok(None);
    }
    let Some((owner, repo)) = resolve_slug(checkout, source_repo).await else {
        return Ok(None);
    };
    // Background poll path: not being connected is a normal state, not an error.
    let Ok(client) = client::Client::new() else {
        return Ok(None);
    };

    // The PR object again — `resolve_pr_state` just read this same path, so this
    // is a conditional hit (a 304 GitHub doesn't bill). We need two fields it
    // doesn't carry into `PrState`: the head sha to hang the check reads off,
    // and `mergeable_state` for the merge gate. Re-reading beats threading the
    // raw JSON through the resolver just to save one free round-trip.
    let (status, pr) = client
        .rest_get_conditional(&format!("/repos/{owner}/{repo}/pulls/{number}"))
        .await?;
    if !status.is_success() {
        return Ok(None);
    }
    let Some(sha) = pr["head"]["sha"].as_str().filter(|s| !s.is_empty()) else {
        // A PR whose head ref is gone has no commit to read checks from.
        return Ok(None);
    };
    let merge_state = parse_merge_state(pr["mergeable_state"].as_str().unwrap_or("unknown"));

    // Both halves must resolve: GraphQL's `statusCheckRollup` merges App
    // check-runs with legacy commit statuses, so reporting one without the
    // other would under-count.
    let (Ok(Some(check_runs)), Ok(Some(statuses))) = (
        fetch_all_checks(
            &client,
            &format!("/repos/{owner}/{repo}/commits/{sha}/check-runs"),
            "check_runs",
        )
        .await,
        fetch_all_checks(
            &client,
            &format!("/repos/{owner}/{repo}/commits/{sha}/status"),
            "statuses",
        )
        .await,
    ) else {
        return Ok(None);
    };

    Ok(Some(rollup_checks(
        merge_state,
        parse_commit_checks(&check_runs, &statuses),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The REST state parser must land on the same verdicts as the GraphQL one,
    /// since both write the same store slice. The interesting case is MERGED:
    /// REST spells it `state:"closed"` + `merged:true`, and a merged PR's
    /// mergeability is meaningless.
    #[test]
    fn rest_state_maps_merged_closed_and_open() {
        let merged = parse_pr_state_rest(&json!({
            "number": 7, "html_url": "https://github.com/o/r/pull/7", "title": "t",
            "state": "closed", "merged": true, "mergeable": null,
            "created_at": "2026-01-01T00:00:00Z", "merged_at": "2026-01-02T00:00:00Z"
        }));
        assert_eq!(merged.state, PrStatus::Merged);
        assert_eq!(merged.mergeable, MergeableState::Unknown);
        assert!(merged.merged_at.is_some());

        let closed = parse_pr_state_rest(&json!({
            "number": 8, "state": "closed", "merged": false, "mergeable": null
        }));
        assert_eq!(closed.state, PrStatus::Closed);

        let open = parse_pr_state_rest(&json!({
            "number": 9, "state": "open", "merged": false, "mergeable": true
        }));
        assert_eq!(open.state, PrStatus::Open);
        assert_eq!(open.mergeable, MergeableState::Mergeable);

        // `mergeable: null` on an open PR is "not computed yet", not a conflict.
        let uncomputed = parse_pr_state_rest(&json!({
            "number": 10, "state": "open", "merged": false, "mergeable": null
        }));
        assert_eq!(uncomputed.mergeable, MergeableState::Unknown);

        let conflicting = parse_pr_state_rest(&json!({
            "number": 11, "state": "open", "merged": false, "mergeable": false
        }));
        assert_eq!(conflicting.mergeable, MergeableState::Conflicting);
    }

    /// A PR merged via the API can report `merged_at` without `merged:true`;
    /// either signal alone must read as merged.
    #[test]
    fn rest_state_infers_merged_from_timestamp() {
        let pr = parse_pr_state_rest(&json!({
            "number": 12, "state": "closed", "merged_at": "2026-01-02T00:00:00Z"
        }));
        assert_eq!(pr.state, PrStatus::Merged);
    }

    /// Check-runs and legacy statuses both land in one list, normalized to the
    /// same shape the GraphQL rollup produces.
    #[test]
    fn rest_checks_merge_runs_and_legacy_statuses() {
        let check_runs = vec![
            json!({"name": "build", "status": "completed", "conclusion": "success",
                   "details_url": "https://ci/build", "started_at": "2026-01-01T00:00:00Z",
                   "completed_at": "2026-01-01T00:05:00Z"}),
            json!({"name": "test", "status": "in_progress", "conclusion": null}),
        ];
        let statuses = vec![
            json!({"context": "ci/legacy", "state": "success", "target_url": "https://ci/legacy",
                   "created_at": "2026-01-01T00:00:00Z"}),
            json!({"context": "ci/broken", "state": "error", "target_url": null}),
        ];
        let runs = parse_commit_checks(&check_runs, &statuses);
        assert_eq!(runs.len(), 4);

        let checks = rollup_checks(MergeState::Unstable, runs);
        assert_eq!(checks.total, 4);
        assert_eq!(checks.passed, 2); // build + ci/legacy
        assert_eq!(checks.failed, 1); // ci/broken (error -> failure)
        assert_eq!(checks.pending, 1); // test
        assert_eq!(checks.rollup, "failing");
        assert_eq!(checks.required_failing, vec!["ci/broken".to_string()]);

        let build = checks.runs.iter().find(|r| r.name == "build").unwrap();
        assert_eq!(build.url.as_deref(), Some("https://ci/build"));
        assert_eq!(build.completed_at.as_deref(), Some("2026-01-01T00:05:00Z"));
    }

    /// A commit with neither check-runs nor statuses rolls up as "none", not
    /// as a failure — the same verdict the GraphQL path gives an empty rollup.
    #[test]
    fn rest_checks_empty_is_none() {
        let runs = parse_commit_checks(&[], &[]);
        assert!(runs.is_empty());
        assert_eq!(rollup_checks(MergeState::Clean, runs).rollup, "none");
    }

    /// Regression guard for the pagination bug: these endpoints default to 30
    /// items a page, so a commit with more checks than one page used to roll up
    /// over a truncated subset. A failure sitting past the first page must still
    /// reach the rollup — the difference between a red pill and a green one over
    /// a broken build.
    #[test]
    fn rest_checks_rollup_sees_beyond_one_page() {
        // 40 checks: 39 passing, and the last one — past a 30-item page —
        // failing.
        let mut nodes: Vec<Value> = (0..39)
            .map(|i| json!({"name": format!("ok-{i}"), "status": "completed", "conclusion": "success"}))
            .collect();
        nodes.push(json!({"name": "late-failure", "status": "completed", "conclusion": "failure"}));

        let full = rollup_checks(MergeState::Unstable, parse_commit_checks(&nodes, &[]));
        assert_eq!(full.total, 40);
        assert_eq!(full.failed, 1);
        assert_eq!(full.rollup, "failing");
        assert_eq!(full.required_failing, vec!["late-failure".to_string()]);

        // What a single default-size page would have produced: all green, and
        // the failing name absent entirely.
        let truncated = rollup_checks(MergeState::Unstable, parse_commit_checks(&nodes[..30], &[]));
        assert_eq!(truncated.rollup, "passing");
        assert!(truncated.required_failing.is_empty());
    }

    /// REST spells the merge state lowercase; GraphQL spells it uppercase.
    /// Both must reach the same variant.
    #[test]
    fn merge_state_accepts_both_spellings() {
        assert_eq!(parse_merge_state("blocked"), MergeState::Blocked);
        assert_eq!(parse_merge_state("BLOCKED"), MergeState::Blocked);
        assert_eq!(parse_merge_state("has_hooks"), MergeState::HasHooks);
        assert_eq!(parse_merge_state("something_new"), MergeState::Unknown);
    }
}
