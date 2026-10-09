//! What one run proposes. Every assertion goes through the service's write
//! policy, which holds extractor writes (`Landing::Held`) or finds them
//! duplicates; nothing here lands on the graph.

use crate::capture::extract::prompt::PROMPT_VERSION;
use crate::context::model::*;
use crate::context::ContextService;

use super::{
    agent_stamp, extracted_at, project, run, run_row, seed_decision, seed_decision_as, seed_entity,
    service, user_stamp, Canned, Failing, AGENT_QUOTE, PROJECT, REPO, USER_QUOTE,
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

fn pending(service: &ContextService) -> Vec<Proposal> {
    service
        .store()
        .proposals(PROJECT, Some(ProposalStatus::Pending))
        .unwrap()
}

/// The pending assertion proposals, as `(input, stamp, relation, about_pending, evidence)`.
#[allow(clippy::type_complexity)]
fn held(
    service: &ContextService,
) -> Vec<(
    AssertionInput,
    Stamp,
    ProposedRelation,
    Vec<String>,
    Vec<Evidence>,
)> {
    pending(service)
        .into_iter()
        .filter_map(|p| match p.payload {
            ProposalPayload::Assertion {
                input,
                stamp,
                relation,
                about_pending,
            } => Some((input, stamp, relation, about_pending, p.evidence)),
            ProposalPayload::Entity { .. } => None,
        })
        .collect()
}

fn pending_entities(service: &ContextService) -> Vec<EntityInput> {
    pending(service)
        .into_iter()
        .filter_map(|p| match p.payload {
            ProposalPayload::Entity { input, .. } => Some(input),
            ProposalPayload::Assertion { .. } => None,
        })
        .collect()
}

fn assertions(service: &ContextService) -> Vec<Assertion> {
    service.store().load(PROJECT).unwrap().assertions
}

#[test]
fn a_fenced_answer_proposes_the_entity_and_the_run_is_recorded() {
    let (service, _dir) = service();
    let text = format!("```json\n{}\n```", answer(&[entity_json("auth")], &[]));
    let summary = run(&service, &Canned(text.clone()));

    assert_eq!(summary.entities_proposed, 1);
    assert!(summary.error.is_none());
    // Proposed, not landed.
    assert!(service.store().load(PROJECT).unwrap().entities.is_empty());
    let proposals = pending(&service);
    assert_eq!(proposals.len(), 1);
    assert_eq!(
        proposals[0].observation_id.as_deref(),
        Some(summary.observation_id.as_str())
    );
    match &proposals[0].payload {
        ProposalPayload::Entity { input, stamp } => {
            assert_eq!(input.slug, "auth");
            assert_eq!(stamp.author.kind, AuthorKind::Extractor);
            assert_eq!(stamp.author.provider.as_deref(), Some("claude"));
            assert_eq!(stamp.provenance.repo.as_deref(), Some(REPO));
        }
        other => panic!("expected an entity proposal, got {other:?}"),
    }

    let (output, error) = run_row(&service, &summary.observation_id);
    assert_eq!(output.as_deref(), Some(text.as_str()));
    assert!(error.is_none());
    assert!(extracted_at(&service, &summary.observation_id).is_some());
    let runs: i64 = service.with_conn(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM context.extractor_runs WHERE prompt_version = ?1",
            [PROMPT_VERSION],
            |r| r.get(0),
        )
        .unwrap()
    });
    assert_eq!(runs, 1);
}

#[test]
fn malformed_output_is_kept_on_the_run_and_nothing_is_proposed() {
    let (service, _dir) = service();
    let summary = run(&service, &Canned("Sorry, nothing to report.".into()));

    assert!(summary.error.as_deref().unwrap().contains("unparseable"));
    let (output, error) = run_row(&service, &summary.observation_id);
    assert_eq!(output.as_deref(), Some("Sorry, nothing to report."));
    assert!(error.unwrap().contains("unparseable"));
    assert!(service.store().load(PROJECT).unwrap().entities.is_empty());
    assert!(service.store().proposals(PROJECT, None).unwrap().is_empty());
    assert!(extracted_at(&service, &summary.observation_id).is_none());
}

