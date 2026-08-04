//! Drainer decision tests.
//!
//! Everything the drainer *decides* is a pure function over a snapshot
//! ([`pick_next`], [`unsatisfied_deps`], [`resolve_workflow`], [`settle`],
//! [`build_brief`]), so the rules are tested here without a tokio runtime, a
//! clock, or a database. The tick itself is the thin part: read a snapshot,
//! call these, write the answer back.

use super::*;
use crate::roadmap::types::{Horizon, ItemSource};

/// A queued item at `rank` in the project's priority order. `rank` is what
/// orders the queue (0032), so the tests set it explicitly rather than relying
/// on insertion order; timestamps are irrelevant to every decision here and are
/// left at zero.
fn item(code: &str, rank: f64) -> RoadmapItem {
    RoadmapItem {
        id: format!("id-{code}"),
        project_id: "p1".into(),
        code: code.into(),
        parent_id: None,
        title: format!("do {code}"),
        why: String::new(),
        horizon: Horizon::Next,
        status: ItemStatus::Queued,
        rank,
        area: None,
        source: ItemSource::User,
        accept: Vec::new(),
        deps: Vec::new(),
        agent_id: None,
        workflow_def_id: None,
        run_id: None,
        pr_url: None,
        pr_number: None,
        hold_reason: None,
        held_by: None,
        held_at: None,
        created_at: 0,
        updated_at: 0,
    }
}

fn codes(list: &[&str]) -> HashSet<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

/// The PR a finished run recorded on its row.
fn pr(number: Option<i64>) -> FinalizedPr {
    FinalizedPr {
        url: "https://github.com/o/r/pull/42".into(),
        number,
    }
}

// ───────────────────────────── ordering ─────────────────────────────────

#[test]
fn the_queue_follows_rank() {
    // The slice arrives in the DAO's rank order (`store::list`), which is the
    // order the board draws — so the head is the item the user dragged to the
    // top, not the one that happens to be oldest. Horizon is deliberately not
    // consulted: a `later` item the user queued outranks a `now` item they
    // didn't.
    let mut first = item("FLT-101", 1.0);
    first.horizon = Horizon::Later;
    let mut second = item("FLT-100", 2.0);
    second.horizon = Horizon::Now;
    let queue = vec![first, second];

    assert_eq!(
        pick_next(&queue, 0, &codes(&[]), &codes(&["FLT-100", "FLT-101"])),
        Decision::Dispatch(0)
    );
}

#[test]
fn an_empty_queue_decides_nothing() {
    assert_eq!(pick_next(&[], 0, &codes(&[]), &codes(&[])), Decision::Empty);
}

// ───────────────────────────── what is in the queue ─────────────────────

/// The three rows that are `queued` and still not claimable. Pure over a board
/// snapshot, so the skips are pinned without a database — and so a future edit
/// can't quietly drop one and leave the queue dispatching work it must not.
#[test]
fn a_held_a_handed_off_and_an_unqueued_row_are_not_in_the_queue() {
    let ready = item("FLT-100", 1.0);

    let mut held = item("FLT-101", 2.0);
    held.hold_reason = Some("direction unclear".into());
    held.held_by = Some(EventActor::Pm);
    held.held_at = Some(1);
    assert!(held.is_held());

    let mut handed = item("FLT-102", 3.0);
    handed.agent_id = Some("w1".into());

    let mut open = item("FLT-103", 4.0);
    open.status = ItemStatus::Open;

    let board = vec![ready.clone(), held, handed, open];
    assert_eq!(
        dispatchable(&board)
            .iter()
            .map(|i| i.code.clone())
            .collect::<Vec<_>>(),
        vec![ready.code],
        "only the plain queued row is claimable"
    );
}

