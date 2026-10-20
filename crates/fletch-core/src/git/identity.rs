//! Whose commits a push would publish. GitHub attributes a commit, and verifies
//! its signature, by the committer email; one made under an overridden identity
//! lands as nobody's and unverified, and only a rewrite fixes it. Checked
//! before the push, while the rewrite is still local — and the check names the
//! exact commit it read, so the push can publish that one and no later.

use std::path::Path;

use super::cmd::git_output;
use crate::error::{Error, Result};

/// The checkout's `user.email`, or `None` when unset.
pub async fn configured_email(repo: &Path) -> Result<Option<String>> {
    let out = git_output(repo, &["config", "--get", "user.email"]).await?;
    let email = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok((!email.is_empty()).then_some(email))
}

/// The commit `refs/heads/<branch>` points at — the full ref, so a branch
/// named like a path (`docs` beside a docs/ directory) is a revision.
pub async fn branch_tip(repo: &Path, branch: &str) -> Result<String> {
    let rev = format!("refs/heads/{branch}");
    let out = git_output(repo, &["rev-parse", "--verify", "--quiet", &rev]).await?;
    if !out.status.success() {
        return Err(Error::Git(format!("no local branch {branch}")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Committer emails other than `email` on the commits reachable from `tip`
/// that origin lacks, deduplicated. Committer, not author: a cherry-pick keeps
/// its author, but the committer is always the local identity unless it was
/// overridden.
pub async fn foreign_committers(repo: &Path, tip: &str, email: &str) -> Result<Vec<String>> {
    let out = git_output(
        repo,
        &[
            "log",
            "--format=%ce",
            tip,
            "--not",
            "--remotes=origin",
            "--",
        ],
    )
    .await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "log {tip} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let mut seen = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let committer = line.trim();
        if committer.is_empty() || committer.eq_ignore_ascii_case(email) {
            continue;
        }
        if !seen
            .iter()
            .any(|s: &String| s.eq_ignore_ascii_case(committer))
        {
            seen.push(committer.to_string());
        }
    }
    Ok(seen)
}

/// What the identity check found at the branch's tip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    /// Every new commit is the checkout's own; `tip` is the commit the check
    /// read, which is the one to push.
    Own { tip: String },
    /// Why the branch must not be pushed as it stands.
    Foreign(String),
}

/// Check the commits `branch` would publish. A checkout with no email
/// configured has nothing to compare against and passes with its tip.
pub async fn check_identity(repo: &Path, branch: &str) -> Result<Identity> {
    let tip = branch_tip(repo, branch).await?;
    check_identity_of(repo, branch, tip).await
}

/// [`check_identity`] of an exact commit: `tip` is what a push of `branch`
/// would publish, whether or not the branch exists locally (a workflow run
/// pushes a pinned ref from a detached HEAD).
pub async fn check_identity_of(repo: &Path, branch: &str, tip: String) -> Result<Identity> {
    let Some(email) = configured_email(repo).await? else {
        return Ok(Identity::Own { tip });
    };
    let foreign = foreign_committers(repo, &tip, &email).await?;
    if foreign.is_empty() {
        return Ok(Identity::Own { tip });
    }
    Ok(Identity::Foreign(format!(
        "refusing to push '{branch}': new commits were made as {}, not the checkout's \
         {email} — GitHub would show them as nobody's and unverified. Rewrite them with \
         the configured identity (`git rebase <base> --exec 'git commit --amend --no-edit'` \
         re-stamps the committer and keeps each author), then push again with \
         `args.force=true`",
        foreign.join(", ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(repo: &Path, args: &[&str]) {
        super::super::testing::git(repo, args);
    }

    fn head(repo: &Path) -> String {
        let out = std::process::Command::new("git")
            .current_dir(repo)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn repo_with_commit(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        run(dir, &["init", "-q", "-b", "main"]);
        run(dir, &["config", "user.email", "me@example.com"]);
        run(dir, &["config", "user.name", "Me"]);
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        run(dir, &["add", "-A"]);
        run(dir, &["commit", "-q", "-m", "init"]);
    }

    #[tokio::test]
    async fn own_commits_pass_with_their_tip_and_an_overridden_committer_is_named() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        repo_with_commit(&repo);
        assert_eq!(
            configured_email(&repo).await.unwrap().as_deref(),
            Some("me@example.com")
        );
        assert_eq!(
            check_identity(&repo, "main").await.unwrap(),
            Identity::Own { tip: head(&repo) }
        );

        std::fs::write(repo.join("b.txt"), b"y").unwrap();
        run(&repo, &["add", "-A"]);
        run(
            &repo,
            &[
                "-c",
                "user.email=bot@example.com",
                "commit",
                "-q",
                "-m",
                "as the bot",
            ],
        );
        let Identity::Foreign(why) = check_identity(&repo, "main").await.unwrap() else {
            panic!("an overridden committer must be refused");
        };
        assert!(why.contains("bot@example.com"), "{why}");
        assert!(why.contains("me@example.com"), "{why}");
        assert!(why.contains("--amend --no-edit"), "{why}");
        assert!(
            !why.contains("--reset-author"),
            "must not erase authors: {why}"
        );
    }

    #[tokio::test]
    async fn a_branch_named_like_a_path_is_still_a_branch() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        repo_with_commit(&repo);
        run(&repo, &["checkout", "-q", "-b", "docs"]);
        std::fs::create_dir_all(repo.join("docs")).unwrap();
        std::fs::write(repo.join("docs/a.md"), b"#").unwrap();
        run(&repo, &["add", "-A"]);
        run(&repo, &["commit", "-q", "-m", "docs"]);
        // Bare `docs` would be both a revision and a path to git; the check
        // must read the branch, not refuse the push.
        assert_eq!(
            check_identity(&repo, "docs").await.unwrap(),
            Identity::Own { tip: head(&repo) }
        );
    }

    #[tokio::test]
    async fn a_cherry_picked_author_is_not_a_foreign_committer() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        repo_with_commit(&repo);
        std::fs::write(repo.join("c.txt"), b"z").unwrap();
        run(&repo, &["add", "-A"]);
        run(
            &repo,
            &[
                "commit",
                "-q",
                "-m",
                "theirs",
                "--author=Them <them@example.com>",
            ],
        );
        assert!(matches!(
            check_identity(&repo, "main").await.unwrap(),
            Identity::Own { .. }
        ));
    }

    #[tokio::test]
    async fn commits_origin_already_has_are_not_checked() {
        let td = tempfile::tempdir().unwrap();
        let remote = td.path().join("origin.git");
        run(
            td.path(),
            &["init", "-q", "--bare", remote.to_str().unwrap()],
        );
        let repo = td.path().join("repo");
        repo_with_commit(&repo);
        run(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        std::fs::write(repo.join("b.txt"), b"y").unwrap();
        run(&repo, &["add", "-A"]);
        run(
            &repo,
            &[
                "-c",
                "user.email=bot@example.com",
                "commit",
                "-q",
                "-m",
                "as the bot",
            ],
        );
        run(&repo, &["push", "-q", "origin", "main"]);
        assert!(matches!(
            check_identity(&repo, "main").await.unwrap(),
            Identity::Own { .. }
        ));
    }

    #[tokio::test]
    async fn no_configured_email_means_nothing_to_compare() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        repo_with_commit(&repo);
        run(&repo, &["config", "--unset", "user.email"]);
        // Only the repo's own config is cleared; the developer's global one may
        // still answer, in which case the commit (made as me@example.com) is
        // rightly foreign to it. With no email at all, the check passes.
        let verdict = check_identity(&repo, "main").await.unwrap();
        match configured_email(&repo).await.unwrap() {
            None => assert!(matches!(verdict, Identity::Own { .. })),
            Some(global) => assert!(
                matches!(&verdict, Identity::Foreign(why) if why.contains(&global)),
                "{verdict:?}"
            ),
        }
    }

    #[tokio::test]
    async fn a_missing_branch_is_an_error_not_a_pass() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        repo_with_commit(&repo);
        assert!(check_identity(&repo, "nope").await.is_err());
    }
}
