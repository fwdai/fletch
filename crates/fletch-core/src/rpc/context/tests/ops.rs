use super::test_support::*;
use super::*;

use serde_json::json;

use crate::context::{AssertionStatus, AuthorKind, Rel};

#[tokio::test]
async fn an_entity_lands_once_per_slug() {
    let (d, _dir) = dispatcher();

    let resp = call(
        &d,
        "context_record_entity",
        json!({ "slug": "billing", "kind": "feature", "name": "Billing", "summary": "invoices" }),
    )
    .await;
    let p = payload(&resp);
    assert_eq!(p["slug"], "billing");
    let id = p["id"].as_str().unwrap();

    let graph = d.store.load(PROJECT).unwrap();
    let e = graph.entity(id).unwrap();
    assert_eq!(e.author.agent_id.as_deref(), Some(AGENT));
    assert_eq!(e.source.kind, SourceKind::AgentTurn);
    assert_eq!(e.source.reference.as_deref(), Some("sess-1"));

    let e = error(
        &call(
            &d,
            "context_record_entity",
            json!({ "slug": "billing", "name": "Billing again", "summary": "dupe" }),
        )
        .await,
    );
    assert!(e.contains("already taken") && e.contains("`id`"), "{e}");
}

#[tokio::test]
async fn relates_links_the_new_entity_to_resolved_ones() {
    let (d, _dir) = dispatcher();
    let payments = entity(&d, "payments").await;

    let resp = call(
        &d,
        "context_record_entity",
        json!({
            "slug": "billing", "name": "Billing", "summary": "invoices",
            "relates": [{ "rel": "part_of", "to": "payments" }],
        }),
    )
    .await;
    let billing = payload(&resp)["id"].as_str().unwrap().to_string();

    let graph = d.store.load(PROJECT).unwrap();
    assert!(graph
        .relations
        .iter()
        .any(|r| r.from == billing && r.to == payments && r.rel == Rel::PartOf));
}

#[tokio::test]
async fn a_decision_about_an_unknown_entity_says_how_to_fix_it() {
    let (d, _dir) = dispatcher();

    let e = error(&call(&d, "context_record_decision", decision("nope", "x")).await);
    assert!(
        e.contains("nope") && e.contains("context_record_entity"),
        "{e}"
    );

    let e = error(&call(&d, "context_record_decision", decision("", "x")).await);
    assert!(e.contains("`about`"), "{e}");
}

#[tokio::test]
async fn a_restatement_is_not_recorded_twice() {
    let (d, _dir) = dispatcher();
    entity(&d, "billing").await;

    let first = payload(
        &call(
            &d,
            "context_record_decision",
            decision("billing", "Invoices are immutable."),
        )
        .await,
    );
    let again = payload(
        &call(
            &d,
            "context_record_decision",
            decision("billing", "invoices are immutable"),
        )
        .await,
    );
    assert_eq!(again["already_recorded"], first["id"]);
    assert_eq!(d.store.load(PROJECT).unwrap().assertions.len(), 1);
}

#[tokio::test]
async fn a_conflicting_decision_is_a_two_step() {
    let (d, _dir) = dispatcher();
    entity(&d, "billing").await;
    let first = payload(
        &call(
            &d,
            "context_record_decision",
            decision("billing", "Invoices are immutable"),
        )
        .await,
    );

    // Step one: nothing written, the head named.
    let conflict = payload(
        &call(
            &d,
            "context_record_decision",
            decision("billing", "Invoices may be voided"),
        )
        .await,
    );
    assert_eq!(conflict["conflict"], "related");
    assert_eq!(conflict["heads"][0]["id"], first["id"]);
    assert_eq!(conflict["heads"][0]["statement"], "Invoices are immutable");
    assert_eq!(conflict["heads"][0]["author_kind"], "agent");
    assert!(conflict["hint"].as_str().unwrap().contains("supersedes"));
    assert_eq!(d.store.load(PROJECT).unwrap().assertions.len(), 1);

    // Step two: the agent says how the two relate.
    let mut args = decision("billing", "Invoices may be voided");
    args["supersedes"] = json!({ "id": first["id"], "reasoning": "finance asked for voids" });
    let second = payload(&call(&d, "context_record_decision", args).await);
    assert_eq!(second["status"], "provisional");

    let graph = d.store.load(PROJECT).unwrap();
    assert_eq!(graph.assertions.len(), 2);
    let old = graph.assertion(first["id"].as_str().unwrap()).unwrap();
    assert_eq!(old.superseded_by.as_deref(), second["id"].as_str());
}

