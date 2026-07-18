//! Repo/worktree lifecycle: init, commit-all, fork snapshot/carry, worktree
//! removal/prune, and the unborn-HEAD seed used before forking a checkout.

use std::path::Path;

use crate::error::{Error, Result};

use super::branch::rev_parse;
use super::cmd::{git_output_env, identity_env, merge_git_env, no_hooks_env, run_git, run_git_env};

/// `git init` a fresh repository at `path` (created if absent). Used by the
/// New Project "create" flow before seeding an initial commit.
pub async fn init_repo(path: &Path) -> Result<()> {
    // No `current_dir` — the target may not exist yet (`git init` creates it),
    // and spawning with a missing cwd fails before git ever runs.
    let out = crate::git_dist::bare_command()
        .args([
            "init",
            path.to_str()
                .ok_or_else(|| Error::InvalidPath(path.display().to_string()))?,
        ])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "init failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Stage everything and create a commit in `repo`. Uses the user's git
/// identity when configured; otherwise falls back to the signed-in profile
/// (see `identity_env`) so a machine with no `.gitconfig` can still commit.
pub async fn commit_all(repo: &Path, message: &str) -> Result<()> {
    run_git(repo, &["add", "-A"], "add -A").await?;

    let env = merge_git_env(&[&identity_env(repo).await, &no_hooks_env()]);
    let out = git_output_env(repo, &["commit", "-m", message], &env).await?;
    if !out.status.success() {
        // `git commit` writes the common "nothing to commit, working tree
        // clean" diagnostic to *stdout*, not stderr — so report both, else a
        // clean-tree failure surfaces as an empty, undebuggable message.
        let detail = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        return Err(Error::Git(format!("commit failed: {}", detail.trim())));
    }
    Ok(())
}

/// Capture `checkout`'s current working tree — tracked modifications plus
/// untracked, non-ignored files, and deletions — into a commit object WITHOUT
/// touching the checkout's real index, HEAD, or working tree, and return its
/// sha. Used by fork "carry code": the snapshot is created in the *source*
/// checkout's object store (so a live agent is left undisturbed) and later
/// fetched into the fork by [`carry_worktree`]. The snapshot's parent is the
/// source's HEAD, so it records the full state (committed + uncommitted).
pub async fn snapshot_worktree(checkout: &Path) -> Result<String> {
    // A throwaway index so `add -A` never stages into the live agent's index.
    let tmp = tempfile::Builder::new()
        .prefix("fletch-fork-index-")
        .tempfile()
        .map_err(Error::from)?;
    let index_env = vec![(
        "GIT_INDEX_FILE".to_string(),
        tmp.path().display().to_string(),
    )];

    // Seed the temp index from HEAD, then stage every working-tree change
    // (adds/mods/dels, honoring .gitignore) into it — the snapshot tree.
    let stage_env = merge_git_env(&[&index_env, &no_hooks_env()]);
    run_git_env(
        checkout,
        &["read-tree", "HEAD"],
        &stage_env,
        "fork snapshot read-tree",
    )
    .await?;
    run_git_env(checkout, &["add", "-A"], &stage_env, "fork snapshot add").await?;
    let tree = run_git_env(
        checkout,
        &["write-tree"],
        &stage_env,
        "fork snapshot write-tree",
    )
    .await?;
    let tree = String::from_utf8_lossy(&tree.stdout).trim().to_string();

    // commit-tree writes no hooks but needs an identity, same fallback as commit.
    let commit_env = merge_git_env(&[&index_env, &identity_env(checkout).await, &no_hooks_env()]);
    let commit = run_git_env(
        checkout,
        &[
            "commit-tree",
            &tree,
            "-p",
            "HEAD",
            "-m",
            "fletch: fork snapshot",
        ],
        &commit_env,
        "fork snapshot commit-tree",
    )
    .await?;
    Ok(String::from_utf8_lossy(&commit.stdout).trim().to_string())
}

/// Point `dest`'s working tree at `snapshot` (fetched from the `source`
/// checkout) while keeping `dest`'s HEAD on `base` — so the carried work shows
/// as uncommitted changes against the fork's base, mirroring the parent's own
/// diff. Used by fork "carry code" after `dest` is provisioned clean at `base`.
pub async fn carry_worktree(dest: &Path, source: &Path, snapshot: &str, base: &str) -> Result<()> {
    let source_str = source
        .to_str()
        .ok_or_else(|| Error::InvalidPath(source.display().to_string()))?;
    // Bring the snapshot commit + its reachable objects into the fork's store.
    run_git(
        dest,
        &["fetch", "--no-tags", source_str, snapshot],
        "carry fetch",
    )
    .await?;
    // Materialize the snapshot exactly (adds/mods/dels), then move HEAD + index
    // back to base, leaving the working tree — so the delta reads as unstaged
    // working-tree changes, exactly like the parent's uncommitted state.
    let env = no_hooks_env();
    run_git_env(
        dest,
        &["reset", "--hard", snapshot],
        &env,
        "carry reset --hard",
    )
    .await?;
    run_git_env(
        dest,
        &["reset", "--mixed", base],
        &env,
        "carry reset --mixed",
    )
    .await?;
    Ok(())
}

pub async fn worktree_remove(repo: &Path, worktree_path: &Path, force: bool) -> Result<()> {
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    let path_str = worktree_path
        .to_str()
        .ok_or_else(|| Error::InvalidPath(worktree_path.display().to_string()))?;
    args.push(path_str);
    run_git(repo, &args, "worktree remove").await?;
    Ok(())
}

/// Drop any internal `.git/worktrees/<id>` refs whose linked working tree
/// no longer exists. Safe to run unconditionally — git just no-ops when
/// there's nothing to prune.
pub async fn worktree_prune(repo: &Path) -> Result<()> {
    run_git(repo, &["worktree", "prune"], "worktree prune").await?;
    Ok(())
}

/// Stage everything and create the repo's first commit. `--allow-empty` so an
/// empty folder still gets a HEAD — worktrees can't fork without one. Uses the
/// identity fallback like every other commit-creating op.
pub async fn commit_initial(repo: &Path) -> Result<()> {
    run_git(repo, &["add", "-A"], "add -A").await?;
    let env = merge_git_env(&[&identity_env(repo).await, &no_hooks_env()]);
    let out = git_output_env(
        repo,
        &["commit", "--allow-empty", "-m", "Initial commit"],
        &env,
    )
    .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "initial commit failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Process-wide lock serializing the unborn-HEAD seed in `ensure_head_commit`.
/// Contended only when a repo has no commits yet and two spawns race to seed it;
/// the common HEAD-already-exists path never acquires it (see the double-check).
static HEAD_SEED_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Guarantee `repo` has a resolvable `HEAD` so a workspace can fork from it.
/// A repo that's been `git init`'d but never committed has an unborn HEAD, and
/// neither `git clone --shared` (the default workspace mode) nor `worktree add`
/// can fork a repo without one — the fork fails and the agent never launches.
/// Seed the repo's first commit in that case; no-op when HEAD already resolves.
/// Idempotent, so it's safe to call on every provisioning.
///
/// Concurrency: the check (`rev_parse`) and the act (`commit_initial`) are
/// guarded by a process-wide lock, with a double-check inside it. Without the
/// lock two overlapping spawns against the same commit-less repo could both pass
/// the check and each seed — leaving a stray empty "Initial commit", or failing
/// one spawn on `.git/index.lock` contention. The lock is taken only once HEAD
/// is confirmed unborn, so the common case stays lock-free. This serializes
/// within one Fletch process; two separate processes seeding the same brand-new
/// repo at the same instant still fall back to git's own index locking — a
/// vanishingly small, self-limiting window that closes the moment a commit lands.
pub async fn ensure_head_commit(repo: &Path) -> Result<()> {
    if rev_parse(repo, "HEAD").await.is_ok() {
        return Ok(());
    }
    // Unborn HEAD — serialize the seed so racing spawns don't double-commit.
    let _guard = HEAD_SEED_LOCK.lock().await;
    // A racer may have seeded HEAD while we waited for the lock; re-check.
    if rev_parse(repo, "HEAD").await.is_ok() {
        return Ok(());
    }
    commit_initial(repo).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::process::Command;

    async fn config(repo: &Path, key: &str, val: &str) {
        let out = Command::new("git")
            .current_dir(repo)
            .args(["config", key, val])
            .output()
            .await
            .unwrap();
        assert!(out.status.success());
    }

    /// Write an executable `.git/hooks/<name>` that drops `sentinel` and fails.
    /// If git ran it, the sentinel would exist (and, for pre-* hooks, the op
    /// would be aborted). Unix-only: hooks must be executable to run.
    #[cfg(unix)]
    fn write_failing_hook(repo: &Path, name: &str, sentinel: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let hooks = repo.join(".git/hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        let path = hooks.join(name);
        std::fs::write(
            &path,
            format!("#!/bin/sh\ntouch '{}'\nexit 1\n", sentinel.display()),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn commit_all_ignores_workspace_pre_commit_hook() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path();
        init_repo(repo).await.unwrap();
        config(repo, "user.email", "t@example.com").await;
        config(repo, "user.name", "Tester").await;

        let sentinel = td.path().join("hook-ran");
        write_failing_hook(repo, "pre-commit", &sentinel);

        std::fs::write(repo.join("a.txt"), b"x").unwrap();
        // With hooks honored, the `exit 1` pre-commit would abort this commit.
        commit_all(repo, "first").await.unwrap();

        assert!(!sentinel.exists(), "workspace pre-commit hook must not run");
        // And the commit actually landed.
        let log = run_git(repo, &["log", "--oneline"], "log").await.unwrap();
        assert!(String::from_utf8_lossy(&log.stdout).contains("first"));
    }

    #[tokio::test]
    async fn snapshot_and_carry_reproduce_working_tree_at_base() {
        // Parent checkout: a base commit (with a .gitignore), then uncommitted
        // work covering every case — modify, delete, add, and an ignored file.
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("src");
        std::fs::create_dir(&src).unwrap();
        init_repo(&src).await.unwrap();
        config(&src, "user.email", "t@example.com").await;
        config(&src, "user.name", "Tester").await;
        std::fs::write(src.join(".gitignore"), b"ignored.txt\n").unwrap();
        std::fs::write(src.join("keep.txt"), b"base").unwrap();
        std::fs::write(src.join("drop.txt"), b"remove me").unwrap();
        commit_all(&src, "base").await.unwrap();
        let base = rev_parse(&src, "HEAD").await.unwrap();

        std::fs::write(src.join("keep.txt"), b"modified").unwrap();
        std::fs::remove_file(src.join("drop.txt")).unwrap();
        std::fs::write(src.join("new.txt"), b"added").unwrap();
        std::fs::write(src.join("ignored.txt"), b"secret").unwrap();

        let snap = snapshot_worktree(&src).await.unwrap();
        // No side effects on the live checkout: HEAD is untouched.
        assert_eq!(rev_parse(&src, "HEAD").await.unwrap(), base);

        // Fork checkout: a clone sitting at base, like a freshly-provisioned one.
        let dst = td.path().join("dst");
        let clone = Command::new("git")
            .args(["clone", "-q", src.to_str().unwrap(), dst.to_str().unwrap()])
            .output()
            .await
            .unwrap();
        assert!(clone.status.success());

        carry_worktree(&dst, &src, &snap, &base).await.unwrap();

        // Working tree now mirrors the parent's: mod/add applied, deletion gone,
        // ignored file never carried.
        assert_eq!(std::fs::read(dst.join("keep.txt")).unwrap(), b"modified");
        assert_eq!(std::fs::read(dst.join("new.txt")).unwrap(), b"added");
        assert!(!dst.join("drop.txt").exists());
        assert!(!dst.join("ignored.txt").exists());
        // HEAD stays at base — the carried work reads as uncommitted changes.
        assert_eq!(rev_parse(&dst, "HEAD").await.unwrap(), base);
        let status = run_git(&dst, &["status", "--porcelain"], "status")
            .await
            .unwrap();
        assert!(
            !String::from_utf8_lossy(&status.stdout).trim().is_empty(),
            "carried changes should show as uncommitted"
        );
    }

    #[tokio::test]
    async fn commit_all_clean_tree_reports_nothing_to_commit() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path();
        init_repo(repo).await.unwrap();
        config(repo, "user.email", "t@example.com").await;
        config(repo, "user.name", "Tester").await;

        std::fs::write(repo.join("a.txt"), b"x").unwrap();
        commit_all(repo, "first").await.unwrap();

        // Tree is now clean: git writes "nothing to commit" to stdout and exits
        // non-zero. The error must surface that, not an empty string.
        let err = commit_all(repo, "second").await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("nothing to commit"), "got: {msg}");
    }

    #[tokio::test]
    async fn ensure_head_commit_seeds_unborn_head_and_is_idempotent() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path();
        init_repo(repo).await.unwrap();
        config(repo, "user.email", "t@example.com").await;
        config(repo, "user.name", "Tester").await;

        // Unborn HEAD: no commit yet.
        assert!(rev_parse(repo, "HEAD").await.is_err());

        ensure_head_commit(repo).await.unwrap();
        let first = rev_parse(repo, "HEAD").await.unwrap();

        // No-op on a repo that already has a HEAD — same commit, no error.
        ensure_head_commit(repo).await.unwrap();
        assert_eq!(first, rev_parse(repo, "HEAD").await.unwrap());
    }

    /// Overlapping spawns against the same commit-less repo must seed exactly one
    /// commit and none may fail. Without the internal lock the check-then-act
    /// race lets multiple callers each commit (a stray empty "Initial commit") or
    /// fail one on `.git/index.lock` contention.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ensure_head_commit_concurrent_seeds_exactly_one_commit() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().to_path_buf();
        init_repo(&repo).await.unwrap();
        config(&repo, "user.email", "t@example.com").await;
        config(&repo, "user.name", "Tester").await;
        assert!(rev_parse(&repo, "HEAD").await.is_err());

        // Fire many concurrent calls at the unborn-HEAD repo at once.
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..8 {
            let r = repo.clone();
            set.spawn(async move { ensure_head_commit(&r).await });
        }
        while let Some(joined) = set.join_next().await {
            // No task panicked, and no call returned Err (no lock-contention
            // failure surfaced to a spawn).
            joined.unwrap().unwrap();
        }

        // Exactly one commit — no duplicate from a lost race.
        let out = run_git(&repo, &["rev-list", "--count", "HEAD"], "rev-list")
            .await
            .unwrap();
        let count = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert_eq!(count, "1", "expected exactly one commit, got {count}");
    }
}
