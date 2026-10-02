//! Repo/worktree lifecycle: init, commit-all, the working-tree snapshot behind
//! checkpoints, worktree removal/prune, and the unborn-HEAD seed used before
//! forking a checkout.

use std::path::Path;

use crate::error::{Error, Result};

use super::branch::rev_parse;
use super::cmd::{git_output_env, identity_env, merge_git_env, run_git, run_git_env};

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

    let env = identity_env(repo).await;
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
/// sha. The snapshot is created in the checkout's own object store (so a live
/// agent is left undisturbed) and its parent is the checkout's HEAD, so it
/// records the full state (committed + uncommitted). What `git::checkpoint`
/// pins; [`apply_snapshot`] is its inverse.
pub async fn snapshot_worktree(checkout: &Path) -> Result<String> {
    // A throwaway index so `add -A` never stages into the live agent's index.
    let tmp = tempfile::Builder::new()
        .prefix("fletch-snapshot-index-")
        .tempfile()
        .map_err(Error::from)?;
    let index_env = vec![(
        "GIT_INDEX_FILE".to_string(),
        tmp.path().display().to_string(),
    )];

    // Seed the temp index, then stage every working-tree change (adds/mods/
    // dels, honoring .gitignore) into it — the snapshot tree. `read-tree`
    // replaces whatever the copy attempt left in the temp index.
    if !seed_index_from_live(checkout, tmp.path(), &index_env).await {
        run_git_env(
            checkout,
            &["read-tree", "HEAD"],
            &index_env,
            "snapshot read-tree",
        )
        .await?;
    }
    run_git_env(checkout, &["add", "-A"], &index_env, "snapshot add").await?;
    let tree = run_git_env(checkout, &["write-tree"], &index_env, "snapshot write-tree").await?;
    let tree = String::from_utf8_lossy(&tree.stdout).trim().to_string();

    // commit-tree writes no hooks but needs an identity, same fallback as commit.
    let commit_env = merge_git_env(&[&index_env, &identity_env(checkout).await]);
    let commit = run_git_env(
        checkout,
        &[
            "commit-tree",
            &tree,
            "-p",
            "HEAD",
            "-m",
            "fletch: worktree snapshot",
        ],
        &commit_env,
        "snapshot commit-tree",
    )
    .await?;
    Ok(String::from_utf8_lossy(&commit.stdout).trim().to_string())
}

/// Seed the snapshot's temp index with a copy of the checkout's real index, so
/// `add -A` re-hashes only the files whose stat data changed. Seeding from
/// `read-tree HEAD` instead writes zeroed stat data, which re-hashes every
/// tracked file — fine once per fork, too slow on every turn of a big repo.
///
/// Both seeds yield the same tree: `add -A` converges the index on the working
/// tree wherever it starts, and git's racy-entry checks catch stale stat data.
/// The exception is an entry `add -A` never compares with the working tree —
/// skip-worktree (sparse checkouts) or assume-unchanged — so a copy carrying
/// either is not used. Returns whether the copy seeded the index; on `false`
/// the caller seeds from HEAD.
async fn seed_index_from_live(checkout: &Path, tmp: &Path, index_env: &[(String, String)]) -> bool {
    let Ok(out) = run_git(
        checkout,
        &["rev-parse", "--git-path", "index"],
        "rev-parse --git-path",
    )
    .await
    else {
        return false;
    };
    // Relative to the checkout (`.git/index`), or absolute in a linked worktree.
    let live = checkout.join(String::from_utf8_lossy(&out.stdout).trim());
    // Inline rather than on a blocking thread: a local copy of a few MB at
    // most, and a snapshot abandoned mid-copy (a checkpoint timing out) must
    // not leave a detached copy to recreate the temp file after its cleanup.
    if copy_index(&live, tmp).is_err() {
        return false;
    }
    // `ls-files -v` tags assume-unchanged entries lowercase and skip-worktree
    // ones `S`.
    let Ok(out) = run_git_env(
        checkout,
        &["ls-files", "-v", "-z"],
        index_env,
        "ls-files -v",
    )
    .await
    else {
        return false;
    };
    !out.stdout
        .split(|b| *b == 0)
        .filter_map(|entry| entry.first())
        .any(|tag| tag.is_ascii_lowercase() || *tag == b'S')
}

/// Copy the index at `live` to `dest`, keeping its mtime. Git distrusts the
/// stat data of an entry modified in the same instant the index was written
/// (racy git) by comparing it with the index file's mtime — a copy stamped
/// later would vouch for exactly those entries. The mtime is read first, so a
/// live index replaced mid-copy can only make the copy look older (stricter).
fn copy_index(live: &Path, dest: &Path) -> std::io::Result<()> {
    let mtime = std::fs::metadata(live)?.modified()?;
    std::fs::copy(live, dest)?;
    std::fs::File::options()
        .write(true)
        .open(dest)?
        .set_modified(mtime)
}

