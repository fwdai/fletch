use super::*;
use crate::rpc::roadmap::deltas::proposable;
use crate::rpc::roadmap::test_support::*;
use crate::rpc::RpcDispatcher;

use serde_json::{json, Value};

use crate::roadmap::brakes;
use crate::roadmap::events::{self, EventActor, EventKind};
use crate::roadmap::store;
use crate::roadmap::types::{ItemStatus, NewItem};

#[test]
fn hold_stops_one_item_and_advances_nothing() {
    let db = test_db("p1");
    assert!(propose(&db, one_item("target")).ok); // MCA-100
    let before = store::list(&db.lock(), "p1").unwrap();

    let (resp, held) = hold(
        &db,
        json!({"scope": "MCA-100", "reason": "the run is building the case this ticket scoped out"}),
    );
    assert!(resp.ok, "{resp:?}");
    let out: Value = serde_json::from_str(&resp.stdout.unwrap()).unwrap();
    assert_eq!(out["held"]["scope"], "MCA-100");

    let Some(Held::Item(item, event)) = held else {
        panic!("an item hold announces the row and its line");
    };
    assert_eq!(
        item.hold_reason.as_deref(),
        Some("the run is building the case this ticket scoped out")
    );
    assert_eq!(item.held_by, Some(EventActor::Pm));
    assert_eq!(item.status, before[0].status);
    assert_eq!(item.rank, before[0].rank);
    assert_eq!(item.title, before[0].title);
    assert_eq!(event.kind, EventKind::Held);
    assert_eq!(event.actor, EventActor::Pm);
    assert_eq!(event.detail.as_deref(), item.hold_reason.as_deref());

    let resp = list(&db, Value::Null);
    let rows: Vec<Value> = board_rows(&resp);
    assert_eq!(
        rows[0]["held"]["reason"],
        "the run is building the case this ticket scoped out"
    );
    assert_eq!(rows[0]["held"]["by"], "pm");
    assert_eq!(rows[0]["last_event"]["kind"], "held");
}

#[test]
fn hold_project_stops_the_whole_board() {
    let db = test_db("p1");
    assert!(propose(&db, one_item("target")).ok); // MCA-100

    let (resp, held) = hold(
        &db,
        json!({"scope": "project", "reason": "the same workflow has failed the same way twice"}),
    );
    assert!(resp.ok, "{resp:?}");
    let out: Value = serde_json::from_str(&resp.stdout.unwrap()).unwrap();
    assert_eq!(out["held"]["scope"], "project");

    let Some(Held::Project(stored)) = held else {
        panic!("a project hold announces its row");
    };
    assert_eq!(
        stored.reason,
        "the same workflow has failed the same way twice"
    );
    assert_eq!(stored.held_by, EventActor::Pm);
    assert_eq!(brakes::get_project(&db.lock(), "p1").unwrap(), Some(stored));

    let rows = store::list(&db.lock(), "p1").unwrap();
    assert!(rows.iter().all(|i| !i.is_held()));
    let trail = events::list_for_item(&db.lock(), &rows[0].id).unwrap();
    assert!(trail.iter().all(|e| e.kind == EventKind::Proposed));
}

#[test]
fn hold_rejects_bad_asks_precisely() {
    let db = test_db("p1");
    assert!(propose(&db, one_item("target")).ok); // MCA-100
    let long = "x".repeat(brakes::MAX_REASON + 1);

    for (args, needle) in [
        (json!({"scope": "MCA-777", "reason": "why"}), "no item"),
        (json!({"scope": "MCA-777", "reason": "why"}), "\"project\""),
        (
            json!({"scope": "MCA-100", "reason": "  "}),
            "`reason` is required",
        ),
        (json!({"scope": "MCA-100"}), "`reason` is required"),
        (
            json!({"scope": "  ", "reason": "why"}),
            "`scope` is required",
        ),
        (
            json!({"scope": "MCA-100", "reason": long.clone()}),
            "keep it under",
        ),
        (
            json!({"scope": "MCA-100", "reason": "why", "status": "done"}),
            "unknown field",
        ),
        (json!({"reason": "no scope"}), "missing field `scope`"),
    ] {
        let (resp, held) = hold(&db, args);
        assert!(!resp.ok, "should have been rejected");
        let e = resp.error.unwrap();
        assert!(e.contains(needle), "expected {needle:?} in {e:?}");
        assert!(held.is_none());
    }
    assert!(!hold(&db, Value::Null).0.ok);
    let rows = store::list(&db.lock(), "p1").unwrap();
    assert!(rows.iter().all(|i| !i.is_held()));
    assert!(brakes::get_project(&db.lock(), "p1").unwrap().is_none());

    let (resp, _) = hold(
        &db,
        json!({"scope": "MCA-100", "reason": "y".repeat(brakes::MAX_REASON)}),
    );
    assert!(resp.ok, "{resp:?}");
}

