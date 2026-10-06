//! All SQL against `context.*`. Writes append an event and apply it to the
//! projection in one transaction; reads load a project's whole [`Graph`].
//!
//! Nothing here issues `UPDATE` or `DELETE` against `events`; `assertions`
//! rows change only in `status`, and only when a confirmed / abandoned /
//! retracted event is applied. A test greps this file for both.
//!
//! Every write goes through one private [`apply`], and `rebuild_projection`
//! replays the log through the same function, so the live projection and a
//! rebuilt one cannot drift apart.

use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

use super::model::*;
use super::{compile, resolve, ContextError, Result, HOST_ID_KEY, PROJECT_ID_KEY};
use crate::database::{get_setting, now_millis, set_setting};

#[cfg(test)]
#[path = "tests/store.rs"]
mod tests;

/// Same mutex handle as the rest of the engine (`roadmap::Db`, `workflow::Db`):
/// `context.db` is attached to that one connection.
pub type Db = Arc<Mutex<Connection>>;

/// The one writer and reader of `context.*`. Cheap to clone; share it through
/// `EngineCtx`.
#[derive(Clone)]
pub struct ContextStore {
    db: Db,
    /// This host's writer id, read (or minted) from `settings` once.
    host_id: String,
}

/// Read the project's context id from `project_settings`, minting one if the
/// project has none yet. `fletch_project_id` is the host-local `projects.id`.
pub fn context_project_id(conn: &Connection, fletch_project_id: &str) -> Result<String> {
    conn.execute(
        "INSERT OR IGNORE INTO project_settings (project_id, key, value) VALUES (?1, ?2, ?3)",
        params![fletch_project_id, PROJECT_ID_KEY, new_id()],
    )?;
    Ok(conn.query_row(
        "SELECT value FROM project_settings WHERE project_id = ?1 AND key = ?2",
        [fletch_project_id, PROJECT_ID_KEY],
        |r| r.get(0),
    )?)
}

impl ContextStore {
    /// Reads `settings.context.host_id`, minting it on first use.
    pub fn new(db: Db) -> Result<Self> {
        let host_id = {
            let conn = db.lock();
            match get_setting(&conn, HOST_ID_KEY) {
                Some(id) => id,
                None => {
                    let id = new_id();
                    set_setting(&conn, HOST_ID_KEY, &id)
                        .map_err(|e| ContextError::Invalid(e.to_string()))?;
                    id
                }
            }
        };
        Ok(Self { db, host_id })
    }

    /// A store over a fresh temp-dir database initialised by
    /// `database::init` (so `context.db` is attached and migrated exactly as
    /// on a host). For tests in any module; the `TempDir` must outlive the
    /// store. Never used by the running host.
    pub fn temp() -> Result<(Self, tempfile::TempDir)> {
        let dir = tempfile::tempdir().map_err(|e| ContextError::Invalid(e.to_string()))?;
        let db =
            crate::database::init(dir.path()).map_err(|e| ContextError::Invalid(e.to_string()))?;
        // A test host has the developer gate open; what it tests is the layer.
        crate::database::set_setting(&db.lock(), super::DEV_SETTING, "true")
            .map_err(|e| ContextError::Invalid(e.to_string()))?;
        Ok((Self::new(db)?, dir))
    }

    pub fn host_id(&self) -> &str {
        &self.host_id
    }

    pub fn db(&self) -> &Db {
        &self.db
    }

