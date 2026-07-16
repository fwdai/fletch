//! Workspace provisioning: how an agent's checkout comes into existence.
//!
//! Two modes. `Worktree` is a linked `git worktree` whose `.git` file points
//! back into the origin repo. `Clone` is a self-contained checkout with its own
//! real, writable `.git`, required by the Docker engine: a linked worktree's
//! `.git` file references the origin repo's
//! `.git/worktrees/<name>` by absolute path, so containerizing it would mean
//! mounting the user's real `.git` — a sandbox escape (a writable `.git/hooks`
//! executes on the host the next time the user runs git).
//!
//! The clone is made with `git clone --shared`: it borrows the source's object
//! store via `.git/objects/info/alternates` (an absolute path to the source's
//! objects) and copies **no** objects, so a spawn costs kilobytes and
//! milliseconds instead of a full history copy. New objects (agent commits,
//! fetches) land in the clone's own `.git/objects`; reads of existing history
//! fall through to the borrowed store. Because the clone lives at the normal
//! host path, all host-side git (diff polling, RPC commit/push,
//! archive/restore) operates on it unchanged. For Docker the borrowed object
//! store is mounted read-only at its identical host path (see
//! `sandbox::docker::engine`); under seatbelt no mount is needed (same
//! filesystem, reads open, writes blocked outside the workspace by policy).
//! The source object store must therefore remain present for the clone's
//! lifetime — Fletch owns the source repo lifecycle, so this holds.
//!
//! The effective mode is chosen per agent at spawn time (see
//! `supervisor::lifecycle::effective_workspace_mode`): Docker always uses
//! `Clone`; seatbelt uses `Clone` too unless the `workspace_mode` dev flag
//! (set via sqlite, not exposed in UI) opts back into linked worktrees.

use std::path::Path;

use crate::error::{Error, Result};
use crate::git;

/// Settings-table key overriding the provisioning mode under seatbelt:
/// `"worktree"` or `"clone"`. Docker always uses `Clone` regardless of this
/// key; see `supervisor::lifecycle::effective_workspace_mode`.
pub const WORKSPACE_MODE_SETTING: &str = "workspace_mode";

/// How an agent workspace is materialized from its source repo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceMode {
    /// Linked `git worktree` sharing the source repo's object store.
    Worktree,
    /// Self-contained `git clone --shared` — its own writable `.git`, objects
    /// borrowed from the source via alternates (Docker-safe).
    Clone,
}

/// What to check out where. `base_ref` is any commit-ish; pass `"HEAD"` for
/// "the source repo's current HEAD" (the no-base behavior).
pub struct CheckoutSpec<'a> {
    /// The user's real repo root.
    pub source_repo: &'a Path,
    /// Commit-ish the workspace starts from, checked out detached.
    pub base_ref: &'a str,
    /// Workspace path (`workspace::repo_checkout_path(agent_id, subdir)`).
    pub dest: &'a Path,
}

/// Create the workspace at `spec.dest`, detached at `spec.base_ref`.
pub async fn provision(mode: WorkspaceMode, spec: &CheckoutSpec<'_>) -> Result<()> {
    match mode {
        WorkspaceMode::Worktree => {
            git::worktree_add_detached(spec.source_repo, spec.dest, Some(spec.base_ref)).await
        }
        // `--shared`: objects borrowed from the source. For Docker the borrowed
        // store is mounted RO at launch — safe for the primary workspace and
        // any repo present when the container starts.
        WorkspaceMode::Clone => clone_detached(spec, true).await,
    }
}

/// Provision a **self-contained** clone (full object copy, no alternates),
/// detached at `spec.base_ref`. For a repo added to an *already-running* Docker
/// agent: the container's bind mounts are fixed at `docker run`, so a `--shared`
/// clone's borrowed object store could never be mounted and in-container git
/// would fail with missing objects. A self-contained clone needs no extra mount
/// and works immediately. Seatbelt has no such constraint and uses [`provision`]
/// (`--shared`) even for added repos — same filesystem, no mount involved.
pub async fn provision_self_contained(spec: &CheckoutSpec<'_>) -> Result<()> {
    clone_detached(spec, false).await
}

/// Shared clone-arm body for [`provision`] / [`provision_self_contained`]:
/// clone (borrowing objects when `shared`), run the post-clone fixups, then
/// check out `spec.base_ref` detached.
async fn clone_detached(spec: &CheckoutSpec<'_>, shared: bool) -> Result<()> {
    clone_base(spec, shared).await?;
    finish_clone(spec, |dest| async move {
        git::run_git(
            &dest,
            &["checkout", "--detach", spec.base_ref],
            &format!("checkout --detach {}", spec.base_ref),
        )
        .await?;
        Ok(())
    })
    .await
}

/// Provision a workflow step workspace: a `--shared` clone of the source repo
/// that then forks from `spec.base_ref` **in the run repository** (spec §12.1).
///
/// A previous step's commit exists only in that step's (now-disposable) clone
/// and was ferried into the run repo, so it is unreachable from a fresh clone of
/// the source. This is the one provisioning extension §12.1 calls for: after the
/// source clone, `git fetch <run-repo> <base_ref>` before detaching. Step 1's
/// base is the run's `base_sha` (already present in the source clone), so the
/// fetch is skipped when the ref already resolves — the same `commit_present`
/// gate the branch-restore path uses.
pub async fn provision_forking_run_repo(spec: &CheckoutSpec<'_>, run_repo: &Path) -> Result<()> {
    clone_base(spec, true).await?;
    finish_clone(spec, |dest| async move {
        if !commit_present(&dest, spec.base_ref).await {
            let src = path_str(run_repo)?;
            let refspec = format!("{}:{}", spec.base_ref, spec.base_ref);
            git::run_git(
                &dest,
                &["fetch", &src, &refspec],
                "fetch fork ref from run repo",
            )
            .await?;
        }
        git::run_git(
            &dest,
            &["checkout", "--detach", spec.base_ref],
            &format!("checkout --detach {}", spec.base_ref),
        )
        .await?;
        Ok(())
    })
    .await
}

