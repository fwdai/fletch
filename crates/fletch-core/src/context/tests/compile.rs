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
fn heads_are_the_live_end_of_each_chain() {
    let g = chain();
    assert_eq!(ids(heads_about(&g, "f1")), ["a3"]);
    assert_eq!(current_heads(&g).len(), 1);
}

#[test]
fn a_live_successor_further_down_still_replaces() {
    // Retracting the middle link does not resurrect the first: the chain
    // ends in a live assertion, and that is what stands.
    let mut g = chain();
    g.assertions[1].status = AssertionStatus::Retracted;
    assert_eq!(ids(heads_about(&g, "f1")), ["a3"]);
    assert!(!is_current(&g, &g.assertions[0]));
}

#[test]
fn heads_skip_retracted_abandoned_and_other_entities() {
    let mut g = chain();
    for a in &mut g.assertions {
        a.status = AssertionStatus::Retracted;
    }
    assert!(heads_about(&g, "f1").is_empty());
    g.assertions[2].status = AssertionStatus::Abandoned;
    assert!(heads_about(&g, "f1").is_empty());
    g.assertions[2].status = AssertionStatus::Provisional;
    assert_eq!(ids(heads_about(&g, "f1")), ["a3"]);
    assert!(heads_about(&g, "f2").is_empty());
}

