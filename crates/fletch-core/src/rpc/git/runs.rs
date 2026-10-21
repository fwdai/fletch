//! GitHub Actions ops: the workflow runs of the repository, a run's job
//! log, and triggering a workflow. `pr_checks` answers for a pull request;
//! once the work has merged there is no PR to ask, and "did the deploy run
//! on main?" was the single most common thing agents read through `gh run
//! list`. The reads are like the PR reads: not gated by `AgentCaps` and
//! never an `EVENT_ACTION_DONE`. `workflow_dispatch` is a publish op: it
//! runs the repository's secrets against a ref, so a grant that may not
//! publish is refused and the user is asked first, as for a push.

use serde_json::{json, Value};

use super::args::{arg_branch_named, refuse_option_like};
use super::pr_reads::{find_run, json_ok, positive_u32_arg, CheckRef};
use super::GitDispatcher;
use crate::github::RunListFilter;
use crate::rpc::Response;

/// GitHub takes at most ten inputs; refusing more here names the limit
/// instead of relaying a 422.
const MAX_DISPATCH_INPUTS: usize = 10;

const RUN_LIST_DEFAULT_LIMIT: u32 = 20;

/// GitHub's vocabulary for the runs list's `status` filter: a status or a
/// conclusion, one parameter.
const RUN_STATUSES: &[&str] = &[
    "queued",
    "in_progress",
    "completed",
    "waiting",
    "requested",
    "pending",
    "success",
    "failure",
    "cancelled",
    "skipped",
    "neutral",
    "stale",
    "timed_out",
    "action_required",
];