#[test]
fn a_failed_run_is_kept_with_its_error_and_not_marked_extracted() {
    let (service, _dir) = service();
    let summary = run(&service, &Failing);
    assert_eq!(summary.error.as_deref(), Some("not logged in"));
    let (output, error) = run_row(&service, &summary.observation_id);
    assert!(output.is_none());
    assert_eq!(error.as_deref(), Some("not logged in"));
    assert!(extracted_at(&service, &summary.observation_id).is_none());
}

/// The prompt version the run is recorded with.
#[test]
fn the_run_records_prompt_version_3() {
    let (service, _dir) = service();
    let summary = run(&service, &Canned(answer(&[], &[])));
    let version: String = service.with_conn(|conn| {
        conn.query_row(
            "SELECT prompt_version FROM context.extractor_runs WHERE observation_id = ?1",
            [&summary.observation_id],
            |r| r.get(0),
        )
        .unwrap()
    });
    assert_eq!(version, "3");
}

/// A model that ignores the caps cannot flood the review: the first entity
/// and the first three assertions are looked at, the rest only counted.
#[test]
fn proposals_past_the_caps_are_counted_and_dropped() {
    let (service, _dir) = service();
    seed_entity(&service, "auth");
    let assertions: Vec<String> = ["One", "Two", "Three", "Four", "Five"]
        .iter()
        .map(|s| assertion_json("auth", "fact", s, AGENT_QUOTE, r#"{"kind": "new"}"#))
        .collect();
    let summary = run(
        &service,
        &Canned(answer(
            &[entity_json("billing"), entity_json("search")],
            &assertions,
        )),
    );
    assert_eq!(summary.capped, 3);
    assert_eq!(summary.entities_proposed, 1);
    assert_eq!(summary.held, 3);
    let slugs: Vec<String> = pending_entities(&service)
        .into_iter()
        .map(|e| e.slug)
        .collect();
    assert_eq!(slugs, ["billing"]);
    let mut statements: Vec<String> = held(&service)
        .into_iter()
        .map(|(input, ..)| input.statement)
        .collect();
    statements.sort();
    assert_eq!(statements, ["One", "Three", "Two"]);
}

/// The implementation domain is below the extractor's bar, whatever its
/// evidence: counted, not held.
#[test]
fn an_implementation_domain_assertion_is_dropped() {
    let (service, _dir) = service();
    seed_entity(&service, "auth");
    let implementation = assertion_json(
        "auth",
        "decision",
        "Refresh in a loop",
        USER_QUOTE,
        r#"{"kind": "new"}"#,
    )
    .replace(r#""architectural""#, r#""implementation""#);
    let summary = run(
        &service,
        &Canned(answer(
            &[],
            &[
                implementation,
                assertion_json("auth", "fact", "Kept", AGENT_QUOTE, r#"{"kind": "new"}"#),
            ],
        )),
    );
    assert_eq!(summary.off_domain, 1);
    assert_eq!(summary.held, 1);
    let held = held(&service);
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].0.statement, "Kept");
}

#[test]
fn an_existing_entity_is_skipped_and_a_new_one_is_proposed() {
    let (service, _dir) = service();
    seed_entity(&service, "auth");
    // One entity per run, by the cap.
    let mut summary = run(&service, &Canned(answer(&[entity_json("auth")], &[])));
    assert_eq!(summary.entities_skipped, 1);
    assert_eq!(summary.entities_proposed, 0);
    summary = run(&service, &Canned(answer(&[entity_json("billing")], &[])));
    assert_eq!(summary.entities_skipped, 0);
    assert_eq!(summary.entities_proposed, 1);
    let slugs: Vec<String> = pending_entities(&service)
        .into_iter()
        .map(|e| e.slug)
        .collect();
    assert_eq!(slugs, ["billing"]);
    assert_eq!(service.store().load(PROJECT).unwrap().entities.len(), 1);
}

/// An entity a run proposed and nobody has ruled on yet is not proposed
/// again by the next run; that run's assertions about it still name it.
#[test]
fn a_pending_entity_is_not_proposed_again() {
    let (service, _dir) = service();
    let first = run(&service, &Canned(answer(&[entity_json("billing")], &[])));
    assert_eq!(first.entities_proposed, 1);

    let about_billing = assertion_json(
        "billing",
        "fact",
        "Tokens expire hourly",
        AGENT_QUOTE,
        r#"{"kind": "new"}"#,
    );
    let second = run(
        &service,
        &Canned(answer(&[entity_json("billing")], &[about_billing])),
    );
    assert_eq!(second.entities_proposed, 0);
    assert_eq!(second.entities_skipped, 1);
    assert_eq!(second.held, 1);
    let slugs: Vec<String> = pending_entities(&service)
        .into_iter()
        .map(|e| e.slug)
        .collect();
    assert_eq!(slugs, ["billing"]);
    let (_, _, _, about_pending, _) = &held(&service)[0];
    assert_eq!(about_pending, &["billing"]);
}

/// The model is never shown pending entities, so a later run names one
/// without listing it; the assertion still carries it as `about_pending`.
#[test]
fn an_assertion_naming_an_entity_pending_from_an_earlier_run_keeps_it() {
    let (service, _dir) = service();
    run(&service, &Canned(answer(&[entity_json("billing")], &[])));

    let about_billing = assertion_json(
        "billing",
        "fact",
        "Tokens expire hourly",
        AGENT_QUOTE,
        r#"{"kind": "new"}"#,
    );
    let summary = run(&service, &Canned(answer(&[], &[about_billing])));
    assert_eq!(summary.held, 1);
    assert_eq!(summary.assertions_skipped, 0);
    let (input, _, _, about_pending, _) = &held(&service)[0];
    assert!(input.about.is_empty());
    assert_eq!(about_pending, &["billing"]);
}

/// What a person dismissed stays dismissed: the next run's proposal of the
/// same entity is skipped, and its repeat of the same assertion is not held.
#[test]
fn a_dismissed_proposal_is_not_proposed_again() {
    let (service, _dir) = service();
    seed_entity(&service, "auth");
    let about_auth = assertion_json(
        "auth",
        "fact",
        "Tokens expire hourly",
        AGENT_QUOTE,
        r#"{"kind": "new"}"#,
    );
    let both = || {
        Canned(answer(
            &[entity_json("billing")],
            std::slice::from_ref(&about_auth),
        ))
    };
    run(&service, &both());
    service
        .dismiss_all_pending(&project(), DismissReason::Wrong, Author::user(), None)
        .unwrap();

    let again = run(&service, &both());
    assert_eq!(again.entities_proposed, 0);
    assert_eq!(again.entities_skipped, 1);
    assert_eq!(again.held, 0);
    assert_eq!(again.redismissed, 1);
    assert!(pending(&service).is_empty());
}

/// The same assertion from two runs is one card with both runs' quotes.
#[test]
fn a_repeated_assertion_is_one_proposal_with_both_quotes() {
    let (service, _dir) = service();
    seed_entity(&service, "auth");
    let saying = |quote: &str| {
        Canned(answer(
            &[],
            &[assertion_json(
                "auth",
                "fact",
                "Tokens expire hourly",
                quote,
                r#"{"kind": "new"}"#,
            )],
        ))
    };
    assert_eq!(run(&service, &saying(AGENT_QUOTE)).held, 1);
    assert_eq!(
        run(&service, &saying("the refresh job runs each hour")).held,
        1
    );

    let held = held(&service);
    assert_eq!(held.len(), 1);
    let quotes: Vec<&str> = held[0].4.iter().map(|e| e.quote.as_str()).collect();
    assert_eq!(quotes, [AGENT_QUOTE, "the refresh job runs each hour"]);
}

/// Both a user-stated and an agent-stated assertion are held, each carrying
/// the status it would land with: the user's word is `confirmed`, the
/// agent's `provisional` until its branch's fate is known.
#[test]
fn assertions_are_held_with_their_status_whoever_stated_them() {
    let (service, _dir) = service();
    let auth = seed_entity(&service, "auth");
    let summary = run(
        &service,
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
    );
    assert_eq!(summary.held, 2);
    assert_eq!(summary.unverified, 0);
    assert!(assertions(&service).is_empty(), "nothing lands");
    let held = held(&service);
    let by_statement = |s: &str| held.iter().find(|(i, ..)| i.statement == s).unwrap();

    let (input, stamp, _, _, evidence) = by_statement("Tokens expire hourly");
    assert_eq!(input.status, AssertionStatus::Provisional);
    assert_eq!(stamp.source.kind, SourceKind::AgentTurn);
    assert_eq!(evidence[0].quote, AGENT_QUOTE);
    assert_eq!(evidence[0].turn_id.as_deref(), Some("t1"));

    // The user's record states the user's words; the model's sentence is
    // its reading and yields to a rationale the model gave.
    let (input, stamp, _, _, evidence) = by_statement(USER_QUOTE);
    assert_eq!(input.rationale, "because");
    assert_eq!(input.status, AssertionStatus::Confirmed);
    assert_eq!(input.about, vec![auth]);
    assert_eq!(stamp.source.kind, SourceKind::UserTurn);
    assert_eq!(stamp.source.reference.as_deref(), Some("t1"));
    assert_eq!(stamp.author.kind, AuthorKind::Extractor);
    assert_eq!(stamp.provenance.workspace_id.as_deref(), Some("ws-1"));
    assert_eq!(stamp.provenance.repo.as_deref(), Some(REPO));
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].quote, USER_QUOTE);
    assert_eq!(evidence[0].session_id.as_deref(), Some("sess-1"));
}

