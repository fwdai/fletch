//! Turn checkpoints for an agent's workspace: before a turn reaches the agent,
//! pin every checkout as it stands (`git::checkpoint`), so the code as of that
//! message outlives the edits the turn goes on to make.

use std::path::PathBuf;

use crate::error::Result;
use crate::git::checkpoint;

use super::Supervisor;

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

impl Supervisor {
    /// Checkpoint every checkout of `agent_id` under `turn_id`. Called as a
    /// turn is delivered, before the agent sees it (`deliver_as_turn`).
    ///
    /// Best-effort: a checkout that can't be captured (a config the hardening
    /// refuses, a vanished directory) is logged and skipped, and the send goes
    /// ahead regardless. Nothing is captured when there is no settled tree:
    /// - mid-turn, the agent is editing it — a delivery that lost a race to
    ///   another turn start, which `deliver_as_turn` then refuses;
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
        for repo in &record.repos {
            let captured = match repo.checkout_path(agent_id) {
                Ok(checkout) => checkpoint::capture(&checkout, turn_id).await,
                Err(e) => Err(e),
            };
            if let Err(e) = captured {
                tracing::warn!(error = %e, agent_id, turn_id, subdir = %repo.subdir, "turn checkpoint failed");
            }
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
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
