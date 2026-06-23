//! Thin wrapper around `git worktree`.
//!
//! Kept deliberately minimal — the v1 supervisor only needs to add a
//! worktree on a fresh branch and remove it later.

use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

use crate::error::{Error, Result};

/// Hard cap on the spawn-time `git fetch`. A fetch over a hung SSH/TCP
/// connection can otherwise block for the OS keep-alive window (75–120s), far
/// past the supervisor's 15s spawn watchdog — which would mark the agent
/// `Error` while the background task later still runs `start_process`, leaving
/// a live process under a failed-looking agent. Bounding it keeps the fetch
/// inside the spawn budget; on timeout we fall back to local HEAD.
const FETCH_TIMEOUT: Duration = Duration::from_secs(8);

/// Hard cap on quick network-bound ops (push/pull and the gh PR actions) so a
/// dropped connection surfaces as a finite error the UI can show, instead of
/// hanging a spinner for the OS keep-alive window. Deliberately generous —
/// these transfer little data, so 60s only ever trips on a stalled connection,
/// not on slow-but-progressing work. NOT used for `clone` (a large repo can
/// legitimately take minutes); clone's retry wedge is handled by self-heal in
/// `new_project::clone` instead.
const NET_TIMEOUT: Duration = Duration::from_secs(60);

/// Run a network-bound command under `NET_TIMEOUT`, killing the process (and
/// its SSH/HTTP child) on expiry via `kill_on_drop` rather than orphaning it.
/// `what` names the op for the timeout message. Mirrors `fetch_fork_point`'s
/// inline pattern; shared so push/pull and the gh PR ops stay consistent.
pub(crate) async fn output_timed(cmd: &mut Command, what: &str) -> Result<std::process::Output> {
    cmd.kill_on_drop(true);
    tokio::time::timeout(NET_TIMEOUT, cmd.output())
        .await
        .map_err(|_| {
            // `Error::Other` (not `Error::Git`) — `what` already names the op
            // (e.g. "gh pr create"), so the "git command failed:" prefix would
            // mislabel non-git callers.
            Error::Other(format!(
                "{what} timed out after {}s — check your network connection",
                NET_TIMEOUT.as_secs()
            ))
        })?
        .map_err(Error::from)
}

/// Create a worktree on detached HEAD (no branch yet). Used by the
/// instant-spawn flow so we don't pollute `git branch` for agents
/// that may never receive a user message. The branch is created
/// later via `checkout_new_branch` when we have a slug from the
/// first user message.
///
/// `base` is the commit-ish the worktree starts from (e.g. `origin/main`
/// after a fresh fetch). When `None`, it starts from the repo's current
/// HEAD — the legacy behavior.
pub async fn worktree_add_detached(
    repo: &Path,
    worktree_path: &Path,
    base: Option<&str>,
) -> Result<()> {
    let path = worktree_path
        .to_str()
        .ok_or_else(|| Error::InvalidPath(worktree_path.display().to_string()))?;
    let mut args = vec!["worktree", "add", "--detach", path];
    if let Some(base) = base {
        args.push(base);
    }
    let out = Command::new("git")
        .current_dir(repo)
        .args(&args)
        .output()
        .await?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();

    // Recoverable case: the target path already exists but git doesn't track it
    // as a worktree — an orphan left by a crashed spawn or another instance
    // that shares this worktrees root. Prune stale registrations, and if the
    // path still isn't a live worktree, clear the orphan and retry once. We
    // never delete a *registered* worktree: that would be someone else's live
    // checkout, so in that case we fall through to the original error.
    //
    // We gate on `worktree_path.exists()` rather than on the git error text:
    // the stderr phrasing ("already exists") varies by git version and is
    // translated under non-C locales, so matching it would silently skip the
    // recovery on, say, a French or Japanese machine. The on-disk check is the
    // actual condition this branch handles and is locale-independent.
    if worktree_path.exists() && !is_registered_worktree(repo, worktree_path).await {
        let _ = worktree_prune(repo).await;
        if !is_registered_worktree(repo, worktree_path).await && worktree_path.exists() {
            if let Err(e) = tokio::fs::remove_dir_all(worktree_path).await {
                tracing::warn!(path = %worktree_path.display(), error = %e, "orphan worktree dir cleanup failed");
            }
        }
        if !worktree_path.exists() {
            let retry = Command::new("git")
                .current_dir(repo)
                .args(&args)
                .output()
                .await?;
            if retry.status.success() {
                tracing::info!(path = %worktree_path.display(), "recovered orphan worktree path on spawn");
                return Ok(());
            }
            return Err(Error::Git(format!(
                "worktree add --detach failed: {}",
                String::from_utf8_lossy(&retry.stderr).trim()
            )));
        }
    }

    Err(Error::Git(format!("worktree add --detach failed: {stderr}")))
}

