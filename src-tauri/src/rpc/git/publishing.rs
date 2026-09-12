use std::path::Path;

use serde_json::{json, Value};

use super::args::{arg_bool, arg_branch, arg_branch_named, refuse_option_like};
use super::targets::{approval_repo, with_repo};
use super::{GitDispatcher, EVENT_BRANCH_CREATED, EVENT_PR_OPENED};
use crate::rpc::{approval, Response, RpcEvent};

impl GitDispatcher {
    /// Fails closed when the gate is on but there is no window to ask through.
    async fn refuse_unless_publish_approved(
        &self,
        op: &str,
        repo: Option<&str>,
        detail: &str,
    ) -> Option<String> {
        if !approval::enabled() {
            return None;
        }
        let Some((app, agent_id)) = &self.approval else {
            return Some(format!(
                "not publishing ({detail}): publish confirmation is enabled but this \
                 session has no window to ask through"
            ));
        };
        // `None` for the primary: the frontend keys it unsuffixed, and a mismatched
        // key strands an unattended push on a prompt nobody answers.
        let repo = approval_repo(repo, self.default_subdir.as_deref());
        approval::refuse_unless_approved(app, agent_id, op, repo, detail).await
    }

    pub(super) async fn open_pr(&self, id: &str, args: &Value) -> (Response, Vec<RpcEvent>) {
        let t = match self.target(id, args) {
            Ok(t) => t,
            Err(resp) => return (resp, Vec::new()),
        };
        let title = args.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let body_arg = args.get("body").and_then(|v| v.as_str()).unwrap_or("");
        // Live issue lookup, not a spawn-time snapshot: a later composer pick must
        // reach the trailer. Idempotent.
        let live_ref = self
            .issue_agent
            .as_deref()
            .and_then(crate::issues::live_issue_ref);
        let closes = (t.subdir == self.default_subdir)
            .then_some(live_ref.as_deref())
            .flatten();
        let body_owned = crate::github::with_closes_trailer(body_arg, closes);
        let body = body_owned.as_str();
        let requested = arg_branch(args);
        // Resolved before the head branch is materialized so a malformed base fails
        // without a stray branch. The effective base is reported on `EVENT_PR_OPENED`,
        // which rewrites the checkout's recorded base.
        let requested_base = arg_branch_named(args, "base");
        let base = requested_base
            .clone()
            .unwrap_or_else(|| t.base_branch.clone());
        if let Some(resp) = refuse_option_like(id, "open_pr", "base", &base) {
            return (resp, Vec::new());
        }
        // Only the requested base is probed, so a typo can't leave a pushed branch
        // with no PR. `None` means "couldn't ask" and must not block.
        if requested_base.is_some()
            && crate::git::remote_branch_exists(&t.cwd, &base).await == Some(false)
        {
            return (
                Response::err(
                    id,
                    format!(
                        "open_pr: base branch {base:?} does not exist on origin — \
                         pass an existing branch as `args.base`"
                    ),
                ),
                Vec::new(),
            );
        }

        let current = match crate::git::current_branch(&t.cwd).await {
            Ok(b) => b,
            Err(e) => return (Response::err(id, format!("open_pr: {e}")), Vec::new()),
        };

        let mut effects = Vec::new();
        let branch = match (current, requested) {
            (Some(cur), None) => cur,
            (Some(cur), Some(req)) if req == cur => cur,
            (Some(_), Some(req)) => match materialize_branch(&t.cwd, &req).await {
                Ok(name) => {
                    effects.push(RpcEvent::named(
                        EVENT_BRANCH_CREATED,
                        with_repo(json!({ "branch": name }), &t.subdir),
                    ));
                    name
                }
                Err(e) => return (Response::err(id, format!("open_pr: {e}")), effects),
            },
            (None, req) => {
                let desired = req.unwrap_or_else(|| fallback_branch(title));
                match materialize_branch(&t.cwd, &desired).await {
                    Ok(name) => {
                        effects.push(RpcEvent::named(
                            EVENT_BRANCH_CREATED,
                            with_repo(json!({ "branch": name }), &t.subdir),
                        ));
                        name
                    }
                    Err(e) => return (Response::err(id, format!("open_pr: {e}")), effects),
                }
            }
        };

        if let Some(resp) = refuse_option_like(id, "open_pr", "branch", &branch) {
            return (resp, effects);
        }
        // Deliberately the *recorded* base, never `base`: reading it from the request
        // would let an agent name a decoy base and publish over the real one.
        if let Some(why) = self.caps.refuses_branch(&branch, &t.base_branch) {
            return (Response::err(id, why), effects);
        }
        if let Some(why) = self
            .refuse_unless_publish_approved(
                "open_pr",
                t.subdir.as_deref(),
                &format!("open a pull request from {branch} into {base}"),
            )
            .await
        {
            return (Response::err(id, why), effects);
        }
        if let Err(e) = crate::git::push(&t.cwd, &branch, false).await {
            return (
                Response::err(id, format!("open_pr push failed: {e}")),
                effects,
            );
        }
        match crate::github::pr_create(&t.cwd, title, body, &base).await {
            Ok(pr) => {
                crate::telemetry::track("pr_opened", json!({ "source": "agent_rpc" }));
                effects.push(RpcEvent::named(
                    EVENT_PR_OPENED,
                    with_repo(json!({ "number": pr.number, "base": base }), &t.subdir),
                ));
                (Response::ok(id, 0, pr.url, String::new()), effects)
            }
            Err(e) => (Response::err(id, format!("open_pr: {e}")), effects),
        }
    }

