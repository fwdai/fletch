//! PR merge-gate + per-check rollup: the GraphQL selection, node extractor,
//! branch lookup, and the pure normalizer.

use std::path::Path;

use serde_json::{json, Value};

use crate::error::Result;

use super::query::{branch_pr_nodes, branch_prs_query, graphql_opt, pick_branch_pr, repo_ref};
use super::types::*;

/// GraphQL selection for the merge gate + per-check rollup on a PR node.
/// `startedAt: createdAt` aliases StatusContext's field to the name the parser
/// (shared with the CheckRun arm) expects. Reused by the branch lookup
/// (`pr_checks`) and the by-number batch (`pr_checks_batch`).
pub(crate) const PR_CHECKS_FIELDS: &str = r#"mergeStateStatus
           commits(last:1){nodes{commit{statusCheckRollup{contexts(first:100){nodes{
             __typename
             ... on CheckRun { name status conclusion detailsUrl startedAt completedAt }
             ... on StatusContext { context state targetUrl startedAt: createdAt }
           }}}}}}"#;

/// Extract [`PrChecks`] from a PR node carrying [`PR_CHECKS_FIELDS`].
pub(crate) fn pr_checks_from_node(pr: &Value) -> PrChecks {
    let merge_state = pr["mergeStateStatus"]
        .as_str()
        .unwrap_or("UNKNOWN")
        .to_string();
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
    let passed = runs
        .iter()
        .filter(|r| r.status == "completed" && !is_failing(r))
        .count() as u32;
    let rollup_summary = if total == 0 {
        "none"
    } else if failed > 0 {
        "failing"
    } else if pending > 0 {
        "pending"
    } else {
        "passing"
    };
    let required_failing = runs
        .iter()
        .filter(|r| is_failing(r))
        .map(|r| r.name.clone())
        .collect();

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
