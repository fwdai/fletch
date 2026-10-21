//! Read-only PR ops: the sanctioned way for an agent to see its PR, CI and
//! conversation, since it holds no GitHub credentials of its own. Not gated by
//! `AgentCaps` and never an `EVENT_ACTION_DONE`: reading changes nothing.
//!
//! The fetchers answer `Ok(None)` for every quiet degradation alike — no
//! origin, no token, a rate-limit pause, a PR that isn't there — because their
//! other callers poll in the background. An agent acts on the answer, so each
//! op tells those apart up front ([`GitDispatcher::pr_read_target`]) and only
//! then reads "none" as "no such pull request".
//!
//! The repository is resolved from the user's repository (`Target::source_repo`),
//! whose `origin` no agent can write, never from the checkout's: a repointed
//! checkout origin would otherwise read another repository's pull requests
//! with the user's token. The checkout supplies only its current branch.

use serde::Serialize;
use serde_json::Value;

use super::targets::Target;
use super::GitDispatcher;
use crate::rpc::Response;

const DEFAULT_COMMENT_LIMIT: u32 = 50;

pub(super) fn json_ok(id: &str, op: &str, value: &impl Serialize) -> Response {
    match serde_json::to_string_pretty(value) {
        Ok(json) => Response::ok(id, 0, json, String::new()),
        Err(e) => Response::err(id, format!("{op}: {e}")),
    }
}

/// `Ok(None)` when `key` is absent or null; an error for anything but a
/// positive integer that fits the u32 the fetchers take.
pub(super) fn positive_u32_arg(args: &Value, key: &str) -> Result<Option<u32>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .filter(|n| *n > 0)
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| format!("`{key}` must be a positive integer, got {v}")),
    }
}

pub(super) fn comment_limit(args: &Value) -> Result<u32, String> {
    Ok(positive_u32_arg(args, "limit")?
        .unwrap_or(DEFAULT_COMMENT_LIMIT)
        .min(crate::github::CONVERSATION_COMMENTS_MAX))
}

/// GitHub answers "not found" for what the token cannot see as well as for
/// what is not there, so no read may claim absence outright.
const NO_BRANCH_PR: &str =
    "no pull request for this branch, or the host's GitHub account cannot see this repository";

fn not_found_or_hidden(number: u32) -> String {
    format!(
        "pull request #{number} was not found in this repository, or is not visible to the \
         host's GitHub account"
    )
}

/// The PR for the branch checked out in `t.cwd`, looked up in the user's
/// repository. Reaches the fetcher only after `pr_read_target`, so `None`
/// here is a branch with no PR (or a detached HEAD) — or a repository the
/// token cannot read, which GitHub reports the same way.
pub(super) async fn branch_pr(
    id: &str,
    op: &str,
    t: &Target,
) -> Result<crate::github::PrState, Response> {
    let fail = |e: crate::error::Error| Response::err(id, format!("{op}: {e}"));
    let Some(branch) = crate::git::current_branch(&t.cwd).await.map_err(fail)? else {
        return Err(Response::err(id, format!("{op}: {NO_BRANCH_PR}")));
    };
    match crate::github::pr_view_branch(&t.source_repo, &branch).await {
        Ok(Some(pr)) => Ok(pr),
        Ok(None) => Err(Response::err(id, format!("{op}: {NO_BRANCH_PR}"))),
        Err(e) => Err(fail(e)),
    }
}

impl GitDispatcher {
    /// The target once the reasons a read could answer nothing have been
    /// ruled out, each with its own message: a user's repository whose origin
    /// isn't GitHub, no host token, a rate-limit pause (retryable). Checked in
    /// that order so a repository with no remote answers the same with or
    /// without a token.
    pub(super) async fn pr_read_target(
        &self,
        id: &str,
        op: &str,
        args: &Value,
    ) -> Result<Target, Response> {
        let t = self.target(id, args)?;
        if crate::github::repo_ref(&t.source_repo).await.is_none() {
            return Err(Response::err(
                id,
                format!("{op}: origin is not a GitHub repository (or there is no origin)"),
            ));
        }
        if crate::github::client::token().is_none() {
            return Err(Response::err(
                id,
                format!("{op}: the host is not connected to GitHub — the user must sign in"),
            ));
        }
        if crate::github::client::is_backing_off() {
            return Err(Response::err(
                id,
                format!(
                    "{op}: GitHub rate limit reached; the host is backing off — try again shortly"
                ),
            ));
        }
        Ok(t)
    }

