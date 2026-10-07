//! The extractor over a temp store with a fake model. Every test here is
//! canned JSON in, proposals out: the extractor never lands anything.

mod archive;
mod input;
mod parse;
mod pipeline;
mod schedule;

use crate::context::model::*;
use crate::context::{ContextService, ContextStore, Project};
use crate::error::Result;

use super::{ExtractInput, ExtractOutput, Extractor, TurnText};

pub const PROJECT: &str = "p-test";

/// Answers with the same text every time.
pub struct Canned(pub String);

impl Extractor for Canned {
    fn extract(&self, _: &ExtractInput) -> Result<ExtractOutput> {
        Ok(ExtractOutput {
            text: self.0.clone(),
            tokens_in: Some(10),
            tokens_out: Some(5),
        })
    }

    fn model(&self) -> String {
        "fake/canned".into()
    }
}

/// A CLI that could not run.
pub struct Failing;

impl Extractor for Failing {
    fn extract(&self, _: &ExtractInput) -> Result<ExtractOutput> {
        Err(crate::error::Error::Other("not logged in".into()))
    }

    fn model(&self) -> String {
        "fake/failing".into()
    }
}

pub fn store() -> (ContextStore, tempfile::TempDir) {
    ContextStore::temp().unwrap()
}

/// The service the pipeline writes through, over a temp store.
pub fn service() -> (ContextService, tempfile::TempDir) {
    let (store, dir) = store();
    (
        ContextService::new(
            store.db().clone(),
            std::sync::Arc::new(crate::host::sink::NullSink),
        )
        .unwrap(),
        dir,
    )
}

pub fn project() -> Project {
    Project {
        id: PROJECT.into(),
        fletch_id: "fp-test".into(),
    }
}

pub fn user_stamp() -> Stamp {
    Stamp {
        author: Author::user(),
        // A user-stated head, as the UI records one: only the service can
        // mint a `user_turn` source, and only from a verified quote.
        source: Source::ui(),
        provenance: Provenance::default(),
    }
}

pub fn agent_stamp() -> Stamp {
    Stamp {
        author: Author::agent("ws-1", "claude"),
        source: Source::new(SourceKind::AgentTurn, Some("t0".into())),
        provenance: Provenance::default(),
    }
}

/// A feature entity `slug`, recorded by the user.
pub fn seed_entity(service: &ContextService, slug: &str) -> Id {
    service
        .record_entity(
            &project(),
            EntityInput {
                id: None,
                slug: slug.into(),
                kind: EntityKind::Feature,
                name: slug.replace('-', " "),
                summary: format!("{slug} summary"),
                aliases: Vec::new(),
                paths: Vec::new(),
            },
            user_stamp(),
        )
        .unwrap()
}

/// A confirmed, adopted architectural decision about `entity_id`, with `stamp`.
pub fn seed_decision(
    service: &ContextService,
    entity_id: &str,
    statement: &str,
    stamp: Stamp,
) -> Id {
    seed_decision_as(
        service,
        entity_id,
        statement,
        stamp,
        AssertionStatus::Confirmed,
    )
}

/// Seeded through the service before the run: what is there already, not a
/// write of the extractor's. Named `New` so an agent stamp lands next to any
/// head instead of being handed the heads back.
pub fn seed_decision_as(
    service: &ContextService,
    entity_id: &str,
    statement: &str,
    stamp: Stamp,
    status: AssertionStatus,
) -> Id {
    let landing = service
        .record_decision(
            &project(),
            Candidate {
                input: AssertionInput {
                    kind: AssertionKind::Decision,
                    domain: Domain::Architectural,
                    stance: Stance::Adopted,
                    statement: statement.into(),
                    rationale: String::new(),
                    valid_from: None,
                    paths: Vec::new(),
                    about: vec![entity_id.into()],
                    supersedes: None,
                    contradicts: Vec::new(),
                    status,
                },
                user: None,
                relation: Some(ProposedRelation {
                    kind: RelationKind::New,
                    target: None,
                    reasoning: None,
                }),
                evidence: Vec::new(),
                about_pending: Vec::new(),
                observation_id: None,
            },
            stamp,
        )
        .unwrap();
    match landing {
        Landing::Recorded { id, .. } => id,
        other => panic!("seed did not land: {other:?}"),
    }
}

/// What the user says in the one turn [`run`] extracts over.
pub const USER_TEXT: &str = "Let's use JWT for sessions, and never store tokens in local storage.";
/// The agent's reply in that turn.
pub const AGENT_TEXT: &str = "Done. Tokens expire hourly, so the refresh job runs each hour.";
/// A quote found in the user's text, and one found only in the agent's.
pub const USER_QUOTE: &str = "use JWT for sessions";
pub const AGENT_QUOTE: &str = "Tokens expire hourly";

/// One turn's worth of input over the store's current graph.
pub fn input(service: &ContextService, user: &str) -> ExtractInput {
    let graph = service.store().load(PROJECT).unwrap();
    ExtractInput::new(
        Some("Add auth".into()),
        vec![TurnText {
            turn_id: "t1".into(),
            user: user.into(),
            assistant: Some(AGENT_TEXT.into()),
        }],
        &graph,
    )
}

/// The checkout the run is stamped with.
pub const REPO: &str = "app";

pub fn provenance() -> Provenance {
    Provenance {
        repo: Some(REPO.into()),
        workspace_id: Some("ws-1".into()),
        branch: Some("feat/auth".into()),
        commit_sha: None,
        session_id: Some("sess-1".into()),
        turn_id: Some("t1".into()),
    }
}

/// Run the pipeline over `service` with `extractor`.
pub fn run(service: &ContextService, extractor: &dyn Extractor) -> super::Summary {
    super::pipeline::process(
        service,
        &project(),
        Author::extractor("ws-1", "claude"),
        provenance(),
        input(service, USER_TEXT),
        extractor,
    )
    .unwrap()
}

/// `(output, error)` of the one run recorded for `observation_id`.
pub fn run_row(service: &ContextService, observation_id: &str) -> (Option<String>, Option<String>) {
    service.with_conn(|conn| {
        conn.query_row(
            "SELECT output, error FROM context.extractor_runs WHERE observation_id = ?1",
            [observation_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    })
}

/// `extracted_at` of an observation.
pub fn extracted_at(service: &ContextService, observation_id: &str) -> Option<i64> {
    service.with_conn(|conn| {
        conn.query_row(
            "SELECT extracted_at FROM context.observations WHERE id = ?1",
            [observation_id],
            |r| r.get(0),
        )
        .unwrap()
    })
}
