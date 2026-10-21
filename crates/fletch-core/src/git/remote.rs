//! Transport from a checkout, run with the **user's repository's**
//! configuration.
//!
//! A checkout's `.git` is the agent's to write: its config (`remote.*`,
//! `url.*.insteadOf`, `http.*`, `fetch.recurseSubmodules`, …), its submodule
//! configs under `.git/modules`, its refs. Host-side git runs fetch, push and
//! ls-remote with the user's credentials — the app's GitHub token, and in the
//! user's own repository their agent socket — so a transport that read any of
//! that from the checkout could be steered: an unapproved `git_fetch` made to
//! authenticate against whatever repository the agent named, a push recursing
//! into a submodule the agent pointed elsewhere. Listing the keys that steer
//! transport and refusing them one by one is a race against git's config
//! surface.
//!
//! So transport in a checkout reads **no** checkout configuration at all. It
//! runs the way the signer does (`git::signing`): in the user's own repository,
//! which no agent can write, with the checkout supplying only what it must.
//! Git's own worktree mechanism makes that exact split:
//!
//! - `GIT_COMMON_DIR` is the user's repository's `.git`, so configuration —
//!   the remote, its push URLs, the user's `insteadOf` rewrites and proxies and
//!   credential helpers — is the user's, exactly as their terminal reads it;
//! - `GIT_OBJECT_DIRECTORY` is the checkout's object store, so what a fetch
//!   brings lands in the checkout (and what a push sends is read from it);
//! - `GIT_DIR` is the checkout's, so per-worktree refs (`refs/worktree/*`,
//!   `FETCH_HEAD`) land in the checkout — and the user's refs are never
//!   touched, because every fetch and push names an explicit URL (taken from
//!   the user's config) rather than `origin`, so git maintains no
//!   remote-tracking ref in the common dir.
//!
//! The checkout's `refs/remotes/origin/*` is then reconciled from what was
//! fetched, by plain `update-ref` in the checkout. Recursion into submodules
//! is switched off on every invocation, and the agreement check that the
//! checkout's own `origin` still names the user's remote stays: a checkout an
//! agent repointed is refused outright rather than silently served from the
//! real remote.
//!
//! The user's own repository, when it is what is pushed (a new project's first
//! publish), needs none of this: git runs in it plainly, with `origin` by name.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::cmd::{git_output, git_output_stdin};
use crate::error::{Error, Result};

/// Scratch namespace, per-worktree, for what the fetch remote has.
const FETCHED: &str = "refs/worktree/fletch/origin";
/// Scratch namespace, per-worktree, for what each push remote has — one
/// subtree per push URL, in `git remote get-url --push --all` order.
const PUSHED: &str = "refs/worktree/fletch/push";

/// Configuration every transport invocation carries, over the user's own.
/// Recursion is what a push or fetch must never do here: it would run the
/// same transport inside `.git/modules/*`, whose config is the agent's. The
/// GitHub rewrite keeps a GitHub ssh remote on https, where the app's token
/// authenticates, as provisioning has always written it into checkouts; a
/// user's own longer-matching `insteadOf` still wins, as git ranks them.
/// Maintenance is off because this git sees the user's config over the
/// checkout's objects, a mixture nothing should repack.
const TRANSPORT_CONFIG: &[(&str, &str)] = &[
    ("fetch.recurseSubmodules", "no"),
    ("push.recurseSubmodules", "no"),
    ("submodule.recurse", "false"),
    ("gc.auto", "0"),
    ("maintenance.auto", "false"),
    ("fetch.writeCommitGraph", "false"),
    ("url.https://github.com/.insteadOf", "git@github.com:"),
    ("url.https://github.com/.insteadOf", "ssh://git@github.com/"),
];

/// Transport for one checkout, bound to the user's repository it was cloned
/// from. Opening it runs the config refusal on the checkout and the
/// agreement check on its `origin`.
#[derive(Debug)]
pub struct Transport {
    checkout: PathBuf,
    source_repo: PathBuf,
    /// `None` when the checkout *is* the user's repository.
    split: Option<Split>,
}

/// The three directories that split a transport between the two repositories.
#[derive(Debug)]
struct Split {
    git_dir: PathBuf,
    common_dir: PathBuf,
    objects: PathBuf,
}