/// The one that matters most: a held item is skipped even when it is the head of
/// the queue with every dependency landed — and the ready item behind it still
/// goes. A hold stops one item, not the board (that is the project hold).
#[test]
fn a_held_head_is_skipped_and_the_next_item_still_dispatches() {
    let mut held = item("FLT-100", 1.0);
    held.hold_reason = Some("wrong scope".into());
    let ready = item("FLT-101", 2.0);

    let queue = dispatchable(&[held, ready]);
    assert_eq!(queue.len(), 1);
    assert_eq!(
        pick_next(&queue, 0, &codes(&[]), &codes(&["FLT-100", "FLT-101"])),
        Decision::Dispatch(0),
    );
    assert_eq!(queue[0].code, "FLT-101");
}

// ───────────────────────────── dependencies ─────────────────────────────

#[test]
fn a_done_dependency_lets_an_item_through() {
    let mut it = item("FLT-101", 10.0);
    it.deps = vec!["FLT-100".into()];

    assert_eq!(
        pick_next(
            &[it],
            0,
            &codes(&["FLT-100"]),
            &codes(&["FLT-100", "FLT-101"])
        ),
        Decision::Dispatch(0)
    );
}

#[test]
fn an_in_review_dependency_still_blocks() {
    // `in_review` means the PR is open, not merged: a dependant forked now
    // would build on a tree that doesn't contain the work it depends on.
    let mut it = item("FLT-101", 10.0);
    it.deps = vec!["FLT-100".into()];

    assert_eq!(
        pick_next(
            &[it],
            0,
            // FLT-100 exists but is not done.
            &codes(&[]),
            &codes(&["FLT-100", "FLT-101"])
        ),
        Decision::Blocked {
            item_id: "id-FLT-101".into(),
            waiting_on: vec!["FLT-100".into()],
        }
    );
}

#[test]
fn a_dependency_that_no_longer_exists_counts_as_satisfied() {
    // The item it pointed at was deleted off the board, and a deleted item
    // never ships — waiting for it would block this one forever.
    let mut it = item("FLT-101", 10.0);
    it.deps = vec!["FLT-100".into()];

    assert_eq!(
        pick_next(&[it], 0, &codes(&[]), &codes(&["FLT-101"])),
        Decision::Dispatch(0)
    );
}

#[test]
fn a_blocked_head_does_not_block_the_rest_of_the_queue() {
    // Skipped, never failed: FLT-100's turn comes when its dep lands.
    let mut blocked = item("FLT-100", 10.0);
    blocked.deps = vec!["FLT-099".into()];
    let ready = item("FLT-101", 20.0);

    assert_eq!(
        pick_next(
            &[blocked, ready],
            0,
            &codes(&[]),
            &codes(&["FLT-099", "FLT-100", "FLT-101"])
        ),
        Decision::Dispatch(1)
    );
}

#[test]
fn an_all_blocked_queue_reports_the_head_and_what_it_waits_on() {
    let mut head = item("FLT-100", 10.0);
    head.deps = vec!["FLT-098".into(), "FLT-099".into()];
    let mut tail = item("FLT-101", 20.0);
    tail.deps = vec!["FLT-100".into()];

    let known = codes(&["FLT-098", "FLT-099", "FLT-100", "FLT-101"]);
    // FLT-098 landed; FLT-099 hasn't.
    assert_eq!(
        pick_next(&[head, tail], 0, &codes(&["FLT-098"]), &known),
        Decision::Blocked {
            item_id: "id-FLT-100".into(),
            waiting_on: vec!["FLT-099".into()],
        }
    );
}

#[test]
fn unsatisfied_deps_reports_only_the_live_unlanded_ones() {
    let deps = vec![
        "FLT-100".to_string(), // done
        "FLT-101".to_string(), // exists, not done
        "FLT-102".to_string(), // deleted
    ];
    assert_eq!(
        unsatisfied_deps(&deps, &codes(&["FLT-100"]), &codes(&["FLT-100", "FLT-101"])),
        vec!["FLT-101".to_string()]
    );
}

// ───────────────────────────── concurrency ──────────────────────────────

#[test]
fn the_cap_holds_the_queue_even_with_a_ready_item() {
    let ready = item("FLT-100", 10.0);
    assert_eq!(
        pick_next(
            &[ready],
            MAX_CONCURRENT_ROADMAP_RUNS,
            &codes(&[]),
            &codes(&["FLT-100"])
        ),
        Decision::AtCapacity
    );
}

