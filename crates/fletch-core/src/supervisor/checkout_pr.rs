//! Moving a checkout's working tree to one of its PRs.

use crate::error::{Error, Result};
use crate::git;
use crate::github::PrState;
use crate::host::EngineCtx;

use super::delegation::{self, checkout_subdir};
use super::events::emit_branch;
use super::Supervisor;

impl Supervisor {
    /// Check out the head branch of PR `number` — one of the checkout's PRs —
    /// and focus it, so the tree, the focused PR and every action about
    /// either are on one branch.
    ///
    /// The checkout is the agent's working directory, so the tree only moves
    /// while nothing else is in it, and nothing gets in while it moves: the
    /// agent's delivery lock and input route are held as for a send, so no
    /// turn Fletch starts runs under the switch, and the tree is reserved
    /// against a delegation's admission and a local review's claim
    /// (`delegation::reserve_tree`). A turn the provider opens on its own is
    /// the one start neither orders, as for an undo or a rewind.
    ///
    /// It carries nothing across either — a tree with uncommitted work is
    /// refused, since `git checkout` would take the changes to the other
    /// branch. Each refusal says what to do instead, and none moves the focus.
    pub async fn checkout_pr(
        &self,
        ctx: &EngineCtx,
        agent_id: &str,
        subdir: Option<&str>,
        number: u32,
    ) -> Result<PrState> {
        // First, so an unknown id never creates a delivery lock.
        let record = self.workspace.agent(agent_id)?;
        let key = (agent_id.to_string(), checkout_subdir(&record, subdir)?);
        let _delivering = self.lock_delivery(agent_id).await;
        let _route = self.open_route(agent_id)?;
        if self.is_busy(agent_id) {
            return Err(Error::Other(
                "The agent is working in this checkout. Switch branches when its turn ends.".into(),
            ));
        }
        let _moving =
            delegation::reserve_tree(delegation::global(), crate::review::engine::global(), &key)?;

        let (repo, checkout, _hold) = crate::commands::agent_repo_checkout(self, agent_id, subdir)?;
        let branch = self
            .workspace
            .repo_pr_history(agent_id, &repo.subdir)?
            .into_iter()
            .find(|pr| pr.number == number)
            .ok_or_else(|| Error::Other(format!("#{number} is not one of this checkout's PRs")))?
            .branch
            .filter(|b| !b.is_empty())
            .ok_or_else(|| Error::Other(format!("#{number}'s branch isn't known yet")))?;

        let current = git::current_branch(&checkout).await?;
        if current.as_deref() != Some(branch.as_str()) {
            if git::has_uncommitted_changes(&checkout).await? {
                let on = current.map(|b| format!(" on {b}")).unwrap_or_default();
                return Err(Error::Other(format!(
                    "Uncommitted changes{on} would move to {branch}. Commit or stash them first."
                )));
            }
            if !git::branch_name_taken(&checkout, &branch).await? {
                git::Transport::open(&checkout, &repo.repo_path)
                    .await?
                    .fetch_branches(git::Branches::One(&branch))
                    .await
                    .map_err(|e| Error::Git(format!("checkout {branch} failed: {e}")))?;
            }
            git::switch_branch(&checkout, &branch).await?;
        }

        // The branch the checkout is on is a record of its own (a push reads
        // it), kept in step here as it is when a push creates one.
        if repo.branch.as_deref() != Some(branch.as_str()) {
            self.workspace
                .set_repo_branch(agent_id, &repo.subdir, &branch)?;
            emit_branch(ctx.sink.as_ref(), agent_id, &repo.subdir, &branch);
        }
        self.set_focused_pr(ctx, agent_id, subdir, number)
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::github::{MergeableState, PrStatus};
    use crate::supervisor::tests::{committed_repo, record_in_checkouts, test_supervisor};
    use crate::workspace::AgentStatus;

    const SUBDIR: &str = "repo-0";

    fn pr(number: u32, branch: &str) -> PrState {
        PrState {
            number,
            url: format!("https://github.com/o/r/pull/{number}"),
            state: PrStatus::Open,
            title: "t".to_string(),
            mergeable: MergeableState::Unknown,
            opened_at: None,
            merged_at: None,
            branch: Some(branch.to_string()),
            base: None,
        }
    }

    /// An idle agent whose one checkout is on `feat/b` (PR #8, focused) and
    /// also holds `feat/a` (PR #7), each one commit past the base.
    async fn agent_on_two_prs(dir: &Path, id: &str) -> (Arc<Supervisor>, PathBuf) {
        let checkout = committed_repo(dir, "repo").await;
        for branch in ["feat/a", "feat/b"] {
            git::checkout_new_branch(&checkout, branch).await.unwrap();
            std::fs::write(checkout.join("a.txt"), branch).unwrap();
            git::commit_all(&checkout, branch).await.unwrap();
        }
        let sup = Arc::new(test_supervisor());
        let mut record = record_in_checkouts(&sup, id, std::slice::from_ref(&checkout));
        record.session_id = None;
        record.repos[0].branch = Some("feat/b".to_string());
        sup.workspace.add_agent(&mut record).unwrap();
        for pr in [pr(7, "feat/a"), pr(8, "feat/b")] {
            sup.workspace.record_repo_pr(id, SUBDIR, &pr).unwrap();
        }
        sup.workspace
            .set_focused_pr(id, SUBDIR, 8)
            .unwrap()
            .unwrap();
        (sup, checkout)
    }

    async fn branch_of(checkout: &Path) -> String {
        git::current_branch(checkout).await.unwrap().unwrap()
    }

    fn focused(sup: &Supervisor, id: &str) -> (Option<i64>, Option<String>) {
        let repo = sup.workspace.agent(id).unwrap().repos.remove(0);
        (repo.pr_number, repo.branch)
    }

    #[tokio::test]
    async fn checks_out_the_prs_branch_and_focuses_it() {
        let td = tempfile::tempdir().unwrap();
        let (sup, checkout) = agent_on_two_prs(td.path(), "kennet").await;
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();

        let pr = sup.checkout_pr(&ctx, "kennet", None, 7).await.unwrap();

        assert_eq!(pr.number, 7);
        assert_eq!(branch_of(&checkout).await, "feat/a");
        assert_eq!(std::fs::read(checkout.join("a.txt")).unwrap(), b"feat/a");
        assert_eq!(
            focused(&sup, "kennet"),
            (Some(7), Some("feat/a".to_string()))
        );
    }

    #[tokio::test]
    async fn refuses_to_carry_uncommitted_work_to_the_other_branch() {
        let td = tempfile::tempdir().unwrap();
        let (sup, checkout) = agent_on_two_prs(td.path(), "medway").await;
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        std::fs::write(checkout.join("new.txt"), b"wip").unwrap();

        let err = sup.checkout_pr(&ctx, "medway", None, 7).await.unwrap_err();

        assert!(
            err.to_string().contains("Commit or stash them first"),
            "{err}"
        );
        assert_eq!(branch_of(&checkout).await, "feat/b");
        assert_eq!(
            focused(&sup, "medway"),
            (Some(8), Some("feat/b".to_string()))
        );
    }

    /// A send holds the delivery lock until its turn has started. A switch
    /// asked for meanwhile waits for it rather than moving the tree under the
    /// turn, and then finds the agent working.
    #[tokio::test]
    async fn waits_for_a_turn_being_started_and_then_refuses() {
        let td = tempfile::tempdir().unwrap();
        let (sup, checkout) = agent_on_two_prs(td.path(), "tamar").await;
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        let sending = sup.lock_delivery("tamar").await;

        let switch = tokio::spawn({
            let (sup, ctx) = (sup.clone(), ctx.clone());
            async move { sup.checkout_pr(&ctx, "tamar", None, 7).await }
        });
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(!switch.is_finished(), "held behind the send");
        assert_eq!(branch_of(&checkout).await, "feat/b");

        // The send's turn starts, as `deliver_as_turn` flips it under the lock.
        sup.statuses
            .lock()
            .insert("tamar".to_string(), AgentStatus::Running);
        drop(sending);

        let err = switch.await.unwrap().unwrap_err();
        assert!(err.to_string().contains("agent is working"), "{err}");
        assert_eq!(branch_of(&checkout).await, "feat/b");
        assert_eq!(
            focused(&sup, "tamar"),
            (Some(8), Some("feat/b".to_string()))
        );
    }

    /// A refused switch leaves nothing held: not the delivery lock it took on
    /// the way, and not the tree, which the switch after it then gets.
    #[tokio::test]
    async fn a_switch_refused_for_a_tree_already_moving_holds_nothing_afterwards() {
        let td = tempfile::tempdir().unwrap();
        let (sup, checkout) = agent_on_two_prs(td.path(), "wye").await;
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        let key = ("wye".to_string(), None);
        let reserved =
            delegation::reserve_tree(delegation::global(), crate::review::engine::global(), &key)
                .unwrap();

        // The tree is taken, so this switch is turned away…
        let err = sup.checkout_pr(&ctx, "wye", None, 7).await.unwrap_err();
        assert!(err.to_string().contains("switching branches"), "{err}");
        // …having released the delivery lock it took on the way.
        assert!(sup.delivery_locks.lock()["wye"].try_lock().is_ok());
        assert_eq!(branch_of(&checkout).await, "feat/b");

        drop(reserved);
        sup.checkout_pr(&ctx, "wye", None, 7).await.unwrap();
        assert_eq!(branch_of(&checkout).await, "feat/a");
        assert!(!delegation::tree_moving(&key), "released when done");
    }
}
