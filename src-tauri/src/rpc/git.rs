//! Git-specific RPC dispatcher layered on top of the generic mailbox transport.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};

use super::{Response, RpcDispatcher, RpcEvent, RpcFuture};

/// App-side ceiling on a single op. A hung command surfaces as an error
/// response rather than blocking the watcher forever. The agent runs its own
/// (shorter) poll timeout independently — see the instruction block.
const OP_TIMEOUT: Duration = Duration::from_secs(120);

pub const EVENT_BRANCH_CREATED: &str = "git.branch_created";
pub const EVENT_PR_OPENED: &str = "git.pr_opened";
/// Emitted whenever a *mutating* git op completes successfully. This is the
/// authoritative "the agent actually performed a git action this turn" signal:
/// the UI's delegation tracking uses it to attribute a git/PR state change to
/// the agent's turn instead of inferring causality from a polled snapshot
/// (which can't tell agent work from a manual action or a pre-existing match).
pub const EVENT_ACTION_DONE: &str = "git.action_done";

/// Host-brokered ops that change remote state — the ones that must run on the
/// host because they need GitHub credentials that never enter the sandbox, and
/// whose success means the agent did the delegated work. Local mutations
/// (commit, merge, conflict-resolve) now run as native in-container git and are
/// *not* here — their delegation signal arrives out-of-band via the clone's
/// `post-commit`/`post-merge` hooks (see `signal_git_action`). Read-only ops
/// (status, fetch) and the test ops (echo/ping) are excluded so they never read
/// as a completed delegation.
fn is_mutating_op(op: &str) -> bool {
    matches!(op, "git_push" | "open_pr")
}

/// The local-git action names a delegation hook may report. Kept to a closed
/// set so a compromised hook can't fabricate an arbitrary op string into the
/// UI's delegation attribution. These mirror the op names the frontend's
/// `gitActionProvesKind` still recognizes for backward compatibility.
fn is_signalable_action(action: &str) -> bool {
    matches!(action, "git_commit" | "git_update_branch")
}

#[derive(Clone)]
pub struct GitDispatcher {
    /// Default checkout (the agent's primary repo) — used when an op carries
    /// no `args.repo`, which keeps single-repo agents byte-identical.
    cwd: PathBuf,
    base_branch: String,
    /// Sibling checkouts by subdir (directory name under the workspace root),
    /// each with its own base branch. Includes the primary. Empty for
    /// dispatchers built without `with_repos` (tests, old call sites) — then
    /// `args.repo` is rejected as unknown.
    repos: std::collections::HashMap<String, (PathBuf, String)>,
}

impl GitDispatcher {
    pub fn new(cwd: PathBuf, base_branch: String) -> Self {
        Self {
            cwd,
            base_branch,
            repos: std::collections::HashMap::new(),
        }
    }

    /// Register the agent's tracked checkouts as `(subdir, cwd, base_branch)`
    /// so ops can be pointed at any of them via `args.repo`.
    pub fn with_repos(mut self, repos: Vec<(String, PathBuf, String)>) -> Self {
        self.repos = repos
            .into_iter()
            .map(|(subdir, cwd, base)| (subdir, (cwd, base)))
            .collect();
        self
    }

