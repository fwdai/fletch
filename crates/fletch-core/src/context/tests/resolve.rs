use super::entity;
use super::*;
use crate::context::fixtures::*;

fn people() -> Graph {
    let mut auth = feature("f1", "auth");
    auth.name = "Authentication".into();
    auth.aliases = vec!["login".into(), "SSO".into()];
    let mut billing = feature("f2", "billing");
    billing.name = "Billing".into();
    billing.aliases = vec!["payments".into()];
    let mut old = feature("f3", "signin");
    old.status = EntityStatus::Merged;
    old.merged_into = Some("f1".into());
    let mut gone = feature("f4", "legacy");
    gone.status = EntityStatus::Archived;
    gone.aliases = vec!["login".into()];
    graph(vec![auth, billing, old, gone], Vec::new(), Vec::new())
}

#[test]
fn resolves_each_tier() {
    let g = people();
    assert_eq!(entity(&g, "f2").unwrap().slug, "billing");
    assert_eq!(entity(&g, "AUTH").unwrap().id, "f1");
    assert_eq!(entity(&g, "sso").unwrap().id, "f1");
    assert_eq!(entity(&g, "authentication").unwrap().id, "f1");
}

#[test]
fn id_wins_over_slug_and_archived_never_matches() {
    let g = people();
    assert_eq!(entity(&g, "login").unwrap().id, "f1");
    assert!(matches!(
        entity(&g, "legacy"),
        Err(ContextError::UnknownEntity(_))
    ));
    assert!(matches!(
        entity(&g, "f4"),
        Err(ContextError::UnknownEntity(_))
    ));
}

#[test]
fn merged_entity_redirects_once() {
    let g = people();
    assert_eq!(entity(&g, "signin").unwrap().id, "f1");
    assert_eq!(entity(&g, "f3").unwrap().id, "f1");
}

#[test]
fn ambiguous_alias_lists_slugs() {
    let mut g = people();
    g.entities[1].aliases.push("Login".into());
    match entity(&g, "login") {
        Err(ContextError::AmbiguousEntity(r, slugs)) => {
            assert_eq!(r, "login");
            assert_eq!(slugs, vec!["auth", "billing"]);
        }
        other => panic!("expected ambiguity, got {other:?}"),
    }
}

#[test]
fn unknown_reference() {
    assert!(
        matches!(entity(&people(), "nope"), Err(ContextError::UnknownEntity(r)) if r == "nope")
    );
}

#[test]
fn entities_splits_found_and_unknown() {
    let mut g = people();
    g.entities[1].aliases.push("login".into());
    let refs = [
        "auth".to_string(),
        "nope".into(),
        "login".into(),
        "payments".into(),
    ];
    let (found, unknown) = entities(&g, &refs);
    assert_eq!(
        found.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        ["f1", "f2"]
    );
    assert_eq!(unknown, ["nope", "login"]);
}

// ---------------------------------------------------------------------------
// classify

fn heads() -> Graph {
    let mut older = assertion("d1", &["f1"], "Use Postgres");
    older.recorded_at = 10;
    let mut newer = assertion("d2", &["f1"], "Use Postgres with pgvector");
    newer.recorded_at = 20;
    graph(
        vec![feature("f1", "auth"), feature("f2", "billing")],
        vec![older, newer],
        Vec::new(),
    )
}

#[test]
fn classify_duplicate_normalises_statement() {
    let r = classify(&heads(), &input(&["f1"], "  use   POSTGRES. "));
    assert_eq!(r.kind, RelationKind::Duplicate);
    assert_eq!(r.target.as_deref(), Some("d1"));
}

#[test]
fn classify_never_judges_a_different_statement() {
    // Replacing or contradicting a head is a judgment left to the caller;
    // text alone only knows a restatement.
    let r = classify(&heads(), &input(&["f1"], "Use SQLite"));
    assert_eq!(r.kind, RelationKind::New);
    assert!(r.target.is_none());

    let mut rejected = input(&["f1"], "Use Postgres");
    rejected.stance = Stance::Rejected;
    assert_eq!(classify(&heads(), &rejected).kind, RelationKind::New);

    let mut fact = input(&["f1"], "Postgres is not used");
    fact.kind = AssertionKind::Fact;
    assert_eq!(classify(&heads(), &fact).kind, RelationKind::New);
}