    /// Runs `f` inside one transaction on the locked connection.
    fn write<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self.db.lock();
        let tx = conn.unchecked_transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    /// Inserts the event row (next `seq` for this host, computed in the
    /// caller's transaction) and applies it to the projection.
    fn append(
        &self,
        conn: &Connection,
        project_id: &str,
        stamp: Stamp,
        recorded_at: i64,
        payload: EventPayload,
    ) -> Result<Event> {
        let seq: u64 = conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM context.events WHERE host_id = ?1",
            [&self.host_id],
            |r| r.get(0),
        )?;
        let event = Event {
            id: new_id(),
            project_id: project_id.to_string(),
            host_id: self.host_id.clone(),
            seq,
            recorded_at,
            stamp,
            v: PAYLOAD_VERSION,
            payload,
        };
        conn.execute(
            "INSERT INTO context.events
               (id, project_id, host_id, seq, recorded_at, author, source, provenance, type, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                event.id,
                event.project_id,
                event.host_id,
                event.seq,
                event.recorded_at,
                json(&event.stamp.author)?,
                json(&event.stamp.source)?,
                json(&event.stamp.provenance)?,
                event.payload.type_name(),
                payload_json(&event)?,
            ],
        )?;
        apply(conn, &event)?;
        Ok(event)
    }

    // -- writes ------------------------------------------------------------

    /// Appends `entity_recorded`. `input.id == None` creates (the slug must be
    /// free; a second active `vision` is `VisionExists`); `Some` records a
    /// revision of that entity (the slug may change if the new one is free).
    pub(super) fn record_entity(
        &self,
        project_id: &str,
        input: EntityInput,
        stamp: Stamp,
    ) -> Result<Id> {
        self.write(|conn| self.record_entity_in(conn, project_id, input, stamp))
    }

    fn record_entity_in(
        &self,
        conn: &Connection,
        project_id: &str,
        input: EntityInput,
        stamp: Stamp,
    ) -> Result<Id> {
        let id = match &input.id {
            Some(id) => {
                require_entity(conn, project_id, id)?;
                id.clone()
            }
            None => new_id(),
        };
        let slug = clean_slug(&input.slug)?;
        if slug_taken(conn, project_id, &slug, &id)? {
            return Err(ContextError::SlugTaken(slug));
        }
        if input.kind == EntityKind::Vision && vision_exists(conn, project_id, &id)? {
            return Err(ContextError::VisionExists);
        }
        let name = capped("name", clean_line(&input.name), MAX_NAME)?;
        let name = if name.is_empty() { slug.clone() } else { name };
        let summary = capped("summary", clean_line(&input.summary), MAX_SUMMARY)?;
        let payload = EventPayload::EntityRecorded {
            id: id.clone(),
            slug,
            kind: input.kind,
            name,
            summary,
            aliases: input
                .aliases
                .iter()
                .map(|a| clean_line(a))
                .filter(|a| !a.is_empty())
                .collect(),
            paths: input
                .paths
                .iter()
                .map(|p| clean_line(p))
                .filter(|p| !p.is_empty())
                .collect(),
        };
        self.append(conn, project_id, stamp, now_millis(), payload)?;
        Ok(id)
    }

    /// Appends `assertion_recorded` with its `about`, `supersedes` and
    /// `contradicts` edges. Errors: `NoSubject`, `MissingReasoning`, `NotHead`
    /// (the superseded assertion is not current), `UnknownEntity`,
    /// `UnknownAssertion`, `Invalid` (a supersession across kinds, domains
    /// or subjects). The raw primitive: writers go through [`Self::land`];
    /// only the store's own tests seed with it.
    #[cfg(test)]
    pub(super) fn record_assertion(
        &self,
        project_id: &str,
        input: AssertionInput,
        stamp: Stamp,
    ) -> Result<Id> {
        self.write(|conn| {
            let graph = load_graph(conn, project_id)?;
            self.record_assertion_in(conn, project_id, &graph, input, stamp)
        })
    }

    /// `graph` is the project's projection loaded in this transaction: the
    /// supersession target must be current by `compile::is_current` — the
    /// same rule the read model and the landing policy apply — not merely
    /// without a successor row, or a head whose successor was abandoned could
    /// never be superseded again.
    fn record_assertion_in(
        &self,
        conn: &Connection,
        project_id: &str,
        graph: &Graph,
        input: AssertionInput,
        stamp: Stamp,
    ) -> Result<Id> {
        if input.about.is_empty() {
            return Err(ContextError::NoSubject);
        }
        let statement = capped("statement", clean_line(&input.statement), MAX_STATEMENT)?;
        if statement.is_empty() {
            return Err(ContextError::Invalid(
                "an assertion needs a statement".into(),
            ));
        }
        let rationale = capped("rationale", clean_line(&input.rationale), MAX_RATIONALE)?;
        for entity_id in &input.about {
            require_entity(conn, project_id, entity_id)?;
        }
        if let Some(sup) = &input.supersedes {
            if sup.reasoning.trim().is_empty() {
                return Err(ContextError::MissingReasoning);
            }
            require_assertion(conn, project_id, &sup.id)?;
            let target = graph
                .assertion(&sup.id)
                .filter(|t| compile::is_current(graph, t))
                .ok_or_else(|| ContextError::NotHead(sup.id.clone()))?;
            let shared = target.about.iter().any(|e| input.about.contains(e));
            if target.kind != input.kind || target.domain != input.domain || !shared {
                return Err(ContextError::Invalid(
                    "a decision supersedes a decision about the same entity in the same domain: \
                     the superseded assertion must share the kind, the domain and a subject"
                        .into(),
                ));
            }
        }
        for c in &input.contradicts {
            require_assertion(conn, project_id, &c.id)?;
        }
        let id = new_id();
        let recorded_at = now_millis();
        let payload = EventPayload::AssertionRecorded {
            id: id.clone(),
            kind: input.kind,
            domain: input.domain,
            stance: input.stance,
            statement,
            rationale,
            valid_from: input.valid_from.unwrap_or(recorded_at),
            paths: input.paths,
            about: input.about,
            supersedes: input.supersedes,
            contradicts: input.contradicts,
            status: input.status,
        };
        self.append(conn, project_id, stamp, recorded_at, payload)?;
        Ok(id)
    }

    /// Appends `linked` / `unlinked`. Linking an edge that exists, or
    /// unlinking one that does not, is a no-op that still records the event.
    pub(super) fn link(&self, project_id: &str, change: LinkChange, stamp: Stamp) -> Result<()> {
        if change.from == change.to {
            return Err(ContextError::Invalid(
                "an entity cannot relate to itself".into(),
            ));
        }
        self.write(|conn| {
            require_entity(conn, project_id, &change.from)?;
            require_entity(conn, project_id, &change.to)?;
            let LinkChange { from, to, rel, add } = change;
            let payload = if add {
                EventPayload::Linked { from, to, rel }
            } else {
                EventPayload::Unlinked { from, to, rel }
            };
            self.append(conn, project_id, stamp, now_millis(), payload)?;
            Ok(())
        })
    }

    pub(super) fn retract(
        &self,
        project_id: &str,
        assertion_id: &str,
        reason: &str,
        stamp: Stamp,
    ) -> Result<()> {
        if reason.trim().is_empty() {
            return Err(ContextError::Invalid(
                "retracting an assertion needs a reason".into(),
            ));
        }
        self.assertion_event(
            project_id,
            assertion_id,
            stamp,
            EventPayload::Retracted {
                assertion_id: assertion_id.to_string(),
                reason: reason.to_string(),
            },
        )
    }

    /// The one write policy: classify the candidate against what stands now,
    /// apply the rule for the stamp's author kind, and record, park or refuse
    /// — all under one transaction, so no writer can race the check. See
    /// [`Landing`] for the policy table.
    pub(super) fn land(
        &self,
        project_id: &str,
        candidate: Candidate,
        stamp: Stamp,
    ) -> Result<Landing> {
        let policy_for = stamp.author.kind;
        self.write(|conn| self.land_in(conn, project_id, candidate, stamp, policy_for))
    }

    /// [`Self::land`] inside the caller's transaction. `policy_for` is the
    /// actor whose rule applies: a writer's own kind, or `User` when a person
    /// accepts a proposal (the ruling is theirs; `stamp` stays the
    /// proposer's, for provenance).
    fn land_in(
        &self,
        conn: &Connection,
        project_id: &str,
        candidate: Candidate,
        stamp: Stamp,
        policy_for: AuthorKind,
    ) -> Result<Landing> {
        let Candidate {
            mut input,
            relation,
            evidence,
            about_pending,
            observation_id,
            user: _,
        } = candidate;
        let about_pending = resolve_pending(conn, project_id, &mut input, about_pending)?;
        let graph = load_graph(conn, project_id)?;
        // A restatement is a duplicate — unless it is the very head the
        // candidate revises (same words, new rationale or stance).
        if let ProposedRelation {
            kind: RelationKind::Duplicate,
            target: Some(id),
            ..
        } = resolve::classify(&graph, &input)
        {
            let revising_it = input.supersedes.as_ref().is_some_and(|s| s.id == id)
                || relation.as_ref().is_some_and(|r| {
                    r.kind == RelationKind::Supersedes && r.target.as_deref() == Some(&*id)
                });
            if !revising_it {
                return Ok(Landing::Duplicate { id });
            }
        }
        // An explicit relation is honoured only against a head that
        // stands now; anything else is "the store decides".
        let relation = relation.filter(|r| match r.kind {
            RelationKind::Supersedes | RelationKind::Contradicts => r
                .target
                .as_deref()
                .and_then(|id| graph.assertion(id))
                .is_some_and(|t| compile::is_current(&graph, t)),
            _ => true,
        });
        let held = match policy_for {
            AuthorKind::User => false,
            AuthorKind::Agent => {
                if relation.is_none() && input.supersedes.is_none() && input.contradicts.is_empty()
                {
                    let heads = resolve::related_heads(&graph, &input);
                    if !heads.is_empty() {
                        return Ok(Landing::Related {
                            heads: heads.into_iter().cloned().collect(),
                        });
                    }
                }
                false
            }
            // Deterministic sources land next to what is there; only a
            // person decides what replaces what.
            AuthorKind::Ingester => false,
            AuthorKind::Extractor => true,
        };
        if held {
            let proposal = Proposal {
                id: new_id(),
                project_id: project_id.to_string(),
                observation_id,
                payload: ProposalPayload::Assertion {
                    input,
                    stamp,
                    relation: relation.unwrap_or(ProposedRelation {
                        kind: RelationKind::New,
                        target: None,
                        reasoning: None,
                    }),
                    about_pending,
                },
                evidence,
                status: ProposalStatus::Pending,
                dismiss_reason: None,
                created_at: now_millis(),
                ruled_at: None,
                ruled_by: None,
            };
            insert_proposal(conn, &proposal)?;
            return Ok(Landing::Held {
                proposal_id: proposal.id,
            });
        }
        require_accepted(&about_pending)?;
        if let Some(relation) = &relation {
            with_relation(&mut input, relation)?;
        }
        let status = input.status;
        let id = self.record_assertion_in(conn, project_id, &graph, input, stamp)?;
        Ok(Landing::Recorded { id, status })
    }

    /// Settle the provisional assertions one checkout of a workspace made:
    /// `outcome` is `Confirmed` (its PR merged) or `Abandoned` (archived
    /// without merging). Matches `provenance.repo == repo`; the primary
    /// checkout (`is_primary`) also settles records stamped with no repo.
    /// Returns how many.
    pub(super) fn settle(
        &self,
        project_id: &str,
        workspace_id: &str,
        repo: &str,
        is_primary: bool,
        outcome: AssertionStatus,
        stamp: Stamp,
    ) -> Result<usize> {
        let payload = |assertion_id: String| match outcome {
            AssertionStatus::Confirmed => Ok(EventPayload::Confirmed { assertion_id }),
            AssertionStatus::Abandoned => Ok(EventPayload::Abandoned { assertion_id }),
            other => Err(ContextError::Invalid(format!(
                "a checkout settles to confirmed or abandoned, not {}",
                tag(&other)?
            ))),
        };
        self.write(|conn| {
            let ids = provisional_in_checkout(conn, project_id, workspace_id, repo, is_primary)?;
            for id in &ids {
                self.append(
                    conn,
                    project_id,
                    stamp.clone(),
                    now_millis(),
                    payload(id.clone())?,
                )?;
            }
            Ok(ids.len())
        })
    }

    /// Close a `contradicts` edge with a ruling. Both sides stay as they are.
    /// The edge must exist (in either orientation); a later ruling replaces
    /// an earlier one.
    pub(super) fn resolve_contradiction(
        &self,
        project_id: &str,
        a: &str,
        b: &str,
        reasoning: &str,
        stamp: Stamp,
    ) -> Result<()> {
        let reasoning = clean_line(reasoning);
        if reasoning.is_empty() {
            return Err(ContextError::Invalid(
                "resolving a contradiction needs reasoning".into(),
            ));
        }
        self.write(|conn| {
            require_assertion(conn, project_id, a)?;
            require_assertion(conn, project_id, b)?;
            if !contradiction_exists(conn, a, b)? {
                return Err(no_contradiction(a, b));
            }
            let payload = EventPayload::ContradictionResolved {
                a: a.to_string(),
                b: b.to_string(),
                reasoning,
            };
            self.append(conn, project_id, stamp, now_millis(), payload)?;
            Ok(())
        })
    }

    /// One assertion's settlement; writers settle a checkout at a time
    /// through [`Self::settle`], so these two are the store's tests' only.
    #[cfg(test)]
    pub(super) fn confirm(&self, project_id: &str, assertion_id: &str, stamp: Stamp) -> Result<()> {
        self.assertion_event(
            project_id,
            assertion_id,
            stamp,
            EventPayload::Confirmed {
                assertion_id: assertion_id.to_string(),
            },
        )
    }

    #[cfg(test)]
    pub(super) fn abandon(&self, project_id: &str, assertion_id: &str, stamp: Stamp) -> Result<()> {
        self.assertion_event(
            project_id,
            assertion_id,
            stamp,
            EventPayload::Abandoned {
                assertion_id: assertion_id.to_string(),
            },
        )
    }

    fn assertion_event(
        &self,
        project_id: &str,
        assertion_id: &str,
        stamp: Stamp,
        payload: EventPayload,
    ) -> Result<()> {
        self.write(|conn| {
            require_assertion(conn, project_id, assertion_id)?;
            self.append(conn, project_id, stamp, now_millis(), payload)?;
            Ok(())
        })
    }

    pub(super) fn archive_entity(
        &self,
        project_id: &str,
        entity_id: &str,
        stamp: Stamp,
    ) -> Result<()> {
        self.write(|conn| {
            require_entity(conn, project_id, entity_id)?;
            let payload = EventPayload::EntityArchived {
                id: entity_id.to_string(),
            };
            self.append(conn, project_id, stamp, now_millis(), payload)?;
            Ok(())
        })
    }

    /// Redirects every `about` and `relates` edge of `entity_id` to `into`
    /// and leaves `entity_id` in `merged` status; entities already merged
    /// into `entity_id` are pointed at `into` too, so a chain stays one hop.
    /// `into` must be active and must not lead back to `entity_id`.
    pub(super) fn merge_entities(
        &self,
        project_id: &str,
        entity_id: &str,
        into: &str,
        stamp: Stamp,
    ) -> Result<()> {
        if entity_id == into {
            return Err(ContextError::Invalid(
                "an entity cannot be merged into itself".into(),
            ));
        }
        self.write(|conn| {
            // Both sides active: an archived or already-merged source has
            // nothing left to move, and an inactive target cannot be
            // reached — which is also what rules out a cycle.
            for (which, id) in [("source", entity_id), ("target", into)] {
                let (status, _) = entity_state(conn, project_id, id)?;
                if status != EntityStatus::Active {
                    return Err(ContextError::Invalid(format!(
                        "`{id}` is {}; the merge {which} must be an active entity",
                        tag(&status)?
                    )));
                }
            }
            let payload = EventPayload::EntityMerged {
                id: entity_id.to_string(),
                into: into.to_string(),
            };
            self.append(conn, project_id, stamp, now_millis(), payload)?;
            Ok(())
        })
    }

    // -- reads -------------------------------------------------------------

    /// The whole projection for one project.
    pub fn load(&self, project_id: &str) -> Result<Graph> {
        load_graph(&self.db.lock(), project_id)
    }

    /// Every event of one project in `(recorded_at, host_id, seq)` order —
    /// the order a replay applies them in.
    pub fn events(&self, project_id: &str) -> Result<Vec<Event>> {
        let conn = self.db.lock();
        load_events(&conn, project_id)
    }

    /// Drops the project's projection rows and replays its events. Recovery
    /// and the replay-equivalence test; never part of a normal write.
    pub fn rebuild_projection(&self, project_id: &str) -> Result<()> {
        self.write(|conn| {
            clear_projection(conn, project_id)?;
            for event in load_events(conn, project_id)? {
                apply(conn, &event)?;
            }
            Ok(())
        })
    }

    // -- pipeline ----------------------------------------------------------

    pub fn add_observation(&self, observation: &Observation) -> Result<()> {
        let conn = self.db.lock();
        conn.execute(
            "INSERT INTO context.observations
               (id, project_id, source, provenance, input_hash, plan, created_at, extracted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                observation.id,
                observation.project_id,
                json(&observation.source)?,
                json(&observation.provenance)?,
                observation.input_hash,
                observation.plan,
                observation.created_at,
                observation.extracted_at,
            ],
        )?;
        Ok(())
    }

    pub fn mark_extracted(&self, observation_id: &str) -> Result<()> {
        let conn = self.db.lock();
        conn.execute(
            "UPDATE context.observations SET extracted_at = ?2 WHERE id = ?1",
            params![observation_id, now_millis()],
        )?;
        Ok(())
    }

    pub fn add_extractor_run(&self, run: &ExtractorRun) -> Result<()> {
        let conn = self.db.lock();
        conn.execute(
            "INSERT INTO context.extractor_runs
               (id, observation_id, model, prompt_version, output, tokens_in, tokens_out,
                duration_ms, error, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                run.id,
                run.observation_id,
                run.model,
                run.prompt_version,
                run.output,
                run.tokens_in,
                run.tokens_out,
                run.duration_ms,
                run.error,
                run.created_at,
            ],
        )?;
        Ok(())
    }

    pub(super) fn add_proposal(&self, proposal: &Proposal) -> Result<()> {
        insert_proposal(&self.db.lock(), proposal)
    }

    pub fn proposals(
        &self,
        project_id: &str,
        status: Option<ProposalStatus>,
    ) -> Result<Vec<Proposal>> {
        let conn = self.db.lock();
        let status = status.map(|s| tag(&s)).transpose()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {PROPOSAL_COLUMNS} FROM context.proposals
             WHERE project_id = ?1 AND (?2 IS NULL OR status = ?2)
             ORDER BY created_at, id"
        ))?;
        let rows = stmt.query_map(params![project_id, status], proposal_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn proposal(&self, proposal_id: &str) -> Result<Option<Proposal>> {
        let conn = self.db.lock();
        get_proposal(&conn, proposal_id)
    }

    /// Lands a proposal and marks it `status` (`Accepted` by the user, `Auto`
    /// by rule). An entity proposal is recorded as is. An assertion proposal
    /// is rebuilt as a [`Candidate`] and goes through [`Self::land_in`] under
    /// the user's rule — classified against what stands *now*, not at
    /// proposal time: a restatement of a current head dismisses the proposal
    /// as `Duplicate` (returning that head); a `supersedes` / `contradicts`
    /// target that is no longer current is `Invalid` and the proposal stays
    /// pending. `Confirms` records nothing: when confirmed it settles its
    /// target. Subjects that were only proposed (`about_pending`) are
    /// resolved now; one still missing is `Invalid("accept the entity …
    /// first")`. Returns the recorded (or duplicated) id.
    pub(super) fn accept_proposal(
        &self,
        proposal_id: &str,
        status: ProposalStatus,
        ruled_by: Author,
    ) -> Result<Id> {
        self.write(|conn| {
            let proposal = require_pending(conn, proposal_id)?;
            let project_id = &proposal.project_id;
            let (input, stamp, relation, about_pending) = match proposal.payload {
                ProposalPayload::Entity { input, stamp } => {
                    let id = self.record_entity_in(conn, project_id, input, stamp)?;
                    rule_proposal(conn, proposal_id, status, None, &ruled_by)?;
                    return Ok(id);
                }
                ProposalPayload::Assertion {
                    input,
                    stamp,
                    relation,
                    about_pending,
                } => (input, stamp, relation, about_pending),
            };
            // A human ruling is a confirmation: what the user accepts is not
            // waiting on any branch.
            let mut input = input;
            if status == ProposalStatus::Accepted {
                input.status = AssertionStatus::Confirmed;
            }
            match relation.kind {
                RelationKind::Confirms => {
                    // Nothing new is said; a confirmed restatement settles a
                    // provisional target.
                    let id = relation_target(&relation)?;
                    if input.status == AssertionStatus::Confirmed {
                        let event = EventPayload::Confirmed {
                            assertion_id: id.clone(),
                        };
                        self.append(conn, project_id, stamp, now_millis(), event)?;
                    }
                    rule_proposal(conn, proposal_id, status, None, &ruled_by)?;
                    return Ok(id);
                }
                // A duplicate's target counts too: a restatement of a head
                // that has since been replaced must not come back as new.
                RelationKind::Supersedes | RelationKind::Contradicts | RelationKind::Duplicate => {
                    let target = relation_target(&relation)?;
                    let graph = load_graph(conn, project_id)?;
                    let current = graph
                        .assertion(&target)
                        .is_some_and(|t| compile::is_current(&graph, t));
                    if !current {
                        return Err(ContextError::Invalid(format!(
                            "assertion `{target}` is no longer current; \
                             dismiss this proposal or record it afresh"
                        )));
                    }
                }
                RelationKind::New => {}
            }
            let candidate = Candidate {
                input,
                user: None,
                relation: Some(relation),
                evidence: proposal.evidence,
                about_pending,
                observation_id: proposal.observation_id,
            };
            match self.land_in(conn, project_id, candidate, stamp, AuthorKind::User)? {
                Landing::Recorded { id, .. } => {
                    rule_proposal(conn, proposal_id, status, None, &ruled_by)?;
                    Ok(id)
                }
                Landing::Duplicate { id } => {
                    rule_proposal(
                        conn,
                        proposal_id,
                        ProposalStatus::Dismissed,
                        Some(DismissReason::Duplicate),
                        &ruled_by,
                    )?;
                    Ok(id)
                }
                other => Err(ContextError::Invalid(format!(
                    "internal: a ruling cannot be parked ({other:?})"
                ))),
            }
        })
    }

    pub(super) fn dismiss_proposal(
        &self,
        proposal_id: &str,
        reason: DismissReason,
        ruled_by: Author,
    ) -> Result<()> {
        self.write(|conn| {
            require_pending(conn, proposal_id)?;
            rule_proposal(
                conn,
                proposal_id,
                ProposalStatus::Dismissed,
                Some(reason),
                &ruled_by,
            )
        })
    }

    // -- telemetry ---------------------------------------------------------

    pub fn log_read(&self, read: &ReadRecord) -> Result<()> {
        let conn = self.db.lock();
        let served = serde_json::json!({
            "entities": read.served_entities,
            "assertions": read.served_assertions,
        });
        conn.execute(
            "INSERT INTO context.reads
               (id, project_id, agent_id, workspace_id, session_id, query, served, misses,
                chars, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                new_id(),
                read.project_id,
                read.agent_id,
                read.workspace_id,
                read.session_id,
                json(&read.query)?,
                served.to_string(),
                json(&read.misses)?,
                read.chars as i64,
                now_millis(),
            ],
        )?;
        Ok(())
    }

    pub fn stats(&self, project_id: &str) -> Result<Stats> {
        let conn = self.db.lock();
        let graph = load_graph(&conn, project_id)?;
        // The rule compile and the UI apply: a tension counts while it is
        // unresolved and both sides stand now.
        let contradictions = graph
            .contradictions
            .iter()
            .filter(|c| {
                c.resolution.is_none()
                    && graph.current.contains(&c.a)
                    && graph.current.contains(&c.b)
            })
            .count();
        let count = |sql: &str| -> Result<usize> {
            let n: i64 = conn.query_row(sql, [project_id], |r| r.get(0))?;
            Ok(n as usize)
        };
        let mut stmt = conn.prepare(
            "SELECT m.value, COUNT(*) AS n
             FROM context.reads r, json_each(r.misses) m
             WHERE r.project_id = ?1
             GROUP BY m.value ORDER BY n DESC, m.value LIMIT 10",
        )?;
        let top_misses = stmt
            .query_map([project_id], |r| {
                Ok((r.get(0)?, r.get::<_, i64>(1)? as usize))
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Stats {
            entities: count(
                "SELECT COUNT(*) FROM context.entities WHERE project_id = ?1 AND status = 'active'",
            )?,
            assertions: count("SELECT COUNT(*) FROM context.assertions WHERE project_id = ?1")?,
            heads: graph.current.len(),
            provisional: count(
                "SELECT COUNT(*) FROM context.assertions
                 WHERE project_id = ?1 AND status = 'provisional'",
            )?,
            contradictions,
            pending_proposals: count(
                "SELECT COUNT(*) FROM context.proposals
                 WHERE project_id = ?1 AND status = 'pending'",
            )?,
            reads: count("SELECT COUNT(*) FROM context.reads WHERE project_id = ?1")?,
            top_misses,
        })
    }
}

// ---------------------------------------------------------------------------
// The projector

/// Applies one event to the projection. The only code that writes
/// `entities`, `assertions` and the edge tables, for live writes and replay
/// alike. An event that targets a row that is not there is an error, not a
/// zero-row update: live writes validate first, so this bites only a bad
/// replay, which must be loud.
fn apply(conn: &Connection, event: &Event) -> Result<()> {
    match &event.payload {
        EventPayload::EntityRecorded {
            id,
            slug,
            kind,
            name,
            summary,
            aliases,
            paths,
        } => {
            conn.execute(
                "INSERT INTO context.entities
                   (id, project_id, slug, kind, name, summary, aliases, paths, status,
                    recorded_at, author, source)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'active', ?9, ?10, ?11)
                 ON CONFLICT(id) DO UPDATE SET
                   slug = excluded.slug, kind = excluded.kind, name = excluded.name,
                   summary = excluded.summary, aliases = excluded.aliases,
                   paths = excluded.paths, recorded_at = excluded.recorded_at,
                   author = excluded.author, source = excluded.source",
                params![
                    id,
                    event.project_id,
                    slug,
                    tag(kind)?,
                    name,
                    summary,
                    json(aliases)?,
                    json(paths)?,
                    event.recorded_at,
                    json(&event.stamp.author)?,
                    json(&event.stamp.source)?,
                ],
            )?;
        }
        EventPayload::EntityArchived { id } => {
            let n = conn.execute(
                "UPDATE context.entities SET status = 'archived' WHERE id = ?1",
                [id],
            )?;
            if n == 0 {
                return Err(ContextError::UnknownEntity(id.clone()));
            }
        }
        EventPayload::EntityMerged { id, into } => {
            require_entity(conn, &event.project_id, into)?;
            conn.execute(
                "INSERT OR IGNORE INTO context.about (assertion_id, entity_id)
                 SELECT assertion_id, ?2 FROM context.about WHERE entity_id = ?1",
                [id, into],
            )?;
            conn.execute("DELETE FROM context.about WHERE entity_id = ?1", [id])?;
            conn.execute(
                "INSERT OR IGNORE INTO context.relates (from_id, to_id, rel)
                 SELECT ?2, to_id, rel FROM context.relates WHERE from_id = ?1",
                [id, into],
            )?;
            conn.execute("DELETE FROM context.relates WHERE from_id = ?1", [id])?;
            conn.execute(
                "INSERT OR IGNORE INTO context.relates (from_id, to_id, rel)
                 SELECT from_id, ?2, rel FROM context.relates WHERE to_id = ?1",
                [id, into],
            )?;
            conn.execute("DELETE FROM context.relates WHERE to_id = ?1", [id])?;
            // A relation between the two becomes a self-relation: drop it.
            conn.execute(
                "DELETE FROM context.relates WHERE from_id = ?1 AND to_id = ?1",
                [into],
            )?;
            let n = conn.execute(
                "UPDATE context.entities SET status = 'merged', merged_into = ?2 WHERE id = ?1",
                [id, into],
            )?;
            if n == 0 {
                return Err(ContextError::UnknownEntity(id.clone()));
            }
            // Path compression: whatever pointed at `id` now points at `into`.
            conn.execute(
                "UPDATE context.entities SET merged_into = ?2 WHERE merged_into = ?1",
                [id, into],
            )?;
        }
        EventPayload::AssertionRecorded {
            id,
            kind,
            domain,
            stance,
            statement,
            rationale,
            valid_from,
            paths,
            about,
            supersedes,
            contradicts,
            status,
        } => {
            conn.execute(
                "INSERT INTO context.assertions
                   (id, project_id, kind, domain, stance, statement, rationale, valid_from,
                    paths, status, recorded_at, author, source, provenance)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    id,
                    event.project_id,
                    tag(kind)?,
                    tag(domain)?,
                    tag(stance)?,
                    statement,
                    rationale,
                    valid_from,
                    json(paths)?,
                    tag(status)?,
                    event.recorded_at,
                    json(&event.stamp.author)?,
                    json(&event.stamp.source)?,
                    json(&event.stamp.provenance)?,
                ],
            )?;
            for entity_id in about {
                require_entity(conn, &event.project_id, entity_id)?;
                conn.execute(
                    "INSERT OR IGNORE INTO context.about (assertion_id, entity_id) VALUES (?1, ?2)",
                    [id, entity_id],
                )?;
            }
            if let Some(sup) = supersedes {
                require_assertion(conn, &event.project_id, &sup.id)?;
                conn.execute(
                    "INSERT OR IGNORE INTO context.supersedes (new_id, old_id, reasoning)
                     VALUES (?1, ?2, ?3)",
                    [id, &sup.id, &sup.reasoning],
                )?;
            }
            for c in contradicts {
                require_assertion(conn, &event.project_id, &c.id)?;
                conn.execute(
                    "INSERT OR IGNORE INTO context.contradicts (a_id, b_id, reasoning)
                     VALUES (?1, ?2, ?3)",
                    params![id, c.id, c.reasoning],
                )?;
            }
        }
        EventPayload::Linked { from, to, rel } => {
            require_entity(conn, &event.project_id, from)?;
            require_entity(conn, &event.project_id, to)?;
            conn.execute(
                "INSERT OR IGNORE INTO context.relates (from_id, to_id, rel) VALUES (?1, ?2, ?3)",
                params![from, to, tag(rel)?],
            )?;
        }
        EventPayload::Unlinked { from, to, rel } => {
            conn.execute(
                "DELETE FROM context.relates WHERE from_id = ?1 AND to_id = ?2 AND rel = ?3",
                params![from, to, tag(rel)?],
            )?;
        }
        EventPayload::Retracted { assertion_id, .. } => {
            set_status(conn, assertion_id, AssertionStatus::Retracted)?;
        }
        EventPayload::Confirmed { assertion_id } => {
            set_status(conn, assertion_id, AssertionStatus::Confirmed)?;
        }
        EventPayload::Abandoned { assertion_id } => {
            set_status(conn, assertion_id, AssertionStatus::Abandoned)?;
        }
        EventPayload::ContradictionResolved { a, b, reasoning } => {
            require_assertion(conn, &event.project_id, a)?;
            require_assertion(conn, &event.project_id, b)?;
            let n = conn.execute(
                "UPDATE context.contradicts
                 SET resolved_at = ?3, resolution = ?4, resolved_by = ?5
                 WHERE (a_id = ?1 AND b_id = ?2) OR (a_id = ?2 AND b_id = ?1)",
                params![
                    a,
                    b,
                    event.recorded_at,
                    reasoning,
                    json(&event.stamp.author)?
                ],
            )?;
            if n == 0 {
                return Err(no_contradiction(a, b));
            }
        }
    }
    Ok(())
}

