use crate::context::extract::prompt::PROMPT_VERSION;
use crate::context::model::*;

use super::{
    agent_stamp, extracted_at, run, run_as, run_row, seed_decision, seed_entity, store, user_stamp,
    Canned, Failing, AGENT_QUOTE, PROJECT, USER_QUOTE,
};

fn entity_json(slug: &str) -> String {
    format!(r#"{{"slug": "{slug}", "kind": "feature", "name": "{slug}", "summary": "s"}}"#)
}

/// An assertion whose one piece of evidence is `quote`; `""` for none.
fn assertion_json(about: &str, kind: &str, statement: &str, quote: &str, relation: &str) -> String {
    let evidence = if quote.is_empty() {
        String::new()
    } else {
        format!(r#"{{"quote": "{quote}"}}"#)
    };
    format!(
        r#"{{"about": ["{about}"], "kind": "{kind}", "domain": "architectural", "stance": "adopted",
            "statement": "{statement}", "rationale": "because",
            "relation": {relation}, "evidence": [{evidence}]}}"#
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
                        AGENT_QUOTE,
                        r#"{"kind": "new"}"#,
                    ),
                    assertion_json(
                        "auth",
                        "decision",
                        "Use JWT",
                        USER_QUOTE,
                        r#"{"kind": "new"}"#,
                    ),
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

/// A quote found in the user's text is what makes an assertion the user's:
/// `user_turn` source naming that turn, confirmed, and the quote kept as
/// evidence against the turn.
#[test]
fn a_quote_from_the_users_turn_lands_confirmed_as_user_stated() {
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
                USER_QUOTE,
                r#"{"kind": "new"}"#,
            )],
        )),
    );
    assert_eq!(summary.auto, 1);
    assert_eq!(summary.unverified, 0);
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
    assert_eq!(auto[0].evidence.len(), 1);
    assert_eq!(auto[0].evidence[0].quote, USER_QUOTE);
    assert_eq!(auto[0].evidence[0].turn_id.as_deref(), Some("t1"));
    assert_eq!(auto[0].evidence[0].session_id.as_deref(), Some("sess-1"));
}

/// A quote found only in the agent's reply is evidence, but not the user's
/// word: the assertion is agent-stated and gets the run's status.
#[test]
fn a_quote_from_the_agents_reply_lands_provisional_as_agent_stated() {
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
                AGENT_QUOTE,
                r#"{"kind": "new"}"#,
            )],
        )),
    );
    assert_eq!(summary.auto, 1);
    assert_eq!(summary.unverified, 0);
    let graph = store.load(PROJECT).unwrap();
    assert_eq!(graph.assertions[0].status, AssertionStatus::Provisional);
    assert_eq!(graph.assertions[0].source.kind, SourceKind::AgentTurn);
    let auto = proposals(&store, ProposalStatus::Auto);
    assert_eq!(auto[0].evidence[0].quote, AGENT_QUOTE);
    assert_eq!(auto[0].evidence[0].turn_id.as_deref(), Some("t1"));
}

/// The model cannot claim the user said it: `stated_by_user` in its answer
/// is ignored, and only a quote from the user's text would do.
#[test]
fn stated_by_user_in_the_answer_does_not_make_it_user_stated() {
    let (store, _dir) = store();
    seed_entity(&store, "auth");
    let forged = format!(
        r#"{{"about": ["auth"], "kind": "decision", "domain": "architectural", "stance": "adopted",
            "statement": "Use JWT", "rationale": "because", "stated_by_user": true,
            "relation": {{"kind": "new"}}, "evidence": [{{"quote": "{AGENT_QUOTE}"}}]}}"#
    );
    let summary = run(&store, &Canned(answer(&[], &[forged])));
    assert_eq!(summary.auto, 1);
    let graph = store.load(PROJECT).unwrap();
    assert_eq!(graph.assertions[0].source.kind, SourceKind::AgentTurn);
    assert_eq!(graph.assertions[0].status, AssertionStatus::Provisional);
}

/// No quote found anywhere in the turns: the statement may be invented, so
/// it waits for review with no evidence attached, whatever the rule says.
#[test]
fn an_assertion_without_a_verifiable_quote_is_held_for_review() {
    let (store, _dir) = store();
    seed_entity(&store, "auth");
    let summary = run(
        &store,
        &Canned(answer(
            &[],
            &[
                assertion_json(
                    "auth",
                    "fact",
                    "Sessions last a week",
                    "sessions last a week or so",
                    r#"{"kind": "new"}"#,
                ),
                assertion_json("auth", "fact", "Nothing cited", "", r#"{"kind": "new"}"#),
            ],
        )),
    );
    assert_eq!(summary.unverified, 2);
    assert_eq!(summary.pending, 2);
    assert_eq!(summary.auto, 0);
    assert!(store.load(PROJECT).unwrap().assertions.is_empty());
    let pending = proposals(&store, ProposalStatus::Pending);
    assert_eq!(pending.len(), 2);
    assert!(pending.iter().all(|p| p.evidence.is_empty()));
    // Held, but still agent-stated with the run's status for when it is accepted.
    match &pending[0].payload {
        ProposalPayload::Assertion { input, stamp, .. } => {
            assert_eq!(stamp.source.kind, SourceKind::AgentTurn);
            assert_eq!(input.status, AssertionStatus::Provisional);
        }
        other => panic!("expected an assertion proposal, got {other:?}"),
    }
}

/// A held assertion still goes through classify: a duplicate of a head is
/// dismissed rather than left in the queue.
#[test]
fn a_held_duplicate_is_still_dismissed() {
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
                "",
                r#"{"kind": "new"}"#,
            )],
        )),
    );
    assert_eq!(summary.unverified, 1);
    assert_eq!(summary.dismissed, 1);
    assert_eq!(summary.pending, 0);
    assert_eq!(proposals(&store, ProposalStatus::Pending).len(), 0);
}