impl GitDispatcher {
    /// The repository's workflow runs, newest first: `args.branch`,
    /// `args.workflow` (a file name like `ci.yml`, or its id), `args.status`
    /// (a status or a conclusion) and `args.event` narrow them; `args.limit`
    /// default 20, max 100.
    pub(super) async fn run_list(&self, id: &str, args: &Value) -> Response {
        let filter = match run_list_filter(args) {
            Ok(f) => f,
            Err(e) => return Response::err(id, format!("run_list: {e}")),
        };
        if let Some(resp) = filter
            .branch
            .as_deref()
            .and_then(|b| refuse_option_like(id, "run_list", "branch", b))
        {
            return resp;
        }
        let t = match self.pr_read_target(id, "run_list", args).await {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        match crate::github::run_list(&t.source_repo, &filter).await {
            Ok(Some(runs)) => json_ok(id, "run_list", &runs),
            Ok(None) => Response::err(
                id,
                match &filter.workflow {
                    Some(w) => format!(
                        "run_list: GitHub did not list runs for workflow {w:?} — there is no such \
                         workflow file, or the repository is not visible to the host's GitHub \
                         account"
                    ),
                    None => "run_list: GitHub did not list this repository's workflow runs — it \
                             may not be visible to the host's GitHub account; try again shortly"
                        .to_string(),
                },
            ),
            Err(e) => Response::err(id, format!("run_list: {e}")),
        }
    }

    /// The log tail and annotations of one job of run `args.run` (an id from
    /// `run_list`). `args.job` names the job by id or name; without it the
    /// first failing job is read, or the only job. `args.lines` as for
    /// `pr_check_log`. What agents did with `gh run view --log-failed` on a
    /// run that has no pull request.
    pub(super) async fn run_log(&self, id: &str, args: &Value) -> Response {
        let run_id = match positive_u64_arg(args, "run") {
            Ok(Some(n)) => n,
            Ok(None) => {
                return Response::err(
                    id,
                    "run_log requires `run`: a run's `id` as run_list lists them",
                )
            }
            Err(e) => return Response::err(id, format!("run_log: {e}")),
        };
        let wanted = match args.get("job") {
            None | Some(Value::Null) => None,
            Some(_) => match CheckRef::from_args(&json!({ "check": args["job"] })) {
                Some(job) => Some(job),
                None => {
                    return Response::err(
                        id,
                        "run_log: `job` must be a job's `id` or `name` as run_log lists them",
                    )
                }
            },
        };
        let lines = match positive_u32_arg(args, "lines") {
            Ok(n) => (n.map(|n| n as usize))
                .unwrap_or(crate::github::CHECK_LOG_DEFAULT_LINES)
                .min(crate::github::CHECK_LOG_MAX_LINES),
            Err(e) => return Response::err(id, format!("run_log: {e}")),
        };
        let t = match self.pr_read_target(id, "run_log", args).await {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        let jobs = match crate::github::run_jobs(&t.source_repo, run_id).await {
            Ok(Some(jobs)) => jobs,
            Ok(None) => {
                return Response::err(
                    id,
                    format!(
                        "run_log: run {run_id} was not found in this repository, or is not \
                         visible to the host's GitHub account"
                    ),
                )
            }
            Err(e) => return Response::err(id, format!("run_log: {e}")),
        };
        let job = match pick_job(&jobs, wanted.as_ref()) {
            Ok(job) => job,
            Err(why) => return Response::err(id, format!("run_log: {why}")),
        };
        let Some(job_id) = job.id else {
            return Response::err(id, format!("run_log: job {:?} has no id to read", job.name));
        };
        match crate::github::check_log(&t.source_repo, job_id, lines).await {
            Ok(log) => {
                let mut out = serde_json::to_value(&log).unwrap_or_default();
                out["run"] = json!(run_id);
                out["job"] = json!(job.name);
                out["status"] = json!(job.status);
                out["conclusion"] = json!(job.conclusion);
                out["url"] = json!(job.url);
                out["jobs"] = json!(jobs
                    .iter()
                    .map(|j| json!({ "id": j.id, "name": j.name, "status": j.status, "conclusion": j.conclusion }))
                    .collect::<Vec<_>>());
                json_ok(id, "run_log", &out)
            }
            Err(e) => Response::err(id, format!("run_log: {e}")),
        }
    }
}

impl GitDispatcher {
    /// Trigger `args.workflow` (a file name like `deploy.yml`, or its id) on
    /// `args.ref` (the checked-out branch when absent) with `args.inputs`.
    /// What agents did with `gh workflow run` to kick a deploy. Gated like a
    /// push: the prompt names the workflow and the ref.
    pub(super) async fn workflow_dispatch(&self, id: &str, args: &Value) -> Response {
        let workflow = match workflow_arg(args) {
            Ok(Some(w)) => w,
            Ok(None) => {
                return Response::err(
                    id,
                    "workflow_dispatch requires `workflow`: a workflow file name like \
                     \"deploy.yml\" (as run_list's `workflow` spells it) or its id",
                )
            }
            Err(e) => return Response::err(id, format!("workflow_dispatch: {e}")),
        };
        let inputs = match dispatch_inputs(args) {
            Ok(inputs) => inputs,
            Err(e) => return Response::err(id, format!("workflow_dispatch: {e}")),
        };
        let requested_ref = match args.get("ref") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
            Some(other) => {
                return Response::err(
                    id,
                    format!("workflow_dispatch: `ref` must be a branch or tag name, got {other}"),
                )
            }
        };
        if let Some(resp) = requested_ref
            .as_deref()
            .and_then(|r| refuse_option_like(id, "workflow_dispatch", "ref", r))
        {
            return resp;
        }
        let t = match self.pr_read_target(id, "workflow_dispatch", args).await {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        let git_ref =
            match requested_ref {
                Some(r) => r,
                None => match crate::git::current_branch(&t.cwd).await {
                    Ok(Some(branch)) => branch,
                    Ok(None) => return Response::err(
                        id,
                        "workflow_dispatch: HEAD is detached — pass `args.ref` (a branch or tag \
                         on origin) to run the workflow on",
                    ),
                    Err(e) => return Response::err(id, format!("workflow_dispatch: {e}")),
                },
            };
        if let Some(why) = self
            .refuse_unless_publish_approved(
                "workflow_dispatch",
                t.subdir.as_deref(),
                &format!("run workflow {workflow} on {git_ref}"),
            )
            .await
        {
            return Response::err(id, why);
        }
        match crate::github::workflow_dispatch(&t.source_repo, &workflow, &git_ref, &inputs).await {
            Ok(runs_url) => json_ok(
                id,
                "workflow_dispatch",
                &json!({
                    "workflow": workflow,
                    "ref": git_ref,
                    "inputs": inputs,
                    "dispatched": true,
                    "runs_url": runs_url,
                    "note": "GitHub reports no run id for a dispatch; run_list with this \
                             workflow and branch finds the run once it is queued",
                }),
            ),
            Err(e) => Response::err(id, format!("workflow_dispatch: {e}")),
        }
    }
}

/// `args.inputs` as GitHub takes them: an object of at most ten string
/// values. Numbers and booleans are spelled out, since every workflow input
/// is a string to GitHub; anything else is refused rather than sent.
pub(super) fn dispatch_inputs(args: &Value) -> Result<serde_json::Map<String, Value>, String> {
    let object = match args.get("inputs") {
        None | Some(Value::Null) => return Ok(serde_json::Map::new()),
        Some(Value::Object(o)) => o,
        Some(other) => return Err(format!("`inputs` must be an object, got {other}")),
    };
    if object.len() > MAX_DISPATCH_INPUTS {
        return Err(format!(
            "`inputs` holds {} values; GitHub takes at most {MAX_DISPATCH_INPUTS}",
            object.len()
        ));
    }
    object
        .iter()
        .map(|(key, value)| {
            let text = match value {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                other => {
                    return Err(format!(
                        "`inputs.{key}` must be a string, number or boolean, got {other}"
                    ))
                }
            };
            Ok((key.clone(), Value::String(text)))
        })
        .collect()
}

/// `Ok(None)` when `key` is absent or null; an error for anything but a
/// positive integer. Run ids outgrow a u32.
pub(super) fn positive_u64_arg(args: &Value, key: &str) -> Result<Option<u64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .filter(|n| *n > 0)
            .map(Some)
            .ok_or_else(|| format!("`{key}` must be a positive integer, got {v}")),
    }
}

