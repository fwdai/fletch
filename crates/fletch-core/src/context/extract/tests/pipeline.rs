use crate::context::extract::prompt::PROMPT_VERSION;
use crate::context::model::*;

use super::{
    agent_stamp, extracted_at, run, run_as, run_row, seed_decision, seed_entity, store, user_stamp,
    Canned, Failing, PROJECT,
};

fn entity_json(slug: &str) -> String {
    format!(r#"{{"slug": "{slug}", "kind": "feature", "name": "{slug}", "summary": "s"}}"#)
}

fn assertion_json(
    about: &str,
    kind: &str,
    statement: &str,
    by_user: bool,
    relation: &str,
) -> String {
    format!(
        r#"{{"about": ["{about}"], "kind": "{kind}", "domain": "architectural", "stance": "adopted",
            "statement": "{statement}", "rationale": "because", "stated_by_user": {by_user},
            "relation": {relation}, "evidence": [{{"quote": "{statement}"}}]}}"#
    )
}

fn answer(entities: &[String], assertions: &[String]) -> String {
    format!(
        r#"{{"entities": [{}], "assertions": [{}]}}"#,
        entities.join(","),
        assertions.join(",")
    )
}

fn proposals(store: &crate::context::ContextStore, status: ProposalStatus) -> Vec<Proposal> {
    store.proposals(PROJECT, Some(status)).unwrap()
}

#[test]
fn a_fenced_answer_lands_and_the_run_is_recorded() {
    let (store, _dir) = store();
    let text = format!("```json\n{}\n```", answer(&[entity_json("auth")], &[]));
    let summary = run(&store, &Canned(text.clone()));

    assert_eq!(summary.entities_landed, 1);
    assert!(summary.error.is_none());
    let graph = store.load(PROJECT).unwrap();
    let auth = graph.entities.iter().find(|e| e.slug == "auth").unwrap();
    assert_eq!(auth.author.kind, AuthorKind::Extractor);
    assert_eq!(auth.author.provider.as_deref(), Some("claude"));

    let (output, error) = run_row(&store, &summary.observation_id);
    assert_eq!(output.as_deref(), Some(text.as_str()));
    assert!(error.is_none());
    assert!(extracted_at(&store, &summary.observation_id).is_some());
    // The audit trail: an auto proposal for the entity.
    let auto = proposals(&store, ProposalStatus::Auto);
    assert_eq!(auto.len(), 1);
    assert!(matches!(auto[0].payload, ProposalPayload::Entity { .. }));
    let runs: i64 = store
        .db()
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM context.extractor_runs WHERE prompt_version = ?1",
            [PROMPT_VERSION],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(runs, 1);
}

#[test]
fn malformed_output_is_kept_on_the_run_and_nothing_lands() {
    let (store, _dir) = store();
    let summary = run(&store, &Canned("Sorry, nothing to report.".into()));

    assert!(summary.error.as_deref().unwrap().contains("unparseable"));
    let (output, error) = run_row(&store, &summary.observation_id);
    assert_eq!(output.as_deref(), Some("Sorry, nothing to report."));
    assert!(error.unwrap().contains("unparseable"));
    assert!(store.load(PROJECT).unwrap().entities.is_empty());
    assert!(store.proposals(PROJECT, None).unwrap().is_empty());
    assert!(extracted_at(&store, &summary.observation_id).is_none());
}

#[test]
fn a_failed_run_is_kept_with_its_error_and_not_marked_extracted() {
    let (store, _dir) = store();
    let summary = run(&store, &Failing);
    assert_eq!(summary.error.as_deref(), Some("not logged in"));
    let (output, error) = run_row(&store, &summary.observation_id);
    assert!(output.is_none());
    assert_eq!(error.as_deref(), Some("not logged in"));
    assert!(extracted_at(&store, &summary.observation_id).is_none());
}

