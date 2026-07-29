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
/// inside the spawn budget; on timeout the caller degrades to a local ref.
const FETCH_TIMEOUT: Duration = Duration::from_secs(8);

/// Best-effort fetch of `branch` from `origin`, returning the SHA that
/// `origin/<branch>` resolves to **in `repo`** afterwards — the tip a checkout
/// should fork from — or `None` when the remote couldn't be reached.
///
/// Run this on the freshly-provisioned **workspace clone**, not on the source
/// repo, and that is the whole point. A workspace is `git clone --shared
/// <source path>`, and `git clone` of a local path maps the source's *local*
/// `refs/heads/*` into the clone's `refs/remotes/origin/*` (the source's own
/// remote-tracking refs are never copied); provisioning then repoints `origin`
/// at the real GitHub URL. So before this fetch, a workspace's `origin/<branch>`
/// looks like a remote-tracking ref but is really a snapshot of the source's
/// local branch — arbitrarily far behind the branch on GitHub. Fetching here
/// corrects that ref *and* yields the true fork point; fetching into the source
/// instead would do neither, because the clone borrows the source's objects but
/// not its refs.
///
/// Returns the SHA rather than the symbolic `origin/<branch>` because the fork
/// point must stay pinned: it is recorded as the workspace's `base_sha` (the
/// diff baseline) and passed to `checkout --detach`, where a *branch name* the
/// clone has no local head for would trip git's remote-DWIM (an implicit `-b`)
/// and fail.
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
    // Timed out, failed to spawn, or non-zero exit → the caller degrades.
    match fetched {
        Ok(Ok(out)) if out.status.success() => {}
        _ => return None,
    }
    // Resolve the remote-tracking ref to a SHA here, in the repo the fetch
    // updated. This both confirms the refspec mapped the branch into
    // refs/remotes and pins the base to the fetched tip.
    let remote_ref = format!("origin/{branch}");
    rev_parse(repo, &remote_ref).await.ok()
}

/// Remote-tracking refs, then local heads, for the two conventional default
/// branch names — the probe order [`default_branch`] falls through when the
/// repo has no `origin/HEAD` symref to read.
const DEFAULT_BRANCH_CANDIDATES: [&str; 4] = [
    "refs/remotes/origin/main",
    "refs/remotes/origin/master",
    "refs/heads/main",
    "refs/heads/master",
];