/// Create the workspace for a restored agent that had already materialized a
/// branch, checked out at `spec.base_ref` (its archived tip). Returns `true`
/// when it landed on the branch, `false` when it degraded to a detached
/// checkout (see [`recover_and_checkout`]) — the caller then records no branch,
/// exactly as for a never-pushed agent.
///
/// Worktree: the branch is created in the source repo (worktree branches are
/// refs of the origin repo) and the worktree attached to it — always on-branch.
/// Clone: the branch is created inside the clone; when `base_ref` isn't present
/// in the source repo (the agent's commits lived only in the torn-down clone),
/// it is recovered from `origin`. Recovery no longer requires the branch to
/// still exist on the remote: a branch auto-deleted after its PR merged is
/// restored by fetching the tip commit directly (still reachable from the base
/// branch), and a truly-lost tip falls back to `fallback_ref`.
///
/// `origin_branch` is the branch name as the remote knows it. It differs from
/// `branch` when restore renamed to dodge a local collision (`feat` →
/// `feat-restored`): the remote only has the original name, so fetching the
/// renamed one would fail and make a pushed branch unrestorable.
///
/// `fallback_ref` is a last-resort commit-ish (the archived parent-branch tip)
/// to open detached at when the agent's own tip is gone for good.
pub async fn provision_on_branch(
    mode: WorkspaceMode,
    spec: &CheckoutSpec<'_>,
    branch: &str,
    origin_branch: &str,
    fallback_ref: Option<&str>,
) -> Result<bool> {
    match mode {
        WorkspaceMode::Worktree => {
            git::branch_create_at(spec.source_repo, branch, spec.base_ref).await?;
            git::worktree_add_branch(spec.source_repo, spec.dest, branch).await?;
            Ok(true)
        }
        WorkspaceMode::Clone => {
            // Restore always relaunches the container, so the RO mount for a
            // `--shared` clone's borrowed store is re-established at launch.
            clone_base(spec, true).await?;
            let branch = branch.to_string();
            let origin_branch = origin_branch.to_string();
            let fallback = fallback_ref.map(str::to_string);
            finish_clone(spec, |dest| async move {
                recover_and_checkout(
                    &dest,
                    spec.base_ref,
                    &branch,
                    &origin_branch,
                    fallback.as_deref(),
                )
                .await
            })
            .await
        }
    }
}

/// Recover a restored agent's branch tip inside the fresh clone `dest` and check
/// the workspace out on it, degrading gracefully as availability shrinks so a
/// merged-and-deleted branch — or even a lost tip — never blocks a restore:
///
///   1. tip already present (borrowed via alternates) → local `branch` at tip
///   2. `origin/<origin_branch>` still exists → fetch it → local `branch` at tip
///   3. branch gone but the tip is still reachable on origin (the usual
///      auto-delete-after-merge case: the commit survives on the base branch)
///      → fetch the tip by SHA → **detached** at the tip
///   4. tip unrecoverable, but `fallback_ref` (the parent-branch tip) is
///      reachable → **detached** there, so the workspace still opens with its
///      history and PR link even though the agent's own commits are gone
///
/// Returns `true` when it landed on `branch`, `false` when it detached.
async fn recover_and_checkout(
    dest: &Path,
    tip: &str,
    branch: &str,
    origin_branch: &str,
    fallback_ref: Option<&str>,
) -> Result<bool> {
    // 1 + 2: the tip is local, or its branch still exists on origin → the
    // agent's branch is meaningfully still there, so recreate it at the tip.
    if commit_present(dest, tip).await || fetch_branch(dest, origin_branch).await.is_ok() {
        git::run_git(
            dest,
            &["checkout", "-b", branch, tip],
            &format!("checkout -b {branch}"),
        )
        .await?;
        return Ok(true);
    }
    // 3: branch is gone but the merged commit lingers on the base branch —
    // fetch it by SHA. The branch name no longer means anything on the remote,
    // so open detached rather than fabricating a local branch for it.
    if fetch_commit(dest, tip).await.is_ok() {
        checkout_detached(dest, tip).await?;
        return Ok(false);
    }
    // 4: the tip is gone for good. Open at the parent base (if we can reach it)
    // so the workspace, its session history, and its PR link still come back —
    // only the agent's own commits are unrecoverable.
    if let Some(base) = fallback_ref {
        if commit_present(dest, base).await || fetch_commit(dest, base).await.is_ok() {
            tracing::warn!(
                tip,
                fallback = base,
                "restore: branch tip unrecoverable from origin; opening detached \
                 at parent base (agent commits not restored)"
            );
            checkout_detached(dest, base).await?;
            return Ok(false);
        }
    }
    Err(Error::Git(format!(
        "restore: branch tip {tip} is unreachable — origin has neither branch \
         {origin_branch} nor the commit, and no fallback base was recoverable"
    )))
}

async fn checkout_detached(dest: &Path, at: &str) -> Result<()> {
    git::run_git(
        dest,
        &["checkout", "--detach", at],
        &format!("checkout --detach {at}"),
    )
    .await?;
    Ok(())
}

/// Remove the workspace at `spec.dest`.
pub async fn teardown(mode: WorkspaceMode, spec: &CheckoutSpec<'_>) -> Result<()> {
    match mode {
        WorkspaceMode::Worktree => {
            // Prune first so a stale registration never blocks the remove;
            // best-effort, like the pre-existing disposition sweep.
            let _ = git::worktree_prune(spec.source_repo).await;
            git::worktree_remove(spec.source_repo, spec.dest, true).await
        }
        // A clone is self-contained: nothing to unregister in the source repo.
        WorkspaceMode::Clone => match tokio::fs::remove_dir_all(spec.dest).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::Other(format!(
                "remove clone workspace {}: {e}",
                spec.dest.display()
            ))),
        },
    }
}

/// Which mode produced the workspace on disk — a linked worktree has a `.git`
/// *file* (pointer into the origin repo), a clone has a `.git` *directory*.
/// `None` when the path holds neither (already gone, or never provisioned).
/// Teardown callers use this instead of re-reading the settings key, so a
/// dev-flag flip between spawn and archive can't tear down with the wrong arm.
pub fn detect_mode(dest: &Path) -> Option<WorkspaceMode> {
    let git_path = dest.join(".git");
    let meta = std::fs::symlink_metadata(&git_path).ok()?;
    if meta.is_dir() {
        Some(WorkspaceMode::Clone)
    } else {
        Some(WorkspaceMode::Worktree)
    }
}

