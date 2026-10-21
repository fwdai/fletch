//! The host publication layer. Every push of a repository Fletch manages goes
//! through [`publish`] — the agent's `git_push` / `open_pr` ops, the Git
//! panel's Push, a workflow run's finalize, a new project's first publish — so
//! what a publish involves is decided once, here, and no path can skip it:
//!
//! - **Signing.** When the user's own repository signs its commits, the
//!   commits origin lacks are replaced by signed copies before the push
//!   (`git::signing`). The decision and the key come from the user's repo, the
//!   `source_repo`; the agent's checkout contributes objects only.
//! - **Identity, when signing.** A signature is the user vouching, so a commit
//!   made as someone else is refused rather than signed (`git::identity`), and
//!   it is checked over the same refreshed refs the signing range is cut from.
//!   The agent ops also check identity before their approval prompt; the Git
//!   panel and a run's finalize rely on this one.
//! - **The push itself** publishes exactly the commit this function settled
//!   on, so nothing that lands on the branch meanwhile goes out with it.
//!
//! The transport primitives behind this are private to `git`: there is no
//! other way to push.

use std::path::Path;

use super::identity::{branch_tip, check_identity_of, Identity};
use super::signing::{host_signs_commits, refresh_remote_refs, sign_unpublished};
use super::transport::push_commit;
use crate::error::{Error, Result};

/// What a publish sends: the branch as it stands when read, or one exact
/// commit a caller has already examined.
pub enum Tip<'a> {
    Branch,
    Commit(&'a str),
}

/// One push, described.
pub struct Publication<'a> {
    /// The repository the push runs in: an agent checkout, a workflow run
    /// repo, or (for a new project) the user's repo itself.
    pub checkout: &'a Path,
    /// The user's own repository `checkout` was cloned from — the one
    /// repository on this machine no agent can write. Its config decides
    /// whether commits are signed and with what; the signer runs there.
    /// The same path as `checkout` when the user's repo is what is pushed,
    /// which is then published as is: no identity check, no signing pass.
    pub source_repo: &'a Path,
    /// The remote branch, `refs/heads/<branch>` on `origin`. Also the local
    /// branch moved onto the signed tip, when there is one.
    pub branch: &'a str,
    /// A lease-guarded force push (`--force-with-lease --force-if-includes`).
    pub force: bool,
}

/// What went out.
#[derive(Debug)]
pub struct Published {
    /// The transport's one-word summary: `pushed` or `up-to-date`.
    pub summary: String,
    /// The commit now at `refs/heads/<branch>` on origin — the signed copy
    /// when signing rewrote the tip.
    pub tip: String,
}

pub async fn publish(p: &Publication<'_>, tip: Tip<'_>) -> Result<Published> {
    let tip = match tip {
        Tip::Branch => branch_tip(p.checkout, p.branch).await?,
        Tip::Commit(sha) => sha.to_string(),
    };
    // Signing on publish is for work made where the key is not: an agent's
    // checkout. The user's own repository is pushed as it stands — its commits
    // already carry whatever signature the user's git gave them, its history
    // legitimately holds other committers, and a first publish must not
    // rewrite every commit the user ever made.
    let agent_work = p.checkout != p.source_repo;
    let tip = if agent_work && host_signs_commits(p.source_repo).await? {
        signed(p, tip).await?
    } else {
        tip
    };
    let summary = push_commit(p.checkout, p.source_repo, &tip, p.branch, p.force).await?;
    Ok(Published { summary, tip })
}

/// `tip` with its unpublished commits signed, and the local branch moved onto
/// the result. The order is the point: the refresh can widen "what origin
/// lacks" past what an earlier identity check saw (a stale `origin/main`
/// covering an unpushed commit), and signing must never reach a commit the
/// check did not — so the check runs after the refresh, on the same refs the
/// signing range is cut from.
async fn signed(p: &Publication<'_>, tip: String) -> Result<String> {
    refresh_remote_refs(p.checkout, p.source_repo).await?;
    let tip = match check_identity_of(p.checkout, p.branch, tip).await? {
        Identity::Own { tip } => tip,
        Identity::Foreign(why) => return Err(Error::Git(why)),
    };
    let signed = sign_unpublished(p.checkout, p.source_repo, &tip)
        .await
        .map_err(|e| {
            Error::Git(format!(
                "could not sign the commits '{}' would publish: {e}. Your repository's git \
                 config sets commit.gpgsign=true, so Fletch signs on push with the key it \
                 names; make that key available to Fletch, or set commit.gpgsign=false",
                p.branch
            ))
        })?;
    if signed != tip {
        move_branch(p.checkout, p.branch, &tip, &signed).await?;
    }
    Ok(signed)
}

