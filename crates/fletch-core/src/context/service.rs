//! The context layer's one front door for everything outside the module:
//! the agent ops, the host commands, the ingesters and the extractor all
//! write through here and nowhere else (an invariant test keeps it so).
//!
//! What it owns, so no caller can skip it:
//! - the gate: a project is resolved through [`ContextService::open`], which
//!   refuses when the developer gate or the project's own flag is off;
//! - trust: `user_quote` is verified against the user's own turns by
//!   [`super::trust`], never taken from the caller;
//! - the write policy: every assertion goes through [`ContextStore::land`],
//!   which classifies and decides under one transaction;
//! - settlement: a checkout's outcome confirms or abandons exactly the
//!   provisional records made in that checkout.
//!
//! Reads (`load`, `proposals`, `stats`, `compile`) go to the store directly
//! through [`ContextService::store`]; they change nothing.

use rusqlite::Connection;

use super::model::*;
use super::store::{ContextStore, Db};
use super::trust;
use super::{ContextError, Result};

/// A project the gate let through: its context id and its host-local id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub fletch_id: String,
}

#[derive(Clone)]
pub struct ContextService {
    store: ContextStore,
    db: Db,
}

impl ContextService {
    pub fn new(db: Db) -> Result<Self> {
        Ok(Self {
            store: ContextStore::new(db.clone())?,
            db,
        })
    }

    /// Reads, and the pipeline's own bookkeeping tables (observations,
    /// extractor runs). Every write to the log goes through the methods below.
    pub fn store(&self) -> &ContextStore {
        &self.store
    }

    /// Whether the layer is on for a project at all; the one place the two
    /// flags are read.
    pub fn is_enabled(&self, fletch_project_id: &str) -> bool {
        super::enabled(&self.db.lock(), fletch_project_id)
    }

    /// Resolve a project through the gate. `Err(Disabled)` when the layer is
    /// off for it; the context id is minted on first use.
    pub fn open(&self, fletch_project_id: &str) -> Result<Project> {
        let conn = self.db.lock();
        if !super::enabled(&conn, fletch_project_id) {
            return Err(ContextError::Disabled);
        }
        Ok(Project {
            id: super::context_project_id(&conn, fletch_project_id)?,
            fletch_id: fletch_project_id.to_string(),
        })
    }

    /// The gate plus the extractor's own switch.
    pub fn open_for_extraction(&self, fletch_project_id: &str) -> Result<Option<Project>> {
        let project = self.open(fletch_project_id)?;
        let on = super::extract_enabled(&self.db.lock(), fletch_project_id);
        Ok(on.then_some(project))
    }

    /// Verify a user quote against the user's turns of `workspace_id`
    /// (`trust::find_user_quote`). `Ok(None)` for no quote; `Err` when the
    /// quote is not found — a writer never gets to assert it anyway.
    pub fn verify_user_quote(
        &self,
        workspace_id: &str,
        quote: Option<&str>,
    ) -> Result<Option<trust::UserStated>> {
        let Some(quote) = quote.map(str::trim).filter(|q| !q.is_empty()) else {
            return Ok(None);
        };
        let turns: Vec<trust::UserTurnText> =
            crate::workspace::WorkspaceManager::new(self.db.clone())
                .read_history_turns(workspace_id)
                .map_err(|e| {
                    ContextError::Invalid(format!("could not read the user's turns: {e}"))
                })?
                .into_iter()
                .map(|t| trust::UserTurnText {
                    turn_id: t.turn_id,
                    text: t.text,
                })
                .collect();
        trust::find_user_quote(&turns, quote)
            .map(Some)
            .ok_or_else(|| {
                ContextError::Invalid(format!(
                    "`user_quote` was not found in the user's messages of this workspace (quote at \
                     least {} characters of what they wrote, verbatim). Leave it out to record this \
                     as your own, provisional, statement.",
                    trust::MIN_QUOTE_CHARS
                ))
            })
    }

    /// Record an assertion under the one write policy ([`Landing`]).
    pub fn record_decision(
        &self,
        project: &Project,
        candidate: Candidate,
        stamp: Stamp,
    ) -> Result<Landing> {
        self.store.land(&project.id, candidate, stamp)
    }

    pub fn record_entity(&self, project: &Project, input: EntityInput, stamp: Stamp) -> Result<Id> {
        self.store.record_entity(&project.id, input, stamp)
    }

    pub fn link(&self, project: &Project, change: LinkChange, stamp: Stamp) -> Result<()> {
        self.store.link(&project.id, change, stamp)
    }

    pub fn retract(
        &self,
        project: &Project,
        assertion_id: &str,
        reason: &str,
        stamp: Stamp,
    ) -> Result<()> {
        self.store.retract(&project.id, assertion_id, reason, stamp)
    }

    pub fn archive_entity(&self, project: &Project, entity_id: &str, stamp: Stamp) -> Result<()> {
        self.store.archive_entity(&project.id, entity_id, stamp)
    }

    pub fn merge_entities(
        &self,
        project: &Project,
        entity_id: &str,
        into: &str,
        stamp: Stamp,
    ) -> Result<()> {
        self.store
            .merge_entities(&project.id, entity_id, into, stamp)
    }

    pub fn resolve_contradiction(
        &self,
        project: &Project,
        a: &str,
        b: &str,
        reasoning: &str,
        stamp: Stamp,
    ) -> Result<()> {
        self.store
            .resolve_contradiction(&project.id, a, b, reasoning, stamp)
    }

    /// One checkout's fate settles its provisional records: `repo` is the
    /// checkout subdir, `is_primary` whether it is the workspace's first
    /// repo (whose fate also covers records stamped with no repo).
    pub fn settle(
        &self,
        project: &Project,
        workspace_id: &str,
        repo: &str,
        is_primary: bool,
        merged: bool,
        stamp: Stamp,
    ) -> Result<usize> {
        let outcome = if merged {
            AssertionStatus::Confirmed
        } else {
            AssertionStatus::Abandoned
        };
        self.store
            .settle(&project.id, workspace_id, repo, is_primary, outcome, stamp)
    }

    /// Proposals: add one (held by a writer or the extractor), rule on one.
    pub fn add_proposal(&self, proposal: &Proposal) -> Result<()> {
        self.store.add_proposal(proposal)
    }

    pub fn accept_proposal(&self, project: &Project, proposal_id: &str, by: Author) -> Result<Id> {
        self.require_proposal(project, proposal_id)?;
        self.store
            .accept_proposal(proposal_id, ProposalStatus::Accepted, by)
    }

    pub fn dismiss_proposal(
        &self,
        project: &Project,
        proposal_id: &str,
        reason: DismissReason,
        by: Author,
    ) -> Result<()> {
        self.require_proposal(project, proposal_id)?;
        self.store.dismiss_proposal(proposal_id, reason, by)
    }

    /// A ruling is scoped to the project the gate opened: a proposal id from
    /// another project is "no such proposal" here.
    fn require_proposal(&self, project: &Project, proposal_id: &str) -> Result<()> {
        match self.store.proposal(proposal_id)? {
            Some(p) if p.project_id == project.id => Ok(()),
            _ => Err(ContextError::Invalid(
                "no such proposal in this project".into(),
            )),
        }
    }

    /// For the pipeline modules that need the raw connection for their own
    /// bookkeeping tables (observations, runs).
    pub fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> T) -> T {
        f(&self.db.lock())
    }
}