    pub(super) async fn pr_view(&self, id: &str, args: &Value) -> Response {
        let t = match self.pr_read_target(id, "pr_view", args).await {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        match branch_pr(id, "pr_view", &t).await {
            Ok(pr) => json_ok(id, "pr_view", &pr),
            Err(resp) => resp,
        }
    }

    /// `args.number` targets a specific PR instead of the branch's.
    pub(super) async fn pr_checks(&self, id: &str, args: &Value) -> Response {
        let number = match positive_u32_arg(args, "number") {
            Ok(n) => n,
            Err(e) => return Response::err(id, format!("pr_checks: {e}")),
        };
        let t = match self.pr_read_target(id, "pr_checks", args).await {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        let number = match number {
            Some(n) => n,
            None => match branch_pr(id, "pr_checks", &t).await {
                Ok(pr) => pr.number,
                Err(resp) => return resp,
            },
        };
        // A 404 is "not found or not visible" and says so; any other silence
        // is a retry, not a wrong-PR hunt.
        match crate::github::pr_checks_read(&t.source_repo, None, number).await {
            Ok(crate::github::ChecksRead::Checks(checks)) => json_ok(id, "pr_checks", &checks),
            Ok(crate::github::ChecksRead::NotFound) => {
                Response::err(id, format!("pr_checks: {}", not_found_or_hidden(number)))
            }
            Ok(crate::github::ChecksRead::Unavailable(why)) => Response::err(
                id,
                format!("pr_checks: GitHub did not answer ({why}); try again shortly"),
            ),
            Err(e) => Response::err(id, format!("pr_checks: {e}")),
        }
    }

    /// The newest `args.limit` conversation comments on the branch's PR,
    /// oldest first. Review threads are `pr_threads`.
    pub(super) async fn pr_comments(&self, id: &str, args: &Value) -> Response {
        let limit = match comment_limit(args) {
            Ok(l) => l,
            Err(e) => return Response::err(id, format!("pr_comments: {e}")),
        };
        let t = match self.pr_read_target(id, "pr_comments", args).await {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        let number = match branch_pr(id, "pr_comments", &t).await {
            Ok(pr) => pr.number,
            Err(resp) => return resp,
        };
        match crate::github::pr_conversation_comments(&t.source_repo, number, limit).await {
            Ok(Some(comments)) => json_ok(id, "pr_comments", &comments),
            Ok(None) => Response::err(id, format!("pr_comments: {}", not_found_or_hidden(number))),
            Err(e) => Response::err(id, format!("pr_comments: {e}")),
        }
    }

    /// The tail of one check's job log plus its annotations. `args.check` is
    /// the run's `id` or `name` as `pr_checks` lists them; `args.lines` (default
    /// 200, max 2000) is how much of the log's end to return; `args.number`
    /// targets another PR. Without this, agents reading CI failures reached
    /// for `gh run view --log-failed`.
    pub(super) async fn pr_check_log(&self, id: &str, args: &Value) -> Response {
        let Some(wanted) = CheckRef::from_args(args) else {
            return Response::err(
                id,
                "pr_check_log requires `check`: a run's `id` or `name` as pr_checks lists them",
            );
        };
        let lines = match positive_u32_arg(args, "lines") {
            Ok(n) => (n.map(|n| n as usize))
                .unwrap_or(crate::github::CHECK_LOG_DEFAULT_LINES)
                .min(crate::github::CHECK_LOG_MAX_LINES),
            Err(e) => return Response::err(id, format!("pr_check_log: {e}")),
        };
        let number = match positive_u32_arg(args, "number") {
            Ok(n) => n,
            Err(e) => return Response::err(id, format!("pr_check_log: {e}")),
        };
        let t = match self.pr_read_target(id, "pr_check_log", args).await {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        let number = match number {
            Some(n) => n,
            None => match branch_pr(id, "pr_check_log", &t).await {
                Ok(pr) => pr.number,
                Err(resp) => return resp,
            },
        };
        let checks = match crate::github::pr_checks_read(&t.source_repo, None, number).await {
            Ok(crate::github::ChecksRead::Checks(checks)) => checks,
            Ok(crate::github::ChecksRead::NotFound) => {
                return Response::err(id, format!("pr_check_log: {}", not_found_or_hidden(number)))
            }
            Ok(crate::github::ChecksRead::Unavailable(why)) => {
                return Response::err(
                    id,
                    format!("pr_check_log: GitHub did not answer ({why}); try again shortly"),
                )
            }
            Err(e) => return Response::err(id, format!("pr_check_log: {e}")),
        };
        let run = match find_run(&checks.runs, &wanted) {
            Ok(run) => run,
            Err(why) => return Response::err(id, format!("pr_check_log: {why}")),
        };
        let Some(run_id) = run.id else {
            return Response::err(
                id,
                format!(
                    "pr_check_log: {:?} is a commit status, not a check run, so it has no job \
                     log; its url is {}",
                    run.name,
                    run.url.as_deref().unwrap_or("not given")
                ),
            );
        };
        match crate::github::check_log(&t.source_repo, run_id, lines).await {
            Ok(log) => {
                let mut out = serde_json::to_value(&log).unwrap_or_default();
                out["name"] = serde_json::json!(run.name);
                out["status"] = serde_json::json!(run.status);
                out["conclusion"] = serde_json::json!(run.conclusion);
                out["url"] = serde_json::json!(run.url);
                json_ok(id, "pr_check_log", &out)
            }
            Err(e) => Response::err(id, format!("pr_check_log: {e}")),
        }
    }
}

/// How a request names a check: by the id `pr_checks` reported, or by name.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum CheckRef {
    Id(u64),
    Name(String),
}

impl CheckRef {
    pub(super) fn from_args(args: &Value) -> Option<Self> {
        match args.get("check")? {
            Value::Number(n) => n.as_u64().filter(|n| *n > 0).map(Self::Id),
            Value::String(s) => {
                let s = s.trim();
                if s.is_empty() {
                    return None;
                }
                // "4242" as a string is still an id.
                Some(
                    s.parse()
                        .map(Self::Id)
                        .unwrap_or_else(|_| Self::Name(s.to_string())),
                )
            }
            _ => None,
        }
    }
}

/// The one run `wanted` names: an exact id, an exact name, or a unique
/// case-insensitive substring of a name. Anything else is an error that
/// lists the runs there are, so the agent can pick.
pub(super) fn find_run<'a>(
    runs: &'a [crate::github::CheckRun],
    wanted: &CheckRef,
) -> std::result::Result<&'a crate::github::CheckRun, String> {
    let names = || {
        runs.iter()
            .map(|r| match r.id {
                Some(id) => format!("{} (id {id})", r.name),
                None => r.name.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    match wanted {
        CheckRef::Id(id) => runs
            .iter()
            .find(|r| r.id == Some(*id))
            .ok_or_else(|| format!("no check with id {id}; the PR's checks are: {}", names())),
        CheckRef::Name(name) => {
            if let Some(run) = runs.iter().find(|r| r.name == *name) {
                return Ok(run);
            }
            let needle = name.to_lowercase();
            let loose: Vec<&crate::github::CheckRun> = runs
                .iter()
                .filter(|r| r.name.to_lowercase().contains(&needle))
                .collect();
            match loose.as_slice() {
                [one] => Ok(one),
                [] => Err(format!(
                    "no check named {name:?}; the PR's checks are: {}",
                    names()
                )),
                many => Err(format!(
                    "{name:?} matches several checks ({}); name one exactly or pass its id",
                    many.iter()
                        .map(|r| r.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
        }
    }
}
