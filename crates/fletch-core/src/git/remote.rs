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
//! ([`Transport::published_tips`], [`Transport::tip`]), never from a ref in
//! the checkout. It runs in the user's own repository, which no agent can
//! write, with the checkout supplying only what it must. Git's own worktree
//! mechanism makes that exact split:
//!
//! - `GIT_COMMON_DIR` is the user's repository's `.git`, so configuration —
//!   the remote, its push URLs, the user's `insteadOf` rewrites and proxies and
//!   credential helpers — is the user's, exactly as their terminal reads it;
//! - `GIT_OBJECT_DIRECTORY` is the checkout's object store, so what a fetch
//!   brings lands in the checkout (and what a push sends is read from it);
//! - `GIT_DIR` is the checkout's, so per-worktree refs (`refs/worktree/*`)
//!   land in the checkout — and the user's refs are never
//!   touched, because every fetch and push names an explicit URL (taken from
//!   the user's config) rather than `origin`, so git maintains no
//!   remote-tracking ref in the common dir.
//!
//! One file in `GIT_DIR` is configuration: `config.worktree`, read when the
//! user's repository enables `extensions.worktreeConfig`, and then ranking
//! above the user's own config. A checkout that has one is refused at
//! [`Transport::open`]; Fletch never writes one there.
//!
//! The signer (`git::signing`) takes the object store alone
//! ([`Transport::object_command`]): it runs in the user's repository as
//! plainly as their own `git commit` there would.
//!
//! The checkout's `refs/remotes/origin/*` is then set from what was fetched by
//! git in the checkout itself — a fetch from `.` — for the agent's own use;
//! nothing host-side trusts it. The agreement check that the checkout's own
//! `origin` still names the user's remote stays: a checkout an agent repointed
//! is refused outright rather than silently served from the real remote.
//!
//! The user's own repository, when it is what is pushed (a new project's first
//! publish), needs none of the split: git runs in it plainly, with `origin`
//! by name.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::cmd::{git_output, git_output_stdin, output_with_stdin, NET_TIMEOUT};
use crate::error::{Error, Result};

/// Per-worktree namespace a fetch of every branch lands in before the
/// checkout's `origin/*` is set from it.
const FETCHED: &str = "refs/worktree/fletch/origin";

/// The checkout's remote-tracking namespace for `origin`.
const TRACKING: &str = "refs/remotes/origin";

/// Configuration every transport invocation carries, over the user's own.
/// Maintenance is off because this git sees the user's config over the
/// checkout's objects, a mixture nothing should repack. The GitHub rewrite
/// keeps a GitHub ssh remote on https, where the app's token authenticates:
/// the rule [`crate::github::github_https_remote`] states, which provisioning
/// writes into checkouts and the agreement check compares by, applied here to
/// the user's URLs; a user's own longer-matching `insteadOf` still wins, as
/// git ranks them. Recursion into submodules is off on every spawn
/// (`hardening::NEUTRALISED`).
const TRANSPORT_CONFIG: &[(&str, &str)] = &[
    ("gc.auto", "0"),
    ("maintenance.auto", "false"),
    ("fetch.writeCommitGraph", "false"),
    ("url.https://github.com/.insteadOf", "git@github.com:"),
    ("url.https://github.com/.insteadOf", "ssh://git@github.com/"),
];