/// The one mutation an assertion row ever sees.
fn set_status(conn: &Connection, assertion_id: &str, status: AssertionStatus) -> Result<()> {
    let n = conn.execute(
        "UPDATE context.assertions SET status = ?2 WHERE id = ?1",
        params![assertion_id, tag(&status)?],
    )?;
    if n == 0 {
        return Err(ContextError::UnknownAssertion(assertion_id.to_string()));
    }
    Ok(())
}

/// Removes one project's projection rows. Edge tables carry no `project_id`,
/// so their rows are found through the assertions / entities they hang off.
fn clear_projection(conn: &Connection, project_id: &str) -> Result<()> {
    const ASSERTIONS: &str = "SELECT id FROM context.assertions WHERE project_id = ?1";
    const ENTITIES: &str = "SELECT id FROM context.entities WHERE project_id = ?1";
    for sql in [
        format!("DELETE FROM context.about WHERE assertion_id IN ({ASSERTIONS})"),
        format!("DELETE FROM context.supersedes WHERE new_id IN ({ASSERTIONS})"),
        format!("DELETE FROM context.contradicts WHERE a_id IN ({ASSERTIONS})"),
        format!("DELETE FROM context.relates WHERE from_id IN ({ENTITIES})"),
        "DELETE FROM context.assertions WHERE project_id = ?1".to_string(),
        "DELETE FROM context.entities WHERE project_id = ?1".to_string(),
    ] {
        conn.execute(&sql, [project_id])?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Validation lookups

fn require_entity(conn: &Connection, project_id: &str, id: &str) -> Result<()> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM context.entities WHERE id = ?1 AND project_id = ?2",
        [id, project_id],
        |r| r.get(0),
    )?;
    if n == 0 {
        return Err(ContextError::UnknownEntity(id.to_string()));
    }
    Ok(())
}

