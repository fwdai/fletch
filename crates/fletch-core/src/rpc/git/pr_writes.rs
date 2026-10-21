//! Edits to a pull request the agent makes with the host's credentials.
//!
//! The description written at `open_pr` goes stale the moment a follow-up
//! commit lands, and the `## Decisions` section Fletch harvests at merge lives
//! in that body. Without a way to edit it, agents reported "I can't edit the
//! PR body from here" and handed the text to the user; this is the op they
//! were missing. A base change also moves the branch the checkout is reviewed
//! against, so it is relayed as [`EVENT_PR_EDITED`] for the record.

use serde_json::{json, Value};

use super::args::{arg_branch_named, refuse_option_like};
use super::pr_reads::{branch_pr, json_ok, positive_u32_arg};
use super::targets::{with_repo, Target};
use super::{GitDispatcher, EVENT_PR_EDITED};
use crate::github::PrEdit;
use crate::rpc::{Response, RpcEvent};

impl GitDispatcher {
    /// `args.number` targets a PR other than the branch's; `title`, `body`,
    /// `base` and `draft` are each optional, at least one is required.
    pub(super) async fn pr_edit(&self, id: &str, args: &Value) -> (Response, Vec<RpcEvent>) {
        let mut edit = match parse_edit(args) {
            Ok(edit) => edit,
            Err(e) => return (Response::err(id, format!("pr_edit: {e}")), Vec::new()),
        };
        let number = match positive_u32_arg(args, "number") {
            Ok(n) => n,
            Err(e) => return (Response::err(id, format!("pr_edit: {e}")), Vec::new()),
        };
        if let Some(base) = &edit.base {
            if let Some(resp) = refuse_option_like(id, "pr_edit", "base", base) {
                return (resp, Vec::new());
            }
        }
        let t = match self.pr_read_target(id, "pr_edit", args).await {
            Ok(t) => t,
            Err(resp) => return (resp, Vec::new()),
        };
        // Probed the way `open_pr` probes a requested base, so a typo cannot
        // retarget the PR at a branch GitHub then refuses with a bare 422.
        if let Some(base) = &edit.base {
            if crate::git::remote_branch_exists(&t.cwd, &t.source_repo, base).await == Some(false) {
                return (
                    Response::err(
                        id,
                        format!(
                            "pr_edit: base branch {base:?} does not exist on origin — \
                             pass an existing branch as `args.base`"
                        ),
                    ),
                    Vec::new(),
                );
            }
        }
        let number = match number {
            Some(n) => n,
            None => match branch_pr(id, "pr_edit", &t).await {
                Ok(pr) => pr.number,
                Err(resp) => return (resp, Vec::new()),
            },
        };
        // A replaced body keeps the closing trailer `open_pr` added, as that
        // op would: the issue is still the one this work closes.
        if let Some(body) = edit.body.take() {
            edit.body = Some(crate::github::with_closes_trailer(
                &body,
                self.live_closes(&t).as_deref(),
            ));
        }
        let edited = edited_fields(&edit);
        // In the user's repository, whose origin no agent can repoint.
        match crate::github::pr_edit(&t.source_repo, number, &edit).await {
            Ok(url) => {
                let mut effects = Vec::new();
                if let Some(base) = &edit.base {
                    effects.push(RpcEvent::named(
                        EVENT_PR_EDITED,
                        with_repo(json!({ "number": number, "base": base }), &t.subdir),
                    ));
                }
                let mut summary = json!({ "number": number, "edited": edited });
                if let Some(url) = url {
                    summary["url"] = json!(url);
                }
                (json_ok(id, "pr_edit", &summary), effects)
            }
            Err(e) => (Response::err(id, format!("pr_edit: {e}")), Vec::new()),
        }
    }

    /// A comment on the PR's conversation (`args.body`; `args.number` for
    /// another PR). The place to say what a follow-up push changed, which
    /// agents did with `gh pr comment` and three guessed op names.
    pub(super) async fn pr_comment(&self, id: &str, args: &Value) -> Response {
        self.comment_on(id, "pr_comment", args, false).await
    }

    /// A comment on issue `args.number` (required).
    pub(super) async fn issue_comment(&self, id: &str, args: &Value) -> Response {
        self.comment_on(id, "issue_comment", args, true).await
    }

    async fn comment_on(&self, id: &str, op: &str, args: &Value, issue: bool) -> Response {
        let body = match args.get("body").and_then(Value::as_str).map(str::trim) {
            Some(b) if !b.is_empty() => b.to_string(),
            _ => return Response::err(id, format!("{op} requires a non-empty `body`")),
        };
        let number = match positive_u32_arg(args, "number") {
            Ok(n) => n,
            Err(e) => return Response::err(id, format!("{op}: {e}")),
        };
        if issue && number.is_none() {
            return Response::err(id, format!("{op} requires `number`"));
        }
        let t = match self.pr_read_target(id, op, args).await {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        let number = match number {
            Some(n) => n,
            None => match branch_pr(id, op, &t).await {
                Ok(pr) => pr.number,
                Err(resp) => return resp,
            },
        };
        match crate::github::post_comment(&t.source_repo, number, &body).await {
            Ok(url) => json_ok(id, op, &json!({ "number": number, "url": url })),
            Err(e) => Response::err(id, format!("{op}: {e}")),
        }
    }

    /// The issue the primary checkout's PR closes, read live so a composer
    /// pick made after spawn still reaches the trailer. `None` for a sibling
    /// checkout, whose PR is not the one bound to the issue.
    pub(super) fn live_closes(&self, t: &Target) -> Option<String> {
        if t.subdir != self.default_subdir {
            return None;
        }
        self.issue_agent
            .as_deref()
            .and_then(crate::issues::live_issue_ref)
    }
}

/// The optional string field `key`, trimmed; an empty or non-string value
/// is refused rather than written blank over the PR.
fn text_field(args: &Value, key: &str) -> Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.trim().to_string())),
        Some(Value::String(_)) => Err(format!("`{key}` must not be empty")),
        Some(other) => Err(format!("`{key}` must be a string, got {other}")),
    }
}

fn parse_edit(args: &Value) -> Result<PrEdit, String> {
    let draft = match args.get("draft") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(other) => return Err(format!("`draft` must be true or false, got {other}")),
    };
    let edit = PrEdit {
        title: text_field(args, "title")?,
        // The body keeps its inner line breaks: only a blank body is refused.
        body: match args.get("body") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
            Some(Value::String(_)) => return Err("`body` must not be empty".into()),
            Some(other) => return Err(format!("`body` must be a string, got {other}")),
        },
        base: arg_branch_named(args, "base"),
        draft,
    };
    if edit.is_empty() {
        return Err(
            "nothing to edit — pass at least one of `title`, `body`, `base`, `draft`".into(),
        );
    }
    Ok(edit)
}

fn edited_fields(edit: &PrEdit) -> Vec<&'static str> {
    [
        ("title", edit.title.is_some()),
        ("body", edit.body.is_some()),
        ("base", edit.base.is_some()),
        ("draft", edit.draft.is_some()),
    ]
    .into_iter()
    .filter_map(|(name, set)| set.then_some(name))
    .collect()
}

#[cfg(test)]
#[path = "tests/pr_edit.rs"]
mod tests;
