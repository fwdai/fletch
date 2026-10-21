use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;

use super::args::{arg_branch_named, refuse_option_like};
use super::GitDispatcher;
use crate::git::{Branches, Transport};
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
    /// same reason as push. The remote is the user's repository's, never the
    /// checkout's `origin` (`git::remote`): this op needs no approval, so a
    /// checkout whose origin the agent repointed would otherwise make the host
    /// fetch any repository the token can read into the agent's reach.
    ///
    /// `args.ref` names one branch, `args.refs` several, and either may be a
    /// pull request's head (`pull/12/head`), which lands as `origin/pr/12`.
    /// Without a ref the base branch is fetched. Agents needed several
    /// branches of a stack and other PRs' heads, and ran `git fetch origin …`
    /// themselves to get them.
    pub(super) async fn git_fetch(&self, id: &str, args: &Value) -> Response {
        let t = match self.target(id, args) {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        let refs = match requested_refs(args) {
            Ok(refs) if refs.is_empty() => vec![t.base_branch.clone()],
            Ok(refs) => refs,
            Err(e) => return Response::err(id, format!("git_fetch: {e}")),
        };
        for r in &refs {
            if let Some(resp) = refuse_option_like(id, "git_fetch", "ref", r) {
                return resp;
            }
        }
        let transport = match Transport::open(&t.cwd, &t.source_repo).await {
            Ok(transport) => transport,
            Err(e) => return Response::err(id, format!("git_fetch: {e}")),
        };
        // A failed fetch is a hard error, or the agent merges a stale
        // `origin/<branch>`. One round trip per ref: the transport lands each
        // under its own tracking name.
        for r in &refs {
            if let Err(e) = transport.fetch_branches(requested_branch(r)).await {
                return Response::err(
                    id,
                    format!("git_fetch: fetch origin {} failed: {e}", refs.join(" ")),
                );
            }
        }
        Response::ok(id, 0, String::new(), String::new())
    }

    /// The branches on `origin` right now, as `{ "branch", "sha" }` rows,
    /// optionally narrowed to those starting with `args.prefix`. Asked over
    /// the wire through the same transport as a fetch — the user's remote,
    /// never the checkout's — so a branch created since the last fetch is
    /// seen; agents ran `git ls-remote --heads origin` for this when checking
    /// whether a stack's branches existed.
    pub(super) async fn git_ls_remote(&self, id: &str, args: &Value) -> Response {
        let t = match self.target(id, args) {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        let prefix = arg_branch_named(args, "prefix");
        let transport = match Transport::open(&t.cwd, &t.source_repo).await {
            Ok(transport) => transport,
            Err(e) => return Response::err(id, format!("git_ls_remote: {e}")),
        };
        let heads = match transport.branches().await {
            Ok(heads) => heads,
            Err(e) => return Response::err(id, format!("git_ls_remote: {e}")),
        };
        // The filter is applied here, not passed to git: a pattern argument is
        // a glob to ls-remote, and the agent asked for a prefix.
        match serde_json::to_string_pretty(&remote_heads(heads, prefix.as_deref())) {
            Ok(json) => Response::ok(id, 0, json, String::new()),
            Err(e) => Response::err(id, format!("git_ls_remote: {e}")),
        }
    }
}

/// `args.ref` (one) and `args.refs` (several), trimmed, blanks dropped.
pub(super) fn requested_refs(args: &Value) -> std::result::Result<Vec<String>, String> {
    let mut refs = Vec::new();
    if let Some(one) = arg_branch_named(args, "ref") {
        refs.push(one);
    }
    match args.get("refs") {
        None | Some(Value::Null) => {}
        Some(Value::Array(items)) => {
            for item in items {
                match item.as_str().map(str::trim) {
                    Some(s) if !s.is_empty() => refs.push(s.to_string()),
                    Some(_) => {}
                    None => {
                        return Err(format!(
                            "`refs` must be an array of branch names, got {item}"
                        ))
                    }
                }
            }
        }
        Some(other) => {
            return Err(format!(
                "`refs` must be an array of branch names, got {other}"
            ))
        }
    }
    refs.dedup();
    Ok(refs)
}

/// What one requested ref names: a branch, or a pull request's head spelled
/// `pull/<n>/head` or `refs/pull/<n>/head`, which the transport lands as
/// `origin/pr/<n>` (GitHub advertises it, no clone tracks it by default).
pub(super) fn requested_branch(r: &str) -> Branches<'_> {
    let rest = r.strip_prefix("refs/").unwrap_or(r);
    rest.strip_prefix("pull/")
        .and_then(|s| s.strip_suffix("/head"))
        .and_then(|n| n.parse::<u32>().ok())
        .filter(|n| *n > 0)
        .map(Branches::PullRequest)
        .unwrap_or(Branches::One(r))
}

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
pub(super) struct RemoteHead {
    pub branch: String,
    pub sha: String,
}

/// The transport's `(branch, commit)` pairs as rows, keeping those under
/// `prefix` when given.
pub(super) fn remote_heads(heads: Vec<(String, String)>, prefix: Option<&str>) -> Vec<RemoteHead> {
    heads
        .into_iter()
        .filter(|(branch, _)| prefix.map_or(true, |p| branch.starts_with(p)))
        .map(|(branch, sha)| RemoteHead { branch, sha })
        .collect()
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