#[test]
fn capacity_is_checked_before_dependencies() {
    // An at-capacity project says so rather than reporting a dep block the user
    // can't act on — the item may well be unblocked by the time a slot frees.
    let mut blocked = item("FLT-100", 10.0);
    blocked.deps = vec!["FLT-099".into()];
    assert_eq!(
        pick_next(
            &[blocked],
            MAX_CONCURRENT_ROADMAP_RUNS,
            &codes(&[]),
            &codes(&["FLT-099", "FLT-100"])
        ),
        Decision::AtCapacity
    );
}

// ───────────────────────────── workflow choice ──────────────────────────

#[test]
fn an_items_own_workflow_wins_over_the_project_default() {
    let mut it = item("FLT-100", 10.0);
    it.workflow_def_id = Some("wf-item".into());
    assert_eq!(
        resolve_workflow(&it, Some("wf-default")),
        Some("wf-item".to_string())
    );
}

#[test]
fn an_item_without_one_falls_back_to_the_project_default() {
    let it = item("FLT-100", 10.0);
    assert_eq!(
        resolve_workflow(&it, Some("wf-default")),
        Some("wf-default".to_string())
    );
}

#[test]
fn no_workflow_anywhere_resolves_to_nothing() {
    // Explicitly not a hardcoded fallback spec: inventing a workflow for work
    // the user queued is worse than asking them to pick one.
    assert_eq!(resolve_workflow(&item("FLT-100", 10.0), None), None);
}

// ───────────────────────────── settlement ───────────────────────────────

#[test]
fn a_live_run_leaves_its_item_alone() {
    for status in [RunStatus::Pending, RunStatus::Running, RunStatus::Paused] {
        assert_eq!(settle(Some(status), None), Settlement::Running);
    }
}

#[test]
fn a_finished_run_with_a_pr_lands_in_review() {
    // `merge_sweep` is what takes it from here to `done`, once GitHub says the
    // PR merged.
    assert_eq!(
        settle(Some(RunStatus::Done), Some(&pr(Some(42)))),
        Settlement::InReview
    );
}

#[test]
fn a_pr_whose_number_never_landed_still_reaches_review() {
    // The URL is what makes it reviewable; the number is what makes it
    // *pollable*. Without one the item sits in review until a human moves it,
    // which is better than settling work that has an open PR straight to done.
    assert_eq!(
        settle(Some(RunStatus::Done), Some(&pr(None))),
        Settlement::InReview
    );
}

#[test]
fn a_finished_run_with_no_pr_is_simply_done() {
    // A workflow with `open_pr: false` finished the work; nothing else is
    // coming, so there is nothing to review.
    assert_eq!(settle(Some(RunStatus::Done), None), Settlement::Done);
}

#[test]
fn a_lost_run_releases_its_item_back_to_the_board() {
    // Never back to `queued`: an auto-retry loop on a failing workflow burns
    // tokens all night. Re-queueing is the user's call, once they know why.
    assert_eq!(
        settle(Some(RunStatus::Failed), None),
        Settlement::Released("its run failed")
    );
    assert_eq!(
        settle(Some(RunStatus::Canceled), None),
        Settlement::Released("its run was canceled")
    );
    assert_eq!(
        settle(None, None),
        Settlement::Released("its run was deleted")
    );
}

// ───────────────────────────── crash recovery ───────────────────────────

/// A migrated in-memory DB, as the app opens the real file.
fn test_conn() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    crate::database::get_migrations()
        .to_latest(&mut conn)
        .unwrap();
    conn
}