/// The job `wanted` names, or — unnamed — the first failing job, or the only
/// job there is. A run with several jobs and none failing is an error that
/// lists them, so the agent names one.
pub(super) fn pick_job<'a>(
    jobs: &'a [crate::github::CheckRun],
    wanted: Option<&CheckRef>,
) -> Result<&'a crate::github::CheckRun, String> {
    if let Some(wanted) = wanted {
        return find_run(jobs, wanted);
    }
    if let Some(failing) = jobs.iter().find(|j| crate::github::is_failing(j)) {
        return Ok(failing);
    }
    match jobs {
        [one] => Ok(one),
        [] => Err("the run has no jobs yet".to_string()),
        many => Err(format!(
            "no job of the run failed; pass `job` to pick one of: {}",
            many.iter()
                .map(|j| match j.id {
                    Some(id) => format!("{} (id {id})", j.name),
                    None => j.name.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// `args.workflow` as the workflow endpoints take it: a file name
/// (`ci.yml`; a `.github/workflows/` prefix is dropped, since that is how
/// `run_list` rows and the repository spell it) or a numeric id. One path
/// segment, so anything else is refused rather than sent.
pub(super) fn workflow_arg(args: &Value) -> Result<Option<String>, String> {
    let raw = match args.get("workflow") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Number(n)) => match n.as_u64().filter(|n| *n > 0) {
            Some(n) => return Ok(Some(n.to_string())),
            None => {
                return Err(format!(
                    "`workflow` must be a file name or a positive id, got {n}"
                ))
            }
        },
        // Blank reads as absent: no filter for `run_list`, "required" for a dispatch.
        Some(Value::String(s)) if s.trim().is_empty() => return Ok(None),
        Some(Value::String(s)) => s.trim(),
        Some(other) => {
            return Err(format!(
                "`workflow` must be a file name like \"ci.yml\" or an id, got {other}"
            ))
        }
    };
    let name = raw
        .strip_prefix(".github/workflows/")
        .unwrap_or(raw)
        .trim_start_matches('/');
    let well_formed = !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !well_formed {
        return Err(format!(
            "`workflow` must be a workflow file name like \"ci.yml\" (or its id), got {raw:?}"
        ));
    }
    Ok(Some(name.to_string()))
}

pub(super) fn run_list_filter(args: &Value) -> Result<RunListFilter, String> {
    let status = match args.get("status") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if RUN_STATUSES.contains(&s.trim()) => Some(s.trim().to_string()),
        Some(other) => {
            return Err(format!(
                "`status` must be one of {}, got {other}",
                RUN_STATUSES.join(", ")
            ))
        }
    };
    let event = match args.get("event") {
        None | Some(Value::Null) => None,
        Some(Value::String(s))
            if !s.trim().is_empty()
                && s.trim().chars().all(|c| c.is_ascii_lowercase() || c == '_') =>
        {
            Some(s.trim().to_string())
        }
        Some(other) => {
            return Err(format!(
                "`event` must be a trigger name like \"push\" or \"workflow_dispatch\", got {other}"
            ))
        }
    };
    Ok(RunListFilter {
        branch: arg_branch_named(args, "branch"),
        workflow: workflow_arg(args)?,
        status,
        event,
        limit: positive_u32_arg(args, "limit")?
            .unwrap_or(RUN_LIST_DEFAULT_LIMIT)
            .min(crate::github::RUN_LIST_MAX),
    })
}

#[cfg(test)]
#[path = "tests/runs.rs"]
mod tests;
