use super::*;
use crate::rpc::roadmap::test_support::*;
use crate::rpc::RpcDispatcher;

use serde_json::{json, Value};

use crate::roadmap::events::{self, EventActor, EventKind};
use crate::roadmap::store;
use crate::roadmap::types::{Horizon, ItemPatch, ItemSource, ItemStatus};

#[tokio::test]
async fn propose_creates_proposed_pm_rows_and_returns_their_codes() {
    let db = test_db("p1");
    let d = dispatcher(&db, "p1");

    let resp = d
        .dispatch(
            "r1",
            "roadmap_propose",
            &json!({"items": [
                {"title": "Ship the drainer", "why": "the queue needs one",
                 "horizon": "now", "area": "workflow", "accept": ["it drains"]},
                {"title": "Second", "why": "also", "horizon": "later"},
            ]}),
        )
        .await
        .0;
    assert!(resp.ok, "{resp:?}");

    let out: Value = serde_json::from_str(&resp.stdout.unwrap()).unwrap();
    let created = out["created"].as_array().unwrap();
    assert_eq!(created.len(), 2);
    assert_eq!(created[0]["code"], "MCA-100");
    assert_eq!(created[1]["code"], "MCA-101");
    assert_eq!(created[0]["title"], "Ship the drainer");

    let rows = store::list(&db.lock(), "p1").unwrap();
    assert_eq!(rows.len(), 2);
    for row in &rows {
        assert_eq!(row.status, ItemStatus::Proposed);
        assert_eq!(row.source, ItemSource::Pm);
        let history = events::list_for_item(&db.lock(), &row.id).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].kind, EventKind::Proposed);
        assert_eq!(history[0].actor, EventActor::Pm);
    }
    assert_eq!(rows[0].horizon, Horizon::Now);
    assert_eq!(rows[0].accept, vec!["it drains".to_string()]);
    assert_eq!(rows[0].area.as_deref(), Some("workflow"));
}

#[test]
fn propose_rejects_the_whole_batch_on_any_bad_item() {
    let db = test_db("p1");

    let resp = propose(&db, json!({ "items": [] }));
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("non-empty"));

    assert!(!propose(&db, Value::Null).ok);

    for (args, needle) in [
        (
            json!({"items": [{"title": "ok", "horizon": "next"}, {"title": "  ", "horizon": "next"}]}),
            "item 2",
        ),
        (
            json!({"items": [{"title": "ok", "horizon": "soon"}]}),
            "unknown horizon",
        ),
        (json!({"items": [{"title": "ok"}]}), "`horizon` is required"),
        (
            json!({"items": [{"title": "ok", "horizon": "now", "horizen": "next"}]}),
            "unknown field",
        ),
        (
            json!({"items": [{"title": "ok", "horizon": "now", "status": "open"}]}),
            "unknown field",
        ),
        (
            json!({"items": [{"title": "ok", "horizon": "now", "size": "M"}]}),
            "unknown field",
        ),
        (
            json!({"items": [{"title": "ok", "horizon": "now", "epic": "roadmap"}]}),
            "unknown field",
        ),
    ] {
        let resp = propose(&db, args);
        assert!(!resp.ok, "should have been rejected");
        let e = resp.error.unwrap();
        assert!(e.contains(needle), "expected {needle:?} in {e:?}");
    }

    let many: Vec<Value> = (0..MAX_BATCH + 1)
        .map(|n| json!({"title": format!("t{n}"), "horizon": "later"}))
        .collect();
    let resp = propose(&db, json!({ "items": many }));
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("too many"));

    assert!(store::list(&db.lock(), "p1").unwrap().is_empty());
}

#[test]
fn deps_must_name_a_code_on_the_board_or_an_item_in_this_batch() {
    let db = test_db("p1");
    assert!(propose(&db, one_item("first")).ok);

    let resp = propose(
        &db,
        json!({"items": [{"title": "second", "horizon": "next", "deps": ["MCA-100"]}]}),
    );
    assert!(resp.ok, "{resp:?}");

    let resp = propose(
        &db,
        json!({"items": [{"title": "third", "horizon": "next", "deps": ["MCA-999"]}]}),
    );
    assert!(!resp.ok);
    let e = resp.error.unwrap();
    assert!(
        e.contains("MCA-999") && e.contains("MCA-100, MCA-101"),
        "{e}"
    );
    assert!(e.contains("#n"), "{e}");
    assert_eq!(store::list(&db.lock(), "p1").unwrap().len(), 2);
}

