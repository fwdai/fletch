use crate::rpc::roadmap::test_support::*;
use crate::rpc::RpcDispatcher;

use serde_json::{json, Value};

#[test]
fn the_brief_reads_empty_until_one_is_ruled_in() {
    let db = test_db("p1");

    // The empty marker, explicit: "no memory yet" must not look like a
    // failed read, because the PM's next move differs (draft one).
    let resp = brief(&db, Value::Null);
    assert!(resp.ok, "{resp:?}");
    assert_eq!(resp.stdout.unwrap(), r#"{"brief":null}"#);

    // Proposing does not write it — that is the whole gate.
    assert!(
        propose_brief(&db, json!({"content": "# Fletch\n\nSupervised agents."}))
            .0
            .ok
    );
    let resp = brief(&db, Value::Null);
    let out: Value = serde_json::from_str(&resp.stdout.unwrap()).unwrap();
    assert_eq!(out["brief"], Value::Null, "an ask is not the brief");

    // Only the ruling writes memory (the user's typed command, exercised
    // here through the one function behind it).
    {
        let conn = db.lock();
        crate::roadmap::memory::accept(&conn, "p1")
            .unwrap()
            .unwrap();
    }
    let resp = brief(&db, Value::Null);
    let out: Value = serde_json::from_str(&resp.stdout.unwrap()).unwrap();
    assert_eq!(
        out["brief"]["content"],
        json!("# Fletch\n\nSupervised agents.")
    );
    // Written a moment ago, so no age — the same "just now" the item
    // trail's absent age means.
    assert_eq!(out["brief"]["age"], Value::Null);

    // No filter, no section picker: the op takes the whole document.
    let resp = brief(&db, json!({"section": "vision"}));
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("takes no args"));
}

#[test]
fn propose_brief_parks_one_ask_and_refuses_an_unusable_one() {
    let db = test_db("p1");

    let want = "# Fletch\n\n## Rejected\n\n- sprints";
    let (resp, stored) = propose_brief(
        &db,
        json!({"content": format!("  {want}  "), "note": "records the pruning"}),
    );
    assert!(resp.ok, "{resp:?}");
    let out: Value = serde_json::from_str(&resp.stdout.unwrap()).unwrap();
    // The size the app measured, so the PM can see its document didn't grow
    // a paragraph in transit.
    assert_eq!(out["proposed"]["brief"]["bytes"], json!(want.len()));
    let p = stored.unwrap();
    assert_eq!(p.content, want, "trimmed, otherwise verbatim");
    assert_eq!(p.note.as_deref(), Some("records the pruning"));

    // A newer ask replaces it: one pending document per project, so the user
    // rules on the PM's current position.
    let (resp, stored) = propose_brief(&db, json!({"content": "# Fletch\n\nRewritten."}));
    assert!(resp.ok, "{resp:?}");
    assert_eq!(
        crate::roadmap::memory::get_proposal(&db.lock(), "p1").unwrap(),
        stored
    );
    assert_eq!(stored.unwrap().note, None, "the replacement's note wins");

    let over = "x".repeat(crate::roadmap::memory::MAX_CONTENT + 1);
    for (args, needle) in [
        (json!({"content": "   "}), "required"),
        (json!({}), "required"),
        (json!({"content": over}), "keep the brief under"),
        // A misspelled field would otherwise be silently dropped.
        (json!({"content": "fine", "notes": "why"}), "unknown field"),
    ] {
        let (resp, stored) = propose_brief(&db, args);
        assert!(!resp.ok, "should have been rejected");
        let e = resp.error.unwrap();
        assert!(e.contains(needle), "expected {needle:?} in {e:?}");
        assert!(stored.is_none());
    }
    // Args at all are required, and no refusal above disturbed the pending
    // ask.
    assert!(!propose_brief(&db, Value::Null).0.ok);
    assert_eq!(
        crate::roadmap::memory::get_proposal(&db.lock(), "p1")
            .unwrap()
            .unwrap()
            .content,
        "# Fletch\n\nRewritten."
    );
}

#[tokio::test]
async fn the_brief_ops_route_through_the_dispatcher() {
    let db = test_db("p1");
    let d = dispatcher(&db, "p1");

    let resp = d
        .dispatch(
            "r1",
            "roadmap_propose_brief_update",
            &json!({"content": "# Fletch"}),
        )
        .await
        .0;
    assert!(resp.ok, "{resp:?}");
    let resp = d.dispatch("r2", "roadmap_brief", &Value::Null).await.0;
    assert!(resp.ok, "{resp:?}");
    assert_eq!(resp.stdout.unwrap(), r#"{"brief":null}"#);
}
