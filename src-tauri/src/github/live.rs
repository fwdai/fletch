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
fn parse_commit_checks(check_runs: &Value, combined_status: &Value) -> Vec<CheckRun> {
    let runs = check_runs["check_runs"]
        .as_array()
        .map(|arr| arr.iter().map(parse_check_run).collect::<Vec<_>>())
        .unwrap_or_default();
    let contexts = combined_status["statuses"]
        .as_array()
        .map(|arr| arr.iter().map(parse_status_context).collect::<Vec<_>>())
        .unwrap_or_default();
    runs.into_iter().chain(contexts).collect()
}

/// Fetch PR state + CI for one PR by number over conditional REST.
///
/// `Ok(None)` when the slug can't be resolved, the PR is gone, or a rate-limit
/// backoff is active — the panel then keeps its last-known values, matching
/// every other read op's degradation contract. Checks are omitted (rather than
/// reported empty) when the commit reads fail, so a transient error can't blank
/// a passing rollup into a false "no checks".
pub async fn pr_live_number(
    checkout: &Path,
    source_repo: Option<&Path>,
    number: u32,
) -> Result<Option<PrLive>> {
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

    let (status, pr) = client
        .rest_get_conditional(&format!("/repos/{owner}/{repo}/pulls/{number}"))
        .await?;
    if !status.is_success() {
        return Ok(None);
    }
    let state = parse_pr_state_rest(&pr);

    // CI is only read for an open PR: a merged or closed PR's checks can never
    // change again, and the panel doesn't render them, so the two commit reads
    // would be pure chatter. State alone keeps flowing, which is what tells the
    // panel the PR merged in the first place.
    if !matches!(state.state, PrStatus::Open) {
        return Ok(Some(PrLive {
            state,
            checks: None,
        }));
    }

    let Some(sha) = pr["head"]["sha"].as_str().filter(|s| !s.is_empty()) else {
        // No head sha (a PR whose head ref is gone) — state still stands.
        return Ok(Some(PrLive {
            state,
            checks: None,
        }));
    };
    let merge_state = parse_merge_state(pr["mergeable_state"].as_str().unwrap_or("unknown"));

    let runs = match (
        client
            .rest_get_conditional(&format!("/repos/{owner}/{repo}/commits/{sha}/check-runs"))
            .await,
        client
            .rest_get_conditional(&format!("/repos/{owner}/{repo}/commits/{sha}/status"))
            .await,
    ) {
        (Ok((cs, check_runs)), Ok((ss, combined))) if cs.is_success() && ss.is_success() => {
            parse_commit_checks(&check_runs, &combined)
        }
        _ => {
            return Ok(Some(PrLive {
                state,
                checks: None,
            }))
        }
    };

    Ok(Some(PrLive {
        state,
        checks: Some(rollup_checks(merge_state, runs)),
    }))
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
        let check_runs = json!({"check_runs": [
            {"name": "build", "status": "completed", "conclusion": "success",
             "details_url": "https://ci/build", "started_at": "2026-01-01T00:00:00Z",
             "completed_at": "2026-01-01T00:05:00Z"},
            {"name": "test", "status": "in_progress", "conclusion": null}
        ]});
        let combined = json!({"statuses": [
            {"context": "ci/legacy", "state": "success", "target_url": "https://ci/legacy",
             "created_at": "2026-01-01T00:00:00Z"},
            {"context": "ci/broken", "state": "error", "target_url": null}
        ]});
        let runs = parse_commit_checks(&check_runs, &combined);
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
        let runs = parse_commit_checks(&json!({"check_runs": []}), &json!({"statuses": []}));
        assert!(runs.is_empty());
        assert_eq!(rollup_checks(MergeState::Clean, runs).rollup, "none");
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
