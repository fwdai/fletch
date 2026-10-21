//! GitHub Actions: workflow runs and what a failing job left behind (the job
//! log and the annotations). Read by the agent's `run_list`, `run_log` and
//! `pr_check_log` ops; the id on a [`CheckRun`] is the handle.
//!
//! A GitHub Actions job and its check run share one id, so the Actions log
//! endpoint takes the check-run id directly. Checks from other apps have no
//! Actions log (the endpoint answers 404) but may carry annotations, which
//! come from the check-runs API and work for every app.

use std::path::Path;

use serde_json::Value;

use crate::error::{Error, Result};

use super::client;
use super::query::require_repo_ref;
use super::types::*;

/// Most lines a caller may ask for; the whole log of a long job is megabytes.
pub const CHECK_LOG_MAX_LINES: usize = 2000;
pub const CHECK_LOG_DEFAULT_LINES: usize = 200;

/// GitHub's page cap for the runs and jobs lists.
pub const RUN_LIST_MAX: u32 = 100;

/// The repository's workflow runs matching `filter`, newest first, over
/// REST. `Ok(None)` when GitHub answers anything but success: a repository
/// the token cannot see, or a `workflow` that names no workflow file.
pub async fn run_list(checkout: &Path, filter: &RunListFilter) -> Result<Option<Vec<WorkflowRun>>> {
    let (owner, repo) = require_repo_ref(checkout).await?;
    let client = client::Client::new()?;
    let base = match &filter.workflow {
        Some(workflow) => format!("/repos/{owner}/{repo}/actions/workflows/{workflow}/runs"),
        None => format!("/repos/{owner}/{repo}/actions/runs"),
    };
    let per_page = filter.limit.clamp(1, RUN_LIST_MAX).to_string();
    let mut params = vec![("per_page", per_page.as_str())];
    for (key, value) in [
        ("branch", &filter.branch),
        ("status", &filter.status),
        ("event", &filter.event),
    ] {
        if let Some(value) = value {
            params.push((key, value));
        }
    }
    let (status, body) = client
        .rest_get_observed(&client::with_query(&base, &params))
        .await?;
    if !status.is_success() {
        return Ok(None);
    }
    Ok(Some(parse_run_list(&body)))
}

/// The jobs of run `id` (its latest attempt), as [`CheckRun`]s: a job and
/// its check run share an id, so the same shape names the log to read.
/// `Ok(None)` when GitHub answers anything but success for the run.
pub async fn run_jobs(checkout: &Path, id: u64) -> Result<Option<Vec<CheckRun>>> {
    let (owner, repo) = require_repo_ref(checkout).await?;
    let client = client::Client::new()?;
    let base = format!("/repos/{owner}/{repo}/actions/runs/{id}/jobs");
    let per_page = RUN_LIST_MAX.to_string();
    // Paged to the end: a large matrix puts its failed job past the first
    // hundred, and a job pick over a partial list would report no failure.
    // `total_count` says when the list is complete; a short page does too.
    let mut jobs = Vec::new();
    for page in 1..=JOBS_MAX_PAGES {
        let page = page.to_string();
        let path = client::with_query(
            &base,
            &[
                ("filter", "latest"),
                ("per_page", &per_page),
                ("page", &page),
            ],
        );
        let (status, body) = client.rest_get_observed(&path).await?;
        if !status.is_success() {
            return Ok(None);
        }
        let batch = parse_run_jobs(&body);
        let short = batch.len() < RUN_LIST_MAX as usize;
        jobs.extend(batch);
        let complete = body["total_count"]
            .as_u64()
            .is_some_and(|total| jobs.len() as u64 >= total);
        if short || complete {
            break;
        }
    }
    Ok(Some(jobs))
}

/// Pages of jobs a run is read to; a thousand jobs is beyond any matrix
/// GitHub runs, so this only bounds a list that never reports its total.
const JOBS_MAX_PAGES: u32 = 10;

