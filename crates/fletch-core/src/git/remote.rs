//! Transport from a checkout, run with the **user's repository's**
//! configuration, and the remotes' state read over the wire.
//!
//! A checkout's `.git` is the agent's to write: its config (`remote.*`,
//! `url.*.insteadOf`, `http.*`, `fetch.recurseSubmodules`, …), its submodule
//! configs under `.git/modules`, and every one of its refs. Host-side git runs
//! fetch, push and ls-remote with the user's credentials — the app's GitHub
//! token, and in the user's own repository their agent socket — so a transport
//! that read any of that from the checkout could be steered: an unapproved
//! `git_fetch` made to authenticate against whatever repository the agent
//! named, a push recursing into a submodule the agent pointed elsewhere, a
//! planted ref passing an unsigned commit off as published. Listing what to
//! refuse one key or one ref at a time is a race against git's surface.
//!
//! So transport reads **no** checkout configuration, and the decisions that
//! depend on a remote's state — what is already published, what a force
//! push's lease must expect — read that state from the remote itself
//! ([`Transport::published_tips`], [`Transport::push_tip`]), never from a ref
//! in the checkout. It runs the way the signer does (`git::signing`): in the
//! user's own repository, which no agent can write, with the checkout
//! supplying only what it must. Git's own worktree mechanism makes that
//! exact split:
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
//! The checkout's `refs/remotes/origin/*` is reconciled from what was fetched
//! by plain `update-ref` in the checkout, for the agent's own use; nothing
//! host-side trusts it. Recursion into submodules is switched off on every
//! invocation, and the agreement check that the checkout's own `origin` still
//! names the user's remote stays: a checkout an agent repointed is refused
//! outright rather than silently served from the real remote.
//!
//! The user's own repository, when it is what is pushed (a new project's first
//! publish), needs none of the split: git runs in it plainly, with `origin`
//! by name.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::cmd::{git_output, git_output_stdin};
use crate::error::{Error, Result};

