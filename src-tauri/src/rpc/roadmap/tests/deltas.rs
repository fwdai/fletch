use crate::rpc::roadmap::test_support::*;

use serde_json::{json, Value};

use crate::roadmap::events::{self, EventKind};
use crate::roadmap::proposals::{self, ProposalKind};
use crate::roadmap::store;
use crate::roadmap::types::{ItemStatus, NewItem};

/// The urgent one (see .context/roadmap-pm-plan.md, A4): a dep patch that
/// closes a loop is refused at propose time, so the user is never offered a
/// diff whose acceptance would wedge the queue.
#[test]
fn propose_update_refuses_a_dep_patch_that_closes_a_loop() {
    let db = test_db("p1");
    assert!(propose(&db, one_item("first")).ok); // MCA-100
    assert!(
        propose(
            &db,
            json!({"items": [{"title": "second", "horizon": "next", "deps": ["MCA-100"]}]})
        )
        .ok
    ); // MCA-101, after MCA-100

    let (resp, stored) = propose_update(
        &db,
        json!({"code": "MCA-100", "patch": {"deps": ["MCA-101"]},
               "note": "actually the other way round"}),
    );
    assert!(!resp.ok);
    let e = resp.error.unwrap();
    assert!(e.contains("MCA-100 → MCA-101 → MCA-100"), "{e}");
    assert!(stored.is_none(), "a refused ask is not parked");
    assert!(proposals::list_for_project(&db.lock(), "p1")
        .unwrap()
        .is_empty());
}

#[test]
fn propose_update_parks_a_delta_and_the_listing_shows_it() {
    let db = test_db("p1");
    assert!(propose(&db, one_item("target")).ok); // MCA-100
    assert!(propose(&db, one_item("dep")).ok); // MCA-101

    let (resp, stored) = propose_update(
        &db,
        json!({"code": "MCA-100", "note": "scope grew",
               "patch": {"title": "Retitled", "deps": ["MCA-101"]}}),
    );
    assert!(resp.ok, "{resp:?}");
    let out: Value = serde_json::from_str(&resp.stdout.unwrap()).unwrap();
    // The response quotes what was asked, so the PM can say it in the chat.
    assert_eq!(out["proposed"]["code"], "MCA-100");
    assert_eq!(out["proposed"]["fields"], json!(["title", "deps"]));

    // Parked, not applied: the row is untouched until the user rules.
    let rows = store::list(&db.lock(), "p1").unwrap();
    assert_eq!(rows[0].title, "target");
    let p = stored.unwrap();
    assert_eq!(p.kind, ProposalKind::Update);
    assert_eq!(p.note.as_deref(), Some("scope grew"));
    // And no history either — the ruling writes history, not the ask.
    assert!(events::list_for_item(&db.lock(), &rows[0].id)
        .unwrap()
        .iter()
        .all(|e| e.kind == EventKind::Proposed));

    // The compact listing carries the pending ask, so the PM never
    // re-proposes blind.
    let resp = list(&db, Value::Null);
    let listed: Vec<Value> = board_rows(&resp);
    let pp = &listed[0]["pending_proposal"];
    assert_eq!(pp["kind"], "update");
    assert_eq!(pp["note"], "scope grew");
    assert_eq!(pp["fields"], json!(["title", "deps"]));
    assert!(listed[1].get("pending_proposal").is_none());
}

#[test]
fn propose_update_rejects_bad_asks_precisely() {
    let db = test_db("p1");
    assert!(propose(&db, one_item("target")).ok); // MCA-100
    {
        // An item already being built — not reshapeable by proposal.
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
        .unwrap(); // MCA-101
    }

    for (args, needle) in [
        // The lifecycle is not the PM's to move, even by proposal.
        (
            json!({"code": "MCA-100", "patch": {"status": "open"}}),
            "unknown field",
        ),
        (
            json!({"code": "MCA-100", "patch": {"horizen": "next"}}),
            "unknown field",
        ),
        (
            json!({"code": "MCA-100", "patch": {"horizon": "soon"}}),
            "invalid Horizon",
        ),
        (
            json!({"code": "MCA-100", "patch": {"deps": ["MCA-999"]}}),
            "MCA-999",
        ),
        (
            json!({"code": "MCA-100", "patch": {"deps": ["MCA-100"]}}),
            "depend on itself",
        ),
        (
            json!({"code": "MCA-100", "patch": {"title": "  "}}),
            "cannot be blank",
        ),
        (
            json!({"code": "MCA-100", "patch": {}}),
            "at least one field",
        ),
        (
            json!({"code": "MCA-777", "patch": {"title": "x"}}),
            "no item",
        ),
        // The refusal names the status, so the PM knows why and when.
        (
            json!({"code": "MCA-101", "patch": {"title": "x"}}),
            "MCA-101 is active",
        ),
    ] {
        let (resp, stored) = propose_update(&db, args);
        assert!(!resp.ok, "should have been rejected");
        let e = resp.error.unwrap();
        assert!(e.contains(needle), "expected {needle:?} in {e:?}");
        assert!(stored.is_none());
    }
    // None of the above parked anything.
    assert!(proposals::list_for_project(&db.lock(), "p1")
        .unwrap()
        .is_empty());
}

#[test]
fn a_newer_ask_replaces_the_pending_one() {
    let db = test_db("p1");
    assert!(propose(&db, one_item("target")).ok); // MCA-100

    let (first, _) = propose_update(&db, json!({"code": "MCA-100", "patch": {"title": "first"}}));
    assert!(first.ok);
    let (second, stored) = propose_discard(
        &db,
        json!({"code": "MCA-100", "reason": "superseded by the auth slice"}),
    );
    assert!(second.ok, "{second:?}");

    // One pending ask per item: the discard replaced the retitle.
    let pending = proposals::list_for_project(&db.lock(), "p1").unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0], stored.unwrap());
    assert_eq!(pending[0].kind, ProposalKind::Discard);
    assert_eq!(pending[0].patch, None);
}

#[test]
fn propose_discard_requires_a_reason() {
    let db = test_db("p1");
    assert!(propose(&db, one_item("target")).ok); // MCA-100

    let (resp, stored) = propose_discard(&db, json!({"code": "MCA-100", "reason": "  "}));
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("`reason` is required"));
    assert!(stored.is_none());

    // And args at all, for both ops.
    assert!(!propose_discard(&db, Value::Null).0.ok);
    assert!(!propose_update(&db, Value::Null).0.ok);
}
