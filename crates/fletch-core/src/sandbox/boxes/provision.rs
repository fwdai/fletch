//! A checkout provisioned *inside* a box, replacing `sandbox::provision`'s
//! clone for box-stamped agents. Only the base delivery differs: `git init` in
//! the guest, and the host pushes the base commit in over the channel (a full
//! copy — no `--shared`, no alternates; the guest has no view of the user's
//! repository). Everything after is the one pipeline every checkout gets
//! (`sandbox::provision::fixups`), git and file writes routed to the guest by
//! the executor seam, with the checkout from the pushed base as its step.
//!
//! The host also gets an empty placeholder directory at the checkout path: the
//! ssh launcher is spawned with the checkout as its cwd, which must exist on
//! the host, and the host-side checks that key on the directory (`is_inside`
//! canonicalizes it) then behave as for any checkout. Teardown removes it with
//! the agent's parent dir.

use std::path::Path;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::git;
use crate::sandbox::provision::{self as clone, BaseFreshness, CheckoutSpec};

use super::mirror;
use super::registry::{BoxIdentity, BoxRegistry, RunningBox};

/// `git::fetch_fork_point`'s bound, for the same reason: this sits inside the
/// supervisor's spawn watchdog.
const BASE_FETCH_TIMEOUT: Duration = Duration::from_secs(8);
/// The push of a whole repository into the guest, over vsock.
const PUSH_TIMEOUT: Duration = Duration::from_secs(60);
/// The ref the base commit arrives under.
const BASE_REF: &str = "refs/fletch/base";

/// Where HEAD lands once the base is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Landing<'a> {
    /// Detached at the base, as every fresh workspace opens.
    Detached,
    /// On a branch of this name at the base: a restored agent's own branch.
    Branch(&'a str),
}

/// Provision `spec.dest` inside `identity`'s box at `spec.base_ref`, resolved
/// in `spec.source_repo`. With `base_branch` the base is first fetched from
/// the user's `origin` on the source repo (as `base_freshness` refreshes it),
/// so the workspace starts at the branch as GitHub has it; the fallback is
/// `spec.base_ref` as this machine knows it, reported `Stale`.
pub async fn provision(
    identity: &BoxIdentity,
    spec: &CheckoutSpec<'_>,
    base_branch: Option<&str>,
    landing: Landing<'_>,
) -> Result<BaseFreshness> {
    let running = BoxRegistry::global().ensure_running(identity).await?;
    if !spec.dest.starts_with(&identity.writable_root) {
        return Err(Error::InvalidPath(format!(
            "{} is not under the box's writable root {}",
            spec.dest.display(),
            identity.writable_root.display()
        )));
    }
    std::fs::create_dir_all(spec.dest)?;

    let has_origin = git::git_output(spec.source_repo, &["remote", "get-url", "origin"])
        .await?
        .status
        .success();
    let (sha, freshness) = base_commit(spec, base_branch, has_origin).await?;

    if let Err(e) = deliver_base(&running, spec, &sha, base_branch.filter(|_| has_origin)).await {
        clone::discard(spec.dest).await;
        return Err(e);
    }

    clone::fixups(spec, |dest| async move {
        match landing {
            Landing::Detached => {
                git::run_git(
                    &dest,
                    &["checkout", "--detach", BASE_REF],
                    "checkout --detach",
                )
                .await?;
            }
            Landing::Branch(branch) => {
                git::run_git(
                    &dest,
                    &["checkout", "-b", branch, BASE_REF],
                    &format!("checkout -b {branch}"),
                )
                .await?;
            }
        }
        Ok(())
    })
    .await?;
    mirror::record_source(&running.dir, spec.dest, spec.source_repo)?;
    // The provider's home in the guest (onboarding state, shared config) is
    // seeded by every launch (`home::seed`), not here: a checkout is
    // provisioned once, the user's config changes between launches.
    Ok(freshness)
}