fn require_assertion(conn: &Connection, project_id: &str, id: &str) -> Result<()> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM context.assertions WHERE id = ?1 AND project_id = ?2",
        [id, project_id],
        |r| r.get(0),
    )?;
    if n == 0 {
        return Err(ContextError::UnknownAssertion(id.to_string()));
    }
    Ok(())
}

/// `(status, merged_into)` of an entity of the project.
fn entity_state(
    conn: &Connection,
    project_id: &str,
    id: &str,
) -> Result<(EntityStatus, Option<Id>)> {
    conn.query_row(
        "SELECT status, merged_into FROM context.entities WHERE id = ?1 AND project_id = ?2",
        [id, project_id],
        |r| Ok((from_tag(&r.get::<_, String>(0)?)?, r.get(1)?)),
    )
    .optional()?
    .ok_or_else(|| ContextError::UnknownEntity(id.to_string()))
}

fn active_entity_by_slug(conn: &Connection, project_id: &str, slug: &str) -> Result<Option<Id>> {
    Ok(conn
        .query_row(
            "SELECT id FROM context.entities
             WHERE project_id = ?1 AND slug = ?2 AND status = 'active'",
            [project_id, slug.trim()],
            |r| r.get(0),
        )
        .optional()?)
}

/// Resolve the subjects that were only proposed when the candidate was made
/// (slugs, any case) against the project's active entities now, adding the
/// found ones to `input.about`; the ones still missing come back.
fn resolve_pending(
    conn: &Connection,
    project_id: &str,
    input: &mut AssertionInput,
    pending: Vec<String>,
) -> Result<Vec<String>> {
    let mut missing = Vec::new();
    for slug in pending {
        match active_entity_by_slug(conn, project_id, &slug)? {
            Some(id) if input.about.contains(&id) => {}
            Some(id) => input.about.push(id),
            None => missing.push(slug),
        }
    }
    Ok(missing)
}