    pub(super) async fn git_push(&self, id: &str, args: &Value) -> (Response, Vec<RpcEvent>) {
        let t = match self.target(id, args) {
            Ok(t) => t,
            Err(resp) => return (resp, Vec::new()),
        };
        let cwd = t.cwd;
        let current = match crate::git::current_branch(&cwd).await {
            Ok(b) => b,
            Err(e) => return (Response::err(id, format!("git_push: {e}")), Vec::new()),
        };

        let mut effects = Vec::new();
        let branch = match current {
            Some(cur) => cur,
            None => match arg_branch(args) {
                Some(req) => match materialize_branch(&cwd, &req).await {
                    Ok(name) => {
                        effects.push(RpcEvent::named(
                            EVENT_BRANCH_CREATED,
                            with_repo(json!({ "branch": name }), &t.subdir),
                        ));
                        name
                    }
                    Err(e) => return (Response::err(id, format!("git_push: {e}")), effects),
                },
                None => {
                    return (
                        Response::err(
                            id,
                            "git_push: HEAD is detached — pass `args.branch` (e.g. \"fix/…\") to create the branch",
                        ),
                        effects,
                    )
                }
            },
        };

        if let Some(resp) = refuse_option_like(id, "git_push", "branch", &branch) {
            return (resp, effects);
        }
        if let Some(why) = self
            .refuse_unless_publish_approved(
                "git_push",
                t.subdir.as_deref(),
                &format!("push {branch}"),
            )
            .await
        {
            return (Response::err(id, why), effects);
        }

        // Earliest point `branch` is known; must precede the push.
        if let Some(why) = self.caps.refuses_branch(&branch, &t.base_branch) {
            return (Response::err(id, why), effects);
        }

        // Lease-based, so a stale local view can't clobber remote work it hasn't seen.
        let force = arg_bool(args, "force");
        // SECURITY: the lease passes for any branch the agent just fetched, so a force
        // is authorized against the spawn-stable own branch and fails closed when none
        // is recorded. Non-force pushes are untouched.
        if force {
            if let Some(why) = self.caps.refuses_force(&branch, t.own_branch.as_deref()) {
                return (Response::err(id, why), effects);
            }
        }
        match crate::git::push(&cwd, &branch, force).await {
            Ok(summary) => (Response::ok(id, 0, summary, String::new()), effects),
            Err(e) => (Response::err(id, e.to_string()), effects),
        }
    }
}

/// The one place an agent's branch is born, so the user's branch prefix is
/// applied here and nowhere else.
async fn materialize_branch(checkout: &Path, desired: &str) -> std::result::Result<String, String> {
    let desired = crate::publish_prefs::apply_branch_prefix(desired);
    crate::git::checkout_new_unique_branch(checkout, &desired)
        .await
        .map_err(|e| e.to_string())
}

fn fallback_branch(title: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = false;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "chore/update".to_string()
    } else {
        format!("chore/{slug}")
    }
}

#[cfg(test)]
#[path = "tests/open_pr.rs"]
mod open_pr_tests;

#[cfg(test)]
#[path = "tests/push.rs"]
mod push_tests;
