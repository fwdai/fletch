use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};

use super::approval;
use super::caps::AgentCaps;
use super::{Response, RpcDispatcher, RpcEvent, RpcFuture};

/// A hung command surfaces as an error instead of blocking the watcher; the
/// agent's own poll timeout is shorter.
const OP_TIMEOUT: Duration = Duration::from_secs(120);

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

/// `subdir` is `None` for dispatchers built without `with_repos`; consumers then
/// fall back to the primary.
struct Target {
    subdir: Option<String>,
    cwd: PathBuf,
    base_branch: String,
    /// The force-push anchor. `None` until the host has recorded the branch and the
    /// dispatcher was rebuilt, which fails a force closed.
    own_branch: Option<String>,
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

    fn target(&self, id: &str, args: &Value) -> std::result::Result<Target, Response> {
        let requested = args
            .get("repo")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        match requested {
            None => Ok(Target {
                subdir: self.default_subdir.clone(),
                cwd: self.cwd.clone(),
                base_branch: self.base_branch.clone(),
                own_branch: self.own_branch.clone(),
            }),
            Some(name) => match self.repos.get(name) {
                Some((cwd, base, own)) => Ok(Target {
                    subdir: Some(name.to_string()),
                    cwd: cwd.clone(),
                    base_branch: base.clone(),
                    own_branch: own.clone(),
                }),
                None => {
                    let mut known: Vec<&str> = self.repos.keys().map(String::as_str).collect();
                    known.sort_unstable();
                    Err(Response::err(
                        id,
                        format!(
                            "unknown repo {name:?}; tracked checkouts: {}",
                            known.join(", ")
                        ),
                    ))
                }
            },
        }
    }
}

