//! Read-only PR ops: the sanctioned way for an agent to see its PR, CI and
//! conversation, since it holds no GitHub credentials of its own. Not gated by
//! `AgentCaps` and never an `EVENT_ACTION_DONE`: reading changes nothing.

use serde::Serialize;
use serde_json::Value;

use super::GitDispatcher;
use crate::rpc::Response;

const DEFAULT_COMMENT_LIMIT: u32 = 50;
/// `issue_comments` reads one 100-item page, so a larger limit could not be honoured.
const MAX_COMMENT_LIMIT: u32 = 100;

fn not_answered(op: &str) -> String {
    format!(
        "{op}: GitHub did not answer (not connected, not a GitHub origin, or rate-limited); \
         try again shortly"
    )
}

fn json_ok(id: &str, op: &str, value: &impl Serialize) -> Response {
    match serde_json::to_string_pretty(value) {
        Ok(json) => Response::ok(id, 0, json, String::new()),
        Err(e) => Response::err(id, format!("{op}: {e}")),
    }
}

/// `Ok(None)` when `key` is absent or null; an error for anything but a
/// positive integer that fits the u32 the fetchers take.
fn positive_u32_arg(args: &Value, key: &str) -> Result<Option<u32>, String> {
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
        .min(MAX_COMMENT_LIMIT))
}

/// The current branch's PR number, or the error response to return.
async fn branch_pr_number(id: &str, op: &str, cwd: &std::path::Path) -> Result<u32, Response> {
    match crate::github::pr_view(cwd).await {
        Ok(Some(pr)) => Ok(pr.number),
        Ok(None) => Err(Response::err(
            id,
            format!("{op}: no pull request for this branch"),
        )),
        Err(e) => Err(Response::err(id, format!("{op}: {e}"))),
    }
}

impl GitDispatcher {
    pub(super) async fn pr_view(&self, id: &str, args: &Value) -> Response {
        let t = match self.target(id, args) {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        match crate::github::pr_view(&t.cwd).await {
            Ok(Some(pr)) => json_ok(id, "pr_view", &pr),
            Ok(None) => Response::err(id, "pr_view: no pull request for this branch"),
            Err(e) => Response::err(id, format!("pr_view: {e}")),
        }
    }

    /// `args.number` targets a specific PR instead of the branch's.
    pub(super) async fn pr_checks(&self, id: &str, args: &Value) -> Response {
        let number = match positive_u32_arg(args, "number") {
            Ok(n) => n,
            Err(e) => return Response::err(id, format!("pr_checks: {e}")),
        };
        let t = match self.target(id, args) {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        let number = match number {
            Some(n) => n,
            None => match branch_pr_number(id, "pr_checks", &t.cwd).await {
                Ok(n) => n,
                Err(resp) => return resp,
            },
        };
        match crate::github::pr_checks_live(&t.cwd, None, number).await {
            Ok(Some(checks)) => json_ok(id, "pr_checks", &checks),
            Ok(None) => Response::err(id, not_answered("pr_checks")),
            Err(e) => Response::err(id, format!("pr_checks: {e}")),
        }
    }

    /// The PR's conversation (issue) comments, newest `args.limit` kept. Review
    /// threads are `pr_threads`.
    pub(super) async fn pr_comments(&self, id: &str, args: &Value) -> Response {
        let limit = match comment_limit(args) {
            Ok(l) => l,
            Err(e) => return Response::err(id, format!("pr_comments: {e}")),
        };
        let t = match self.target(id, args) {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        let number = match branch_pr_number(id, "pr_comments", &t.cwd).await {
            Ok(n) => n,
            Err(resp) => return resp,
        };
        match crate::github::issue_comments(&t.cwd, &number.to_string(), limit).await {
            Ok(Some(comments)) => json_ok(id, "pr_comments", &comments),
            Ok(None) => Response::err(id, not_answered("pr_comments")),
            Err(e) => Response::err(id, format!("pr_comments: {e}")),
        }
    }
}