/// Trigger `workflow` (a file name or id) on `git_ref` with `inputs`, as
/// `gh workflow run` does. GitHub answers 204 with no run id; the returned
/// URL is the workflow's runs page, and `run_list` narrowed to the workflow
/// and branch finds the run. A workflow without a `workflow_dispatch`
/// trigger, a missing required input or an unknown ref is GitHub's 422,
/// surfaced with its message.
pub async fn workflow_dispatch(
    checkout: &Path,
    workflow: &str,
    git_ref: &str,
    inputs: &serde_json::Map<String, Value>,
) -> Result<String> {
    let (owner, repo) = require_repo_ref(checkout).await?;
    let client = client::Client::new()?;
    let mut payload = serde_json::json!({ "ref": git_ref });
    if !inputs.is_empty() {
        payload["inputs"] = Value::Object(inputs.clone());
    }
    let (status, body) = client
        .rest(
            reqwest::Method::POST,
            &format!("/repos/{owner}/{repo}/actions/workflows/{workflow}/dispatches"),
            Some(&payload),
        )
        .await?;
    if !status.is_success() {
        let why = match status.as_u16() {
            404 => format!(
                "workflow {workflow:?} was not found in this repository, or the repository is \
                 not visible to the host's GitHub account"
            ),
            _ => super::pr::detailed_rest_error(&body),
        };
        return Err(Error::Gh(format!("workflow dispatch failed: {why}")));
    }
    Ok(workflow_runs_url(&owner, &repo, workflow))
}

/// Where the runs of `workflow` are listed on GitHub: the workflow's own
/// page for a file name, the repository's Actions tab for a numeric id.
pub(crate) fn workflow_runs_url(owner: &str, repo: &str, workflow: &str) -> String {
    if workflow.bytes().all(|b| b.is_ascii_digit()) {
        format!("https://github.com/{owner}/{repo}/actions")
    } else {
        format!("https://github.com/{owner}/{repo}/actions/workflows/{workflow}")
    }
}