/// Transport for one checkout, bound to the user's repository it was cloned
/// from. Opening it runs the config refusal on the checkout and the
/// agreement check on its `origin`, and reads the remotes from the user's
/// config once.
#[derive(Debug)]
pub struct Transport {
    checkout: PathBuf,
    source_repo: PathBuf,
    /// `None` when the checkout *is* the user's repository.
    split: Option<Split>,
    /// The URL of the user's `origin`; `origin` itself in the user's own
    /// repository, so git keeps its tracking refs there.
    fetch_remote: String,
    /// Every push URL of the user's `origin` (`pushurl`s, else the URL), in
    /// git's order; `[origin]` in the user's own repository.
    push_remotes: Vec<String>,
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
    /// Every branch of the fetch remote, pruning what is gone, and the tips
    /// of each push remote. The view a publish needs.
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
            let git_dir = git_path(checkout, "--absolute-git-dir").await?;
            // The one file git reads from `GIT_DIR` as configuration: when
            // the user's repository enables `extensions.worktreeConfig`,
            // `config.worktree` there outranks the user's config — every
            // key, the remote URL included — and it is the agent's to
            // write. No checkout Fletch makes has one, so one is refused
            // rather than read.
            let worktree_config = git_dir.join("config.worktree");
            if worktree_config.metadata().is_ok_and(|meta| meta.len() > 0) {
                return Err(Error::Git(format!(
                    "refusing to fetch or push: '{}' exists, and git would read it as the \
                     repository's configuration. Fletch's host-side git takes its configuration \
                     from your repository only; remove the file",
                    worktree_config.display()
                )));
            }
            Some(Split {
                git_dir,
                common_dir: git_path(source_repo, "--git-common-dir").await?,
                objects: git_path(checkout, "--git-common-dir")
                    .await?
                    .join("objects"),
            })
        };
        let mut t = Self {
            checkout: checkout.to_path_buf(),
            source_repo: source_repo.to_path_buf(),
            split,
            fetch_remote: "origin".to_string(),
            push_remotes: vec!["origin".to_string()],
        };
        if t.split.is_some() {
            t.fetch_remote = t.origin_urls(&[]).await?.swap_remove(0);
            t.push_remotes = t.origin_urls(&["--push", "--all"]).await?;
        }
        Ok(t)
    }

    /// `git remote get-url <extra> origin` in the user's repository: never
    /// empty.
    async fn origin_urls(&self, extra: &[&str]) -> Result<Vec<String>> {
        let mut cmd = self.command();
        cmd.args(["remote", "get-url"]).args(extra).arg("origin");
        let out = cmd.output().await?;
        let urls: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect();
        if !out.status.success() || urls.is_empty() {
            return Err(Error::Git(format!(
                "no origin remote: '{}' has none to publish to or fetch from",
                self.source_repo.display()
            )));
        }
        Ok(urls)
    }

    /// A git command in the user's repository over the checkout's object
    /// store and nothing else of the checkout's — no `GIT_DIR`, so not even a
    /// per-worktree config file can reach it. The signer's command.
    pub(crate) fn object_command(&self) -> tokio::process::Command {
        let mut cmd = crate::git_dist::command(&self.source_repo);
        if let Some(split) = &self.split {
            cmd.env("GIT_OBJECT_DIRECTORY", &split.objects);
        }
        cmd.kill_on_drop(true);
        cmd
    }

    /// A git command in the user's repository's configuration, over the
    /// checkout's objects and per-worktree refs, with the app's GitHub auth.
    fn command(&self) -> tokio::process::Command {
        let mut cmd = self.object_command();
        if let Some(split) = &self.split {
            cmd.env("GIT_DIR", &split.git_dir)
                .env("GIT_COMMON_DIR", &split.common_dir);
        }
        for (key, value) in TRANSPORT_CONFIG {
            cmd.arg("-c").arg(format!("{key}={value}"));
        }
        super::cmd::apply_github_auth(&mut cmd);
        cmd
    }

    /// Where fetches go (see the field).
    pub(crate) fn fetch_remote(&self) -> &str {
        &self.fetch_remote
    }

    /// Where pushes go (see the field).
    pub(crate) fn push_remotes(&self) -> &[String] {
        &self.push_remotes
    }

    /// The fetch remote, then each push remote it is not.
    fn remotes(&self) -> Vec<&str> {
        let mut all = vec![self.fetch_remote.as_str()];
        for url in &self.push_remotes {
            if !all.contains(&url.as_str()) {
                all.push(url);
            }
        }
        all
    }

    /// Mirror `which` branches into the checkout's `refs/remotes/origin/*`,
    /// under the network timeout.
    pub async fn fetch_branches(&self, which: Branches<'_>) -> Result<()> {
        self.fetch_branches_within(which, NET_TIMEOUT).await
    }

    /// [`Self::fetch_branches`] with its own deadline, for the spawn path.
    pub(crate) async fn fetch_branches_within(
        &self,
        which: Branches<'_>,
        timeout: Duration,
    ) -> Result<()> {
        match which {
            Branches::One(branch) => {
                let refspec = format!("refs/heads/{branch}");
                if self.split.is_none() {
                    // Fetched from `origin` by name, git moves the tracking
                    // ref itself once nothing blocks it.
                    self.clear_conflicting(branch).await?;
                    return self
                        .fetch(&self.fetch_remote, &[refspec], false, timeout)
                        .await;
                }
                // Into a scratch ref of this fetch's own, never `FETCH_HEAD`:
                // that file is one per checkout, and a fetch the agent runs
                // alongside (its own `git_fetch`, say) would overwrite it
                // between our fetch and our read, and another branch's tip
                // would land under `origin/<branch>`. The scratch name is
                // flat, so nothing stale can block it.
                let scratch = format!(
                    "refs/worktree/fletch/fetch/{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_nanos())
                        .unwrap_or_default()
                );
                self.fetch(
                    &self.fetch_remote,
                    &[format!("+{refspec}:{scratch}")],
                    false,
                    timeout,
                )
                .await?;
                let fetched = resolve(&self.checkout, &scratch).await?;
                self.update_refs(&format!("option no-deref\ndelete {scratch}\n"))
                    .await?;
                let fetched =
                    fetched.ok_or_else(|| Error::Git(format!("fetched nothing for {branch}")))?;
                self.set_tracking(branch, &fetched).await
            }
            Branches::All => {
                let into = if self.split.is_some() {
                    FETCHED
                } else {
                    TRACKING
                };
                self.fetch(
                    &self.fetch_remote,
                    &[format!("+refs/heads/*:{into}/*")],
                    true,
                    timeout,
                )
                .await?;
                if self.split.is_some() {
                    self.mirror_fetched().await?;
                }
                // Each push remote's branches too, by name and for their
                // objects only (a name, not an id: every server serves an
                // advertised ref, not every one an unadvertised want), so
                // that what a fork alone has is in the checkout's store: the
                // published tips a signed publish stops at must be present
                // locally.
                for url in self.remotes().into_iter().skip(1) {
                    let branches: Vec<String> = self
                        .heads(url, None)
                        .await?
                        .into_iter()
                        .map(|(name, _)| format!("refs/heads/{name}"))
                        .collect();
                    if !branches.is_empty() {
                        self.fetch(url, &branches, false, timeout).await?;
                    }
                }
                Ok(())
            }
        }
    }

    /// The checkout's `origin/*` made equal to [`FETCHED`] by git in the
    /// checkout, fetching from itself: pruning comes before updating, so a
    /// branch renamed from `feat` to `feat/sub` (or back) lands, and a
    /// symref such as `origin/HEAD` is kept.
    async fn mirror_fetched(&self) -> Result<()> {
        let refspec = format!("+{FETCHED}/*:{TRACKING}/*");
        let out = git_output(
            &self.checkout,
            &[
                "fetch",
                "--quiet",
                "--prune",
                "--no-write-fetch-head",
                ".",
                &refspec,
            ],
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

    /// Every branch tip the fetch remote and each push remote advertise right
    /// now, read over the wire — the commits that are **published**, for a
    /// signed publish to stop at. Deliberately not read from any ref in the
    /// checkout, which the agent could write to pass an unsigned commit off as
    /// published.
    pub(crate) async fn published_tips(&self) -> Result<Vec<String>> {
        let mut tips = BTreeSet::new();
        for remote in self.remotes() {
            tips.extend(
                self.heads(remote, None)
                    .await?
                    .into_iter()
                    .map(|(_, sha)| sha),
            );
        }
        Ok(tips.into_iter().collect())
    }

    /// The commit `remote` (the fetch remote or one of the push remotes)
    /// advertises at exactly `refs/heads/<branch>` right now; `None` when it
    /// has no such branch. A glob is no branch: the pattern `ls-remote` is
    /// given can match others, but only a verbatim name counts.
    pub(crate) async fn tip(&self, remote: &str, branch: &str) -> Result<Option<String>> {
        Ok(self
            .heads(remote, Some(branch))
            .await?
            .into_iter()
            .find(|(name, _)| name == branch)
            .map(|(_, sha)| sha))
    }

    /// `(branch, commit)` for every branch `remote` advertises, or for those
    /// matching `refs/heads/<branch>`. The pattern is always prefixed, so a
    /// branch name never reaches git in an option position.
    async fn heads(&self, remote: &str, branch: Option<&str>) -> Result<Vec<(String, String)>> {
        let mut cmd = self.command();
        cmd.args(["ls-remote", "--heads", remote]);
        if let Some(branch) = branch {
            cmd.arg(format!("refs/heads/{branch}"));
        }
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
        self.set_tracking(branch, commit).await
    }

    /// Fetch `commit` itself into the checkout's store, no ref: a tip whose
    /// branch is gone, still reachable on the remote.
    pub(crate) async fn fetch_commit(&self, commit: &str) -> Result<()> {
        self.fetch(
            &self.fetch_remote,
            &[commit.to_string()],
            false,
            NET_TIMEOUT,
        )
        .await
    }

    /// `git push` of `refspec` to `url`, one of [`Self::push_remotes`], with
    /// `extra` arguments (a lease) ahead of it.
    pub(crate) async fn push(
        &self,
        url: &str,
        extra: &[String],
        refspec: &str,
    ) -> Result<std::process::Output> {
        let mut cmd = self.command();
        cmd.arg("push").args(extra).args([url, refspec]);
        super::cmd::output_timed(&mut cmd, "git push").await
    }

    /// `git fetch` from `remote`, the refspecs on stdin (a push remote can
    /// advertise more tips than an argument list holds), and never writing
    /// `FETCH_HEAD`: nothing here reads it, and it is shared with whatever
    /// else fetches in the checkout.
    async fn fetch(
        &self,
        remote: &str,
        refspecs: &[String],
        prune: bool,
        timeout: Duration,
    ) -> Result<()> {
        let mut cmd = self.command();
        cmd.args(["fetch", "--quiet", "--stdin", "--no-write-fetch-head"]);
        if prune {
            cmd.arg("--prune");
        }
        cmd.arg(remote);
        let input: String = refspecs.iter().map(|r| format!("{r}\n")).collect();
        let out = output_with_stdin(cmd, input.as_bytes(), timeout, "git fetch").await?;
        if !out.status.success() {
            return Err(Error::Git(format!(
                "fetch from {remote} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(())
    }

    /// `refs/remotes/origin/<branch>` in the checkout set to `commit`.
    async fn set_tracking(&self, branch: &str, commit: &str) -> Result<()> {
        self.clear_conflicting(branch).await?;
        self.update_refs(&format!("update {TRACKING}/{branch} {commit}\n"))
            .await
    }

    /// Delete what would stop `refs/remotes/origin/<branch>` from being
    /// written in the checkout: a ref named like an ancestor of it (`feat`
    /// for `feat/sub`) or a descendant (`feat/sub` for `feat`). A fetch of
    /// one branch does not prune, so a rename leaves one behind.
    async fn clear_conflicting(&self, branch: &str) -> Result<()> {
        let descendants = format!("{TRACKING}/{branch}/");
        let out = git_output(
            &self.checkout,
            &["for-each-ref", "--format=%(refname)", &descendants],
        )
        .await?;
        let ancestors = branch
            .match_indices('/')
            .map(|(at, _)| format!("{TRACKING}/{}", &branch[..at]));
        let script: String = ancestors
            .chain(
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .map(str::to_string),
            )
            .map(|name| format!("option no-deref\ndelete {name}\n"))
            .collect();
        self.update_refs(&script).await
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
/// checkout's `origin`; this is the tamper check, run once, by
/// [`Transport::open`]. Nothing to check when `checkout` is the user's
/// repository itself.
async fn require_origin_unchanged(checkout: &Path, source_repo: &Path) -> Result<()> {
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
    use super::super::testing::{bare, clone_elsewhere, commit_file, git, init_repo, repos};
    use super::*;

    /// `FETCH_HEAD` is one file per checkout, shared with whatever else
    /// fetches there; a single-branch fetch takes its tip from a ref of its
    /// own, never from that file, and leaves it alone.
    #[tokio::test]
    async fn a_single_branch_fetch_never_reads_or_writes_fetch_head() {
        let td = tempfile::tempdir().unwrap();
        let fx = repos(td.path());
        let other = clone_elsewhere(td.path(), &fx.origin, "other");
        let moved = commit_file(&other, "moved.txt");
        git(&other, &["push", "-q", "origin", "main"]);
        // What a concurrent fetch of another branch would leave behind.
        let planted = format!("{}\t\tbranch 'side' of elsewhere\n", fx.base);
        std::fs::write(fx.checkout.join(".git/FETCH_HEAD"), &planted).unwrap();

        let t = Transport::open(&fx.checkout, &fx.source).await.unwrap();
        t.fetch_branches(Branches::One("main")).await.unwrap();

        assert_eq!(
            git(&fx.checkout, &["rev-parse", "refs/remotes/origin/main"]),
            moved
        );
        assert_eq!(
            std::fs::read_to_string(fx.checkout.join(".git/FETCH_HEAD")).unwrap(),
            planted,
            "left to whoever wrote it"
        );
        assert!(
            git(
                &fx.checkout,
                &["for-each-ref", "refs/worktree/fletch/fetch/"]
            )
            .is_empty(),
            "the scratch ref is gone"
        );
    }

    /// With `extensions.worktreeConfig` in the user's repository, git reads
    /// the checkout's `config.worktree` above the user's config — the remote
    /// URL included. A checkout that has one is refused, not read.
    #[tokio::test]
    async fn a_checkout_with_a_worktree_config_is_refused() {
        let td = tempfile::tempdir().unwrap();
        let fx = repos(td.path());
        git(&fx.source, &["config", "extensions.worktreeConfig", "true"]);
        std::fs::write(
            fx.checkout.join(".git/config.worktree"),
            "[remote \"origin\"]\n\turl = https://github.com/victim/private.git\n",
        )
        .unwrap();
        let err = Transport::open(&fx.checkout, &fx.source).await.unwrap_err();
        assert!(err.to_string().contains("config.worktree"), "{err}");

        std::fs::write(fx.checkout.join(".git/config.worktree"), "").unwrap();
        let t = Transport::open(&fx.checkout, &fx.source).await.unwrap();
        assert_eq!(
            t.fetch_remote(),
            fx.origin.to_str().unwrap(),
            "an empty one is nothing"
        );
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
    /// and nothing lands in the user's repository, not even `FETCH_HEAD`.
    #[tokio::test]
    async fn a_fetch_lands_in_the_checkout_and_leaves_the_users_repo_untouched() {
        let td = tempfile::tempdir().unwrap();
        let fx = repos(td.path());
        let other = clone_elsewhere(td.path(), &fx.origin, "other");
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
            .push(&t.push_remotes()[0], &[], &format!("{tip}:refs/heads/main"))
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
        let (a, b) = (
            fork_a.to_string_lossy().into_owned(),
            fork_b.to_string_lossy().into_owned(),
        );
        assert_eq!(t.push_remotes(), [a.clone(), b.clone()]);
        assert_eq!(t.tip(&a, "main").await.unwrap(), None);
        let tip = commit_file(&fx.checkout, "one.txt");
        for url in t.push_remotes() {
            let out = t
                .push(url, &[], &format!("{tip}:refs/heads/main"))
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
            t.tip(&b, "main").await.unwrap().as_deref(),
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
        assert_eq!(t.push_remotes(), ["origin".to_string()]);
        let tip = commit_file(&fx.source, "one.txt");
        let out = t
            .push("origin", &[], &format!("{tip}:refs/heads/main"))
            .await
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            branch_tip(&fx.origin, "main").as_deref(),
            Some(tip.as_str())
        );
        git(&fx.source, &["push", "-q", "origin", "main:side"]);
        t.fetch_branches(Branches::All).await.unwrap();
        t.fetch_branches(Branches::One("main")).await.unwrap();
        assert!(tracking_refs(&fx.source).contains("refs/remotes/origin/side"));
        assert!(
            !fx.source.join(".git/refs/worktree").exists(),
            "no scratch refs in the user's repository"
        );
    }
}