/// The base delivery: an empty repository at the checkout path in the guest,
/// then from the host the base commit `sha` (and, with `tracking`, the
/// tracking ref `origin/<tracking>` the Git panel measures against) over the
/// channel, a full copy of what it reaches.
async fn deliver_base(
    running: &RunningBox,
    spec: &CheckoutSpec<'_>,
    sha: &str,
    tracking: Option<&str>,
) -> Result<()> {
    let dest = path_str(spec.dest)?;
    let mut init = crate::git_dist::command(&running.identity.writable_root);
    init.args(["init", "-q", &dest]);
    git_ok(&mut init, "git init in the box").await?;

    let url = running.git_url(spec.dest);
    let mut push = crate::git_dist::command(spec.source_repo);
    push.env("GIT_SSH_COMMAND", running.channel.git_ssh_command());
    push.args(["push", "--quiet", &url, &format!("+{sha}:{BASE_REF}")]);
    if let Some(base) = tracking {
        push.arg(format!("+{sha}:refs/remotes/origin/{base}"));
    }
    let out = tokio::time::timeout(PUSH_TIMEOUT, push.output())
        .await
        .map_err(|_| Error::Git("pushing the base into the box timed out".into()))??;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "pushing the base into the box failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Restore a box agent's checkout at `spec.base_ref` (its archived tip): on
/// `branch` when the tip can be had, detached otherwise, and at `fallback_ref`
/// (the archived parent-branch tip) when the tip is gone for good — the same
/// ladder `sandbox::provision::provision_on_branch` walks for a clone, over the
/// source repository's store (fetching the tip from `origin` by id into it when
/// it is missing, as `git::fetch_commit` does for a merged-then-deleted
/// branch). Returns whether it landed on `branch`.
pub async fn restore(
    identity: &BoxIdentity,
    spec: &CheckoutSpec<'_>,
    branch: Option<&str>,
    fallback_ref: Option<&str>,
) -> Result<bool> {
    let tip_present = present(spec.source_repo, spec.base_ref).await || {
        if let Err(e) = git::fetch_commit(spec.source_repo, spec.base_ref).await {
            tracing::debug!(tip = spec.base_ref, error = %e, "restore: tip fetch failed");
        }
        present(spec.source_repo, spec.base_ref).await
    };
    if tip_present {
        let landing = branch.map_or(Landing::Detached, Landing::Branch);
        provision(identity, spec, None, landing).await?;
        return Ok(branch.is_some());
    }
    if let Some(base) = fallback_ref {
        if present(spec.source_repo, base).await {
            tracing::warn!(
                tip = spec.base_ref,
                fallback = base,
                "restore: branch tip unrecoverable from origin; opening the box detached \
                 at the parent base (agent commits not restored)"
            );
            let at_base = CheckoutSpec {
                source_repo: spec.source_repo,
                base_ref: base,
                dest: spec.dest,
            };
            provision(identity, &at_base, None, Landing::Detached).await?;
            return Ok(false);
        }
    }
    Err(Error::Git(format!(
        "restore: branch tip {} is unreachable: origin does not serve the commit, and no \
         fallback base was recoverable",
        spec.base_ref
    )))
}

/// The commit the workspace opens at, and how fresh it is.
async fn base_commit(
    spec: &CheckoutSpec<'_>,
    base_branch: Option<&str>,
    has_origin: bool,
) -> Result<(String, BaseFreshness)> {
    if let Some(base) = base_branch {
        if !has_origin {
            let sha = resolve(spec.source_repo, spec.base_ref).await?;
            return Ok((sha, BaseFreshness::NoRemote));
        }
        let fetched =
            tokio::time::timeout(BASE_FETCH_TIMEOUT, git::fetch_base(spec.source_repo, base))
                .await
                .map_err(|_| Error::Git("fetching the base timed out".into()))
                .and_then(|r| r);
        match fetched {
            Ok(()) => {
                if let Ok(sha) =
                    resolve(spec.source_repo, &format!("refs/remotes/origin/{base}")).await
                {
                    return Ok((sha, BaseFreshness::Fetched));
                }
            }
            Err(e) => tracing::warn!(
                base,
                error = %e,
                "could not fetch the base from origin while provisioning a box; \
                 starting from the inherited ref, which may be stale"
            ),
        }
        let sha = resolve(spec.source_repo, spec.base_ref)
            .await
            .map_err(|_| {
                Error::Git(format!(
                    "could not fetch `{base}` from origin, and this machine has no copy of it \
                 to fall back to — there is no commit to open the workspace at."
                ))
            })?;
        return Ok((sha, BaseFreshness::Stale));
    }
    // A restore names an exact commit; nothing to be stale against.
    let sha = resolve(spec.source_repo, spec.base_ref).await?;
    Ok((sha, BaseFreshness::NoRemote))
}

/// Whether `rev` names a commit in `repo`.
async fn present(repo: &Path, rev: &str) -> bool {
    matches!(git::resolve(repo, rev).await, Ok(Some(_)))
}

async fn resolve(repo: &Path, rev: &str) -> Result<String> {
    git::resolve(repo, rev)
        .await?
        .ok_or_else(|| Error::Git(format!("{rev} is not a commit in {}", repo.display())))
}

async fn git_ok(cmd: &mut tokio::process::Command, what: &str) -> Result<()> {
    let out = git::output_timed(cmd, what).await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "{what} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

fn path_str(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| Error::InvalidPath(path.display().to_string()))
}