/// Is `path` currently registered as a worktree of `repo`? Used by spawn to
/// tell an orphan directory (safe to clear) from a live checkout (must not
/// touch). A `false` here authorizes the caller to `remove_dir_all` the path,
/// so when the listing itself fails (transient lock contention, a process-limit
/// spike) we return `true`: "can't confirm it's an orphan, so don't touch it."
/// The caller then falls through to the original error rather than risking the
/// deletion of a live checkout.
async fn is_registered_worktree(repo: &Path, path: &Path) -> bool {
    let out = match Command::new("git")
        .current_dir(repo)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .await
    {
        Ok(out) if out.status.success() => out,
        _ => return true,
    };
    // `tokio::fs::canonicalize` keeps these path-resolution syscalls off the
    // executor thread — `std::fs` would block it if the filesystem is slow
    // (network mount, loaded disk).
    let canonical = tokio::fs::canonicalize(path).await.ok();
    let stdout = String::from_utf8_lossy(&out.stdout);
    for listed in stdout.lines().filter_map(|l| l.strip_prefix("worktree ")) {
        let listed = Path::new(listed);
        if listed == path {
            return true;
        }
        if let Some(a) = &canonical {
            if let Ok(b) = tokio::fs::canonicalize(listed).await {
                if a == &b {
                    return true;
                }
            }
        }
    }
    false
}

/// Best-effort fetch of `branch` from `origin` so a freshly-spawned worktree
/// can fork from the latest remote state rather than a stale local ref.
/// Returns the commit-ish a worktree should be based on — `origin/<branch>`
/// when the fetch succeeded and the remote branch resolves — otherwise `None`,
/// signalling the caller to fall back to local HEAD.
///
/// Never errors: a missing `origin`, an offline machine, or a purely local
/// branch are all expected and simply mean "use local state".
pub async fn fetch_fork_point(repo: &Path, branch: &str) -> Option<String> {
    // `kill_on_drop` so a timeout actually tears down the hung git process
    // (and its SSH child) rather than orphaning it to keep blocking on the
    // dead connection.
    let fetched = tokio::time::timeout(
        FETCH_TIMEOUT,
        Command::new("git")
            .current_dir(repo)
            .args(["fetch", "origin", branch])
            .kill_on_drop(true)
            .output(),
    )
    .await;
    // Timed out, failed to spawn, or non-zero exit → fall back to local HEAD.
    match fetched {
        Ok(Ok(out)) if out.status.success() => {}
        _ => return None,
    }
    // Confirm the remote-tracking ref resolves before handing it back — a
    // refspec that doesn't map this branch into refs/remotes would otherwise
    // leave us pointing at a ref that `worktree add` can't use.
    let remote_ref = format!("origin/{branch}");
    rev_parse(repo, &remote_ref).await.ok().map(|_| remote_ref)
}