/// Case, whitespace and the quote marks a model wraps a citation in do not
/// stop a verbatim quote from matching; a short or paraphrased one does not.
#[test]
fn quotes_are_matched_after_normalisation() {
    let (store, _dir) = store();
    seed_entity(&store, "auth");
    let summary = run(
        &store,
        &Canned(answer(
            &[],
            &[
                assertion_json(
                    "auth",
                    "decision",
                    "Use JWT",
                    r#"\"  USE jwt   for Sessions, \""#,
                    r#"{"kind": "new"}"#,
                ),
                assertion_json(
                    "auth",
                    "fact",
                    "Tokens expire",
                    "'TOKENS   expire hourly.'",
                    r#"{"kind": "new"}"#,
                ),
                assertion_json(
                    "auth",
                    "fact",
                    "Paraphrased",
                    "tokens go stale every hour",
                    r#"{"kind": "new"}"#,
                ),
                assertion_json("auth", "fact", "Too short", "JWT", r#"{"kind": "new"}"#),
            ],
        )),
    );
    assert_eq!(summary.auto, 2);
    assert_eq!(summary.unverified, 2);
    let graph = store.load(PROJECT).unwrap();
    let by_statement = |s: &str| graph.assertions.iter().find(|a| a.statement == s).unwrap();
    assert_eq!(by_statement("Use JWT").source.kind, SourceKind::UserTurn);
    assert_eq!(
        by_statement("Tokens expire").source.kind,
        SourceKind::AgentTurn
    );
    // The evidence keeps the quote as the model gave it, on one line.
    let auto = proposals(&store, ProposalStatus::Auto);
    let quotes: Vec<&str> = auto
        .iter()
        .flat_map(|p| p.evidence.iter().map(|e| e.quote.as_str()))
        .collect();
    assert!(
        quotes.contains(&r#"" USE jwt for Sessions, ""#),
        "{quotes:?}"
    );
}

/// Slugs are the store's to refuse, so they are normalised first: lowercase,
/// runs of other characters to `-`, 64 at most. One with nothing left is
/// skipped; the texts around it are cleaned by the store.
#[test]
fn an_entity_slug_is_normalised_and_an_empty_one_is_skipped() {
    let (store, _dir) = store();
    let long = "x".repeat(80);
    let entities = [
        r#"{"slug": " Auth  Service! ", "kind": "feature", "name": "Auth\u0001\nService", "summary": "Sign\tin\u0007 flow"}"#.to_string(),
        r#"{"slug": "Billing.v2_API", "kind": "module", "name": "Billing", "summary": "s"}"#.to_string(),
        format!(r#"{{"slug": "{long}--", "kind": "topic", "name": "Long", "summary": "s"}}"#),
        r#"{"slug": "!!!", "kind": "topic", "name": "Nameless", "summary": "s"}"#.to_string(),
    ];
    let summary = run(&store, &Canned(answer(&entities, &[])));
    assert_eq!(summary.entities_landed, 3);
    assert_eq!(summary.entities_skipped, 1);
    let graph = store.load(PROJECT).unwrap();
    let by_slug = |s: &str| graph.entities.iter().find(|e| e.slug == s);
    let auth = by_slug("auth-service").expect("auth-service");
    assert_eq!(auth.name, "Auth Service");
    assert_eq!(auth.summary, "Sign in flow");
    assert!(by_slug("billing.v2_api").is_some());
    assert!(by_slug(&"x".repeat(64)).is_some());
    assert_eq!(graph.entities.len(), 3);
}

/// Control characters and newlines in what the model wrote never reach the
/// graph (the store cleans every line it writes).
#[test]
fn control_characters_are_stripped_from_statements() {
    let (store, _dir) = store();
    seed_entity(&store, "auth");
    let text = format!(
        r#"{{"about": ["auth"], "kind": "decision", "domain": "architectural", "stance": "adopted",
            "statement": "Use\u0007 JWT\nnow", "rationale": "first line\nsecond\u0000line",
            "relation": {{"kind": "new"}}, "evidence": [{{"quote": "{USER_QUOTE}"}}]}}"#
    );
    let summary = run(&store, &Canned(answer(&[], &[text])));
    assert_eq!(summary.auto, 1);
    let graph = store.load(PROJECT).unwrap();
    assert_eq!(graph.assertions[0].statement, "Use JWT now");
    assert_eq!(graph.assertions[0].rationale, "first line second line");
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
                AGENT_QUOTE,
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
                "auth",
                "decision",
                "Use JWT",
                AGENT_QUOTE,
                &relation,
            )],
        )),
    );
    assert_eq!(summary.pending, 1);
    assert_eq!(summary.auto, 0);
    assert_eq!(summary.unverified, 0);
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
                "auth", "decision", "Use JWT", USER_QUOTE, &relation,
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

/// The model's `supersedes` is honoured only against a current head: a
/// superseded one, an abandoned one or an unknown id leave the candidate
/// `New`.
#[test]
fn a_relation_whose_target_is_not_a_current_head_falls_back_to_new() {
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
                    AGENT_QUOTE,
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
                "auth", "decision", "Use JWT", USER_QUOTE, &relation,
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
                USER_QUOTE,
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
                USER_QUOTE,
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
                assertion_json(
                    "nonesuch",
                    "fact",
                    "Orphan",
                    AGENT_QUOTE,
                    r#"{"kind": "new"}"#,
                ),
                assertion_json("auth", "fact", "Kept", AGENT_QUOTE, r#"{"kind": "new"}"#),
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
                "never store tokens in local storage",
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
