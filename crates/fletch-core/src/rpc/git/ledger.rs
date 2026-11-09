//! What a push is authorized against, read when the push happens.
//!
//! Never a copy taken when the dispatcher was built: within one session a merge
//! closes the lower PR of a stack, a new PR moves the recorded base, and a
//! force moves the tip the next force is checked against. Two sources, each
//! the authority for what it answers:
//!
//! - [`BranchLedger`], the host's own records — the checkout's recorded base
//!   and the branches this workspace created on origin, with the commit it last
//!   published to each. Written here, at the push, so the next request reads
//!   it whatever order the watcher applies events in.
//! - [`PullRequestIndex`], GitHub — which open pull requests have a branch as
//!   their head or their base, whoever opened them.

use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::host::EngineCtx;
use crate::rpc::RpcFuture;
use crate::workspace::OwnedBranch;

/// The host's records of one agent's branches. `subdir` names the checkout;
/// `None` is the primary.
pub trait BranchLedger: Send + Sync {
    /// The checkout's recorded base, or `None` when nothing is recorded (the
    /// dispatcher's spawn-time base then stands).
    fn base(&self, subdir: Option<&str>) -> Option<String>;
    /// The branches this workspace created on origin. A read that fails owns
    /// nothing, which only refuses more.
    fn owned(&self, subdir: Option<&str>) -> Vec<OwnedBranch>;
    /// A push landed `tip` on `branch`; `created` when origin held no such
    /// branch before it (`WorkspaceManager::record_branch_push`).
    fn record_push(&self, subdir: Option<&str>, branch: &str, tip: &str, created: bool);
}

/// The workspace database, for one agent.
pub struct WorkspaceLedger {
    ctx: Arc<EngineCtx>,
    agent_id: String,
}

impl WorkspaceLedger {
    pub fn new(ctx: Arc<EngineCtx>, agent_id: &str) -> Self {
        Self {
            ctx,
            agent_id: agent_id.to_string(),
        }
    }

    /// The checkout's subdir and the workspace manager, or `None` before boot
    /// or for a checkout the record no longer tracks.
    fn checkout(
        &self,
        subdir: Option<&str>,
    ) -> Option<(
        Arc<crate::supervisor::Supervisor>,
        crate::workspace::TrackedRepo,
    )> {
        let sup = self.ctx.supervisor()?;
        let record = sup.workspace.agent(&self.agent_id).ok()?;
        let repo = match subdir {
            Some(s) => record.repos.into_iter().find(|r| r.subdir == s)?,
            None => record.repos.into_iter().next()?,
        };
        Some((sup, repo))
    }
}

impl BranchLedger for WorkspaceLedger {
    fn base(&self, subdir: Option<&str>) -> Option<String> {
        self.checkout(subdir)?.1.parent_branch
    }

    fn owned(&self, subdir: Option<&str>) -> Vec<OwnedBranch> {
        let Some((sup, repo)) = self.checkout(subdir) else {
            return Vec::new();
        };
        sup.workspace
            .owned_branches(&self.agent_id, &repo.subdir)
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, agent_id = %self.agent_id, "reading owned branches failed");
                Vec::new()
            })
    }

    fn record_push(&self, subdir: Option<&str>, branch: &str, tip: &str, created: bool) {
        let Some((sup, repo)) = self.checkout(subdir) else {
            return;
        };
        if let Err(e) =
            sup.workspace
                .record_branch_push(&self.agent_id, &repo.subdir, branch, tip, created)
        {
            tracing::warn!(
                error = %e,
                agent_id = %self.agent_id,
                branch = %branch,
                "recording a push failed"
            );
        }
    }
}

/// An in-memory ledger: what a dispatcher without an engine (tests, a nested
/// tool) records into and reads back. Starts empty, so it owns nothing until
/// one of its own pushes creates a branch.
#[derive(Default)]
pub struct MemoryLedger {
    owned: Mutex<Vec<(String, OwnedBranch)>>,
}

impl MemoryLedger {
    #[cfg(test)]
    pub fn owning(branches: &[(&str, Option<&str>)]) -> Self {
        let ledger = Self::default();
        ledger
            .owned
            .lock()
            .extend(branches.iter().map(|(name, tip)| {
                (
                    String::new(),
                    OwnedBranch {
                        name: name.to_string(),
                        tip: tip.map(str::to_string),
                    },
                )
            }));
        ledger
    }
}

impl BranchLedger for MemoryLedger {
    fn base(&self, _subdir: Option<&str>) -> Option<String> {
        None
    }

    fn owned(&self, subdir: Option<&str>) -> Vec<OwnedBranch> {
        let key = subdir.unwrap_or_default();
        self.owned
            .lock()
            .iter()
            .filter(|(s, _)| s == key)
            .map(|(_, b)| b.clone())
            .collect()
    }

    fn record_push(&self, subdir: Option<&str>, branch: &str, tip: &str, created: bool) {
        let key = subdir.unwrap_or_default();
        let mut owned = self.owned.lock();
        match owned.iter_mut().find(|(s, b)| s == key && b.name == branch) {
            Some((_, b)) => b.tip = Some(tip.to_string()),
            None if created => owned.push((
                key.to_string(),
                OwnedBranch {
                    name: branch.to_string(),
                    tip: Some(tip.to_string()),
                },
            )),
            None => {}
        }
    }
}

/// Which side of a pull request a branch is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrSide {
    Head,
    Base,
}

/// Open pull requests, as GitHub knows them now.
pub trait PullRequestIndex: Send + Sync {
    /// Whether an open pull request of `source_repo`'s repository has `branch`
    /// on `side`; `None` when GitHub couldn't answer.
    fn has_open_pr<'a>(
        &'a self,
        source_repo: &'a Path,
        side: PrSide,
        branch: &'a str,
    ) -> RpcFuture<'a, Option<bool>>;
}

/// GitHub's REST pulls list, through the host's token.
pub struct GitHubPrIndex;

impl PullRequestIndex for GitHubPrIndex {
    fn has_open_pr<'a>(
        &'a self,
        source_repo: &'a Path,
        side: PrSide,
        branch: &'a str,
    ) -> RpcFuture<'a, Option<bool>> {
        Box::pin(async move {
            let filter = crate::github::PrListFilter {
                state: "open".into(),
                head: (side == PrSide::Head).then(|| branch.to_string()),
                base: (side == PrSide::Base).then(|| branch.to_string()),
                limit: crate::github::PR_LIST_MAX,
            };
            let rows = crate::github::pr_list_filtered(source_repo, &filter)
                .await
                .ok()??;
            // Matched again here rather than trusted to the filter: the answer
            // authorizes a push, so it must be about this branch exactly.
            Some(rows.iter().any(|pr| match side {
                PrSide::Head => pr.branch == branch,
                PrSide::Base => pr.base == branch,
            }))
        })
    }
}

/// A fixed answer, for tests.
#[cfg(test)]
pub struct StaticPrIndex {
    pub heads: Vec<String>,
    pub bases: Vec<String>,
    /// `false` answers every question with "couldn't ask".
    pub reachable: bool,
}

#[cfg(test)]
impl PullRequestIndex for StaticPrIndex {
    fn has_open_pr<'a>(
        &'a self,
        _source_repo: &'a Path,
        side: PrSide,
        branch: &'a str,
    ) -> RpcFuture<'a, Option<bool>> {
        let listed = match side {
            PrSide::Head => &self.heads,
            PrSide::Base => &self.bases,
        };
        let answer = self.reachable.then(|| listed.iter().any(|b| b == branch));
        Box::pin(async move { answer })
    }
}
