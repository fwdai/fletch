//! The context layer's one front door for everything outside the module:
//! the agent ops, the host commands, the ingesters and the extractor all
//! write through here and nowhere else (an invariant test keeps it so).
//!
//! What it owns, so no caller can skip it:
//! - the gate: a project is resolved through [`ContextService::open`], which
//!   refuses when the developer gate or the project's own flag is off — and
//!   the gate is read again on every operation: [`ContextService::check`]
//!   for a read, and inside every write's transaction by the store
//!   (`require_enabled_for`), so a [`Project`] held from when the layer was
//!   on buys nothing once it is off;
//! - trust: `user_quote` is verified against the user's own turns by
//!   [`super::trust`], never taken from the caller;
//! - the write policy: every assertion goes through [`ContextStore::land`],
//!   which classifies and decides under one transaction;
//! - settlement: a checkout's outcome confirms or abandons exactly the
//!   provisional records made in that checkout;
//! - the change pulse: every write that changes what a project's context
//!   says ends in one [`super::CHANGED_EVENT`] on the host's sink, whichever
//!   writer made it, so every open tab — local or remote — reloads.
//!
//! Reads that serve context go through [`ContextService::graph`] (gated);
//! the overview and the pipeline's own bookkeeping tables (observations,
//! extractor runs) reach the store through [`ContextService::store`].

use rusqlite::Connection;
use serde::Serialize;

use super::model::*;
use super::store::{ContextStore, Db};
use super::trust;
use super::{ContextError, Result};
use crate::host::Sink;

/// A project the gate let through: its context id and its host-local id. A
/// name for the project, not a lasting permission — see the module doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub fletch_id: String,
}

#[derive(Clone)]
pub struct ContextService {
    store: ContextStore,
    db: Db,
    sink: Sink,
}

#[derive(Serialize)]
struct Changed<'a> {
    project_id: &'a str,
}

impl ContextService {
    pub fn new(db: Db, sink: Sink) -> Result<Self> {
        Ok(Self {
            store: ContextStore::new(db.clone())?,
            db,
            sink,
        })
    }

    /// The overview's whole read, and the pipeline's own bookkeeping tables
    /// (observations, extractor runs, reads). Every write to the log goes
    /// through the methods below; context served to an agent is read
    /// through [`Self::graph`].
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

    /// The gate, read now, for a project resolved earlier: `Err(Disabled)`
    /// when either switch has since been turned off. What every read of
    /// served context goes through (writes read it again in their own
    /// transaction).
    pub fn check(&self, project: &Project) -> Result<()> {
        if self.is_enabled(&project.fletch_id) {
            Ok(())
        } else {
            Err(ContextError::Disabled)
        }
    }

    /// The project's graph, for what is served to an agent: the gate first.
    pub fn graph(&self, project: &Project) -> Result<Graph> {
        self.check(project)?;
        self.store.load(&project.id)
    }

