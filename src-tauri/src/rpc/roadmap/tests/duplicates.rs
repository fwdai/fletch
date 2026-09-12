use super::*;
use crate::rpc::roadmap::test_support::*;

use serde_json::Value;

use crate::roadmap::store;

#[test]
fn a_near_duplicate_title_warns_but_still_lands() {
    let db = test_db("p1");
    assert!(propose(&db, one_item("Add dark mode support")).ok); // MCA-100
    {
        let conn = db.lock();
        let item = &store::list(&conn, "p1").unwrap()[0];
        store::reject(&conn, &item.id, "theming is out of scope").unwrap();
    }

    let resp = propose(&db, one_item("Dark mode support"));
    assert!(resp.ok, "{resp:?}");
    let payload: Value = serde_json::from_str(resp.stdout.as_ref().unwrap()).unwrap();
    assert!(
        payload.get("created").is_some(),
        "a warning is advice, not a refusal"
    );
    let warnings = payload["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1);
    let w = warnings[0].as_str().unwrap();
    assert!(w.contains("MCA-100"), "{w}");
    assert!(w.contains("REJECTED"), "{w}");
    assert!(w.contains("theming is out of scope"), "{w}");

    let quiet = propose(&db, one_item("Queue drainer"));
    let payload: Value = serde_json::from_str(quiet.stdout.as_ref().unwrap()).unwrap();
    assert!(payload.get("warnings").is_none());
}

#[test]
fn similar_titles_is_containment_with_a_two_word_floor() {
    assert!(similar_titles("Add dark mode support", "Dark mode"));
    assert!(similar_titles("The drainer of the queue", "queue drainer"));
    assert!(!similar_titles("Dark mode", "Light mode"));
    assert!(!similar_titles("drainer", "drainer"));
    assert!(!similar_titles(
        "Persist worktree state across restarts",
        "Persist the user's editor theme and window state"
    ));
}
