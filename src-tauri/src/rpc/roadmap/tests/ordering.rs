use crate::rpc::roadmap::test_support::*;

use serde_json::{json, Value};

use crate::roadmap::order_proposals;
use crate::roadmap::store;
use crate::roadmap::types::{ItemStatus, NewItem};

#[test]
fn propose_order_parks_the_whole_sequence() {
    let db = test_db("p1");
    assert!(propose(&db, one_item("first")).ok); // MCA-100
    assert!(propose(&db, one_item("second")).ok); // MCA-101
    assert!(propose(&db, one_item("third")).ok); // MCA-102

    let (resp, stored) = propose_order(
        &db,
        json!({"codes": ["MCA-102", "MCA-100", "MCA-101"], "note": "the dep goes first"}),
    );
    assert!(resp.ok, "{resp:?}");
    let out: Value = serde_json::from_str(&resp.stdout.unwrap()).unwrap();
    assert_eq!(
        out["proposed"]["order"],
        json!(["MCA-102", "MCA-100", "MCA-101"])
    );

    // Parked, not applied: the board's order is untouched until the user
    // rules on it.
    let p = stored.unwrap();
    assert_eq!(p.codes, vec!["MCA-102", "MCA-100", "MCA-101"]);
    assert_eq!(p.note.as_deref(), Some("the dep goes first"));
    let rows = store::list(&db.lock(), "p1").unwrap();
    assert_eq!(
        rows.iter().map(|i| i.code.as_str()).collect::<Vec<_>>(),
        vec!["MCA-100", "MCA-101", "MCA-102"]
    );
}

#[test]
fn propose_order_rejects_anything_but_the_exact_orderable_set() {
    let db = test_db("p1");
    assert!(propose(&db, one_item("first")).ok); // MCA-100
    assert!(propose(&db, one_item("second")).ok); // MCA-101
    {
        // An item already being built: its place in the queue is settled.
        let conn = db.lock();
        store::create(
            &conn,
            "p1",
            &NewItem {
                title: "building".into(),
                status: Some(ItemStatus::Active),
                ..Default::default()
            },
        )
        .unwrap(); // MCA-102
    }

    for (args, needle) in [
        (json!({"codes": []}), "must list every orderable item"),
        // Blank entries are trimmed away, which makes this an empty ask.
        (json!({"codes": ["  "]}), "must list every orderable item"),
        (json!({"codes": ["MCA-100"]}), "MCA-101"),
        (
            json!({"codes": ["MCA-100", "MCA-101", "MCA-999"]}),
            "not an item on this board",
        ),
        (
            json!({"codes": ["MCA-100", "MCA-101", "MCA-102"]}),
            "MCA-102 is active",
        ),
        (
            json!({"codes": ["MCA-100", "MCA-100", "MCA-101"]}),
            "appears twice",
        ),
        // A misspelled field would otherwise be silently dropped.
        (
            json!({"codes": ["MCA-100", "MCA-101"], "notes": "why"}),
            "unknown field",
        ),
    ] {
        let (resp, stored) = propose_order(&db, args);
        assert!(!resp.ok, "should have been rejected");
        let e = resp.error.unwrap();
        assert!(e.contains(needle), "expected {needle:?} in {e:?}");
        assert!(stored.is_none());
    }
    // Args at all are required, and nothing above parked an ask.
    assert!(!propose_order(&db, Value::Null).0.ok);
    assert!(order_proposals::get(&db.lock(), "p1").unwrap().is_none());
}

#[test]
fn a_newer_order_ask_replaces_the_pending_one() {
    let db = test_db("p1");
    assert!(propose(&db, one_item("first")).ok); // MCA-100
    assert!(propose(&db, one_item("second")).ok); // MCA-101

    assert!(
        propose_order(&db, json!({"codes": ["MCA-100", "MCA-101"]}))
            .0
            .ok
    );
    let (resp, stored) = propose_order(
        &db,
        json!({"codes": ["MCA-101", "MCA-100"], "note": "changed my mind"}),
    );
    assert!(resp.ok, "{resp:?}");
    // One pending ask per board — the user rules on the current position.
    assert_eq!(order_proposals::get(&db.lock(), "p1").unwrap(), stored);
    assert_eq!(stored.unwrap().codes, vec!["MCA-101", "MCA-100"]);
}