fn require_accepted(missing: &[String]) -> Result<()> {
    match missing.first() {
        Some(slug) => Err(ContextError::Invalid(format!(
            "accept the entity `{slug}` first"
        ))),
        None => Ok(()),
    }
}

fn relation_target(relation: &ProposedRelation) -> Result<Id> {
    relation.target.clone().ok_or_else(|| {
        ContextError::Invalid(format!(
            "a `{}` proposal needs a target assertion",
            tag(&relation.kind).unwrap_or_default()
        ))
    })
}

/// Sets the edges an honoured relation implies on the input. `Confirms` and
/// `Duplicate` imply none: nothing new is recorded for those, and the
/// callers settle them before getting here.
fn with_relation(input: &mut AssertionInput, relation: &ProposedRelation) -> Result<()> {
    match relation.kind {
        RelationKind::New | RelationKind::Confirms | RelationKind::Duplicate => {}
        RelationKind::Supersedes => {
            // The proposer's reasoning when it gave one; the candidate's own
            // rationale otherwise, so a parked supersession can still land.
            let reasoning = relation
                .reasoning
                .clone()
                .filter(|r| !r.trim().is_empty())
                .unwrap_or_else(|| input.rationale.clone());
            input.supersedes = Some(Supersede {
                id: relation_target(relation)?,
                reasoning,
            });
        }
        RelationKind::Contradicts => input.contradicts.push(Contradict {
            id: relation_target(relation)?,
            reasoning: relation.reasoning.clone(),
        }),
    }
    Ok(())
}

