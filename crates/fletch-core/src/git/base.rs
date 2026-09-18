//! Where an agent checkout's base lives — resolved once, in one place.
//!
//! An agent workspace is a `git clone` of a source repo, left detached at the
//! fetched `origin/<base>` tip. The clone also inherits a `refs/heads/<base>`
//! from the source, and that local ref is a snapshot of whatever the user had
//! when the clone was taken — it never advances afterwards. Resolving the base
//! by bare name (or local-head-first) therefore reads a *stale* commit and
//! reports commits and diffs that don't exist: HEAD identical to `origin/main`,
//! yet "6 ahead, +5639 −524".
//!
//! Every call site used to carry its own ladder. This module is the only one:
//! full refnames, remote-tracking first, and a fork point that is the merge-base
//! with the base's *current* tip rather than the spawn-time `base_sha` (the
//! agent's base can be redirected after spawn — see
//! `supervisor::rpc_watch`, which persists a redirected PR base).

use std::path::Path;

use super::cmd::git_output_env;

/// The base an agent checkout is measured against, resolved once.
#[derive(Debug, Clone)]
pub struct ResolvedBase {
    /// Branch name (for display and for write ops that need a live branch).
    pub name: String,
    /// The base's tip commit, or None when it can't be resolved in this checkout.
    pub tip: Option<String>,
    /// merge-base(HEAD, tip), else `base_sha` when it's an ancestor of HEAD, else None.
    pub fork_point: Option<String>,
}

/// Resolve `name` — the base branch of an agent checkout cloned from
/// `source_repo` — into the commits the rest of the app measures against.
///
/// `tip` is the first of these that names a commit **present in `checkout`**
/// (a `--no-hardlinks` clone does not share the source's object store, so the
/// source's refs can point at objects this checkout has never seen):
///
/// 1. the source repo's `refs/remotes/origin/<name>` — the freshest, because
///    the background `refresh_base_freshness` sweep advances it;
/// 2. the clone's own `refs/remotes/origin/<name>`, as fetched at spawn;
/// 3. the clone's `refs/heads/<name>`, which is stale by construction but is
///    still the base for a local-only repo that has no origin at all.
///
/// `fork_point` is `merge-base(HEAD, tip)` — the commit this checkout actually
/// diverged from — falling back to the spawn-time `base_sha` only when that is
/// still an ancestor of HEAD. `None` for either field means "couldn't be
/// resolved here"; callers degrade (no counts, uncommitted-only diffs) rather
/// than guess, because every guess at this layer is a phantom commit on screen.
pub async fn resolve_base(
    checkout: &Path,
    source_repo: &Path,
    name: &str,
    base_sha: Option<&str>,
) -> ResolvedBase {
    let tip = resolve_tip(checkout, source_repo, name).await;
    let mut fork_point = match &tip {
        Some(tip) => read(checkout, &["merge-base", "HEAD", tip]).await,
        None => None,
    };
    if fork_point.is_none() {
        // No tip, or histories that share no commit (a re-created base branch).
        // The spawn-time fork point still describes this checkout as long as
        // HEAD is built on it.
        if let Some(sha) = base_sha {
            if succeeds(checkout, &["merge-base", "--is-ancestor", sha, "HEAD"]).await {
                fork_point = Some(sha.to_string());
            }
        }
    }
    ResolvedBase {
        name: name.to_string(),
        tip,
        fork_point,
    }
}

/// The base's tip commit, by the ladder documented on [`resolve_base`]. Always
/// full refnames: a bare name resolves through git's ref-search order, where a
/// tag or a `refs/heads/` snapshot can answer for the remote-tracking branch.
async fn resolve_tip(checkout: &Path, source_repo: &Path, name: &str) -> Option<String> {
    let tracking = format!("refs/remotes/origin/{name}");
    if let Some(sha) = rev_parse_quiet(source_repo, &tracking).await {
        if has_commit(checkout, &sha).await {
            return Some(sha);
        }
    }
    for refname in [tracking, format!("refs/heads/{name}")] {
        if let Some(sha) = rev_parse_quiet(checkout, &refname).await {
            return Some(sha);
        }
    }
    None
}