/// Scratch namespace, per-worktree, a fetch lands branches in before they are
/// reconciled into `refs/remotes/origin/*`.
const FETCHED: &str = "refs/worktree/fletch/origin";

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

    /// Mirror `which` branches into the checkout's `refs/remotes/origin/*`,
    /// under the network timeout.
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
                // A ref named like an ancestor or a descendant of `branch`
                // (`feat` beside `feat/sub`) would block the one about to be
                // written, in the scratch namespace and the tracking one.
                for namespace in [FETCHED, "refs/remotes/origin"] {
                    self.clear_conflicting(namespace, branch).await?;
                }
                let refspec = format!("+refs/heads/{branch}:{FETCHED}/{branch}");
                self.fetch(&remote, &[&refspec], false, timeout).await?;
                self.reconcile_tracking(Some(branch)).await
            }
            Branches::All => {
                // Each push remote's branches too, so that what a fork alone
                // has is in the checkout's store — the published tips a
                // signed publish must not rewrite are then present locally.
                let mut remotes = vec![remote];
                for url in self.push_remotes().await? {
                    if !remotes.contains(&url) {
                        remotes.push(url);
                    }
                }
                for (i, url) in remotes.iter().enumerate() {
                    let subtree = if i == 0 {
                        FETCHED.to_string()
                    } else {
                        format!("{FETCHED}-push{i}")
                    };
                    self.fetch(url, &[&format!("+refs/heads/*:{subtree}/*")], true, timeout)
                        .await?;
                }
                self.reconcile_tracking(None).await
            }
        }
    }

    /// Every branch tip the fetch remote and each push remote advertise right
    /// now, read over the wire — the commits that are **published**, for a
    /// signed publish to stop at. Deliberately not read from any ref in the
    /// checkout, which the agent could write to pass an unsigned commit off as
    /// published.
    pub(crate) async fn published_tips(&self) -> Result<Vec<String>> {
        let mut remotes = vec![self.fetch_remote().await?];
        for url in self.push_remotes().await? {
            if !remotes.contains(&url) {
                remotes.push(url);
            }
        }
        let mut tips = BTreeSet::new();
        for remote in &remotes {
            for (_, sha) in self.heads(remote).await? {
                tips.insert(sha);
            }
        }
        Ok(tips.into_iter().collect())
    }

    /// The commit a push's lease should expect at `branch` on the push remote
    /// `index`, as that remote advertises it right now: `None` when it has no
    /// such branch.
    pub(crate) async fn push_tip(&self, index: usize, branch: &str) -> Result<Option<String>> {
        let Some(url) = self.push_remotes().await?.get(index).cloned() else {
            return Ok(None);
        };
        Ok(self
            .heads(&url)
            .await?
            .into_iter()
            .find(|(name, _)| name == branch)
            .map(|(_, sha)| sha))
    }

    /// `(branch, commit)` for every branch `remote` advertises.
    async fn heads(&self, remote: &str) -> Result<Vec<(String, String)>> {
        let mut cmd = self.command();
        cmd.args(["ls-remote", "--heads", remote]);
        let out = super::cmd::output_timed(&mut cmd, "git ls-remote").await?;
        if !out.status.success() {
            return Err(Error::Git(format!(
                "ls-remote {remote} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| {
                let (sha, name) = line.split_once('\t')?;
                let branch = name.trim().strip_prefix("refs/heads/")?;
                Some((branch.to_string(), sha.trim().to_string()))
            })
            .collect())
    }

    /// Record that `origin/<branch>` now stands at `commit` — what a
    /// named-remote push would have left behind, for the agent's view. The
    /// user's own repository pushed `origin` by name, so git did this itself.
    pub(crate) async fn record_pushed(&self, branch: &str, commit: &str) -> Result<()> {
        if self.split.is_none() {
            return Ok(());
        }
        self.clear_conflicting("refs/remotes/origin", branch)
            .await?;
        self.update_refs(&format!("update refs/remotes/origin/{branch} {commit}\n"))
            .await
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
    /// `origin/HEAD` kept). Deletions go first, on their own: a branch
    /// renamed from `feat` to `feat/sub` (or back) needs the old name gone
    /// before the new one can exist beneath or above it.
    async fn copy_namespace(&self, from: &str, to: &str) -> Result<()> {
        let wanted = self.refs_under(from).await?;
        let current = self.refs_under(to).await?;
        let deletions: String = current
            .iter()
            .filter(|(name, _)| !wanted.iter().any(|(n, _)| n == name))
            .map(|(name, _)| format!("delete {to}/{name}\n"))
            .collect();
        self.update_refs(&deletions).await?;
        let updates: String = wanted
            .iter()
            .map(|(name, sha)| format!("update {to}/{name} {sha}\n"))
            .collect();
        self.update_refs(&updates).await
    }

    /// Delete every ref under `namespace` that would stop `branch` from being
    /// written there: one named like an ancestor of it (`feat` for
    /// `feat/sub`) or a descendant (`feat/sub` for `feat`).
    async fn clear_conflicting(&self, namespace: &str, branch: &str) -> Result<()> {
        let script: String = self
            .refs_under(namespace)
            .await?
            .into_iter()
            .filter(|(name, _)| {
                branch.starts_with(&format!("{name}/")) || name.starts_with(&format!("{branch}/"))
            })
            .map(|(name, _)| format!("delete {namespace}/{name}\n"))
            .collect();
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
/// names, in the https form provisioning wrote. Transport never reads the
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
pub(crate) mod tests {
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
    pub(crate) fn bare(dir: &Path, name: &str) -> PathBuf {
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

    fn tracking_refs(checkout: &Path) -> String {
        git(
            checkout,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "refs/remotes/origin/",
            ],
        )
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
    /// rewrites; the agreement check reads the configured value, so a
    /// rewrite on both sides changes nothing.
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
        let refs = tracking_refs(&fx.checkout);
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
        let refs = tracking_refs(&fx.checkout);
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
        assert!(tracking_refs(&fx.checkout).contains("origin/side"));
        git(&other, &["push", "-q", "origin", "--delete", "side"]);
        t.fetch_branches(Branches::All).await.unwrap();
        assert!(!tracking_refs(&fx.checkout).contains("side"), "pruned");
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
        git(&fx.checkout, &["commit", "-q", "-m", "bump sub"]);
        let tip = git(&fx.checkout, &["rev-parse", "HEAD"]);
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

    /// The user's `pushurl`s are where pushes go, every one of them, and a
    /// force's lease asks the push remote itself for its tip.
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
        assert_eq!(t.push_tip(0, "main").await.unwrap(), None);
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
        assert_eq!(
            t.push_tip(1, "main").await.unwrap().as_deref(),
            Some(tip.as_str())
        );
    }

    /// What is published is what the remotes advertise, asked of them — and
    /// not any ref in the checkout, which the agent can write.
    #[tokio::test]
    async fn published_tips_come_from_the_remotes_not_the_checkouts_refs() {
        let td = tempfile::tempdir().unwrap();
        let fx = repos(td.path());
        let fork = bare(td.path(), "fork.git");
        git(
            &fx.source,
            &[
                "remote",
                "set-url",
                "--push",
                "origin",
                fork.to_str().unwrap(),
            ],
        );
        let fork_only = commit_file(&fx.source, "fork.txt");
        git(
            &fx.source,
            &["push", "-q", fork.to_str().unwrap(), "main:feat"],
        );
        git(&fx.source, &["reset", "-q", "--hard", &fx.base]);
        let planted = commit_file(&fx.checkout, "unsigned.txt");
        for refname in [
            "refs/remotes/origin/planted",
            "refs/worktree/fletch/origin/planted",
            "refs/worktree/fletch/push/0/planted",
        ] {
            git(&fx.checkout, &["update-ref", refname, &planted]);
        }

        let t = Transport::open(&fx.checkout, &fx.source).await.unwrap();
        let tips = t.published_tips().await.unwrap();

        assert_eq!(tips, {
            let mut want = vec![fx.base.clone(), fork_only.clone()];
            want.sort();
            want
        });
        assert!(!tips.contains(&planted));
    }

    /// A branch renamed to a name nested under its old one (or the reverse)
    /// reconciles, whether every branch is fetched or that one alone.
    #[tokio::test]
    async fn a_branch_replaced_by_a_nested_name_reconciles() {
        let td = tempfile::tempdir().unwrap();
        let fx = repos(td.path());
        let t = Transport::open(&fx.checkout, &fx.source).await.unwrap();
        git(&fx.source, &["push", "-q", "origin", "main:feat"]);
        t.fetch_branches(Branches::All).await.unwrap();
        assert!(tracking_refs(&fx.checkout).contains("origin/feat"));

        git(&fx.source, &["push", "-q", "origin", "--delete", "feat"]);
        git(&fx.source, &["push", "-q", "origin", "main:feat/sub"]);
        t.fetch_branches(Branches::All).await.unwrap();
        let refs = tracking_refs(&fx.checkout);
        assert!(refs.contains("refs/remotes/origin/feat/sub"), "{refs}");
        assert!(
            !refs.lines().any(|r| r == "refs/remotes/origin/feat"),
            "{refs}"
        );

        git(
            &fx.source,
            &["push", "-q", "origin", "--delete", "feat/sub"],
        );
        git(&fx.source, &["push", "-q", "origin", "main:feat"]);
        t.fetch_branches(Branches::One("feat")).await.unwrap();
        let refs = tracking_refs(&fx.checkout);
        assert!(
            refs.lines().any(|r| r == "refs/remotes/origin/feat"),
            "{refs}"
        );
        assert!(!refs.contains("feat/sub"), "{refs}");

        git(&fx.source, &["push", "-q", "origin", "--delete", "feat"]);
        git(&fx.source, &["push", "-q", "origin", "main:feat/sub"]);
        t.fetch_branches(Branches::One("feat/sub")).await.unwrap();
        let refs = tracking_refs(&fx.checkout);
        assert!(refs.contains("refs/remotes/origin/feat/sub"), "{refs}");
        assert!(
            !refs.lines().any(|r| r == "refs/remotes/origin/feat"),
            "{refs}"
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
