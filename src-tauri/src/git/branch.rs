//! Branch, fork-point, rev-parse and existence queries against a checkout.

use std::path::Path;
use std::time::Duration;

use crate::error::{Error, Result};

use super::cmd::{apply_github_auth, git_output, no_hooks_env, run_git, run_git_env};

/// Hard cap on the spawn-time `git fetch`. A fetch over a hung SSH/TCP
/// connection can otherwise block for the OS keep-alive window (75–120s), far
/// past the supervisor's 15s spawn watchdog — which would mark the agent
/// `Error` while the background task later still runs `start_process`, leaving
/// a live process under a failed-looking agent. Bounding it keeps the fetch
/// inside the spawn budget; on timeout we fall back to local HEAD.
const FETCH_TIMEOUT: Duration = Duration::from_secs(8);

/// Best-effort fetch of `branch` from `origin` so a freshly-spawned checkout
/// can fork from the latest remote state rather than a stale local ref.
/// Returns the commit-ish a checkout should be based on — the SHA that
/// `origin/<branch>` resolves to **in this repo** when the fetch succeeded —
/// otherwise `None`, signalling the caller to fall back to local HEAD.
///
/// The SHA (not the symbolic `origin/<branch>`) is essential for Clone-mode
/// provisioning: the checkout is a `git clone --shared` of this repo, so
/// inside the clone `origin/<branch>` resolves to this repo's *local*
/// `refs/heads/<branch>` — potentially stale — not the remote-tracking ref
/// the fetch just updated. A SHA resolves identically everywhere, and its
/// objects are reachable from the clone via alternates (shared clones) or
/// the copied object store (self-contained clones).
///
/// Never errors: a missing `origin`, an offline machine, or a purely local
/// branch are all expected and simply mean "use local state".
pub async fn fetch_fork_point(repo: &Path, branch: &str) -> Option<String> {
    // `kill_on_drop` so a timeout actually tears down the hung git process
    // (and its SSH child) rather than orphaning it to keep blocking on the
    // dead connection.
    let mut fetch_cmd = crate::git_dist::command(repo);
    fetch_cmd
        .args(["fetch", "origin", branch])
        .kill_on_drop(true);
    apply_github_auth(&mut fetch_cmd);
    let fetched = tokio::time::timeout(FETCH_TIMEOUT, fetch_cmd.output()).await;
    // Timed out, failed to spawn, or non-zero exit → fall back to local HEAD.
    match fetched {
        Ok(Ok(out)) if out.status.success() => {}
        _ => return None,
    }
    // Resolve the remote-tracking ref to a SHA here, in the repo the fetch
    // updated. This both confirms the refspec mapped the branch into
    // refs/remotes and pins the base to the fetched tip regardless of which
    // repo (source or clone) later checks it out.
    let remote_ref = format!("origin/{branch}");
    rev_parse(repo, &remote_ref).await.ok()
}

/// Inside an existing checkout, create a new branch at the current
/// commit and check it out (`git checkout -b <branch>`). Used to
/// promote a detached-HEAD checkout onto a named branch once the
/// first user message gives us a slug.
pub async fn checkout_new_branch(checkout: &Path, branch: &str) -> Result<()> {
    // `checkout` fires `post-checkout`, which would run on the host against an
    // agent-writable workspace — disable workspace hooks for this invocation.
    run_git_env(
        checkout,
        &["checkout", "-b", branch],
        &no_hooks_env(),
        &format!("checkout -b {branch}"),
    )
    .await?;
    Ok(())
}

/// Most same-named branches we'll step over before giving up when
/// materializing an agent's branch. A modest cap so a pathological
/// pile-up surfaces as an error rather than an unbounded probe loop.
const MAX_BRANCH_SUFFIX: u32 = 1000;

/// Whether `branch` is already claimed — either a local head or a known
/// remote-tracking ref (`origin/<branch>`). Used to pick a collision-free
/// name when materializing an agent's branch at push time.
///
/// The remote check reads `refs/remotes/origin/<branch>`, which reflects the
/// last fetch rather than a live `ls-remote` — a branch created on the remote
/// since then isn't seen, and `git push` would update it. That race is rare
/// and acceptable; avoiding a network round-trip on every push isn't.
pub async fn branch_name_taken(checkout: &Path, branch: &str) -> Result<bool> {
    if branch_exists(checkout, branch).await? {
        return Ok(true);
    }
    let refname = format!("refs/remotes/origin/{branch}");
    let out = git_output(checkout, &["show-ref", "--verify", "--quiet", &refname]).await?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Ok(false),
    }
}

