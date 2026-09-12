use super::*;
use crate::rpc::roadmap::deltas::proposable;
use crate::rpc::roadmap::test_support::*;
use crate::rpc::RpcDispatcher;

use serde_json::{json, Value};

use crate::roadmap::events::{self, EventActor, EventKind};
use crate::roadmap::store;
use crate::roadmap::types::{ItemStatus, NewItem};

#[test]
fn note_records_an_observation_and_advances_nothing() {
    let db = test_db("p1");
    assert!(propose(&db, one_item("target")).ok); // MCA-100
    let before = store::list(&db.lock(), "p1").unwrap();

    let (resp, event) = note(
        &db,
        json!({"code": "MCA-100", "note": "the run solved a narrower problem than this asked for"}),
    );
    assert!(resp.ok, "{resp:?}");
    let out: Value = serde_json::from_str(&resp.stdout.unwrap()).unwrap();
    assert_eq!(out["noted"]["code"], "MCA-100");

    let event = event.expect("the note is returned so the card can hear about it");
    assert_eq!(event.kind, EventKind::Note);
    assert_eq!(event.actor, EventActor::Pm);
    assert_eq!(
        event.detail.as_deref(),
        Some("the run solved a narrower problem than this asked for")
    );
    assert_eq!(event.item_id, before[0].id);
    assert_eq!(event.project_id, "p1");

    assert_eq!(store::list(&db.lock(), "p1").unwrap(), before);
    let trail = events::list_for_item(&db.lock(), &before[0].id).unwrap();
    assert_eq!(trail.len(), 2);
    assert_eq!(trail[0], event);
}

#[test]
fn note_lands_on_items_no_proposal_could_touch() {
    let db = test_db("p1");
    for status in [ItemStatus::Active, ItemStatus::InReview, ItemStatus::Done] {
        let it = {
            let conn = db.lock();
            store::create(
                &conn,
                "p1",
                &NewItem {
                    title: "shipped something".into(),
                    status: Some(status),
                    ..Default::default()
                },
            )
            .unwrap()
        };
        let (resp, event) = note(
            &db,
            json!({"code": it.code, "note": "narrower than agreed"}),
        );
        assert!(
            resp.ok,
            "a note on {} must be allowed: {resp:?}",
            status.as_str()
        );
        assert_eq!(event.unwrap().item_id, it.id);
        let items = store::list(&db.lock(), "p1").unwrap();
        assert!(proposable(&items, &it.code).is_err());
    }
}

#[test]
fn note_rejects_bad_asks_precisely() {
    let db = test_db("p1");
    assert!(propose(&db, one_item("target")).ok); // MCA-100
    let long = "x".repeat(MAX_NOTE + 1);

    for (args, needle) in [
        (json!({"code": "MCA-777", "note": "hi"}), "no item"),
        (
            json!({"code": "MCA-100", "note": "   "}),
            "`note` is required",
        ),
        (json!({"code": "MCA-100"}), "`note` is required"),
        (
            json!({"code": "MCA-100", "note": long.clone()}),
            "keep it under",
        ),
        (
            json!({"code": "MCA-100", "note": "hi", "status": "done"}),
            "unknown field",
        ),
        (json!({"note": "no code"}), "missing field `code`"),
    ] {
        let (resp, event) = note(&db, args);
        assert!(!resp.ok, "should have been rejected");
        let e = resp.error.unwrap();
        assert!(e.contains(needle), "expected {needle:?} in {e:?}");
        assert!(event.is_none());
    }
    assert!(!note(&db, Value::Null).0.ok);
    let items = store::list(&db.lock(), "p1").unwrap();
    let trail = events::list_for_item(&db.lock(), &items[0].id).unwrap();
    assert!(
        trail.iter().all(|e| e.kind == EventKind::Proposed),
        "{trail:?}"
    );

    let (resp, _) = note(
        &db,
        json!({"code": "MCA-100", "note": "y".repeat(MAX_NOTE)}),
    );
    assert!(resp.ok, "{resp:?}");
}

#[tokio::test]
async fn note_routes_through_the_dispatcher() {
    let db = test_db("p1");
    let d = dispatcher(&db, "p1");
    assert!(propose(&db, one_item("target")).ok); // MCA-100

    let resp = d
        .dispatch(
            "r1",
            "roadmap_note",
            &json!({"code": "MCA-100", "note": "watch the migration on this one"}),
        )
        .await
        .0;
    assert!(resp.ok, "{resp:?}");
    let items = store::list(&db.lock(), "p1").unwrap();
    let trail = events::list_for_item(&db.lock(), &items[0].id).unwrap();
    assert_eq!(trail[0].kind, EventKind::Note);
    assert_eq!(trail[0].actor, EventActor::Pm);
    let resp = list(&db, Value::Null);
    let rows: Vec<Value> = board_rows(&resp);
    assert_eq!(rows[0]["last_event"]["kind"], "note");
    assert_eq!(
        rows[0]["last_event"]["detail"],
        "watch the migration on this one"
    );
}
