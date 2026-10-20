//! Read-only PR ops: the sanctioned way for an agent to see its PR, CI and
//! conversation, since it holds no GitHub credentials of its own. Not gated by
//! `AgentCaps` and never an `EVENT_ACTION_DONE`: reading changes nothing.
//!
//! The fetchers answer `Ok(None)` for every quiet degradation alike — no
//! origin, no token, a rate-limit pause, a PR that isn't there — because their
//! other callers poll in the background. An agent acts on the answer, so each
//! op tells those apart up front ([`GitDispatcher::pr_read_target`]) and only
//! then reads "none" as "no such pull request".

use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use super::targets::Target;
use super::GitDispatcher;
use crate::rpc::Response;

const DEFAULT_COMMENT_LIMIT: u32 = 50;

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
        .min(crate::github::CONVERSATION_COMMENTS_MAX))
}

/// The current branch's PR number. Reaches the fetcher only after
/// `pr_read_target`, so `None` here is a branch with no PR.
async fn branch_pr_number(id: &str, op: &str, cwd: &Path) -> Result<u32, Response> {
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
    /// The target checkout once the reasons a read could answer nothing have
    /// been ruled out, each with its own message: an origin that isn't GitHub,
    /// no host token, a rate-limit pause (retryable). Checked in that order so
    /// a checkout with no remote answers the same with or without a token.
    async fn pr_read_target(&self, id: &str, op: &str, args: &Value) -> Result<Target, Response> {
        let t = self.target(id, args)?;
        if crate::github::repo_ref(&t.cwd).await.is_none() {
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
        let t = match self.pr_read_target(id, "pr_checks", args).await {
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
        // Absence is reported only when GitHub positively said so (a 404 for
        // the PR); any other silence is a retry, not a wrong-PR hunt.
        match crate::github::pr_checks_read(&t.cwd, None, number).await {
            Ok(crate::github::ChecksRead::Checks(checks)) => json_ok(id, "pr_checks", &checks),
            Ok(crate::github::ChecksRead::NoSuchPr) => Response::err(
                id,
                format!("pr_checks: GitHub has no pull request #{number} in this repository"),
            ),
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
        let number = match branch_pr_number(id, "pr_comments", &t.cwd).await {
            Ok(n) => n,
            Err(resp) => return resp,
        };
        match crate::github::pr_conversation_comments(&t.cwd, number, limit).await {
            Ok(Some(comments)) => json_ok(id, "pr_comments", &comments),
            Ok(None) => Response::err(
                id,
                format!("pr_comments: GitHub has no pull request #{number} in this repository"),
            ),
            Err(e) => Response::err(id, format!("pr_comments: {e}")),
        }
    }
}