/// A roadmap-dispatched run for `item_id`, in `status`, created at `created_at`.
fn run(conn: &Connection, id: &str, item_id: &str, status: &str, created_at: i64) {
    conn.execute(
        "INSERT INTO wf_run (id, name, spec_json, task, project_id, repo_path, run_dir, branch,
                             base_sha, status, budgets_json, spent_json, created_at, updated_at,
                             roadmap_item_id)
         VALUES (?1, 'n', '{}', 't', 'p1', '/r', '/d', 'wf/x', 'sha', ?2, '{}', '{}', ?3, ?3, ?4)",
        rusqlite::params![id, status, created_at, item_id],
    )
    .unwrap();
}

#[test]
fn recovery_adopts_only_a_live_run() {
    // The item is `active` with `run_id` NULL — a claim whose launch never wrote
    // back. The newest back-linked run is a *terminal* one from a previous
    // cycle: adopting it would settle this claim against last cycle's outcome
    // (resurrecting a merged-or-dead PR as `in_review`). Only the live run may
    // be adopted.
    let conn = test_conn();
    run(&conn, "old-done", "id-FLT-100", "done", 10);
    run(&conn, "live", "id-FLT-100", "running", 20);
    assert_eq!(
        dispatched_run_id(&conn, "id-FLT-100"),
        Some("live".to_string())
    );

    // Newest is terminal and nothing live remains: the claim's run never
    // started, which is the release path — not an adoption.
    run(&conn, "newest-failed", "id-FLT-101", "failed", 30);
    run(&conn, "older-canceled", "id-FLT-101", "canceled", 20);
    assert_eq!(dispatched_run_id(&conn, "id-FLT-101"), None);

    // No back-link at all is the same answer.
    assert_eq!(dispatched_run_id(&conn, "id-FLT-999"), None);
}

// ───────────────────────────── durable history ──────────────────────────