    /// Resolve the checkout an op targets: `args.repo` (a tracked subdir) when
    /// present, the primary checkout otherwise. An unknown repo is an error
    /// response listing the tracked names, so a typo can't silently operate on
    /// the wrong repo.
    fn target(&self, id: &str, args: &Value) -> std::result::Result<(PathBuf, String), Response> {
        let requested = args
            .get("repo")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        match requested {
            None => Ok((self.cwd.clone(), self.base_branch.clone())),
            Some(name) => match self.repos.get(name) {
                Some((cwd, base)) => Ok((cwd.clone(), base.clone())),
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
        let (resp, mut effects) = match op {
            "open_pr" => self.open_pr(id, args).await,
            "git_push" => self.git_push(id, args).await,
            // Credentialed fetch of the base branch for the native in-container
            // merge in `update-branch`: the token stays host-side, so the agent
            // can't fetch a private remote itself.
            "git_fetch" => (self.git_fetch(id, args).await, Vec::new()),
            // Out-of-band delegation signal from the clone's local git hooks
            // (post-commit / post-merge). Not a mutation the host performs —
            // it only relays that the agent's native git action ran.
            "signal_git_action" => self.signal_git_action(id, args),
            "echo" => (self.echo(id, args).await, Vec::new()),
            "ping" => (
                Response::ok(id, 0, "pong".to_string(), String::new()),
                Vec::new(),
            ),
            "git_status" => match self.target(id, args) {
                Ok((cwd, _)) => (
                    run_git_command(id, &cwd, &["status", "--porcelain=v1", "--branch"], &[]).await,
                    Vec::new(),
                ),
                Err(resp) => (resp, Vec::new()),
            },
            other => (
                Response::err(id, format!("unknown op: {other}")),
                Vec::new(),
            ),
        };
        // A successful mutating op is ground truth that the agent performed the
        // git action this turn — surface it so the UI doesn't have to guess from
        // a snapshot. Emitted alongside the op-specific branch/PR events.
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

    /// Relay a native-git delegation signal from a clone-installed hook. The
    /// hook can't emit an app event itself, so it pings this op; we translate it
    /// into the same `EVENT_ACTION_DONE` a host-side mutation would emit, so the
    /// UI attributes the agent's in-container commit/merge to its turn exactly as
    /// before. Synchronous: no git runs, we only validate and emit.
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
        let (cwd, base_branch) = match self.target(id, args) {
            Ok(t) => t,
            Err(resp) => return (resp, Vec::new()),
        };
        let title = args.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let body = args.get("body").and_then(|v| v.as_str()).unwrap_or("");
        let requested = arg_branch(args);

        let current = match crate::git::current_branch(&cwd).await {
            Ok(b) => b,
            Err(e) => return (Response::err(id, format!("open_pr: {e}")), Vec::new()),
        };

        let mut effects = Vec::new();
        let branch = match (current, requested) {
            (Some(cur), None) => cur,
            (Some(cur), Some(req)) if req == cur => cur,
            (Some(_), Some(req)) => match materialize_branch(&cwd, &req).await {
                Ok(name) => {
                    effects.push(RpcEvent::named(
                        EVENT_BRANCH_CREATED,
                        json!({ "branch": name }),
                    ));
                    name
                }
                Err(e) => return (Response::err(id, format!("open_pr: {e}")), effects),
            },
            (None, req) => {
                let desired = req.unwrap_or_else(|| fallback_branch(title));
                match materialize_branch(&cwd, &desired).await {
                    Ok(name) => {
                        effects.push(RpcEvent::named(
                            EVENT_BRANCH_CREATED,
                            json!({ "branch": name }),
                        ));
                        name
                    }
                    Err(e) => return (Response::err(id, format!("open_pr: {e}")), effects),
                }
            }
        };

        if let Err(e) = crate::git::push(&cwd, &branch, false).await {
            return (
                Response::err(id, format!("open_pr push failed: {e}")),
                effects,
            );
        }
        match crate::github::pr_create(&cwd, title, body, &base_branch).await {
            Ok(pr) => {
                crate::telemetry::track("pr_opened", json!({ "source": "agent_rpc" }));
                effects.push(RpcEvent::named(
                    EVENT_PR_OPENED,
                    json!({ "number": pr.number }),
                ));
                (Response::ok(id, 0, pr.url, String::new()), effects)
            }
            Err(e) => (Response::err(id, format!("open_pr: {e}")), effects),
        }
    }

    async fn git_push(&self, id: &str, args: &Value) -> (Response, Vec<RpcEvent>) {
        let (cwd, _) = match self.target(id, args) {
            Ok(t) => t,
            Err(resp) => return (resp, Vec::new()),
        };
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
                            json!({ "branch": name }),
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

        // `args.force` opts into `--force-with-lease` for pushing a rewritten
        // history (e.g. after the agent rebased its branch). Lease-based so a
        // stale local view can't clobber remote work it hasn't seen.
        let force = arg_bool(args, "force");
        match crate::git::push(&cwd, &branch, force).await {
            Ok(summary) => (Response::ok(id, 0, summary, String::new()), effects),
            Err(e) => (Response::err(id, e.to_string()), effects),
        }
    }

    /// Fetch a base branch from `origin` with the host-held GitHub token, so the
    /// agent can then run a native in-container `git merge origin/<base>` without
    /// the token ever entering the sandbox. Read-only on the local repo (updates
    /// only the `origin/<base>` remote-tracking ref); the merge, conflict
    /// resolution, and merge commit are the agent's native git. `args.ref`
    /// selects the branch (the `update-branch` playbook passes its `base`);
    /// absent, the spawn parent branch is used. Hooks are disabled on this
    /// host-side invocation for the same reason as push (agent-writable `.git`).
    async fn git_fetch(&self, id: &str, args: &Value) -> Response {
        let (cwd, base_branch) = match self.target(id, args) {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        let branch = match arg_branch_named(args, "ref") {
            Some(r) => r,
            None => base_branch,
        };
        if branch.starts_with('-') {
            return Response::err(
                id,
                format!("git_fetch: refusing option-like ref {branch:?}"),
            );
        }
        let auth = crate::git::merge_git_env(&[
            &crate::github::git_auth_env(),
            &crate::git::no_hooks_env(),
        ]);
        let resp = run_git_command(id, &cwd, &["fetch", "origin", &branch], &auth).await;
        // `run_git_command` reports `ok: true` for any git that *ran*, carrying a
        // non-zero result only in `exit_code`. For fetch that's a trap: a missing
        // ref or a transient remote failure would leave `origin/<branch>` stale
        // while the agent merges it anyway. Convert a non-zero fetch into a hard
        // error so the `update-branch` flow stops here instead of merging old
        // state. A response that's already an error (spawn/timeout) passes through
        // unchanged, keeping its original message.
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

fn arg_branch(args: &Value) -> Option<String> {
    arg_branch_named(args, "branch")
}

/// Read a boolean flag arg by key, defaulting to `false` when absent or not a
/// bool. Used for `git_push`'s `force` opt-in.
fn arg_bool(args: &Value, key: &str) -> bool {
    args.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

/// Read a trimmed, non-empty string arg by key. Used for `branch` (push/PR) and
/// `ref` (fetch).
fn arg_branch_named(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

async fn materialize_branch(checkout: &Path, desired: &str) -> std::result::Result<String, String> {
    crate::git::checkout_new_unique_branch(checkout, desired)
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
        GitDispatcher::new(cwd.to_path_buf(), "main".to_string())
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
    async fn git_fetch_without_remote_reports_error() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q", "-b", "main"]);

        let disp = dispatcher(&repo);
        let (resp, effects) = disp
            .dispatch_inner("f1", "git_fetch", &json!({"ref": "main"}))
            .await;
        // No origin remote → git exits non-zero. This must surface as a hard
        // error, not an ok response, so the agent stops instead of merging a
        // stale `origin/<base>`. And it's never a completed mutation.
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

        let rpc_dir = td.path().join("rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        let requests = rpc_dir.join("requests");
        let dispatcher = dispatcher(&repo);

        // First push seeds the remote branch.
        write_request(&requests, "p1.json", r#"{"id":"p1","op":"git_push"}"#);
        process_pending(&rpc_dir, &dispatcher).await;
        let v: Value = serde_json::from_str(
            &std::fs::read_to_string(rpc_dir.join("responses/p1.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(v["ok"], true, "seed push should succeed: {v}");

        // Rewrite local history so the branch diverges from the remote.
        run_git(&repo, &["commit", "-q", "--amend", "-m", "rewritten"]);

        // A normal push is rejected as non-fast-forward.
        write_request(&requests, "p2.json", r#"{"id":"p2","op":"git_push"}"#);
        process_pending(&rpc_dir, &dispatcher).await;
        let v: Value = serde_json::from_str(
            &std::fs::read_to_string(rpc_dir.join("responses/p2.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(v["ok"], false, "diverged push must be rejected: {v}");

        // Force (lease-guarded) push rewrites the remote branch.
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

        // Remote now points at the rewritten commit.
        let local = std::process::Command::new("git")
            .current_dir(&repo)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        let remote_head = std::process::Command::new("git")
            .current_dir(&remote)
            .args(["rev-parse", "refs/heads/main"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&local.stdout).trim(),
            String::from_utf8_lossy(&remote_head.stdout).trim(),
            "remote main should match the rewritten local HEAD"
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

        // Repo A seeds the remote and records origin/main = c1.
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

        // Repo B advances the remote with a commit A never fetches. Clone with
        // an explicit `-b main` so the checkout doesn't depend on the bare
        // repo's default HEAD (which follows the host's `init.defaultBranch`).
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

        // A rewrites its own history WITHOUT integrating B's commit, then tries
        // to force-push. A's origin/main tracking ref is stale (still c1).
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
        // `--force-if-includes` must catch this even though the stale tracking
        // ref would have passed a bare `--force-with-lease`.
        assert_eq!(
            v["ok"], false,
            "force push must be refused when the remote advanced with a commit we never integrated: {v}"
        );

        // The remote still points at B's commit — nothing was clobbered.
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
        // A dirty file only in `b`, so the two checkouts are distinguishable.
        std::fs::write(b.join("x.txt"), b"x").unwrap();

        let disp = GitDispatcher::new(a.clone(), "main".into()).with_repos(vec![
            ("a".into(), a.clone(), "main".into()),
            ("b".into(), b.clone(), "main".into()),
        ]);

        let (resp, _fx) = disp
            .dispatch_inner("s1", "git_status", &json!({"repo": "b"}))
            .await;
        assert_eq!(resp.exit_code, Some(0), "status in b: {resp:?}");
        assert!(
            resp.stdout.as_deref().unwrap_or_default().contains("x.txt"),
            "targeting `b` must see its dirty file: {resp:?}"
        );

        // No `repo` arg → the primary checkout, exactly as before.
        let (resp, _fx) = disp.dispatch_inner("s2", "git_status", &Value::Null).await;
        assert!(
            !resp.stdout.as_deref().unwrap_or_default().contains("x.txt"),
            "the default target must remain the primary checkout: {resp:?}"
        );
    }

    #[tokio::test]
    async fn unknown_repo_arg_is_rejected_with_tracked_names() {
        let td = tempfile::tempdir().unwrap();
        let a = td.path().join("a");
        std::fs::create_dir_all(&a).unwrap();
        run_git(&a, &["init", "-q", "-b", "main"]);

        let disp = GitDispatcher::new(a.clone(), "main".into()).with_repos(vec![(
            "a".into(),
            a.clone(),
            "main".into(),
        )]);
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

        // Read-only op: never an action-done signal.
        let (_r, status_fx) = disp.dispatch_inner("s", "git_status", &Value::Null).await;
        assert!(
            !has_action_done(&status_fx, "git_status"),
            "git_status is read-only and must not signal an action"
        );

        // A failed mutating op (push with no remote) must NOT signal success:
        // `is_mutating_op` gates the emit on `resp.ok`.
        let (resp, push_fx) = disp.dispatch_inner("p", "git_push", &Value::Null).await;
        assert!(!resp.ok);
        assert!(
            !has_action_done(&push_fx, "git_push"),
            "a failed git_push must not signal an action"
        );
    }
}
