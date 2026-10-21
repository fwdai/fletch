//! The one remote host-side transport may reach from a checkout.
//!
//! A checkout's `.git/config` is the agent's to write, and `remote.origin.url`
//! is just a line in it. Host-side git runs fetch, push and ls-remote in the
//! checkout with the user's credentials — the app's GitHub token, and in the
//! user's own repository their agent socket — so a transport that trusted the
//! checkout's `origin` would authenticate wherever the agent pointed it: a
//! `git_fetch` needs no approval, and the objects of any private repository
//! the token can read would land in the checkout.
//!
//! So the remote is never read from the checkout. It is derived from the
//! **source repository** the checkout was cloned from — the user's own, which
//! no agent can write — in the https form provisioning wrote into the
//! checkout, and every transport names that URL explicitly instead of
//! `origin`. The checkout's own `origin` is then only checked: it must still
//! say the same, or the operation is refused and names the change. The
//! remaining config that could redirect an explicit URL (`url.*.insteadOf`,
//! a proxy, a disabled certificate check) is refused by
//! [`super::hardening`] before any transport spawns.

use std::path::Path;

use super::cmd::git_output;
use crate::error::{Error, Result};

/// The URL every host-side fetch, push and ls-remote in `checkout` talks to.
///
/// When `checkout` is `source_repo` — the user's own repository being pushed
/// (a new project's first publish) — its own `origin` is the answer. Otherwise
/// the answer comes from `source_repo`'s `origin` and the checkout's must
/// agree with it.
pub async fn trusted_remote(checkout: &Path, source_repo: &Path) -> Result<String> {
    let Some(source) = origin_url(source_repo).await? else {
        return Err(Error::Git(format!(
            "no origin remote: '{}' has none to publish to or fetch from",
            source_repo.display()
        )));
    };
    if checkout == source_repo {
        return Ok(source);
    }
    let trusted = crate::github::github_https_remote(&source);
    match origin_url(checkout).await? {
        Some(url) if url == trusted => Ok(trusted),
        Some(url) => Err(Error::Git(format!(
            "refusing to reach '{url}': the checkout's origin was changed from the remote it \
             was cloned for ({trusted}). Fletch's host-side git talks only to that remote; \
             restore it with `git remote set-url origin {trusted}`"
        ))),
        None => Err(Error::Git(format!(
            "refusing to fetch or push: the checkout has no origin, while the repository it was \
             cloned from publishes to {trusted}. Restore it with `git remote set-url origin \
             {trusted}`"
        ))),
    }
}

/// `+refs/heads/<branch>:refs/remotes/origin/<branch>` — what `git fetch
/// origin <branch>` updates through `remote.origin.fetch`, spelled out because
/// the fetch names a URL and so reads no remote's refspec. `branch` can only
/// ever be a branch name inside it, never an option.
pub fn tracking_refspec(branch: &str) -> String {
    format!("+refs/heads/{branch}:refs/remotes/origin/{branch}")
}

/// Every branch, for the refresh before a publish.
pub const ALL_TRACKING_REFSPEC: &str = "+refs/heads/*:refs/remotes/origin/*";

async fn origin_url(repo: &Path) -> Result<Option<String>> {
    let out = git_output(repo, &["remote", "get-url", "origin"]).await?;
    if !out.status.success() {
        return Ok(None);
    }
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok((!url.is_empty()).then_some(url))
}

#[cfg(test)]
mod tests {
    use super::super::testing::{commit_file, git, init_repo};
    use super::*;

    /// A source repo with `origin` and a checkout cloned from it, each with
    /// origin as provisioning leaves them.
    fn repos(dir: &Path) -> (std::path::PathBuf, std::path::PathBuf, String) {
        let origin = dir.join("origin.git");
        git(dir, &["init", "-q", "--bare", origin.to_str().unwrap()]);
        let source = dir.join("source");
        init_repo(&source);
        git(
            &source,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        commit_file(&source, "base.txt");
        let checkout = dir.join("checkout");
        git(
            dir,
            &[
                "clone",
                "-q",
                source.to_str().unwrap(),
                checkout.to_str().unwrap(),
            ],
        );
        git(
            &checkout,
            &["remote", "set-url", "origin", origin.to_str().unwrap()],
        );
        (source, checkout, origin.to_string_lossy().into_owned())
    }

    #[tokio::test]
    async fn the_source_repos_origin_is_the_remote_when_the_checkout_agrees() {
        let td = tempfile::tempdir().unwrap();
        let (source, checkout, origin) = repos(td.path());
        assert_eq!(trusted_remote(&checkout, &source).await.unwrap(), origin);
    }

    #[tokio::test]
    async fn a_checkout_whose_origin_was_changed_is_refused() {
        let td = tempfile::tempdir().unwrap();
        let (source, checkout, origin) = repos(td.path());
        git(
            &checkout,
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/victim/private.git",
            ],
        );
        let err = trusted_remote(&checkout, &source).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("refusing"), "{msg}");
        assert!(msg.contains("victim/private"), "{msg}");
        assert!(msg.contains(&origin), "names the remote to restore: {msg}");

        git(&checkout, &["remote", "remove", "origin"]);
        let err = trusted_remote(&checkout, &source).await.unwrap_err();
        assert!(err.to_string().contains("refusing"), "{err}");
    }

    #[tokio::test]
    async fn a_source_without_origin_has_no_remote_to_offer() {
        let td = tempfile::tempdir().unwrap();
        let (source, checkout, _) = repos(td.path());
        git(&source, &["remote", "remove", "origin"]);
        let err = trusted_remote(&checkout, &source).await.unwrap_err();
        assert!(err.to_string().contains("no origin remote"), "{err}");
    }

    /// The checkout gets the https form of an ssh origin (provisioning's
    /// rewrite), and that is what agrees.
    #[tokio::test]
    async fn an_ssh_origin_in_the_source_is_trusted_in_its_https_form() {
        let td = tempfile::tempdir().unwrap();
        let (source, checkout, _) = repos(td.path());
        git(
            &source,
            &["remote", "set-url", "origin", "git@github.com:me/repo.git"],
        );
        git(
            &checkout,
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/me/repo.git",
            ],
        );
        assert_eq!(
            trusted_remote(&checkout, &source).await.unwrap(),
            "https://github.com/me/repo.git"
        );
    }

    #[tokio::test]
    async fn the_users_own_repo_uses_its_origin_as_it_stands() {
        let td = tempfile::tempdir().unwrap();
        let (source, _, _) = repos(td.path());
        git(
            &source,
            &["remote", "set-url", "origin", "git@github.com:me/repo.git"],
        );
        assert_eq!(
            trusted_remote(&source, &source).await.unwrap(),
            "git@github.com:me/repo.git",
            "an ssh origin stays ssh: the user's key is reachable there"
        );
    }

    #[test]
    fn a_branch_is_only_ever_a_branch_inside_the_refspec() {
        assert_eq!(
            tracking_refspec("--upload-pack=evil"),
            "+refs/heads/--upload-pack=evil:refs/remotes/origin/--upload-pack=evil"
        );
    }
}