#[test]
fn a_batch_can_order_itself_with_hash_references() {
    let db = test_db("p1");
    let resp = propose(
        &db,
        json!({"items": [
            {"title": "the seam", "horizon": "now"},
            {"title": "the drainer", "horizon": "now", "deps": ["#1"]},
            {"title": "the card", "horizon": "next", "deps": ["#2", "#1"]},
        ]}),
    );
    assert!(resp.ok, "{resp:?}");

    let rows = store::list(&db.lock(), "p1").unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows[0].deps.is_empty());
    assert_eq!(rows[1].deps, vec!["MCA-100".to_string()]);
    assert_eq!(
        rows[2].deps,
        vec!["MCA-101".to_string(), "MCA-100".to_string()]
    );

    let resp = propose(
        &db,
        json!({"items": [
            {"title": "later", "horizon": "next", "deps": ["#2"]},
            {"title": "first", "horizon": "next"},
        ]}),
    );
    assert!(resp.ok, "{resp:?}");
    let rows = store::list(&db.lock(), "p1").unwrap();
    assert_eq!(rows[3].deps, vec!["MCA-104".to_string()]);
}

#[test]
fn a_batch_that_closes_a_loop_is_refused_whole() {
    let db = test_db("p1");
    let resp = propose(
        &db,
        json!({"items": [
            {"title": "one", "horizon": "now", "deps": ["#2"]},
            {"title": "two", "horizon": "now", "deps": ["#1"]},
        ]}),
    );
    assert!(!resp.ok);
    let e = resp.error.unwrap();
    assert!(e.contains("loop"), "{e}");
    assert!(
        e.contains("#1 → #2 → #1"),
        "the refusal spells the loop: {e}"
    );
    assert!(store::list(&db.lock(), "p1").unwrap().is_empty());
}

#[test]
fn a_batch_item_waiting_on_an_existing_loop_is_refused() {
    let db = test_db("p1");
    assert!(propose(&db, one_item("a")).ok); // MCA-100
    assert!(propose(&db, one_item("b")).ok); // MCA-101
    {
        let conn = db.lock();
        let rows = store::list(&conn, "p1").unwrap();
        for (row, dep) in [(&rows[0], "MCA-101"), (&rows[1], "MCA-100")] {
            store::update(
                &conn,
                &row.id,
                &ItemPatch {
                    deps: Some(vec![dep.to_string()]),
                    ..Default::default()
                },
            )
            .unwrap();
        }
    }

    let resp = propose(
        &db,
        json!({"items": [{"title": "after", "horizon": "now", "deps": ["MCA-100"]}]}),
    );
    assert!(!resp.ok);
    let e = resp.error.unwrap();
    assert!(e.contains("MCA-100 → MCA-101 → MCA-100"), "{e}");
    assert_eq!(store::list(&db.lock(), "p1").unwrap().len(), 2);
}

#[test]
fn a_hash_reference_out_of_range_is_refused() {
    let db = test_db("p1");
    for (deps, needle) in [
        (json!(["#3"]), "not an item in this batch"),
        (json!(["#0"]), "not an item in this batch"),
        (json!(["#two"]), "not an item in this batch"),
    ] {
        let resp = propose(
            &db,
            json!({"items": [
                {"title": "one", "horizon": "now", "deps": deps},
                {"title": "two", "horizon": "now"},
            ]}),
        );
        assert!(!resp.ok, "should have been rejected");
        let e = resp.error.unwrap();
        assert!(e.contains(needle), "expected {needle:?} in {e:?}");
        assert!(e.contains("item 1"), "the refusal names the item: {e}");
    }
    assert!(store::list(&db.lock(), "p1").unwrap().is_empty());
}
