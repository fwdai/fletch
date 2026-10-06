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
use super::{ContextError, Result, HOST_ID_KEY, PROJECT_ID_KEY};
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
    pub fn record_entity(&self, project_id: &str, input: EntityInput, stamp: Stamp) -> Result<Id> {
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
        if slug_taken(conn, project_id, &input.slug, &id)? {
            return Err(ContextError::SlugTaken(input.slug));
        }
        if input.kind == EntityKind::Vision && vision_exists(conn, project_id, &id)? {
            return Err(ContextError::VisionExists);
        }
        let payload = EventPayload::EntityRecorded {
            id: id.clone(),
            slug: input.slug,
            kind: input.kind,
            name: input.name,
            summary: input.summary,
            aliases: input.aliases,
            paths: input.paths,
        };
        self.append(conn, project_id, stamp, now_millis(), payload)?;
        Ok(id)
    }

    /// Appends `assertion_recorded` with its `about`, `supersedes` and
    /// `contradicts` edges. Errors: `NoSubject`, `MissingReasoning`, `NotHead`
    /// (the superseded assertion is not a head), `UnknownEntity`,
    /// `UnknownAssertion`.
    pub fn record_assertion(
        &self,
        project_id: &str,
        input: AssertionInput,
        stamp: Stamp,
    ) -> Result<Id> {
        self.write(|conn| self.record_assertion_in(conn, project_id, input, stamp))
    }

    fn record_assertion_in(
        &self,
        conn: &Connection,
        project_id: &str,
        input: AssertionInput,
        stamp: Stamp,
    ) -> Result<Id> {
        if input.about.is_empty() {
            return Err(ContextError::NoSubject);
        }
        for entity_id in &input.about {
            require_entity(conn, project_id, entity_id)?;
        }
        if let Some(sup) = &input.supersedes {
            if sup.reasoning.trim().is_empty() {
                return Err(ContextError::MissingReasoning);
            }
            require_assertion(conn, project_id, &sup.id)?;
            if !is_head(conn, &sup.id)? {
                return Err(ContextError::NotHead(sup.id.clone()));
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
            statement: input.statement,
            rationale: input.rationale,
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
    pub fn link(&self, project_id: &str, change: LinkChange, stamp: Stamp) -> Result<()> {
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

    pub fn retract(
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

    pub fn confirm(&self, project_id: &str, assertion_id: &str, stamp: Stamp) -> Result<()> {
        self.assertion_event(
            project_id,
            assertion_id,
            stamp,
            EventPayload::Confirmed {
                assertion_id: assertion_id.to_string(),
            },
        )
    }

    pub fn abandon(&self, project_id: &str, assertion_id: &str, stamp: Stamp) -> Result<()> {
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

    pub fn archive_entity(&self, project_id: &str, entity_id: &str, stamp: Stamp) -> Result<()> {
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
    /// and leaves `entity_id` in `merged` status.
    pub fn merge_entities(
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
            require_entity(conn, project_id, entity_id)?;
            require_entity(conn, project_id, into)?;
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
        let conn = self.db.lock();
        Ok(Graph {
            project_id: project_id.to_string(),
            entities: load_entities(&conn, project_id)?,
            assertions: load_assertions(&conn, project_id)?,
            relations: load_relations(&conn, project_id)?,
        })
    }

    /// Every event of one project in `(host_id, seq)` order.
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

    /// Provisional assertions recorded from `workspace_id` (by provenance).
    pub fn provisional_for_workspace(
        &self,
        project_id: &str,
        workspace_id: &str,
    ) -> Result<Vec<Id>> {
        let conn = self.db.lock();
        let mut stmt = conn.prepare(
            "SELECT id FROM context.assertions
             WHERE project_id = ?1 AND status = 'provisional'
               AND json_extract(provenance, '$.workspace_id') = ?2
             ORDER BY recorded_at, id",
        )?;
        let ids = stmt.query_map([project_id, workspace_id], |r| r.get(0))?;
        Ok(ids.collect::<rusqlite::Result<_>>()?)
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

    pub fn add_proposal(&self, proposal: &Proposal) -> Result<()> {
        let conn = self.db.lock();
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

    /// Lands a proposal: applies its payload as events (`record_entity` /
    /// `record_assertion`, honouring `relation`) and marks it `status`
    /// (`Accepted` by the user, `Auto` by rule). Returns the recorded id.
    pub fn accept_proposal(
        &self,
        proposal_id: &str,
        status: ProposalStatus,
        ruled_by: Author,
    ) -> Result<Id> {
        self.write(|conn| {
            let proposal = require_pending(conn, proposal_id)?;
            let project_id = &proposal.project_id;
            let id = match proposal.payload {
                ProposalPayload::Entity { input, stamp } => {
                    self.record_entity_in(conn, project_id, input, stamp)?
                }
                ProposalPayload::Assertion {
                    mut input,
                    stamp,
                    relation,
                } => {
                    let target = |relation: &ProposedRelation| {
                        relation.target.clone().ok_or_else(|| {
                            ContextError::Invalid(format!(
                                "a `{}` proposal needs a target assertion",
                                tag(&relation.kind).unwrap_or_default()
                            ))
                        })
                    };
                    // A human ruling is a confirmation: what the user accepts
                    // is not waiting on any branch.
                    if status == ProposalStatus::Accepted {
                        input.status = AssertionStatus::Confirmed;
                    }
                    match relation.kind {
                        RelationKind::New => {}
                        RelationKind::Supersedes => {
                            // The proposer's reasoning when it gave one; the
                            // candidate's own rationale otherwise, so a parked
                            // supersession can still be accepted.
                            let reasoning = relation
                                .reasoning
                                .clone()
                                .filter(|r| !r.trim().is_empty())
                                .unwrap_or_else(|| input.rationale.clone());
                            input.supersedes = Some(Supersede {
                                id: target(&relation)?,
                                reasoning,
                            });
                        }
                        RelationKind::Contradicts => input.contradicts.push(Contradict {
                            id: target(&relation)?,
                            reasoning: relation.reasoning.clone(),
                        }),
                        RelationKind::Confirms => {
                            // Nothing new is said; a confirmed restatement
                            // settles a provisional target.
                            let id = target(&relation)?;
                            if input.status == AssertionStatus::Confirmed {
                                let event = EventPayload::Confirmed {
                                    assertion_id: id.clone(),
                                };
                                self.append(conn, project_id, stamp, now_millis(), event)?;
                            }
                            rule_proposal(conn, proposal_id, status, None, &ruled_by)?;
                            return Ok(id);
                        }
                        RelationKind::Duplicate => {
                            let id = target(&relation)?;
                            rule_proposal(conn, proposal_id, status, None, &ruled_by)?;
                            return Ok(id);
                        }
                    }
                    self.record_assertion_in(conn, project_id, input, stamp)?
                }
            };
            rule_proposal(conn, proposal_id, status, None, &ruled_by)?;
            Ok(id)
        })
    }

    pub fn dismiss_proposal(
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
            heads: count(
                "SELECT COUNT(*) FROM context.assertions a WHERE a.project_id = ?1
                   AND NOT EXISTS (SELECT 1 FROM context.supersedes s WHERE s.old_id = a.id)",
            )?,
            provisional: count(
                "SELECT COUNT(*) FROM context.assertions
                 WHERE project_id = ?1 AND status = 'provisional'",
            )?,
            contradictions: count(
                "SELECT COUNT(*) FROM context.contradicts c
                 WHERE c.a_id IN (SELECT id FROM context.assertions WHERE project_id = ?1)",
            )?,
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
/// alike.
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
            conn.execute(
                "UPDATE context.entities SET status = 'archived' WHERE id = ?1",
                [id],
            )?;
        }
        EventPayload::EntityMerged { id, into } => {
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
            conn.execute(
                "UPDATE context.entities SET status = 'merged', merged_into = ?2 WHERE id = ?1",
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
                conn.execute(
                    "INSERT OR IGNORE INTO context.about (assertion_id, entity_id) VALUES (?1, ?2)",
                    [id, entity_id],
                )?;
            }
            if let Some(sup) = supersedes {
                conn.execute(
                    "INSERT OR IGNORE INTO context.supersedes (new_id, old_id, reasoning)
                     VALUES (?1, ?2, ?3)",
                    [id, &sup.id, &sup.reasoning],
                )?;
            }
            for c in contradicts {
                conn.execute(
                    "INSERT OR IGNORE INTO context.contradicts (a_id, b_id, reasoning)
                     VALUES (?1, ?2, ?3)",
                    params![id, c.id, c.reasoning],
                )?;
            }
        }
        EventPayload::Linked { from, to, rel } => {
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
    }
    Ok(())
}

/// The one mutation an assertion row ever sees.
fn set_status(conn: &Connection, assertion_id: &str, status: AssertionStatus) -> Result<()> {
    conn.execute(
        "UPDATE context.assertions SET status = ?2 WHERE id = ?1",
        params![assertion_id, tag(&status)?],
    )?;
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

fn is_head(conn: &Connection, assertion_id: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM context.supersedes WHERE old_id = ?1",
        [assertion_id],
        |r| r.get(0),
    )?;
    Ok(n == 0)
}

/// Whether another entity of the project (not `entity_id` itself) holds `slug`.
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

fn load_assertions(conn: &Connection, project_id: &str) -> Result<Vec<Assertion>> {
    const OF_PROJECT: &str = "SELECT id FROM context.assertions WHERE project_id = ?1";
    let mut about: HashMap<Id, Vec<Id>> = HashMap::new();
    for (a, e) in pairs(
        conn,
        &format!("SELECT assertion_id, entity_id FROM context.about WHERE assertion_id IN ({OF_PROJECT}) ORDER BY rowid"),
        project_id,
    )? {
        about.entry(a).or_default().push(e);
    }
    let mut supersedes: HashMap<Id, Supersede> = HashMap::new();
    let mut superseded_by: HashMap<Id, Id> = HashMap::new();
    let mut stmt = conn.prepare(&format!(
        "SELECT new_id, old_id, reasoning FROM context.supersedes WHERE new_id IN ({OF_PROJECT})"
    ))?;
    for row in stmt.query_map([project_id], |r| {
        Ok((
            r.get::<_, Id>(0)?,
            r.get::<_, Id>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (new_id, old_id, reasoning) = row?;
        superseded_by.insert(old_id.clone(), new_id.clone());
        supersedes.insert(
            new_id,
            Supersede {
                id: old_id,
                reasoning,
            },
        );
    }
    let mut contradicts: HashMap<Id, Vec<Id>> = HashMap::new();
    for (a, b) in pairs(
        conn,
        &format!("SELECT a_id, b_id FROM context.contradicts WHERE a_id IN ({OF_PROJECT}) OR b_id IN ({OF_PROJECT}) ORDER BY rowid"),
        project_id,
    )? {
        contradicts.entry(a.clone()).or_default().push(b.clone());
        contradicts.entry(b).or_default().push(a);
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
            superseded_by: superseded_by.remove(&id),
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

fn load_events(conn: &Connection, project_id: &str) -> Result<Vec<Event>> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, host_id, seq, recorded_at, author, source, provenance, payload
         FROM context.events WHERE project_id = ?1 ORDER BY host_id, seq",
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
