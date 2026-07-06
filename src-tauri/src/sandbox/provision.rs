//! Workspace provisioning: how an agent's checkout comes into existence.
//!
//! Two modes. `Worktree` is the historical behavior — a linked `git worktree`
//! whose `.git` file points back into the origin repo. `Clone` is a
//! self-contained checkout with its own real, writable `.git`, required by the
//! Docker engine: a linked worktree's `.git` file references the origin repo's
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
//! The mode is selected by the `workspace_mode` settings key (dev flag, not
//! exposed in UI — set via sqlite for testing) so the clone path can be
//! exercised under seatbelt before the Docker engine exists.

use std::path::Path;

use crate::error::{Error, Result};
use crate::git;

/// Settings-table key selecting the provisioning mode: `"worktree"` (default)
/// or `"clone"`. Read at spawn time; slice B2 forces `Clone` whenever the
/// engine is Docker regardless of this key.
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

impl WorkspaceMode {
    /// Parse the `workspace_mode` setting. Anything other than an explicit
    /// `"clone"` — including absent or unrecognized values — falls back to
    /// `Worktree`, the historical default, so a typo'd dev flag can't change
    /// behavior silently on top of a warning.
    pub fn from_setting(value: Option<&str>) -> Self {
        match value {
            Some("clone") => WorkspaceMode::Clone,
            Some("worktree") | None => WorkspaceMode::Worktree,
            Some(other) => {
                tracing::warn!(value = %other, "unrecognized workspace_mode setting; using worktree");
                WorkspaceMode::Worktree
            }
        }
    }
}

/// What to check out where. `base_ref` is any commit-ish; pass `"HEAD"` for
/// "the source repo's current HEAD" (the legacy no-base behavior).
pub struct CheckoutSpec<'a> {
    /// The user's real repo root.
    pub source_repo: &'a Path,
    /// Commit-ish the workspace starts from, checked out detached.
    pub base_ref: &'a str,
    /// Workspace path (`workspace::repo_worktree_path(agent_id, subdir)`).
    pub dest: &'a Path,
}