fn with_repo(mut payload: Value, subdir: &Option<String>) -> Value {
    if let Some(s) = subdir {
        payload["repo"] = json!(s);
    }
    payload
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

impl GitDispatcher {
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
            "git_status" => match self.target(id, args) {
                Ok(t) => (
                    run_git_command(id, &t.cwd, &["status", "--porcelain=v1", "--branch"], &[])
                        .await,
                    Vec::new(),
                ),
                Err(resp) => (resp, Vec::new()),
            },
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

    async fn pr_threads(&self, id: &str, args: &Value) -> Response {
        let t = match self.target(id, args) {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        let number = match crate::github::pr_view(&t.cwd).await {
            Ok(Some(pr)) => pr.number,
            Ok(None) => return Response::err(id, "pr_threads: no pull request for this branch"),
            Err(e) => return Response::err(id, format!("pr_threads: {e}")),
        };
        match crate::github::pr_threads_number(&t.cwd, None, number).await {
            Ok(Some(threads)) => match serde_json::to_string_pretty(&threads.unresolved) {
                Ok(json) => Response::ok(id, 0, json, String::new()),
                Err(e) => Response::err(id, format!("pr_threads: {e}")),
            },
            Ok(None) => Response::ok(id, 0, "[]".to_string(), String::new()),
            Err(e) => Response::err(id, format!("pr_threads: {e}")),
        }
    }

    async fn reply_thread(&self, id: &str, args: &Value) -> Response {
        let (Some(thread), Some(body)) = (
            args.get("thread").and_then(|v| v.as_str()),
            args.get("body").and_then(|v| v.as_str()),
        ) else {
            return Response::err(id, "reply_thread requires `thread` and `body`");
        };
        match crate::github::pr_reply_thread(thread, body).await {
            Ok(()) => Response::ok(id, 0, "replied".to_string(), String::new()),
            Err(e) => Response::err(id, format!("reply_thread: {e}")),
        }
    }

    /// Separate from `reply_thread` so a disagreement can be left open.
    async fn resolve_thread(&self, id: &str, args: &Value) -> Response {
        let Some(thread) = args.get("thread").and_then(|v| v.as_str()) else {
            return Response::err(id, "resolve_thread requires `thread`");
        };
        match crate::github::pr_resolve_thread(thread).await {
            Ok(()) => Response::ok(id, 0, "resolved".to_string(), String::new()),
            Err(e) => Response::err(id, format!("resolve_thread: {e}")),
        }
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

    async fn open_pr(&self, id: &str, args: &Value) -> (Response, Vec<RpcEvent>) {
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

    async fn git_push(&self, id: &str, args: &Value) -> (Response, Vec<RpcEvent>) {
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

    /// Fetches with the host-held token so the agent can merge `origin/<base>`
    /// natively without the token entering the sandbox. Hooks are disabled for the
    /// same reason as push.
    async fn git_fetch(&self, id: &str, args: &Value) -> Response {
        let t = match self.target(id, args) {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        let cwd = t.cwd;
        let branch = match arg_branch_named(args, "ref") {
            Some(r) => r,
            None => t.base_branch,
        };
        if let Some(resp) = refuse_option_like(id, "git_fetch", "ref", &branch) {
            return resp;
        }
        let auth = crate::github::git_auth_env();
        let resp = run_git_command(id, &cwd, &["fetch", "origin", &branch], &auth).await;
        // `run_git_command` reports `ok` for any git that ran; a non-zero fetch must be
        // a hard error or the agent merges a stale `origin/<branch>`.
        if !resp.ok || resp.exit_code == Some(0) {
            return resp;
        }
        let detail = resp
            .stderr
            .filter(|s| !s.trim().is_empty())
            .or(resp.stdout)
            .unwrap_or_default();
        Response::err(
            id,
            format!("git_fetch: fetch origin {branch} failed: {}", detail.trim()),
        )
    }
}

fn approval_repo<'a>(subdir: Option<&'a str>, primary: Option<&str>) -> Option<&'a str> {
    subdir.filter(|s| Some(*s) != primary)
}

fn arg_branch(args: &Value) -> Option<String> {
    arg_branch_named(args, "branch")
}

/// A leading `-` could be read by git as an option. `git::push` uses a
/// fully-qualified refspec, but the same strings reach `pr_create`, the fetch
/// argv and event payloads.
fn refuse_option_like(id: &str, op: &str, kind: &str, value: &str) -> Option<Response> {
    value
        .starts_with('-')
        .then(|| Response::err(id, format!("{op}: refusing option-like {kind} {value:?}")))
}

fn arg_bool(args: &Value, key: &str) -> bool {
    args.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn arg_branch_named(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
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

async fn run_git_command(
    id: &str,
    cwd: &Path,
    args: &[&str],
    env: &[(String, String)],
) -> Response {
    // Built directly rather than via `git::cmd`, so the config guard is applied
    // here: `git_status` runs clean filters, which would execute a planted one.
    if let Err(e) = crate::git::hardening::refuse_steerable_config(cwd).await {
        return Response::err(id, e.to_string());
    }
    let mut cmd = crate::git_dist::command(cwd);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (k, v) in env {
        cmd.env(k, v);
    }

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Response::err(id, format!("spawn git: {e}")),
    };

    match tokio::time::timeout(OP_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(out)) => Response::ok(
            id,
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ),
        Ok(Err(e)) => Response::err(id, format!("run git: {e}")),
        Err(_) => Response::err(id, format!("op timed out after {}s", OP_TIMEOUT.as_secs())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::{ensure_mailbox, process_pending};

    fn write_request(requests: &Path, name: &str, body: &str) {
        std::fs::write(requests.join(name), body).unwrap();
    }

    fn run_git(repo: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .current_dir(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn dispatcher(cwd: &Path) -> GitDispatcher {
        GitDispatcher::new(
            cwd.to_path_buf(),
            "main".to_string(),
            AgentCaps::interactive(),
        )
    }

    #[tokio::test]
    async fn git_status_runs_a_real_command() {
        let td = tempfile::tempdir().unwrap();
        let rpc_dir = td.path().join(".fletch-rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        write_request(
            &rpc_dir.join("requests"),
            "req-4.json",
            r#"{"id":"req-4","op":"git_status"}"#,
        );

        let dispatcher = dispatcher(td.path());
        process_pending(&rpc_dir, &dispatcher).await;

        let body = std::fs::read_to_string(rpc_dir.join("responses/req-4.json")).unwrap();
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["id"], "req-4");
        assert_eq!(v["ok"], true);
        assert!(v["exit_code"].is_number());
    }

    #[tokio::test]
    async fn echo_round_trips_free_text() {
        let td = tempfile::tempdir().unwrap();
        let rpc_dir = td.path().join(".fletch-rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        write_request(
            &rpc_dir.join("requests"),
            "req-echo.json",
            r#"{"id":"req-echo","op":"echo","args":{"message":"hello from the agent"}}"#,
        );

        let dispatcher = dispatcher(td.path());
        process_pending(&rpc_dir, &dispatcher).await;

        let body = std::fs::read_to_string(rpc_dir.join("responses/req-echo.json")).unwrap();
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["id"], "req-echo");
        assert_eq!(v["ok"], true);
        assert_eq!(v["stdout"], "hello from the agent");
    }

    #[tokio::test]
    async fn signal_git_action_emits_action_done_for_known_actions() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q"]);

        let rpc_dir = td.path().join("rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        write_request(
            &rpc_dir.join("requests"),
            "s1.json",
            r#"{"id":"s1","op":"signal_git_action","args":{"action":"git_commit"}}"#,
        );

        let dispatcher = dispatcher(&repo);
        let effects = process_pending(&rpc_dir, &dispatcher).await;

        let body = std::fs::read_to_string(rpc_dir.join("responses/s1.json")).unwrap();
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], true, "response: {body}");
        assert!(
            has_action_done(&effects, "git_commit"),
            "a post-commit hook signal must relay an action-done, got: {effects:?}"
        );
    }

    #[tokio::test]
    async fn signal_git_action_rejects_unknown_action() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q"]);

        let disp = dispatcher(&repo);
        let (resp, effects) = disp
            .dispatch_inner("s", "signal_git_action", &json!({"action": "rm -rf"}))
            .await;
        assert!(!resp.ok, "an unrecognized action must be rejected");
        assert!(
            effects.is_empty(),
            "a rejected signal must emit nothing, got: {effects:?}"
        );
    }

    #[tokio::test]
    async fn git_push_refuses_option_named_head_branch() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q", "-b", "main"]);
        run_git(&repo, &["config", "user.email", "t@example.com"]);
        run_git(&repo, &["config", "user.name", "Tester"]);
        std::fs::write(repo.join("a.txt"), b"x").unwrap();
        run_git(&repo, &["add", "-A"]);
        run_git(&repo, &["commit", "-q", "-m", "init"]);

        // Repoint HEAD at an option-named branch, as an agent controlling its own
        // `.git` could.
        let head = std::process::Command::new("git")
            .current_dir(&repo)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        let sha = String::from_utf8_lossy(&head.stdout).trim().to_string();
        std::fs::write(repo.join(".git/refs/heads/--mirror"), format!("{sha}\n")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/--mirror\n").unwrap();

        let disp = dispatcher(&repo);
        let (resp, fx) = disp.dispatch_inner("p", "git_push", &Value::Null).await;
        assert!(
            !resp.ok,
            "an option-named HEAD must be refused before any push: {resp:?}"
        );
        assert!(
            resp.error
                .as_deref()
                .unwrap_or_default()
                .contains("option-like"),
            "error must name the refusal, got: {:?}",
            resp.error
        );
        assert!(fx.is_empty(), "a refused push must emit nothing: {fx:?}");
    }

    #[tokio::test]
    async fn open_pr_refuses_option_like_base() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q", "-b", "main"]);

        let disp = dispatcher(&repo);
        let (resp, fx) = disp
            .dispatch_inner(
                "b1",
                "open_pr",
                &json!({"title": "t", "base": "--upload-pack=evil"}),
            )
            .await;
        assert!(!resp.ok, "an option-named base must be refused: {resp:?}");
        assert!(
            resp.error
                .as_deref()
                .unwrap_or_default()
                .contains("option-like base"),
            "error must name the refused base, got: {:?}",
            resp.error
        );
        assert!(fx.is_empty(), "a refused PR must emit nothing: {fx:?}");
    }

    #[tokio::test]
    async fn open_pr_refuses_a_base_missing_from_origin() {
        let td = tempfile::tempdir().unwrap();
        let origin = td.path().join("origin.git");
        std::fs::create_dir_all(&origin).unwrap();
        run_git(&origin, &["init", "-q", "--bare", "-b", "main"]);

        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q", "-b", "main"]);
        run_git(&repo, &["config", "user.email", "t@example.com"]);
        run_git(&repo, &["config", "user.name", "Tester"]);
        run_git(
            &repo,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        std::fs::write(repo.join("a.txt"), b"x").unwrap();
        run_git(&repo, &["add", "-A"]);
        run_git(&repo, &["commit", "-q", "-m", "init"]);
        run_git(&repo, &["push", "-q", "origin", "main"]);

        let disp = dispatcher(&repo);
        let (resp, fx) = disp
            .dispatch_inner(
                "b3",
                "open_pr",
                &json!({"title": "t", "branch": "fix/typo-base", "base": "mainn"}),
            )
            .await;
        assert!(
            !resp.ok,
            "a base origin does not have must be refused: {resp:?}"
        );
        assert!(
            resp.error
                .as_deref()
                .unwrap_or_default()
                .contains("does not exist on origin"),
            "error must name the missing base, got: {:?}",
            resp.error
        );
        assert!(
            fx.is_empty(),
            "nothing may be created before the base is known good: {fx:?}"
        );
        let branches = std::process::Command::new("git")
            .current_dir(&repo)
            .args(["branch", "--list", "fix/typo-base"])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&branches.stdout).trim().is_empty(),
            "the head branch must not be materialized for a bad base"
        );
    }

    /// `ls-remote`'s ref argument is a pattern: a glob base matches a real branch
    /// and exits 0, but GitHub rejects it as a literal base.
    #[tokio::test]
    async fn open_pr_refuses_a_glob_base_that_matches_a_real_branch() {
        let td = tempfile::tempdir().unwrap();
        let origin = td.path().join("origin.git");
        std::fs::create_dir_all(&origin).unwrap();
        run_git(&origin, &["init", "-q", "--bare", "-b", "main"]);

        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q", "-b", "main"]);
        run_git(&repo, &["config", "user.email", "t@example.com"]);
        run_git(&repo, &["config", "user.name", "Tester"]);
        run_git(
            &repo,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        std::fs::write(repo.join("a.txt"), b"x").unwrap();
        run_git(&repo, &["add", "-A"]);
        run_git(&repo, &["commit", "-q", "-m", "init"]);
        run_git(&repo, &["push", "-q", "origin", "main"]);
        run_git(&repo, &["push", "-q", "origin", "main:refs/heads/feat/x"]);

        let disp = dispatcher(&repo);
        let (resp, fx) = disp
            .dispatch_inner(
                "b5",
                "open_pr",
                &json!({"title": "t", "branch": "fix/glob-base", "base": "feat/*"}),
            )
            .await;
        assert!(
            !resp.ok,
            "a glob base must be refused even though ls-remote matches it: {resp:?}"
        );
        assert!(
            resp.error
                .as_deref()
                .unwrap_or_default()
                .contains("does not exist on origin"),
            "error must name the missing base, got: {:?}",
            resp.error
        );
        assert!(fx.is_empty(), "nothing may be created: {fx:?}");
    }

    #[tokio::test]
    async fn open_pr_does_not_refuse_a_base_it_could_not_verify() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q", "-b", "main"]);
        run_git(&repo, &["config", "user.email", "t@example.com"]);
        run_git(&repo, &["config", "user.name", "Tester"]);
        std::fs::write(repo.join("a.txt"), b"x").unwrap();
        run_git(&repo, &["add", "-A"]);
        run_git(&repo, &["commit", "-q", "-m", "init"]);
        run_git(&repo, &["checkout", "-q", "-b", "fix/unverifiable"]);

        let disp = dispatcher(&repo);
        let (resp, _fx) = disp
            .dispatch_inner("b4", "open_pr", &json!({"title": "t", "base": "feat/x"}))
            .await;
        assert!(!resp.ok);
        let err = resp.error.as_deref().unwrap_or_default();
        assert!(
            !err.contains("does not exist on origin"),
            "an unanswerable probe must not be reported as a missing base: {err}"
        );
        assert!(err.contains("push failed"), "got: {err}");
    }

    #[tokio::test]
    async fn open_pr_base_override_does_not_unlock_the_recorded_base() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q", "-b", "release/2.0"]);

        let disp = GitDispatcher::new(
            repo.clone(),
            "release/2.0".to_string(),
            AgentCaps::interactive(),
        );
        let (resp, fx) = disp
            .dispatch_inner("b2", "open_pr", &json!({"title": "t", "base": "feat/x"}))
            .await;
        assert!(
            !resp.ok,
            "publishing the review base must stay refused whatever base the PR names: {resp:?}"
        );
        assert!(
            resp.error
                .as_deref()
                .unwrap_or_default()
                .contains("reviewed against"),
            "error must be the review-base refusal, got: {:?}",
            resp.error
        );
        assert!(fx.is_empty(), "a refused PR must emit nothing: {fx:?}");
    }

    #[tokio::test]
    async fn git_fetch_without_remote_reports_error() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q", "-b", "main"]);

        let disp = dispatcher(&repo);
        let (resp, effects) = disp
            .dispatch_inner("f1", "git_fetch", &json!({"ref": "main"}))
            .await;
        assert!(
            !resp.ok,
            "a failed fetch must be an error response, got: {resp:?}"
        );
        assert!(
            resp.error.as_deref().unwrap_or_default().contains("failed"),
            "error should explain the fetch failed, got: {:?}",
            resp.error
        );
        assert!(!has_action_done(&effects, "git_fetch"));
    }

    #[tokio::test]
    async fn git_fetch_refuses_option_like_ref() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q", "-b", "main"]);

        let disp = dispatcher(&repo);
        let (resp, _fx) = disp
            .dispatch_inner("f2", "git_fetch", &json!({"ref": "--upload-pack=evil"}))
            .await;
        assert!(!resp.ok);
        assert!(resp
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("option-like"));
    }

    #[tokio::test]
    async fn git_push_without_remote_reports_error() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q", "-b", "main"]);
        run_git(&repo, &["config", "user.email", "t@example.com"]);
        run_git(&repo, &["config", "user.name", "Tester"]);
        std::fs::write(repo.join("a.txt"), b"x").unwrap();
        run_git(&repo, &["add", "-A"]);
        run_git(&repo, &["commit", "-q", "-m", "init"]);
        // Own branch: pushing the review base is refused outright, which would mask
        // the missing-remote error under test.
        run_git(&repo, &["checkout", "-q", "-b", "fix/no-remote"]);

        let rpc_dir = td.path().join("rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        write_request(
            &rpc_dir.join("requests"),
            "p1.json",
            r#"{"id":"p1","op":"git_push"}"#,
        );

        let dispatcher = dispatcher(&repo);
        process_pending(&rpc_dir, &dispatcher).await;

        let body = std::fs::read_to_string(rpc_dir.join("responses/p1.json")).unwrap();
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], false);
        let err = v["error"].as_str().unwrap();
        assert!(!err.contains("unknown op"), "got: {err}");
        assert!(err.contains("push failed"), "got: {err}");
    }

    #[test]
    fn the_primary_checkout_is_reported_as_no_repo() {
        assert_eq!(approval_repo(Some("app"), Some("app")), None);
        assert_eq!(approval_repo(Some("web"), Some("app")), Some("web"));
        assert_eq!(approval_repo(None, None), None);
    }

    #[tokio::test]
    async fn git_push_refuses_the_review_base() {
        let td = tempfile::tempdir().unwrap();
        let remote = td.path().join("remote.git");
        run_git(
            td.path(),
            &["init", "-q", "--bare", remote.to_str().unwrap()],
        );
        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q", "-b", "main"]);
        run_git(&repo, &["config", "user.email", "t@example.com"]);
        run_git(&repo, &["config", "user.name", "Tester"]);
        run_git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        std::fs::write(repo.join("a.txt"), b"x").unwrap();
        run_git(&repo, &["add", "-A"]);
        run_git(&repo, &["commit", "-q", "-m", "init"]);

        let rpc_dir = td.path().join("rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        write_request(
            &rpc_dir.join("requests"),
            "p1.json",
            r#"{"id":"p1","op":"git_push","args":{"force":true}}"#,
        );
        process_pending(&rpc_dir, &dispatcher(&repo)).await;

        let v: Value = serde_json::from_str(
            &std::fs::read_to_string(rpc_dir.join("responses/p1.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            v["ok"], false,
            "pushing the review base must be refused: {v}"
        );
        assert!(v["error"].as_str().unwrap().contains("refusing to publish"));
        let refs = std::process::Command::new("git")
            .args(["--git-dir", remote.to_str().unwrap(), "for-each-ref"])
            .output()
            .expect("git");
        assert!(
            String::from_utf8_lossy(&refs.stdout).trim().is_empty(),
            "the remote must have no refs"
        );
    }

    #[tokio::test]
    async fn git_push_detached_without_branch_arg_errors() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q", "-b", "main"]);
        run_git(&repo, &["config", "user.email", "t@example.com"]);
        run_git(&repo, &["config", "user.name", "Tester"]);
        std::fs::write(repo.join("a.txt"), b"x").unwrap();
        run_git(&repo, &["add", "-A"]);
        run_git(&repo, &["commit", "-q", "-m", "init"]);
        run_git(&repo, &["checkout", "-q", "--detach"]);

        let rpc_dir = td.path().join("rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        write_request(
            &rpc_dir.join("requests"),
            "p1.json",
            r#"{"id":"p1","op":"git_push"}"#,
        );

        let dispatcher = dispatcher(&repo);
        process_pending(&rpc_dir, &dispatcher).await;

        let body = std::fs::read_to_string(rpc_dir.join("responses/p1.json")).unwrap();
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], false);
        assert!(
            v["error"].as_str().unwrap().contains("args.branch"),
            "got: {body}"
        );
    }

    #[tokio::test]
    async fn git_push_detached_materializes_named_branch() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q", "-b", "main"]);
        run_git(&repo, &["config", "user.email", "t@example.com"]);
        run_git(&repo, &["config", "user.name", "Tester"]);
        std::fs::write(repo.join("a.txt"), b"x").unwrap();
        run_git(&repo, &["add", "-A"]);
        run_git(&repo, &["commit", "-q", "-m", "init"]);
        run_git(&repo, &["checkout", "-q", "--detach"]);

        let rpc_dir = td.path().join("rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        write_request(
            &rpc_dir.join("requests"),
            "p1.json",
            r#"{"id":"p1","op":"git_push","args":{"branch":"fix/thing"}}"#,
        );

        let dispatcher = dispatcher(&repo);
        let effects = process_pending(&rpc_dir, &dispatcher).await;

        let head = std::process::Command::new("git")
            .current_dir(&repo)
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "fix/thing");
        assert!(
            effects.iter().any(|e| matches!(
                e,
                RpcEvent::Named { name, payload }
                    if name == EVENT_BRANCH_CREATED && payload["branch"] == "fix/thing"
            )),
            "expected a branch-created event, got: {effects:?}"
        );
    }

    #[tokio::test]
    async fn git_push_force_rewrites_diverged_remote() {
        let td = tempfile::tempdir().unwrap();
        let remote = td.path().join("origin.git");
        run_git(
            td.path(),
            &["init", "-q", "--bare", remote.to_str().unwrap()],
        );

        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q", "-b", "main"]);
        run_git(&repo, &["config", "user.email", "t@example.com"]);
        run_git(&repo, &["config", "user.name", "Tester"]);
        run_git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        std::fs::write(repo.join("a.txt"), b"x").unwrap();
        run_git(&repo, &["add", "-A"]);
        run_git(&repo, &["commit", "-q", "-m", "init"]);
        // Own branch: force-push is only reachable off the review base.
        run_git(&repo, &["checkout", "-q", "-b", "fix/diverged"]);

        let rpc_dir = td.path().join("rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        let requests = rpc_dir.join("requests");
        let dispatcher = dispatcher(&repo).with_own_branch(Some("fix/diverged".to_string()));

        write_request(&requests, "p1.json", r#"{"id":"p1","op":"git_push"}"#);
        process_pending(&rpc_dir, &dispatcher).await;
        let v: Value = serde_json::from_str(
            &std::fs::read_to_string(rpc_dir.join("responses/p1.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(v["ok"], true, "seed push should succeed: {v}");

        run_git(&repo, &["commit", "-q", "--amend", "-m", "rewritten"]);

        write_request(&requests, "p2.json", r#"{"id":"p2","op":"git_push"}"#);
        process_pending(&rpc_dir, &dispatcher).await;
        let v: Value = serde_json::from_str(
            &std::fs::read_to_string(rpc_dir.join("responses/p2.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(v["ok"], false, "diverged push must be rejected: {v}");

        write_request(
            &requests,
            "p3.json",
            r#"{"id":"p3","op":"git_push","args":{"force":true}}"#,
        );
        process_pending(&rpc_dir, &dispatcher).await;
        let v: Value = serde_json::from_str(
            &std::fs::read_to_string(rpc_dir.join("responses/p3.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(v["ok"], true, "force push should succeed: {v}");

        let local = std::process::Command::new("git")
            .current_dir(&repo)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        let remote_head = std::process::Command::new("git")
            .current_dir(&remote)
            .args(["rev-parse", "refs/heads/fix/diverged"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&local.stdout).trim(),
            String::from_utf8_lossy(&remote_head.stdout).trim(),
            "the remote branch should match the rewritten local HEAD"
        );
    }

    #[tokio::test]
    async fn git_push_force_refuses_a_branch_the_agent_does_not_own() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q", "-b", "main"]);
        run_git(&repo, &["config", "user.email", "t@example.com"]);
        run_git(&repo, &["config", "user.name", "Tester"]);
        std::fs::write(repo.join("a.txt"), b"x").unwrap();
        run_git(&repo, &["add", "-A"]);
        run_git(&repo, &["commit", "-q", "-m", "init"]);
        run_git(&repo, &["checkout", "-q", "-b", "develop"]);

        let disp = dispatcher(&repo).with_own_branch(Some("fix/mine".to_string()));
        let (resp, fx) = disp
            .dispatch_inner("p", "git_push", &json!({ "force": true }))
            .await;
        assert!(
            !resp.ok,
            "force-pushing a non-own branch must be refused before any push: {resp:?}"
        );
        assert!(
            resp.error.as_deref().unwrap_or_default().contains("force"),
            "the refusal must name the force constraint, got: {:?}",
            resp.error
        );
        assert!(
            fx.is_empty(),
            "a refused force push must emit nothing: {fx:?}"
        );

        let (resp, _fx) = disp.dispatch_inner("p2", "git_push", &Value::Null).await;
        let err = resp.error.as_deref().unwrap_or_default();
        assert!(
            err.contains("push failed") && !err.contains("force is limited"),
            "a non-force push must reach the transport, not the force gate: {err:?}"
        );
    }

    #[tokio::test]
    async fn git_push_force_is_refused_until_the_own_branch_is_recorded() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q", "-b", "main"]);
        run_git(&repo, &["config", "user.email", "t@example.com"]);
        run_git(&repo, &["config", "user.name", "Tester"]);
        std::fs::write(repo.join("a.txt"), b"x").unwrap();
        run_git(&repo, &["add", "-A"]);
        run_git(&repo, &["commit", "-q", "-m", "init"]);
        run_git(&repo, &["checkout", "-q", "-b", "fix/fresh"]);

        let disp = dispatcher(&repo);
        let (resp, fx) = disp
            .dispatch_inner("p", "git_push", &json!({ "force": true }))
            .await;
        assert!(
            !resp.ok,
            "force must fail closed while no own branch is recorded: {resp:?}"
        );
        assert!(
            resp.error.as_deref().unwrap_or_default().contains("force"),
            "the refusal must name the force constraint, got: {:?}",
            resp.error
        );
        assert!(
            fx.is_empty(),
            "a refused force push must emit nothing: {fx:?}"
        );
    }

    #[tokio::test]
    async fn git_push_force_refuses_when_remote_advanced_unseen() {
        let td = tempfile::tempdir().unwrap();
        let remote = td.path().join("origin.git");
        run_git(
            td.path(),
            &["init", "-q", "--bare", remote.to_str().unwrap()],
        );

        let a = td.path().join("a");
        std::fs::create_dir_all(&a).unwrap();
        run_git(&a, &["init", "-q", "-b", "main"]);
        run_git(&a, &["config", "user.email", "a@example.com"]);
        run_git(&a, &["config", "user.name", "A"]);
        run_git(&a, &["remote", "add", "origin", remote.to_str().unwrap()]);
        std::fs::write(a.join("f.txt"), b"1").unwrap();
        run_git(&a, &["add", "-A"]);
        run_git(&a, &["commit", "-q", "-m", "c1"]);
        run_git(&a, &["push", "-u", "-q", "origin", "main"]);

        // Explicit `-b main`: the bare repo's default HEAD follows the host's
        // `init.defaultBranch`.
        let b = td.path().join("b");
        run_git(
            td.path(),
            &[
                "clone",
                "-q",
                "-b",
                "main",
                remote.to_str().unwrap(),
                b.to_str().unwrap(),
            ],
        );
        run_git(&b, &["config", "user.email", "b@example.com"]);
        run_git(&b, &["config", "user.name", "B"]);
        std::fs::write(b.join("g.txt"), b"2").unwrap();
        run_git(&b, &["add", "-A"]);
        run_git(&b, &["commit", "-q", "-m", "c2"]);
        run_git(&b, &["push", "-q", "origin", "main"]);

        run_git(&a, &["commit", "-q", "--amend", "-m", "c1-rewritten"]);

        let rpc_dir = td.path().join("rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        write_request(
            &rpc_dir.join("requests"),
            "p.json",
            r#"{"id":"p","op":"git_push","args":{"force":true}}"#,
        );
        let dispatcher = dispatcher(&a);
        process_pending(&rpc_dir, &dispatcher).await;
        let v: Value = serde_json::from_str(
            &std::fs::read_to_string(rpc_dir.join("responses/p.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            v["ok"], false,
            "force push must be refused when the remote advanced with a commit we never integrated: {v}"
        );

        let remote_head = std::process::Command::new("git")
            .current_dir(&remote)
            .args(["rev-parse", "refs/heads/main"])
            .output()
            .unwrap();
        let b_head = std::process::Command::new("git")
            .current_dir(&b)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&remote_head.stdout).trim(),
            String::from_utf8_lossy(&b_head.stdout).trim(),
            "remote main must be untouched after the refused force push"
        );
    }

    #[tokio::test]
    async fn repo_arg_targets_sibling_checkout() {
        let td = tempfile::tempdir().unwrap();
        let a = td.path().join("a");
        let b = td.path().join("b");
        for repo in [&a, &b] {
            std::fs::create_dir_all(repo).unwrap();
            run_git(repo, &["init", "-q", "-b", "main"]);
        }
        std::fs::write(b.join("x.txt"), b"x").unwrap();

        let disp = GitDispatcher::new(a.clone(), "main".into(), AgentCaps::interactive())
            .with_repos(vec![
                ("a".into(), a.clone(), "main".into(), None),
                ("b".into(), b.clone(), "main".into(), None),
            ]);

        let (resp, _fx) = disp
            .dispatch_inner("s1", "git_status", &json!({"repo": "b"}))
            .await;
        assert_eq!(resp.exit_code, Some(0), "status in b: {resp:?}");
        assert!(
            resp.stdout.as_deref().unwrap_or_default().contains("x.txt"),
            "targeting `b` must see its dirty file: {resp:?}"
        );

        let (resp, _fx) = disp.dispatch_inner("s2", "git_status", &Value::Null).await;
        assert!(
            !resp.stdout.as_deref().unwrap_or_default().contains("x.txt"),
            "the default target must remain the primary checkout: {resp:?}"
        );
    }

    #[tokio::test]
    async fn branch_events_carry_the_targeted_repo() {
        let td = tempfile::tempdir().unwrap();
        let a = td.path().join("a");
        let b = td.path().join("b");
        for repo in [&a, &b] {
            std::fs::create_dir_all(repo).unwrap();
            run_git(repo, &["init", "-q", "-b", "main"]);
            run_git(repo, &["config", "user.email", "t@example.com"]);
            run_git(repo, &["config", "user.name", "Tester"]);
            std::fs::write(repo.join("f.txt"), b"x").unwrap();
            run_git(repo, &["add", "-A"]);
            run_git(repo, &["commit", "-q", "-m", "init"]);
            run_git(repo, &["checkout", "-q", "--detach"]);
        }

        let disp = GitDispatcher::new(a.clone(), "main".into(), AgentCaps::interactive())
            .with_repos(vec![
                ("a".into(), a.clone(), "main".into(), None),
                ("b".into(), b.clone(), "main".into(), None),
            ]);

        let (_resp, fx) = disp
            .dispatch_inner(
                "p1",
                "git_push",
                &json!({"repo": "b", "branch": "feat/backend"}),
            )
            .await;
        assert!(
            fx.iter().any(|e| matches!(
                e,
                RpcEvent::Named { name, payload }
                    if name == EVENT_BRANCH_CREATED
                        && payload["branch"] == "feat/backend"
                        && payload["repo"] == "b"
            )),
            "branch event must carry the targeted repo, got: {fx:?}"
        );

        let (_resp, fx) = disp
            .dispatch_inner("p2", "git_push", &json!({"branch": "feat/front"}))
            .await;
        assert!(
            fx.iter().any(|e| matches!(
                e,
                RpcEvent::Named { name, payload }
                    if name == EVENT_BRANCH_CREATED && payload["repo"] == "a"
            )),
            "defaulted op must attribute to the primary subdir, got: {fx:?}"
        );
    }

    #[tokio::test]
    async fn unknown_repo_arg_is_rejected_with_tracked_names() {
        let td = tempfile::tempdir().unwrap();
        let a = td.path().join("a");
        std::fs::create_dir_all(&a).unwrap();
        run_git(&a, &["init", "-q", "-b", "main"]);

        let disp = GitDispatcher::new(a.clone(), "main".into(), AgentCaps::interactive())
            .with_repos(vec![("a".into(), a.clone(), "main".into(), None)]);
        let (resp, fx) = disp
            .dispatch_inner("p", "git_push", &json!({"repo": "nope"}))
            .await;
        assert!(!resp.ok, "an unknown repo must be rejected: {resp:?}");
        let err = resp.error.as_deref().unwrap_or_default();
        assert!(err.contains("unknown repo"), "got: {err}");
        assert!(err.contains('a'), "should list tracked checkouts: {err}");
        assert!(fx.is_empty(), "a rejected op must emit nothing: {fx:?}");
    }

    #[test]
    fn fallback_branch_slugifies_title() {
        assert_eq!(
            fallback_branch("Fix the Login Crash!"),
            "chore/fix-the-login-crash"
        );
        assert_eq!(
            fallback_branch("  Add   CSV export  "),
            "chore/add-csv-export"
        );
        assert_eq!(fallback_branch(""), "chore/update");
        assert_eq!(fallback_branch("!!!"), "chore/update");
    }

    fn has_action_done(effects: &[RpcEvent], expect_op: &str) -> bool {
        effects.iter().any(|e| {
            let RpcEvent::Named { name, payload } = e;
            name == EVENT_ACTION_DONE && payload["op"] == expect_op
        })
    }

    #[tokio::test]
    async fn read_only_and_failed_ops_emit_no_action_done() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q", "-b", "main"]);
        run_git(&repo, &["config", "user.email", "t@example.com"]);
        run_git(&repo, &["config", "user.name", "Tester"]);
        std::fs::write(repo.join("a.txt"), b"x").unwrap();
        run_git(&repo, &["add", "-A"]);
        run_git(&repo, &["commit", "-q", "-m", "init"]);
        let disp = dispatcher(&repo);

        let (_r, status_fx) = disp.dispatch_inner("s", "git_status", &Value::Null).await;
        assert!(
            !has_action_done(&status_fx, "git_status"),
            "git_status is read-only and must not signal an action"
        );

        let (resp, push_fx) = disp.dispatch_inner("p", "git_push", &Value::Null).await;
        assert!(!resp.ok);
        assert!(
            !has_action_done(&push_fx, "git_push"),
            "a failed git_push must not signal an action"
        );
    }
}