/// Materialize a branch on a (typically detached) checkout at its current
/// HEAD, picking the first collision-free name from `desired`, `desired-2`,
/// `desired-3`, … and checking it out. Returns the name actually used.
///
/// This is the single point where an agent's branch is born — at the first
/// push, named from the agent's conventional choice (`fix/…`, `feat/…`,
/// `chore/…`) rather than a placeholder allocated at spawn.
pub async fn checkout_new_unique_branch(checkout: &Path, desired: &str) -> Result<String> {
    for n in 1..=MAX_BRANCH_SUFFIX {
        let candidate = if n == 1 {
            desired.to_string()
        } else {
            format!("{desired}-{n}")
        };
        // Propagate a probe error rather than masking it as "free": treating a
        // transient show-ref failure as an open name would attempt a checkout
        // that fails confusingly. Surfacing it lets the caller report honestly.
        if !branch_name_taken(checkout, &candidate).await? {
            checkout_new_branch(checkout, &candidate).await?;
            return Ok(candidate);
        }
    }
    Err(Error::Git(format!(
        "no free branch name for `{desired}` within {MAX_BRANCH_SUFFIX} tries"
    )))
}

/// Return the name of the currently-checked-out branch in the repo,
/// or `None` if HEAD is detached. Used by the supervisor to record
/// the parent branch when spawning an agent checkout.
pub async fn current_branch(repo: &Path) -> Result<Option<String>> {
    let out = git_output(repo, &["symbolic-ref", "--short", "-q", "HEAD"]).await?;
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
/// spawning a checkout — on collision it falls back to a name that
/// includes the agent's place id.
pub async fn branch_exists(repo: &Path, branch: &str) -> Result<bool> {
    let refname = format!("refs/heads/{branch}");
    let out = git_output(repo, &["show-ref", "--verify", "--quiet", &refname]).await?;
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
    let out = run_git(
        repo,
        &["rev-parse", "--verify", refname],
        &format!("rev-parse {refname}"),
    )
    .await?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// List all local branches in the repo, sorted alphabetically.
pub async fn list_local_branches(repo: &Path) -> Result<Vec<String>> {
    let out = run_git(
        repo,
        &[
            "for-each-ref",
            "refs/heads",
            "--format=%(refname:short)",
            "--sort=refname",
        ],
        "for-each-ref",
    )
    .await?;
    let branches = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();
    Ok(branches)
}

#[cfg(test)]
mod tests {
    use super::super::worktree::{commit_all, init_repo};
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
    async fn fetch_fork_point_returns_fetched_tip_sha_not_stale_local_head() {
        // Clone-mode workspaces are `git clone --shared` of the source repo,
        // where `origin/<branch>` resolves to the source's *local* head —
        // stale when the user hasn't pulled. The fork point must therefore be
        // the SHA of the freshly-fetched remote tip, resolved in the source
        // repo, never the symbolic `origin/<branch>`.
        let td = tempfile::tempdir().unwrap();

        // `upstream` plays the true remote.
        let upstream = td.path().join("upstream");
        init_repo(&upstream).await.unwrap();
        config(&upstream, "user.email", "t@example.com").await;
        config(&upstream, "user.name", "Tester").await;
        std::fs::write(upstream.join("a.txt"), b"one").unwrap();
        commit_all(&upstream, "first").await.unwrap();
        run_git(&upstream, &["checkout", "-B", "main"], "checkout -B main")
            .await
            .unwrap();

        // `source` is the user's repo, cloned before upstream advanced.
        let source = td.path().join("source");
        let out = Command::new("git")
            .current_dir(td.path())
            .args(["clone", upstream.to_str().unwrap(), "source"])
            .output()
            .await
            .unwrap();
        assert!(out.status.success());

        // Upstream advances; the source's local `main` is now stale.
        std::fs::write(upstream.join("b.txt"), b"two").unwrap();
        commit_all(&upstream, "second").await.unwrap();
        let upstream_tip = rev_parse(&upstream, "main").await.unwrap();
        let stale_local = rev_parse(&source, "main").await.unwrap();
        assert_ne!(stale_local, upstream_tip);

        let base = fetch_fork_point(&source, "main").await.unwrap();
        assert_eq!(base, upstream_tip);
    }
}