fn contradiction_exists(conn: &Connection, a: &str, b: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM context.contradicts
         WHERE (a_id = ?1 AND b_id = ?2) OR (a_id = ?2 AND b_id = ?1)",
        [a, b],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn no_contradiction(a: &str, b: &str) -> ContextError {
    ContextError::Invalid(format!(
        "no contradiction is recorded between `{a}` and `{b}`"
    ))
}

/// Provisional assertions one checkout of a workspace made; the primary
/// checkout also owns records stamped with no repo.
fn provisional_in_checkout(
    conn: &Connection,
    project_id: &str,
    workspace_id: &str,
    repo: &str,
    is_primary: bool,
) -> Result<Vec<Id>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM context.assertions
         WHERE project_id = ?1 AND status = 'provisional'
           AND json_extract(provenance, '$.workspace_id') = ?2
           AND (json_extract(provenance, '$.repo') = ?3
                OR (?4 AND json_extract(provenance, '$.repo') IS NULL))
         ORDER BY recorded_at, id",
    )?;
    let ids = stmt.query_map(params![project_id, workspace_id, repo, is_primary], |r| {
        r.get(0)
    })?;
    Ok(ids.collect::<rusqlite::Result<_>>()?)
}

/// Text caps, enforced here so every writer — agent op, extractor, ingester,
/// UI — lands the same shape; a writer that wants a friendlier message checks
/// the same numbers first.
pub const MAX_NAME: usize = 120;
pub const MAX_SUMMARY: usize = 600;
pub const MAX_STATEMENT: usize = 300;
pub const MAX_RATIONALE: usize = 1000;