/// The model cannot claim the user said it: `stated_by_user` in its answer
/// is ignored, and only a quote from the user's text would do.
#[test]
fn stated_by_user_in_the_answer_does_not_make_it_user_stated() {
    let (service, _dir) = service();
    seed_entity(&service, "auth");
    let forged = format!(
        r#"{{"about": ["auth"], "kind": "decision", "domain": "architectural", "stance": "adopted",
            "statement": "Use JWT", "rationale": "because", "stated_by_user": true,
            "relation": {{"kind": "new"}}, "evidence": [{{"quote": "{AGENT_QUOTE}"}}]}}"#
    );
    let summary = run(&service, &Canned(answer(&[], &[forged])));
    assert_eq!(summary.held, 1);
    let (input, stamp, ..) = &held(&service)[0];
    assert_eq!(stamp.source.kind, SourceKind::AgentTurn);
    assert_eq!(input.status, AssertionStatus::Provisional);
}

/// No quote found anywhere in the turns — a paraphrase, one too short to
/// match, or none at all: the statement may be invented, so nothing is held.
#[test]
fn an_assertion_without_a_verifiable_quote_is_not_held() {
    let (service, _dir) = service();
    seed_entity(&service, "auth");
    let summary = run(
        &service,
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
                assertion_json("auth", "fact", "Too short", "JWT", r#"{"kind": "new"}"#),
                assertion_json("auth", "fact", "Nothing cited", "", r#"{"kind": "new"}"#),
            ],
        )),
    );
    assert_eq!(summary.unverified, 3);
    assert_eq!(summary.held, 0);
    assert!(pending(&service).is_empty());
    assert!(assertions(&service).is_empty());
}

