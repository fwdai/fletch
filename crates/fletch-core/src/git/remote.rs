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

/// Where host-side transport in a checkout reads from and writes to: the
/// user's repository's `origin`, as its fetch URL and its push URL (the
/// `pushurl` of a triangular setup, else the same as fetch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedRemote {
    /// Named by every fetch and ls-remote.
    pub fetch: String,
    /// Named by every push.
    pub push: String,
}

/// The remote every host-side fetch, push and ls-remote in `checkout` talks
/// to.
///
/// When `checkout` is `source_repo` — the user's own repository being pushed
/// (a new project's first publish) — its own `origin` is the answer, as it
/// stands. Otherwise the answer comes from `source_repo`'s `origin`, in the
/// https form provisioning wrote into the checkout, and the checkout's must
/// agree with it. Both are compared in that same form: `git remote get-url`
/// reports a URL after the user's own `url.*.insteadOf` rewrites, so a global
/// ssh rewrite shows on both sides alike and must not read as a change.
pub async fn trusted_remote(checkout: &Path, source_repo: &Path) -> Result<TrustedRemote> {
    let Some(fetch) = origin_url(source_repo, Direction::Fetch).await? else {
        return Err(Error::Git(format!(
            "no origin remote: '{}' has none to publish to or fetch from",
            source_repo.display()
        )));
    };
    let push = origin_url(source_repo, Direction::Push)
        .await?
        .unwrap_or_else(|| fetch.clone());
    if checkout == source_repo {
        return Ok(TrustedRemote { fetch, push });
    }
    let trusted = TrustedRemote {
        fetch: crate::github::github_https_remote(&fetch),
        push: crate::github::github_https_remote(&push),
    };
    match origin_url(checkout, Direction::Fetch).await? {
        Some(url) if crate::github::github_https_remote(&url) == trusted.fetch => Ok(trusted),
        Some(url) => Err(Error::Git(format!(
            "refusing to reach '{url}': the checkout's origin was changed from the remote it \
             was cloned for ({}). Fletch's host-side git talks only to that remote; restore it \
             with `git remote set-url origin {}`",
            trusted.fetch, trusted.fetch
        ))),
        None => Err(Error::Git(format!(
            "refusing to fetch or push: the checkout has no origin, while the repository it was \
             cloned from publishes to {}. Restore it with `git remote set-url origin {}`",
            trusted.fetch, trusted.fetch
        ))),
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Fetch,
    Push,
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

/// `origin`'s URL in `repo` for `direction`, as git would use it (`get-url`
/// applies the user's `insteadOf` rewrites); `None` without an origin.
async fn origin_url(repo: &Path, direction: Direction) -> Result<Option<String>> {
    let args: &[&str] = match direction {
        Direction::Fetch => &["remote", "get-url", "origin"],
        Direction::Push => &["remote", "get-url", "--push", "origin"],
    };
    let out = git_output(repo, args).await?;
    if !out.status.success() {
        return Ok(None);
    }
    // `--push` lists every push URL; the first is the one git pushes to first.
    let url = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
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
        assert_eq!(
            trusted_remote(&checkout, &source).await.unwrap(),
            TrustedRemote {
                fetch: origin.clone(),
                push: origin
            }
        );
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
            trusted_remote(&checkout, &source).await.unwrap().fetch,
            "https://github.com/me/repo.git"
        );
    }

    /// `git remote get-url` reports a URL after the user's own `insteadOf`
    /// rewrites, on both sides alike; the agreement check must see through
    /// them rather than report an unchanged checkout as repointed.
    #[tokio::test]
    async fn the_users_insteadof_rewrite_does_not_read_as_a_change() {
        let td = tempfile::tempdir().unwrap();
        let (source, checkout, _) = repos(td.path());
        for repo in [&source, &checkout] {
            git(
                repo,
                &[
                    "remote",
                    "set-url",
                    "origin",
                    "https://github.com/me/repo.git",
                ],
            );
            // Stands in for the user's global rewrite; `get-url` applies it
            // from any scope.
            git(
                repo,
                &[
                    "config",
                    "url.git@github.com:.insteadOf",
                    "https://github.com/",
                ],
            );
            assert_eq!(
                git(repo, &["remote", "get-url", "origin"]),
                "git@github.com:me/repo.git",
                "the rewrite is in effect"
            );
        }
        assert_eq!(
            trusted_remote(&checkout, &source).await.unwrap().fetch,
            "https://github.com/me/repo.git"
        );
    }

    /// A triangular setup fetches from one URL and pushes to another; the
    /// source repo's `pushurl` is where the checkout's pushes go.
    #[tokio::test]
    async fn the_source_repos_pushurl_is_where_pushes_go() {
        let td = tempfile::tempdir().unwrap();
        let (source, checkout, origin) = repos(td.path());
        git(
            &source,
            &[
                "remote",
                "set-url",
                "--push",
                "origin",
                "git@github.com:me/fork.git",
            ],
        );
        let remote = trusted_remote(&checkout, &source).await.unwrap();
        assert_eq!(remote.fetch, origin);
        assert_eq!(remote.push, "https://github.com/me/fork.git");
        let own = trusted_remote(&source, &source).await.unwrap();
        assert_eq!(own.push, "git@github.com:me/fork.git");
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
            trusted_remote(&source, &source).await.unwrap().fetch,
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