#[test]
fn a_dead_superseder_replaces_nothing() {
    // An abandoned branch's rewrite (or a retracted one) must not erase the
    // head it rewrote.
    let mut g = chain();
    g.assertions[2].status = AssertionStatus::Abandoned;
    assert_eq!(ids(heads_about(&g, "f1")), ["a2"]);
    g.assertions[2].status = AssertionStatus::Retracted;
    assert_eq!(ids(heads_about(&g, "f1")), ["a2"]);
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

fn constraint(id: &str, about: &[&str], statement: &str, domain: Domain) -> Assertion {
    let mut a = assertion(id, about, statement);
    a.kind = AssertionKind::Constraint;
    a.domain = domain;
    a
}

/// A vision, constraints of every shape that should and should not reach the
/// overview, `modules` modules with long summaries and a path each, and a
/// feature the extractor minted.
fn project(modules: usize) -> Graph {
    let mut entities = vec![vision("v"), feature("f1", "billing")];
    for i in 0..modules {
        let mut m = entity(
            &format!("m{i}"),
            &format!("module-{i:02}"),
            EntityKind::Module,
        );
        m.summary = "x".repeat(300);
        m.paths = vec![format!("src/module_{i}"), "elsewhere".into()];
        entities.push(m);
    }
    let mut minted = feature("f9", "minted");
    minted.author = Author::extractor("ws", "claude");
    entities.push(minted);

    let mut rejected = constraint("c4", &["f1"], "Rejected constraint", Domain::Business);
    rejected.stance = Stance::Rejected;
    let mut replaced = constraint("c5", &["f1"], "Replaced constraint", Domain::Business);
    replaced.superseded_by = Some("c2".into());
    let mut c2 = constraint("c2", &["f1"], "Prices are in cents", Domain::Business);
    c2.supersedes = Some(Supersede {
        id: "c5".into(),
        reasoning: "floats".into(),
    });
    c2.rationale = "r".repeat(900);
    graph(
        entities,
        vec![
            constraint(
                "c1",
                &["m0"],
                "Payments never touch the main DB",
                Domain::Architectural,
            ),
            c2,
            constraint("c3", &["f1"], "Use tabs", Domain::Implementation),
            rejected,
            replaced,
            assertion("d1", &["f1"], "A decision, not a constraint"),
        ],
        Vec::new(),
    )
}

#[test]
fn the_overview_is_vision_constraints_modules_then_the_index() {
    let b = overview(&project(2), 0);
    assert!(b.overview);
    assert_eq!(b.vision.as_ref().map(|v| v.id.as_str()), Some("v"));
    let mut served = ids(b.assertions.iter().map(|a| &a.assertion));
    served.sort();
    assert_eq!(served, ["c1", "c2"], "adopted current constraints only");
    assert!(b.entities.iter().all(|e| e.entity.id != "f9"));

    let md = render_markdown(&b);
    let order = [
        "## Vision\nShip the thing\n",
        "## Constraints",
        "### Business\n- [adopted] Prices are in cents (about: `billing`) (id: c2)\n",
        "### Architectural\n- [adopted] Payments never touch the main DB (about: `module-00`) (id: c1)\n",
        "## Modules\n- `module-",
        "## Index\n**Features:** billing (\"billing\")\n",
    ];
    let positions: Vec<usize> = order
        .iter()
        .map(|n| {
            md.find(n)
                .unwrap_or_else(|| panic!("missing {n:?} in:\n{md}"))
        })
        .collect();
    assert!(positions.windows(2).all(|w| w[0] < w[1]), "{md}");
    assert!(
        md.contains(&format!(
            "- `module-00` — {}… (`src/module_0`)\n",
            "x".repeat(140)
        )),
        "{md}"
    );
    assert!(!md.contains("minted") && !md.contains("Warnings"), "{md}");
    assert!(!md.contains(&"r".repeat(10)), "constraints stay thin: {md}");
}

#[test]
fn the_overview_drops_entities_before_constraints_and_never_the_vision() {
    let g = project(40);
    let b = overview(&g, 0);
    let md = render_markdown(&b);
    assert!(
        md.len() <= OVERVIEW_BUDGET_CHARS,
        "{} chars:\n{md}",
        md.len()
    );
    assert!(!b.truncated.is_empty());
    assert!(md.contains("## Vision\nShip the thing"), "{md}");
    assert!(md.contains("Payments never touch the main DB"), "{md}");
    assert!(md.contains("Prices are in cents"), "{md}");
    assert!(md.contains("Truncated to fit the budget"), "{md}");
    assert!(!md.contains("constraints not shown"), "{md}");

    // A budget nothing fits keeps only the vision.
    let b = overview(&g, 1);
    assert!(b.vision.is_some());
    assert!(b.assertions.is_empty());
    assert_eq!(&b.truncated[b.truncated.len() - 2..], ["c2", "c1"]);
    assert_eq!(
        reasons(&b),
        [("v", EntryReason::Vision)],
        "every other entity dropped"
    );
}

#[test]
fn the_overview_sheds_low_ranked_constraints_to_meet_its_budget() {
    let mut assertions = Vec::new();
    for i in 0..300 {
        let mut c = constraint(
            &format!("c{i:03}"),
            &["f1"],
            &format!("Constraint number {i} holds"),
            Domain::Architectural,
        );
        c.recorded_at = i;
        assertions.push(c);
    }
    let g = graph(
        vec![vision("v"), feature("f1", "billing")],
        assertions,
        Vec::new(),
    );
    let b = overview(&g, 0);
    let md = render_markdown(&b);
    assert!(
        md.len() <= OVERVIEW_BUDGET_CHARS,
        "{} chars:\n{md}",
        md.len()
    );
    assert!(md.contains("## Vision\nShip the thing"), "{md}");
    let shown = b.assertions.len();
    assert!(shown > 0 && shown < 300, "{shown} shown");
    assert_eq!(b.assertions[0].assertion.id, "c299", "newest first");
    assert_eq!(b.truncated.first().map(String::as_str), Some("f1"));
    let dropped: Vec<String> = (0..300 - shown).map(|i| format!("c{i:03}")).collect();
    assert_eq!(b.truncated[1..], dropped, "oldest dropped, lowest first");
    let line = format!(
        "Truncated to fit the budget: {} constraints not shown; call context_get for the full set",
        300 - shown
    );
    assert!(b.warnings.contains(&line), "{:?}", b.warnings);
    assert!(md.contains(&line), "{md}");
}

#[test]
fn overview_constraints_rank_user_then_confirmed_then_newest() {
    let mut extracted = constraint("ext", &["f1"], "Extracted", Domain::Business);
    extracted.author = Author::extractor("ws", "claude");
    extracted.recorded_at = 50;
    let mut user_provisional = constraint("user-prov", &["f1"], "Stated", Domain::Business);
    user_provisional.status = AssertionStatus::Provisional;
    let mut old = constraint("old", &["f1"], "Old", Domain::Business);
    old.recorded_at = 1;
    let mut new = constraint("new", &["f1"], "New", Domain::Business);
    new.recorded_at = 2;
    let g = graph(
        vec![vision("v"), feature("f1", "billing")],
        vec![extracted, user_provisional, old, new],
        Vec::new(),
    );
    let b = overview(&g, 0);
    assert_eq!(
        ids(b.assertions.iter().map(|a| &a.assertion)),
        ["new", "old", "user-prov", "ext"]
    );
}

#[test]
fn the_overview_mode_of_compile_is_the_overview() {
    let g = project(3);
    let q = CompileQuery {
        overview: true,
        entities: vec!["billing".into()],
        ..Default::default()
    };
    assert_eq!(compile(&g, &q, None), overview(&g, 0));
}

#[test]
fn an_empty_project_has_an_empty_overview() {
    assert_eq!(render_markdown(&overview(&Graph::default(), 0)), "");
}