/// Which branches a fetch mirrors into the checkout.
#[derive(Clone, Copy, Debug)]
pub enum Branches<'a> {
    /// Every branch of the fetch remote and of each push remote, pruning
    /// what is gone. The view a publish needs.
    All,
    /// One branch of the fetch remote.
    One(&'a str),
}

impl Transport {
    pub async fn open(checkout: &Path, source_repo: &Path) -> Result<Self> {
        super::hardening::refuse_steerable_config(checkout).await?;
        require_origin_unchanged(checkout, source_repo).await?;
        let split = if checkout == source_repo {
            None
        } else {
            Some(Split {
                git_dir: git_path(checkout, "--absolute-git-dir").await?,
                common_dir: git_path(source_repo, "--git-common-dir").await?,
                objects: git_path(checkout, "--git-common-dir")
                    .await?
                    .join("objects"),
            })
        };
        Ok(Self {
            checkout: checkout.to_path_buf(),
            source_repo: source_repo.to_path_buf(),
            split,
        })
    }

    /// A git command in the user's repository's configuration, over the
    /// checkout's objects and per-worktree refs, with the app's GitHub auth.
    fn command(&self) -> tokio::process::Command {
        let mut cmd = crate::git_dist::command(&self.source_repo);
        if let Some(split) = &self.split {
            cmd.env("GIT_DIR", &split.git_dir)
                .env("GIT_COMMON_DIR", &split.common_dir)
                .env("GIT_OBJECT_DIRECTORY", &split.objects);
        }
        for (key, value) in TRANSPORT_CONFIG {
            cmd.arg("-c").arg(format!("{key}={value}"));
        }
        super::cmd::apply_github_auth(&mut cmd);
        cmd.kill_on_drop(true);
        cmd
    }

