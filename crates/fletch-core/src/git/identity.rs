//! Whose commits a push would publish.
//!
//! GitHub links a commit to an account by its email and only marks a signature
//! verified when the committer email belongs to the account that holds the
//! key. A commit made with an overridden identity — `-c user.email=…`, a
//! `GIT_COMMITTER_EMAIL` in the environment — therefore lands as nobody's and
//! unverified, and the only remedy is a history rewrite. The host checks for
//! that before it pushes, while the fix is still local and cheap.

use std::path::Path;

use super::cmd::git_output;
use crate::error::Result;

/// The email the checkout's config resolves for new commits (local over
/// global), or `None` when nothing sets one.
pub async fn configured_email(repo: &Path) -> Result<Option<String>> {
    // Exits 1 with empty stdout when the key is unset — not an error here.
    let out = git_output(repo, &["config", "--get", "user.email"]).await?;
    let email = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok((!email.is_empty()).then_some(email))
}

/// Committer emails on `branch` that origin does not have yet and that differ
/// from `email` — deduplicated, newest commit first. Committer, not author:
/// git stamps the committer from the local identity on every commit it
/// creates here (a cherry-pick keeps its author on purpose), so a foreign
/// committer can only mean the identity was overridden.
pub async fn foreign_committers(repo: &Path, branch: &str, email: &str) -> Result<Vec<String>> {
    // `--not --remotes=origin` keeps the check to what this push would publish:
    // merged-in base commits and anything already on origin are theirs to own.
    let out = git_output(
        repo,
        &["log", "--format=%ce", branch, "--not", "--remotes=origin"],
    )
    .await?;
    if !out.status.success() {
        return Err(crate::error::Error::Git(format!(
            "log {branch} failed: {}",
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

/// Why `branch` must not be pushed as it stands, or `None`: the commits it
/// would publish carry a committer email other than the checkout's own.
/// Skipped when the checkout sets no email at all — there is nothing to
/// compare against, and the commit-time fallback identity is the host's own.
pub async fn refuse_foreign_identity(repo: &Path, branch: &str) -> Result<Option<String>> {
    let Some(email) = configured_email(repo).await? else {
        return Ok(None);
    };
    let foreign = foreign_committers(repo, branch, &email).await?;
    if foreign.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!(
        "refusing to push '{branch}': its new commits were made as {} but this checkout's \
         git config is {email}. GitHub would show them as nobody's and unverified. Commit \
         with the configured identity — plain `git commit`, no `-c user.email` or \
         GIT_COMMITTER_* override — and rewrite the ones already made (e.g. `git rebase \
         <base> --exec 'git commit --amend --no-edit --reset-author'`), then push again \
         with `args.force=true` since the history changed",
        foreign.join(", ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(repo: &Path, args: &[&str]) {
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
    async fn own_commits_pass_and_an_overridden_committer_is_named() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        repo_with_commit(&repo);
        assert_eq!(
            configured_email(&repo).await.unwrap().as_deref(),
            Some("me@example.com")
        );
        assert!(refuse_foreign_identity(&repo, "main")
            .await
            .unwrap()
            .is_none());

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
        let why = refuse_foreign_identity(&repo, "main")
            .await
            .unwrap()
            .expect("an overridden committer must be refused");
        assert!(why.contains("bot@example.com"), "{why}");
        assert!(why.contains("me@example.com"), "{why}");
        assert!(why.contains("--reset-author"), "{why}");
    }

    #[tokio::test]
    async fn a_cherry_picked_author_is_not_a_foreign_committer() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        repo_with_commit(&repo);
        std::fs::write(repo.join("c.txt"), b"z").unwrap();
        run(&repo, &["add", "-A"]);
        // Someone else's authorship, committed here: the committer is still us.
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
        assert!(refuse_foreign_identity(&repo, "main")
            .await
            .unwrap()
            .is_none());
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
        // Once origin holds the foreign commit, it is history, not this push.
        run(&repo, &["push", "-q", "origin", "main"]);
        assert!(refuse_foreign_identity(&repo, "main")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn no_configured_email_means_nothing_to_compare() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        repo_with_commit(&repo);
        run(&repo, &["config", "--unset", "user.email"]);
        // Only the repo's own config is cleared; a global one may still answer.
        // Either way the check must not refuse a checkout for lacking an email.
        let email = configured_email(&repo).await.unwrap();
        let why = refuse_foreign_identity(&repo, "main").await.unwrap();
        if email.is_none() {
            assert!(why.is_none());
        }
    }
}
