//! Turn checkpoints for an agent's workspace: before a turn reaches the agent,
//! pin every checkout as it stands (`git::checkpoint`), so the code as of that
//! message outlives the edits the turn goes on to make. A new workspace can
//! start from a workspace's pinned code ([`PinnedCode`]): a fork's code.

use std::path::PathBuf;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::git::checkpoint;

use super::Supervisor;

/// What to do instead when a fork can't have the code it asked for.
pub(super) const OTHER_CODE: &str = "Fork with \"Current code\" or \"Clean from base\" instead.";

/// How long a turn's checkpoint may hold up its delivery, across all of the
/// workspace's checkouts. A snapshot normally takes well under a second; this
/// bounds a pathological checkout (a vast untracked tree, a wedged filesystem)
/// that git's own 120s-per-command cap would let stall the send. On expiry the
/// running git is killed and the turn goes out without a checkpoint.
pub(super) const CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);

/// One checkout of a workspace and its checkpoint for a turn.
#[derive(Debug, Clone)]
pub struct RepoCheckpoint {
    /// The tracked repo's subdir within the workspace (`TrackedRepo::subdir`).
    pub subdir: String,
    pub checkout: PathBuf,
    /// The snapshot commit, or `None` when the checkout has no checkpoint for
    /// the turn: it was attached after the turn, or capture was skipped or
    /// failed.
    pub sha: Option<String>,
}

/// The code of every checkout of one workspace, each pinned in its checkout
/// under one key (`git::checkpoint`): what a new workspace starts from, a
/// fork's code. Only ever built whole — by [`Supervisor::pin_code`] or
/// [`Supervisor::code_at`] — so a fork can't start from part of a
/// workspace's code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedCode {
    /// The key every snapshot is pinned under: a turn id, or a key of its own
    /// for code pinned on demand.
    key: String,
    /// One per repo of the workspace.
    checkouts: Vec<PinnedCheckout>,
}

/// One checkout holding its snapshot of a [`PinnedCode`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedCheckout {
    /// The repo it is a checkout of (`TrackedRepo::repo_path`): a new
    /// workspace's checkout of the same repo starts from it. Not the subdir,
    /// which each workspace names on its own and so can differ between them.
    pub repo_path: PathBuf,
    /// Its subdir in the workspace it belongs to, to name it by.
    pub subdir: String,
    pub checkout: PathBuf,
}

impl PinnedCode {
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn checkouts(&self) -> &[PinnedCheckout] {
        &self.checkouts
    }
}

impl Supervisor {
    /// Checkpoint every checkout of `agent_id` under `turn_id`. Called as a
    /// turn is delivered, before the agent sees it (`deliver_as_turn`).
    ///
    /// Best-effort: a checkout that can't be captured (a config the hardening
    /// refuses, a vanished directory) is logged and skipped, and the whole
    /// capture gives up after [`CAPTURE_TIMEOUT`]; the send goes ahead either
    /// way. Nothing is captured when there is no settled tree:
    /// - mid-turn or mid-spawn — `deliver_as_turn` then holds the message
    ///   rather than start its turn;
    /// - a workflow step agent (`owner_run_id`) works in a tree its run shares.
    ///
    /// Live-injected messages and native PTY typing never get here.
    pub(super) async fn checkpoint_turn(&self, agent_id: &str, turn_id: &str) {
        let Ok(record) = self.workspace.agent(agent_id) else {
            return;
        };
        if record.owner_run_id.is_some() || self.is_busy(agent_id) {
            return;
        }
        let capture_all = async {
            for repo in &record.repos {
                let captured = match repo.checkout_path(agent_id) {
                    Ok(checkout) => checkpoint::capture(&checkout, turn_id).await,
                    Err(e) => Err(e),
                };
                if let Err(e) = captured {
                    tracing::warn!(error = %e, agent_id, turn_id, subdir = %repo.subdir, "turn checkpoint failed");
                }
            }
        };
        if tokio::time::timeout(CAPTURE_TIMEOUT, capture_all)
            .await
            .is_err()
        {
            tracing::warn!(
                agent_id,
                turn_id,
                "turn checkpoint timed out after {}s; delivering without it",
                CAPTURE_TIMEOUT.as_secs()
            );
        }
    }

    /// Each of `agent_id`'s checkouts with its checkpoint for `turn_id`, in
    /// `repos` order — what fork ("code as of this message") and rewind
    /// restore from.
    pub async fn turn_checkpoints(
        &self,
        agent_id: &str,
        turn_id: &str,
    ) -> Result<Vec<RepoCheckpoint>> {
        let record = self.workspace.agent(agent_id)?;
        let mut checkpoints = Vec::with_capacity(record.repos.len());
        for repo in &record.repos {
            let checkout = repo.checkout_path(agent_id)?;
            let sha = checkpoint::resolve(&checkout, turn_id).await?;
            checkpoints.push(RepoCheckpoint {
                subdir: repo.subdir.clone(),
                checkout,
                sha,
            });
        }
        Ok(checkpoints)
    }

