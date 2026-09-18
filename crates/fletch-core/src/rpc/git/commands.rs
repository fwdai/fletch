use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;

use super::args::{arg_branch_named, refuse_option_like};
use super::GitDispatcher;
use crate::rpc::Response;

/// A hung command surfaces as an error instead of blocking the watcher; the
/// agent's own poll timeout is shorter.
pub(super) const OP_TIMEOUT: Duration = Duration::from_secs(120);

impl GitDispatcher {
    pub(super) async fn git_status(&self, id: &str, args: &Value) -> Response {
        match self.target(id, args) {
            Ok(t) => {
                run_git_command(id, &t.cwd, &["status", "--porcelain=v1", "--branch"], &[]).await
            }
            Err(resp) => resp,
        }
    }

    /// Fetches with the host-held token so the agent can merge `origin/<base>`
    /// natively without the token entering the sandbox. Hooks are disabled for the
    /// same reason as push.
    pub(super) async fn git_fetch(&self, id: &str, args: &Value) -> Response {
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

pub(super) async fn run_git_command(
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
#[path = "tests/commands.rs"]
mod tests;
