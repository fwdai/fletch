use super::*;
use crate::rpc::roadmap::test_support::*;

use serde_json::{json, Value};

use crate::roadmap::events::{self, EventActor, EventKind};
use crate::roadmap::store;
use crate::roadmap::types::{ItemStatus, NewItem};

#[test]
fn list_returns_the_project_board_and_filters_by_status() {
    let db = test_db("p1");
    assert!(propose(&db, one_item("proposed one")).ok);
    {
        let conn = db.lock();
        let done = store::create(
            &conn,
            "p1",
            &NewItem {
                title: "shipped".into(),
                status: Some(ItemStatus::Done),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(done.code, "MCA-101");
    }

    let resp = list(&db, Value::Null);
    assert!(resp.ok, "{resp:?}");
    let rows: Vec<Value> = board_rows(&resp);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["code"], "MCA-100");
    assert_eq!(rows[0]["status"], "proposed");
    assert_eq!(rows[0]["why"], "because");
    assert_eq!(rows[1]["status"], "done");
    assert!(rows[1].get("why").is_none());
    assert!(rows[1].get("area").is_none());

    let resp = list(&db, json!({ "status": ["done"] }));
    let rows: Vec<Value> = board_rows(&resp);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["code"], "MCA-101");

    let resp = list(&db, json!({ "status": ["shipped"] }));
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("unknown status"));
}

#[test]
fn the_listing_carries_each_items_last_event_and_pr() {
    let db = test_db("p1");
    assert!(propose(&db, one_item("proposed one")).ok); // MCA-100
    let failed = {
        let conn = db.lock();
        let it = store::create(
            &conn,
            "p1",
            &NewItem {
                title: "failed one".into(),
                status: Some(ItemStatus::Open),
                ..Default::default()
            },
        )
        .unwrap(); // MCA-101
        events::record(
            &conn,
            &it.id,
            "p1",
            EventActor::Drainer,
            EventKind::RunFailed,
            Some("its run failed"),
        )
        .unwrap();
        it
    };
    {
        let conn = db.lock();
        let it = store::create(
            &conn,
            "p1",
            &NewItem {
                title: "in review".into(),
                status: Some(ItemStatus::InReview),
                ..Default::default()
            },
        )
        .unwrap(); // MCA-102
        store::update(
            &conn,
            &it.id,
            &crate::roadmap::types::ItemPatch {
                pr_url: Some(Some("https://github.com/o/r/pull/7".into())),
                ..Default::default()
            },
        )
        .unwrap();
        events::record(
            &conn,
            &it.id,
            "p1",
            EventActor::Drainer,
            EventKind::PrOpened,
            Some("https://github.com/o/r/pull/7"),
        )
        .unwrap();
    }

    let resp = list(&db, Value::Null);
    let rows: Vec<Value> = board_rows(&resp);
    assert_eq!(rows[0]["last_event"]["kind"], "proposed");
    assert!(rows[0]["last_event"].get("detail").is_none());
    assert!(rows[0]["last_event"].get("age").is_none());
    assert!(rows[0].get("pr").is_none());

    assert_eq!(rows[1]["code"], failed.code);
    assert_eq!(rows[1]["last_event"]["kind"], "run_failed");
    assert_eq!(rows[1]["last_event"]["detail"], "its run failed");

    assert_eq!(rows[2]["last_event"]["kind"], "pr_opened");
    assert_eq!(rows[2]["pr"]["url"], "https://github.com/o/r/pull/7");
    assert!(rows[2]["pr"].get("number").is_none());

    {
        let conn = db.lock();
        store::create(
            &conn,
            "p1",
            &NewItem {
                title: "silent".into(),
                ..Default::default()
            },
        )
        .unwrap();
    }
    let resp = list(&db, Value::Null);
    let rows: Vec<Value> = board_rows(&resp);
    assert!(rows[3].get("last_event").is_none());
}

#[test]
fn the_age_of_an_event_reads_in_the_coarsest_true_unit() {
    let now = 1_000_000_000_000;
    let min = 60_000;
    for (ago, expected) in [
        (0, None),
        (min - 1, None),
        (min, Some("1m")),
        (59 * min, Some("59m")),
        (60 * min, Some("1h")),
        (23 * 60 * min + 59 * min, Some("23h")),
        (24 * 60 * min, Some("1d")),
        (9 * 24 * 60 * min, Some("9d")),
    ] {
        assert_eq!(age(now, now - ago).as_deref(), expected, "{ago}ms ago");
    }
    assert_eq!(age(now, now + 5 * min), None);
}

#[test]
fn another_projects_board_is_invisible() {
    let db = test_db("p1");
    {
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, created_at) VALUES ('p2', 'other', 0)",
            [],
        )
        .unwrap();
        store::create(
            &conn,
            "p2",
            &NewItem {
                title: "theirs".into(),
                ..Default::default()
            },
        )
        .unwrap();
    }
    let resp = list(&db, Value::Null);
    let payload: Value = serde_json::from_str(resp.stdout.as_ref().unwrap()).unwrap();
    assert_eq!(payload["items"], json!([]));
    assert!(payload.get("not_doing").is_none());
}

#[test]
fn the_listing_splits_the_decision_log_from_the_board() {
    let db = test_db("p1");
    assert!(propose(&db, one_item("keep")).ok); // MCA-100
    assert!(propose(&db, one_item("kill")).ok); // MCA-101
    {
        let conn = db.lock();
        let killed = store::list(&conn, "p1")
            .unwrap()
            .into_iter()
            .find(|i| i.title == "kill")
            .unwrap();
        store::reject(&conn, &killed.id, "cadence over ceremony").unwrap();
    }

    let resp = list(&db, Value::Null);
    let payload: Value = serde_json::from_str(resp.stdout.as_ref().unwrap()).unwrap();
    let items = payload["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "the ruled-off row leaves the board half");
    assert_eq!(items[0]["title"], "keep");
    let log = payload["not_doing"].as_array().unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0]["code"], "MCA-101");
    assert_eq!(log[0]["close_reason"], "cadence over ceremony");
    assert!(
        payload.get("not_doing_omitted").is_none(),
        "nothing clipped"
    );

    let refused = list(&db, json!({"status": ["rejected"]}));
    assert!(!refused.ok);
    assert!(refused.error.unwrap().contains("not_doing"));
}
