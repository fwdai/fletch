use super::*;
use crate::context::fixtures::*;

fn from(a: Assertion, kind: SourceKind, workspace: &str, at: i64) -> Assertion {
    Assertion {
        source: Source::new(kind, None),
        provenance: Provenance {
            workspace_id: Some(workspace.into()),
            ..Default::default()
        },
        recorded_at: at,
        ..a
    }
}

fn ids(found: &[TidyCandidate]) -> Vec<(&str, TidyReason, &str)> {
    found
        .iter()
        .map(|c| (c.retract.as_str(), c.reason, c.of.as_str()))
        .collect()
}

#[test]
fn a_pr_line_restating_the_workspaces_own_record_is_its_twin() {
    let own = from(
        assertion("own", &["f1"], "Sessions refresh server-side."),
        SourceKind::AgentTurn,
        "ws1",
        1,
    );
    let line = from(
        assertion("line", &["f1"], "Refresh happens on the server."),
        SourceKind::Pr,
        "ws1",
        2,
    );
    // Not twins: another workspace, another kind, another entity, or the
    // other way round in time.
    let elsewhere = from(
        assertion("else", &["f1"], "Elsewhere."),
        SourceKind::Pr,
        "ws2",
        3,
    );
    let mut other_kind = from(
        assertion("kind", &["f1"], "A rule."),
        SourceKind::Pr,
        "ws1",
        3,
    );
    other_kind.kind = AssertionKind::Constraint;
    let other_entity = from(
        assertion("ent", &["f2"], "About f2."),
        SourceKind::Pr,
        "ws1",
        3,
    );
    let earlier = from(
        assertion("early", &["f1"], "First."),
        SourceKind::Pr,
        "ws1",
        0,
    );
    let g = graph(
        vec![feature("f1", "auth"), feature("f2", "billing")],
        vec![own, line, elsewhere, other_kind, other_entity, earlier],
        Vec::new(),
    );
    assert_eq!(ids(&candidates(&g)), [("line", TidyReason::TwinOf, "own")]);
}

#[test]
fn a_head_restating_a_replaced_statement_is_listed_against_it() {
    let mut old = assertion("old", &["f1"], "Every spawn carries the socket.");
    old.superseded_by = Some("new".into());
    let mut new = assertion("new", &["f1"], "The socket only outside agent trees.");
    new.supersedes = Some(Supersede {
        id: "old".into(),
        reasoning: "scoped".into(),
    });
    let back = from(
        assertion("back", &["f1"], "every spawn carries the socket"),
        SourceKind::Pr,
        "ws1",
        5,
    );
    let g = graph(
        vec![feature("f1", "auth")],
        vec![old, new, back],
        Vec::new(),
    );
    assert_eq!(
        ids(&candidates(&g)),
        [("back", TidyReason::RestatesRetired, "old")]
    );
}

#[test]
fn a_rewording_in_its_own_chain_and_a_withdrawn_statement_are_not_restatements() {
    // `new` replaces `old` with the same words and a new rationale: its own
    // history, not a resurrection.
    let mut old = assertion("old", &["f1"], "Prices in cents.");
    old.superseded_by = Some("new".into());
    let mut new = assertion("new", &["f1"], "Prices in cents.");
    new.supersedes = Some(Supersede {
        id: "old".into(),
        reasoning: "better why".into(),
    });
    // `gone` was retracted (a tidied twin, say); `kept` saying the same is
    // the record that stays.
    let mut gone = assertion("gone", &["f1"], "Use tabs.");
    gone.status = AssertionStatus::Retracted;
    let kept = assertion("kept", &["f1"], "Use tabs.");
    let g = graph(
        vec![feature("f1", "auth")],
        vec![old, new, gone, kept],
        Vec::new(),
    );
    assert!(candidates(&g).is_empty(), "{:?}", candidates(&g));
}

#[test]
fn the_reasoning_names_the_rule_and_the_other_assertion() {
    let c = TidyCandidate {
        retract: "a".into(),
        statement: String::new(),
        reason: TidyReason::TwinOf,
        of: "b".into(),
        of_statement: String::new(),
    };
    assert_eq!(c.reasoning(), "tidy: twin of b");
    let c = TidyCandidate {
        reason: TidyReason::RestatesRetired,
        ..c
    };
    assert_eq!(c.reasoning(), "tidy: restates retired b");
}