#[test]
fn related_heads_share_an_entity_and_the_domain() {
    let g = heads();
    let ids = |c: &AssertionInput| {
        let mut v: Vec<&str> = related_heads(&g, c).iter().map(|a| a.id.as_str()).collect();
        v.sort();
        v
    };
    assert_eq!(ids(&input(&["f1"], "Use SQLite")), vec!["d1", "d2"]);
    assert!(ids(&input(&["f2"], "Stripe for billing")).is_empty());
    let mut other_domain = input(&["f1"], "Use SQLite");
    other_domain.domain = Domain::Business;
    assert!(ids(&other_domain).is_empty());
}

#[test]
fn classify_new_when_nothing_shares_entity_or_domain() {
    assert_eq!(
        classify(&heads(), &input(&["f2"], "Stripe for billing")).kind,
        RelationKind::New
    );
    let mut other_domain = input(&["f1"], "Use SQLite");
    other_domain.domain = Domain::Business;
    assert_eq!(classify(&heads(), &other_domain).kind, RelationKind::New);
}

#[test]
fn classify_sees_only_what_stands_now() {
    // d1 replaced by a live d2: a restatement of d1 is new, not a duplicate.
    let mut g = heads();
    g.assertions[0].superseded_by = Some("d2".into());
    assert_eq!(
        classify(&g, &input(&["f1"], "Use Postgres")).kind,
        RelationKind::New
    );
    // Retract d2 and d1 stands again (compile::is_current), so the same
    // restatement is now a duplicate of d1.
    g.assertions[1].status = AssertionStatus::Retracted;
    let r = classify(&g, &input(&["f1"], "Use Postgres"));
    assert_eq!(r.kind, RelationKind::Duplicate);
    assert_eq!(r.target.as_deref(), Some("d1"));
}

// ---------------------------------------------------------------------------
// auto_rule

fn relation(kind: RelationKind) -> ProposedRelation {
    ProposedRelation {
        kind,
        target: Some("d1".into()),
        reasoning: None,
    }
}

fn stamp(source: Source) -> Stamp {
    Stamp {
        author: Author::agent("a", "p"),
        source,
        provenance: Provenance::default(),
    }
}

#[test]
fn auto_rule_simple_rows() {
    let g = heads();
    let agent = stamp(Source::new(SourceKind::AgentTurn, None));
    for kind in [
        RelationKind::New,
        RelationKind::Confirms,
        RelationKind::Duplicate,
    ] {
        assert!(auto_rule(&g, &relation(kind), &agent));
    }
    assert!(!auto_rule(
        &g,
        &relation(RelationKind::Contradicts),
        &stamp(Source::ui())
    ));
}

#[test]
fn auto_rule_supersedes_depends_on_who_said_what() {
    let mut g = heads();
    let agent = stamp(Source::new(SourceKind::AgentTurn, None));
    let sup = relation(RelationKind::Supersedes);

    assert!(auto_rule(&g, &sup, &stamp(Source::ui())));
    assert!(
        auto_rule(&g, &sup, &stamp(Source::new(SourceKind::UserTurn, None))),
        "what the user said through an agent counts as user-stated"
    );
    assert!(
        !auto_rule(&g, &sup, &agent),
        "an agent may not override a user turn"
    );

    g.assertions[0].source = Source::new(SourceKind::Pr, Some("12".into()));
    assert!(
        auto_rule(&g, &sup, &agent),
        "a user author via a PR is not user-stated"
    );

    g.assertions[0].source = Source::new(SourceKind::UserTurn, None);
    g.assertions[0].author = Author::agent("b", "p");
    assert!(
        !auto_rule(&g, &sup, &agent),
        "an agent relaying a user turn is user-stated"
    );
}
