//! Read-only issue ops: an agent asked to look into "issue 914" or to triage
//! the tracker reads it here instead of through the GitHub CLI. Not gated by
//! `AgentCaps` and never an `EVENT_ACTION_DONE`, like the PR reads.

use serde_json::Value;

use super::pr_reads::{json_ok, positive_u32_arg};
use super::GitDispatcher;
use crate::rpc::Response;

const ISSUE_LIST_DEFAULT_LIMIT: u32 = 20;
const ISSUE_COMMENTS_DEFAULT_LIMIT: u32 = 50;

impl GitDispatcher {
    /// One issue by `args.number`, with its newest `args.comments` comments
    /// (default 50, max 100; 0 for none).
    pub(super) async fn issue_view(&self, id: &str, args: &Value) -> Response {
        let number = match positive_u32_arg(args, "number") {
            Ok(Some(n)) => n,
            Ok(None) => return Response::err(id, "issue_view requires `number`"),
            Err(e) => return Response::err(id, format!("issue_view: {e}")),
        };
        let comments = match args.get("comments") {
            None | Some(Value::Null) => ISSUE_COMMENTS_DEFAULT_LIMIT,
            Some(v) => match v.as_u64() {
                Some(n) => (n.min(u64::from(crate::github::ISSUE_LIST_MAX))) as u32,
                None => {
                    return Response::err(
                        id,
                        format!("issue_view: `comments` must be a non-negative integer, got {v}"),
                    )
                }
            },
        };
        let t = match self.pr_read_target(id, "issue_view", args).await {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        match crate::github::issue_view(&t.source_repo, number, comments).await {
            Ok(Some(issue)) => json_ok(id, "issue_view", &issue),
            Ok(None) => Response::err(
                id,
                format!(
                    "issue_view: issue #{number} was not found in this repository, or is not \
                     visible to the host's GitHub account"
                ),
            ),
            Err(e) => Response::err(id, format!("issue_view: {e}")),
        }
    }

    /// The repository's issues: `args.state` open (default), closed or all;
    /// `args.labels` a comma-separated list each must carry; `args.limit`
    /// default 20, max 100.
    pub(super) async fn issue_list(&self, id: &str, args: &Value) -> Response {
        let state = match args.get("state") {
            None | Some(Value::Null) => "open".to_string(),
            Some(Value::String(s)) if matches!(s.trim(), "open" | "closed" | "all") => {
                s.trim().to_string()
            }
            Some(other) => {
                return Response::err(
                    id,
                    format!(
                        "issue_list: `state` must be \"open\", \"closed\" or \"all\", got {other}"
                    ),
                )
            }
        };
        let labels = match args.get("labels") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
            Some(Value::Array(items)) => {
                let names: Vec<&str> = items.iter().filter_map(Value::as_str).collect();
                Some(names.join(",")).filter(|s| !s.is_empty())
            }
            Some(other) => {
                return Response::err(
                    id,
                    format!(
                        "issue_list: `labels` must be a string or a list of names, got {other}"
                    ),
                )
            }
        };
        let limit = match positive_u32_arg(args, "limit") {
            Ok(n) => n
                .unwrap_or(ISSUE_LIST_DEFAULT_LIMIT)
                .min(crate::github::ISSUE_LIST_MAX),
            Err(e) => return Response::err(id, format!("issue_list: {e}")),
        };
        let t = match self.pr_read_target(id, "issue_list", args).await {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        match crate::github::issue_list_all(&t.source_repo, &state, labels.as_deref(), limit).await
        {
            Ok(Some(issues)) => json_ok(id, "issue_list", &issues),
            Ok(None) => Response::err(
                id,
                "issue_list: GitHub did not list this repository's issues — it may not be \
                 visible to the host's GitHub account, or issues are disabled; try again shortly",
            ),
            Err(e) => Response::err(id, format!("issue_list: {e}")),
        }
    }
}

#[cfg(test)]
#[path = "tests/issues.rs"]
mod tests;