/// A restatement of a current head is the policy's `Duplicate`: counted,
/// nothing held, nothing landed.
#[test]
fn a_duplicate_of_a_head_is_counted_and_not_held() {
    let (service, _dir) = service();
    let auth = seed_entity(&service, "auth");
    // The head is the user's own sentence; a user-quoted restatement (whose
    // statement becomes the quote) and an agent-quoted one both duplicate it.
    seed_decision(&service, &auth, "Use JWT for sessions.", user_stamp());
    let summary = run(
        &service,
        &Canned(answer(
            &[],
            &[
                assertion_json(
                    "auth",
                    "decision",
                    "use jwt",
                    USER_QUOTE,
                    r#"{"kind": "new"}"#,
                ),
                assertion_json(
                    "auth",
                    "decision",
                    "use JWT for sessions",
                    AGENT_QUOTE,
                    r#"{"kind": "new"}"#,
                ),
            ],
        )),
    );
    assert_eq!(summary.duplicates, 2);
    assert_eq!(summary.held, 0);
    assert!(pending(&service).is_empty());
    assert_eq!(assertions(&service).len(), 1);
}

/// Case, whitespace and the quote marks a model wraps a citation in do not
/// stop a verbatim quote from matching; a paraphrased one does not (nor a
/// short one: see `an_assertion_without_a_verifiable_quote_is_not_held`).
#[test]
fn quotes_are_matched_after_normalisation() {
    let (service, _dir) = service();
    seed_entity(&service, "auth");
    let summary = run(
        &service,
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
            ],
        )),
    );
    assert_eq!(summary.held, 2);
    assert_eq!(summary.unverified, 1);
    let held = held(&service);
    let by_statement = |s: &str| held.iter().find(|(i, ..)| i.statement == s).unwrap();
    // The statement is the quote as the user would read it: their case,
    // single-spaced, the citation's quote marks and comma gone.
    assert_eq!(
        by_statement("USE jwt for Sessions").1.source.kind,
        SourceKind::UserTurn
    );
    assert_eq!(
        by_statement("Tokens expire").1.source.kind,
        SourceKind::AgentTurn
    );
    // The evidence keeps the quote as the model gave it, on one line.
    let quotes: Vec<&str> = held
        .iter()
        .flat_map(|(.., evidence)| evidence.iter().map(|e| e.quote.as_str()))
        .collect();
    assert!(
        quotes.contains(&r#"" USE jwt for Sessions, ""#),
        "{quotes:?}"
    );
}