    /// The one change pulse, after a write landed.
    fn changed(&self, project: &Project) {
        crate::host::emit(
            self.sink.as_ref(),
            super::CHANGED_EVENT,
            &Changed {
                project_id: &project.fletch_id,
            },
        );
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
    ///
    /// Trust is applied here, once, for every writer: a candidate with a
    /// verified user quote *states that quote* — the writer's own statement
    /// is refused if it differs, its reading belongs in the rationale — and
    /// carries the `user_turn` source, confirmed. Without one, the record is
    /// confirmed only when its source is one a person stood behind (the UI,
    /// a merged PR); an agent's or a model's turn is provisional whatever the
    /// writer asked for.
    pub fn record_decision(
        &self,
        project: &Project,
        mut candidate: Candidate,
        mut stamp: Stamp,
    ) -> Result<Landing> {
        match candidate.user.take() {
            Some(found) => {
                let statement = candidate.input.statement.trim();
                if !statement.is_empty()
                    && trust::normalise(statement) != trust::normalise(found.quote())
                {
                    return Err(ContextError::Invalid(
                        "a user-stated record states the user's words: leave `statement` out (or \
                         equal to the quote) and put your reading in `rationale`"
                            .into(),
                    ));
                }
                candidate.input.statement = found.quote().to_string();
                candidate.input.status = AssertionStatus::Confirmed;
                stamp.source = found.source();
            }
            None => {
                let vouched = matches!(
                    stamp.source.kind,
                    SourceKind::Ui | SourceKind::Pr | SourceKind::Roadmap
                );
                if stamp.source.kind == SourceKind::UserTurn {
                    return Err(ContextError::Invalid(
                        "a user_turn source needs a verified quote".into(),
                    ));
                }
                if !vouched && candidate.input.status == AssertionStatus::Confirmed {
                    candidate.input.status = AssertionStatus::Provisional;
                }
            }
        }
        let landing = self.store.land(&project.id, candidate, stamp)?;
        // A duplicate and a two-step reply write nothing.
        if matches!(landing, Landing::Recorded { .. } | Landing::Held { .. }) {
            self.changed(project);
        }
        Ok(landing)
    }

    pub fn record_entity(&self, project: &Project, input: EntityInput, stamp: Stamp) -> Result<Id> {
        let id = self.store.record_entity(&project.id, input, stamp)?;
        self.changed(project);
        Ok(id)
    }

    pub fn link(&self, project: &Project, change: LinkChange, stamp: Stamp) -> Result<()> {
        self.store.link(&project.id, change, stamp)?;
        self.changed(project);
        Ok(())
    }

    pub fn retract(
        &self,
        project: &Project,
        assertion_id: &str,
        reason: &str,
        stamp: Stamp,
    ) -> Result<()> {
        self.store
            .retract(&project.id, assertion_id, reason, stamp)?;
        self.changed(project);
        Ok(())
    }

    pub fn archive_entity(&self, project: &Project, entity_id: &str, stamp: Stamp) -> Result<()> {
        self.store.archive_entity(&project.id, entity_id, stamp)?;
        self.changed(project);
        Ok(())
    }

    pub fn merge_entities(
        &self,
        project: &Project,
        entity_id: &str,
        into: &str,
        stamp: Stamp,
    ) -> Result<()> {
        self.store
            .merge_entities(&project.id, entity_id, into, stamp)?;
        self.changed(project);
        Ok(())
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
            .resolve_contradiction(&project.id, a, b, reasoning, stamp)?;
        self.changed(project);
        Ok(())
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
        let settled =
            self.store
                .settle(&project.id, workspace_id, repo, is_primary, outcome, stamp)?;
        if settled > 0 {
            self.changed(project);
        }
        Ok(settled)
    }

    /// Proposals: add one (held by a writer or the extractor), rule on one.
    /// A ruling is scoped to the project the gate opened: a proposal id from
    /// another project is unknown here (the store keeps that rule).
    pub fn add_proposal(&self, project: &Project, proposal: &Proposal) -> Result<()> {
        if proposal.project_id != project.id {
            return Err(ContextError::Invalid(
                "a proposal is added to the project it was made for".into(),
            ));
        }
        self.store.add_proposal(proposal)?;
        self.changed(project);
        Ok(())
    }

    pub fn accept_proposal(&self, project: &Project, proposal_id: &str, by: Author) -> Result<Id> {
        let id =
            self.store
                .accept_proposal(&project.id, proposal_id, ProposalStatus::Accepted, by)?;
        self.changed(project);
        Ok(id)
    }

    pub fn dismiss_proposal(
        &self,
        project: &Project,
        proposal_id: &str,
        reason: DismissReason,
        by: Author,
    ) -> Result<()> {
        self.store
            .dismiss_proposal(&project.id, proposal_id, reason, by)?;
        self.changed(project);
        Ok(())
    }

    /// For the pipeline modules that need the raw connection for their own
    /// bookkeeping tables (observations, runs).
    pub fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> T) -> T {
        f(&self.db.lock())
    }
}

#[cfg(test)]
#[path = "tests/service.rs"]
mod tests;