/// `<refname>`'s commit, or `None` when the ref is absent. `--quiet` so an
/// unknown ref is an empty, successful answer rather than noise on stderr.
async fn rev_parse_quiet(repo: &Path, refname: &str) -> Option<String> {
    read(repo, &["rev-parse", "--verify", "--quiet", refname]).await
}

/// Whether `sha` is a commit this checkout actually holds.
async fn has_commit(checkout: &Path, sha: &str) -> bool {
    let spec = format!("{sha}^{{commit}}");
    rev_parse_quiet(checkout, &spec).await.is_some()
}

/// Read a single-line answer out of git, or `None` for any failure — an absent
/// ref, an object the store doesn't hold, a broken repo. Resolving a base is
/// advisory by contract, so nothing here is worth an error.
async fn read(repo: &Path, args: &[&str]) -> Option<String> {
    let out = git_output_env(repo, args, &optional_locks_off())
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Whether `git <args>` exits zero — for the commands that answer with their
/// exit code and no output (`merge-base --is-ancestor`).
async fn succeeds(repo: &Path, args: &[&str]) -> bool {
    git_output_env(repo, args, &optional_locks_off())
        .await
        .is_ok_and(|out| out.status.success())
}

/// Every read here is a ref/object lookup that never needs the index, so tell
/// git not to take `.git/index.lock` for its opportunistic stat refresh — the
/// agent is writing in this tree concurrently (same reason `git_state`'s
/// `read_command` sets it).
fn optional_locks_off() -> [(String, String); 1] {
    [("GIT_OPTIONAL_LOCKS".to_string(), "0".to_string())]
}

#[cfg(test)]
mod tests {
    use super::super::branch::rev_parse;
    use super::super::cmd::run_git;
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

    async fn commit(repo: &Path, file: &str, body: &str) -> String {
        std::fs::write(repo.join(file), body).unwrap();
        commit_all(repo, body).await.unwrap();
        rev_parse(repo, "HEAD").await.unwrap()
    }

    /// A source repo on `main` plus an agent-style clone of it, detached at
    /// `origin/main`. Returns (source, clone).
    async fn source_and_clone(td: &Path) -> (PathBuf, PathBuf) {
        let source = td.join("source");
        init_repo(&source).await.unwrap();
        config(&source, "user.email", "t@example.com").await;
        config(&source, "user.name", "Tester").await;
        commit(&source, "a.txt", "one").await;
        run_git(&source, &["branch", "-m", "main"], "branch -m main")
            .await
            .unwrap();

        let clone = td.join("clone");
        let out = Command::new("git")
            .current_dir(td)
            .args(["clone", source.to_str().unwrap(), "clone"])
            .output()
            .await
            .unwrap();
        assert!(out.status.success());
        config(&clone, "user.email", "t@example.com").await;
        config(&clone, "user.name", "Tester").await;
        let tip = rev_parse(&clone, "refs/remotes/origin/main").await.unwrap();
        run_git(&clone, &["checkout", "--detach", &tip], "detach")
            .await
            .unwrap();
        (source, clone)
    }

    /// The bug this module exists for: a clone's `refs/heads/main` is a snapshot
    /// from clone time and never advances, so a name-based ladder that reads it
    /// reports phantom commits on a checkout sitting exactly on `origin/main`.
    #[tokio::test]
    async fn stale_local_head_never_wins_over_the_tracking_ref() {
        let td = tempfile::tempdir().unwrap();
        let (source, clone) = source_and_clone(td.path()).await;
        let stale = rev_parse(&clone, "refs/heads/main").await.unwrap();

        // The base moves and both repos learn about it, as the freshness sweep
        // and the spawn fetch leave things. The clone's `refs/heads/main` stays
        // where it was.
        let moved = commit(&source, "b.txt", "two").await;
        run_git(&clone, &["fetch", "origin", "main"], "fetch")
            .await
            .unwrap();
        run_git(
            &clone,
            &["checkout", "--detach", &moved],
            "detach onto base",
        )
        .await
        .unwrap();
        assert_ne!(stale, moved);

        let base = resolve_base(&clone, &source, "main", Some(&stale)).await;
        assert_eq!(base.name, "main");
        assert_eq!(base.tip.as_deref(), Some(moved.as_str()));
        assert_eq!(base.fork_point.as_deref(), Some(moved.as_str()));
    }

    /// One commit of the agent's own work: the fork point is the merge-base with
    /// the *current* base tip, not the spawn-time `base_sha` (which the base can
    /// outrun, and which a redirected PR base can make plain wrong).
    #[tokio::test]
    async fn fork_point_is_the_merge_base_not_the_recorded_base_sha() {
        let td = tempfile::tempdir().unwrap();
        let (source, clone) = source_and_clone(td.path()).await;
        let older = rev_parse(&clone, "HEAD").await.unwrap();

        // The base advances and the clone forks from the new tip.
        let moved = commit(&source, "b.txt", "two").await;
        run_git(&clone, &["fetch", "origin", "main"], "fetch")
            .await
            .unwrap();
        run_git(
            &clone,
            &["checkout", "--detach", &moved],
            "detach onto base",
        )
        .await
        .unwrap();
        let work = commit(&clone, "c.txt", "agent work").await;

        let base = resolve_base(&clone, &source, "main", Some(&older)).await;
        assert_eq!(base.tip.as_deref(), Some(moved.as_str()));
        assert_eq!(base.fork_point.as_deref(), Some(moved.as_str()));
        assert_ne!(base.fork_point.as_deref(), Some(older.as_str()));
        assert_ne!(base.fork_point.as_deref(), Some(work.as_str()));
    }

    /// The source's tracking ref only wins when its commit is present in the
    /// checkout. A clone that doesn't share the source's objects (no alternates)
    /// must fall back to its own ref rather than name a commit it can't read.
    #[tokio::test]
    async fn source_tracking_ref_wins_only_when_the_commit_is_present() {
        let td = tempfile::tempdir().unwrap();
        let (source, clone) = source_and_clone(td.path()).await;
        let at_clone_time = rev_parse(&clone, "refs/remotes/origin/main").await.unwrap();

        // The source's tracking ref moves ahead. This clone was taken with
        // `--no-hardlinks`-equivalent isolation (a plain file:// clone copies
        // objects), so the new commit simply isn't here.
        run_git(&source, &["checkout", "main"], "checkout main")
            .await
            .unwrap();
        let moved = commit(&source, "b.txt", "two").await;
        run_git(
            &source,
            &["update-ref", "refs/remotes/origin/main", &moved],
            "advance source tracking ref",
        )
        .await
        .unwrap();
        assert!(!has_commit(&clone, &moved).await);

        let base = resolve_base(&clone, &source, "main", None).await;
        assert_eq!(base.tip.as_deref(), Some(at_clone_time.as_str()));

        // Once the commit is present, the source's ref is the freshest answer.
        run_git(&clone, &["fetch", "origin", "main"], "fetch")
            .await
            .unwrap();
        let base = resolve_base(&clone, &source, "main", None).await;
        assert_eq!(base.tip.as_deref(), Some(moved.as_str()));
    }

    /// Nothing resolves: no origin, no local branch by that name. The tip is
    /// unknown, and the recorded `base_sha` stands in for the fork point only
    /// while HEAD is still built on it.
    #[tokio::test]
    async fn unresolvable_base_falls_back_to_base_sha_only_when_it_is_an_ancestor() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        init_repo(&repo).await.unwrap();
        config(&repo, "user.email", "t@example.com").await;
        config(&repo, "user.name", "Tester").await;
        let first = commit(&repo, "a.txt", "one").await;
        let head = commit(&repo, "b.txt", "two").await;
        run_git(&repo, &["branch", "-m", "wip"], "branch -m wip")
            .await
            .unwrap();

        // An unrelated commit on a disjoint history — recorded, but not ours.
        let other = td.path().join("other");
        init_repo(&other).await.unwrap();
        config(&other, "user.email", "t@example.com").await;
        config(&other, "user.name", "Tester").await;
        let unrelated = commit(&other, "z.txt", "elsewhere").await;

        let base = resolve_base(&repo, &repo, "main", Some(&first)).await;
        assert_eq!(base.tip, None);
        assert_eq!(base.fork_point.as_deref(), Some(first.as_str()));

        let base = resolve_base(&repo, &repo, "main", Some(&unrelated)).await;
        assert_eq!(base.tip, None);
        assert_eq!(base.fork_point, None);

        let base = resolve_base(&repo, &repo, "main", None).await;
        assert_eq!(base.fork_point, None);
        assert_eq!(rev_parse(&repo, "HEAD").await.unwrap(), head);
    }
}
