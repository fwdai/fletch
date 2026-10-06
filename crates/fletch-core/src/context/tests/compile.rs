use super::*;
use crate::context::fixtures::*;

fn ids<'a>(v: impl IntoIterator<Item = &'a Assertion>) -> Vec<&'a str> {
    v.into_iter().map(|a| a.id.as_str()).collect()
}

fn reasons(bundle: &Bundle) -> Vec<(&str, EntryReason)> {
    bundle
        .entities
        .iter()
        .map(|b| (b.entity.id.as_str(), b.reason))
        .collect()
}

#[test]
fn heads_as_of_picks_the_link_in_force() {
    let g = chain();
    assert_eq!(ids(heads_about(&g, "f1", None)), ["a3"]);
    assert_eq!(ids(heads_about(&g, "f1", Some(250))), ["a2"]);
    assert_eq!(ids(heads_about(&g, "f1", Some(100))), ["a1"]);
    assert!(heads_about(&g, "f1", Some(50)).is_empty());
}

#[test]
fn heads_skip_retracted_abandoned_and_other_entities() {
    let mut g = chain();
    for a in &mut g.assertions {
        a.status = AssertionStatus::Retracted;
    }
    assert!(heads_about(&g, "f1", None).is_empty());
    g.assertions[2].status = AssertionStatus::Abandoned;
    assert!(heads_about(&g, "f1", None).is_empty());
    g.assertions[2].status = AssertionStatus::Provisional;
    assert_eq!(ids(heads_about(&g, "f1", None)), ["a3"]);
    assert!(heads_about(&g, "f2", None).is_empty());
}

#[test]
fn a_dead_superseder_replaces_nothing() {
    // An abandoned branch's rewrite (or a retracted one) must not erase the
    // head it rewrote.
    let mut g = chain();
    g.assertions[2].status = AssertionStatus::Abandoned;
    assert_eq!(ids(heads_about(&g, "f1", None)), ["a2"]);
    g.assertions[2].status = AssertionStatus::Retracted;
    assert_eq!(ids(heads_about(&g, "f1", None)), ["a2"]);
}

#[test]
fn history_is_newest_predecessor_first() {
    let g = chain();
    assert_eq!(ids(history(&g, "a3")), ["a2", "a1"]);
    assert_eq!(ids(history(&g, "a2")), ["a1"]);
    assert!(history(&g, "a1").is_empty());
}

fn town() -> Graph {
    let mut archived = feature("f9", "old");
    archived.status = EntityStatus::Archived;
    graph(
        vec![
            vision("v"),
            feature("f1", "auth"),
            feature("f2", "billing"),
            entity("m1", "db", EntityKind::Module),
            archived,
        ],
        vec![
            assertion("d1", &["f1"], "Use sessions"),
            assertion("d2", &["m1"], "Use Postgres"),
            assertion("d3", &["f2"], "Stripe"),
        ],
        vec![
            link("f1", "m1", Rel::DependsOn),
            link("f2", "v", Rel::Serves),
        ],
    )
}

#[test]
fn empty_query_is_the_map() {
    let b = compile(&town(), &CompileQuery::default(), None);
    assert_eq!(b.vision.as_ref().map(|v| v.id.as_str()), Some("v"));
    let mut got = reasons(&b);
    got.sort_by_key(|(id, _)| *id);
    assert_eq!(
        got,
        [
            ("f1", EntryReason::Named),
            ("f2", EntryReason::Named),
            ("m1", EntryReason::Named),
            ("v", EntryReason::Vision),
        ]
    );
    let mut served: Vec<&str> = b
        .assertions
        .iter()
        .map(|a| a.assertion.id.as_str())
        .collect();
    served.sort();
    assert_eq!(served, ["d1", "d2", "d3"]);
    assert!(b.misses.is_empty() && b.warnings.is_empty());
}

#[test]
fn named_entry_pulls_one_hop_and_records_misses() {
    let q = CompileQuery {
        entities: vec!["auth".into(), "nothing".into()],
        ..Default::default()
    };
    let b = compile(&town(), &q, None);
    assert_eq!(
        reasons(&b),
        [
            ("v", EntryReason::Vision),
            ("f1", EntryReason::Named),
            ("m1", EntryReason::Neighbour)
        ]
    );
    assert_eq!(b.entities[1].relations.len(), 1);
    assert_eq!(ids(b.assertions.iter().map(|a| &a.assertion)), ["d1", "d2"]);
    assert_eq!(b.misses, ["nothing"]);
    assert_eq!(b.warnings, ["No context recorded for: nothing"]);
}

