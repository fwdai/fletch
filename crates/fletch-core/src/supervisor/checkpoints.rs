//! Turn checkpoints for an agent's workspace: before a turn reaches the agent,
//! pin every checkout as it stands (`git::checkpoint`), so the code as of that
//! message outlives the edits the turn goes on to make. A new workspace can
//! start from such a pinned snapshot ([`CodeSource`]): a fork's code.

use std::path::PathBuf;
use std::time::Duration;

use crate::error::Result;
use crate::git::checkpoint;

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
}