fn capped(field: &str, value: String, max: usize) -> Result<String> {
    if value.chars().count() > max {
        return Err(ContextError::Invalid(format!(
            "`{field}` is over {max} characters; keep it short, this is read by every agent"
        )));
    }
    Ok(value)
}

/// Slugs are identifiers that end up inside every agent's instruction block,
/// so they are one token of `[a-z0-9._-]`, lowercase, at most 64 characters.
/// Anything else is refused rather than quietly rewritten: a writer that
/// cannot spell a slug has no business naming an entity.
fn clean_slug(raw: &str) -> Result<String> {
    let slug = raw.trim().to_lowercase();
    let ok = !slug.is_empty()
        && slug.len() <= 64
        && slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && slug
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric());
    if ok {
        Ok(slug)
    } else {
        Err(ContextError::Invalid(format!(
            "slug `{raw}` must be one lowercase token of letters, digits, `-`, `_` or `.` \
             (64 characters at most)"
        )))
    }
}

/// Free text that is rendered into prompts and the UI: control characters
/// (newlines included) become spaces and whitespace collapses, so a value is
/// always one line and cannot shape the markdown around it.
pub(crate) fn clean_line(raw: &str) -> String {
    raw.split(|c: char| c.is_control() || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether another entity of the project (not `entity_id` itself) holds
/// `slug`. The column is `COLLATE NOCASE`, so `=` compares case-insensitively.
fn slug_taken(conn: &Connection, project_id: &str, slug: &str, entity_id: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM context.entities
         WHERE project_id = ?1 AND slug = ?2 AND id != ?3",
        [project_id, slug, entity_id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Whether the project already has an active vision other than `entity_id`.
fn vision_exists(conn: &Connection, project_id: &str, entity_id: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM context.entities
         WHERE project_id = ?1 AND kind = 'vision' AND status = 'active' AND id != ?2",
        [project_id, entity_id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

// ---------------------------------------------------------------------------
// Reading the projection

/// One project's projection off a connection: `load` reads it under the
/// lock, `land` reads it inside its transaction.
fn load_graph(conn: &Connection, project_id: &str) -> Result<Graph> {
    let contradictions = load_contradictions(conn, project_id)?;
    let mut graph = Graph {
        project_id: project_id.to_string(),
        entities: load_entities(conn, project_id)?,
        assertions: load_assertions(conn, project_id, &contradictions)?,
        relations: load_relations(conn, project_id)?,
        current: Vec::new(),
        contradictions,
    };
    graph.current = compile::current_heads(&graph).into_iter().collect();
    graph.current.sort();
    Ok(graph)
}

fn load_entities(conn: &Connection, project_id: &str) -> Result<Vec<Entity>> {
    let mut stmt = conn.prepare(
        "SELECT id, slug, kind, name, summary, aliases, paths, status, merged_into,
                recorded_at, author, source
         FROM context.entities WHERE project_id = ?1 ORDER BY recorded_at, id",
    )?;
    let rows = stmt.query_map([project_id], |r| {
        Ok(Entity {
            id: r.get(0)?,
            slug: r.get(1)?,
            kind: from_tag(&r.get::<_, String>(2)?)?,
            name: r.get(3)?,
            summary: r.get(4)?,
            aliases: from_json(&r.get::<_, String>(5)?)?,
            paths: from_json(&r.get::<_, String>(6)?)?,
            status: from_tag(&r.get::<_, String>(7)?)?,
            merged_into: r.get(8)?,
            recorded_at: r.get(9)?,
            author: from_json(&r.get::<_, String>(10)?)?,
            source: from_json(&r.get::<_, String>(11)?)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn load_contradictions(conn: &Connection, project_id: &str) -> Result<Vec<ContradictionEdge>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT a_id, b_id, reasoning, resolved_at, resolution, resolved_by
         FROM context.contradicts
         WHERE a_id IN ({OF_PROJECT}) OR b_id IN ({OF_PROJECT}) ORDER BY rowid"
    ))?;
    let rows = stmt.query_map([project_id], |r| {
        let resolution = match (
            r.get::<_, Option<i64>>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<String>>(5)?,
        ) {
            (Some(at), Some(reasoning), Some(by)) => Some(Resolution {
                reasoning,
                at,
                by: from_json(&by)?,
            }),
            _ => None,
        };
        Ok(ContradictionEdge {
            a: r.get(0)?,
            b: r.get(1)?,
            reasoning: r.get(2)?,
            resolution,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

const OF_PROJECT: &str = "SELECT id FROM context.assertions WHERE project_id = ?1";

fn load_assertions(
    conn: &Connection,
    project_id: &str,
    contradictions: &[ContradictionEdge],
) -> Result<Vec<Assertion>> {
    let mut about: HashMap<Id, Vec<Id>> = HashMap::new();
    for (a, e) in pairs(
        conn,
        &format!("SELECT assertion_id, entity_id FROM context.about WHERE assertion_id IN ({OF_PROJECT}) ORDER BY rowid"),
        project_id,
    )? {
        about.entry(a).or_default().push(e);
    }
    let mut supersedes: HashMap<Id, Supersede> = HashMap::new();
    // `superseded_by` is one id, but an assertion can have several successors
    // once an abandoned rewrite is superseded again. The pick is
    // deterministic: a live successor (not retracted or abandoned) first,
    // then the most recent by `recorded_at` (id breaks ties), so
    // `compile::is_current`'s forward walk follows the branch that stands.
    let mut superseded_by: HashMap<Id, (bool, i64, Id)> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT s.new_id, s.old_id, s.reasoning, a.status, a.recorded_at
         FROM context.supersedes s JOIN context.assertions a ON a.id = s.new_id
         WHERE a.project_id = ?1",
    )?;
    for row in stmt.query_map([project_id], |r| {
        Ok((
            r.get::<_, Id>(0)?,
            r.get::<_, Id>(1)?,
            r.get::<_, String>(2)?,
            from_tag::<AssertionStatus>(&r.get::<_, String>(3)?)?,
            r.get::<_, i64>(4)?,
        ))
    })? {
        let (new_id, old_id, reasoning, status, recorded_at) = row?;
        let rank = (resolve::is_live(status), recorded_at, new_id.clone());
        let best = superseded_by
            .entry(old_id.clone())
            .or_insert_with(|| rank.clone());
        if rank > *best {
            *best = rank;
        }
        supersedes.insert(
            new_id,
            Supersede {
                id: old_id,
                reasoning,
            },
        );
    }
    let mut contradicts: HashMap<Id, Vec<Id>> = HashMap::new();
    for edge in contradictions {
        contradicts
            .entry(edge.a.clone())
            .or_default()
            .push(edge.b.clone());
        contradicts
            .entry(edge.b.clone())
            .or_default()
            .push(edge.a.clone());
    }

    let mut stmt = conn.prepare(
        "SELECT id, kind, domain, stance, statement, rationale, valid_from, paths, status,
                recorded_at, author, source, provenance
         FROM context.assertions WHERE project_id = ?1 ORDER BY recorded_at, id",
    )?;
    let rows = stmt.query_map([project_id], |r| {
        let id: Id = r.get(0)?;
        Ok(Assertion {
            kind: from_tag(&r.get::<_, String>(1)?)?,
            domain: from_tag(&r.get::<_, String>(2)?)?,
            stance: from_tag(&r.get::<_, String>(3)?)?,
            statement: r.get(4)?,
            rationale: r.get(5)?,
            valid_from: r.get(6)?,
            paths: from_json(&r.get::<_, String>(7)?)?,
            status: from_tag(&r.get::<_, String>(8)?)?,
            recorded_at: r.get(9)?,
            author: from_json(&r.get::<_, String>(10)?)?,
            source: from_json(&r.get::<_, String>(11)?)?,
            provenance: from_json(&r.get::<_, String>(12)?)?,
            about: about.remove(&id).unwrap_or_default(),
            supersedes: supersedes.remove(&id),
            superseded_by: superseded_by.remove(&id).map(|(_, _, by)| by),
            contradicts: contradicts.remove(&id).unwrap_or_default(),
            id,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn load_relations(conn: &Connection, project_id: &str) -> Result<Vec<Relation>> {
    let mut stmt = conn.prepare(
        "SELECT from_id, to_id, rel FROM context.relates
         WHERE from_id IN (SELECT id FROM context.entities WHERE project_id = ?1)
         ORDER BY from_id, to_id, rel",
    )?;
    let rows = stmt.query_map([project_id], |r| {
        Ok(Relation {
            from: r.get(0)?,
            to: r.get(1)?,
            rel: from_tag(&r.get::<_, String>(2)?)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Two-id rows of an edge table, for one project.
fn pairs(conn: &Connection, sql: &str, project_id: &str) -> Result<Vec<(Id, Id)>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([project_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// In replay order: wall clock first, so a host's event lands after the one
/// from another host it builds on; `(host_id, seq)` breaks ties.
fn load_events(conn: &Connection, project_id: &str) -> Result<Vec<Event>> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, host_id, seq, recorded_at, author, source, provenance, payload
         FROM context.events WHERE project_id = ?1 ORDER BY recorded_at, host_id, seq",
    )?;
    let rows = stmt.query_map([project_id], |r| {
        let (v, payload) = payload_from_json(&r.get::<_, String>(8)?)?;
        Ok(Event {
            id: r.get(0)?,
            project_id: r.get(1)?,
            host_id: r.get(2)?,
            seq: r.get(3)?,
            recorded_at: r.get(4)?,
            stamp: Stamp {
                author: from_json(&r.get::<_, String>(5)?)?,
                source: from_json(&r.get::<_, String>(6)?)?,
                provenance: from_json(&r.get::<_, String>(7)?)?,
            },
            v,
            payload,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// ---------------------------------------------------------------------------
// Proposals

const PROPOSAL_COLUMNS: &str = "id, project_id, observation_id, payload, evidence, status, \
                                dismiss_reason, created_at, ruled_at, ruled_by";

fn proposal_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Proposal> {
    Ok(Proposal {
        id: r.get(0)?,
        project_id: r.get(1)?,
        observation_id: r.get(2)?,
        payload: from_json(&r.get::<_, String>(3)?)?,
        evidence: from_json(&r.get::<_, String>(4)?)?,
        status: from_tag(&r.get::<_, String>(5)?)?,
        dismiss_reason: r
            .get::<_, Option<String>>(6)?
            .map(|s| from_tag(&s))
            .transpose()?,
        created_at: r.get(7)?,
        ruled_at: r.get(8)?,
        ruled_by: r
            .get::<_, Option<String>>(9)?
            .map(|s| from_json(&s))
            .transpose()?,
    })
}

fn insert_proposal(conn: &Connection, proposal: &Proposal) -> Result<()> {
    conn.execute(
        "INSERT INTO context.proposals
           (id, project_id, observation_id, payload, evidence, status, dismiss_reason,
            created_at, ruled_at, ruled_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            proposal.id,
            proposal.project_id,
            proposal.observation_id,
            json(&proposal.payload)?,
            json(&proposal.evidence)?,
            tag(&proposal.status)?,
            proposal.dismiss_reason.map(|r| tag(&r)).transpose()?,
            proposal.created_at,
            proposal.ruled_at,
            proposal.ruled_by.as_ref().map(json).transpose()?,
        ],
    )?;
    Ok(())
}

fn get_proposal(conn: &Connection, proposal_id: &str) -> Result<Option<Proposal>> {
    Ok(conn
        .query_row(
            &format!("SELECT {PROPOSAL_COLUMNS} FROM context.proposals WHERE id = ?1"),
            [proposal_id],
            proposal_from_row,
        )
        .optional()?)
}

fn require_pending(conn: &Connection, proposal_id: &str) -> Result<Proposal> {
    let proposal = get_proposal(conn, proposal_id)?
        .ok_or_else(|| ContextError::Invalid(format!("unknown proposal `{proposal_id}`")))?;
    if proposal.status != ProposalStatus::Pending {
        return Err(ContextError::Invalid(format!(
            "proposal `{proposal_id}` has already been ruled"
        )));
    }
    Ok(proposal)
}

fn rule_proposal(
    conn: &Connection,
    proposal_id: &str,
    status: ProposalStatus,
    reason: Option<DismissReason>,
    ruled_by: &Author,
) -> Result<()> {
    conn.execute(
        "UPDATE context.proposals
         SET status = ?2, dismiss_reason = ?3, ruled_at = ?4, ruled_by = ?5
         WHERE id = ?1",
        params![
            proposal_id,
            tag(&status)?,
            reason.map(|r| tag(&r)).transpose()?,
            now_millis(),
            json(ruled_by)?,
        ],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Marshalling

fn json<T: Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_string(value)?)
}

/// Errors land as `FromSqlConversionFailure` so row mappers can use `?`.
fn from_json<T: DeserializeOwned>(s: &str) -> rusqlite::Result<T> {
    serde_json::from_str(s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

/// A `snake_case` enum's serde name, for the plain-text kind / status columns.
fn tag<T: Serialize>(value: &T) -> Result<String> {
    match serde_json::to_value(value)? {
        serde_json::Value::String(s) => Ok(s),
        other => Err(ContextError::Invalid(format!("not a string tag: {other}"))),
    }
}

fn from_tag<T: DeserializeOwned>(s: &str) -> rusqlite::Result<T> {
    from_json(&serde_json::Value::String(s.to_string()).to_string())
}

/// The `payload` column: the tagged payload object with `v` added, so the
/// version travels with the shape it describes.
fn payload_json(event: &Event) -> Result<String> {
    let mut value = serde_json::to_value(&event.payload)?;
    if let serde_json::Value::Object(map) = &mut value {
        map.insert("v".into(), event.v.into());
    }
    Ok(value.to_string())
}

fn payload_from_json(s: &str) -> rusqlite::Result<(u32, EventPayload)> {
    let value: serde_json::Value = from_json(s)?;
    let v = value
        .get("v")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(PAYLOAD_VERSION as u64) as u32;
    Ok((v, from_json(&value.to_string())?))
}