#[test]
fn a_rejected_item_cannot_be_held() {
    let db = test_db("p1");
    assert!(propose(&db, one_item("target")).ok); // MCA-100
    {
        let conn = db.lock();
        let item = &store::list(&conn, "p1").unwrap()[0];
        store::reject(&conn, &item.id, "not needed").unwrap();
    }

    let (resp, held) = hold(&db, json!({"scope": "MCA-100", "reason": "wait"}));
    assert!(!resp.ok);
    let e = resp.error.unwrap();
    assert!(e.contains("was rejected"), "{e}");
    assert!(e.contains("reopen"), "the refusal names the way out: {e}");
    assert!(held.is_none());
    let rows = store::list(&db.lock(), "p1").unwrap();
    assert!(!rows[0].is_held(), "nothing written");
}

#[test]
fn holding_an_already_held_scope_replaces_the_reason() {
    let db = test_db("p1");
    assert!(propose(&db, one_item("target")).ok); // MCA-100

    assert!(
        hold(&db, json!({"scope": "MCA-100", "reason": "first"}))
            .0
            .ok
    );
    let (resp, held) = hold(&db, json!({"scope": "MCA-100", "reason": "second"}));
    assert!(resp.ok, "{resp:?}");
    let Some(Held::Item(item, _)) = held else {
        panic!("expected an item hold");
    };
    assert_eq!(item.hold_reason.as_deref(), Some("second"));
    let trail = events::list_for_item(&db.lock(), &item.id).unwrap();
    let held_lines: Vec<&str> = trail
        .iter()
        .filter(|e| e.kind == EventKind::Held)
        .filter_map(|e| e.detail.as_deref())
        .collect();
    assert_eq!(held_lines, vec!["second", "first"]);

    assert!(hold(&db, json!({"scope": "project", "reason": "one"})).0.ok);
    assert!(hold(&db, json!({"scope": "project", "reason": "two"})).0.ok);
    let stored = brakes::get_project(&db.lock(), "p1").unwrap().unwrap();
    assert_eq!(stored.reason, "two", "one hold per board");
}

#[test]
fn hold_lands_on_items_no_proposal_could_touch() {
    let db = test_db("p1");
    for status in [ItemStatus::Active, ItemStatus::InReview, ItemStatus::Done] {
        let it = {
            let conn = db.lock();
            store::create(
                &conn,
                "p1",
                &NewItem {
                    title: "in flight".into(),
                    status: Some(status),
                    ..Default::default()
                },
            )
            .unwrap()
        };
        let (resp, held) = hold(
            &db,
            json!({"scope": it.code, "reason": "this answers a narrower question than agreed"}),
        );
        assert!(
            resp.ok,
            "a hold on {} must be allowed: {resp:?}",
            status.as_str()
        );
        let Some(Held::Item(item, _)) = held else {
            panic!("expected an item hold");
        };
        assert!(item.is_held());
        let items = store::list(&db.lock(), "p1").unwrap();
        assert!(proposable(&items, &it.code).is_err());
    }
}

#[tokio::test]
async fn hold_routes_through_the_dispatcher_and_release_does_not_exist() {
    let db = test_db("p1");
    let d = dispatcher(&db, "p1");
    assert!(propose(&db, one_item("target")).ok); // MCA-100

    let resp = d
        .dispatch(
            "r1",
            "roadmap_hold",
            &json!({"scope": "MCA-100", "reason": "confirm the direction first"}),
        )
        .await
        .0;
    assert!(resp.ok, "{resp:?}");
    let rows = store::list(&db.lock(), "p1").unwrap();
    assert_eq!(
        rows[0].hold_reason.as_deref(),
        Some("confirm the direction first")
    );

    let resp = d
        .dispatch("r2", "roadmap_release", &json!({"scope": "MCA-100"}))
        .await
        .0;
    assert!(!resp.ok);
    let e = resp.error.unwrap();
    assert!(e.contains("unknown roadmap op"), "{e}");
    assert!(e.contains("roadmap_hold"), "{e}");
    assert!(!e.contains("roadmap_release,"), "{e}");
    let rows = store::list(&db.lock(), "p1").unwrap();
    assert!(rows[0].is_held());
}
