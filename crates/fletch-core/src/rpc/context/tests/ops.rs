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

/// A workspace with one live session, so the dispatcher can read the user's
/// turns. The rows the lineage reader needs and nothing more.
fn seed_session(d: &ContextDispatcher) {
    let conn = d.db.lock();
    let now = crate::database::now_millis();
    conn.execute(
        "INSERT INTO projects (id, name, created_at) VALUES (?1, 'p', ?2)",
        rusqlite::params![FLETCH_PROJECT, now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO workspaces (id, project_id, name, created_at) VALUES (?1, ?2, ?1, ?3)",
        rusqlite::params![AGENT, FLETCH_PROJECT, now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, workspace_id, provider, created_at) VALUES ('sess-1', ?1, 'claude', ?2)",
        rusqlite::params![AGENT, now],
    )
    .unwrap();
}

fn user_said(d: &ContextDispatcher, turn_id: &str, text: &str) {
    assert!(crate::workspace::WorkspaceManager::new(d.db.clone())
        .insert_user_turn(AGENT, turn_id, text, &[])
        .unwrap());
}

#[tokio::test]
async fn the_user_s_word_lands_confirmed_and_the_agent_s_provisional() {
    let (d, _dir) = dispatcher();
    seed_session(&d);
    user_said(
        &d,
        "t1",
        "One rule for billing: never store card numbers, anywhere.",
    );
    entity(&d, "billing").await;

    let mut args = decision("billing", "Never store card numbers");
    args["kind"] = json!("constraint");
    args["user_quote"] = json!("“Never store card numbers, anywhere.”");
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
    assert_eq!(user.source.reference.as_deref(), Some("t1"));
    assert_eq!(user.author.kind, AuthorKind::Agent);
    let agent = graph.assertion(agent["id"].as_str().unwrap()).unwrap();
    assert_eq!(agent.status, AssertionStatus::Provisional);
    assert_eq!(agent.source.kind, SourceKind::AgentTurn);
    assert_eq!(agent.provenance.workspace_id.as_deref(), Some(AGENT));
    assert_eq!(agent.provenance.session_id.as_deref(), Some("sess-1"));
}

#[tokio::test]
async fn a_user_quote_the_user_never_wrote_is_refused() {
    let (d, _dir) = dispatcher();
    seed_session(&d);
    user_said(&d, "t1", "Let's keep invoices immutable.");
    entity(&d, "billing").await;

    // A paraphrase, the agent's own words, and no session at all: all refused,
    // nothing written. The claim is never taken from the caller.
    let mut args = decision("billing", "Invoices are immutable");
    args["user_quote"] = json!("invoices can never change");
    let e = error(&call(&d, "context_record_decision", args.clone()).await);
    assert!(e.contains("not found in the user's messages"), "{e}");
    assert!(d.store.load(PROJECT).unwrap().assertions.is_empty());

    args["user_quote"] = json!("immutable");
    let e = error(&call(&d, "context_record_decision", args).await);
    assert!(e.contains("not found"), "{e}");
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