/// Make `checkout`'s working tree exactly `snapshot`'s tree with HEAD (and the
/// branch it is on, if any) at `head`, so the snapshot's delta from `head`
/// reads as uncommitted changes — the inverse of [`snapshot_worktree`].
/// Untracked files the snapshot lacks are removed; ignored files are never
/// touched (no `clean -x`). Used by checkpoint restore (`head` = the HEAD the
/// snapshot was taken on).
pub(crate) async fn apply_snapshot(checkout: &Path, snapshot: &str, head: &str) -> Result<()> {
    // Materialize the snapshot exactly (adds/mods/dels), drop what it doesn't
    // track, then move HEAD + index to `head`, leaving the working tree.
    run_git(
        checkout,
        &["reset", "--hard", snapshot],
        "snapshot reset --hard",
    )
    .await?;
    run_git(checkout, &["clean", "-fd"], "snapshot clean").await?;
    run_git(
        checkout,
        &["reset", "--mixed", head],
        "snapshot reset --mixed",
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
    let env = identity_env(repo).await;
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

    /// A committed repo with a `.gitignore`, then uncommitted work of every
    /// kind: a modification, a deletion, a new file, an ignored file, a staged
    /// change modified again, and an intent-to-add entry.
    async fn repo_with_mixed_changes(dir: &Path) {
        init_repo(dir).await.unwrap();
        config(dir, "user.email", "t@example.com").await;
        config(dir, "user.name", "Tester").await;
        std::fs::write(dir.join(".gitignore"), b"ignored.txt\n").unwrap();
        for name in ["keep.txt", "drop.txt", "staged.txt"] {
            std::fs::write(dir.join(name), b"base").unwrap();
        }
        commit_all(dir, "base").await.unwrap();

        std::fs::write(dir.join("keep.txt"), b"modified").unwrap();
        std::fs::remove_file(dir.join("drop.txt")).unwrap();
        std::fs::write(dir.join("new.txt"), b"added").unwrap();
        std::fs::write(dir.join("ignored.txt"), b"secret").unwrap();
        std::fs::write(dir.join("staged.txt"), b"staged").unwrap();
        run_git(dir, &["add", "staged.txt"], "add").await.unwrap();
        std::fs::write(dir.join("staged.txt"), b"staged then edited").unwrap();
        std::fs::write(dir.join("intent.txt"), b"intent").unwrap();
        run_git(dir, &["add", "-N", "intent.txt"], "add -N")
            .await
            .unwrap();
    }

    /// The tree the original seeding produces — a temp index from `read-tree
    /// HEAD`, then `add -A` — which the index-copy seed must reproduce.
    async fn read_tree_seeded_tree(repo: &Path) -> String {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let env = vec![(
            "GIT_INDEX_FILE".to_string(),
            tmp.path().display().to_string(),
        )];
        run_git_env(repo, &["read-tree", "HEAD"], &env, "read-tree")
            .await
            .unwrap();
        run_git_env(repo, &["add", "-A"], &env, "add")
            .await
            .unwrap();
        let out = run_git_env(repo, &["write-tree"], &env, "write-tree")
            .await
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    async fn snapshot_tree(repo: &Path) -> String {
        let snap = snapshot_worktree(repo).await.unwrap();
        rev_parse(repo, &format!("{snap}^{{tree}}")).await.unwrap()
    }

    /// Whether the index-copy seed would be used for `repo`'s current index.
    async fn copy_seed_used(repo: &Path) -> bool {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let env = vec![(
            "GIT_INDEX_FILE".to_string(),
            tmp.path().display().to_string(),
        )];
        seed_index_from_live(repo, tmp.path(), &env).await
    }

    #[tokio::test]
    async fn index_copy_seed_gives_the_read_tree_seed_tree() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path();
        repo_with_mixed_changes(repo).await;

        let live_index = || async {
            run_git(repo, &["ls-files", "--stage"], "ls-files")
                .await
                .unwrap()
                .stdout
        };
        let before = live_index().await;

        assert!(copy_seed_used(repo).await, "a plain index is copied");
        assert_eq!(snapshot_tree(repo).await, read_tree_seeded_tree(repo).await);
        assert_eq!(live_index().await, before, "the live index is untouched");
    }

    /// Entries `add -A` never compares with the working tree would hide their
    /// changes from a copied index, so either bit forces the HEAD seed — and the
    /// snapshot still records the hidden edits.
    #[tokio::test]
    async fn skip_worktree_and_assume_unchanged_fall_back_to_the_head_seed() {
        for bit in ["--skip-worktree", "--assume-unchanged"] {
            let td = tempfile::tempdir().unwrap();
            let repo = td.path();
            repo_with_mixed_changes(repo).await;
            run_git(repo, &["update-index", bit, "keep.txt"], "update-index")
                .await
                .unwrap();

            assert!(!copy_seed_used(repo).await, "{bit} must not be copied");
            let tree = snapshot_tree(repo).await;
            assert_eq!(tree, read_tree_seeded_tree(repo).await, "{bit}");
            let keep = run_git(repo, &["show", &format!("{tree}:keep.txt")], "show")
                .await
                .unwrap();
            assert_eq!(keep.stdout, b"modified", "{bit} hid an edit");
        }
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