    /// `agent_id`'s current code: every checkout pinned as it stands now,
    /// under one fresh key. Fails if any checkout can't be captured.
    pub async fn pin_code(&self, agent_id: &str) -> Result<PinnedCode> {
        let record = self.workspace.agent(agent_id)?;
        if record.repos.is_empty() {
            return Err(Error::Other(format!("{agent_id} has no checkouts")));
        }
        let key = uuid::Uuid::new_v4().to_string();
        let mut checkouts = Vec::with_capacity(record.repos.len());
        for repo in &record.repos {
            let checkout = repo.checkout_path(agent_id)?;
            checkpoint::capture(&checkout, &key).await?;
            checkouts.push(PinnedCheckout {
                repo_path: repo.repo_path.clone(),
                subdir: repo.subdir.clone(),
                checkout,
            });
        }
        Ok(PinnedCode { key, checkouts })
    }

    /// The code `agent_id`'s checkouts held as `turn_id` was delivered: its
    /// checkpoint in every one of them. Lookup errors propagate; a checkout
    /// without the checkpoint (attached after the turn, or capture skipped or
    /// failed) refuses the whole, naming the repos it's missing for, and so
    /// does a workspace whose checkouts are gone (archived).
    pub async fn code_at(&self, agent_id: &str, turn_id: &str) -> Result<PinnedCode> {
        let record = self.workspace.agent(agent_id)?;
        if record.repos.is_empty() {
            return Err(Error::Other(format!(
                "The code as of this message isn't available: the workspace that ran it \
                 ({agent_id}) no longer has its checkouts. {OTHER_CODE}"
            )));
        }
        let mut checkouts = Vec::with_capacity(record.repos.len());
        let mut missing = Vec::new();
        for repo in &record.repos {
            let checkout = repo.checkout_path(agent_id)?;
            match checkpoint::resolve(&checkout, turn_id).await? {
                Some(_) => checkouts.push(PinnedCheckout {
                    repo_path: repo.repo_path.clone(),
                    subdir: repo.subdir.clone(),
                    checkout,
                }),
                None => missing.push(repo.subdir.as_str()),
            }
        }
        if !missing.is_empty() {
            return Err(Error::Other(format!(
                "The code as of this message wasn't kept for {}. {OTHER_CODE}",
                missing.join(", ")
            )));
        }
        Ok(PinnedCode {
            key: turn_id.to_string(),
            checkouts,
        })
    }

