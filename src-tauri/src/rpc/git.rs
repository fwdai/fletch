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

/// Ops that change repo/PR state — the ones whose success means the agent did
/// the delegated work. Read-only ops (status) and the test ops (echo/ping) are
/// excluded so they never read as a completed delegation.
fn is_mutating_op(op: &str) -> bool {
    matches!(op, "git_commit" | "git_push" | "open_pr" | "git_update_branch")
}

#[derive(Clone)]
pub struct GitDispatcher {
    cwd: PathBuf,
    base_branch: String,
}

impl GitDispatcher {
    pub fn new(cwd: PathBuf, base_branch: String) -> Self {
        Self { cwd, base_branch }
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
            "echo" => (self.echo(id, args).await, Vec::new()),
            "ping" => (Response::ok(id, 0, "pong".to_string(), String::new()), Vec::new()),
            "git_status" => (
                run_git_command(
                    id,
                    &self.cwd,
                    &["status", "--porcelain=v1", "--branch"],
                    &[],
                )
                .await,
                Vec::new(),
            ),
            "git_commit" => (self.git_commit(id, args).await, Vec::new()),
            "git_update_branch" => (self.git_update_branch(id).await, Vec::new()),
            other => (Response::err(id, format!("unknown op: {other}")), Vec::new()),
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

    async fn git_commit(&self, id: &str, args: &Value) -> Response {
        let message = args.get("message").and_then(|v| v.as_str()).unwrap_or("");
        if message.trim().is_empty() {
            return Response::err(id, "git_commit requires a non-empty `message` arg");
        }
        match crate::git::commit_all(&self.cwd, message).await {
            Ok(()) => Response::ok(id, 0, "committed".to_string(), String::new()),
            Err(e) => Response::err(id, e.to_string()),
        }
    }

    async fn open_pr(&self, id: &str, args: &Value) -> (Response, Vec<RpcEvent>) {
        let title = args.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let body = args.get("body").and_then(|v| v.as_str()).unwrap_or("");
        let requested = arg_branch(args);

        let current = match crate::git::current_branch(&self.cwd).await {
            Ok(b) => b,
            Err(e) => return (Response::err(id, format!("open_pr: {e}")), Vec::new()),
        };

        let mut effects = Vec::new();
        let branch = match (current, requested) {
            (Some(cur), None) => cur,
            (Some(cur), Some(req)) if req == cur => cur,
            (Some(_), Some(req)) => match materialize_branch(&self.cwd, &req).await {
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
                match materialize_branch(&self.cwd, &desired).await {
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

        if let Err(e) = crate::git::push(&self.cwd, &branch).await {
            return (
                Response::err(id, format!("open_pr push failed: {e}")),
                effects,
            );
        }
        match crate::github::pr_create(&self.cwd, title, body, &self.base_branch).await {
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
        let current = match crate::git::current_branch(&self.cwd).await {
            Ok(b) => b,
            Err(e) => return (Response::err(id, format!("git_push: {e}")), Vec::new()),
        };

        let mut effects = Vec::new();
        let branch = match current {
            Some(cur) => cur,
            None => match arg_branch(args) {
                Some(req) => match materialize_branch(&self.cwd, &req).await {
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

        match crate::git::push(&self.cwd, &branch).await {
            Ok(summary) => (Response::ok(id, 0, summary, String::new()), effects),
            Err(e) => (Response::err(id, e.to_string()), effects),
        }
    }

    async fn git_update_branch(&self, id: &str) -> Response {
        // Auth for the fetch, plus hook-disabling on both ops: the workspace is
        // agent-writable, so a planted hook must not run on the host. `merge_git_env`
        // reindexes so the auth and no-hooks `GIT_CONFIG_*` sets don't collide.
        let no_hooks = crate::git::no_hooks_env();
        let auth = crate::git::merge_git_env(&[&crate::github::git_auth_env(), &no_hooks]);
        let fetch = run_git_command(id, &self.cwd, &["fetch", "origin", &self.base_branch], &auth)
            .await;
        if !fetch.ok || fetch.exit_code != Some(0) {
            return fetch;
        }
        // A clean merge creates a merge commit (needs an identity) and fires
        // `post-merge` on the host (needs hooks disabled).
        let env = crate::git::merge_git_env(&[
            &crate::git::identity_env(&self.cwd).await,
            &no_hooks,
        ]);
        let target = format!("origin/{}", self.base_branch);
        run_git_command(id, &self.cwd, &["merge", "--no-edit", &target], &env).await
    }
}

fn arg_branch(args: &Value) -> Option<String> {
    args.get("branch")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

async fn materialize_branch(worktree: &Path, desired: &str) -> std::result::Result<String, String> {
    crate::git::checkout_new_unique_branch(worktree, desired)
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

async fn run_git_command(id: &str, cwd: &Path, args: &[&str], env: &[(String, String)]) -> Response {
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
    async fn git_commit_stages_and_commits_the_worktree() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q"]);
        run_git(&repo, &["config", "user.email", "t@example.com"]);
        run_git(&repo, &["config", "user.name", "Tester"]);
        std::fs::write(repo.join("new.txt"), b"hello").unwrap();

        let rpc_dir = td.path().join("rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        write_request(
            &rpc_dir.join("requests"),
            "c1.json",
            r#"{"id":"c1","op":"git_commit","args":{"message":"add new.txt"}}"#,
        );

        let dispatcher = dispatcher(&repo);
        process_pending(&rpc_dir, &dispatcher).await;

        let body = std::fs::read_to_string(rpc_dir.join("responses/c1.json")).unwrap();
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], true, "response: {body}");

        let log = std::process::Command::new("git")
            .current_dir(&repo)
            .args(["log", "--oneline"])
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&log.stdout).contains("add new.txt"));
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

    fn setup_origin_and_clone() -> (tempfile::TempDir, std::path::PathBuf) {
        let td = tempfile::tempdir().unwrap();
        let origin = td.path().join("origin.git");
        std::fs::create_dir_all(&origin).unwrap();
        run_git(&origin, &["init", "-q", "--bare", "-b", "main"]);

        let work = td.path().join("work");
        let out = std::process::Command::new("git")
            .current_dir(td.path())
            .args(["clone", "-q", origin.to_str().unwrap(), "work"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        run_git(&work, &["config", "user.email", "t@example.com"]);
        run_git(&work, &["config", "user.name", "Tester"]);
        run_git(&work, &["checkout", "-q", "-b", "main"]);
        std::fs::write(work.join("a.txt"), b"base\n").unwrap();
        run_git(&work, &["add", "-A"]);
        run_git(&work, &["commit", "-q", "-m", "init"]);
        run_git(&work, &["push", "-q", "-u", "origin", "main"]);

        run_git(&work, &["checkout", "-q", "-b", "feat"]);
        std::fs::write(work.join("feat.txt"), b"feature\n").unwrap();
        run_git(&work, &["add", "-A"]);
        run_git(&work, &["commit", "-q", "-m", "feat work"]);
        run_git(&work, &["checkout", "-q", "main"]);
        std::fs::write(work.join("b.txt"), b"advance\n").unwrap();
        run_git(&work, &["add", "-A"]);
        run_git(&work, &["commit", "-q", "-m", "main advances"]);
        run_git(&work, &["push", "-q", "origin", "main"]);
        run_git(&work, &["checkout", "-q", "feat"]);
        (td, work)
    }

    #[tokio::test]
    async fn git_update_branch_merges_the_advanced_base() {
        let (td, work) = setup_origin_and_clone();
        let rpc_dir = td.path().join("rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        write_request(
            &rpc_dir.join("requests"),
            "u1.json",
            r#"{"id":"u1","op":"git_update_branch"}"#,
        );

        let dispatcher = dispatcher(&work);
        process_pending(&rpc_dir, &dispatcher).await;

        let body = std::fs::read_to_string(rpc_dir.join("responses/u1.json")).unwrap();
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], true, "response: {body}");
        assert_eq!(v["exit_code"], 0, "response: {body}");
        assert!(work.join("b.txt").exists());
    }

    #[tokio::test]
    async fn git_update_branch_reports_conflicts_and_leaves_merge_open() {
        let (td, work) = setup_origin_and_clone();
        std::fs::write(work.join("a.txt"), b"feat version\n").unwrap();
        run_git(&work, &["add", "-A"]);
        run_git(&work, &["commit", "-q", "-m", "feat edits a"]);
        run_git(&work, &["checkout", "-q", "main"]);
        std::fs::write(work.join("a.txt"), b"main version\n").unwrap();
        run_git(&work, &["add", "-A"]);
        run_git(&work, &["commit", "-q", "-m", "main edits a"]);
        run_git(&work, &["push", "-q", "origin", "main"]);
        run_git(&work, &["checkout", "-q", "feat"]);

        let rpc_dir = td.path().join("rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        write_request(
            &rpc_dir.join("requests"),
            "u2.json",
            r#"{"id":"u2","op":"git_update_branch"}"#,
        );

        let dispatcher = dispatcher(&work);
        process_pending(&rpc_dir, &dispatcher).await;

        let body = std::fs::read_to_string(rpc_dir.join("responses/u2.json")).unwrap();
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], true, "response: {body}");
        assert_ne!(v["exit_code"], 0, "response: {body}");
        assert!(v["stdout"].as_str().unwrap().contains("CONFLICT"));
        let status = std::process::Command::new("git")
            .current_dir(&work)
            .args(["status", "--porcelain=v1"])
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&status.stdout).contains("UU a.txt"));
    }

    #[tokio::test]
    async fn git_commit_rejects_empty_message() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q", "-b", "main"]);
        run_git(&repo, &["config", "user.email", "t@example.com"]);
        run_git(&repo, &["config", "user.name", "Tester"]);

        let rpc_dir = td.path().join("rpc");
        ensure_mailbox(&rpc_dir).unwrap();
        write_request(
            &rpc_dir.join("requests"),
            "c2.json",
            r#"{"id":"c2","op":"git_commit","args":{"message":"  "}}"#,
        );

        let dispatcher = dispatcher(&repo);
        process_pending(&rpc_dir, &dispatcher).await;

        let body = std::fs::read_to_string(rpc_dir.join("responses/c2.json")).unwrap();
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().contains("message"));
    }

