//! A checkout provisioned *inside* a box, replacing `sandbox::provision`'s
//! clone for box-stamped agents: `git init` in the guest,
//! the host pushes the base commit in over the channel (a full copy — no
//! `--shared`, no alternates; the guest has no view of the user's repository),
//! the guest checks it out, then the fixups every clone gets. Those that are
//! git in the checkout (`seed_identity`, `rewrite_origin_head`) are the
//! existing ones, routed to the guest by the seam; the hooks are written over
//! the channel because they need the executable bit.
//!
//! The host also gets an empty placeholder directory at the checkout path: the
//! ssh launcher is spawned with the checkout as its cwd, which must exist on
//! the host, and the host-side checks that key on the directory (`is_inside`
//! canonicalizes it) then behave as for any checkout. Teardown removes it with
//! the agent's parent dir.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use fletch_box::Channel;

use crate::error::{Error, Result};
use crate::git;
use crate::sandbox::provision::{self as clone, BaseFreshness, CheckoutSpec};

use super::registry::{BoxIdentity, BoxRegistry};
use super::{executor, mirror};

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

    let origin = origin_url(spec.source_repo).await?;
    let (sha, freshness) = base_commit(spec, base_branch, origin.is_some()).await?;

    // In the guest: an empty repository at the checkout path.
    let dest = path_str(spec.dest)?;
    let mut init = crate::git_dist::command(&identity.writable_root);
    init.args(["init", "-q", &dest]);
    git_ok(&mut init, "git init in the box").await?;

    // From the host: the base commit (and the tracking ref the Git panel
    // measures against) over the channel, a full copy of what it reaches.
    let url = running.git_url(spec.dest);
    let mut push = crate::git_dist::command(spec.source_repo);
    push.env("GIT_SSH_COMMAND", running.channel.git_ssh_command());
    push.args(["push", "--quiet", &url, &format!("+{sha}:{BASE_REF}")]);
    if let Some(base) = base_branch.filter(|_| origin.is_some()) {
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

    // The clone's fixups, in the clone's order: origin first (the checkout
    // step may need it), hooks and identity, the checkout, `origin/HEAD` last.
    match &origin {
        Some(url) => {
            git::run_git(
                spec.dest,
                &["remote", "add", "origin", url],
                "remote add origin",
            )
            .await?;
        }
        None => tracing::info!(
            source = %spec.source_repo.display(),
            "source repo has no origin remote; the box checkout gets none"
        ),
    }
    clone::seed_identity(spec).await?;
    install_hooks(&running.channel, spec.dest).await?;
    match landing {
        Landing::Detached => {
            git::run_git(
                spec.dest,
                &["checkout", "--detach", BASE_REF],
                "checkout --detach",
            )
            .await?;
        }
        Landing::Branch(branch) => {
            git::run_git(
                spec.dest,
                &["checkout", "-b", branch, BASE_REF],
                &format!("checkout -b {branch}"),
            )
            .await?;
        }
    }
    clone::rewrite_origin_head(spec).await;
    mirror::record_source(&running.dir, spec.dest, spec.source_repo)?;
    // The provider's home in the guest (onboarding state, shared config) is
    // seeded by every launch (`home::seed`), not here: a checkout is
    // provisioned once, the user's config changes between launches.
    Ok(freshness)
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

/// The source's `origin` in the https form provisioning writes, if any.
async fn origin_url(source_repo: &Path) -> Result<Option<String>> {
    let out = git::git_output(source_repo, &["remote", "get-url", "origin"]).await?;
    Ok(out
        .status
        .success()
        .then(|| crate::github::github_https_remote(String::from_utf8_lossy(&out.stdout).trim())))
}

/// The delegation hooks and the attribution hook, as `sandbox::provision` and
/// `attribution` write them into a host clone, put over the channel with the
/// executable bit the guest's git requires.
async fn install_hooks(channel: &Arc<Channel>, dest: &Path) -> Result<()> {
    let hooks = dest.join(".git/hooks");
    let files = [
        (
            "post-merge",
            clone::delegation_hook_script(r#"action="git_update_branch""#),
        ),
        (
            "post-commit",
            clone::delegation_hook_script(clone::POST_COMMIT_ACTION),
        ),
        ("commit-msg", crate::attribution::commit_msg_hook()),
    ];
    for (name, body) in files {
        put(channel, body.into_bytes(), hooks.join(name), 0o755).await?;
    }
    Ok(())
}

async fn put(
    channel: &Arc<Channel>,
    bytes: Vec<u8>,
    path: std::path::PathBuf,
    mode: u32,
) -> Result<()> {
    let channel = channel.clone();
    tokio::task::spawn_blocking(move || executor::put(&channel, &bytes, &path, mode))
        .await
        .map_err(|e| Error::Other(e.to_string()))?
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
