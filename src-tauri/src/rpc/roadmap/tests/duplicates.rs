use super::*;
use crate::rpc::roadmap::test_support::*;

use serde_json::Value;

use crate::roadmap::store;

/// A near-duplicate title lands anyway — the ruling is the gate — but the
/// response says so, and a match on a *rejected* item quotes the reason, so
/// the PM can surface the old decision instead of re-litigating it blind.
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

    // A title sharing nothing warns about nothing.
    let quiet = propose(&db, one_item("Queue drainer"));
    let payload: Value = serde_json::from_str(quiet.stdout.as_ref().unwrap()).unwrap();
    assert!(payload.get("warnings").is_none());
}

/// The similarity heuristic, pinned: substantial containment of the
/// smaller title's words, at least two of them, stopwords and case aside.
#[test]
fn similar_titles_is_containment_with_a_two_word_floor() {
    assert!(similar_titles("Add dark mode support", "Dark mode"));
    assert!(similar_titles("The drainer of the queue", "queue drainer"));
    // One shared word is never enough, however small the title.
    assert!(!similar_titles("Dark mode", "Light mode"));
    assert!(!similar_titles("drainer", "drainer"));
    // Sharing little of the smaller set is not a match.
    assert!(!similar_titles(
        "Persist worktree state across restarts",
        "Persist the user's editor theme and window state"
    ));
}
