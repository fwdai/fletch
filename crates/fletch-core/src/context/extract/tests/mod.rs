//! The extractor over a temp store with a fake model. Every test here is
//! canned JSON in, proposals and events out.

mod archive;
mod input;
mod parse;
mod pipeline;
mod schedule;

use crate::context::model::*;
use crate::context::ContextStore;
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

pub fn user_stamp() -> Stamp {
    Stamp {
        author: Author::user(),
        source: Source::new(SourceKind::UserTurn, Some("t0".into())),
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
pub fn seed_entity(store: &ContextStore, slug: &str) -> Id {
    seed_entity_in(store, PROJECT, slug)
}

pub fn seed_entity_in(store: &ContextStore, project_id: &str, slug: &str) -> Id {
    store
        .record_entity(
            project_id,
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
pub fn seed_decision(store: &ContextStore, entity_id: &str, statement: &str, stamp: Stamp) -> Id {
    seed_decision_in(
        store,
        PROJECT,
        entity_id,
        statement,
        stamp,
        AssertionStatus::Confirmed,
    )
}

pub fn seed_decision_in(
    store: &ContextStore,
    project_id: &str,
    entity_id: &str,
    statement: &str,
    stamp: Stamp,
    status: AssertionStatus,
) -> Id {
    store
        .record_assertion(
            project_id,
            AssertionInput {
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
            stamp,
        )
        .unwrap()
}

/// What the user says in the one turn [`run`] extracts over.
pub const USER_TEXT: &str = "Let's use JWT for sessions, and never store tokens in local storage.";
/// The agent's reply in that turn.
pub const AGENT_TEXT: &str = "Done. Tokens expire hourly, so the refresh job runs each hour.";
/// A quote found in the user's text, and one found only in the agent's.
pub const USER_QUOTE: &str = "use JWT for sessions";
pub const AGENT_QUOTE: &str = "Tokens expire hourly";

/// One turn's worth of input over the store's current graph.
pub fn input(store: &ContextStore, user: &str) -> ExtractInput {
    let graph = store.load(PROJECT).unwrap();
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

pub fn provenance() -> Provenance {
    Provenance {
        workspace_id: Some("ws-1".into()),
        branch: Some("feat/auth".into()),
        commit_sha: None,
        session_id: Some("sess-1".into()),
        turn_id: Some("t1".into()),
    }
}

/// Run the pipeline over `store` with `extractor`, as a turn-end run does.
pub fn run(store: &ContextStore, extractor: &dyn Extractor) -> super::Summary {
    run_as(store, extractor, AssertionStatus::Provisional)
}

/// [`run`] with what agent-stated assertions get spelled out: an archive run
/// passes the branch's settled outcome.
pub fn run_as(
    store: &ContextStore,
    extractor: &dyn Extractor,
    agent_status: AssertionStatus,
) -> super::Summary {
    super::pipeline::process(
        store,
        PROJECT,
        Author::extractor("ws-1", "claude"),
        provenance(),
        agent_status,
        input(store, USER_TEXT),
        extractor,
    )
    .unwrap()
}

/// `(output, error)` of the one run recorded for `observation_id`.
pub fn run_row(store: &ContextStore, observation_id: &str) -> (Option<String>, Option<String>) {
    store
        .db()
        .lock()
        .query_row(
            "SELECT output, error FROM context.extractor_runs WHERE observation_id = ?1",
            [observation_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
}

/// `extracted_at` of an observation.
pub fn extracted_at(store: &ContextStore, observation_id: &str) -> Option<i64> {
    store
        .db()
        .lock()
        .query_row(
            "SELECT extracted_at FROM context.observations WHERE id = ?1",
            [observation_id],
            |r| r.get(0),
        )
        .unwrap()
}