/// `git clone` + origin rewrite + repo-local identity copy — the parts shared
/// by every clone-arm entry point. Leaves HEAD wherever the clone put it;
/// callers do their own checkout.
///
/// `shared` picks the object strategy:
/// - `true` → `git clone --shared`: borrows the source's object store via
///   `.git/objects/info/alternates` (an absolute path to the source's objects)
///   and copies no objects, so the clone is kilobytes on disk. The source
///   object store must stay present for the clone's lifetime (Fletch owns the
///   source repo lifecycle). Borrowed objects are only ever *referenced* —
///   never written — and for Docker the store is mounted read-only, so a
///   container can't reach through the alternates link to mutate the source.
/// - `false` → `git clone --no-hardlinks`: a full, self-contained object copy
///   with no alternates. Used when the clone can't rely on the borrowed store
///   being mounted — a repo added to an already-running Docker container, whose
///   bind mounts are fixed at `docker run` (see [`provision_self_contained`]).
///
/// Caveat — gc/prune on the source (`shared` only): `--shared` breaks only if
/// the source prunes an object the clone references. Base history stays
/// reachable from the source's own refs, so ordinary `git gc --auto` (prunes
/// only unreachable objects past the grace period) is safe. The real risk is an
/// aggressive `git prune` / `gc --prune=now` on the source *while an agent is
/// live and referencing a since-deleted base branch*. Not hardened against here
/// (the common case is safe); if that becomes a problem, disable gc on the
/// source while any agent is active (crash-safe, via transient env-config).
async fn clone_base(spec: &CheckoutSpec<'_>, shared: bool) -> Result<()> {
    let source = path_str(spec.source_repo)?;
    let dest = path_str(spec.dest)?;

    // A leftover directory at the target can only be an orphan from a crashed
    // spawn: agent-id allocation refuses ids whose checkout dir physically
    // exists (`occupied_checkout_dirs`), so no live workspace can be here.
    // Clear it rather than letting `git clone` fail on a non-empty dir.
    if spec.dest.exists() {
        tracing::warn!(path = %spec.dest.display(), "clearing orphan dir at clone target");
        tokio::fs::remove_dir_all(spec.dest).await?;
    }

    // `--shared` borrows objects via an alternates file (cheap, RO for Docker);
    // `--no-hardlinks` makes a full self-contained copy (no shared inodes, so a
    // container can't corrupt the source even without the RO mount).
    //
    // No timeout: `--shared` is near-instant and a local copy can legitimately
    // take minutes, so keep the unbounded shape (and `kill_on_drop`) consistent
    // with `new_project::clone`; the child is still reaped if the spawn task is
    // aborted.
    let object_flag = if shared { "--shared" } else { "--no-hardlinks" };
    let out = crate::git_dist::command(spec.source_repo)
        .args(["clone", object_flag, &source, &dest])
        .kill_on_drop(true)
        .output()
        .await?;
    if !out.status.success() {
        // Self-heal: a partial clone dir would make every retry fail with
        // "already exists" (mirrors `new_project::clone`).
        let _ = tokio::fs::remove_dir_all(spec.dest).await;
        return Err(Error::Git(format!(
            "clone {object_flag} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Run the shared post-clone fixups, then the mode-specific checkout step.
/// The origin rewrite must come first: the branch-restore checkout may need
/// to `fetch origin`, which has to hit the real remote, not the source path.
/// Any failure tears the half-built clone down so nothing orphaned blocks a
/// retry (a clone is self-contained, so `rm -rf` is always safe here).
async fn finish_clone<F, Fut, T>(spec: &CheckoutSpec<'_>, checkout: F) -> Result<T>
where
    F: FnOnce(std::path::PathBuf) -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let result = async {
        rewrite_origin(spec).await?;
        seed_identity(spec).await?;
        install_delegation_hooks(spec.dest).await?;
        checkout(spec.dest.to_path_buf()).await
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_dir_all(spec.dest).await;
    }
    result
}

/// Point the clone's `origin` at the source repo's real remote so push/PR/
/// fetch behave exactly as they would from a checkout. When the source has no
/// `origin`, the clone's implicit local-path `origin` is *removed*: keeping it
/// would let `git push -u origin <branch>` silently create branches and
/// objects inside the user's source repo. With no remote at all, push fails
/// cleanly — the same terminal state the source repo itself is in.
async fn rewrite_origin(spec: &CheckoutSpec<'_>) -> Result<()> {
    let out = git::git_output(spec.source_repo, &["remote", "get-url", "origin"]).await?;
    if !out.status.success() {
        tracing::info!(
            source = %spec.source_repo.display(),
            "source repo has no origin remote; removing the clone's local-path remote"
        );
        git::run_git(
            spec.dest,
            &["remote", "remove", "origin"],
            "remote remove origin",
        )
        .await?;
        return Ok(());
    }
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    git::run_git(
        spec.dest,
        &["remote", "set-url", "origin", &url],
        "remote set-url origin",
    )
    .await?;
    Ok(())
}

/// Install Fletch's delegation-signal git hooks into the clone's `.git/hooks`.
///
/// With local git mutations (commit, merge, conflict-resolve) now running as
/// native in-container git rather than host RPC ops, the old "a mutating RPC op
/// succeeded" delegation signal is gone for those actions. These hooks restore
/// it without teaching the agent anything: on the agent's own `git commit` /
/// `git merge`, git runs the hook, which pings the RPC mailbox
/// (`$FLETCH_RPC_DIR`, inherited from the triggering git process's env) so the
/// host relays an `agent:git-action` event — exactly what the panel's
/// delegation tracking consumes.
///
/// Contained by construction: host-side git (diff polling, push, PR, fetch)
/// runs with `core.hooksPath=/dev/null` (`git::no_hooks_env`), so these hooks
/// never fire on the host — only on the agent's sandboxed git, where any command
/// they run is already inside the sandbox. The hook is best-effort and always
/// exits 0, so a missing mailbox or a slow write can never fail the agent's
/// commit. Installed only for clones (this is the clone-arm path); a linked
/// worktree's hooks live in the user's real repo and must never be touched.
async fn install_delegation_hooks(dest: &Path) -> Result<()> {
    let hooks_dir = dest.join(".git/hooks");
    // A fresh clone always has `.git/hooks`, but create it defensively so a
    // future object-layout change can't silently drop the signal.
    tokio::fs::create_dir_all(&hooks_dir).await?;
    // post-merge fires on a completed clean `git merge` (fast-forward or merge
    // commit): the action is unambiguously a base merge.
    let post_merge = hooks_dir.join("post-merge");
    tokio::fs::write(
        &post_merge,
        delegation_hook_script(r#"action="git_update_branch""#),
    )
    .await?;
    set_executable(&post_merge).await?;
    // post-commit fires on *every* plain `git commit` — including the commit
    // that completes a *conflicted* merge, which never reaches post-merge. Those
    // two cases must report different actions or an unrelated commit made during
    // an `update-branch` delegation would falsely satisfy it. A merge-completion
    // commit is a merge commit (it has a second parent, `HEAD^2`); a plain commit
    // does not — so branch on that.
    let post_commit = hooks_dir.join("post-commit");
    let set_action = concat!(
        "if git rev-parse -q --verify HEAD^2 >/dev/null 2>&1; then\n",
        "  action=\"git_update_branch\"\n",
        "else\n",
        "  action=\"git_commit\"\n",
        "fi",
    );
    tokio::fs::write(&post_commit, delegation_hook_script(set_action)).await?;
    set_executable(&post_commit).await?;
    Ok(())
}

/// The body of a delegation hook. `set_action` is a shell fragment that assigns
/// the reported op to `$action` (a literal for post-merge, a merge-commit test
/// for post-commit). POSIX `sh`, no bashisms: runs in the container image's
/// shell and macOS `/bin/sh` alike. Writes the mailbox request atomically
/// (`.tmp` then `mv`) so the watcher never reads a half-written file, and
/// swallows every error — the git op must not depend on the signal landing.
fn delegation_hook_script(set_action: &str) -> String {
    format!(
        r#"#!/bin/sh
# Fletch-managed: delegation signal. Pings the app RPC mailbox so the panel can
# attribute this git action to the agent's turn. Best-effort; never blocks or
# fails the git operation. Do not edit — reinstalled on provision.
[ -n "$FLETCH_RPC_DIR" ] || exit 0
reqdir="$FLETCH_RPC_DIR/requests"
[ -d "$reqdir" ] || exit 0
{set_action}
id="hook-$$-$(date +%s%N 2>/dev/null || date +%s)"
tmp="$reqdir/$id.json.tmp"
printf '{{"id":"%s","op":"signal_git_action","args":{{"action":"%s"}}}}' "$id" "$action" > "$tmp" 2>/dev/null \
  && mv "$tmp" "$reqdir/$id.json" 2>/dev/null
exit 0
"#
    )
}

/// Mark a file user/group/other-executable (0755). The hooks must be executable
/// or git silently ignores them.
async fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = tokio::fs::metadata(path).await?.permissions();
    perms.set_mode(0o755);
    tokio::fs::set_permissions(path, perms).await?;
    Ok(())
}

/// Seed the clone with a git identity it can commit under **without** the
/// host's global gitconfig. A container can't see that file, so with local git
/// mutations now running as native in-container `git commit`, a clone that
/// inherited its identity only from the host's global config would die with
/// git's "Please tell me who you are" — the retired host-side commit path used
/// to paper over this via `git::identity_env`.
///
/// Write the *effective* identity the source repo resolves (`--get`, i.e.
/// local ▸ global ▸ system — exactly what the host itself would author with)
/// into the clone's own config, and fill any half the host can't resolve from
/// the signed-in profile / neutral default. This is the same fallback
/// `git::identity_env` applied, but persisted into the clone so in-container
/// git sees it. Clones are ephemeral, so freezing the value here is harmless.
async fn seed_identity(spec: &CheckoutSpec<'_>) -> Result<()> {
    let (fallback_name, fallback_email) = crate::git_dist::fallback_identity();
    for (key, fallback) in [("user.name", fallback_name), ("user.email", fallback_email)] {
        // `--get` (no `--local`) is the effective value across every scope;
        // empty or exit-1 means the host resolves nothing, so fall back.
        let effective = git::git_output(spec.source_repo, &["config", "--get", key])
            .await
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .filter(|v| !v.is_empty());
        let value = effective.unwrap_or(fallback);
        git::run_git(
            spec.dest,
            &["config", key, &value],
            &format!("config {key}"),
        )
        .await?;
    }
    Ok(())
}

/// Does `commit` resolve to a commit already present in `repo`?
async fn commit_present(repo: &Path, commit: &str) -> bool {
    let spec = format!("{commit}^{{commit}}");
    matches!(
        git::git_output(repo, &["rev-parse", "--verify", "--quiet", &spec]).await,
        Ok(out) if out.status.success()
    )
}

/// `git fetch origin <branch>` with the GitHub token auth every other network
/// op gets, bounded like push/pull so a dead connection surfaces finitely.
async fn fetch_branch(repo: &Path, branch: &str) -> Result<()> {
    let mut cmd = crate::git_dist::command(repo);
    cmd.args(["fetch", "origin", branch]);
    for (k, v) in crate::github::git_auth_env() {
        cmd.env(k, v);
    }
    let out = git::output_timed(&mut cmd, "git fetch").await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "fetch origin {branch} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// `git fetch origin <sha>` — recover a specific commit when the branch that
/// carried it is gone from the remote (auto-deleted after a PR merge). GitHub
/// serves any commit still reachable from an advertised ref — the base branch
/// it merged into, plus `refs/pull/*/head` — so a merged tip stays fetchable by
/// SHA even once its branch is deleted. Same bounded, authed shape as
/// [`fetch_branch`].
async fn fetch_commit(repo: &Path, sha: &str) -> Result<()> {
    let mut cmd = crate::git_dist::command(repo);
    cmd.args(["fetch", "origin", sha]);
    for (k, v) in crate::github::git_auth_env() {
        cmd.env(k, v);
    }
    let out = git::output_timed(&mut cmd, "git fetch").await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "fetch origin {sha} failed: {}",
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn run(repo: &Path, args: &[&str]) -> String {
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
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Init a source repo with two commits; returns (repo, first_sha, head_sha).
    fn fixture_repo(dir: &Path) -> (PathBuf, String, String) {
        let repo = dir.join("source");
        std::fs::create_dir_all(&repo).unwrap();
        run(&repo, &["init", "-q", "-b", "main"]);
        run(&repo, &["config", "user.email", "t@example.com"]);
        run(&repo, &["config", "user.name", "Tester"]);
        std::fs::write(repo.join("a.txt"), b"one").unwrap();
        run(&repo, &["add", "-A"]);
        run(&repo, &["commit", "-q", "-m", "first"]);
        let first = run(&repo, &["rev-parse", "HEAD"]);
        std::fs::write(repo.join("b.txt"), b"two").unwrap();
        run(&repo, &["add", "-A"]);
        run(&repo, &["commit", "-q", "-m", "second"]);
        let head = run(&repo, &["rev-parse", "HEAD"]);
        (repo, first, head)
    }

    #[tokio::test]
    async fn worktree_provision_detaches_at_base_and_teardown_removes() {
        let td = tempfile::tempdir().unwrap();
        let (repo, first, _head) = fixture_repo(td.path());
        let dest = td.path().join("wt");
        let spec = CheckoutSpec {
            source_repo: &repo,
            base_ref: &first,
            dest: &dest,
        };

        provision(WorkspaceMode::Worktree, &spec).await.unwrap();
        assert_eq!(run(&dest, &["rev-parse", "HEAD"]), first);
        assert_eq!(detect_mode(&dest), Some(WorkspaceMode::Worktree));

        teardown(WorkspaceMode::Worktree, &spec).await.unwrap();
        assert!(!dest.exists());
        // The registration is gone too — the same path is reusable.
        assert!(!run(&repo, &["worktree", "list", "--porcelain"]).contains("wt"));
    }

    #[tokio::test]
    async fn clone_provision_detaches_at_base_ref() {
        let td = tempfile::tempdir().unwrap();
        let (repo, first, _head) = fixture_repo(td.path());
        let dest = td.path().join("clone");
        let spec = CheckoutSpec {
            source_repo: &repo,
            base_ref: &first,
            dest: &dest,
        };

        provision(WorkspaceMode::Clone, &spec).await.unwrap();
        assert_eq!(run(&dest, &["rev-parse", "HEAD"]), first);
        // Detached HEAD: symbolic-ref exits non-zero.
        let out = std::process::Command::new("git")
            .current_dir(&dest)
            .args(["symbolic-ref", "-q", "HEAD"])
            .output()
            .unwrap();
        assert!(!out.status.success(), "clone HEAD should be detached");
        assert_eq!(detect_mode(&dest), Some(WorkspaceMode::Clone));
    }

    #[tokio::test]
    async fn clone_provision_detaches_at_sha_of_non_checked_out_branch() {
        // The spawn fork-point fallback resolves the base branch to a SHA
        // (lifecycle.rs) precisely because a name can't work here: a clone's
        // only local branch is the source's HEAD branch, so `checkout --detach
        // <other-branch-name>` trips git's remote-DWIM (an implicit `-b`) and
        // dies with "'--detach' cannot be used with '-b/-B/--orphan'". Model
        // that spawn: source checked out on a side branch, base = the SHA of
        // `main`'s tip.
        let td = tempfile::tempdir().unwrap();
        let (repo, _first, main_tip) = fixture_repo(td.path());
        run(&repo, &["checkout", "-q", "-b", "side"]);
        std::fs::write(repo.join("s.txt"), b"side").unwrap();
        run(&repo, &["add", "-A"]);
        run(&repo, &["commit", "-q", "-m", "side work"]);

        let dest = td.path().join("clone");
        let spec = CheckoutSpec {
            source_repo: &repo,
            base_ref: &main_tip,
            dest: &dest,
        };
        provision(WorkspaceMode::Clone, &spec).await.unwrap();
        assert_eq!(run(&dest, &["rev-parse", "HEAD"]), main_tip);
        // Detached, not on a DWIM-created local branch.
        let out = std::process::Command::new("git")
            .current_dir(&dest)
            .args(["symbolic-ref", "-q", "HEAD"])
            .output()
            .unwrap();
        assert!(!out.status.success(), "clone HEAD should be detached");
    }

    #[tokio::test]
    async fn clone_rewrites_origin_to_source_remote() {
        let td = tempfile::tempdir().unwrap();
        let (repo, _first, head) = fixture_repo(td.path());
        run(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/widget.git",
            ],
        );
        let dest = td.path().join("clone");
        let spec = CheckoutSpec {
            source_repo: &repo,
            base_ref: &head,
            dest: &dest,
        };

        provision(WorkspaceMode::Clone, &spec).await.unwrap();
        assert_eq!(
            run(&dest, &["remote", "get-url", "origin"]),
            "https://github.com/acme/widget.git"
        );
    }

    #[tokio::test]
    async fn clone_without_source_origin_removes_local_path_remote() {
        let td = tempfile::tempdir().unwrap();
        let (repo, _first, head) = fixture_repo(td.path());
        let dest = td.path().join("clone");
        let spec = CheckoutSpec {
            source_repo: &repo,
            base_ref: &head,
            dest: &dest,
        };

        provision(WorkspaceMode::Clone, &spec).await.unwrap();
        // The implicit local-path origin must be gone: a push from the clone
        // must never be able to write into the user's source repo.
        assert_eq!(run(&dest, &["remote"]), "");
    }

    #[tokio::test]
    async fn clone_installs_executable_delegation_hooks() {
        use std::os::unix::fs::PermissionsExt;
        let td = tempfile::tempdir().unwrap();
        let (repo, _first, head) = fixture_repo(td.path());
        let dest = td.path().join("clone");
        let spec = CheckoutSpec {
            source_repo: &repo,
            base_ref: &head,
            dest: &dest,
        };

        provision(WorkspaceMode::Clone, &spec).await.unwrap();

        for hook in ["post-commit", "post-merge"] {
            let path = dest.join(".git/hooks").join(hook);
            let body = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("{hook} missing"));
            assert!(
                body.contains("signal_git_action"),
                "{hook} must ping the signal op"
            );
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert!(
                mode & 0o111 != 0,
                "{hook} must be executable, mode={mode:o}"
            );
        }
        // post-merge always reports a base merge; post-commit distinguishes a
        // merge-completion commit (HEAD^2) from a plain one.
        let post_merge = std::fs::read_to_string(dest.join(".git/hooks/post-merge")).unwrap();
        assert!(post_merge.contains(r#"action="git_update_branch""#));
        let post_commit = std::fs::read_to_string(dest.join(".git/hooks/post-commit")).unwrap();
        assert!(
            post_commit.contains("HEAD^2"),
            "post-commit must test for a merge commit"
        );
        assert!(post_commit.contains(r#"action="git_update_branch""#));
        assert!(post_commit.contains(r#"action="git_commit""#));
    }

    #[tokio::test]
    async fn post_commit_hook_reports_merge_commit_as_update_branch() {
        // The conflicted-merge path of `update-branch`: the completing commit is
        // a merge commit (two parents), so post-commit must report a base merge
        // rather than a plain commit — otherwise the delegation couldn't tell it
        // apart from an unrelated commit.
        let td = tempfile::tempdir().unwrap();
        let (repo, _first, head) = fixture_repo(td.path());
        let dest = td.path().join("clone");
        let spec = CheckoutSpec {
            source_repo: &repo,
            base_ref: &head,
            dest: &dest,
        };
        provision(WorkspaceMode::Clone, &spec).await.unwrap();

        // Two branches that diverge on different files → a clean but non-ff
        // merge. `--no-commit` stops before committing (so post-merge does not
        // fire), leaving the merge for a manual `git commit` that post-commit
        // sees as a two-parent merge commit.
        run(&dest, &["checkout", "-q", "-b", "target"]);
        std::fs::write(dest.join("t.txt"), b"t").unwrap();
        run(&dest, &["add", "-A"]);
        run(&dest, &["commit", "-q", "-m", "target edit"]);
        run(&dest, &["checkout", "-q", "-b", "side", "HEAD~1"]);
        std::fs::write(dest.join("s.txt"), b"s").unwrap();
        run(&dest, &["add", "-A"]);
        run(&dest, &["commit", "-q", "-m", "side edit"]);
        run(&dest, &["checkout", "-q", "target"]);
        run(&dest, &["merge", "--no-ff", "--no-commit", "side"]);

        let mailbox = td.path().join("mbox");
        std::fs::create_dir_all(mailbox.join("requests")).unwrap();
        let out = std::process::Command::new("git")
            .current_dir(&dest)
            .env("FLETCH_RPC_DIR", &mailbox)
            .args(["commit", "--no-edit"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        let reqs: Vec<_> = std::fs::read_dir(mailbox.join("requests"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .collect();
        assert_eq!(reqs.len(), 1, "exactly one signal expected");
        let body = std::fs::read_to_string(reqs[0].path()).unwrap();
        assert!(
            body.contains(r#""action":"git_update_branch""#),
            "a merge-completion commit must report a base merge, body: {body}"
        );
    }

    #[tokio::test]
    async fn delegation_hook_fires_on_a_native_commit() {
        // End-to-end: a plain in-repo `git commit` runs the installed hook,
        // which writes a well-formed signal request into the mailbox dir.
        let td = tempfile::tempdir().unwrap();
        let (repo, _first, head) = fixture_repo(td.path());
        let dest = td.path().join("clone");
        let spec = CheckoutSpec {
            source_repo: &repo,
            base_ref: &head,
            dest: &dest,
        };
        provision(WorkspaceMode::Clone, &spec).await.unwrap();
        // Land on a branch so a commit is straightforward.
        run(&dest, &["checkout", "-q", "-b", "work"]);

        let mailbox = td.path().join("mbox");
        let requests = mailbox.join("requests");
        std::fs::create_dir_all(&requests).unwrap();
        std::fs::write(dest.join("c.txt"), b"change").unwrap();
        run(&dest, &["add", "-A"]);
        // The hook reads $FLETCH_RPC_DIR from the committing process's env.
        let out = std::process::Command::new("git")
            .current_dir(&dest)
            .env("FLETCH_RPC_DIR", &mailbox)
            .args(["commit", "-q", "-m", "hooked"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        let entries: Vec<_> = std::fs::read_dir(&requests)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .collect();
        assert_eq!(entries.len(), 1, "exactly one signal request expected");
        let body = std::fs::read_to_string(entries[0].path()).unwrap();
        assert!(body.contains(r#""op":"signal_git_action""#), "body: {body}");
        assert!(body.contains(r#""action":"git_commit""#), "body: {body}");
    }

    #[tokio::test]
    async fn clone_copies_repo_local_identity() {
        let td = tempfile::tempdir().unwrap();
        let (repo, _first, head) = fixture_repo(td.path());
        let dest = td.path().join("clone");
        let spec = CheckoutSpec {
            source_repo: &repo,
            base_ref: &head,
            dest: &dest,
        };

        provision(WorkspaceMode::Clone, &spec).await.unwrap();
        assert_eq!(run(&dest, &["config", "--local", "user.name"]), "Tester");
        assert_eq!(
            run(&dest, &["config", "--local", "user.email"]),
            "t@example.com"
        );
    }

    #[tokio::test]
    async fn clone_seeds_identity_when_source_resolves_none_locally() {
        let td = tempfile::tempdir().unwrap();
        let (repo, _first, head) = fixture_repo(td.path());
        // Drop the repo-local identity the fixture set: the source now resolves
        // an identity only from ambient global/system config, or nothing —
        // exactly the reviewer's "identity lives only in host global config,
        // which the container can't see" case.
        run(&repo, &["config", "--local", "--unset", "user.name"]);
        run(&repo, &["config", "--local", "--unset", "user.email"]);
        let dest = td.path().join("clone");
        let spec = CheckoutSpec {
            source_repo: &repo,
            base_ref: &head,
            dest: &dest,
        };

        provision(WorkspaceMode::Clone, &spec).await.unwrap();

        // Whatever the host's ambient config, the clone must resolve a non-empty
        // identity in its OWN config so a native in-container `git commit` never
        // dies with "Please tell me who you are".
        assert!(!run(&dest, &["config", "--local", "user.name"]).is_empty());
        assert!(!run(&dest, &["config", "--local", "user.email"]).is_empty());
    }

    #[tokio::test]
    async fn clone_borrows_objects_via_alternates_without_copying() {
        let td = tempfile::tempdir().unwrap();
        let (repo, _first, head) = fixture_repo(td.path());
        let dest = td.path().join("clone");
        let spec = CheckoutSpec {
            source_repo: &repo,
            base_ref: &head,
            dest: &dest,
        };

        provision(WorkspaceMode::Clone, &spec).await.unwrap();

        // `--shared` writes an alternates file pointing at the source's object
        // store and copies no objects.
        let alternates = dest.join(".git/objects/info/alternates");
        let contents = std::fs::read_to_string(&alternates).unwrap();
        let source_objects = repo.join(".git/objects");
        assert!(
            contents
                .lines()
                .any(|l| Path::new(l.trim()) == source_objects),
            "alternates {contents:?} should list {}",
            source_objects.display()
        );

        // No loose objects were copied into the clone (the whole point of
        // --shared): the source objects live only in the borrowed store.
        let mut loose = 0usize;
        let mut stack = vec![dest.join(".git/objects")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let entry = entry.unwrap();
                let name = entry.file_name();
                // Skip the bookkeeping dirs (`info`, `pack`); count only the
                // `xx/` fan-out dirs holding loose object files.
                if name == "info" || name == "pack" {
                    continue;
                }
                if entry.metadata().unwrap().is_dir() {
                    stack.push(entry.path());
                } else {
                    loose += 1;
                }
            }
        }
        assert_eq!(loose, 0, "--shared must not copy loose objects");

        // History is still fully reachable through the borrowed store.
        assert_eq!(run(&dest, &["rev-parse", "HEAD"]), head);
        assert!(run(&dest, &["log", "--format=%s"]).contains("first"));
    }

    #[tokio::test]
    async fn clone_teardown_removes_everything_and_is_idempotent() {
        let td = tempfile::tempdir().unwrap();
        let (repo, _first, head) = fixture_repo(td.path());
        let dest = td.path().join("clone");
        let spec = CheckoutSpec {
            source_repo: &repo,
            base_ref: &head,
            dest: &dest,
        };

        provision(WorkspaceMode::Clone, &spec).await.unwrap();
        teardown(WorkspaceMode::Clone, &spec).await.unwrap();
        assert!(!dest.exists());
        // Already gone — teardown converges rather than erroring.
        teardown(WorkspaceMode::Clone, &spec).await.unwrap();
    }

    #[tokio::test]
    async fn worktree_provision_on_branch_creates_branch_at_tip() {
        let td = tempfile::tempdir().unwrap();
        let (repo, first, _head) = fixture_repo(td.path());
        let dest = td.path().join("wt");
        let spec = CheckoutSpec {
            source_repo: &repo,
            base_ref: &first,
            dest: &dest,
        };

        provision_on_branch(
            WorkspaceMode::Worktree,
            &spec,
            "feat/restore",
            "feat/restore",
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            run(&dest, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "feat/restore"
        );
        assert_eq!(run(&dest, &["rev-parse", "HEAD"]), first);
    }

    #[tokio::test]
    async fn clone_provision_on_branch_uses_local_commit_when_present() {
        let td = tempfile::tempdir().unwrap();
        let (repo, first, _head) = fixture_repo(td.path());
        let dest = td.path().join("clone");
        let spec = CheckoutSpec {
            source_repo: &repo,
            base_ref: &first,
            dest: &dest,
        };

        provision_on_branch(WorkspaceMode::Clone, &spec, "feat/restore", "feat/restore", None)
            .await
            .unwrap();
        assert_eq!(
            run(&dest, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "feat/restore"
        );
        assert_eq!(run(&dest, &["rev-parse", "HEAD"]), first);
    }

    #[tokio::test]
    async fn clone_provision_on_branch_fetches_missing_tip_from_origin() {
        // The agent's commits lived only in its clone (torn down at archive)
        // and on the real remote. Model that: `origin` (bare) holds a `feat`
        // branch the source repo never fetched.
        let td = tempfile::tempdir().unwrap();
        let (repo, _first, _head) = fixture_repo(td.path());
        let origin = td.path().join("origin.git");
        run(
            td.path(),
            &["init", "-q", "--bare", origin.to_str().unwrap()],
        );
        run(
            &repo,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        run(&repo, &["push", "-q", "origin", "main"]);

        // A second worker pushes `feat` commits the source repo never sees.
        let worker = td.path().join("worker");
        run(
            td.path(),
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                worker.to_str().unwrap(),
            ],
        );
        run(&worker, &["config", "user.email", "w@example.com"]);
        run(&worker, &["config", "user.name", "Worker"]);
        run(&worker, &["checkout", "-q", "-b", "feat"]);
        std::fs::write(worker.join("feat.txt"), b"feature").unwrap();
        run(&worker, &["add", "-A"]);
        run(&worker, &["commit", "-q", "-m", "feat work"]);
        run(&worker, &["push", "-q", "origin", "feat"]);
        let tip = run(&worker, &["rev-parse", "HEAD"]);

        let dest = td.path().join("clone");
        let spec = CheckoutSpec {
            source_repo: &repo,
            base_ref: &tip,
            dest: &dest,
        };
        provision_on_branch(WorkspaceMode::Clone, &spec, "feat", "feat", None)
            .await
            .unwrap();
        assert_eq!(run(&dest, &["rev-parse", "--abbrev-ref", "HEAD"]), "feat");
        assert_eq!(run(&dest, &["rev-parse", "HEAD"]), tip);

        // Restore under a collision-renamed local branch: the fetch must still
        // target the name the remote knows (`feat`), while the local branch is
        // created under the renamed one.
        let dest2 = td.path().join("clone2");
        let spec2 = CheckoutSpec {
            source_repo: &repo,
            base_ref: &tip,
            dest: &dest2,
        };
        provision_on_branch(WorkspaceMode::Clone, &spec2, "feat-restored", "feat", None)
            .await
            .unwrap();
        assert_eq!(
            run(&dest2, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "feat-restored"
        );
        assert_eq!(run(&dest2, &["rev-parse", "HEAD"]), tip);
    }

    #[tokio::test]
    async fn provision_self_contained_copies_objects_without_alternates() {
        // A repo added to a live Docker agent must not borrow via alternates
        // (the container can't mount the borrowed store): it gets a full,
        // self-contained object copy instead.
        let td = tempfile::tempdir().unwrap();
        let (repo, _first, head) = fixture_repo(td.path());
        let dest = td.path().join("clone");
        let spec = CheckoutSpec {
            source_repo: &repo,
            base_ref: &head,
            dest: &dest,
        };

        provision_self_contained(&spec).await.unwrap();

        // No alternates file — objects live in the clone itself.
        assert!(!dest.join(".git/objects/info/alternates").exists());
        // Objects were actually copied in (loose objects present).
        let mut loose = 0usize;
        let mut stack = vec![dest.join(".git/objects")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let entry = entry.unwrap();
                let name = entry.file_name();
                if name == "info" || name == "pack" {
                    continue;
                }
                if entry.metadata().unwrap().is_dir() {
                    stack.push(entry.path());
                } else {
                    loose += 1;
                }
            }
        }
        assert!(loose > 0, "self-contained clone must copy objects in");
        assert_eq!(run(&dest, &["rev-parse", "HEAD"]), head);
        assert_eq!(detect_mode(&dest), Some(WorkspaceMode::Clone));
    }

    #[tokio::test]
    async fn clone_provision_on_branch_restores_unreachable_source_tip_offline() {
        // Regression guard for the seatbelt Clone default. A pre-existing agent
        // archived under the old Worktree default made its commits in the
        // *source* repo's object store; archive teardown deleted the checkout
        // branch, leaving the tip present but unreachable. Restoring it under
        // Clone mode must recover it offline via alternates — no origin, no
        // fetch — so flipping the default doesn't regress legacy archives.
        let td = tempfile::tempdir().unwrap();
        let (repo, _first, _head) = fixture_repo(td.path());

        // Simulate the archived checkout agent's commit landing in the source
        // store, then teardown deleting the branch (tip now unreachable).
        run(&repo, &["checkout", "-q", "-b", "feat"]);
        std::fs::write(repo.join("feat.txt"), b"agent work").unwrap();
        run(&repo, &["add", "-A"]);
        run(&repo, &["commit", "-q", "-m", "agent work"]);
        let tip = run(&repo, &["rev-parse", "HEAD"]);
        run(&repo, &["checkout", "-q", "main"]);
        run(&repo, &["branch", "-q", "-D", "feat"]);
        // The object is unreachable but still present in the source store.
        assert_eq!(run(&repo, &["rev-parse", "--verify", &tip]), tip);
        // No origin: restore must not need the network.
        assert_eq!(run(&repo, &["remote"]), "");

        let dest = td.path().join("clone");
        let spec = CheckoutSpec {
            source_repo: &repo,
            base_ref: &tip,
            dest: &dest,
        };
        provision_on_branch(WorkspaceMode::Clone, &spec, "feat", "feat", None)
            .await
            .unwrap();
        assert_eq!(run(&dest, &["rev-parse", "--abbrev-ref", "HEAD"]), "feat");
        assert_eq!(run(&dest, &["rev-parse", "HEAD"]), tip);
        // The recovered tip's tree is intact (borrowed via alternates).
        assert!(dest.join("feat.txt").exists());
    }

    #[tokio::test]
    async fn clone_provision_on_branch_recovers_merged_deleted_branch_detached() {
        // The reported case: a branch auto-deleted after its PR merged. The
        // branch ref is gone from origin, so `fetch origin <branch>` fails — but
        // the tip commit still lives on the base branch it merged into, so it is
        // fetchable by SHA and the workspace opens detached at it.
        let td = tempfile::tempdir().unwrap();
        let (repo, _first, _head) = fixture_repo(td.path());
        let origin = td.path().join("origin.git");
        run(td.path(), &["init", "-q", "--bare", origin.to_str().unwrap()]);
        // GitHub serves any commit reachable from an advertised ref even with no
        // branch pointing at it; model that so a merged-then-deleted tip is
        // fetchable by SHA (the default local `upload-pack` refuses otherwise).
        run(&origin, &["config", "uploadpack.allowReachableSHA1InWant", "true"]);
        run(&repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
        run(&repo, &["push", "-q", "origin", "main"]);

        // A worker branches `feat`, commits, merges it back into `main` on the
        // remote, then deletes the branch (PR auto-delete after merge).
        let worker = td.path().join("worker");
        run(
            td.path(),
            &["clone", "-q", origin.to_str().unwrap(), worker.to_str().unwrap()],
        );
        run(&worker, &["config", "user.email", "w@example.com"]);
        run(&worker, &["config", "user.name", "Worker"]);
        run(&worker, &["checkout", "-q", "-b", "feat"]);
        std::fs::write(worker.join("feat.txt"), b"feature").unwrap();
        run(&worker, &["add", "-A"]);
        run(&worker, &["commit", "-q", "-m", "feat work"]);
        let tip = run(&worker, &["rev-parse", "HEAD"]);
        run(&worker, &["push", "-q", "origin", "feat"]);
        run(&worker, &["checkout", "-q", "main"]);
        run(&worker, &["merge", "-q", "--no-ff", "-m", "merge feat", "feat"]);
        run(&worker, &["push", "-q", "origin", "main"]);
        run(&worker, &["push", "-q", "origin", "--delete", "feat"]);

        let dest = td.path().join("clone");
        let spec = CheckoutSpec {
            source_repo: &repo,
            base_ref: &tip,
            dest: &dest,
        };
        let on_branch = provision_on_branch(WorkspaceMode::Clone, &spec, "feat", "feat", None)
            .await
            .unwrap();
        assert!(!on_branch, "a deleted branch must restore detached");
        assert_eq!(run(&dest, &["rev-parse", "--abbrev-ref", "HEAD"]), "HEAD");
        assert_eq!(run(&dest, &["rev-parse", "HEAD"]), tip);
        assert!(dest.join("feat.txt").exists());
    }

    #[tokio::test]
    async fn clone_provision_on_branch_falls_back_to_parent_base_when_tip_lost() {
        // Worst case: the tip is gone for good (branch deleted without merging,
        // so it is unreachable on origin and unfetchable by SHA). Restore must
        // still open — detached at the parent base — so the session history and
        // PR link survive even though the agent's own commits cannot.
        let td = tempfile::tempdir().unwrap();
        let (repo, _first, head) = fixture_repo(td.path());
        let origin = td.path().join("origin.git");
        run(td.path(), &["init", "-q", "--bare", origin.to_str().unwrap()]);
        run(&origin, &["config", "uploadpack.allowReachableSHA1InWant", "true"]);
        run(&repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
        run(&repo, &["push", "-q", "origin", "main"]);

        let worker = td.path().join("worker");
        run(
            td.path(),
            &["clone", "-q", origin.to_str().unwrap(), worker.to_str().unwrap()],
        );
        run(&worker, &["config", "user.email", "w@example.com"]);
        run(&worker, &["config", "user.name", "Worker"]);
        run(&worker, &["checkout", "-q", "-b", "feat"]);
        std::fs::write(worker.join("feat.txt"), b"feature").unwrap();
        run(&worker, &["add", "-A"]);
        run(&worker, &["commit", "-q", "-m", "feat work"]);
        let tip = run(&worker, &["rev-parse", "HEAD"]);
        run(&worker, &["push", "-q", "origin", "feat"]);
        // Deleted without ever merging, then pruned → the tip is gone from
        // origin entirely and cannot be fetched by branch or by SHA.
        run(&worker, &["push", "-q", "origin", "--delete", "feat"]);
        run(&origin, &["gc", "--prune=now", "-q"]);

        let dest = td.path().join("clone");
        let spec = CheckoutSpec {
            source_repo: &repo,
            base_ref: &tip,
            dest: &dest,
        };
        // `head` (the parent base) is present in the source store → reachable.
        let on_branch =
            provision_on_branch(WorkspaceMode::Clone, &spec, "feat", "feat", Some(&head))
                .await
                .unwrap();
        assert!(!on_branch);
        assert_eq!(run(&dest, &["rev-parse", "--abbrev-ref", "HEAD"]), "HEAD");
        assert_eq!(run(&dest, &["rev-parse", "HEAD"]), head);
    }

    #[tokio::test]
    async fn clone_recovers_orphan_dir_at_target() {
        let td = tempfile::tempdir().unwrap();
        let (repo, _first, head) = fixture_repo(td.path());
        let dest = td.path().join("clone");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("leftover.txt"), b"stale").unwrap();
        let spec = CheckoutSpec {
            source_repo: &repo,
            base_ref: &head,
            dest: &dest,
        };

        provision(WorkspaceMode::Clone, &spec).await.unwrap();
        assert!(!dest.join("leftover.txt").exists());
        assert!(dest.join("a.txt").exists());
    }

    #[test]
    fn detect_mode_on_missing_path_is_none() {
        assert_eq!(detect_mode(Path::new("/nonexistent/nope")), None);
    }
}