/// Create the workspace at `spec.dest`, detached at `spec.base_ref`.
pub async fn provision(mode: WorkspaceMode, spec: &CheckoutSpec<'_>) -> Result<()> {
    match mode {
        WorkspaceMode::Worktree => {
            git::worktree_add_detached(spec.source_repo, spec.dest, Some(spec.base_ref)).await
        }
        WorkspaceMode::Clone => {
            clone_base(spec).await?;
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
    }
}

/// Create the workspace checked out on a new local branch `branch` pointing at
/// `spec.base_ref`. Restore path — the counterpart of [`provision`] for agents
/// that had already materialized a branch.
///
/// Worktree: the branch is created in the source repo (worktree branches are
/// refs of the origin repo) and the worktree attached to it. Clone: the branch
/// is created inside the clone; when `base_ref` isn't present in the source
/// repo (the agent's commits lived only in the torn-down clone), it is fetched
/// from `origin` via `origin_branch` first — which is why restore of a clone
/// workspace requires the branch to have been pushed.
///
/// `origin_branch` is the branch name as the remote knows it. It differs from
/// `branch` when restore renamed to dodge a local collision (`feat` →
/// `feat-restored`): the remote only has the original name, so fetching the
/// renamed one would fail and make a pushed branch unrestorable.
pub async fn provision_on_branch(
    mode: WorkspaceMode,
    spec: &CheckoutSpec<'_>,
    branch: &str,
    origin_branch: &str,
) -> Result<()> {
    match mode {
        WorkspaceMode::Worktree => {
            git::branch_create_at(spec.source_repo, branch, spec.base_ref).await?;
            git::worktree_add_branch(spec.source_repo, spec.dest, branch).await
        }
        WorkspaceMode::Clone => {
            clone_base(spec).await?;
            let branch = branch.to_string();
            let origin_branch = origin_branch.to_string();
            finish_clone(spec, |dest| async move {
                if !commit_present(&dest, spec.base_ref).await {
                    fetch_branch(&dest, &origin_branch).await?;
                }
                git::run_git(
                    &dest,
                    &["checkout", "-b", &branch, spec.base_ref],
                    &format!("checkout -b {branch}"),
                )
                .await?;
                Ok(())
            })
            .await
        }
    }
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

/// `git clone --shared` + origin rewrite + repo-local identity copy — the
/// parts shared by both clone-arm entry points. Leaves HEAD wherever the clone
/// put it; callers do their own checkout.
///
/// `--shared` borrows the source's object store via
/// `.git/objects/info/alternates` (an absolute path to the source's objects)
/// and copies no objects, so the clone is kilobytes on disk. The source object
/// store must stay present for the clone's lifetime (Fletch owns the source
/// repo lifecycle). Borrowed objects are only ever *referenced* — never written
/// — and for Docker the store is mounted read-only, so a container can't reach
/// through the alternates link to mutate the source.
///
/// Caveat — gc/prune on the source: `--shared` breaks only if the source
/// prunes an object the clone references. Base history stays reachable from the
/// source's own refs, so ordinary `git gc --auto` (prunes only unreachable
/// objects past the grace period) is safe. The real risk is an aggressive
/// `git prune` / `gc --prune=now` on the source *while an agent is live and
/// referencing a since-deleted base branch*. Not hardened against here (the
/// common case is safe); if that becomes a problem, disable gc on the source
/// while any agent is active (crash-safe, via transient env-config).
async fn clone_base(spec: &CheckoutSpec<'_>) -> Result<()> {
    let source = path_str(spec.source_repo)?;
    let dest = path_str(spec.dest)?;

    // A leftover directory at the target can only be an orphan from a crashed
    // spawn: agent-id allocation refuses ids whose worktree dir physically
    // exists (`occupied_worktree_dirs`), so no live workspace can be here.
    // Clear it rather than letting `git clone` fail on a non-empty dir.
    if spec.dest.exists() {
        tracing::warn!(path = %spec.dest.display(), "clearing orphan dir at clone target");
        tokio::fs::remove_dir_all(spec.dest).await?;
    }

    // `--shared` borrows the source's objects via an alternates file and
    // copies none, so a spawn is cheap regardless of repo size. Borrowed
    // objects are read-only (referenced, and RO-mounted for Docker), so a
    // container can't corrupt the source through them.
    //
    // No timeout: `--shared` is near-instant, but keep the unbounded shape
    // (and `kill_on_drop`) consistent with `new_project::clone`; the child is
    // still reaped if the spawn task is aborted.
    let out = crate::git_dist::command(spec.source_repo)
        .args(["clone", "--shared", &source, &dest])
        .kill_on_drop(true)
        .output()
        .await?;
    if !out.status.success() {
        // Self-heal: a partial clone dir would make every retry fail with
        // "already exists" (mirrors `new_project::clone`).
        let _ = tokio::fs::remove_dir_all(spec.dest).await;
        return Err(Error::Git(format!(
            "clone --shared failed: {}",
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
async fn finish_clone<F, Fut>(spec: &CheckoutSpec<'_>, checkout: F) -> Result<()>
where
    F: FnOnce(std::path::PathBuf) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let result = async {
        rewrite_origin(spec).await?;
        copy_local_identity(spec).await?;
        checkout(spec.dest.to_path_buf()).await
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_dir_all(spec.dest).await;
    }
    result
}

/// Point the clone's `origin` at the source repo's real remote so push/PR/
/// fetch behave exactly as they would from a worktree. When the source has no
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

/// Copy repo-local `user.name` / `user.email` into the clone. Global
/// gitconfig already applies host-side; only per-repo identity would
/// otherwise be lost (clones don't inherit the source's local config).
async fn copy_local_identity(spec: &CheckoutSpec<'_>) -> Result<()> {
    for key in ["user.name", "user.email"] {
        // Exits 1 when unset — not an error, just nothing to copy.
        let out = git::git_output(spec.source_repo, &["config", "--local", "--get", key]).await?;
        if !out.status.success() {
            continue;
        }
        let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if value.is_empty() {
            continue;
        }
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

    #[test]
    fn mode_parses_setting_with_worktree_default() {
        assert_eq!(WorkspaceMode::from_setting(None), WorkspaceMode::Worktree);
        assert_eq!(
            WorkspaceMode::from_setting(Some("worktree")),
            WorkspaceMode::Worktree
        );
        assert_eq!(
            WorkspaceMode::from_setting(Some("clone")),
            WorkspaceMode::Clone
        );
        assert_eq!(
            WorkspaceMode::from_setting(Some("docker?!")),
            WorkspaceMode::Worktree
        );
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

        provision_on_branch(WorkspaceMode::Clone, &spec, "feat/restore", "feat/restore")
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
        provision_on_branch(WorkspaceMode::Clone, &spec, "feat", "feat")
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
        provision_on_branch(WorkspaceMode::Clone, &spec2, "feat-restored", "feat")
            .await
            .unwrap();
        assert_eq!(
            run(&dest2, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "feat-restored"
        );
        assert_eq!(run(&dest2, &["rev-parse", "HEAD"]), tip);
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
