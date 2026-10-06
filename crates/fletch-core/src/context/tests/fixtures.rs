//! Hand-built graphs for the pure context logic. Everything defaults to the
//! most ordinary case (active, confirmed, adopted, user-stated); tests set
//! the one field they care about.

#![allow(dead_code)]

use crate::context::model::*;

pub fn entity(id: &str, slug: &str, kind: EntityKind) -> Entity {
    Entity {
        id: id.into(),
        slug: slug.into(),
        kind,
        name: slug.replace('-', " "),
        summary: format!("{slug} summary"),
        aliases: Vec::new(),
        paths: Vec::new(),
        status: EntityStatus::Active,
        merged_into: None,
        recorded_at: 0,
        author: Author::user(),
        source: Source::ui(),
    }
}

pub fn feature(id: &str, slug: &str) -> Entity {
    entity(id, slug, EntityKind::Feature)
}

pub fn vision(id: &str) -> Entity {
    let mut v = entity(id, "vision", EntityKind::Vision);
    v.summary = "Ship the thing".into();
    v
}

pub fn assertion(id: &str, about: &[&str], statement: &str) -> Assertion {
    Assertion {
        id: id.into(),
        kind: AssertionKind::Decision,
        domain: Domain::Architectural,
        stance: Stance::Adopted,
        statement: statement.into(),
        rationale: String::new(),
        valid_from: 0,
        paths: Vec::new(),
        status: AssertionStatus::Confirmed,
        recorded_at: 0,
        author: Author::user(),
        source: Source::new(SourceKind::UserTurn, None),
        provenance: Provenance::default(),
        about: about.iter().map(|s| s.to_string()).collect(),
        supersedes: None,
        superseded_by: None,
        contradicts: Vec::new(),
    }
}

pub fn input(about: &[&str], statement: &str) -> AssertionInput {
    AssertionInput {
        kind: AssertionKind::Decision,
        domain: Domain::Architectural,
        stance: Stance::Adopted,
        statement: statement.into(),
        rationale: String::new(),
        valid_from: None,
        paths: Vec::new(),
        about: about.iter().map(|s| s.to_string()).collect(),
        supersedes: None,
        contradicts: Vec::new(),
        status: AssertionStatus::Confirmed,
    }
}

pub fn link(from: &str, to: &str, rel: Rel) -> Relation {
    Relation {
        from: from.into(),
        to: to.into(),
        rel,
    }
}

pub fn graph(entities: Vec<Entity>, assertions: Vec<Assertion>, relations: Vec<Relation>) -> Graph {
    Graph {
        project_id: "p1".into(),
        entities,
        assertions,
        relations,
        current: Vec::new(),
    }
}

/// `a1 → a2 → a3` about `f1`, valid from 100 / 200 / 300.
pub fn chain() -> Graph {
    let mut a1 = assertion("a1", &["f1"], "first");
    let mut a2 = assertion("a2", &["f1"], "second");
    let mut a3 = assertion("a3", &["f1"], "third");
    a1.valid_from = 100;
    a2.valid_from = 200;
    a3.valid_from = 300;
    a1.superseded_by = Some("a2".into());
    a2.superseded_by = Some("a3".into());
    a2.supersedes = Some(Supersede {
        id: "a1".into(),
        reasoning: "second is better".into(),
    });
    a3.supersedes = Some(Supersede {
        id: "a2".into(),
        reasoning: "third is best".into(),
    });
    graph(vec![feature("f1", "auth")], vec![a1, a2, a3], Vec::new())
}
