use super::*;
use crate::context::fixtures::*;

fn bundle_entity(e: Entity, reason: EntryReason, relations: Vec<Relation>) -> BundleEntity {
    BundleEntity {
        entity: e,
        reason,
        relations,
    }
}

fn bundle_assertion(a: Assertion) -> BundleAssertion {
    BundleAssertion {
        assertion: a,
        flags: AssertionFlags::default(),
        history: Vec::new(),
    }
}

fn sample() -> Bundle {
    let mut constraint = assertion("c1234567-xxxx", &["f1"], "Never store passwords");
    constraint.kind = AssertionKind::Constraint;
    constraint.domain = Domain::Business;
    let mut rejected = assertion("d2", &["f1"], "Use JWT");
    rejected.stance = Stance::Rejected;
    rejected.rationale = "stateless is not needed".into();
    let mut fact = assertion("x1", &["m1"], "Postgres 16");
    fact.kind = AssertionKind::Fact;
    fact.domain = Domain::Implementation;

    let mut d1 = bundle_assertion(assertion("d1234567-aaaa", &["f1", "m1"], "Use sessions"));
    d1.flags.provisional = true;
    d1.flags.contradicted_by = vec!["x1234567-bbbb".into()];

    Bundle {
        project_id: "p1".into(),
        vision: Some(vision("v")),
        overview: false,
        entities: vec![
            bundle_entity(vision("v"), EntryReason::Vision, Vec::new()),
            bundle_entity(
                feature("f1", "auth"),
                EntryReason::Named,
                vec![link("f1", "m1", Rel::DependsOn)],
            ),
            bundle_entity(
                entity("m1", "db", EntityKind::Module),
                EntryReason::Neighbour,
                Vec::new(),
            ),
        ],
        assertions: vec![
            bundle_assertion(rejected),
            d1,
            bundle_assertion(constraint),
            bundle_assertion(fact),
        ],
        contradictions: Vec::new(),
        misses: Vec::new(),
        warnings: vec!["Something is off".into()],
        truncated: Vec::new(),
    }
}

fn position(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("missing {needle:?} in:\n{text}"))
}

#[test]
fn sections_in_order() {
    let md = render_markdown(&sample());
    let order = [
        "# Project context",
        "## Vision\nShip the thing",
        "## Entities",
        "### Features",
        "- **auth** (`auth`) — auth summary\n  - depends_on `db`",
        "### Modules",
        "## Decisions",
        "### Architectural",
        "- [adopted] Use sessions",
        "- [rejected] Use JWT",
        "## Constraints",
        "### Business",
        "## Facts",
        "### Implementation",
        "## Warnings\n- Something is off",
    ];
    let positions: Vec<usize> = order.iter().map(|n| position(&md, n)).collect();
    assert!(positions.windows(2).all(|w| w[0] < w[1]), "{md}");
    assert!(
        !md.contains("### Visions"),
        "the vision is not repeated under entities"
    );
}

#[test]
fn assertion_line_carries_slugs_flags_and_id() {
    let md = render_markdown(&sample());
    assert!(md.contains(
        "- [adopted] Use sessions (about: `auth`, `db`) _(provisional)_ _(contradicted by x1234567)_ (id: d1234567)\n"
    ));
    assert!(
        md.contains("- [rejected] Use JWT (about: `auth`) — stateless is not needed (id: d2)\n")
    );
}

#[test]
fn history_lines_nest_under_the_head() {
    let g = chain();
    let mut head = bundle_assertion(g.assertions[2].clone());
    head.history = vec![g.assertions[1].clone(), g.assertions[0].clone()];
    let b = Bundle {
        assertions: vec![head],
        ..Default::default()
    };
    let md = render_markdown(&b);
    assert!(md.contains(
        "(id: a3)\n  - superseded: second — reasoning: third is best\n  - superseded: first — reasoning: second is better\n"
    ));
}

#[test]
fn empty_sections_are_omitted() {
    let md = render_markdown(&Bundle::default());
    assert_eq!(
        md,
        "# Project context\n\n## Vision\nNo vision recorded yet.\n"
    );
}

#[test]
fn an_overview_bundle_renders_as_the_overview() {
    let b = Bundle {
        overview: true,
        ..sample()
    };
    let md = render_markdown(&b);
    assert!(md.starts_with("## Vision\nShip the thing\n"), "{md}");
    assert!(!md.contains("# Project context"), "{md}");
    assert!(md.contains("\n## Modules\n- `db` — db summary\n"), "{md}");
    assert!(
        md.contains("\n## Index\n**Features:** auth (\"auth\")\n"),
        "{md}"
    );
    assert!(
        md.contains("- [rejected] Use JWT (about: `auth`) (id: d2)\n"),
        "no rationale in the overview: {md}"
    );
    assert!(md.ends_with("## Warnings\n- Something is off\n"), "{md}");
}

#[test]
fn index_lists_active_entities_by_kind() {
    let mut archived = feature("f9", "old");
    archived.status = EntityStatus::Archived;
    let g = graph(
        vec![
            feature("f1", "auth"),
            entity("m1", "db", EntityKind::Module),
            feature("f2", "billing"),
            archived,
        ],
        Vec::new(),
        Vec::new(),
    );
    assert_eq!(
        render_index(&g, 1000),
        "## Project context index\n**Features:** auth (\"auth\"), billing (\"billing\")\n**Modules:** db (\"db\")\n"
    );
    assert_eq!(render_index(&Graph::default(), 1000), "");
}

#[test]
fn index_truncates_cleanly() {
    let entities = (0..20)
        .map(|i| feature(&format!("f{i}"), &format!("feature-{i:02}")))
        .collect();
    let g = graph(entities, Vec::new(), Vec::new());
    let full = render_index(&g, usize::MAX);
    let max = full.len() / 2;
    let cut = render_index(&g, max);
    assert!(cut.len() <= max, "{} > {max}", cut.len());
    assert!(
        cut.starts_with("## Project context index\n**Features:** feature-00 (\"feature 00\"), ")
    );
    assert!(cut.ends_with(" more"), "{cut}");
    let more: usize = cut.rsplit(' ').nth(1).unwrap().parse().unwrap();
    let listed = cut.matches("feature-").count();
    assert_eq!(listed + more, 20);
}

#[test]
fn index_quotes_names_and_leaves_out_what_the_extractor_minted() {
    let mut quoted = feature("f1", "auth");
    quoted.name = "Auth \"core\" ## not a heading".into();
    let mut minted = feature("f2", "billing");
    minted.author = Author::extractor("ws", "claude");
    let g = graph(vec![quoted, minted], Vec::new(), Vec::new());
    assert_eq!(
        render_index(&g, 1000),
        "## Project context index\n**Features:** auth (\"Auth \\\"core\\\" ## not a heading\")\n"
    );
}
