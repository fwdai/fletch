mod args;
mod commands;
mod issue_reads;
mod pr_reads;
mod pr_writes;
mod publishing;
mod releases;
mod review_threads;
mod runs;
mod targets;
mod tour;

#[cfg(test)]
#[path = "tests/support.rs"]
mod test_support;

use std::path::PathBuf;

use serde_json::Value;

use super::caps::AgentCaps;
use super::{Response, RpcDispatcher, RpcEvent, RpcFuture};

/// Emitted once a push has created a branch on origin that was not there
/// before: payload `{ "branch", "current" }` (plus `repo` for a sibling
/// checkout). The watcher records the branch as the workspace's own
/// (`worktree_branches`), and as the checkout's current branch when `current`
/// — false for a stack's other head published through `open_pr`'s `args.head`.
/// Never emitted for a push that failed or for a name any of origin's push
/// URLs already had.
pub const EVENT_BRANCH_CREATED: &str = "git.branch_created";

pub const EVENT_PR_OPENED: &str = "git.pr_opened";

/// Emitted by `pr_edit` when it retargets a PR: payload `{ "number", "base" }`
/// (plus `repo` for a sibling checkout). The watcher records the base on that
/// PR, so every base-keyed reader follows it while it is the focused PR, as it
/// does for `EVENT_PR_OPENED`.
pub const EVENT_PR_EDITED: &str = "git.pr_edited";

/// Emitted when a mutating op succeeds, so the UI attributes the git/PR state
/// change to the agent's turn instead of inferring it from a polled snapshot.
pub const EVENT_ACTION_DONE: &str = "git.action_done";

/// Emitted by `set_title`: the agent named the work this workspace is doing.
/// Payload `{ "title": "..." }`; the watcher persists it on the workspace row.
pub const EVENT_TITLE_SET: &str = "workspace.title_set";

/// Longest title `set_title` accepts. A sidebar subtitle, not a paragraph.
const MAX_TITLE_CHARS: usize = 80;

/// Emitted by `tour_set`: the agent wrote a walkthrough of its changes.
/// Payload `{ "steps": [TourStep] }`, already validated and normalized; the
/// watcher stores it as the workspace's tour, replacing any earlier one.
pub const EVENT_TOUR_SET: &str = "workspace.tour_set";

/// Host-brokered ops whose success means the agent did the delegated work. Local
/// mutations signal out-of-band via the clone's hooks (`signal_git_action`).
fn is_mutating_op(op: &str) -> bool {
    matches!(
        op,
        "git_push"
            | "open_pr"
            | "pr_edit"
            | "pr_comment"
            | "pr_close"
            | "pr_merge"
            | "workflow_dispatch"
            | "issue_comment"
            | "reply_thread"
            | "resolve_thread"
    )
}

/// A closed set, so a compromised hook can't fabricate an op string into the
/// UI's delegation attribution.
fn is_signalable_action(action: &str) -> bool {
    matches!(action, "git_commit" | "git_update_branch")
}

/// One checkout the dispatcher can act on, as `with_repos` records it.
#[derive(Clone)]
pub struct RepoTarget {
    /// The checkout's folder name under the agent dir; what `args.repo` names.
    pub subdir: String,
    pub cwd: PathBuf,
    /// The user's own repository `cwd` was cloned from (`Target::source_repo`).
    pub source_repo: PathBuf,
    /// The base the checkout's work is reviewed against now: the focused PR's
    /// base, else the spawn base. The default base of `open_pr`.
    pub base_branch: String,
    /// The branch the checkout was forked from. Protected like `base_branch`,
    /// whatever the work is stacked on now.
    pub spawn_base: String,
    /// The branches this checkout's pushes created on origin
    /// (`WorkspaceManager::owned_branches`): the ones a force may rewrite, and
    /// never a review base to the guard.
    pub owned: Vec<String>,
}

#[derive(Clone)]
pub struct GitDispatcher {
    cwd: PathBuf,
    /// The user's own repository `cwd` was cloned from — required, never
    /// defaulted to `cwd`: a publish reads its signing config there, and `cwd`
    /// is the agent's to write.
    source_repo: PathBuf,
    base_branch: String,
    /// See `RepoTarget::spawn_base`; `base_branch` until `with_repos`.
    spawn_base: String,
    default_subdir: Option<String>,
    /// See `RepoTarget::owned`, for the default checkout. Empty fails a force
    /// closed.
    owned: Vec<String>,
    /// Primary included. Empty without `with_repos`, so `args.repo` is then rejected.
    repos: std::collections::HashMap<String, RepoTarget>,
    /// Resolved at open_pr time, not construction, so a mid-session issue pick
    /// reaches the closing trailer.
    issue_agent: Option<String>,
    /// Stamped at spawn and never re-read, so a later policy change cannot widen a
    /// running agent.
    caps: AgentCaps,
    /// `None` (tests) makes an enabled gate refuse rather than pass.
    approval: Option<(crate::host::Sink, String)>,
    /// The engine and this agent's id, for the local review threads
    /// (`review::store`) the thread ops read and answer beside GitHub's.
    /// `None` (tests without an engine) makes a local thread id an error.
    review: Option<(std::sync::Arc<crate::host::EngineCtx>, String)>,
}