/// The repo's default branch — the base a new agent forks from when the user
/// didn't pick one on the new-agent screen.
///
/// Read from `refs/remotes/origin/HEAD`, the symref `git clone` writes to record
/// what the remote's HEAD pointed at. That is the same branch GitHub calls the
/// repository default, resolved locally: no network round-trip on the spawn
/// path and no GitHub token required. Repos built by `git init` + `git remote
/// add` never get that symref, so fall through to whichever conventional name
/// actually exists (remote-tracking first, then local, so a mirror of an
/// unfetched repo still resolves), and only then to `"main"`.
///
/// Deliberately infallible and deliberately *never* the currently-checked-out
/// branch. Forking a new agent from whatever the user happens to have open is
/// never the intent — the user may pick their current branch from the dropdown,
/// but it must never be the implicit default. A wrong guess here is recoverable
/// (pick a base in the dropdown); an error would block the spawn.
pub async fn default_branch(repo: &Path) -> String {
    if let Ok(out) = git_output(
        repo,
        &["symbolic-ref", "--short", "-q", "refs/remotes/origin/HEAD"],
    )
    .await
    {
        if out.status.success() {
            let full = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if let Some(branch) = full.strip_prefix("origin/") {
                if !branch.is_empty() {
                    return branch.to_string();
                }
            }
        }
    }
    for refname in DEFAULT_BRANCH_CANDIDATES {
        if let Ok(out) = git_output(repo, &["show-ref", "--verify", "--quiet", refname]).await {
            if out.status.success() {
                // Every candidate is a full refname whose last segment is the
                // branch name (none of the conventional names contain a slash).
                if let Some(branch) = refname.rsplit('/').next() {
                    return branch.to_string();
                }
            }
        }
    }
    "main".to_string()
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
    use std::path::PathBuf;
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
        // The fork point must be the SHA of the freshly-fetched remote tip, not
        // whatever the repo already had locally — the whole reason provisioning
        // calls this instead of resolving a ref it inherited.
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

    /// A repo with a real `origin`, cloned from `upstream` so git writes the
    /// `refs/remotes/origin/HEAD` symref the way it does for a user's checkout.
    /// Returns (upstream, clone).
    async fn cloned_repo(td: &Path, default: &str) -> (PathBuf, PathBuf) {
        let upstream = td.join("upstream");
        init_repo(&upstream).await.unwrap();
        config(&upstream, "user.email", "t@example.com").await;
        config(&upstream, "user.name", "Tester").await;
        std::fs::write(upstream.join("a.txt"), b"one").unwrap();
        commit_all(&upstream, "first").await.unwrap();
        // Rename (not `checkout -B`) so the initial branch doesn't linger: `git
        // init` names it from the host's `init.defaultBranch`, and a stray
        // `main`/`master` would make the fallback probes non-deterministic.
        run_git(&upstream, &["branch", "-m", default], "branch -m default")
            .await
            .unwrap();

        let clone = td.join("clone");
        let out = Command::new("git")
            .current_dir(td)
            .args(["clone", upstream.to_str().unwrap(), "clone"])
            .output()
            .await
            .unwrap();
        assert!(out.status.success());
        (upstream, clone)
    }

    #[tokio::test]
    async fn default_branch_reads_origin_head_not_the_checked_out_branch() {
        // The product rule: a new agent must never implicitly fork from
        // whatever the user happens to have open. `develop` is checked out and
        // `master` also exists — neither may win over the remote's HEAD.
        let td = tempfile::tempdir().unwrap();
        let (_upstream, repo) = cloned_repo(td.path(), "trunk").await;
        run_git(&repo, &["checkout", "-b", "develop"], "checkout -b develop")
            .await
            .unwrap();
        run_git(&repo, &["branch", "master"], "branch master")
            .await
            .unwrap();

        assert_eq!(default_branch(&repo).await, "trunk");
    }

    #[tokio::test]
    async fn default_branch_falls_back_to_a_conventional_name_without_origin_head() {
        // `git init` + `git remote add` never writes the `origin/HEAD` symref,
        // so fall through to whichever conventional name actually exists —
        // `master` here, not the hardcoded `"main"` that used to be assumed.
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        init_repo(&repo).await.unwrap();
        config(&repo, "user.email", "t@example.com").await;
        config(&repo, "user.name", "Tester").await;
        std::fs::write(repo.join("a.txt"), b"x").unwrap();
        commit_all(&repo, "first").await.unwrap();
        run_git(&repo, &["branch", "-m", "master"], "branch -m master")
            .await
            .unwrap();
        run_git(&repo, &["checkout", "-b", "wip"], "checkout -b wip")
            .await
            .unwrap();

        assert_eq!(default_branch(&repo).await, "master");
    }

    #[tokio::test]
    async fn default_branch_last_resort_is_main() {
        // Nothing to go on (no origin, no conventionally-named branch): return
        // `"main"` rather than erroring, so a spawn is never blocked on this.
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        init_repo(&repo).await.unwrap();
        config(&repo, "user.email", "t@example.com").await;
        config(&repo, "user.name", "Tester").await;
        std::fs::write(repo.join("a.txt"), b"x").unwrap();
        commit_all(&repo, "first").await.unwrap();
        run_git(&repo, &["branch", "-m", "trunk"], "branch -m trunk")
            .await
            .unwrap();

        assert_eq!(default_branch(&repo).await, "main");
    }
}