/// Move `refs/heads/<branch>` from `from` to `to` so the agent's next commit
/// builds on the signed tip. Compare-and-swap: a branch that moved since
/// `from` was read fails the publish rather than losing the commit that
/// landed. A branch that does not exist locally (a workflow run publishes a
/// pinned ref from a detached HEAD) has nothing to move.
async fn move_branch(checkout: &Path, branch: &str, from: &str, to: &str) -> Result<()> {
    match branch_tip(checkout, branch).await {
        Ok(at) if at == from => {}
        Ok(_) => {
            return Err(Error::Git(format!(
                "'{branch}' moved while it was being published; push again"
            )))
        }
        Err(_) => return Ok(()),
    }
    let refname = format!("refs/heads/{branch}");
    let out = super::cmd::git_output(
        checkout,
        &[
            "update-ref",
            "-m",
            "fletch: signed on publish",
            &refname,
            to,
            from,
        ],
    )
    .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "moving '{branch}' to its signed tip failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::signing::fixtures::{signing_repos, verified, Signing};
    use super::super::testing::{commit_file, git};
    use super::*;

    fn origin_tip(fx: &Signing, branch: &str) -> String {
        git(
            &fx.source,
            &["ls-remote", "origin", &format!("refs/heads/{branch}")],
        )
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string()
    }

    fn publication<'a>(fx: &'a Signing, branch: &'a str) -> Publication<'a> {
        Publication {
            checkout: &fx.checkout,
            source_repo: &fx.source,
            branch,
            force: false,
        }
    }

    #[tokio::test]
    async fn a_signing_repo_publishes_signed_copies_and_moves_the_branch_onto_them() {
        let td = tempfile::tempdir().unwrap();
        let fx = signing_repos(td.path());
        commit_file(&fx.checkout, "one.txt");
        let tip = commit_file(&fx.checkout, "two.txt");

        let out = publish(&publication(&fx, "main"), Tip::Branch)
            .await
            .unwrap();

        assert_eq!(out.summary, "pushed");
        assert_ne!(out.tip, tip);
        assert_eq!(origin_tip(&fx, "main"), out.tip);
        assert_eq!(
            git(&fx.checkout, &["rev-parse", "refs/heads/main"]),
            out.tip
        );
        assert!(verified(&fx, &out.tip));
        assert!(verified(&fx, &format!("{}~1", out.tip)));
        assert_eq!(
            git(&fx.checkout, &["rev-parse", &format!("{}~2", out.tip)]),
            fx.base
        );
        // The next push has nothing to sign and nothing to send.
        let again = publish(&publication(&fx, "main"), Tip::Branch)
            .await
            .unwrap();
        assert_eq!(again.summary, "up-to-date");
        assert_eq!(again.tip, out.tip);
    }

    #[tokio::test]
    async fn a_repo_that_does_not_sign_publishes_as_is() {
        let td = tempfile::tempdir().unwrap();
        let fx = signing_repos(td.path());
        git(&fx.source, &["config", "commit.gpgsign", "false"]);
        let tip = commit_file(&fx.checkout, "one.txt");

        let out = publish(&publication(&fx, "main"), Tip::Branch)
            .await
            .unwrap();

        assert_eq!(out.tip, tip);
        assert_eq!(origin_tip(&fx, "main"), tip);
        assert!(!verified(&fx, &tip));
    }

    /// `origin/*` in the checkout is whatever the last fetch left — the
    /// source repo's own local heads when the spawn-time fetch failed. A
    /// commit a stale `origin/main` claims as published, and origin does not
    /// have, is signed with the rest.
    #[tokio::test]
    async fn a_stale_remote_ref_does_not_hide_an_unpublished_ancestor() {
        let td = tempfile::tempdir().unwrap();
        let fx = signing_repos(td.path());
        let stale = commit_file(&fx.checkout, "stale.txt");
        git(
            &fx.checkout,
            &["update-ref", "refs/remotes/origin/main", &stale],
        );
        commit_file(&fx.checkout, "two.txt");

        let out = publish(&publication(&fx, "main"), Tip::Branch)
            .await
            .unwrap();

        assert!(verified(&fx, &out.tip));
        assert!(
            verified(&fx, &format!("{}~1", out.tip)),
            "the hidden ancestor is signed too"
        );
        assert_eq!(
            git(&fx.checkout, &["rev-parse", &format!("{}~2", out.tip)]),
            fx.base
        );
    }

    /// The same stale ref can hide a commit the identity check would refuse.
    /// The check runs on the refreshed refs, before signing, so the refresh
    /// never turns a foreign commit into a signed and published one.
    #[tokio::test]
    async fn a_foreign_commit_a_stale_ref_hid_is_refused_not_signed() {
        let td = tempfile::tempdir().unwrap();
        let fx = signing_repos(td.path());
        let repo = fx.checkout.as_path();
        std::fs::write(repo.join("theirs.txt"), b"theirs").unwrap();
        git(repo, &["add", "-A"]);
        git(
            repo,
            &[
                "-c",
                "user.email=bot@example.com",
                "commit",
                "-q",
                "-m",
                "theirs",
            ],
        );
        let foreign = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["update-ref", "refs/remotes/origin/main", &foreign]);
        let tip = commit_file(repo, "two.txt");

        let err = publish(&publication(&fx, "main"), Tip::Branch)
            .await
            .unwrap_err();

        assert!(err.to_string().contains("bot@example.com"), "{err}");
        assert_eq!(origin_tip(&fx, "main"), fx.base, "nothing was pushed");
        assert_eq!(git(repo, &["rev-parse", "refs/heads/main"]), tip);
        assert!(!verified(&fx, &tip));
    }

    /// The checkout's `origin` is the agent's to rewrite; a publish talks only
    /// to the user's repository's remote and refuses when they differ.
    #[tokio::test]
    async fn a_checkout_whose_origin_was_repointed_is_not_published() {
        let td = tempfile::tempdir().unwrap();
        let fx = signing_repos(td.path());
        let elsewhere = td.path().join("elsewhere.git");
        git(
            td.path(),
            &["init", "-q", "--bare", elsewhere.to_str().unwrap()],
        );
        git(
            &fx.checkout,
            &["remote", "set-url", "origin", elsewhere.to_str().unwrap()],
        );
        let tip = commit_file(&fx.checkout, "one.txt");

        let err = publish(&publication(&fx, "main"), Tip::Branch)
            .await
            .unwrap_err();

        assert!(err.to_string().contains("refusing"), "{err}");
        assert_eq!(
            git(&fx.checkout, &["rev-parse", "refs/heads/main"]),
            tip,
            "nothing was rewritten"
        );
        let refs = git(
            td.path(),
            &["--git-dir", elsewhere.to_str().unwrap(), "for-each-ref"],
        );
        assert!(refs.is_empty(), "nothing reached the repointed remote");
        assert_eq!(origin_tip(&fx, "main"), fx.base, "nor the real one");
    }

    #[tokio::test]
    async fn a_branch_that_moved_since_its_tip_was_read_is_not_published() {
        let td = tempfile::tempdir().unwrap();
        let fx = signing_repos(td.path());
        let read = commit_file(&fx.checkout, "one.txt");
        // Something lands after the caller read `read`.
        let newer = commit_file(&fx.checkout, "two.txt");

        let err = publish(&publication(&fx, "main"), Tip::Commit(&read))
            .await
            .unwrap_err();

        assert!(err.to_string().contains("moved"), "{err}");
        assert_eq!(origin_tip(&fx, "main"), fx.base);
        assert_eq!(git(&fx.checkout, &["rev-parse", "refs/heads/main"]), newer);
    }

    /// A workflow run publishes a pinned ref from a detached HEAD onto a
    /// branch it has no local ref for: signed and pushed, nothing to move.
    #[tokio::test]
    async fn a_pinned_commit_with_no_local_branch_is_signed_and_pushed() {
        let td = tempfile::tempdir().unwrap();
        let fx = signing_repos(td.path());
        let pinned = commit_file(&fx.checkout, "step.txt");
        git(&fx.checkout, &["checkout", "-q", "--detach"]);

        let out = publish(&publication(&fx, "wf/run-1"), Tip::Commit(&pinned))
            .await
            .unwrap();

        assert_ne!(out.tip, pinned);
        assert!(verified(&fx, &out.tip));
        assert_eq!(origin_tip(&fx, "wf/run-1"), out.tip);
        assert!(
            !git(&fx.checkout, &["branch", "--list", "wf/run-1"]).contains("wf/run-1"),
            "no local branch was made"
        );
    }

    /// A new project's first publish pushes the user's repo itself, to a
    /// remote with no history: every commit the user ever made is "what
    /// origin lacks", and the history holds a teammate's commits. It goes out
    /// exactly as it stands — no committer check, no rewrite.
    #[tokio::test]
    async fn the_users_own_repo_is_published_as_it_stands() {
        let td = tempfile::tempdir().unwrap();
        let fx = signing_repos(td.path());
        let fresh = td.path().join("fresh.git");
        git(
            td.path(),
            &["init", "-q", "--bare", fresh.to_str().unwrap()],
        );
        git(
            &fx.source,
            &["remote", "set-url", "origin", fresh.to_str().unwrap()],
        );
        // An old commit by someone else, made unsigned.
        std::fs::write(fx.source.join("theirs.txt"), b"theirs").unwrap();
        git(&fx.source, &["add", "-A"]);
        git(
            &fx.source,
            &[
                "-c",
                "user.email=teammate@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                "theirs",
            ],
        );
        let tip = commit_file(&fx.source, "one.txt");

        let out = publish(
            &Publication {
                checkout: &fx.source,
                source_repo: &fx.source,
                branch: "main",
                force: false,
            },
            Tip::Branch,
        )
        .await
        .unwrap();

        assert_eq!(out.tip, tip, "no rewrite");
        assert_eq!(origin_tip(&fx, "main"), tip);
        assert_eq!(git(&fx.source, &["rev-parse", "refs/heads/main"]), tip);
        assert!(
            !verified(&fx, &format!("{tip}~1")),
            "the teammate's commit went out as it was"
        );
    }
}
