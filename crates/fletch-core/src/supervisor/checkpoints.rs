//! Turn checkpoints for an agent's workspace: before a turn reaches the agent,
//! pin every checkout as it stands (`git::checkpoint`), so the code as of that
//! message outlives the edits the turn goes on to make. A new workspace can
//! start from a workspace's pinned code ([`PinnedCode`]): a fork's code. And
//! every checkout can be put back to one, with an undo point: rewind's code
//! half.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::git::checkpoint::{self, LeavingCommit};

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

/// What restoring a turn's code does to an agent's checkouts, in `repos`
/// order: what a confirmation shows before
/// ([`Supervisor::preview_turn_code_restore`]) and what changed after
/// ([`Supervisor::restore_turn_code`]).
#[derive(Debug, Clone, Serialize)]
pub struct RestoreReport {
    pub repos: Vec<RepoRestore>,
}

/// One checkout in a [`RestoreReport`].
#[derive(Debug, Clone, Serialize)]
pub struct RepoRestore {
    /// The tracked repo's subdir within the workspace (`TrackedRepo::subdir`).
    pub subdir: String,
    /// The branch the checkout is on, which goes back with HEAD; `None` when
    /// HEAD is detached.
    pub branch: Option<String>,
    /// The turn's checkpoint, or `None` when the checkout has none: it is then
    /// left as it is.
    pub checkpoint: Option<String>,
    /// The commits the restore takes off the branch, newest first.
    pub leaving: Vec<LeavingCommit>,
}

