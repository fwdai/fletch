//! Turn checkpoints for an agent's workspace: before a turn reaches the agent,
//! pin every checkout as it stands (`git::checkpoint`), so the code as of that
//! message outlives the edits the turn goes on to make. A new workspace can
//! start from such a pinned snapshot ([`CodeSource`]): a fork's code. And
//! every checkout can be put back to one, with an undo point: rewind's code
//! half.

use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::git::checkpoint::{self, LeavingCommit};

use super::Supervisor;

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

/// A snapshot pinned in one checkout (`git::checkpoint`) that a new
/// workspace's checkout of the same subdir starts from — a fork's code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeSource {
    /// The tracked repo's subdir (`TrackedRepo::subdir`); the new checkout of
    /// the same subdir starts from this snapshot.
    pub subdir: String,
    /// The checkout the snapshot is pinned in.
    pub checkout: PathBuf,
    /// The key it is pinned under: a turn id, or a key of its own for code
    /// pinned on demand ([`Supervisor::pin_code`]).
    pub key: String,
}

/// What restoring a turn's code does to an agent's checkouts, in `repos`
/// order: what a confirmation shows before
/// ([`Supervisor::preview_turn_code_restore`]) and what an undo needs after
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
    /// Where the checkout as it stood before the restore is pinned, to undo it
    /// (`git::checkpoint::restore` to this ref). `None` in a preview, and for
    /// a checkout the restore left alone.
    pub undo_ref: Option<String>,
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

    /// Pin every checkout of `agent_id` as it stands now, under one fresh key
    /// — its current code, for a new workspace to start from.
    pub async fn pin_code(&self, agent_id: &str) -> Result<Vec<CodeSource>> {
        let record = self.workspace.agent(agent_id)?;
        let key = uuid::Uuid::new_v4().to_string();
        let mut code = Vec::with_capacity(record.repos.len());
        for repo in &record.repos {
            let checkout = repo.checkout_path(agent_id)?;
            checkpoint::capture(&checkout, &key).await?;
            code.push(CodeSource {
                subdir: repo.subdir.clone(),
                checkout,
                key: key.clone(),
            });
        }
        Ok(code)
    }

    /// Start each of `agent_id`'s checkouts that has a source in `code` from
    /// it: its working tree becomes the snapshot's, and its HEAD the commit
    /// the snapshot was taken on. Committed work stays committed and
    /// uncommitted work uncommitted, as it was in the source.
    pub async fn start_from(&self, agent_id: &str, code: &[CodeSource]) -> Result<()> {
        let record = self.workspace.agent(agent_id)?;
        for repo in &record.repos {
            let Some(source) = code.iter().find(|s| s.subdir == repo.subdir) else {
                continue;
            };
            let checkout = repo.checkout_path(agent_id)?;
            let sha = checkpoint::fetch_into(&checkout, &source.checkout, &source.key).await?;
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
    /// commits made since (`leaving`). Each checkout is pinned first under an
    /// undo ref, which keeps those commits reachable and the restore
    /// reversible. Pushed commits stay on the remote, so the branch's next
    /// push has to force. Ignored files are never touched.
    ///
    /// All or nothing: a failure before the first checkout changes leaves
    /// every one as it was, and a failure during the restore puts back the
    /// ones already changed, from their undo refs.
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
        let mut repos = self.plan_code_restore(agent_id, turn_id).await?;
        for (checkout, repo) in &mut repos {
            if repo.checkpoint.is_some() {
                repo.undo_ref = Some(checkpoint::capture_undo(checkout).await?);
            }
        }
        for (i, (checkout, repo)) in repos.iter().enumerate() {
            let Some(sha) = &repo.checkpoint else {
                continue;
            };
            if let Err(e) = checkpoint::restore(checkout, sha).await {
                for (checkout, repo) in &repos[..=i] {
                    let Some(undo) = &repo.undo_ref else {
                        continue;
                    };
                    if let Err(e) = checkpoint::restore(checkout, undo).await {
                        tracing::warn!(error = %e, agent_id, undo, "putting a checkout back after a failed restore failed");
                    }
                }
                return Err(e);
            }
        }
        Ok(RestoreReport {
            repos: repos.into_iter().map(|(_, repo)| repo).collect(),
        })
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
                undo_ref: None,
            };
            repos.push((found.checkout, repo));
        }
        Ok(repos)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

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
        let mut record = record_in_checkouts(&sup, "denali", &parent);
        sup.workspace.add_agent(&mut record).unwrap();
        let mut record = record_in_checkouts(&sup, "fuji", &child);
        sup.workspace.add_agent(&mut record).unwrap();

        let code = sup.pin_code("denali").await.unwrap();
        let subdirs: Vec<&str> = code.iter().map(|c| c.subdir.as_str()).collect();
        assert_eq!(subdirs, ["repo-0", "repo-1"]);
        assert_eq!(code[0].key, code[1].key, "pinned under one key");
        sup.start_from("fuji", &code).await.unwrap();

        // Each repo, matched by subdir: HEAD is the commit the snapshot was
        // taken on, and what was uncommitted there is uncommitted here.
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
        assert!(report.repos.iter().all(|r| r.undo_ref.is_none()));
        // Nothing moved.
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
        assert_eq!(report.repos[2].undo_ref, None);
        assert_eq!(report.repos[0].leaving[0].sha, tip);

        // Each changed checkout has an undo point that puts it back.
        let undo = report.repos[0].undo_ref.as_deref().unwrap();
        assert!(report.repos[1].undo_ref.is_some());
        checkpoint::restore(&a, undo).await.unwrap();
        assert_eq!(head(&a).await, tip);
        assert_eq!(read(&a, "wip.txt").as_deref(), Some("edited later"));
        assert_eq!(
            read(&a, "later.txt").as_deref(),
            Some("a commit after the turn")
        );
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
}
