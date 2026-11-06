use super::*;
use crate::rpc::roadmap::test_support::*;
use crate::rpc::RpcDispatcher;

use serde_json::{json, Value};

use crate::roadmap::events::{self, EventActor, EventKind};
use crate::roadmap::store;
use crate::roadmap::types::{Horizon, ItemSource};
use crate::roadmap::Db;

fn capture(db: &Db, title: &str) -> RoadmapItem {
    store::create(
        &db.lock(),
        "p1",
        &NewItem {
            title: title.into(),
            status: Some(ItemStatus::Captured),
            ..Default::default()
        },
    )
    .unwrap()
}

fn run(db: &Db, args: Value) -> (Response, Option<Proposed>) {
    let conn = db.lock();
    triage_op(&conn, "p1", "r1", &args)
}

fn out(resp: &Response) -> Value {
    assert!(resp.ok, "{resp:?}");
    serde_json::from_str(resp.stdout.as_ref().unwrap()).unwrap()
}

fn get(db: &Db, id: &str) -> RoadmapItem {
    store::get(&db.lock(), id).unwrap().unwrap()
}

fn trail(db: &Db, id: &str) -> Vec<(EventKind, Option<String>)> {
    events::list_for_item(&db.lock(), id)
        .unwrap()
        .into_iter()
        .rev()
        .map(|e| (e.kind, e.detail))
        .collect()
}

#[tokio::test]
async fn an_ask_turns_the_captured_row_itself_into_a_ghost() {
    let db = test_db("p1");
    let dep = capture(&db, "a dependency");
    {
        let conn = db.lock();
        store::update(
            &conn,
            &dep.id,
            &ItemPatch {
                status: Some(ItemStatus::Open),
                ..Default::default()
            },
        )
        .unwrap();
    }
    let it = capture(&db, "the run panel should remember the port");

    let resp = dispatcher(&db, "p1")
        .dispatch(
            "r1",
            "roadmap_triage",
            &json!({"code": it.code, "kind": "ask", "ticket": {
                "title": "Remember the run panel port per project",
                "why": "it forgets on every relaunch",
                "horizon": "next", "area": "run",
                "accept": ["a relaunch reopens on the last port"],
                "deps": [dep.code],
            }}),
        )
        .await
        .0;
    assert_eq!(
        out(&resp),
        json!({"triaged": {"code": it.code, "kind": "ask"}})
    );

    let row = get(&db, &it.id);
    assert_eq!(row.code, it.code, "same row, same code");
    assert_eq!(row.status, ItemStatus::Proposed);
    assert_eq!(row.kind, Some(CaptureKind::Ask));
    assert_eq!(row.title, "Remember the run panel port per project");
    assert_eq!(row.horizon, Horizon::Next);
    assert_eq!(row.area.as_deref(), Some("run"));
    assert_eq!(row.deps, vec![dep.code.clone()]);
    assert_eq!(row.source, ItemSource::User, "still the user's capture");
    let history = trail(&db, &it.id);
    assert_eq!(history.len(), 1, "captured, then triaged — no create here");
    assert_eq!(
        history[0],
        (EventKind::Proposed, Some("triaged as ask".to_string()))
    );
    let pm = events::latest_for_item(&db.lock(), &it.id)
        .unwrap()
        .unwrap();
    assert_eq!(pm.actor, EventActor::Pm);
}

