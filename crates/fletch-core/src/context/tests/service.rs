//! The service's own rules over a temp store: the change pulse and the gate
//! re-read per operation.

use std::sync::Arc;

use super::*;
use crate::context::{ContextError, ContextStore, EntityKind, EntityStatus};
use crate::host::sink::RecordingSink;

fn service() -> (ContextService, Arc<RecordingSink>, tempfile::TempDir) {
    let (store, dir) = ContextStore::temp().unwrap();
    let sink = Arc::new(RecordingSink::new());
    (
        ContextService::new(store.db().clone(), sink.clone()).unwrap(),
        sink,
        dir,
    )
}

fn project() -> Project {
    Project {
        id: "ctx-svc".into(),
        fletch_id: "fp-svc".into(),
    }
}

fn ui() -> Stamp {
    Stamp {
        author: Author::user(),
        source: Source::ui(),
        provenance: Provenance::default(),
    }
}

fn entity(slug: &str) -> EntityInput {
    EntityInput {
        id: None,
        slug: slug.into(),
        kind: EntityKind::Module,
        name: slug.to_uppercase(),
        summary: String::new(),
        aliases: vec![],
        paths: vec![],
    }
}

fn decision(about: &str, statement: &str) -> Candidate {
    Candidate {
        input: AssertionInput {
            kind: AssertionKind::Decision,
            domain: Domain::Architectural,
            stance: Stance::Adopted,
            statement: statement.into(),
            rationale: String::new(),
            valid_from: None,
            paths: vec![],
            about: vec![about.into()],
            supersedes: None,
            contradicts: vec![],
            status: AssertionStatus::Confirmed,
        },
        user: None,
        relation: None,
        evidence: vec![],
        about_pending: vec![],
        observation_id: None,
    }
}

/// One `context:changed` per write that changed something, carrying the
/// Fletch project id — and none for a write that recorded nothing.
#[test]
fn every_landed_write_pulses_context_changed_once() {
    let (service, sink, _dir) = service();
    let p = project();
    let e = service.record_entity(&p, entity("e"), ui()).unwrap();
    let pulses = || {
        sink.events()
            .into_iter()
            .filter(|(name, _)| name == super::super::CHANGED_EVENT)
            .collect::<Vec<_>>()
    };
    assert_eq!(pulses().len(), 1);
    assert_eq!(pulses()[0].1, serde_json::json!({ "project_id": "fp-svc" }));

    assert!(matches!(
        service.record_decision(&p, decision(&e, "we do it"), ui()),
        Ok(Landing::Recorded { .. })
    ));
    assert_eq!(pulses().len(), 2);
    // A restatement writes nothing and says nothing.
    assert!(matches!(
        service.record_decision(&p, decision(&e, "we do it"), ui()),
        Ok(Landing::Duplicate { .. })
    ));
    assert_eq!(pulses().len(), 2);
    // Settling a checkout that made no records is silent too.
    assert_eq!(
        service
            .settle(&p, "ws-none", "repo", true, true, ui())
            .unwrap(),
        0
    );
    assert_eq!(pulses().len(), 2);
    service.archive_entity(&p, &e, ui()).unwrap();
    assert_eq!(pulses().len(), 3);
}

/// A project resolved while the layer was on is a name, not a permission:
/// once the gate closes, the read is refused by the service and the write
/// by the store, under the same `Disabled`.
#[test]
fn a_project_from_before_the_gate_closed_is_refused() {
    let (service, _sink, _dir) = service();
    let p = project();
    let e = service.record_entity(&p, entity("e"), ui()).unwrap();
    assert!(service.check(&p).is_ok());
    assert_eq!(service.graph(&p).unwrap().entities.len(), 1);

    service.with_conn(|conn| {
        crate::database::set_setting(conn, crate::context::DEV_SETTING, "false").unwrap()
    });
    assert!(matches!(service.check(&p), Err(ContextError::Disabled)));
    assert!(matches!(service.graph(&p), Err(ContextError::Disabled)));
    assert!(matches!(
        service.archive_entity(&p, &e, ui()),
        Err(ContextError::Disabled)
    ));
    assert_eq!(
        service
            .store()
            .load(&p.id)
            .unwrap()
            .entity(&e)
            .unwrap()
            .status,
        EntityStatus::Active
    );
}

/// A proposal is added to, and ruled on in, the project it was made for.
#[test]
fn proposals_are_scoped_to_their_project() {
    let (service, _sink, _dir) = service();
    let p = project();
    let other = Project {
        id: "ctx-other".into(),
        fletch_id: "fp-other".into(),
    };
    let proposal = Proposal {
        id: new_id(),
        project_id: p.id.clone(),
        observation_id: None,
        payload: ProposalPayload::Entity {
            input: entity("f"),
            stamp: ui(),
        },
        evidence: vec![],
        status: ProposalStatus::Pending,
        dismiss_reason: None,
        created_at: 0,
        ruled_at: None,
        ruled_by: None,
    };
    assert!(service.add_proposal(&other, &proposal).is_err());
    service.add_proposal(&p, &proposal).unwrap();
    let err = service
        .accept_proposal(&other, &proposal.id, Author::user())
        .unwrap_err();
    assert!(
        matches!(&err, ContextError::Invalid(m) if m.contains("unknown proposal")),
        "{err}"
    );
    service
        .accept_proposal(&p, &proposal.id, Author::user())
        .unwrap();
}