#[test]
fn vision_fallback_only_without_vision() {
    let mut g = town();
    g.entities.remove(0);
    let b = compile(&g, &CompileQuery::default(), Some("brief".into()));
    assert!(b.vision.is_none());
    assert_eq!(b.vision_fallback.as_deref(), Some("brief"));
    let b = compile(&town(), &CompileQuery::default(), Some("brief".into()));
    assert!(b.vision_fallback.is_none());
}

#[test]
fn text_query_scores_entities_and_statements() {
    let q = CompileQuery {
        query: Some("postgres".into()),
        ..Default::default()
    };
    let b = compile(&town(), &q, None);
    assert_eq!(
        reasons(&b),
        [
            ("v", EntryReason::Vision),
            ("m1", EntryReason::Query),
            ("f1", EntryReason::Neighbour)
        ]
    );
}

#[test]
fn paths_anchor_entities_and_assertions() {
    let mut g = town();
    g.entities[2].paths = vec!["src/billing/".into()];
    g.assertions[1].paths = vec!["./src/db/schema.rs".into()];
    let q = CompileQuery {
        paths: vec!["src/billing/stripe.rs".into(), "src/db".into()],
        ..Default::default()
    };
    let b = compile(&g, &q, None);
    let mut got = reasons(&b);
    got.sort_by_key(|(id, _)| *id);
    assert_eq!(
        got,
        [
            ("f1", EntryReason::Neighbour),
            ("f2", EntryReason::Path),
            ("m1", EntryReason::Path),
            ("v", EntryReason::Vision)
        ]
    );
}

#[test]
fn flags_and_warnings_for_provisional_and_contradicted() {
    let mut g = town();
    g.assertions[0].status = AssertionStatus::Provisional;
    g.assertions[1].contradicts = vec!["d1".into()];
    let q = CompileQuery {
        entities: vec!["auth".into()],
        ..Default::default()
    };
    let b = compile(&g, &q, None);
    let d1 = &b.assertions[0];
    let d2 = &b.assertions[1];
    assert!(d1.flags.provisional && !d2.flags.provisional);
    assert_eq!(d1.flags.contradicted_by, ["d2"]);
    assert_eq!(d2.flags.contradicted_by, ["d1"]);
    assert_eq!(b.contradictions.len(), 1);
    assert_eq!(
        b.warnings,
        [
            "Unresolved: Use sessions vs Use Postgres",
            "1 decision(s) are provisional: recorded from a branch that has not merged",
        ]
    );
}

#[test]
fn history_attached_only_on_request() {
    let g = chain();
    let q = CompileQuery {
        entities: vec!["auth".into()],
        ..Default::default()
    };
    let b = compile(&g, &q, None);
    assert!(b.assertions[0].flags.has_history && b.assertions[0].history.is_empty());
    let b = compile(
        &g,
        &CompileQuery {
            include_history: true,
            ..q
        },
        None,
    );
    assert_eq!(ids(&b.assertions[0].history), ["a2", "a1"]);
}

#[test]
fn budget_drops_lowest_tier_first_and_never_the_vision() {
    let g = town();
    let q = CompileQuery {
        entities: vec!["auth".into()],
        ..Default::default()
    };
    let full = render_markdown(&compile(&g, &q, None)).len();
    let q = CompileQuery {
        budget_chars: full - 1,
        ..q
    };
    let b = compile(&g, &q, None);
    assert_eq!(b.truncated, ["m1"]);
    assert_eq!(
        reasons(&b),
        [("v", EntryReason::Vision), ("f1", EntryReason::Named)]
    );
    assert!(b.warnings.iter().any(|w| w.starts_with("Truncated")));

    let b = compile(
        &g,
        &CompileQuery {
            budget_chars: 1,
            ..q
        },
        None,
    );
    assert_eq!(b.truncated, ["m1", "f1"]);
    assert_eq!(reasons(&b), [("v", EntryReason::Vision)]);
    assert!(b.vision.is_some());
}