/// A project row for the FK, plus one roadmap item in `status`.
fn db_item(conn: &Connection, status: ItemStatus) -> RoadmapItem {
    conn.execute(
        "INSERT OR IGNORE INTO projects (id, name, created_at) VALUES ('p1', 'fletch', 0)",
        [],
    )
    .unwrap();
    store::create(
        conn,
        "p1",
        &crate::roadmap::types::NewItem {
            title: "it".into(),
            status: Some(status),
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn each_settlement_names_its_event() {
    let with_pr = pr(Some(42));
    assert_eq!(settlement_event(&Settlement::Running, None), None);
    assert_eq!(
        settlement_event(&Settlement::InReview, Some(&with_pr)),
        Some((EventKind::PrOpened, Some(with_pr.url.clone())))
    );
    assert_eq!(
        settlement_event(&Settlement::Done, None),
        Some((EventKind::Shipped, None))
    );
    // The durable detail is the same reason string the transient note wraps.
    assert_eq!(
        settlement_event(&Settlement::Released("its run failed"), None),
        Some((EventKind::RunFailed, Some("its run failed".to_string())))
    );
}

#[test]
fn a_claim_records_one_dispatched_event_naming_the_workflow() {
    let conn = test_conn();
    let it = db_item(&conn, ItemStatus::Queued);

    let (claimed, event) = claim_item(&conn, &it.id, "wf-1", Some("Build & review"))
        .unwrap()
        .expect("claims");
    assert_eq!(claimed.status, ItemStatus::Active);
    assert_eq!(claimed.workflow_def_id.as_deref(), Some("wf-1"));
    assert_eq!(event.kind, EventKind::Dispatched);
    assert_eq!(event.actor, EventActor::Drainer);
    // The *name*, not the id: this is the most common line in an item's trail,
    // and the id is already pinned on the row as `workflow_def_id`.
    assert_eq!(event.detail.as_deref(), Some("Build & review"));
    assert_eq!(events::list_for_item(&conn, &it.id).unwrap(), vec![event]);

    // A second claim finds the row no longer queued: no write, and no second
    // event pretending there was one.
    assert!(claim_item(&conn, &it.id, "wf-1", Some("Build & review"))
        .unwrap()
        .is_none());
    assert_eq!(events::list_for_item(&conn, &it.id).unwrap().len(), 1);
}

#[test]
fn a_claim_falls_back_to_the_definition_id_when_the_name_is_gone() {
    // A definition renamed away or deleted between the resolve and the claim: a
    // uuid on the card is poor, an unexplained dispatch is worse.
    let conn = test_conn();
    let it = db_item(&conn, ItemStatus::Queued);
    let (_, event) = claim_item(&conn, &it.id, "wf-orphan", None)
        .unwrap()
        .expect("claims");
    assert_eq!(event.detail.as_deref(), Some("wf-orphan"));
}

#[test]
fn a_definition_name_is_read_off_the_row_or_reported_missing() {
    let conn = test_conn();
    conn.execute(
        "INSERT INTO wf_definition (id, name, spec_json, created_at, updated_at)
         VALUES ('wf-1', 'Build & review', '{}', 0, 0), ('wf-blank', '  ', '{}', 0, 0)",
        [],
    )
    .unwrap();
    assert_eq!(
        definition_name(&conn, "wf-1"),
        Some("Build & review".to_string())
    );
    // A blank name is no name — the caller falls back to the id rather than
    // writing an empty detail.
    assert_eq!(definition_name(&conn, "wf-blank"), None);
    assert_eq!(definition_name(&conn, "wf-gone"), None);
}

#[test]
fn a_release_persists_its_reason_where_the_note_never_lands() {
    // The card's toast ("Back on the board — its run failed.") is a transient
    // `roadmap:queue-note` and is stored nowhere. What this pins is where the
    // reason *does* live: exactly one `run_failed` event carrying it, and no
    // `note` event doubling it up — a reload (any fresh read of the table) finds
    // one line, not two saying the same thing in different words.
    let conn = test_conn();
    let it = db_item(&conn, ItemStatus::Active);

    let (row, event) = apply_and_record(
        &conn,
        &it.id,
        None,
        &ItemPatch {
            status: Some(ItemStatus::Open),
            run_id: Some(None),
            ..Default::default()
        },
        EventActor::Drainer,
        EventKind::RunFailed,
        Some("its run failed".to_string()),
    )
    .unwrap()
    .expect("the item is there to release");
    assert_eq!(row.status, ItemStatus::Open);

    let listed = events::list_for_item(&conn, &it.id).unwrap();
    assert_eq!(listed, vec![event]);
    assert_eq!(listed[0].kind, EventKind::RunFailed);
    assert_eq!(listed[0].detail.as_deref(), Some("its run failed"));

    // One event, of that kind, and nothing of kind `note`: the release explains
    // itself once, on the line the card reads as a failure.
    let by_kind = |kind: &str| -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM roadmap_item_events WHERE item_id = ?1 AND kind = ?2",
            rusqlite::params![it.id, kind],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(by_kind("run_failed"), 1);
    assert_eq!(by_kind("note"), 0);
}

#[test]
fn a_conditional_verdict_that_misses_records_nothing() {
    // The sweep's path: the verdict was decided over a network read, and the
    // row moved meanwhile. No patch lands, so no history may claim one did.
    let conn = test_conn();
    let it = db_item(&conn, ItemStatus::Queued);

    let outcome = apply_and_record(
        &conn,
        &it.id,
        Some(ItemStatus::InReview),
        &ItemPatch {
            status: Some(ItemStatus::Done),
            ..Default::default()
        },
        EventActor::Sweep,
        EventKind::Shipped,
        None,
    )
    .unwrap();
    assert!(outcome.is_none());
    assert!(events::list_for_item(&conn, &it.id).unwrap().is_empty());
}

// ───────────────────────────── wedged queues ────────────────────────────

/// Give `item` a dep list, straight through the DAO — the write paths refuse a
/// loop now, so a board that has one is built by hand here.
fn set_deps(conn: &Connection, item: &RoadmapItem, codes: &[&str]) {
    store::update(
        conn,
        &item.id,
        &ItemPatch {
            deps: Some(codes.iter().map(|c| (*c).to_string()).collect()),
            ..Default::default()
        },
    )
    .unwrap();
}

#[test]
fn a_wedged_queue_head_records_one_blocked_event_not_one_per_tick() {
    // Two queued items waiting on each other: neither is ever `done`, so
    // neither is ever dispatched, and the transient note nobody was watching
    // was the only trace. That is the durable line `EventKind::Blocked` exists
    // for — and it must land once, not once every fifteen seconds.
    let conn = test_conn();
    let a = db_item(&conn, ItemStatus::Queued);
    let b = db_item(&conn, ItemStatus::Queued);
    set_deps(&conn, &a, &[&b.code]);
    set_deps(&conn, &b, &[&a.code]);

    let Claim::Note {
        item,
        text,
        recorded,
    } = plan_and_claim(&conn, "p1")
    else {
        panic!("expected a note about the wedged head");
    };
    assert_eq!(item.id, a.id, "the head of the queue is what's wedged");
    assert!(
        text.contains(&format!("{} → {} → {}", a.code, b.code, a.code)),
        "the loop is named, not just the wait: {text}"
    );
    let event = recorded.expect("a blockage that never resolves is durable");
    assert_eq!(event.kind, EventKind::Blocked);
    assert_eq!(event.actor, EventActor::Drainer);
    // One reason string for both channels, as everywhere else in this module.
    assert_eq!(event.detail.as_deref(), Some(text.as_str()));

    // Next tick, same loop: the note may repeat (it's transient), the row may
    // not.
    let Claim::Note { recorded, .. } = plan_and_claim(&conn, "p1") else {
        panic!("expected a note");
    };
    assert!(
        recorded.is_none(),
        "a durable line must not repeat per tick"
    );
    assert_eq!(events::list_for_item(&conn, &a.id).unwrap().len(), 1);
}

#[test]
fn ordinary_dep_waiting_stays_transient() {
    // The dependency is real work that hasn't landed yet — it will, and then
    // this item dispatches. Nothing durable, or every queue would grow a
    // history of "still waiting" lines it re-derives every tick anyway.
    let conn = test_conn();
    let dep = db_item(&conn, ItemStatus::Open);
    let waiting = db_item(&conn, ItemStatus::Queued);
    set_deps(&conn, &waiting, &[&dep.code]);

    let Claim::Note { text, recorded, .. } = plan_and_claim(&conn, "p1") else {
        panic!("expected a note");
    };
    assert_eq!(text, format!("Waiting on {}", dep.code));
    assert!(recorded.is_none());
    assert!(events::list_for_item(&conn, &waiting.id)
        .unwrap()
        .is_empty());
}

// ───────────────────────────── holds ────────────────────────────────────

/// How far this project's tick got, in one word. A queued item with no workflow
/// resolves to a `Note` ("No workflow to run it under…"), which is *past* the
/// queue selection — so it is the honest signal that the item was in the queue,
/// without a `wf_definition` and a repo row to make a real claim possible.
fn reached(conn: &Connection, project_id: &str) -> &'static str {
    match plan_and_claim(conn, project_id) {
        Claim::Nothing => "nothing",
        Claim::Note { .. } => "in the queue",
        Claim::Claimed(..) => "claimed",
    }
}

#[test]
fn a_held_project_dispatches_nothing_and_says_nothing_on_the_cards() {
    // The board-wide brake. Not a note per row: the reason is one banner above
    // the board, and repeating it on five cards would be the same sentence five
    // times. Nothing is claimed and nothing is recorded — the hold row is
    // already the durable record of why.
    let conn = test_conn();
    let ready = db_item(&conn, ItemStatus::Queued);
    assert_eq!(
        reached(&conn, "p1"),
        "in the queue",
        "unheld, it is dispatchable"
    );

    holds::hold_project(&conn, "p1", "re-planning the quarter", EventActor::Pm).unwrap();
    assert_eq!(
        reached(&conn, "p1"),
        "nothing",
        "a held project never reaches its queue at all"
    );
    assert_eq!(
        store::get(&conn, &ready.id).unwrap().unwrap().status,
        ItemStatus::Queued,
        "the item keeps its place in the queue — a hold is not an unqueue"
    );
    assert!(
        events::list_for_item(&conn, &ready.id).unwrap().is_empty(),
        "a board-wide stop is not history about any one item"
    );

    // Released, the same board is dispatchable again: the brake was the only
    // thing in the way.
    assert!(holds::release_project(&conn, "p1").unwrap());
    assert_eq!(reached(&conn, "p1"), "in the queue");
}

#[test]
fn a_held_item_is_never_claimed_through_the_whole_decision() {
    // The pure filter is pinned above; this is the same rule reached through the
    // real read path, so nothing between the snapshot and the claim can put a
    // held row back in play.
    let conn = test_conn();
    let it = db_item(&conn, ItemStatus::Queued);
    assert_eq!(reached(&conn, "p1"), "in the queue");

    holds::hold_item(&conn, &it.id, "confirm the scope first", EventActor::Pm).unwrap();
    assert_eq!(
        reached(&conn, "p1"),
        "nothing",
        "a held item is not a blocked queue — it simply isn't in the queue"
    );
    assert_eq!(
        store::get(&conn, &it.id).unwrap().unwrap().status,
        ItemStatus::Queued
    );
}

// ───────────────────────────── note dedup ───────────────────────────────

#[test]
fn a_note_repeats_when_the_row_moved_and_stays_quiet_when_it_didnt() {
    let mut said: HashMap<String, SaidNote> = HashMap::new();
    let blocked = item("FLT-100", 10.0);
    let note = "Waiting on FLT-099";

    // First time it's said, and then it's silent — a permanently blocked item
    // must not re-emit the same string every tick.
    assert!(record_note(&mut said, &blocked, note));
    assert!(!record_note(&mut said, &blocked, note));

    // Unqueue + re-queue bumps `updated_at` (every write does), and the note is
    // recomputed to the same string. It has to be said again: the user is
    // looking at a card that has no explanation on it any more.
    let requeued = RoadmapItem {
        updated_at: blocked.updated_at + 1,
        ..blocked.clone()
    };
    assert!(record_note(&mut said, &requeued, note));
    assert!(!record_note(&mut said, &requeued, note));

    // A different reason is always news, version or no version.
    assert!(record_note(&mut said, &requeued, "Waiting on FLT-098"));
}

// ───────────────────────────── the brief ────────────────────────────────

#[test]
fn the_brief_carries_the_item_its_criteria_and_its_ancestry() {
    let mut it = item("FLT-142", 10.0);
    it.title = "Persist worktree state across restarts".into();
    it.why = "A hard quit loses every checkout binding.".into();
    it.accept = vec!["survives a quit".into(), "orphans are offered".into()];
    it.deps = vec!["FLT-140".into()];

    let mut dep = item("FLT-140", 5.0);
    dep.title = "Worktree registry".into();
    dep.status = ItemStatus::Done;

    let brief = build_brief(&it, &[&dep]);

    assert!(brief.starts_with("FLT-142: Persist worktree state across restarts"));
    assert!(brief.contains("A hard quit loses every checkout binding."));
    // Acceptance criteria arrive as a checklist the run can tick through.
    assert!(brief.contains("Done when:"));
    assert!(brief.contains("- [ ] survives a quit"));
    assert!(brief.contains("- [ ] orphans are offered"));
    // What already landed underneath it — context a human in a chat would have
    // and a non-interactive run wouldn't.
    assert!(brief.contains("- FLT-140: Worktree registry (done)"));
    // And the string that lets the board find its way back to the work.
    assert!(brief.contains("[FLT-142]"));
    assert!(brief.contains("pull request title"));
}

#[test]
fn a_bare_item_still_produces_a_usable_brief() {
    // No why, no criteria, no deps: the run gets the title and the tracking
    // instruction, and no empty headings pretending there's more.
    let brief = build_brief(&item("FLT-100", 10.0), &[]);
    assert!(brief.starts_with("FLT-100: do FLT-100"));
    assert!(!brief.contains("Done when:"));
    assert!(!brief.contains("Builds on"));
    assert!(brief.contains("[FLT-100]"));
}