#[test]
fn an_archive_run_lands_agent_stated_assertions_with_the_settled_outcome() {
    for (outcome, expected) in [
        (AssertionStatus::Confirmed, AssertionStatus::Confirmed),
        (AssertionStatus::Abandoned, AssertionStatus::Abandoned),
    ] {
        let (store, _dir) = store();
        seed_entity(&store, "auth");
        let summary = run_as(
            &store,
            &Canned(answer(
                &[],
                &[
                    assertion_json(
                        "auth",
                        "fact",
                        "Tokens expire hourly",
                        false,
                        r#"{"kind": "new"}"#,
                    ),
                    assertion_json("auth", "decision", "Use JWT", true, r#"{"kind": "new"}"#),
                ],
            )),
            outcome,
        );
        assert_eq!(summary.auto, 2);
        let graph = store.load(PROJECT).unwrap();
        let by_statement = |s: &str| graph.assertions.iter().find(|a| a.statement == s).unwrap();
        assert_eq!(by_statement("Tokens expire hourly").status, expected);
        // What the user said is confirmed whatever the branch's fate.
        assert_eq!(by_statement("Use JWT").status, AssertionStatus::Confirmed);
    }
}

#[test]
fn an_existing_entity_is_skipped_and_a_new_one_lands() {
    let (store, _dir) = store();
    seed_entity(&store, "auth");
    let summary = run(
        &store,
        &Canned(answer(&[entity_json("auth"), entity_json("billing")], &[])),
    );
    assert_eq!(summary.entities_skipped, 1);
    assert_eq!(summary.entities_landed, 1);
    let graph = store.load(PROJECT).unwrap();
    let slugs: Vec<&str> = graph.entities.iter().map(|e| e.slug.as_str()).collect();
    assert_eq!(slugs.len(), 2);
    assert!(slugs.contains(&"billing"));
}

#[test]
fn a_user_stated_assertion_lands_confirmed_by_rule() {
    let (store, _dir) = store();
    let auth = seed_entity(&store, "auth");
    let summary = run(
        &store,
        &Canned(answer(
            &[],
            &[assertion_json(
                "auth",
                "decision",
                "Use JWT",
                true,
                r#"{"kind": "new"}"#,
            )],
        )),
    );
    assert_eq!(summary.auto, 1);
    let graph = store.load(PROJECT).unwrap();
    let a = &graph.assertions[0];
    assert_eq!(a.status, AssertionStatus::Confirmed);
    assert_eq!(a.source.kind, SourceKind::UserTurn);
    assert_eq!(a.source.reference.as_deref(), Some("t1"));
    assert_eq!(a.author.kind, AuthorKind::Extractor);
    assert_eq!(a.about, vec![auth]);
    assert_eq!(a.provenance.workspace_id.as_deref(), Some("ws-1"));
    let auto = proposals(&store, ProposalStatus::Auto);
    assert_eq!(auto.len(), 1);
    assert_eq!(auto[0].evidence[0].quote, "Use JWT");
    assert_eq!(auto[0].evidence[0].turn_id.as_deref(), Some("t1"));
}

#[test]
fn an_agent_stated_new_assertion_lands_provisional_by_rule() {
    let (store, _dir) = store();
    seed_entity(&store, "auth");
    let summary = run(
        &store,
        &Canned(answer(
            &[],
            &[assertion_json(
                "auth",
                "fact",
                "Tokens expire hourly",
                false,
                r#"{"kind": "new"}"#,
            )],
        )),
    );
    assert_eq!(summary.auto, 1);
    let graph = store.load(PROJECT).unwrap();
    assert_eq!(graph.assertions[0].status, AssertionStatus::Provisional);
    assert_eq!(graph.assertions[0].source.kind, SourceKind::AgentTurn);
}

/// Two different decisions about one entity are not a supersession on their
/// own: without the model saying so, the second simply joins the first.
#[test]
fn a_different_statement_is_new_unless_the_model_relates_it() {
    let (store, _dir) = store();
    let auth = seed_entity(&store, "auth");
    let head = seed_decision(&store, &auth, "Use sessions", user_stamp());
    let summary = run(
        &store,
        &Canned(answer(
            &[],
            &[assertion_json(
                "auth",
                "decision",
                "Use JWT",
                false,
                r#"{"kind": "new"}"#,
            )],
        )),
    );
    assert_eq!(summary.auto, 1);
    let graph = store.load(PROJECT).unwrap();
    assert_eq!(graph.assertions.len(), 2);
    assert!(graph.assertion(&head).unwrap().is_head());
    assert!(graph.assertions.iter().all(|a| a.supersedes.is_none()));
}

#[test]
fn an_agent_stated_supersession_of_a_user_stated_head_waits_for_review() {
    let (store, _dir) = store();
    let auth = seed_entity(&store, "auth");
    let head = seed_decision(&store, &auth, "Use sessions", user_stamp());
    // No reasoning from the model: the rationale stands in.
    let relation = format!(r#"{{"kind": "supersedes", "target": "{head}"}}"#);
    let summary = run(
        &store,
        &Canned(answer(
            &[],
            &[assertion_json(
                "auth", "decision", "Use JWT", false, &relation,
            )],
        )),
    );
    assert_eq!(summary.pending, 1);
    assert_eq!(summary.auto, 0);
    let pending = proposals(&store, ProposalStatus::Pending);
    assert_eq!(pending.len(), 1);
    match &pending[0].payload {
        ProposalPayload::Assertion { relation, .. } => {
            assert_eq!(relation.kind, RelationKind::Supersedes);
            assert_eq!(relation.target.as_deref(), Some(head.as_str()));
            assert_eq!(relation.reasoning.as_deref(), Some("because"));
        }
        other => panic!("expected an assertion proposal, got {other:?}"),
    }
    // The head is untouched.
    let graph = store.load(PROJECT).unwrap();
    assert_eq!(graph.assertions.len(), 1);
    assert!(graph.assertions[0].is_head());
}

#[test]
fn a_user_stated_supersession_of_a_user_stated_head_lands() {
    let (store, _dir) = store();
    let auth = seed_entity(&store, "auth");
    let head = seed_decision(&store, &auth, "Use sessions", user_stamp());
    let relation =
        format!(r#"{{"kind": "supersedes", "target": "{head}", "reasoning": "changed course"}}"#);
    let summary = run(
        &store,
        &Canned(answer(
            &[],
            &[assertion_json(
                "auth", "decision", "Use JWT", true, &relation,
            )],
        )),
    );
    assert_eq!(summary.auto, 1);
    let graph = store.load(PROJECT).unwrap();
    assert!(!graph.assertion(&head).unwrap().is_head());
    let new = graph.assertions.iter().find(|a| a.is_head()).unwrap();
    assert_eq!(new.supersedes.as_ref().unwrap().id, head);
    assert_eq!(new.source.kind, SourceKind::UserTurn);
}

/// The model's `supersedes` is honoured only against a live head: a superseded
/// one, an abandoned one or an unknown id leave the candidate `New`.
#[test]
fn a_relation_whose_target_is_not_a_live_head_falls_back_to_new() {
    let (store, _dir) = store();
    let auth = seed_entity(&store, "auth");
    let old = seed_decision(&store, &auth, "Use cookies", agent_stamp());
    let abandoned = super::seed_decision_in(
        &store,
        PROJECT,
        &auth,
        "Use PASETO",
        agent_stamp(),
        AssertionStatus::Abandoned,
    );
    // `old` is superseded by hand, so it is no longer a head.
    store
        .record_assertion(
            PROJECT,
            AssertionInput {
                kind: AssertionKind::Decision,
                domain: Domain::Architectural,
                stance: Stance::Adopted,
                statement: "Use sessions, not cookies".into(),
                rationale: String::new(),
                valid_from: None,
                paths: Vec::new(),
                about: vec![auth.clone()],
                supersedes: Some(Supersede {
                    id: old.clone(),
                    reasoning: "cookies are out".into(),
                }),
                contradicts: Vec::new(),
                status: AssertionStatus::Confirmed,
            },
            user_stamp(),
        )
        .unwrap();
    for target in [old.as_str(), abandoned.as_str(), "nonesuch"] {
        let before = store.load(PROJECT).unwrap().assertions.len();
        let relation =
            format!(r#"{{"kind": "supersedes", "target": "{target}", "reasoning": "r"}}"#);
        let summary = run(
            &store,
            &Canned(answer(
                &[],
                &[assertion_json(
                    "auth",
                    "fact",
                    &format!("Fact about {target}"),
                    false,
                    &relation,
                )],
            )),
        );
        assert_eq!(summary.auto, 1, "{target}");
        let graph = store.load(PROJECT).unwrap();
        assert_eq!(graph.assertions.len(), before + 1, "{target}");
        assert!(
            graph.assertions.last().unwrap().supersedes.is_none(),
            "{target}"
        );
    }
}

#[test]
fn the_models_supersedes_target_is_kept_when_it_is_a_head() {
    let (store, _dir) = store();
    let auth = seed_entity(&store, "auth");
    let head = seed_decision(&store, &auth, "Use sessions", agent_stamp());
    let relation = format!(
        r#"{{"kind": "supersedes", "target": "{head}", "reasoning": "the user changed course"}}"#
    );
    let summary = run(
        &store,
        &Canned(answer(
            &[],
            &[assertion_json(
                "auth", "decision", "Use JWT", true, &relation,
            )],
        )),
    );
    // The head was agent-stated, so the rule lets the supersession land.
    assert_eq!(summary.auto, 1);
    let graph = store.load(PROJECT).unwrap();
    let old = graph.assertion(&head).unwrap();
    assert!(!old.is_head());
    let new = graph.assertions.iter().find(|a| a.is_head()).unwrap();
    assert_eq!(new.supersedes.as_ref().unwrap().id, head);
    assert_eq!(
        new.supersedes.as_ref().unwrap().reasoning,
        "the user changed course"
    );
}

#[test]
fn a_contradiction_waits_for_review() {
    let (store, _dir) = store();
    let auth = seed_entity(&store, "auth");
    let head = seed_decision(&store, &auth, "Use sessions", agent_stamp());
    let relation = format!(r#"{{"kind": "contradicts", "target": "{head}"}}"#);
    let summary = run(
        &store,
        &Canned(answer(
            &[],
            &[assertion_json(
                "auth",
                "fact",
                "Auth uses JWT today",
                true,
                &relation,
            )],
        )),
    );
    assert_eq!(summary.pending, 1);
    let pending = proposals(&store, ProposalStatus::Pending);
    match &pending[0].payload {
        ProposalPayload::Assertion { relation, .. } => {
            assert_eq!(relation.kind, RelationKind::Contradicts);
            assert_eq!(relation.target.as_deref(), Some(head.as_str()));
        }
        other => panic!("expected an assertion proposal, got {other:?}"),
    }
    assert_eq!(store.load(PROJECT).unwrap().assertions.len(), 1);
}

#[test]
fn a_duplicate_is_dismissed_for_the_trail() {
    let (store, _dir) = store();
    let auth = seed_entity(&store, "auth");
    seed_decision(&store, &auth, "Use JWT.", user_stamp());
    let summary = run(
        &store,
        &Canned(answer(
            &[],
            &[assertion_json(
                "auth",
                "decision",
                "use jwt",
                true,
                r#"{"kind": "new"}"#,
            )],
        )),
    );
    assert_eq!(summary.dismissed, 1);
    let dismissed = proposals(&store, ProposalStatus::Dismissed);
    assert_eq!(dismissed.len(), 1);
    assert_eq!(dismissed[0].dismiss_reason, Some(DismissReason::Duplicate));
    assert_eq!(
        dismissed[0].ruled_by.as_ref().unwrap().kind,
        AuthorKind::Extractor
    );
    assert_eq!(store.load(PROJECT).unwrap().assertions.len(), 1);
}

#[test]
fn an_unknown_slug_is_skipped_and_the_rest_proceeds() {
    let (store, _dir) = store();
    seed_entity(&store, "auth");
    let summary = run(
        &store,
        &Canned(answer(
            &[],
            &[
                assertion_json("nonesuch", "fact", "Orphan", false, r#"{"kind": "new"}"#),
                assertion_json("auth", "fact", "Kept", false, r#"{"kind": "new"}"#),
            ],
        )),
    );
    assert_eq!(summary.assertions_skipped, 1);
    assert_eq!(summary.auto, 1);
    let graph = store.load(PROJECT).unwrap();
    assert_eq!(graph.assertions.len(), 1);
    assert_eq!(graph.assertions[0].statement, "Kept");
}

#[test]
fn a_new_entity_can_carry_an_assertion_in_the_same_run() {
    let (store, _dir) = store();
    let summary = run(
        &store,
        &Canned(answer(
            &[entity_json("billing")],
            &[assertion_json(
                "billing",
                "constraint",
                "Never store card numbers",
                true,
                r#"{"kind": "new"}"#,
            )],
        )),
    );
    assert_eq!(summary.entities_landed, 1);
    assert_eq!(summary.auto, 1);
    let graph = store.load(PROJECT).unwrap();
    let billing = graph.entities.iter().find(|e| e.slug == "billing").unwrap();
    assert_eq!(graph.assertions[0].about, vec![billing.id.clone()]);
    assert_eq!(graph.assertions[0].kind, AssertionKind::Constraint);
}