#[test]
fn an_idea_keeps_the_row_in_the_inbox_with_a_brief_and_linked_candidates() {
    let db = test_db("p1");
    let it = capture(&db, "what if agents could pair on one checkout");

    let (resp, stored) = run(
        &db,
        json!({"code": it.code, "kind": "idea",
               "brief": "  Worth knowing whether the sandbox can share a writer lock.  ",
               "questions": ["Pairing or hand-off?", " ", "Is read-only enough?"],
               "candidates": [{"title": "Spike a shared writer lock", "horizon": "later"}]}),
    );
    let payload = out(&resp);
    assert_eq!(payload["triaged"], json!({"code": it.code, "kind": "idea"}));
    assert_eq!(payload["created"].as_array().unwrap().len(), 1);

    let row = get(&db, &it.id);
    assert_eq!(
        row.status,
        ItemStatus::Captured,
        "an idea stays in the inbox"
    );
    assert_eq!(row.kind, Some(CaptureKind::Idea));
    let triage = row.triage.unwrap();
    assert_eq!(
        triage.brief.as_deref(),
        Some("Worth knowing whether the sandbox can share a writer lock.")
    );
    assert_eq!(
        triage.questions,
        vec!["Pairing or hand-off?", "Is read-only enough?"]
    );
    assert!(!triage.mined);

    let (items, recorded) = stored.unwrap();
    assert_eq!(
        items[0].id, it.id,
        "the captured row leads what is announced"
    );
    let ghost = &items[1];
    assert_eq!(ghost.status, ItemStatus::Proposed);
    assert_eq!(ghost.source, ItemSource::Pm);
    assert_eq!(ghost.from_item.as_deref(), Some(it.id.as_str()));
    assert_eq!(
        trail(&db, &ghost.id),
        vec![(EventKind::Proposed, Some(format!("from {}", it.code)))]
    );
    assert_eq!(
        trail(&db, &it.id),
        vec![(
            EventKind::Note,
            Some(format!(
                "triaged as idea — brief, 2 questions; proposed {}",
                ghost.code
            ))
        )]
    );
    assert_eq!(recorded.len(), 2);
}