#[tokio::test]
async fn coexists_records_alongside_the_head() {
    let (d, _dir) = dispatcher();
    entity(&d, "billing").await;
    payload(
        &call(
            &d,
            "context_record_decision",
            decision("billing", "Invoices are immutable"),
        )
        .await,
    );

    let mut args = decision("billing", "Invoices are numbered per tenant");
    args["coexists"] = json!(true);
    payload(&call(&d, "context_record_decision", args).await);
    assert_eq!(d.store.load(PROJECT).unwrap().assertions.len(), 2);
}

#[tokio::test]
async fn the_user_s_word_lands_confirmed_and_the_agent_s_provisional() {
    let (d, _dir) = dispatcher();
    entity(&d, "billing").await;

    let mut args = decision("billing", "Never store card numbers");
    args["kind"] = json!("constraint");
    args["stated_by_user"] = json!(true);
    let user = payload(&call(&d, "context_record_decision", args).await);
    assert_eq!(user["status"], "confirmed");

    let mut args = decision("billing", "Invoices render server-side");
    args["domain"] = json!("implementation");
    let agent = payload(&call(&d, "context_record_decision", args).await);
    assert_eq!(agent["status"], "provisional");

    let graph = d.store.load(PROJECT).unwrap();
    let user = graph.assertion(user["id"].as_str().unwrap()).unwrap();
    assert_eq!(user.status, AssertionStatus::Confirmed);
    assert_eq!(user.source.kind, SourceKind::UserTurn);
    assert_eq!(user.author.kind, AuthorKind::Agent);
    let agent = graph.assertion(agent["id"].as_str().unwrap()).unwrap();
    assert_eq!(agent.status, AssertionStatus::Provisional);
    assert_eq!(agent.source.kind, SourceKind::AgentTurn);
    assert_eq!(agent.provenance.workspace_id.as_deref(), Some(AGENT));
    assert_eq!(agent.provenance.session_id.as_deref(), Some("sess-1"));
}

#[tokio::test]
async fn long_statements_are_refused_with_the_cap() {
    let (d, _dir) = dispatcher();
    entity(&d, "billing").await;

    let e = error(
        &call(
            &d,
            "context_record_decision",
            decision("billing", &"x".repeat(301)),
        )
        .await,
    );
    assert!(e.contains("`statement`") && e.contains("300"), "{e}");
    assert_eq!(d.store.load(PROJECT).unwrap().assertions.len(), 0);
}

#[tokio::test]
async fn context_get_serves_markdown_and_logs_the_misses() {
    let (d, _dir) = dispatcher();
    entity(&d, "billing").await;

    let resp = call(
        &d,
        "context_get",
        json!({ "entities": ["billing", "shipping"] }),
    )
    .await;
    assert!(resp.ok, "{:?}", resp.error);
    assert!(resp.stdout.unwrap().contains("billing"));

    let stats = d.store.stats(PROJECT).unwrap();
    assert_eq!(stats.reads, 1);
    assert_eq!(stats.top_misses, vec![("shipping".to_string(), 1)]);

    // No args is the map, not an error.
    assert!(call(&d, "context_get", serde_json::Value::Null).await.ok);
}

#[tokio::test]
async fn link_and_unlink_resolve_slugs() {
    let (d, _dir) = dispatcher();
    let billing = entity(&d, "billing").await;
    let payments = entity(&d, "payments").await;

    let p = payload(
        &call(
            &d,
            "context_link",
            json!({ "from": "billing", "to": "payments", "rel": "part_of" }),
        )
        .await,
    );
    assert_eq!(p["linked"], true);
    let graph = d.store.load(PROJECT).unwrap();
    assert!(graph
        .relations
        .iter()
        .any(|r| r.from == billing && r.to == payments));

    payload(
        &call(
            &d,
            "context_link",
            json!({ "from": "billing", "to": "payments", "rel": "part_of", "remove": true }),
        )
        .await,
    );
    assert!(d.store.load(PROJECT).unwrap().relations.is_empty());

    let e = error(
        &call(
            &d,
            "context_link",
            json!({ "from": "billing", "to": "nope", "rel": "serves" }),
        )
        .await,
    );
    assert!(
        e.contains("nope") && e.contains("context_record_entity"),
        "{e}"
    );
}
