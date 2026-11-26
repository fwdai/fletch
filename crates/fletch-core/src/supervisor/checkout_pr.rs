//! Moving a checkout's working tree to one of its PRs.

use crate::error::{Error, Result};
use crate::git;
use crate::github::PrState;
use crate::host::EngineCtx;
use crate::workspace::AgentStatus;

use super::delegation::{self, checkout_subdir};
use super::events::emit_branch;
use super::Supervisor;

impl Supervisor {
    /// Check out the head branch of PR `number` — one of the checkout's PRs —
    /// and focus it, so the tree, the focused PR and every action about
    /// either are on one branch.
    ///
    /// The checkout is the agent's working directory, so this only runs while
    /// nothing else is in it: no turn, no delegation, no local review. It
    /// carries nothing across either — a tree with uncommitted work is
    /// refused, since `git checkout` would take the changes to the other
    /// branch. Each refusal says what to do instead, and none moves the focus.
    pub async fn checkout_pr(
        &self,
        ctx: &EngineCtx,
        agent_id: &str,
        subdir: Option<&str>,
        number: u32,
    ) -> Result<PrState> {
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

        if matches!(
            self.status_of(agent_id),
            Some(AgentStatus::Running | AgentStatus::Spawning)
        ) {
            return Err(Error::Other(
                "The agent is working in this checkout. Switch branches when its turn ends.".into(),
            ));
        }
        let record = self.workspace.agent(agent_id)?;
        let key = (agent_id.to_string(), checkout_subdir(&record, subdir)?);
        if delegation::global().holds(&key) {
            return Err(Error::Other(
                "The agent is working on this checkout. Switch branches when it is done.".into(),
            ));
        }
        if crate::review::engine::global().reviewing(&key) {
            return Err(Error::Other(
                "A local review is reading this checkout. Stop it to switch branches.".into(),
            ));
        }

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