#[test]
fn a_brainstorm_is_mined_into_linked_ghosts_and_noted() {
    let db = test_db("p1");
    let it = capture(&db, "thoughts after the demo");

    let (resp, _) = run(
        &db,
        json!({"code": it.code, "kind": "brainstorm", "candidates": [
            {"title": "Show the PR state on the sidebar row", "horizon": "next"},
            {"title": "Draft a release note", "horizon": "later", "deps": ["#1"]},
        ]}),
    );
    let created = out(&resp)["created"].as_array().unwrap().clone();
    let codes: Vec<String> = created
        .iter()
        .map(|c| c["code"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(codes.len(), 2);

    let row = get(&db, &it.id);
    assert_eq!(row.status, ItemStatus::Captured);
    assert_eq!(row.kind, Some(CaptureKind::Brainstorm));
    assert!(row.triage.unwrap().mined);
    assert_eq!(
        trail(&db, &it.id),
        vec![(
            EventKind::Note,
            Some(format!(
                "triaged as brainstorm — mined {}",
                codes.join(", ")
            ))
        )]
    );
    let board = store::list(&db.lock(), "p1").unwrap();
    let second = board.iter().find(|i| i.code == codes[1]).unwrap();
    assert_eq!(
        second.deps,
        vec![codes[0].clone()],
        "#n resolves as in propose"
    );
    assert!(board
        .iter()
        .filter(|i| i.status == ItemStatus::Proposed)
        .all(|i| i.from_item.as_deref() == Some(it.id.as_str())));

    // Discarding the mined row leaves its ghosts, unlinked.
    assert!(store::delete(&db.lock(), &it.id).unwrap());
    let board = store::list(&db.lock(), "p1").unwrap();
    assert_eq!(board.len(), 2);
    assert!(board.iter().all(|i| i.from_item.is_none()));
}

#[test]
fn a_triage_that_does_not_fit_is_refused_and_writes_nothing() {
    let db = test_db("p1");
    let it = capture(&db, "a note");
    let ticket = json!({"title": "t", "horizon": "next"});
    let one = json!([{"title": "c", "horizon": "later"}]);
    for (args, needle) in [
        (json!({"code": it.code, "kind": "chore"}), "unknown kind"),
        (
            json!({"code": "MCA-999", "kind": "ask", "ticket": ticket}),
            "no item",
        ),
        (
            json!({"code": it.code, "kind": "ask"}),
            "`ticket` is required",
        ),
        (
            json!({"code": it.code, "kind": "ask", "ticket": ticket, "brief": "b"}),
            "`brief` doesn't belong to ask",
        ),
        (
            json!({"code": it.code, "kind": "ask", "ticket": {"title": "t"}}),
            "`horizon` is required",
        ),
        (
            json!({"code": it.code, "kind": "ask",
                   "ticket": {"title": "t", "horizon": "next", "status": "open"}}),
            "unknown field",
        ),
        (
            json!({"code": it.code, "kind": "ask",
                   "ticket": {"title": "t", "horizon": "next", "deps": [it.code]}}),
            "itself",
        ),
        (
            json!({"code": it.code, "kind": "idea"}),
            "`brief` is required",
        ),
        (
            json!({"code": it.code, "kind": "idea", "brief": "b",
                   "questions": ["1", "2", "3", "4"]}),
            "too many",
        ),
        (
            json!({"code": it.code, "kind": "idea", "brief": "b", "ticket": ticket}),
            "`ticket` doesn't belong to idea",
        ),
        (
            json!({"code": it.code, "kind": "brainstorm", "candidates": []}),
            "at least one",
        ),
        (
            json!({"code": it.code, "kind": "brainstorm", "candidates": one, "questions": []}),
            "`questions` doesn't belong to brainstorm",
        ),
        (
            json!({"code": it.code, "kind": "brainstorm",
                   "candidates": [{"title": "c", "horizon": "later", "deps": [it.code]}]}),
            "captured note",
        ),
        (
            json!({"code": it.code, "kind": "brainstorm", "candidates": [
                {"title": "a", "horizon": "now", "deps": ["#2"]},
                {"title": "b", "horizon": "now", "deps": ["#1"]},
            ]}),
            "loop",
        ),
        (
            json!({"code": it.code, "kind": "ask", "ticket": ticket, "queue": true}),
            "unknown field",
        ),
    ] {
        let (resp, stored) = run(&db, args.clone());
        assert!(!resp.ok, "should have been refused: {args}");
        assert!(stored.is_none());
        let e = resp.error.unwrap();
        assert!(e.contains(needle), "expected {needle:?} in {e:?}");
    }
    let board = store::list(&db.lock(), "p1").unwrap();
    assert_eq!(board.len(), 1, "no candidate slipped through");
    assert_eq!(board[0].status, ItemStatus::Captured);
    assert_eq!(board[0].kind, None);
    assert!(trail(&db, &it.id).is_empty());
}

#[test]
fn only_a_captured_row_is_triaged_and_an_idea_can_be_triaged_again() {
    let db = test_db("p1");
    let it = capture(&db, "a note");
    assert!(
        run(&db, json!({"code": it.code, "kind": "idea", "brief": "b"}))
            .0
            .ok
    );
    let (resp, _) = run(
        &db,
        json!({"code": it.code, "kind": "ask", "ticket": {"title": "t", "horizon": "later"}}),
    );
    assert!(resp.ok, "the answered idea becomes the ticket: {resp:?}");

    let (resp, _) = run(
        &db,
        json!({"code": it.code, "kind": "ask", "ticket": {"title": "t", "horizon": "later"}}),
    );
    let e = resp.error.unwrap();
    assert!(e.contains("proposed") && e.contains("captured"), "{e}");
}

#[test]
fn the_listing_shows_the_inbox_and_where_a_ghost_came_from() {
    let db = test_db("p1");
    let it = capture(&db, "an idea");
    assert!(
        run(
            &db,
            json!({"code": it.code, "kind": "idea", "brief": "b", "questions": ["q?"],
                   "candidates": [{"title": "slice", "horizon": "later"}]}),
        )
        .0
        .ok
    );
    let rows = board_rows(&list(&db, json!({"status": ["captured", "proposed"]})));
    let inbox = rows.iter().find(|r| r["code"] == json!(it.code)).unwrap();
    assert_eq!(inbox["status"], "captured");
    assert_eq!(inbox["kind"], "idea");
    assert_eq!(inbox["triage"], json!({"brief": "b", "questions": ["q?"]}));
    let ghost = rows.iter().find(|r| r["status"] == "proposed").unwrap();
    assert_eq!(ghost["from"], json!(it.code));
    assert!(inbox.get("from").is_none());
}