    /// Start `agent_id`'s checkouts from `code`, each from the snapshot of the
    /// same repo: its working tree becomes the snapshot's, and its HEAD the
    /// commit the snapshot was taken on. Committed work stays committed and
    /// uncommitted work uncommitted, as it was in the source.
    ///
    /// Every repo in `code` must have a checkout here, or its code would be
    /// dropped: that is refused before anything changes, naming the repos (a
    /// repo added to the source workspace by hand, or an ancestor's from
    /// another project, is not one this workspace checks out). A checkout here
    /// with no repo in `code` keeps its clean base, and can't do otherwise:
    /// `code` holds every checkout of its workspace, so that workspace never
    /// had the repo (it joined the project later) and there is no code of it
    /// to copy.
    pub async fn start_from(&self, agent_id: &str, code: &PinnedCode) -> Result<()> {
        let record = self.workspace.agent(agent_id)?;
        let dropped: Vec<&str> = code
            .checkouts
            .iter()
            .filter(|pinned| !record.repos.iter().any(|r| r.repo_path == pinned.repo_path))
            .map(|pinned| pinned.subdir.as_str())
            .collect();
        if !dropped.is_empty() {
            return Err(Error::Other(format!(
                "This workspace has no checkout of {}, so its code can't be carried over. \
                 {OTHER_CODE}",
                dropped.join(", ")
            )));
        }
        for repo in &record.repos {
            let Some(pinned) = code
                .checkouts
                .iter()
                .find(|p| p.repo_path == repo.repo_path)
            else {
                continue;
            };
            let checkout = repo.checkout_path(agent_id)?;
            let sha = checkpoint::fetch_into(&checkout, &pinned.checkout, &code.key).await?;
            checkpoint::restore(&checkout, &sha).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::run_git;
    use crate::supervisor::tests::{committed_repo, record_in_checkouts, test_supervisor};
    use crate::workspace::AgentStatus;
    use std::path::Path;

    const TURN: &str = "2c9d7e41-5a0b-4f6e-8d13-9b7a6c5e4f30";

    #[tokio::test]
    async fn every_checkout_of_the_workspace_is_checkpointed() {
        let td = tempfile::tempdir().unwrap();
        let a = committed_repo(td.path(), "a").await;
        let b = committed_repo(td.path(), "b").await;
        std::fs::write(a.join("wip.txt"), b"uncommitted").unwrap();
        let sup = test_supervisor();
        let mut record = record_in_checkouts(&sup, "denali", &[a.clone(), b.clone()]);
        sup.workspace.add_agent(&mut record).unwrap();

        sup.checkpoint_turn("denali", TURN).await;

        let checkpoints = sup.turn_checkpoints("denali", TURN).await.unwrap();
        let expected = [("repo-0", &a), ("repo-1", &b)];
        assert_eq!(checkpoints.len(), expected.len());
        for (found, (subdir, checkout)) in checkpoints.iter().zip(expected) {
            assert_eq!(found.subdir, subdir);
            assert_eq!(&found.checkout, checkout);
            assert!(found.sha.is_some(), "{subdir} has a checkpoint");
            assert_eq!(
                found.sha,
                checkpoint::resolve(checkout, TURN).await.unwrap()
            );
        }
    }

    #[tokio::test]
    async fn nothing_is_checkpointed_mid_turn_or_for_a_workflow_step() {
        let td = tempfile::tempdir().unwrap();
        let sup = test_supervisor();
        let busy = committed_repo(td.path(), "busy").await;
        let mut record = record_in_checkouts(&sup, "denali", &[busy]);
        sup.workspace.add_agent(&mut record).unwrap();
        sup.statuses
            .lock()
            .insert("denali".to_string(), AgentStatus::Running);
        let step = committed_repo(td.path(), "step").await;
        let mut record = record_in_checkouts(&sup, "rainier", &[step]);
        record.owner_run_id = Some("run-1".to_string());
        sup.workspace.add_agent(&mut record).unwrap();

        for agent_id in ["denali", "rainier"] {
            sup.checkpoint_turn(agent_id, TURN).await;
            let checkpoints = sup.turn_checkpoints(agent_id, TURN).await.unwrap();
            assert_eq!(checkpoints[0].sha, None, "{agent_id}");
        }
    }

    #[tokio::test]
    async fn a_checkout_that_cant_be_captured_does_not_stop_the_others() {
        let td = tempfile::tempdir().unwrap();
        let gone = committed_repo(td.path(), "gone").await;
        let kept = committed_repo(td.path(), "kept").await;
        let sup = test_supervisor();
        let mut record = record_in_checkouts(&sup, "denali", &[gone.clone(), kept.clone()]);
        sup.workspace.add_agent(&mut record).unwrap();
        std::fs::remove_dir_all(&gone).unwrap();

        sup.checkpoint_turn("denali", TURN).await;

        assert!(checkpoint::resolve(&kept, TURN).await.unwrap().is_some());
    }

    /// A `--shared` clone of `source` at `<dir>/<name>`, the shape of every
    /// checkout, with an identity to commit with.
    async fn checkout_of(source: &Path, dir: &Path, name: &str) -> PathBuf {
        let dest = dir.join(name);
        let (source, dest_str) = (source.to_str().unwrap(), dest.to_str().unwrap());
        run_git(dir, &["clone", "-q", "--shared", source, dest_str], "clone")
            .await
            .unwrap();
        for (key, value) in [("user.email", "t@example.com"), ("user.name", "Tester")] {
            run_git(&dest, &["config", key, value], "config")
                .await
                .unwrap();
        }
        dest
    }

    async fn head(checkout: &Path) -> String {
        crate::git::rev_parse(checkout, "HEAD").await.unwrap()
    }

    async fn status(checkout: &Path) -> String {
        let out = run_git(checkout, &["status", "--porcelain"], "status")
            .await
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Workspace `id` with a checkout of each of `sources`, at `checkouts`.
    fn workspace_of(sup: &Supervisor, id: &str, sources: &[&PathBuf], checkouts: &[PathBuf]) {
        let mut record = record_in_checkouts(sup, id, checkouts);
        for (repo, source) in record.repos.iter_mut().zip(sources) {
            sup.workspace
                .add_workspace_repo(source.to_path_buf())
                .unwrap();
            repo.repo_path = source.to_path_buf();
        }
        sup.workspace.add_agent(&mut record).unwrap();
    }

    #[tokio::test]
    async fn a_workspace_starts_from_pinned_code_as_it_was_committed_and_not() {
        let td = tempfile::tempdir().unwrap();
        let sources = [
            committed_repo(td.path(), "src-a").await,
            committed_repo(td.path(), "src-b").await,
        ];
        let mut parent = Vec::new();
        let mut child = Vec::new();
        for (i, source) in sources.iter().enumerate() {
            parent.push(checkout_of(source, td.path(), &format!("parent-{i}")).await);
            child.push(checkout_of(source, td.path(), &format!("child-{i}")).await);
        }
        // The parent's work: a commit then an edit in one repo, a new file
        // in the other.
        std::fs::write(parent[0].join("committed.txt"), b"committed").unwrap();
        crate::git::commit_all(&parent[0], "work").await.unwrap();
        std::fs::write(parent[0].join("a.txt"), b"edited").unwrap();
        std::fs::write(parent[1].join("new.txt"), b"untracked").unwrap();
        let sup = test_supervisor();
        workspace_of(&sup, "denali", &[&sources[0], &sources[1]], &parent);
        // The new workspace names its repos the other way round: they are
        // matched by repo, not by subdir.
        workspace_of(
            &sup,
            "fuji",
            &[&sources[1], &sources[0]],
            &[child[1].clone(), child[0].clone()],
        );

        let code = sup.pin_code("denali").await.unwrap();
        let subdirs: Vec<&str> = code.checkouts().iter().map(|c| c.subdir.as_str()).collect();
        assert_eq!(subdirs, ["repo-0", "repo-1"]);
        sup.start_from("fuji", &code).await.unwrap();

        // Each repo: HEAD is the commit the snapshot was taken on, and what
        // was uncommitted there is uncommitted here.
        for (parent, child) in parent.iter().zip(&child) {
            assert_eq!(head(child).await, head(parent).await);
            assert_eq!(status(child).await, status(parent).await);
        }
        let read = |path: PathBuf| std::fs::read(path).unwrap();
        assert_eq!(read(child[0].join("committed.txt")), b"committed");
        assert_eq!(read(child[0].join("a.txt")), b"edited");
        assert_eq!(read(child[1].join("new.txt")), b"untracked");
        // The parent is left as it was.
        assert_eq!(status(&parent[0]).await, " M a.txt\n");
    }

    #[tokio::test]
    async fn code_a_workspace_cant_take_whole_is_refused_and_a_new_repo_starts_clean() {
        let td = tempfile::tempdir().unwrap();
        let sources = [
            committed_repo(td.path(), "src-a").await,
            committed_repo(td.path(), "src-b").await,
            committed_repo(td.path(), "src-c").await,
        ];
        let dir = td.path();
        let pa = checkout_of(&sources[0], dir, "p-a").await;
        let pb = checkout_of(&sources[1], dir, "p-b").await;
        let ca = checkout_of(&sources[0], dir, "c-a").await;
        let cc = checkout_of(&sources[2], dir, "c-c").await;
        let only_a = checkout_of(&sources[0], dir, "o-a").await;
        for parent in [&pa, &pb, &only_a] {
            std::fs::write(parent.join("a.txt"), b"edited").unwrap();
        }
        let sup = test_supervisor();
        workspace_of(&sup, "denali", &[&sources[0], &sources[1]], &[pa, pb]);
        workspace_of(&sup, "solo", &[&sources[0]], &[only_a]);
        // The new workspace checks out a and c, but not b.
        workspace_of(
            &sup,
            "fuji",
            &[&sources[0], &sources[2]],
            &[ca.clone(), cc.clone()],
        );
        let base = head(&ca).await;

        // b's code has nowhere to go: refused, naming it, before any
        // checkout is touched.
        let both = sup.pin_code("denali").await.unwrap();
        let err = sup.start_from("fuji", &both).await.unwrap_err().to_string();
        assert!(err.contains("no checkout of repo-1,"), "{err}");
        assert_eq!(status(&ca).await, "");
        assert_eq!(head(&ca).await, base);

        // c was never in the source workspace, so there is no code of it to
        // copy: it keeps its clean base while a takes the source's code.
        let a = sup.pin_code("solo").await.unwrap();
        sup.start_from("fuji", &a).await.unwrap();
        assert_eq!(status(&ca).await, " M a.txt\n");
        assert_eq!(status(&cc).await, "");
    }

    #[tokio::test]
    async fn code_at_a_turn_needs_its_checkpoint_in_every_checkout() {
        let td = tempfile::tempdir().unwrap();
        let (a, b) = (
            committed_repo(td.path(), "a").await,
            committed_repo(td.path(), "b").await,
        );
        let sup = test_supervisor();
        let mut record = record_in_checkouts(&sup, "denali", &[a.clone(), b]);
        sup.workspace.add_agent(&mut record).unwrap();
        checkpoint::capture(&a, TURN).await.unwrap();

        let err = sup.code_at("denali", TURN).await.unwrap_err().to_string();
        assert!(err.contains("wasn't kept for repo-1."), "{err}");

        sup.checkpoint_turn("denali", TURN).await;
        let code = sup.code_at("denali", TURN).await.unwrap();
        assert_eq!(code.key(), TURN);
        assert_eq!(code.checkouts().len(), 2);
    }
}