/// Slugs are the store's to refuse, so they are normalised first: lowercase,
/// runs of other characters to `-`, 64 at most. One with nothing left is
/// skipped. One run each, by the cap.
#[test]
fn an_entity_slug_is_normalised_and_an_empty_one_is_skipped() {
    let (service, _dir) = service();
    let long = "x".repeat(80);
    let entities = [
        r#"{"slug": " Auth  Service! ", "kind": "feature", "name": "Auth Service", "summary": "s"}"#.to_string(),
        r#"{"slug": "Billing.v2_API", "kind": "module", "name": "Billing", "summary": "s"}"#.to_string(),
        format!(r#"{{"slug": "{long}--", "kind": "topic", "name": "Long", "summary": "s"}}"#),
        r#"{"slug": "!!!", "kind": "topic", "name": "Nameless", "summary": "s"}"#.to_string(),
    ];
    let (mut proposed, mut skipped) = (0, 0);
    for entity in entities {
        let summary = run(&service, &Canned(answer(&[entity], &[])));
        proposed += summary.entities_proposed;
        skipped += summary.entities_skipped;
    }
    assert_eq!(proposed, 3);
    assert_eq!(skipped, 1);
    let slugs: Vec<String> = pending_entities(&service)
        .into_iter()
        .map(|e| e.slug)
        .collect();
    let long = "x".repeat(64);
    assert_eq!(slugs, ["auth-service", "billing.v2_api", long.as_str()]);
}

/// The model's `supersedes` is handed on as the candidate's relation; with
/// no reasoning from the model, the rationale stands in.
#[test]
fn the_models_supersedes_is_carried_on_the_held_proposal() {
    let (service, _dir) = service();
    let auth = seed_entity(&service, "auth");
    let head = seed_decision(&service, &auth, "Use sessions", agent_stamp());
    let relation = format!(r#"{{"kind": "supersedes", "target": "{head}"}}"#);
    let summary = run(
        &service,
        &Canned(answer(
            &[],
            &[assertion_json(
                "auth", "decision", "Use JWT", USER_QUOTE, &relation,
            )],
        )),
    );
    assert_eq!(summary.held, 1);
    let (_, _, relation, ..) = &held(&service)[0];
    assert_eq!(relation.kind, RelationKind::Supersedes);
    assert_eq!(relation.target.as_deref(), Some(head.as_str()));
    assert_eq!(relation.reasoning.as_deref(), Some("because"));
    // The head is untouched.
    let graph = service.store().load(PROJECT).unwrap();
    assert_eq!(graph.assertions.len(), 1);
    assert_eq!(graph.current, vec![head]);
}

#[test]
fn the_models_contradicts_is_carried_on_the_held_proposal() {
    let (service, _dir) = service();
    let auth = seed_entity(&service, "auth");
    let head = seed_decision(&service, &auth, "Use sessions", agent_stamp());
    let relation = format!(r#"{{"kind": "contradicts", "target": "{head}"}}"#);
    let summary = run(
        &service,
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
    assert_eq!(summary.held, 1);
    let (_, _, relation, ..) = &held(&service)[0];
    assert_eq!(relation.kind, RelationKind::Contradicts);
    assert_eq!(relation.target.as_deref(), Some(head.as_str()));
    assert_eq!(assertions(&service).len(), 1);
}

#[test]
fn an_unknown_slug_is_skipped_and_the_rest_proceeds() {
    let (service, _dir) = service();
    seed_entity(&service, "auth");
    let summary = run(
        &service,
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
    assert_eq!(summary.held, 1);
    let held = held(&service);
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].0.statement, "Kept");
}

/// An assertion about an entity proposed in the same run names it in
/// `about_pending` (by its normalised slug), with nothing in `about` yet.
#[test]
fn an_assertion_about_a_proposed_entity_carries_it_as_pending() {
    let (service, _dir) = service();
    let auth = seed_entity(&service, "auth");
    let summary = run(
        &service,
        &Canned(answer(
            &[
                r#"{"slug": "Billing Service", "kind": "feature", "name": "Billing", "summary": "s"}"#
                    .to_string(),
            ],
            &[
                assertion_json(
                    "Billing Service",
                    "constraint",
                    "Never store card numbers",
                    "never store tokens in local storage",
                    r#"{"kind": "new"}"#,
                ),
                format!(
                    r#"{{"about": ["auth", "billing-service"], "kind": "fact", "domain": "architectural",
                        "stance": "adopted", "statement": "Billing calls auth", "rationale": "",
                        "relation": {{"kind": "new"}}, "evidence": [{{"quote": "{AGENT_QUOTE}"}}]}}"#
                ),
            ],
        )),
    );
    assert_eq!(summary.entities_proposed, 1);
    assert_eq!(summary.held, 2);
    assert_eq!(summary.assertions_skipped, 0);
    let held = held(&service);
    let by_statement = |s: &str| held.iter().find(|(i, ..)| i.statement == s).unwrap();
    // Quoted from the user, so the record states the quote.
    let (input, _, _, about_pending, _) = by_statement("never store tokens in local storage");
    assert!(input.about.is_empty());
    assert_eq!(about_pending, &["billing-service"]);
    assert_eq!(input.kind, AssertionKind::Constraint);
    let (input, _, _, about_pending, _) = by_statement("Billing calls auth");
    assert_eq!(input.about, vec![auth]);
    assert_eq!(about_pending, &["billing-service"]);
}

/// The run proposes; what the workspace already has provisional is not its
/// to settle, at archive or any other time.
#[test]
fn a_run_settles_nothing() {
    let (service, _dir) = service();
    let auth = seed_entity(&service, "auth");
    let mut stamp = agent_stamp();
    stamp.provenance = super::provenance();
    let provisional = seed_decision_as(
        &service,
        &auth,
        "Recorded mid-branch",
        stamp,
        AssertionStatus::Provisional,
    );
    run(
        &service,
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
    let graph = service.store().load(PROJECT).unwrap();
    assert_eq!(
        graph.assertion(&provisional).unwrap().status,
        AssertionStatus::Provisional
    );
}
