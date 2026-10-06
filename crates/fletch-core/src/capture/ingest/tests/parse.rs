use super::*;

fn only(body: &str) -> ParsedDecision {
    let mut parsed = parse_decisions(body);
    assert_eq!(parsed.len(), 1, "{parsed:?}");
    parsed.remove(0)
}

#[test]
fn the_canonical_line_parses_every_field() {
    let d = only(
        "## Decisions\n- adopted · implementation · [auth-session, login] — Sessions refresh server-side. — Client refresh raced the token cache.\n",
    );
    assert_eq!(d.kind, AssertionKind::Decision);
    assert_eq!(d.stance, Stance::Adopted);
    assert_eq!(d.domain, Domain::Implementation);
    assert_eq!(d.about, ["auth-session", "login"]);
    assert_eq!(d.statement, "Sessions refresh server-side.");
    assert_eq!(d.rationale, "Client refresh raced the token cache.");
    assert!(
        d.line.starts_with("adopted ·"),
        "the bullet is stripped from the evidence line"
    );
}

#[test]
fn each_first_field_maps_to_its_kind_and_stance() {
    let body = "## Decisions\n\
        - adopted · implementation · [a] — one\n\
        - rejected · architectural · [a] — two — see #41\n\
        - constraint · business · [a] — three\n\
        - fact · implementation · [a] — four\n";
    let parsed = parse_decisions(body);
    let shape: Vec<_> = parsed
        .iter()
        .map(|d| (d.kind, d.stance, d.domain))
        .collect();
    assert_eq!(
        shape,
        [
            (
                AssertionKind::Decision,
                Stance::Adopted,
                Domain::Implementation
            ),
            (
                AssertionKind::Decision,
                Stance::Rejected,
                Domain::Architectural
            ),
            (AssertionKind::Constraint, Stance::Adopted, Domain::Business),
            (AssertionKind::Fact, Stance::Adopted, Domain::Implementation),
        ]
    );
    assert_eq!(parsed[1].rationale, "see #41");
}

#[test]
fn star_bullets_and_plain_dashes_are_accepted() {
    let d = only("## Decisions\n* Adopted · Business · [billing] - Invoices are voided, never deleted. - Accounting.\n");
    assert_eq!(d.stance, Stance::Adopted);
    assert_eq!(d.domain, Domain::Business);
    assert_eq!(d.about, ["billing"]);
    assert_eq!(d.statement, "Invoices are voided, never deleted.");
    assert_eq!(d.rationale, "Accounting.");
}

#[test]
fn a_missing_rationale_is_empty() {
    let d = only("## Decisions\n- adopted · implementation · [a] — Just the statement\n");
    assert_eq!(d.statement, "Just the statement");
    assert_eq!(d.rationale, "");
}

#[test]
fn a_statement_may_contain_a_dash_when_the_bracket_list_anchors_the_head() {
    let d = only("## Decisions\n- adopted · implementation · [auth-session] — Use server-side refresh — it is simpler.\n");
    assert_eq!(d.about, ["auth-session"]);
    assert_eq!(d.statement, "Use server-side refresh");
    assert_eq!(d.rationale, "it is simpler.");
}

#[test]
fn a_missing_bracket_list_yields_an_empty_about() {
    let d = only(
        "## Decisions\n- adopted · implementation — Sessions refresh server-side. — because\n",
    );
    assert!(d.about.is_empty());
    assert_eq!(d.statement, "Sessions refresh server-side.");
    assert_eq!(d.rationale, "because");
}

#[test]
fn none_and_blank_lines_are_nothing() {
    assert!(parse_decisions("## Decisions\n- none\n").is_empty());
    assert!(parse_decisions("## Decisions\n\n- None.\n\n").is_empty());
}

#[test]
fn the_section_ends_at_the_next_heading() {
    let body = "## Summary\n- adopted · implementation · [a] — not a decision, wrong section\n\
        ## Decisions\n- fact · business · [a] — in\n\
        ## Test plan\n- adopted · implementation · [a] — out\n";
    let parsed = parse_decisions(body);
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].statement, "in");
}

#[test]
fn a_body_without_the_section_is_empty() {
    assert!(parse_decisions("## Summary\nJust a PR.\n").is_empty());
    assert!(parse_decisions("").is_empty());
}

#[test]
fn unreadable_lines_are_dropped_and_the_rest_kept() {
    let body = "## Decisions\n\
        - maybe · implementation · [a] — unknown stance\n\
        - adopted · somewhere · [a] — unknown domain\n\
        - adopted · implementation · [a] —\n\
        - adopted · implementation · [a] — kept\n";
    let parsed = parse_decisions(body);
    assert_eq!(parsed.len(), 1, "{parsed:?}");
    assert_eq!(parsed[0].statement, "kept");
}

#[test]
fn the_heading_is_matched_loosely() {
    assert_eq!(
        parse_decisions("## decisions:\n- fact · business · [a] — x\n").len(),
        1
    );
    assert_eq!(
        parse_decisions("  ## Decisions  \n- fact · business · [a] — x\n").len(),
        1
    );
}