/// The REST `workflow_runs` array → [`WorkflowRun`]s. Pure; a body without
/// the array is an empty list.
pub(crate) fn parse_run_list(body: &Value) -> Vec<WorkflowRun> {
    let text = |v: &Value, key: &str| v[key].as_str().unwrap_or_default().to_string();
    body["workflow_runs"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|run| WorkflowRun {
                    id: run["id"].as_u64().unwrap_or_default(),
                    name: text(run, "name"),
                    workflow: workflow_file(run["path"].as_str().unwrap_or_default()),
                    title: text(run, "display_title"),
                    branch: text(run, "head_branch"),
                    head_sha: text(run, "head_sha"),
                    event: text(run, "event"),
                    status: text(run, "status"),
                    conclusion: run["conclusion"]
                        .as_str()
                        .filter(|c| !c.is_empty())
                        .map(str::to_string),
                    url: text(run, "html_url"),
                    run_number: run["run_number"].as_u64().unwrap_or_default(),
                    run_attempt: run["run_attempt"].as_u64().unwrap_or(1),
                    created_at: text(run, "created_at"),
                    updated_at: text(run, "updated_at"),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `.github/workflows/ci.yml` → `ci.yml`: the spelling the workflow
/// endpoints take, so a row's `workflow` can be passed straight back.
pub(crate) fn workflow_file(path: &str) -> String {
    // GitHub may report the path with the ref it ran at appended
    // (`.github/workflows/build.yml@main`, as its own response example
    // shows; a reusable workflow from another repository too). The
    // workflow endpoints take the bare file name, so the suffix goes —
    // first, since a ref such as `refs/heads/main` has slashes of its own.
    let path = path.split_once('@').map_or(path, |(file, _ref)| file);
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// The REST `jobs` array → [`CheckRun`]s, the run's jobs in order. Pure.
pub(crate) fn parse_run_jobs(body: &Value) -> Vec<CheckRun> {
    let str_of = |v: &Value, key: &str| -> Option<String> {
        v[key]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    body["jobs"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|job| CheckRun {
                    id: job["id"].as_u64(),
                    name: str_of(job, "name").unwrap_or_else(|| "job".into()),
                    status: str_of(job, "status").unwrap_or_else(|| "queued".into()),
                    conclusion: str_of(job, "conclusion"),
                    required: false,
                    url: str_of(job, "html_url"),
                    started_at: str_of(job, "started_at"),
                    completed_at: str_of(job, "completed_at"),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The tail of check `id`'s job log and its annotations. Errors only when
/// nothing can be read: no token, no GitHub origin, a transport failure, or a
/// check with neither a log nor annotations GitHub will show.
pub async fn check_log(checkout: &Path, id: u64, lines: usize) -> Result<CheckLog> {
    let (owner, repo) = require_repo_ref(checkout).await?;
    let client = client::Client::new()?;

    let (log_status, raw) = client
        .rest_text(&format!("/repos/{owner}/{repo}/actions/jobs/{id}/logs"))
        .await?;
    let (log_tail, truncated, note) = if log_status.is_success() {
        let (tail, truncated) = tail_lines(&raw, lines);
        (Some(tail), truncated, None)
    } else {
        let why = match log_status.as_u16() {
            404 | 410 => "no job log for this check: it is not a GitHub Actions job, or its log \
                          has expired; read the annotations and the check's url"
                .to_string(),
            other => format!("the job log could not be read (GitHub answered {other})"),
        };
        (None, false, Some(why))
    };

    let (ann_status, body) = client
        .rest(
            reqwest::Method::GET,
            &format!("/repos/{owner}/{repo}/check-runs/{id}/annotations?per_page=100"),
            None,
        )
        .await?;
    let annotations = if ann_status.is_success() {
        parse_annotations(&body)
    } else if log_tail.is_none() {
        return Err(Error::Gh(format!(
            "check {id} was not found in this repository, or is not visible to the host's \
             GitHub account"
        )));
    } else {
        Vec::new()
    };

    Ok(CheckLog {
        id,
        log_tail,
        truncated,
        annotations,
        note,
    })
}

/// The last `lines` lines of an Actions log, each stripped of the leading
/// `2026-10-21T10:00:00.1234567Z ` stamp the runner adds — the agents that
/// read logs through the CLI piped them through `sed` for exactly that.
/// Returns the tail and whether anything was cut.
pub(crate) fn tail_lines(log: &str, lines: usize) -> (String, bool) {
    let lines = lines.clamp(1, CHECK_LOG_MAX_LINES);
    let all: Vec<&str> = log.lines().collect();
    let skip = all.len().saturating_sub(lines);
    let tail = all[skip..]
        .iter()
        .map(|l| strip_runner_timestamp(l))
        .collect::<Vec<_>>()
        .join("\n");
    (tail, skip > 0)
}

/// `2026-10-21T10:00:00.1234567Z rest` → `rest`; a line without the stamp is
/// returned whole.
fn strip_runner_timestamp(line: &str) -> &str {
    let Some((stamp, rest)) = line.split_once(' ') else {
        return line;
    };
    let looks_like_stamp = stamp.len() >= 20
        && stamp.ends_with('Z')
        && stamp.as_bytes().get(4) == Some(&b'-')
        && stamp.as_bytes().get(10) == Some(&b'T')
        && stamp[..4].bytes().all(|b| b.is_ascii_digit());
    if looks_like_stamp {
        rest
    } else {
        line
    }
}

/// The REST annotations array → [`CheckAnnotation`]s, dropping entries with
/// no message. Pure, so the shape is tested without the network.
pub(crate) fn parse_annotations(body: &Value) -> Vec<CheckAnnotation> {
    body.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|a| {
                    let message = a["message"]
                        .as_str()
                        .map(str::trim)
                        .filter(|m| !m.is_empty())?;
                    Some(CheckAnnotation {
                        path: a["path"].as_str().unwrap_or_default().to_string(),
                        start_line: a["start_line"].as_u64().unwrap_or_default() as u32,
                        end_line: a["end_line"].as_u64().unwrap_or_default() as u32,
                        level: a["annotation_level"]
                            .as_str()
                            .unwrap_or("notice")
                            .to_string(),
                        message: message.to_string(),
                        title: a["title"]
                            .as_str()
                            .filter(|t| !t.is_empty())
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tail_keeps_the_last_lines_and_strips_runner_stamps() {
        let log = "2026-10-21T10:00:00.1234567Z ##[group]Run cargo test\n\
                   2026-10-21T10:00:01.0000000Z    Compiling foo v0.1.0\n\
                   2026-10-21T10:00:02.0000000Z error[E0425]: cannot find value `x`\n\
                   plain line without a stamp\n";
        let (tail, truncated) = tail_lines(log, 2);
        assert_eq!(
            tail,
            "error[E0425]: cannot find value `x`\nplain line without a stamp"
        );
        assert!(truncated);

        let (whole, truncated) = tail_lines(log, 10);
        assert!(!truncated);
        assert!(whole.starts_with("##[group]Run cargo test\n"));
        assert!(!whole.contains("2026-10-21T"));
    }

    #[test]
    fn tail_clamps_the_line_count() {
        let log = (0..5)
            .map(|i| format!("l{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (one, _) = tail_lines(&log, 0);
        assert_eq!(one, "l4", "zero lines reads as one");
        let (all, truncated) = tail_lines(&log, CHECK_LOG_MAX_LINES + 1);
        assert_eq!(all.lines().count(), 5);
        assert!(!truncated);
    }

    #[test]
    fn a_word_that_merely_ends_in_z_is_not_a_stamp() {
        assert_eq!(
            strip_runner_timestamp("fuzz test passed"),
            "fuzz test passed"
        );
        assert_eq!(
            strip_runner_timestamp("2026-10-21T10:00:00Z ok"),
            "ok",
            "a stamp without fractional seconds still counts"
        );
        assert_eq!(strip_runner_timestamp("nospace"), "nospace");
    }

    #[test]
    fn run_list_parses_rows_and_names_the_workflow_file() {
        let body = json!({ "total_count": 2, "workflow_runs": [
            { "id": 9001, "name": "CI", "path": ".github/workflows/ci.yml",
              "display_title": "fix: login", "head_branch": "fix/login", "head_sha": "abc",
              "event": "push", "status": "completed", "conclusion": "failure",
              "html_url": "https://github.com/o/r/actions/runs/9001",
              "run_number": 41, "run_attempt": 2,
              "created_at": "2026-10-21T10:00:00Z", "updated_at": "2026-10-21T10:05:00Z" },
            { "id": 9002, "name": "Deploy", "path": "dynamic/pages/pages-build-deployment",
              "head_branch": "main", "event": "dynamic", "status": "in_progress", "conclusion": null }
        ]});
        let runs = parse_run_list(&body);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].workflow, "ci.yml");
        assert_eq!(runs[0].conclusion.as_deref(), Some("failure"));
        assert_eq!(runs[0].run_attempt, 2);
        assert_eq!(runs[1].workflow, "pages-build-deployment");
        assert_eq!(runs[1].conclusion, None);
        assert_eq!(runs[1].run_attempt, 1, "an absent attempt is the first");
        assert!(parse_run_list(&json!({ "message": "Not Found" })).is_empty());
    }

    /// The `path` GitHub reports can carry the ref the run was at, and a
    /// reusable workflow its repository; the workflow endpoints take the
    /// bare file name, which is what a row must hand back.
    #[test]
    fn a_workflow_path_is_reduced_to_the_file_the_endpoints_take() {
        assert_eq!(
            workflow_file(".github/workflows/build.yml@main"),
            "build.yml"
        );
        assert_eq!(workflow_file(".github/workflows/ci.yml"), "ci.yml");
        assert_eq!(
            workflow_file("octo-org/shared/.github/workflows/deploy.yml@refs/heads/main"),
            "deploy.yml"
        );
        assert_eq!(workflow_file("ci.yml"), "ci.yml");
        assert_eq!(
            workflow_file("dynamic/pages/pages-build-deployment"),
            "pages-build-deployment"
        );
    }

    #[test]
    fn a_dispatched_workflow_is_pointed_at_by_file_or_the_actions_tab() {
        assert_eq!(
            workflow_runs_url("o", "r", "deploy.yml"),
            "https://github.com/o/r/actions/workflows/deploy.yml"
        );
        assert_eq!(
            workflow_runs_url("o", "r", "4242"),
            "https://github.com/o/r/actions"
        );
    }

    #[test]
    fn run_jobs_parse_as_check_runs() {
        let body = json!({ "jobs": [
            { "id": 77, "name": "test (ubuntu)", "status": "completed", "conclusion": "failure",
              "html_url": "https://github.com/o/r/actions/runs/9001/job/77",
              "started_at": "2026-10-21T10:00:00Z", "completed_at": "2026-10-21T10:04:00Z" },
            { "id": 78, "name": "lint", "status": "queued", "conclusion": null }
        ]});
        let jobs = parse_run_jobs(&body);
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].id, Some(77));
        assert_eq!(jobs[0].name, "test (ubuntu)");
        assert!(super::super::checks::is_failing(&jobs[0]));
        assert_eq!(jobs[1].status, "queued");
        assert_eq!(jobs[1].conclusion, None);
        assert!(parse_run_jobs(&json!([])).is_empty());
    }

    #[test]
    fn annotations_parse_and_drop_the_messageless() {
        let body = json!([
            { "path": "src/lib.rs", "start_line": 10, "end_line": 12,
              "annotation_level": "failure", "message": "  mismatched types ",
              "title": "error[E0308]" },
            { "path": "src/x.rs", "start_line": 1, "end_line": 1,
              "annotation_level": "warning", "message": "", "title": "" },
            { "path": "README.md", "start_line": 3, "end_line": 3,
              "annotation_level": null, "message": "spelling", "title": null }
        ]);
        let got = parse_annotations(&body);
        assert_eq!(got.len(), 2);
        assert_eq!(
            got[0],
            CheckAnnotation {
                path: "src/lib.rs".into(),
                start_line: 10,
                end_line: 12,
                level: "failure".into(),
                message: "mismatched types".into(),
                title: Some("error[E0308]".into()),
            }
        );
        assert_eq!(got[1].level, "notice");
        assert_eq!(got[1].title, None);
        assert!(parse_annotations(&json!({ "message": "Not Found" })).is_empty());
    }
}
