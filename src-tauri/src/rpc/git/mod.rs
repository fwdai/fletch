mod args;
mod commands;
mod publishing;
mod review_threads;
mod targets;

#[cfg(test)]
#[path = "tests/support.rs"]
mod test_support;

use std::path::PathBuf;

use serde_json::Value;

use super::caps::AgentCaps;
use super::{Response, RpcDispatcher, RpcEvent, RpcFuture};

pub const EVENT_BRANCH_CREATED: &str = "git.branch_created";

pub const EVENT_PR_OPENED: &str = "git.pr_opened";

/// Emitted when a mutating op succeeds, so the UI attributes the git/PR state
/// change to the agent's turn instead of inferring it from a polled snapshot.
pub const EVENT_ACTION_DONE: &str = "git.action_done";

/// Host-brokered ops whose success means the agent did the delegated work. Local
/// mutations signal out-of-band via the clone's hooks (`signal_git_action`).
fn is_mutating_op(op: &str) -> bool {
    matches!(
        op,
        "git_push" | "open_pr" | "reply_thread" | "resolve_thread"
    )
}

/// A closed set, so a compromised hook can't fabricate an op string into the
/// UI's delegation attribution.
fn is_signalable_action(action: &str) -> bool {
    matches!(action, "git_commit" | "git_update_branch")
}

#[derive(Clone)]
pub struct GitDispatcher {
    cwd: PathBuf,
    base_branch: String,
    default_subdir: Option<String>,
    /// Force-push anchor for the default checkout; `None` fails a force closed.
    own_branch: Option<String>,
    /// Primary included. Empty without `with_repos`, so `args.repo` is then rejected.
    repos: std::collections::HashMap<String, (PathBuf, String, Option<String>)>,
    /// Resolved at open_pr time, not construction, so a mid-session issue pick
    /// reaches the closing trailer.
    issue_agent: Option<String>,
    /// Stamped at spawn and never re-read, so a later policy change cannot widen a
    /// running agent.
    caps: AgentCaps,
    /// `None` (tests) makes an enabled gate refuse rather than pass.
    approval: Option<(tauri::AppHandle, String)>,
}

impl GitDispatcher {
    /// `caps` is required rather than defaulted: its wrong value is a security bug.
    pub fn new(cwd: PathBuf, base_branch: String, caps: AgentCaps) -> Self {
        Self {
            cwd,
            base_branch,
            default_subdir: None,
            own_branch: None,
            repos: std::collections::HashMap::new(),
            issue_agent: None,
            caps,
            approval: None,
        }
    }

    #[cfg(test)]
    pub fn with_own_branch(mut self, branch: Option<String>) -> Self {
        self.own_branch = branch;
        self
    }

    pub fn with_approval(mut self, app: tauri::AppHandle, agent_id: &str) -> Self {
        self.approval = Some((app, agent_id.to_string()));
        self
    }

    pub fn with_close_issue(mut self, agent_id: &str, issue_ref: Option<String>) -> Self {
        crate::issues::set_live_issue_ref(agent_id, issue_ref);
        self.issue_agent = Some(agent_id.to_string());
        self
    }

    pub fn with_repos(mut self, repos: Vec<(String, PathBuf, String, Option<String>)>) -> Self {
        self.repos = repos
            .into_iter()
            .map(|(subdir, cwd, base, own)| (subdir, (cwd, base, own)))
            .collect();
        let primary = self.repos.iter().find(|(_, (cwd, _, _))| *cwd == self.cwd);
        self.default_subdir = primary.map(|(subdir, _)| subdir.clone());
        self.own_branch = primary.and_then(|(_, (_, _, own))| own.clone());
        self
    }

    async fn dispatch_inner(&self, id: &str, op: &str, args: &Value) -> (Response, Vec<RpcEvent>) {
        if let Some(why) = self.caps.refuses(op) {
            return (Response::err(id, why), Vec::new());
        }
        let (resp, mut effects) = match op {
            "open_pr" => self.open_pr(id, args).await,
            "git_push" => self.git_push(id, args).await,
            "git_fetch" => (self.git_fetch(id, args).await, Vec::new()),
            "signal_git_action" => self.signal_git_action(id, args),
            "pr_threads" => (self.pr_threads(id, args).await, Vec::new()),
            "reply_thread" => (self.reply_thread(id, args).await, Vec::new()),
            "resolve_thread" => (self.resolve_thread(id, args).await, Vec::new()),
            "echo" => (self.echo(id, args).await, Vec::new()),
            "ping" => (
                Response::ok(id, 0, "pong".to_string(), String::new()),
                Vec::new(),
            ),
            "git_status" => (self.git_status(id, args).await, Vec::new()),
            other => (
                Response::err(id, format!("unknown op: {other}")),
                Vec::new(),
            ),
        };
        if resp.ok && is_mutating_op(op) {
            effects.push(RpcEvent::named(
                EVENT_ACTION_DONE,
                serde_json::json!({ "op": op }),
            ));
        }
        (resp, effects)
    }

    async fn echo(&self, id: &str, args: &Value) -> Response {
        let message = args.get("message").and_then(|v| v.as_str()).unwrap_or("");
        if message.is_empty() {
            return Response::err(id, "echo requires a non-empty `message` arg");
        }
        Response::ok(id, 0, message.to_string(), String::new())
    }

    /// The clone's hook can't emit an app event itself; this relays it as
    /// `EVENT_ACTION_DONE`. No git runs.
    fn signal_git_action(&self, id: &str, args: &Value) -> (Response, Vec<RpcEvent>) {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
        if !is_signalable_action(action) {
            return (
                Response::err(id, format!("signal_git_action: unknown action {action:?}")),
                Vec::new(),
            );
        }
        let effects = vec![RpcEvent::named(
            EVENT_ACTION_DONE,
            serde_json::json!({ "op": action }),
        )];
        (Response::ok(id, 0, String::new(), String::new()), effects)
    }
}

impl RpcDispatcher for GitDispatcher {
    fn dispatch<'a>(
        &'a self,
        id: &'a str,
        op: &'a str,
        args: &'a Value,
    ) -> RpcFuture<'a, (Response, Vec<RpcEvent>)> {
        Box::pin(async move { self.dispatch_inner(id, op, args).await })
    }
}

#[cfg(test)]
#[path = "tests/dispatcher.rs"]
mod tests;