    fn has_action_done(effects: &[RpcEvent], expect_op: &str) -> bool {
        effects.iter().any(|e| {
            let RpcEvent::Named { name, payload } = e;
            name == EVENT_ACTION_DONE && payload["op"] == expect_op
        })
    }

    #[tokio::test]
    async fn successful_mutating_op_emits_action_done() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q"]);
        run_git(&repo, &["config", "user.email", "t@example.com"]);
        run_git(&repo, &["config", "user.name", "Tester"]);
        std::fs::write(repo.join("new.txt"), b"hello").unwrap();

        let disp = dispatcher(&repo);
        let (resp, effects) = disp
            .dispatch_inner("c1", "git_commit", &json!({"message": "add new.txt"}))
            .await;
        assert!(resp.ok, "commit should succeed");
        assert!(
            has_action_done(&effects, "git_commit"),
            "a successful git_commit must emit the action-done signal"
        );
    }

    #[tokio::test]
    async fn read_only_and_failed_ops_emit_no_action_done() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q"]);
        let disp = dispatcher(&repo);

        // Read-only op: never an action-done signal.
        let (_r, status_fx) = disp.dispatch_inner("s", "git_status", &Value::Null).await;
        assert!(
            !has_action_done(&status_fx, "git_status"),
            "git_status is read-only and must not signal an action"
        );

        // A failed mutating op (empty message) must NOT signal success.
        let (resp, commit_fx) = disp
            .dispatch_inner("c", "git_commit", &json!({"message": "   "}))
            .await;
        assert!(!resp.ok);
        assert!(
            !has_action_done(&commit_fx, "git_commit"),
            "a failed git_commit must not signal an action"
        );
    }
}