    /// The fetch remote, as git in the user's repository would use it. The
    /// user's own repository is addressed as `origin`, so git keeps its
    /// tracking refs itself; a checkout's transport names the URL, so git
    /// keeps none in the user's repository.
    async fn fetch_remote(&self) -> Result<String> {
        if self.split.is_none() {
            return Ok("origin".to_string());
        }
        self.remote_urls(&["remote", "get-url", "origin"])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| Error::Git("no origin remote".into()))
    }

    /// Every push URL of the user's `origin` (`pushurl`s, else the fetch
    /// URL), in git's order. The user's own repository is addressed as
    /// `origin`, as for [`Self::fetch_remote`].
    pub(crate) async fn push_remotes(&self) -> Result<Vec<String>> {
        if self.split.is_none() {
            return Ok(vec!["origin".to_string()]);
        }
        self.remote_urls(&["remote", "get-url", "--push", "--all", "origin"])
            .await
    }

    async fn remote_urls(&self, args: &[&str]) -> Result<Vec<String>> {
        let mut cmd = self.command();
        cmd.args(args);
        let out = cmd.output().await?;
        if !out.status.success() {
            return Err(Error::Git(format!(
                "no origin remote: '{}' has none to publish to or fetch from",
                self.source_repo.display()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// Mirror `which` branches into the checkout's `refs/remotes/origin/*`
    /// (and, for [`Branches::All`], each push remote's branches into
    /// [`PUSHED`]), under the network timeout.
    pub async fn fetch_branches(&self, which: Branches<'_>) -> Result<()> {
        self.fetch_branches_within(which, super::cmd::NET_TIMEOUT)
            .await
    }

    /// [`Self::fetch_branches`] with its own deadline, for the spawn path.
    pub(crate) async fn fetch_branches_within(
        &self,
        which: Branches<'_>,
        timeout: Duration,
    ) -> Result<()> {
        let remote = self.fetch_remote().await?;
        match which {
            Branches::One(branch) => {
                let refspec = format!("+refs/heads/{branch}:{FETCHED}/{branch}");
                self.fetch(&remote, &[&refspec], false, timeout).await?;
                self.reconcile_tracking(Some(branch)).await
            }
            Branches::All => {
                self.fetch(
                    &remote,
                    &[&format!("+refs/heads/*:{FETCHED}/*")],
                    true,
                    timeout,
                )
                .await?;
                self.reconcile_tracking(None).await?;
                self.fetch_push_tips(timeout).await
            }
        }
    }

    /// Each push remote's branches into its [`PUSHED`] subtree, pruned; a
    /// push remote that is the fetch remote is copied from what was just
    /// fetched rather than asked again.
    async fn fetch_push_tips(&self, timeout: Duration) -> Result<()> {
        if self.split.is_none() {
            return Ok(());
        }
        let fetch_remote = self.fetch_remote().await?;
        for (i, url) in self.push_remotes().await?.iter().enumerate() {
            let subtree = format!("{PUSHED}/{i}");
            if *url == fetch_remote {
                self.copy_namespace(FETCHED, &subtree).await?;
            } else {
                self.fetch(url, &[&format!("+refs/heads/*:{subtree}/*")], true, timeout)
                    .await?;
            }
        }
        Ok(())
    }

    /// The commit a push's lease should expect at `branch` on the push
    /// remote `index`, refreshed from that remote right now: `None` when it
    /// has no such branch.
    pub(crate) async fn push_tip(&self, index: usize, branch: &str) -> Result<Option<String>> {
        let Some(url) = self.push_remotes().await?.get(index).cloned() else {
            return Ok(None);
        };
        let subtree = format!("{PUSHED}/{index}");
        if self.split.is_some() {
            self.fetch(
                &url,
                &[&format!("+refs/heads/*:{subtree}/*")],
                true,
                super::cmd::NET_TIMEOUT,
            )
            .await?;
            resolve(&self.checkout, &format!("{subtree}/{branch}")).await
        } else {
            // The user's own repository: git's tracking ref, as git keeps it.
            resolve(&self.checkout, &format!("refs/remotes/origin/{branch}")).await
        }
    }

    /// Record that `branch` on every push remote, and `origin/<branch>`, now
    /// stand at `commit` — what a named-remote push would have left behind.
    pub(crate) async fn record_pushed(&self, branch: &str, commit: &str) -> Result<()> {
        let mut script = format!("update refs/remotes/origin/{branch} {commit}\n");
        if self.split.is_some() {
            for i in 0..self.push_remotes().await?.len() {
                script.push_str(&format!("update {PUSHED}/{i}/{branch} {commit}\n"));
            }
        }
        self.update_refs(&script).await
    }

    /// Fetch `commit` itself into the checkout (`FETCH_HEAD` only): a tip whose
    /// branch is gone, still reachable on the remote.
    pub(crate) async fn fetch_commit(&self, commit: &str) -> Result<()> {
        let remote = self.fetch_remote().await?;
        self.fetch(&remote, &[commit], false, super::cmd::NET_TIMEOUT)
            .await
    }

    /// Whether `branch` exists on the fetch remote right now, as
    /// `ls-remote --exit-code` answers: the raw output on success, exit 2 for
    /// "no such ref", anything else a failure to ask.
    pub(crate) async fn ls_remote_head(&self, branch: &str) -> Result<std::process::Output> {
        let remote = self.fetch_remote().await?;
        let mut cmd = self.command();
        cmd.args([
            "ls-remote",
            "--exit-code",
            "--heads",
            &remote,
            &format!("refs/heads/{branch}"),
        ]);
        super::cmd::output_timed(&mut cmd, "git ls-remote").await
    }

    /// `git push` of `refspec` to push remote `index`, with `extra` arguments
    /// (a lease) ahead of it.
    pub(crate) async fn push(
        &self,
        index: usize,
        extra: &[String],
        refspec: &str,
    ) -> Result<std::process::Output> {
        let url = self
            .push_remotes()
            .await?
            .get(index)
            .cloned()
            .ok_or_else(|| Error::Git("no such push remote".into()))?;
        let mut cmd = self.command();
        cmd.args(["push", "--no-recurse-submodules"]);
        cmd.args(extra);
        cmd.args([&url, refspec]);
        super::cmd::output_timed(&mut cmd, "git push").await
    }

    async fn fetch(
        &self,
        remote: &str,
        refspecs: &[&str],
        prune: bool,
        timeout: Duration,
    ) -> Result<()> {
        let mut cmd = self.command();
        cmd.args(["fetch", "--quiet", "--no-recurse-submodules"]);
        if prune {
            cmd.arg("--prune");
        }
        cmd.arg(remote).args(refspecs);
        let out = tokio::time::timeout(timeout, cmd.output())
            .await
            .map_err(|_| {
                Error::Git(format!("git fetch timed out after {}s", timeout.as_secs()))
            })??;
        if !out.status.success() {
            return Err(Error::Git(format!(
                "fetch {} failed: {}",
                refspecs.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(())
    }

    /// `refs/remotes/origin/*` in the checkout set from [`FETCHED`]: one
    /// branch, or every branch with the rest pruned. The user's own
    /// repository fetched `origin` by name, so git did this itself.
    async fn reconcile_tracking(&self, branch: Option<&str>) -> Result<()> {
        if self.split.is_none() {
            return Ok(());
        }
        match branch {
            Some(branch) => {
                let fetched = resolve(&self.checkout, &format!("{FETCHED}/{branch}"))
                    .await?
                    .ok_or_else(|| Error::Git(format!("fetched nothing for {branch}")))?;
                self.update_refs(&format!("update refs/remotes/origin/{branch} {fetched}\n"))
                    .await
            }
            None => self.copy_namespace(FETCHED, "refs/remotes/origin").await,
        }
    }

    /// `to/*` made equal to `from/*`: every ref under `from` written under
    /// `to`, every other ref under `to` deleted (a symref such as
    /// `origin/HEAD` kept).
    async fn copy_namespace(&self, from: &str, to: &str) -> Result<()> {
        let wanted = self.refs_under(from).await?;
        let current = self.refs_under(to).await?;
        let mut script = String::new();
        for (name, sha) in &wanted {
            script.push_str(&format!("update {to}/{name} {sha}\n"));
        }
        for (name, _) in &current {
            if !wanted.iter().any(|(n, _)| n == name) {
                script.push_str(&format!("delete {to}/{name}\n"));
            }
        }
        self.update_refs(&script).await
    }

    /// `(name under prefix, commit)` for every non-symbolic ref under
    /// `prefix` in the checkout.
    async fn refs_under(&self, prefix: &str) -> Result<Vec<(String, String)>> {
        let out = git_output(
            &self.checkout,
            &[
                "for-each-ref",
                "--format=%(objectname) %(refname) %(symref)",
                &format!("{prefix}/"),
            ],
        )
        .await?;
        if !out.status.success() {
            return Err(Error::Git(format!(
                "for-each-ref {prefix} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| {
                let mut parts = line.split(' ');
                let sha = parts.next()?;
                let name = parts.next()?.strip_prefix(prefix)?.strip_prefix('/')?;
                let symref = parts.next().unwrap_or_default();
                symref
                    .is_empty()
                    .then(|| (name.to_string(), sha.to_string()))
            })
            .collect())
    }

    /// `git update-ref --stdin` in the checkout with `script`, all or nothing.
    async fn update_refs(&self, script: &str) -> Result<()> {
        if script.is_empty() {
            return Ok(());
        }
        let out = git_output_stdin(
            &self.checkout,
            &["update-ref", "--stdin"],
            &[],
            script.as_bytes(),
        )
        .await?;
        if !out.status.success() {
            return Err(Error::Git(format!(
                "updating the checkout's remote refs failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(())
    }
}

/// `rev` as a commit in `repo`, or `None` when it names nothing.
pub async fn resolve(repo: &Path, rev: &str) -> Result<Option<String>> {
    let spec = format!("{rev}^{{commit}}");
    let out = git_output(repo, &["rev-parse", "--verify", "--quiet", &spec]).await?;
    Ok(out
        .status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()))
}

async fn git_path(repo: &Path, what: &str) -> Result<PathBuf> {
    let out = git_output(repo, &["rev-parse", "--path-format=absolute", what]).await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "rev-parse {what} in '{}' failed: {}",
            repo.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

/// The checkout's `origin` must still name the remote the user's repository
/// names, in the https form provisioning wrote (both sides compared in that
/// form, since `git remote get-url` reports URLs after the user's own
/// `insteadOf` rewrites, which show on both alike). Transport never reads the
/// checkout's `origin`; this is the tamper check, and the pull-request reads
/// — which resolve the repository from the checkout's origin — rely on it.
/// Nothing to check when `checkout` is the user's repository itself.
pub async fn require_origin_unchanged(checkout: &Path, source_repo: &Path) -> Result<()> {
    let Some(source) = origin_url(source_repo).await? else {
        return Err(Error::Git(format!(
            "no origin remote: '{}' has none to publish to or fetch from",
            source_repo.display()
        )));
    };
    if checkout == source_repo {
        return Ok(());
    }
    let trusted = crate::github::github_https_remote(&source);
    match origin_url(checkout).await? {
        Some(url) if crate::github::github_https_remote(&url) == trusted => Ok(()),
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

/// `origin`'s fetch URL in `repo` as configured — the raw value, not what
/// `git remote get-url` reports after `insteadOf` rewrites, so a rewrite
/// planted in a checkout cannot dress a repointed origin up as the right one
/// (nor a user's global rewrite make an unchanged one look wrong). `None`
/// without an origin.
async fn origin_url(repo: &Path) -> Result<Option<String>> {
    let out = git_output(repo, &["config", "--get", "remote.origin.url"]).await?;
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

    /// A source repo with `origin` and a `--shared` checkout cloned from it,
    /// each with origin as provisioning leaves them; the pushed base commit.
    pub(crate) struct Repos {
        pub source: PathBuf,
        pub checkout: PathBuf,
        pub origin: PathBuf,
        pub base: String,
    }

    pub(crate) fn repos(dir: &Path) -> Repos {
        let origin = bare(dir, "origin.git");
        let source = dir.join("source");
        init_repo(&source);
        git(
            &source,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        let base = commit_file(&source, "base.txt");
        git(&source, &["push", "-q", "origin", "main"]);
        let checkout = dir.join("checkout");
        git(
            dir,
            &[
                "clone",
                "-q",
                "--shared",
                source.to_str().unwrap(),
                checkout.to_str().unwrap(),
            ],
        );
        git(
            &checkout,
            &["remote", "set-url", "origin", origin.to_str().unwrap()],
        );
        git(&checkout, &["config", "user.email", "me@example.com"]);
        git(&checkout, &["config", "user.name", "Me"]);
        Repos {
            source,
            checkout,
            origin,
            base,
        }
    }

    /// A bare remote whose default branch is `main`, so a clone of it is born
    /// on `main` once `main` is pushed.
    fn bare(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        git(dir, &["init", "-q", "--bare", path.to_str().unwrap()]);
        git(&path, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        path
    }

    fn branch_tip(bare: &Path, branch: &str) -> Option<String> {
        let out = super::super::testing::git_output(
            bare,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        );
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    #[tokio::test]
    async fn a_checkout_whose_origin_was_changed_is_refused() {
        let td = tempfile::tempdir().unwrap();
        let fx = repos(td.path());
        git(
            &fx.checkout,
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/victim/private.git",
            ],
        );
        let err = Transport::open(&fx.checkout, &fx.source).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("refusing"), "{msg}");
        assert!(msg.contains("victim/private"), "{msg}");
        assert!(
            msg.contains(fx.origin.to_str().unwrap()),
            "names the remote to restore: {msg}"
        );

        git(&fx.checkout, &["remote", "remove", "origin"]);
        let err = Transport::open(&fx.checkout, &fx.source).await.unwrap_err();
        assert!(err.to_string().contains("refusing"), "{err}");
    }

    #[tokio::test]
    async fn a_source_without_origin_has_no_remote_to_offer() {
        let td = tempfile::tempdir().unwrap();
        let fx = repos(td.path());
        git(&fx.source, &["remote", "remove", "origin"]);
        let err = Transport::open(&fx.checkout, &fx.source).await.unwrap_err();
        assert!(err.to_string().contains("no origin remote"), "{err}");
    }

    /// `git remote get-url` reports a URL after the user's own `insteadOf`
    /// rewrites, on both sides alike; the agreement check must see through
    /// them rather than report an unchanged checkout as repointed.
    #[tokio::test]
    async fn the_users_insteadof_rewrite_does_not_read_as_a_change() {
        let td = tempfile::tempdir().unwrap();
        let fx = repos(td.path());
        for repo in [&fx.source, &fx.checkout] {
            git(
                repo,
                &[
                    "remote",
                    "set-url",
                    "origin",
                    "https://github.com/me/repo.git",
                ],
            );
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
        require_origin_unchanged(&fx.checkout, &fx.source)
            .await
            .unwrap();
    }

    /// The whole point: transport reads the user's repository's config and
    /// nothing of the checkout's. A rewrite planted in the checkout — the
    /// kind the hardening refusal would catch in a real checkout — changes
    /// nothing; the user's own rewrite in their repository is honoured.
    #[tokio::test]
    async fn transport_reads_the_users_config_and_none_of_the_checkouts() {
        let td = tempfile::tempdir().unwrap();
        let fx = repos(td.path());
        let elsewhere = bare(td.path(), "elsewhere.git");
        git(
            &fx.source,
            &["push", "-q", elsewhere.to_str().unwrap(), "main:planted"],
        );
        // The agent's rewrite: origin's URL "means" elsewhere.
        git(
            &fx.checkout,
            &[
                "config",
                &format!("url.{}.insteadOf", elsewhere.display()),
                fx.origin.to_str().unwrap(),
            ],
        );
        let t = Transport::open(&fx.checkout, &fx.source).await.unwrap();
        t.fetch_branches(Branches::All).await.unwrap();
        let refs = git(
            &fx.checkout,
            &["for-each-ref", "--format=%(refname)", "refs/remotes/"],
        );
        assert!(refs.contains("refs/remotes/origin/main"), "{refs}");
        assert!(
            !refs.contains("planted"),
            "fetched from the real remote: {refs}"
        );

        // The user's own rewrite, in their repository, is what git reads.
        git(
            &fx.source,
            &[
                "config",
                &format!("url.{}.insteadOf", elsewhere.display()),
                fx.origin.to_str().unwrap(),
            ],
        );
        t.fetch_branches(Branches::All).await.unwrap();
        let refs = git(
            &fx.checkout,
            &["for-each-ref", "--format=%(refname)", "refs/remotes/"],
        );
        assert!(refs.contains("refs/remotes/origin/planted"), "{refs}");
    }

    /// What a fetch brings lands in the checkout — objects, `origin/*`,
    /// `FETCH_HEAD` — and nothing lands in the user's repository.
    #[tokio::test]
    async fn a_fetch_lands_in_the_checkout_and_leaves_the_users_repo_untouched() {
        let td = tempfile::tempdir().unwrap();
        let fx = repos(td.path());
        let other = td.path().join("other");
        git(
            td.path(),
            &[
                "clone",
                "-q",
                fx.origin.to_str().unwrap(),
                other.to_str().unwrap(),
            ],
        );
        git(&other, &["config", "user.email", "o@example.com"]);
        git(&other, &["config", "user.name", "O"]);
        let moved = commit_file(&other, "moved.txt");
        git(&other, &["push", "-q", "origin", "main"]);
        git(&other, &["checkout", "-q", "-b", "side"]);
        commit_file(&other, "side.txt");
        git(&other, &["push", "-q", "origin", "side"]);
        let source_refs_before = git(&fx.source, &["for-each-ref"]);

        let t = Transport::open(&fx.checkout, &fx.source).await.unwrap();
        t.fetch_branches(Branches::One("main")).await.unwrap();

        assert_eq!(
            git(&fx.checkout, &["rev-parse", "refs/remotes/origin/main"]),
            moved
        );
        assert!(fx.checkout.join(".git/FETCH_HEAD").exists());
        assert!(!fx.source.join(".git/FETCH_HEAD").exists());
        assert_eq!(git(&fx.source, &["for-each-ref"]), source_refs_before);
        assert!(
            fx.checkout
                .join(".git/objects")
                .join(&moved[..2])
                .join(&moved[2..])
                .exists(),
            "the object is the checkout's"
        );

        // Every branch, pruned: `side` appears, and a since-deleted branch goes.
        t.fetch_branches(Branches::All).await.unwrap();
        assert!(
            git(
                &fx.checkout,
                &["rev-parse", "--verify", "refs/remotes/origin/side"]
            )
            .len()
                == 40
        );
        git(&other, &["push", "-q", "origin", "--delete", "side"]);
        t.fetch_branches(Branches::All).await.unwrap();
        let out = super::super::testing::git_output(
            &fx.checkout,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "refs/remotes/origin/side",
            ],
        );
        assert!(!out.status.success(), "pruned");
        assert_eq!(git(&fx.source, &["for-each-ref"]), source_refs_before);
    }

    /// `fetch.recurseSubmodules` and `push.recurseSubmodules` — from the
    /// user's config, or anywhere — never make transport reach a submodule's
    /// remote: the checkout's `.git/modules/*` config is the agent's.
    #[tokio::test]
    async fn transport_never_recurses_into_submodules() {
        let td = tempfile::tempdir().unwrap();
        let fx = repos(td.path());
        // A submodule whose remote is "private": one secret commit, not yet
        // in the checkout; and a local submodule commit not yet pushed.
        let private = bare(td.path(), "private.git");
        let seed = td.path().join("seed");
        init_repo(&seed);
        commit_file(&seed, "sub.txt");
        git(&seed, &["push", "-q", private.to_str().unwrap(), "main"]);
        git(
            &fx.checkout,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                private.to_str().unwrap(),
                "sub",
            ],
        );
        git(&fx.checkout, &["commit", "-q", "-m", "add sub"]);
        let sub = fx.checkout.join("sub");
        git(&sub, &["config", "user.email", "me@example.com"]);
        git(&sub, &["config", "user.name", "Me"]);
        let sub_local = commit_file(&sub, "local.txt");
        git(&fx.checkout, &["add", "sub"]);
        let tip = {
            git(&fx.checkout, &["commit", "-q", "-m", "bump sub"]);
            git(&fx.checkout, &["rev-parse", "HEAD"])
        };
        let secret = commit_file(&seed, "secret.txt");
        git(&seed, &["push", "-q", private.to_str().unwrap(), "main"]);
        // Recursion asked for everywhere it can be.
        for repo in [&fx.source, &fx.checkout] {
            git(repo, &["config", "fetch.recurseSubmodules", "true"]);
            git(repo, &["config", "push.recurseSubmodules", "on-demand"]);
            git(repo, &["config", "submodule.recurse", "true"]);
        }

        let t = Transport::open(&fx.checkout, &fx.source).await.unwrap();
        t.fetch_branches(Branches::All).await.unwrap();
        let have = super::super::testing::git_output(&sub, &["cat-file", "-e", &secret]);
        assert!(
            !have.status.success(),
            "the submodule's remote was not fetched"
        );

        let out = t
            .push(0, &[], &format!("{tip}:refs/heads/main"))
            .await
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(tip, branch_tip(&fx.origin, "main").unwrap());
        assert_eq!(
            branch_tip(&private, "main").as_deref(),
            Some(secret.as_str()),
            "the submodule's remote was not pushed"
        );
        assert_ne!(sub_local, secret);
    }

    /// The user's `pushurl`s are where pushes go, every one of them; a force
    /// can lease on only one, so it needs exactly one.
    #[tokio::test]
    async fn pushes_go_to_every_push_url() {
        let td = tempfile::tempdir().unwrap();
        let fx = repos(td.path());
        let fork_a = bare(td.path(), "fork-a.git");
        let fork_b = bare(td.path(), "fork-b.git");
        git(
            &fx.source,
            &[
                "remote",
                "set-url",
                "--push",
                "origin",
                fork_a.to_str().unwrap(),
            ],
        );
        git(
            &fx.source,
            &[
                "remote",
                "set-url",
                "--push",
                "--add",
                "origin",
                fork_b.to_str().unwrap(),
            ],
        );
        let t = Transport::open(&fx.checkout, &fx.source).await.unwrap();
        assert_eq!(
            t.push_remotes().await.unwrap(),
            vec![
                fork_a.to_string_lossy().into_owned(),
                fork_b.to_string_lossy().into_owned()
            ]
        );
        let tip = commit_file(&fx.checkout, "one.txt");
        for i in 0..2 {
            let out = t
                .push(i, &[], &format!("{tip}:refs/heads/main"))
                .await
                .unwrap();
            assert!(out.status.success());
        }
        assert_eq!(branch_tip(&fork_a, "main").as_deref(), Some(tip.as_str()));
        assert_eq!(branch_tip(&fork_b, "main").as_deref(), Some(tip.as_str()));
        assert_eq!(
            branch_tip(&fx.origin, "main").as_deref(),
            Some(fx.base.as_str())
        );
        t.record_pushed("main", &tip).await.unwrap();
        assert_eq!(
            t.push_tip(0, "main").await.unwrap().as_deref(),
            Some(tip.as_str())
        );
        assert_eq!(
            t.push_tip(1, "main").await.unwrap().as_deref(),
            Some(tip.as_str())
        );
    }

    #[tokio::test]
    async fn the_users_own_repo_runs_plainly() {
        let td = tempfile::tempdir().unwrap();
        let fx = repos(td.path());
        let t = Transport::open(&fx.source, &fx.source).await.unwrap();
        assert!(t.split.is_none());
        assert_eq!(t.push_remotes().await.unwrap(), vec!["origin".to_string()]);
        let tip = commit_file(&fx.source, "one.txt");
        let out = t
            .push(0, &[], &format!("{tip}:refs/heads/main"))
            .await
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            branch_tip(&fx.origin, "main").as_deref(),
            Some(tip.as_str())
        );
        assert!(
            !fx.source.join(".git/refs/worktree").exists(),
            "no scratch refs in the user's repository"
        );
    }
}