/// Inside an existing worktree, create a new branch at the current
/// commit and check it out (`git checkout -b <branch>`). Used to
/// promote a detached-HEAD worktree onto a named branch once the
/// first user message gives us a slug.
pub async fn checkout_new_branch(worktree: &Path, branch: &str) -> Result<()> {
    let out = Command::new("git")
        .current_dir(worktree)
        .args(["checkout", "-b", branch])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "checkout -b {branch} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// `git init` a fresh repository at `path` (created if absent). Used by the
/// New Project "create" flow before seeding an initial commit.
pub async fn init_repo(path: &Path) -> Result<()> {
    let out = Command::new("git")
        .args([
            "init",
            path.to_str().ok_or_else(|| {
                Error::InvalidPath(path.display().to_string())
            })?,
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

/// Stage everything and create a commit in `repo`. Relies on the user's
/// global git identity (`user.name` / `user.email`); a missing identity
/// surfaces as a `git commit` error.
pub async fn commit_all(repo: &Path, message: &str) -> Result<()> {
    let add = Command::new("git")
        .current_dir(repo)
        .args(["add", "-A"])
        .output()
        .await?;
    if !add.status.success() {
        return Err(Error::Git(format!(
            "add -A failed: {}",
            String::from_utf8_lossy(&add.stderr).trim()
        )));
    }

    let out = Command::new("git")
        .current_dir(repo)
        .args(["commit", "-m", message])
        .output()
        .await?;
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

pub async fn worktree_remove(repo: &Path, worktree_path: &Path, force: bool) -> Result<()> {
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    let path_str = worktree_path
        .to_str()
        .ok_or_else(|| Error::InvalidPath(worktree_path.display().to_string()))?;
    args.push(path_str);
    let out = Command::new("git")
        .current_dir(repo)
        .args(&args)
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "worktree remove failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Drop any internal `.git/worktrees/<id>` refs whose linked working tree
/// no longer exists. Safe to run unconditionally — git just no-ops when
/// there's nothing to prune.
pub async fn worktree_prune(repo: &Path) -> Result<()> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["worktree", "prune"])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "worktree prune failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Return the name of the currently-checked-out branch in the repo,
/// or `None` if HEAD is detached. Used by the supervisor to record
/// the parent branch when spawning an agent worktree.
pub async fn current_branch(repo: &Path) -> Result<Option<String>> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["symbolic-ref", "--short", "-q", "HEAD"])
        .output()
        .await?;
    match out.status.code() {
        Some(0) => {
            let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if name.is_empty() {
                Ok(None)
            } else {
                Ok(Some(name))
            }
        }
        // `symbolic-ref -q` exits 1 in detached-HEAD state. Treat that
        // as "no branch", not an error.
        Some(1) => Ok(None),
        _ => Err(Error::Git(format!(
            "symbolic-ref failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))),
    }
}

/// Whether a local branch with this name exists in the repo. Used by
/// the supervisor to disambiguate auto-generated branch names before
/// spawning a worktree — on collision it falls back to a name that
/// includes the agent's place id.
pub async fn branch_exists(repo: &Path, branch: &str) -> Result<bool> {
    let out = Command::new("git")
        .current_dir(repo)
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .output()
        .await?;
    // Exit 0 = ref exists, exit 1 = not found, anything else = real error.
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(Error::Git(format!(
            "show-ref failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))),
    }
}

/// Resolve a ref to its full SHA. Returns the bare 40-char hex string.
/// Errors if the ref is unknown or git is unhappy.
pub async fn rev_parse(repo: &Path, refname: &str) -> Result<String> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["rev-parse", "--verify", refname])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "rev-parse {refname} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Run `git diff --shortstat <a>..<b>` and parse the additions /
/// deletions counts. Returns zero counts if both refs resolve to the
/// same commit (git prints nothing in that case).
pub async fn diff_shortstat(
    repo: &Path,
    from_sha: &str,
    to_sha: &str,
) -> Result<(u32, u32)> {
    let out = Command::new("git")
        .current_dir(repo)
        .args([
            "diff",
            "--shortstat",
            &format!("{from_sha}..{to_sha}"),
        ])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "diff --shortstat failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(parse_shortstat(&line))
}

/// Run `git diff --shortstat <base>` from a live worktree. This compares the
/// current working tree, including uncommitted changes, against the base ref.
pub async fn worktree_diff_shortstat(
    worktree: &Path,
    base_ref: &str,
) -> Result<(u32, u32)> {
    let out = Command::new("git")
        .current_dir(worktree)
        .args(["diff", "--shortstat", base_ref])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "diff --shortstat {base_ref} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(parse_shortstat(&line))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_shortstat_typical() {
        assert_eq!(
            parse_shortstat(" 3 files changed, 82 insertions(+), 12 deletions(-)"),
            (82, 12)
        );
    }

    #[test]
    fn parse_shortstat_only_additions() {
        assert_eq!(
            parse_shortstat(" 1 file changed, 5 insertions(+)"),
            (5, 0)
        );
    }

    #[test]
    fn parse_shortstat_only_deletions() {
        assert_eq!(
            parse_shortstat(" 2 files changed, 9 deletions(-)"),
            (0, 9)
        );
    }

    #[test]
    fn parse_shortstat_empty() {
        assert_eq!(parse_shortstat(""), (0, 0));
    }

    async fn config(repo: &Path, key: &str, val: &str) {
        let out = Command::new("git")
            .current_dir(repo)
            .args(["config", key, val])
            .output()
            .await
            .unwrap();
        assert!(out.status.success());
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
    async fn fetch_fork_point_without_remote_is_none() {
        // No `origin` configured → best-effort fetch fails and we fall back to
        // local HEAD (None), never an error.
        let td = tempfile::tempdir().unwrap();
        let repo = td.path();
        init_repo(repo).await.unwrap();
        config(repo, "user.email", "t@example.com").await;
        config(repo, "user.name", "Tester").await;
        std::fs::write(repo.join("a.txt"), b"x").unwrap();
        commit_all(repo, "first").await.unwrap();

        assert_eq!(fetch_fork_point(repo, "main").await, None);
    }

    #[tokio::test]
    async fn worktree_add_detached_uses_base_commit_when_given() {
        // A worktree forked from an explicit commit-ish starts at that commit,
        // not at the repo's current HEAD.
        let td = tempfile::tempdir().unwrap();
        let repo = td.path();
        init_repo(repo).await.unwrap();
        config(repo, "user.email", "t@example.com").await;
        config(repo, "user.name", "Tester").await;

        std::fs::write(repo.join("a.txt"), b"one").unwrap();
        commit_all(repo, "first").await.unwrap();
        let first = rev_parse(repo, "HEAD").await.unwrap();
        std::fs::write(repo.join("b.txt"), b"two").unwrap();
        commit_all(repo, "second").await.unwrap();

        // Base the worktree on the first commit even though HEAD is now `second`.
        let wt = td.path().join("wt");
        worktree_add_detached(repo, &wt, Some(&first)).await.unwrap();
        assert_eq!(rev_parse(&wt, "HEAD").await.unwrap(), first);

        // With no base it tracks current HEAD (`second`).
        let head = rev_parse(repo, "HEAD").await.unwrap();
        let wt2 = td.path().join("wt2");
        worktree_add_detached(repo, &wt2, None).await.unwrap();
        assert_eq!(rev_parse(&wt2, "HEAD").await.unwrap(), head);
    }

    #[tokio::test]
    async fn worktree_add_detached_recovers_orphan_dir() {
        // An orphan directory at the target path (left by a crashed spawn or a
        // foreign instance) is not a registered worktree, so add should clear
        // it and succeed rather than failing with "already exists".
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_repo(&repo).await.unwrap();
        config(&repo, "user.email", "t@example.com").await;
        config(&repo, "user.name", "Tester").await;
        std::fs::write(repo.join("a.txt"), b"x").unwrap();
        commit_all(&repo, "first").await.unwrap();

        // Pre-create the target as a plain, untracked directory with a file.
        let wt = td.path().join("orphan");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join("leftover.txt"), b"stale").unwrap();

        worktree_add_detached(&repo, &wt, None).await.unwrap();
        assert!(is_registered_worktree(&repo, &wt).await);
        // The stale contents were cleared and replaced by the checkout.
        assert!(!wt.join("leftover.txt").exists());
        assert!(wt.join("a.txt").exists());
    }

    #[tokio::test]
    async fn worktree_add_detached_refuses_to_clobber_live_worktree() {
        // If the path is a *registered* worktree (someone's live checkout), add
        // must fail rather than delete it.
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_repo(&repo).await.unwrap();
        config(&repo, "user.email", "t@example.com").await;
        config(&repo, "user.name", "Tester").await;
        std::fs::write(repo.join("a.txt"), b"x").unwrap();
        commit_all(&repo, "first").await.unwrap();

        let wt = td.path().join("live");
        worktree_add_detached(&repo, &wt, None).await.unwrap();
        std::fs::write(wt.join("precious.txt"), b"keep me").unwrap();

        // A second add at the same live path must error and leave it untouched.
        assert!(worktree_add_detached(&repo, &wt, None).await.is_err());
        assert!(wt.join("precious.txt").exists());
    }
}

fn parse_shortstat(s: &str) -> (u32, u32) {
    let mut adds = 0u32;
    let mut dels = 0u32;
    for chunk in s.split(',').map(|c| c.trim()) {
        let mut parts = chunk.splitn(2, ' ');
        let n: u32 = match parts.next().and_then(|t| t.parse().ok()) {
            Some(v) => v,
            None => continue,
        };
        let label = parts.next().unwrap_or("");
        if label.starts_with("insertion") {
            adds = n;
        } else if label.starts_with("deletion") {
            dels = n;
        }
    }
    (adds, dels)
}

/// Create a branch at a specific commit. Errors if the branch already
/// exists or the SHA isn't reachable.
pub async fn branch_create_at(repo: &Path, name: &str, sha: &str) -> Result<()> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["branch", name, sha])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "branch {name} {sha} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Create a worktree at `worktree_path` checked out on an existing
/// branch. Counterpart to `worktree_add_detached` — used by restore.
pub async fn worktree_add_branch(
    repo: &Path,
    worktree_path: &Path,
    branch: &str,
) -> Result<()> {
    let out = Command::new("git")
        .current_dir(repo)
        .args([
            "worktree",
            "add",
            worktree_path.to_str().ok_or_else(|| {
                Error::InvalidPath(worktree_path.display().to_string())
            })?,
            branch,
        ])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "worktree add {branch} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Push the current branch to `origin`. Uses `-u` to set the upstream
/// tracking ref on the first push.
/// Returns `"up-to-date"` when the remote already had everything (a no-op
/// push), otherwise `"pushed"`. Lets the UI confirm the outcome instead of
/// silently doing nothing when there was nothing to send.
pub async fn push(worktree: &Path, branch: &str) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.current_dir(worktree).args(["push", "-u", "origin", branch]);
    let out = output_timed(&mut cmd, "git push").await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "push failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    // git reports "Everything up-to-date" on stderr when there was nothing new.
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    if combined.contains("Everything up-to-date") {
        Ok("up-to-date".to_string())
    } else {
        Ok("pushed".to_string())
    }
}

/// Pull latest from the tracking remote branch.
/// Requires `push -u` to have been called first to establish an upstream.
pub async fn pull(worktree: &Path) -> Result<()> {
    let mut cmd = Command::new("git");
    cmd.current_dir(worktree).args(["pull"]);
    let out = output_timed(&mut cmd, "git pull").await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "pull failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Rebase the current branch onto `base` (e.g. "main"). Used by the clean-state
/// panel action to bring the worktree up to date with its base branch when the
/// base has moved ahead. Aborts the rebase on conflict so the worktree is never
/// left mid-rebase — the caller surfaces the error.
pub async fn rebase_onto(worktree: &Path, base: &str) -> Result<()> {
    let out = Command::new("git")
        .current_dir(worktree)
        .args(["rebase", base])
        .output()
        .await?;
    if !out.status.success() {
        // Don't leave the worktree in a detached mid-rebase state.
        let _ = Command::new("git")
            .current_dir(worktree)
            .args(["rebase", "--abort"])
            .output()
            .await;
        return Err(Error::Git(format!(
            "rebase onto {base} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Stage all working-tree changes (including untracked) and create a commit.
/// Errors if there is nothing to commit or if git is unhappy.
pub async fn commit(worktree: &Path, message: &str) -> Result<()> {
    let add = Command::new("git")
        .current_dir(worktree)
        .args(["add", "-A"])
        .output()
        .await?;
    if !add.status.success() {
        return Err(Error::Git(format!(
            "add -A failed: {}",
            String::from_utf8_lossy(&add.stderr).trim()
        )));
    }
    let out = Command::new("git")
        .current_dir(worktree)
        .args(["commit", "-m", message])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "commit failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Discard every uncommitted change in the working tree, including
/// untracked files and directories. Equivalent to a hard reset plus a
/// `clean -fd`. Destructive — caller is responsible for confirming.
pub async fn discard_all(worktree: &Path) -> Result<()> {
    let reset = Command::new("git")
        .current_dir(worktree)
        .args(["reset", "--hard", "HEAD"])
        .output()
        .await?;
    if !reset.status.success() {
        return Err(Error::Git(format!(
            "reset --hard failed: {}",
            String::from_utf8_lossy(&reset.stderr).trim()
        )));
    }
    let clean = Command::new("git")
        .current_dir(worktree)
        .args(["clean", "-fd"])
        .output()
        .await?;
    if !clean.status.success() {
        return Err(Error::Git(format!(
            "clean -fd failed: {}",
            String::from_utf8_lossy(&clean.stderr).trim()
        )));
    }
    Ok(())
}

/// Stash all working-tree changes including untracked files. No message —
/// git generates the default "WIP on <branch>" label.
pub async fn stash_push(worktree: &Path) -> Result<()> {
    let out = Command::new("git")
        .current_dir(worktree)
        .args(["stash", "push", "--include-untracked"])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "stash push failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Abort an in-progress merge, restoring the pre-merge working tree.
pub async fn merge_abort(worktree: &Path) -> Result<()> {
    let out = Command::new("git")
        .current_dir(worktree)
        .args(["merge", "--abort"])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "merge --abort failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Force-delete a local branch. Returns Ok even if the branch never
/// existed in the first place — that's exactly the state the caller
/// usually wants to converge on. Errors only for genuine git failures
/// (e.g. branch checked out in another live worktree).
pub async fn branch_delete(repo: &Path, branch: &str) -> Result<()> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["branch", "-D", branch])
        .output()
        .await?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    // git emits "branch '<x>' not found." with exit 1 when the branch is
    // already gone. Treat that as success — the caller's goal is satisfied.
    if stderr.contains("not found") {
        return Ok(());
    }
    Err(Error::Git(format!(
        "branch -D {branch} failed: {}",
        stderr.trim()
    )))
}

/// List all local branches in the repo, sorted alphabetically.
pub async fn list_local_branches(repo: &Path) -> Result<Vec<String>> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["for-each-ref", "refs/heads", "--format=%(refname:short)", "--sort=refname"])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "for-each-ref failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let branches = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();
    Ok(branches)
}

/// List the worktree's relevant files: everything tracked plus untracked
/// files that aren't gitignored. Paths are repo-relative with forward
/// slashes (git's native form). This is what the File panel browses — it
/// naturally excludes `node_modules`, build output, etc.
pub async fn list_files(worktree: &Path) -> Result<Vec<String>> {
    let out = Command::new("git")
        .current_dir(worktree)
        .args(["ls-files", "-z", "--cached", "--others", "--exclude-standard"])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "ls-files failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect())
}

/// Read a single file's contents at a given ref (e.g. the parent branch),
/// used to show the prior contents of a file the agent deleted.
pub async fn show_file(worktree: &Path, base_ref: &str, path: &str) -> Result<String> {
    let out = Command::new("git")
        .current_dir(worktree)
        .args(["show", &format!("{base_ref}:{path}")])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "show {base_ref}:{path} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Return the 1-indexed line numbers (in the current working-tree file) the
/// agent changed versus `base_ref`, split into purely-added lines and
/// modified lines. Drives the File panel's VS Code-style change gutter.
pub async fn file_changed_lines(
    worktree: &Path,
    base_ref: &str,
    path: &str,
) -> Result<(Vec<u32>, Vec<u32>)> {
    let out = Command::new("git")
        .current_dir(worktree)
        .args(["diff", "--no-color", "-U0", base_ref, "--", path])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "diff -U0 {base_ref} -- {path} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(parse_changed_lines(&String::from_utf8_lossy(&out.stdout)))
}

/// Return the full unified diff of `path` versus `base_ref`, for the Code
/// panel's live view. `-U3` gives three lines of surrounding context per hunk.
pub async fn file_diff(worktree: &Path, base_ref: &str, path: &str) -> Result<String> {
    let out = Command::new("git")
        .current_dir(worktree)
        .args(["diff", "--no-color", "-U3", base_ref, "--", path])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "diff -U3 {base_ref} -- {path} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Parse `git diff -U0` output into (added, modified) new-file line numbers.
/// A hunk that only inserts lines marks them "added"; a hunk that also
/// removes lines marks its inserted lines "modified" (a replacement).
fn parse_changed_lines(diff: &str) -> (Vec<u32>, Vec<u32>) {
    let mut added: Vec<u32> = Vec::new();
    let mut modified: Vec<u32> = Vec::new();
    let mut new_line: u32 = 0;
    let mut hunk_added: Vec<u32> = Vec::new();
    let mut hunk_has_del = false;

    let flush = |hunk_added: &mut Vec<u32>,
                 hunk_has_del: &mut bool,
                 added: &mut Vec<u32>,
                 modified: &mut Vec<u32>| {
        if *hunk_has_del {
            modified.append(hunk_added);
        } else {
            added.append(hunk_added);
        }
        hunk_added.clear();
        *hunk_has_del = false;
    };

    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("@@") {
            flush(&mut hunk_added, &mut hunk_has_del, &mut added, &mut modified);
            // rest looks like " -a,b +c,d @@ ..."; take the "+c[,d]" token.
            if let Some(plus) = rest.split_whitespace().find(|t| t.starts_with('+')) {
                new_line = plus
                    .trim_start_matches('+')
                    .split(',')
                    .next()
                    .and_then(|n| n.parse::<u32>().ok())
                    .unwrap_or(0);
            }
            continue;
        }
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        match line.chars().next() {
            Some('+') => {
                hunk_added.push(new_line);
                new_line = new_line.saturating_add(1);
            }
            Some('-') => hunk_has_del = true,
            Some('\\') => {} // "\ No newline at end of file" — ignore
            _ => new_line = new_line.saturating_add(1), // context line
        }
    }
    flush(&mut hunk_added, &mut hunk_has_del, &mut added, &mut modified);
    (added, modified)
}

#[cfg(test)]
mod changed_lines_tests {
    use super::parse_changed_lines;

    #[test]
    fn pure_additions_are_added() {
        let diff = "\
diff --git a/f b/f
--- a/f
+++ b/f
@@ -0,0 +1,3 @@
+one
+two
+three";
        assert_eq!(parse_changed_lines(diff), (vec![1, 2, 3], vec![]));
    }

    #[test]
    fn replacement_lines_are_modified() {
        // two old lines replaced by two new ones at line 3
        let diff = "\
@@ -3,2 +3,2 @@
-old a
-old b
+new a
+new b";
        assert_eq!(parse_changed_lines(diff), (vec![], vec![3, 4]));
    }

    #[test]
    fn mixed_hunks() {
        let diff = "\
@@ -3,1 +3,2 @@
-was
+now
+extra
@@ -10,0 +11,1 @@
+appended";
        // first hunk has a deletion → its '+' lines are modified (3,4);
        // second hunk is pure addition → added (11).
        assert_eq!(parse_changed_lines(diff), (vec![11], vec![3, 4]));
    }

    #[test]
    fn no_newline_marker_ignored() {
        let diff = "\
@@ -1 +1 @@
-a
+b
\\ No newline at end of file";
        assert_eq!(parse_changed_lines(diff), (vec![], vec![1]));
    }
}