impl GitDispatcher {
    /// `caps` is required rather than defaulted: its wrong value is a security
    /// bug. So is `source_repo`, for the same reason (see the field).
    pub fn new(cwd: PathBuf, source_repo: PathBuf, base_branch: String, caps: AgentCaps) -> Self {
        Self {
            cwd,
            source_repo,
            spawn_base: base_branch.clone(),
            base_branch,
            default_subdir: None,
            owned: Vec::new(),
            repos: std::collections::HashMap::new(),
            issue_agent: None,
            caps,
            approval: None,
            review: None,
        }
    }

    /// Reach the local review threads of this agent's checkouts.
    pub fn with_review(
        mut self,
        ctx: std::sync::Arc<crate::host::EngineCtx>,
        agent_id: &str,
    ) -> Self {
        self.review = Some((ctx, agent_id.to_string()));
        self
    }

    /// The branches the host recorded as this workspace's own.
    #[cfg(test)]
    pub fn with_owned(mut self, branches: &[&str]) -> Self {
        self.owned = branches.iter().map(|b| b.to_string()).collect();
        self
    }

    /// A review base other than the spawn base: the focused PR is stacked.
    #[cfg(test)]
    pub fn with_base(mut self, base: &str) -> Self {
        self.base_branch = base.to_string();
        self
    }

    pub fn with_approval(mut self, sink: crate::host::Sink, agent_id: &str) -> Self {
        self.approval = Some((sink, agent_id.to_string()));
        self
    }

    pub fn with_close_issue(mut self, agent_id: &str, issue_ref: Option<String>) -> Self {
        crate::issues::set_live_issue_ref(agent_id, issue_ref);
        self.issue_agent = Some(agent_id.to_string());
        self
    }

    pub fn with_repos(mut self, repos: Vec<RepoTarget>) -> Self {
        self.repos = repos
            .into_iter()
            .map(|repo| (repo.subdir.clone(), repo))
            .collect();
        let primary = self.repos.values().find(|repo| repo.cwd == self.cwd);
        self.default_subdir = primary.map(|repo| repo.subdir.clone());
        if let Some(primary) = primary {
            self.spawn_base = primary.spawn_base.clone();
            self.owned = primary.owned.clone();
        }
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
            "git_ls_remote" => (self.git_ls_remote(id, args).await, Vec::new()),
            "signal_git_action" => self.signal_git_action(id, args),
            "pr_view" => (self.pr_view(id, args).await, Vec::new()),
            "pr_list" => (self.pr_list(id, args).await, Vec::new()),
            "repo_view" => (self.repo_view(id, args).await, Vec::new()),
            "issue_view" => (self.issue_view(id, args).await, Vec::new()),
            "issue_list" => (self.issue_list(id, args).await, Vec::new()),
            "pr_checks" => (self.pr_checks(id, args).await, Vec::new()),
            "pr_check_log" => (self.pr_check_log(id, args).await, Vec::new()),
            "run_list" => (self.run_list(id, args).await, Vec::new()),
            "run_log" => (self.run_log(id, args).await, Vec::new()),
            "workflow_dispatch" => (self.workflow_dispatch(id, args).await, Vec::new()),
            "release_list" => (self.release_list(id, args).await, Vec::new()),
            "release_view" => (self.release_view(id, args).await, Vec::new()),
            "pr_comments" => (self.pr_comments(id, args).await, Vec::new()),
            "pr_edit" => self.pr_edit(id, args).await,
            "pr_comment" => (self.pr_comment(id, args).await, Vec::new()),
            "pr_close" => (self.pr_close(id, args).await, Vec::new()),
            "pr_merge" => (self.pr_merge(id, args).await, Vec::new()),
            "issue_comment" => (self.issue_comment(id, args).await, Vec::new()),
            "pr_threads" => (self.pr_threads(id, args).await, Vec::new()),
            "reply_thread" => (self.reply_thread(id, args).await, Vec::new()),
            "resolve_thread" => (self.resolve_thread(id, args).await, Vec::new()),
            "echo" => (self.echo(id, args).await, Vec::new()),
            "set_title" => self.set_title(id, args),
            "tour_set" => self.tour_set(id, args),
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

    /// The agent names the work it is doing. Validated here and relayed as
    /// `EVENT_TITLE_SET`; the supervisor's watcher owns the persistence, like
    /// it does for branch and PR events. No git runs.
    fn set_title(&self, id: &str, args: &Value) -> (Response, Vec<RpcEvent>) {
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
        if title.is_empty() {
            return (
                Response::err(id, "set_title requires a non-empty `title` arg"),
                Vec::new(),
            );
        }
        if title.chars().count() > MAX_TITLE_CHARS {
            return (
                Response::err(
                    id,
                    format!("set_title: title exceeds {MAX_TITLE_CHARS} characters; keep it to a few words"),
                ),
                Vec::new(),
            );
        }
        (
            Response::ok(id, 0, title.clone(), String::new()),
            vec![RpcEvent::named(
                EVENT_TITLE_SET,
                serde_json::json!({ "title": title }),
            )],
        )
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