impl Supervisor {
    /// Checkpoint every checkout of `agent_id` under `turn_id`. Called as a
    /// turn is delivered, before the agent sees it (`deliver_as_turn`).
    ///
    /// The turn also retires the undo point a code restore left
    /// ([`Self::restore_turn_code`]): undoing once the agent has acted would
    /// clobber its work, and this checkpoint keeps that code anyway. Dropped
    /// first, being the quick half.
    ///
    /// Best-effort: a checkout that can't be captured (a config the hardening
    /// refuses, a vanished directory) is logged and skipped, and the whole
    /// capture gives up after [`CAPTURE_TIMEOUT`]; the send goes ahead either
    /// way. Nothing is captured when there is no settled tree:
    /// - mid-turn or mid-spawn — `deliver_as_turn` then holds the message
    ///   rather than start its turn;
    /// - a workflow step agent (`owner_run_id`) works in a tree its run shares,
    ///   and so never has a checkpoint to restore, nor an undo point.
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
                let Ok(checkout) = repo.checkout_path(agent_id) else {
                    continue;
                };
                if let Err(e) = checkpoint::drop_undo(&checkout).await {
                    tracing::warn!(error = %e, agent_id, subdir = %repo.subdir, "dropping the code undo point failed");
                }
            }
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

    /// What [`Self::restore_turn_code`] would do to `agent_id`'s checkouts for
    /// `turn_id`, without changing anything — for the confirmation.
    pub async fn preview_turn_code_restore(
        &self,
        agent_id: &str,
        turn_id: &str,
    ) -> Result<RestoreReport> {
        let repos = self.plan_code_restore(agent_id, turn_id).await?;
        Ok(RestoreReport {
            repos: repos.into_iter().map(|(_, repo)| repo).collect(),
        })
    }

    /// Put each of `agent_id`'s checkouts back to its checkpoint for
    /// `turn_id` — files and HEAD, the code as it was before that turn — and
    /// report what changed. A checkout without a checkpoint is left as it is.
    ///
    /// The branch a checkout is on stays checked out and goes back with HEAD
    /// (a reset), so the user is still on the branch they were on, minus the
    /// commits made since (`leaving`). Pushed commits stay on the remote, so
    /// the branch's next push has to force. Ignored files are never touched.
    ///
    /// Each checkout it changes is pinned first as its undo point, which
    /// keeps those commits reachable and the restore undoable
    /// ([`Self::undo_code_restore`]). A checkout has one undo point, so this
    /// restore's replaces any earlier one — in every checkout, so that none of
    /// an older one survives where this restore changes nothing. It lasts until
    /// it is undone or discarded, or the next turn is delivered.
    ///
    /// All or nothing: a failure before the first checkout changes leaves
    /// every one as it was, and a failure during the restore puts back the
    /// ones already changed, from their undo points. Either way no undo point
    /// is left, as there is nothing to undo — unless putting one back fails,
    /// which keeps that point for another try.
    ///
    /// Runs under the caller's delivery lock and input route (`lock_delivery`,
    /// `open_route`), and is refused while a turn runs: the agent would be
    /// editing the very files being replaced.
    pub async fn restore_turn_code(&self, agent_id: &str, turn_id: &str) -> Result<RestoreReport> {
        if self.is_busy(agent_id) {
            return Err(Error::Other(
                "stop the agent before restoring its code".into(),
            ));
        }
        let repos = self.plan_code_restore(agent_id, turn_id).await?;
        for (checkout, _) in &repos {
            checkpoint::drop_undo(checkout).await?;
        }
        let changing: Vec<(&Path, &str)> = repos
            .iter()
            .filter_map(|(checkout, repo)| Some((checkout.as_path(), repo.checkpoint.as_deref()?)))
            .collect();
        for (pinned, (checkout, _)) in changing.iter().enumerate() {
            if let Err(e) = checkpoint::pin_undo(checkout).await {
                take_back_restore(agent_id, &changing[..pinned], 0).await;
                return Err(e);
            }
        }
        for (i, (checkout, sha)) in changing.iter().enumerate() {
            if let Err(e) = checkpoint::restore(checkout, sha).await {
                take_back_restore(agent_id, &changing, i + 1).await;
                return Err(e);
            }
        }
        Ok(RestoreReport {
            repos: repos.into_iter().map(|(_, repo)| repo).collect(),
        })
    }

    /// Undo the last code restore ([`Self::restore_turn_code`]): each checkout
    /// it changed goes back to how it stood before, and the undo points go.
    /// They go only once every checkout is back, so a failure can be tried
    /// again. An error when there is nothing to undo.
    ///
    /// Takes the agent's delivery lock and input route, like a send: no turn
    /// may start on a half-undone tree, and no archive tear it down. Refused
    /// while a turn runs.
    pub async fn undo_code_restore(&self, agent_id: &str) -> Result<()> {
        let checkouts = self.checkouts(agent_id)?;
        let _delivering = self.lock_delivery(agent_id).await;
        let _route = self.open_route(agent_id)?;
        if self.is_busy(agent_id) {
            return Err(Error::Other(
                "stop the agent before undoing the code restore".into(),
            ));
        }
        let mut undoable = Vec::new();
        for checkout in checkouts {
            if checkpoint::has_undo(&checkout).await? {
                undoable.push(checkout);
            }
        }
        if undoable.is_empty() {
            return Err(Error::Other("there is no code restore to undo".into()));
        }
        for checkout in &undoable {
            checkpoint::undo(checkout).await?;
        }
        for checkout in &undoable {
            checkpoint::drop_undo(checkout).await?;
        }
        Ok(())
    }

    /// Let the last code restore's undo points go, keeping the restored code.
    /// Takes the agent's delivery lock and input route, as an undo does.
    pub async fn discard_code_undo(&self, agent_id: &str) -> Result<()> {
        let checkouts = self.checkouts(agent_id)?;
        let _delivering = self.lock_delivery(agent_id).await;
        let _route = self.open_route(agent_id)?;
        for checkout in &checkouts {
            checkpoint::drop_undo(checkout).await?;
        }
        Ok(())
    }

    /// Whether `agent_id`'s last code restore can still be undone — what the
    /// UI offers again after a restart. A read, so it takes no lock.
    pub async fn has_code_undo(&self, agent_id: &str) -> Result<bool> {
        for checkout in self.checkouts(agent_id)? {
            if checkpoint::has_undo(&checkout).await? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `agent_id`'s checkouts, in `repos` order.
    fn checkouts(&self, agent_id: &str) -> Result<Vec<PathBuf>> {
        self.workspace
            .agent(agent_id)?
            .repos
            .iter()
            .map(|repo| repo.checkout_path(agent_id))
            .collect()
    }

    /// Each checkout with what restoring `turn_id`'s checkpoint would do to it.
    async fn plan_code_restore(
        &self,
        agent_id: &str,
        turn_id: &str,
    ) -> Result<Vec<(PathBuf, RepoRestore)>> {
        let mut repos = Vec::new();
        for found in self.turn_checkpoints(agent_id, turn_id).await? {
            let leaving = match &found.sha {
                Some(sha) => checkpoint::leaving_commits(&found.checkout, sha).await?,
                None => Vec::new(),
            };
            let repo = RepoRestore {
                subdir: found.subdir,
                branch: crate::git::current_branch(&found.checkout).await?,
                checkpoint: found.sha,
                leaving,
            };
            repos.push((found.checkout, repo));
        }
        Ok(repos)
    }
}

/// Take back a restore that failed after its undo points were pinned in
/// `pinned`, and after the first `changed` of those checkouts were (perhaps
/// partly) restored: those go back from their points, and no point is left
/// behind. A checkout that can't be put back keeps its point, so an undo can
/// try again.
async fn take_back_restore(agent_id: &str, pinned: &[(&Path, &str)], changed: usize) {
    for (i, (checkout, _)) in pinned.iter().enumerate() {
        let put_back = if i < changed {
            checkpoint::undo(checkout).await
        } else {
            Ok(())
        };
        let taken_back = match put_back {
            Ok(()) => checkpoint::drop_undo(checkout).await,
            Err(e) => Err(e),
        };
        if let Err(e) = taken_back {
            tracing::warn!(error = %e, agent_id, checkout = %checkout.display(), "taking back a failed code restore failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::run_git;
    use crate::supervisor::tests::{committed_repo, record_in_checkouts, test_supervisor};
    use crate::workspace::AgentStatus;

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

    // ── restoring a turn's code ──────────────────────────────────────────

    async fn commit(repo: &Path, file: &str, body: &str) -> String {
        std::fs::write(repo.join(file), body).unwrap();
        crate::git::commit_all(repo, body).await.unwrap();
        head(repo).await
    }

    fn read(repo: &Path, file: &str) -> Option<String> {
        std::fs::read_to_string(repo.join(file)).ok()
    }

    /// Checkouts `a` and `b`, checkpointed at [`TURN`] with uncommitted work
    /// in `a`; then the turn goes on in both, and a third checkout `c` is
    /// attached, after the turn. Returns the supervisor and the checkouts.
    async fn worked_past_the_turn(td: &Path) -> (Supervisor, [PathBuf; 3]) {
        let a = committed_repo(td, "a").await;
        let b = committed_repo(td, "b").await;
        let c = committed_repo(td, "c").await;
        let sup = test_supervisor();
        let mut record = record_in_checkouts(&sup, "denali", &[a.clone(), b.clone()]);
        sup.workspace.add_agent(&mut record).unwrap();
        std::fs::write(a.join("wip.txt"), b"at the turn").unwrap();
        sup.checkpoint_turn("denali", TURN).await;

        commit(&a, "later.txt", "a commit after the turn").await;
        std::fs::write(a.join("wip.txt"), b"edited later").unwrap();
        std::fs::write(b.join("stray.txt"), b"created later").unwrap();
        sup.workspace.add_workspace_repo(c.clone()).unwrap();
        let attached = crate::workspace::TrackedRepo {
            repo_path: c.clone(),
            subdir: "repo-2".into(),
            adopted_checkout: Some(c.clone()),
            ..record.repos[0].clone()
        };
        sup.workspace
            .append_tracked_repo("denali", attached)
            .unwrap();
        (sup, [a, b, c])
    }

    #[tokio::test]
    async fn a_preview_reports_the_restore_and_changes_nothing() {
        let td = tempfile::tempdir().unwrap();
        let (sup, [a, _, c]) = worked_past_the_turn(td.path()).await;
        let tip = head(&a).await;

        let report = sup.preview_turn_code_restore("denali", TURN).await.unwrap();

        let [ra, rb, rc] = &report.repos[..] else {
            panic!("{report:?}")
        };
        assert!(ra.checkpoint.is_some() && rb.checkpoint.is_some());
        assert_eq!(rc.checkpoint, None, "c was attached after the turn");
        assert_eq!(ra.branch, crate::git::current_branch(&a).await.unwrap());
        assert!(ra.branch.is_some());
        assert_eq!(ra.leaving.len(), 1);
        assert_eq!(ra.leaving[0].sha, tip);
        assert_eq!(ra.leaving[0].subject, "a commit after the turn");
        assert!(!ra.leaving[0].pushed);
        assert!(rb.leaving.is_empty() && rc.leaving.is_empty());
        // Nothing moved, and there is nothing to undo.
        assert!(!sup.has_code_undo("denali").await.unwrap());
        assert_eq!(head(&a).await, tip);
        assert_eq!(read(&a, "wip.txt").as_deref(), Some("edited later"));
        assert!(c.join("a.txt").exists());
    }

    #[tokio::test]
    async fn restoring_a_turn_puts_every_checkout_back_and_can_be_undone() {
        let td = tempfile::tempdir().unwrap();
        let (sup, [a, b, c]) = worked_past_the_turn(td.path()).await;
        let branch = crate::git::current_branch(&a).await.unwrap().unwrap();
        let tip = head(&a).await;
        let c_head = head(&c).await;

        let report = sup.restore_turn_code("denali", TURN).await.unwrap();

        // `a` is back on its branch, which went back with HEAD, and has the
        // work it had at the turn, still uncommitted.
        assert_eq!(
            crate::git::current_branch(&a).await.unwrap().as_deref(),
            Some(branch.as_str())
        );
        let at_turn = crate::git::rev_parse(
            &a,
            &format!("{}^", report.repos[0].checkpoint.as_ref().unwrap()),
        )
        .await
        .unwrap();
        assert_eq!(head(&a).await, at_turn);
        assert_eq!(
            crate::git::rev_parse(&a, &format!("refs/heads/{branch}"))
                .await
                .unwrap(),
            at_turn
        );
        assert_eq!(read(&a, "wip.txt").as_deref(), Some("at the turn"));
        assert_eq!(read(&a, "later.txt"), None);
        assert_eq!(read(&b, "stray.txt"), None);
        // `c` had no checkpoint and is left alone.
        assert_eq!(head(&c).await, c_head);
        assert_eq!(report.repos[0].leaving[0].sha, tip);

        // The undo puts every changed checkout back, and is then spent.
        assert!(sup.has_code_undo("denali").await.unwrap());
        sup.undo_code_restore("denali").await.unwrap();
        assert_eq!(head(&a).await, tip);
        assert_eq!(read(&a, "wip.txt").as_deref(), Some("edited later"));
        assert_eq!(
            read(&a, "later.txt").as_deref(),
            Some("a commit after the turn")
        );
        assert_eq!(read(&b, "stray.txt").as_deref(), Some("created later"));
        assert!(!sup.has_code_undo("denali").await.unwrap());
        assert!(sup.undo_code_restore("denali").await.is_err());
    }

    async fn undo_points(checkouts: &[PathBuf]) -> Vec<bool> {
        let mut found = Vec::new();
        for checkout in checkouts {
            found.push(checkpoint::has_undo(checkout).await.unwrap());
        }
        found
    }

    #[tokio::test]
    async fn a_restore_pins_only_what_it_changes_and_clears_older_undo_points() {
        let td = tempfile::tempdir().unwrap();
        let (sup, checkouts) = worked_past_the_turn(td.path()).await;
        // Left over from an earlier restore in `c`, which this one won't change.
        checkpoint::pin_undo(&checkouts[2]).await.unwrap();

        sup.restore_turn_code("denali", TURN).await.unwrap();

        assert_eq!(undo_points(&checkouts).await, [true, true, false]);
    }

    #[tokio::test]
    async fn a_second_restore_replaces_the_first_undo_point() {
        let td = tempfile::tempdir().unwrap();
        let (sup, [a, ..]) = worked_past_the_turn(td.path()).await;
        sup.restore_turn_code("denali", TURN).await.unwrap();
        std::fs::write(a.join("wip.txt"), b"between the restores").unwrap();

        sup.restore_turn_code("denali", TURN).await.unwrap();
        sup.undo_code_restore("denali").await.unwrap();

        // Back to before the second restore, not the first.
        assert_eq!(read(&a, "wip.txt").as_deref(), Some("between the restores"));
        assert_eq!(read(&a, "later.txt"), None);
    }

    #[tokio::test]
    async fn a_discarded_undo_point_leaves_the_restored_code() {
        let td = tempfile::tempdir().unwrap();
        let (sup, [a, ..]) = worked_past_the_turn(td.path()).await;
        sup.restore_turn_code("denali", TURN).await.unwrap();

        sup.discard_code_undo("denali").await.unwrap();

        assert!(!sup.has_code_undo("denali").await.unwrap());
        assert!(sup.undo_code_restore("denali").await.is_err());
        assert_eq!(read(&a, "wip.txt").as_deref(), Some("at the turn"));
    }

    /// Undoing once the agent has worked on the restored code would clobber
    /// that work, so the next turn retires the undo point.
    #[tokio::test]
    async fn the_next_turn_retires_the_undo_point() {
        let td = tempfile::tempdir().unwrap();
        let (sup, checkouts) = worked_past_the_turn(td.path()).await;
        sup.restore_turn_code("denali", TURN).await.unwrap();
        let next = "7a1e5c3b-2d4f-4e6a-9b8c-0d1e2f3a4b5c";

        sup.checkpoint_turn("denali", next).await;

        assert!(!sup.has_code_undo("denali").await.unwrap());
        assert_eq!(undo_points(&checkouts).await, [false; 3]);
        assert!(checkpoint::resolve(&checkouts[0], next)
            .await
            .unwrap()
            .is_some());
    }

    /// A restore that fails part-way puts back the checkouts it changed and
    /// leaves no undo point but for one it couldn't put back.
    #[tokio::test]
    async fn a_failed_restore_is_taken_back() {
        let td = tempfile::tempdir().unwrap();
        let (sup, [a, b, c]) = worked_past_the_turn(td.path()).await;
        let tip = head(&a).await;
        // `b` can be read and pinned, but not reset, and not put back either.
        let lock = b.join(".git").join("index.lock");
        std::fs::write(&lock, b"").unwrap();

        assert!(sup.restore_turn_code("denali", TURN).await.is_err());

        assert_eq!(head(&a).await, tip);
        assert_eq!(read(&a, "wip.txt").as_deref(), Some("edited later"));
        assert_eq!(read(&b, "stray.txt").as_deref(), Some("created later"));
        assert_eq!(
            undo_points(&[a.clone(), b.clone(), c]).await,
            [false, true, false]
        );
        // Once `b` can be written again, the undo it kept finishes the job.
        std::fs::remove_file(&lock).unwrap();
        sup.undo_code_restore("denali").await.unwrap();
        assert_eq!(read(&b, "stray.txt").as_deref(), Some("created later"));
        assert!(!sup.has_code_undo("denali").await.unwrap());
    }

    #[tokio::test]
    async fn commits_origin_already_has_are_reported_pushed() {
        let td = tempfile::tempdir().unwrap();
        let (sup, [a, ..]) = worked_past_the_turn(td.path()).await;
        let origin = td.path().join("origin.git");
        crate::git::run_git(
            td.path(),
            &["init", "-q", "--bare", origin.to_str().unwrap()],
            "init",
        )
        .await
        .unwrap();
        crate::git::run_git(
            &a,
            &["remote", "add", "origin", origin.to_str().unwrap()],
            "remote",
        )
        .await
        .unwrap();
        crate::git::run_git(
            &a,
            &["push", "-q", "origin", "HEAD:refs/heads/feature"],
            "push",
        )
        .await
        .unwrap();
        let unpushed = commit(&a, "unpushed.txt", "not pushed yet").await;

        let report = sup.preview_turn_code_restore("denali", TURN).await.unwrap();

        let leaving: Vec<(&str, bool)> = report.repos[0]
            .leaving
            .iter()
            .map(|c| (c.subject.as_str(), c.pushed))
            .collect();
        assert_eq!(
            leaving,
            [("not pushed yet", false), ("a commit after the turn", true)]
        );
        assert_eq!(report.repos[0].leaving[0].sha, unpushed);
    }

    #[tokio::test]
    async fn the_code_stays_put_under_a_running_turn() {
        let td = tempfile::tempdir().unwrap();
        let (sup, [a, ..]) = worked_past_the_turn(td.path()).await;
        let tip = head(&a).await;
        sup.statuses
            .lock()
            .insert("denali".to_string(), AgentStatus::Running);

        assert!(sup.restore_turn_code("denali", TURN).await.is_err());

        assert_eq!(head(&a).await, tip);
        assert_eq!(read(&a, "wip.txt").as_deref(), Some("edited later"));
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
