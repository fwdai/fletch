use super::*;
use crate::database::get_migrations;
use events::EventKind;
use types::NewItem;

fn test_conn() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    get_migrations().to_latest(&mut conn).unwrap();
    conn.execute(
        "INSERT INTO projects (id, name, created_at) VALUES ('p1', 'fletch', 0)",
        [],
    )
    .unwrap();
    conn
}

fn with_status(conn: &Connection, status: ItemStatus) -> RoadmapItem {
    store::create(
        conn,
        "p1",
        &NewItem {
            title: "it".into(),
            status: Some(status),
            ..Default::default()
        },
    )
    .unwrap()
}

fn test_db(conn: Connection) -> Db {
    Arc::new(Mutex::new(conn))
}

#[test]
fn merging_is_refused_while_a_hold_stands() {
    let conn = test_conn();
    let it = with_status(&conn, ItemStatus::InReview);
    let other = with_status(&conn, ItemStatus::InReview);
    let db = test_db(conn);

    assert!(merge_hold_gate(&db, &it.id).is_ok(), "nothing stops it");
    {
        let conn = db.lock();
        brakes::hold_item(&conn, &it.id, "we agreed something else", EventActor::Pm).unwrap();
    }
    let refused = merge_hold_gate(&db, &it.id).unwrap_err();
    assert!(refused.contains(&it.code), "{refused}");
    assert!(refused.contains("we agreed something else"), "{refused}");
    assert!(refused.contains("Release the hold"), "{refused}");
    assert!(merge_hold_gate(&db, &other.id).is_ok());

    {
        let conn = db.lock();
        brakes::hold_project(&conn, "p1", "re-planning the quarter", EventActor::User).unwrap();
    }
    let board = merge_hold_gate(&db, &other.id).unwrap_err();
    assert!(board.contains("re-planning the quarter"), "{board}");

    assert!(merge_hold_gate(&db, "no-such-item").is_ok());
}

fn status_patch(to: ItemStatus) -> ItemPatch {
    ItemPatch {
        status: Some(to),
        ..Default::default()
    }
}

#[test]
fn creating_an_item_records_created() {
    let conn = test_conn();
    let (item, event) = create_checked(
        &conn,
        "p1",
        &NewItem {
            title: "hand-written".into(),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(event.item_id, item.id);
    assert_eq!(event.project_id, "p1");
    assert_eq!(event.kind, EventKind::Created);
    assert_eq!(event.actor, EventActor::User);
    assert_eq!(event.detail, None);
    assert_eq!(events::list_for_item(&conn, &item.id).unwrap(), vec![event]);
}

#[test]
fn a_failed_create_leaves_no_row_behind() {
    let conn = test_conn();
    assert!(create_checked(
        &conn,
        "no-such-project",
        &NewItem {
            title: "orphan".into(),
            ..Default::default()
        },
    )
    .is_err());
    assert!(store::list(&conn, "no-such-project").unwrap().is_empty());
}

#[test]
fn each_user_transition_writes_exactly_one_event() {
    use ItemStatus::{Done, InReview, Open, Proposed, Queued};
    let conn = test_conn();
    let cases = [
        (Proposed, Open, EventKind::Accepted),
        (Open, Queued, EventKind::Queued),
        (Queued, Open, EventKind::Unqueued),
        (InReview, Done, EventKind::Shipped),
    ];
    for (from, to, kind) in cases {
        let it = with_status(&conn, from);
        let (outcome, event) =
            update_and_record(&conn, &it.id, &status_patch(to), Some(from), false).unwrap();
        assert!(outcome.unwrap().applied);
        let event = event.expect("an applied transition records itself");
        assert_eq!(event.kind, kind);
        assert_eq!(event.actor, EventActor::User);
        assert_eq!(event.detail, None);
        assert_eq!(events::list_for_item(&conn, &it.id).unwrap(), vec![event]);
    }
}

#[test]
fn a_missed_precondition_records_nothing() {
    let conn = test_conn();
    let it = with_status(&conn, ItemStatus::Active);
    let (outcome, event) = update_and_record(
        &conn,
        &it.id,
        &status_patch(ItemStatus::Open),
        Some(ItemStatus::Queued),
        false,
    )
    .unwrap();
    let outcome = outcome.unwrap();
    assert!(!outcome.applied);
    assert_eq!(outcome.item.status, ItemStatus::Active);
    assert!(event.is_none());
    assert!(events::list_for_item(&conn, &it.id).unwrap().is_empty());
}

#[test]
fn a_plain_edit_records_edited() {
    let conn = test_conn();
    let it = with_status(&conn, ItemStatus::Open);
    let patch = ItemPatch {
        title: Some("retitled".into()),
        ..Default::default()
    };
    let (_, event) = update_and_record(&conn, &it.id, &patch, None, false).unwrap();
    assert_eq!(event.unwrap().kind, EventKind::Edited);
}

fn set_autoqueue(conn: &Connection, on: bool) {
    conn.execute(
        "INSERT OR REPLACE INTO project_settings (project_id, key, value)
             VALUES ('p1', 'roadmap.autoqueue', ?1)",
        rusqlite::params![if on { "1" } else { "0" }],
    )
    .unwrap();
}

fn accept(conn: &Connection, id: &str, queue: bool) -> (ItemStatus, EventKind, Option<String>) {
    let (outcome, event) = update_and_record(
        conn,
        id,
        &status_patch(ItemStatus::Open),
        Some(ItemStatus::Proposed),
        queue,
    )
    .unwrap();
    let outcome = outcome.expect("the row is there");
    assert!(outcome.applied);
    let event = event.expect("an applied accept records itself");
    (outcome.item.status, event.kind, event.detail)
}

#[test]
fn the_dial_and_the_button_land_in_the_same_place() {
    assert_eq!(
        autonomy::accept_landing(false, false, false),
        Landing::Board
    );
    assert_eq!(autonomy::accept_landing(true, false, false), Landing::Queue);
    assert_eq!(autonomy::accept_landing(false, true, false), Landing::Queue);
    assert_eq!(autonomy::accept_landing(true, true, false), Landing::Queue);
    assert_eq!(
        autonomy::accept_landing(true, false, true),
        Landing::HeldBack
    );
    assert_eq!(
        autonomy::accept_landing(false, true, true),
        Landing::HeldBack
    );
    assert_eq!(autonomy::accept_landing(false, false, true), Landing::Board);

    assert_eq!(Landing::Board.status(), ItemStatus::Open);
    assert_eq!(Landing::Queue.status(), ItemStatus::Queued);
    assert_eq!(Landing::HeldBack.status(), ItemStatus::Open);
    assert_eq!(Landing::Board.detail(), None);
    assert!(Landing::Queue.detail().is_some());
    assert!(Landing::HeldBack.detail().is_some());
}

#[test]
fn autoqueue_lands_an_accepted_item_in_the_queue() {
    let conn = test_conn();
    set_autoqueue(&conn, true);
    let it = with_status(&conn, ItemStatus::Proposed);

    let (status, kind, detail) = accept(&conn, &it.id, false);
    assert_eq!(status, ItemStatus::Queued);
    assert_eq!(kind, EventKind::Accepted);
    assert_eq!(detail.as_deref(), Some("auto-queued"));
    assert_eq!(events::list_for_item(&conn, &it.id).unwrap().len(), 1);
}

#[test]
fn the_dial_off_accepts_to_the_board_unless_the_click_asked_to_queue() {
    let conn = test_conn();
    let plain = with_status(&conn, ItemStatus::Proposed);
    assert_eq!(
        accept(&conn, &plain.id, false),
        (ItemStatus::Open, EventKind::Accepted, None)
    );

    let queued = with_status(&conn, ItemStatus::Proposed);
    let (status, kind, detail) = accept(&conn, &queued.id, true);
    assert_eq!(status, ItemStatus::Queued);
    assert_eq!(kind, EventKind::Accepted);
    assert_eq!(detail.as_deref(), Some("auto-queued"));
}

#[test]
fn a_hold_keeps_an_accepted_item_off_the_queue() {
    let conn = test_conn();
    set_autoqueue(&conn, true);

    let held = with_status(&conn, ItemStatus::Proposed);
    brakes::hold_item(
        &conn,
        &held.id,
        "confirm the direction first",
        EventActor::Pm,
    )
    .unwrap();
    let (status, kind, detail) = accept(&conn, &held.id, true);
    assert_eq!(status, ItemStatus::Open, "the item's own hold stopped it");
    assert_eq!(kind, EventKind::Accepted);
    assert!(detail.unwrap().contains("held"));

    let ordinary = with_status(&conn, ItemStatus::Proposed);
    brakes::hold_project(&conn, "p1", "re-planning the quarter", EventActor::Pm).unwrap();
    assert_eq!(accept(&conn, &ordinary.id, false).0, ItemStatus::Open);

    assert!(brakes::release_project(&conn, "p1").unwrap());
    let after = with_status(&conn, ItemStatus::Proposed);
    assert_eq!(accept(&conn, &after.id, false).0, ItemStatus::Queued);
}

#[test]
fn nothing_but_an_accept_is_redirected() {
    let conn = test_conn();
    set_autoqueue(&conn, true);

    let queued = with_status(&conn, ItemStatus::Queued);
    let (outcome, event) = update_and_record(
        &conn,
        &queued.id,
        &status_patch(ItemStatus::Open),
        Some(ItemStatus::Queued),
        true,
    )
    .unwrap();
    assert_eq!(outcome.unwrap().item.status, ItemStatus::Open);
    let event = event.unwrap();
    assert_eq!(event.kind, EventKind::Unqueued);
    assert_eq!(event.detail, None);

    let open = with_status(&conn, ItemStatus::Open);
    let (outcome, event) = update_and_record(
        &conn,
        &open.id,
        &ItemPatch {
            title: Some("retitled".into()),
            ..Default::default()
        },
        None,
        true,
    )
    .unwrap();
    assert_eq!(outcome.unwrap().item.status, ItemStatus::Open);
    assert_eq!(event.unwrap().kind, EventKind::Edited);

    assert!(autonomy::is_accept(
        Some(ItemStatus::Proposed),
        &status_patch(ItemStatus::Open)
    ));
    assert!(!autonomy::is_accept(None, &status_patch(ItemStatus::Open)));
    assert!(!autonomy::is_accept(
        Some(ItemStatus::Proposed),
        &status_patch(ItemStatus::Queued)
    ));
}

#[test]
fn getting_an_item_by_id_finds_it_or_reports_nothing() {
    let conn = test_conn();
    let it = with_status(&conn, ItemStatus::Open);
    assert_eq!(store::get(&conn, &it.id).unwrap().as_ref(), Some(&it));
    assert!(store::get(&conn, "no-such-item").unwrap().is_none());
    store::delete(&conn, &it.id).unwrap();
    assert!(store::get(&conn, &it.id).unwrap().is_none());
}

#[test]
fn handing_off_stamps_the_agent_and_names_it_in_history() {
    let conn = test_conn();
    conn.execute(
        "INSERT INTO workspaces (id, project_id, name, created_at)
             VALUES ('w1', 'p1', 'blue-heron', 0)",
        [],
    )
    .unwrap();
    let it = with_status(&conn, ItemStatus::Open);

    let (item, event) = assignments::hand_off(&conn, &it.id, "w1").unwrap();
    assert_eq!(item.agent_id.as_deref(), Some("w1"));
    assert_eq!(item.status, ItemStatus::Open);
    assert_eq!(event.kind, EventKind::Note);
    assert_eq!(event.actor, EventActor::User);
    assert_eq!(event.detail.as_deref(), Some("Handed to agent blue-heron"));
}

#[test]
fn handing_off_degrades_without_a_name_and_refuses_a_dead_item() {
    let conn = test_conn();
    let it = with_status(&conn, ItemStatus::Open);
    let (_, event) = assignments::hand_off(&conn, &it.id, "gone").unwrap();
    assert_eq!(event.detail.as_deref(), Some("Handed to an agent"));
    assert!(assignments::hand_off(&conn, "no-such-item", "gone").is_err());
}

#[test]
fn handing_off_refuses_a_dispatched_item() {
    let conn = test_conn();
    for status in [ItemStatus::Queued, ItemStatus::Active, ItemStatus::InReview] {
        let it = with_status(&conn, status);
        let err = assignments::hand_off(&conn, &it.id, "w1").unwrap_err();
        assert!(err.contains(status.as_str()), "{err}");
        let row = store::get(&conn, &it.id).unwrap().unwrap();
        assert_eq!(row.agent_id, None, "a refusal must not stamp");
        assert!(events::list_for_item(&conn, &it.id).unwrap().is_empty());
    }
}

#[test]
fn holding_an_item_records_the_reason_and_moves_nothing() {
    let conn = test_conn();
    let it = with_status(&conn, ItemStatus::Queued);

    let (held, event) = hold_item(&conn, &it.id, "confirm the scope", EventActor::Pm).unwrap();
    assert_eq!(held.hold_reason.as_deref(), Some("confirm the scope"));
    assert_eq!(held.held_by, Some(EventActor::Pm));
    assert!(held.held_at.is_some());
    assert_eq!(held.status, ItemStatus::Queued, "a hold is not an unqueue");
    assert_eq!(event.kind, EventKind::Held);
    assert_eq!(event.actor, EventActor::Pm);
    assert_eq!(event.detail.as_deref(), Some("confirm the scope"));
    assert_eq!(events::list_for_item(&conn, &it.id).unwrap(), vec![event]);

    let (released, event) = release_item(&conn, &it.id).unwrap();
    assert!(!released.is_held());
    assert_eq!(released.held_by, None);
    assert_eq!(released.held_at, None);
    assert_eq!(released.status, ItemStatus::Queued);
    let event = event.expect("lifting a real hold is history");
    assert_eq!(event.kind, EventKind::Released);
    assert_eq!(event.actor, EventActor::User);
    assert_eq!(event.detail.as_deref(), Some("confirm the scope"));
}

#[test]
fn re_holding_replaces_the_reason_and_keeps_both_lines() {
    let conn = test_conn();
    let it = with_status(&conn, ItemStatus::Open);
    hold_item(&conn, &it.id, "first reason", EventActor::Pm).unwrap();
    let (held, _) = hold_item(&conn, &it.id, "sharper reason", EventActor::User).unwrap();

    assert_eq!(held.hold_reason.as_deref(), Some("sharper reason"));
    assert_eq!(held.held_by, Some(EventActor::User));
    let trail = events::list_for_item(&conn, &it.id).unwrap();
    assert_eq!(trail.len(), 2, "the superseded reason survives as history");
    assert_eq!(trail[0].detail.as_deref(), Some("sharper reason"));
    assert_eq!(trail[1].detail.as_deref(), Some("first reason"));
    assert!(trail.iter().all(|e| e.kind == EventKind::Held));
}

#[test]
fn releasing_an_unheld_item_is_a_quiet_no_op() {
    let conn = test_conn();
    let it = with_status(&conn, ItemStatus::Open);
    let (row, event) = release_item(&conn, &it.id).unwrap();
    assert_eq!(row.id, it.id);
    assert!(event.is_none());
    assert!(events::list_for_item(&conn, &it.id).unwrap().is_empty());
    assert!(release_item(&conn, "no-such-item").is_err());
}

#[test]
fn holding_a_project_records_the_row_and_no_item_history() {
    let conn = test_conn();
    let it = with_status(&conn, ItemStatus::Queued);

    let hold =
        brakes::hold_project(&conn, "p1", "waiting on the design call", EventActor::Pm).unwrap();
    assert_eq!(hold.reason, "waiting on the design call");
    assert_eq!(hold.held_by, EventActor::Pm);
    assert_eq!(brakes::get_project(&conn, "p1").unwrap(), Some(hold));
    assert!(
        events::list_for_item(&conn, &it.id).unwrap().is_empty(),
        "a board-wide stop belongs to no row"
    );

    assert!(brakes::release_project(&conn, "p1").unwrap());
    assert!(brakes::get_project(&conn, "p1").unwrap().is_none());
}

#[test]
fn a_hold_pauses_an_item_without_sealing_it_to_proposals() {
    use ItemStatus::{Active, Done, InReview, Open, Proposed, Queued};
    for status in [Proposed, Open, Queued] {
        assert!(status.is_rulable(), "{}", status.as_str());
        assert!(rulings::proposal_gate(&with_status(&test_conn(), status)).is_ok());
    }
    for status in [Active, InReview, Done] {
        assert!(!status.is_rulable(), "{}", status.as_str());
        let refusal = rulings::proposal_gate(&with_status(&test_conn(), status)).unwrap_err();
        assert!(refusal.contains(status.as_str()), "{refusal}");
    }

    let conn = test_conn();
    let it = with_status(&conn, ItemStatus::Queued);
    let p = pending_update(&conn, &it, Some("this is what it should have said"));
    hold_item(&conn, &it.id, "the scope is wrong", EventActor::Pm).unwrap();

    let rulings::Ruling::Updated { item, .. } = rulings::accept_proposal(&conn, &p.id).unwrap()
    else {
        panic!("a held item is paused, not sealed");
    };
    assert_eq!(item.title, "reshaped");
    assert!(item.is_held(), "and the hold outlives the ruling");
}

#[test]
fn reclaiming_clears_the_agent_and_names_it_in_history() {
    let conn = test_conn();
    conn.execute(
        "INSERT INTO workspaces (id, project_id, name, created_at)
             VALUES ('w1', 'p1', 'blue-heron', 0)",
        [],
    )
    .unwrap();
    let it = with_status(&conn, ItemStatus::Open);
    assignments::hand_off(&conn, &it.id, "w1").unwrap();

    let (item, event) = assignments::reclaim(&conn, &it.id).unwrap();
    assert_eq!(item.agent_id, None);
    assert_eq!(item.status, ItemStatus::Open, "the status is untouched");
    assert_eq!(event.kind, EventKind::Note);
    assert_eq!(event.actor, EventActor::User);
    assert_eq!(
        event.detail.as_deref(),
        Some("Taken back from agent blue-heron")
    );
    assert!(assignments::reclaim(&conn, &it.id).is_err());
}

#[test]
fn reclaiming_refuses_an_unhanded_or_dispatched_item() {
    let conn = test_conn();
    let bare = with_status(&conn, ItemStatus::Open);
    assert!(assignments::reclaim(&conn, &bare.id)
        .unwrap_err()
        .contains("with an agent"));

    for status in [ItemStatus::Queued, ItemStatus::Active, ItemStatus::InReview] {
        let it = with_status(&conn, status);
        store::update(
            &conn,
            &it.id,
            &ItemPatch {
                agent_id: Some(Some("w1".into())),
                ..Default::default()
            },
        )
        .unwrap();
        let err = assignments::reclaim(&conn, &it.id).unwrap_err();
        assert!(err.contains(status.as_str()), "{err}");
        let row = store::get(&conn, &it.id).unwrap().unwrap();
        assert_eq!(
            row.agent_id.as_deref(),
            Some("w1"),
            "a refusal writes nothing"
        );
        assert!(events::list_for_item(&conn, &it.id).unwrap().is_empty());
    }
    assert!(assignments::reclaim(&conn, "no-such-item").is_err());
}

#[test]
fn rejecting_keeps_the_row_and_the_reason_from_any_prework_status() {
    let conn = test_conn();
    for status in [ItemStatus::Proposed, ItemStatus::Open, ItemStatus::Queued] {
        let it = with_status(&conn, status);
        let (item, event, pending) =
            rulings::reject_item(&conn, &it.id, "  out of scope for v1  ").unwrap();
        assert_eq!(item.status, ItemStatus::Rejected);
        assert_eq!(item.close_reason.as_deref(), Some("out of scope for v1"));
        assert!(pending.is_none(), "nothing was pending on this item");
        assert_eq!(event.kind, EventKind::Rejected);
        assert_eq!(event.actor, EventActor::User);
        assert_eq!(event.detail.as_deref(), Some("out of scope for v1"));
        assert_eq!(events::list_for_item(&conn, &it.id).unwrap(), vec![event]);
    }
}

#[test]
fn rejecting_clears_a_hold_and_the_agent_stamp() {
    let conn = test_conn();
    let it = with_status(&conn, ItemStatus::Open);
    brakes::hold_item(&conn, &it.id, "direction unclear", EventActor::Pm).unwrap();
    store::update(
        &conn,
        &it.id,
        &ItemPatch {
            agent_id: Some(Some("w1".into())),
            ..Default::default()
        },
    )
    .unwrap();

    let (item, _, _) = rulings::reject_item(&conn, &it.id, "not doing this after all").unwrap();
    assert!(!item.is_held());
    assert_eq!(item.held_by, None);
    assert_eq!(item.held_at, None);
    assert_eq!(item.agent_id, None);
    assert_eq!(
        item.close_reason.as_deref(),
        Some("not doing this after all")
    );
}

#[test]
fn a_blank_reason_refuses_the_rejection() {
    let conn = test_conn();
    let it = with_status(&conn, ItemStatus::Open);
    let err = rulings::reject_item(&conn, &it.id, "   ").unwrap_err();
    assert!(err.contains("reason"), "{err}");
    let row = store::get(&conn, &it.id).unwrap().unwrap();
    assert_eq!(row.status, ItemStatus::Open);
    assert_eq!(row.close_reason, None);
    assert!(events::list_for_item(&conn, &it.id).unwrap().is_empty());
}

#[test]
fn rejecting_refuses_inflight_shipped_and_already_rejected_items() {
    let conn = test_conn();
    for status in [
        ItemStatus::Active,
        ItemStatus::InReview,
        ItemStatus::Done,
        ItemStatus::Rejected,
    ] {
        let it = with_status(&conn, status);
        let err = rulings::reject_item(&conn, &it.id, "changed our minds").unwrap_err();
        assert!(err.contains(&it.code), "{err}");
        assert!(err.contains(status.as_str()), "{err}");
        let row = store::get(&conn, &it.id).unwrap().unwrap();
        assert_eq!(row.status, status, "a refusal writes nothing");
        assert_eq!(row.close_reason, None);
        assert!(events::list_for_item(&conn, &it.id).unwrap().is_empty());
    }
    assert!(rulings::reject_item(&conn, "no-such-item", "why").is_err());
}

#[test]
fn rejecting_consumes_the_items_pending_proposal() {
    let conn = test_conn();
    let it = with_status(&conn, ItemStatus::Open);
    let p = pending_update(&conn, &it, Some("reshape it"));

    let (_, _, pending) = rulings::reject_item(&conn, &it.id, "no longer relevant").unwrap();
    assert_eq!(
        pending.map(|p| p.id),
        Some(p.id.clone()),
        "returned so the caller can emit `roadmap:proposal-deleted`"
    );
    assert!(proposals::get(&conn, &p.id).unwrap().is_none());
}

#[test]
fn reopening_returns_a_rejected_item_to_the_board() {
    let conn = test_conn();
    let it = with_status(&conn, ItemStatus::Queued);
    rulings::reject_item(&conn, &it.id, "parked for the rewrite").unwrap();

    let (item, event, _) = rulings::reopen_item(&conn, &it.id).unwrap();
    assert_eq!(item.status, ItemStatus::Open);
    assert_eq!(
        item.close_reason, None,
        "an item back in play owes nobody an epitaph"
    );
    let event = event.expect("a real reopen records itself");
    assert_eq!(event.kind, EventKind::Reopened);
    assert_eq!(event.actor, EventActor::User);
    assert_eq!(
        event.detail.as_deref(),
        Some("was rejected — parked for the rewrite")
    );
    assert_eq!(events::list_for_item(&conn, &it.id).unwrap().len(), 2);

    let (again, event, _) = rulings::reopen_item(&conn, &it.id).unwrap();
    assert_eq!(again.status, ItemStatus::Open);
    assert!(event.is_none());
}

#[test]
fn reopening_an_unrejected_item_is_a_noop_that_records_nothing() {
    let conn = test_conn();
    for status in [ItemStatus::Open, ItemStatus::Queued, ItemStatus::Done] {
        let it = with_status(&conn, status);
        let (item, event, _) = rulings::reopen_item(&conn, &it.id).unwrap();
        assert_eq!(item.status, status, "the row comes back as it actually is");
        assert!(event.is_none());
        assert!(events::list_for_item(&conn, &it.id).unwrap().is_empty());
    }
    assert!(rulings::reopen_item(&conn, "no-such-item").is_err());
}

#[test]
fn reopening_corrects_the_wedge_on_queued_dependants() {
    let conn = test_conn();
    let dep = with_status(&conn, ItemStatus::Open);
    let wedged = with_status(&conn, ItemStatus::Queued);
    store::update(
        &conn,
        &wedged.id,
        &ItemPatch {
            deps: Some(vec![dep.code.clone()]),
            ..Default::default()
        },
    )
    .unwrap();
    let unrelated = with_status(&conn, ItemStatus::Queued);
    let unwedged = with_status(&conn, ItemStatus::Open);
    store::update(
        &conn,
        &unwedged.id,
        &ItemPatch {
            deps: Some(vec![dep.code.clone()]),
            ..Default::default()
        },
    )
    .unwrap();

    rulings::reject_item(&conn, &dep.id, "parked").unwrap();
    events::record(
        &conn,
        &wedged.id,
        "p1",
        EventActor::Drainer,
        EventKind::Blocked,
        Some(&format!(
            "Waiting on {}, which was rejected — remove or replace that dependency.",
            dep.code
        )),
    )
    .unwrap();

    let (_, _, corrections) = rulings::reopen_item(&conn, &dep.id).unwrap();
    assert_eq!(corrections.len(), 1, "only the wedged dependant is told");
    assert_eq!(corrections[0].item_id, wedged.id);
    assert_eq!(corrections[0].kind, EventKind::Note);

    let latest = events::list_for_item(&conn, &wedged.id).unwrap();
    assert_eq!(latest[0].kind, EventKind::Note);
    assert_eq!(
        latest[0].detail.as_deref(),
        Some(format!("{} was reopened — waiting on it normally again", dep.code).as_str())
    );
    assert!(events::list_for_item(&conn, &unrelated.id)
        .unwrap()
        .is_empty());
    assert!(events::list_for_item(&conn, &unwedged.id)
        .unwrap()
        .is_empty());
}

#[test]
fn reopening_clears_a_legacy_hold() {
    let conn = test_conn();
    let it = with_status(&conn, ItemStatus::Open);
    rulings::reject_item(&conn, &it.id, "parked").unwrap();
    conn.execute(
        "UPDATE roadmap_items SET hold_reason = 'stale', held_by = 'pm', held_at = 1
             WHERE id = ?1",
        [&it.id],
    )
    .unwrap();

    let (item, _, _) = rulings::reopen_item(&conn, &it.id).unwrap();
    assert_eq!(item.hold_reason, None, "no secret pause survives a reopen");
    assert_eq!(item.held_by, None);
    assert_eq!(item.held_at, None);
}

#[test]
fn a_generic_status_patch_cannot_reject() {
    let conn = test_conn();
    let it = with_status(&conn, ItemStatus::Open);
    let err = update_and_record(
        &conn,
        &it.id,
        &status_patch(ItemStatus::Rejected),
        None,
        false,
    )
    .unwrap_err();
    assert!(err.contains("roadmap_reject_item"), "{err}");
    let row = store::get(&conn, &it.id).unwrap().unwrap();
    assert_eq!(row.status, ItemStatus::Open, "nothing written");
    assert!(events::list_for_item(&conn, &it.id).unwrap().is_empty());
}

#[test]
fn an_oversize_reason_refuses_the_rejection() {
    let conn = test_conn();
    let it = with_status(&conn, ItemStatus::Open);
    let err = rulings::reject_item(&conn, &it.id, &"x".repeat(brakes::MAX_REASON + 1)).unwrap_err();
    assert!(err.contains("characters"), "{err}");
    assert!(rulings::reject_item(&conn, &it.id, &"—".repeat(brakes::MAX_REASON)).is_ok());
}

fn pending_update(conn: &Connection, item: &RoadmapItem, note: Option<&str>) -> Proposal {
    let patch = ProposalPatch {
        title: Some("reshaped".into()),
        horizon: Some(types::Horizon::Now),
        ..Default::default()
    };
    proposals::upsert(
        conn,
        &item.project_id,
        &item.id,
        ProposalKind::Update,
        Some(&patch),
        note,
    )
    .unwrap()
}

#[test]
fn accepting_an_update_applies_records_and_consumes() {
    let conn = test_conn();
    let it = with_status(&conn, ItemStatus::Open);
    let p = pending_update(&conn, &it, Some("scope grew"));

    let ruling = rulings::accept_proposal(&conn, &p.id).unwrap();
    let rulings::Ruling::Updated { item, event } = ruling else {
        panic!("expected Updated");
    };
    assert_eq!(item.title, "reshaped");
    assert_eq!(item.horizon, types::Horizon::Now);
    assert_eq!(event.kind, EventKind::Edited);
    assert_eq!(event.actor, EventActor::User);
    assert_eq!(
        event.detail.as_deref(),
        Some("Accepted a PM proposal — scope grew")
    );
    assert!(proposals::get(&conn, &p.id).unwrap().is_none());
    assert!(rulings::accept_proposal(&conn, &p.id).is_err());
}

#[test]
fn accepting_against_a_raced_away_item_refuses_and_clears() {
    let conn = test_conn();
    let it = with_status(&conn, ItemStatus::Queued);
    let p = pending_update(&conn, &it, None);
    store::update(&conn, &it.id, &status_patch(ItemStatus::Active)).unwrap();

    let rulings::Ruling::Stale { message } = rulings::accept_proposal(&conn, &p.id).unwrap() else {
        panic!("expected Stale");
    };
    assert!(message.contains("active"), "{message}");
    assert!(message.contains(&it.code), "{message}");
    assert!(proposals::get(&conn, &p.id).unwrap().is_none());
    let row = store::get(&conn, &it.id).unwrap().unwrap();
    assert_eq!(row.title, it.title);
    assert!(events::list_for_item(&conn, &it.id).unwrap().is_empty());
}

#[test]
fn accepting_a_discard_keeps_the_row_as_rejected() {
    let conn = test_conn();
    let it = with_status(&conn, ItemStatus::Open);
    let p = proposals::upsert(
        &conn,
        "p1",
        &it.id,
        ProposalKind::Discard,
        None,
        Some("superseded by MCA-101"),
    )
    .unwrap();

    let rulings::Ruling::Updated { item, event } = rulings::accept_proposal(&conn, &p.id).unwrap()
    else {
        panic!("expected Updated");
    };
    assert_eq!(item.id, it.id, "the row survives the ruling");
    assert_eq!(item.status, ItemStatus::Rejected);
    assert_eq!(item.close_reason.as_deref(), Some("superseded by MCA-101"));
    assert_eq!(event.kind, EventKind::Rejected);
    assert_eq!(event.actor, EventActor::User);
    assert_eq!(
        event.detail.as_deref(),
        Some("Rejected a PM proposal — superseded by MCA-101")
    );
    assert!(proposals::get(&conn, &p.id).unwrap().is_none());
    assert!(rulings::accept_proposal(&conn, &p.id).is_err());
}

#[test]
fn accepting_an_order_renumbers_the_board_and_records_nothing() {
    let conn = test_conn();
    let a = with_status(&conn, ItemStatus::Open);
    let b = with_status(&conn, ItemStatus::Queued);
    let c = with_status(&conn, ItemStatus::Proposed);
    order_proposals::upsert(
        &conn,
        "p1",
        &[c.code.clone(), a.code.clone(), b.code.clone()],
        Some("auth first"),
    )
    .unwrap();

    let rulings::OrderRuling::Applied(rows) = rulings::accept_order(&conn, "p1").unwrap() else {
        panic!("expected Applied");
    };
    assert_eq!(
        rows.iter().map(|r| r.code.as_str()).collect::<Vec<_>>(),
        vec![c.code.as_str(), a.code.as_str(), b.code.as_str()]
    );
    assert_eq!(
        store::list(&conn, "p1")
            .unwrap()
            .iter()
            .map(|i| i.code.clone())
            .collect::<Vec<_>>(),
        vec![c.code.clone(), a.code.clone(), b.code.clone()],
        "the board (and the drainer's queue) now reads in the accepted order"
    );
    for it in [&a, &b, &c] {
        assert!(
            events::list_for_item(&conn, &it.id).unwrap().is_empty(),
            "a reorder is bookkeeping, not history"
        );
    }
    assert!(order_proposals::get(&conn, "p1").unwrap().is_none());
    assert!(rulings::accept_order(&conn, "p1").is_err());
}

#[test]
fn accepting_a_stale_order_refuses_and_clears() {
    let conn = test_conn();
    let a = with_status(&conn, ItemStatus::Queued);
    let b = with_status(&conn, ItemStatus::Open);
    order_proposals::upsert(&conn, "p1", &[b.code.clone(), a.code.clone()], None).unwrap();
    store::update(&conn, &a.id, &status_patch(ItemStatus::Active)).unwrap();
    let fresh = with_status(&conn, ItemStatus::Proposed);

    let rulings::OrderRuling::Stale(message) = rulings::accept_order(&conn, "p1").unwrap() else {
        panic!("expected Stale");
    };
    assert!(message.contains("the board changed"), "{message}");
    assert!(message.contains(&a.code), "{message}");
    assert!(order_proposals::get(&conn, "p1").unwrap().is_none());
    assert_eq!(store::get(&conn, &b.id).unwrap().unwrap().rank, b.rank);
    assert_eq!(
        store::get(&conn, &fresh.id).unwrap().unwrap().rank,
        fresh.rank
    );
}

#[test]
fn rejecting_an_order_drops_the_ask_and_touches_nothing() {
    let conn = test_conn();
    let a = with_status(&conn, ItemStatus::Open);
    order_proposals::upsert(&conn, "p1", std::slice::from_ref(&a.code), Some("nope")).unwrap();

    assert!(order_proposals::delete(&conn, "p1").unwrap());
    assert!(order_proposals::get(&conn, "p1").unwrap().is_none());
    assert_eq!(store::get(&conn, &a.id).unwrap().unwrap().rank, a.rank);
    assert!(events::list_for_item(&conn, &a.id).unwrap().is_empty());
}

#[test]
fn a_dep_edit_is_refused_when_it_closes_a_loop_or_names_nothing() {
    let conn = test_conn();
    let a = with_status(&conn, ItemStatus::Open);
    let b = with_status(&conn, ItemStatus::Open);

    let deps_patch = |codes: &[&str]| ItemPatch {
        deps: Some(codes.iter().map(|c| (*c).to_string()).collect()),
        ..Default::default()
    };

    let (outcome, _) =
        update_and_record(&conn, &b.id, &deps_patch(&[&a.code]), None, false).unwrap();
    assert_eq!(outcome.unwrap().item.deps, vec![a.code.clone()]);

    let err = update_and_record(&conn, &a.id, &deps_patch(&[&b.code]), None, false).unwrap_err();
    assert!(err.contains("loop"), "{err}");
    assert!(
        err.contains(&format!("{} → {} → {}", a.code, b.code, a.code)),
        "the refusal names the loop: {err}"
    );
    assert!(store::get(&conn, &a.id).unwrap().unwrap().deps.is_empty());

    let err = update_and_record(&conn, &a.id, &deps_patch(&["MCA-999"]), None, false).unwrap_err();
    assert!(err.contains("MCA-999"), "{err}");

    let err = update_and_record(&conn, &a.id, &deps_patch(&[&a.code]), None, false).unwrap_err();
    assert!(err.contains("depend on itself"), "{err}");
    let err = create_checked(
        &conn,
        "p1",
        &NewItem {
            title: "fresh".into(),
            deps: vec!["MCA-999".into()],
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(err.contains("MCA-999"), "{err}");
    assert!(create_checked(
        &conn,
        "p1",
        &NewItem {
            title: "fresh".into(),
            deps: vec![a.code.clone()],
            ..Default::default()
        },
    )
    .is_ok());
}

#[test]
fn resending_an_items_own_deps_is_not_refused() {
    let conn = test_conn();
    let a = with_status(&conn, ItemStatus::Open);
    let b = with_status(&conn, ItemStatus::Open);
    for (row, dep) in [(&a, &b.code), (&b, &a.code)] {
        store::update(
            &conn,
            &row.id,
            &ItemPatch {
                deps: Some(vec![dep.clone()]),
                ..Default::default()
            },
        )
        .unwrap();
    }

    let patch = ItemPatch {
        title: Some("retitled".into()),
        deps: Some(vec![b.code.clone()]),
        ..Default::default()
    };
    let (outcome, _) = update_and_record(&conn, &a.id, &patch, None, false).unwrap();
    assert_eq!(outcome.unwrap().item.title, "retitled");
}

#[test]
fn accepting_a_dep_patch_that_became_a_loop_refuses_and_clears() {
    let conn = test_conn();
    let a = with_status(&conn, ItemStatus::Open);
    let b = with_status(&conn, ItemStatus::Open);
    let patch = ProposalPatch {
        deps: Some(vec![b.code.clone()]),
        ..Default::default()
    };
    let p = proposals::upsert(
        &conn,
        "p1",
        &a.id,
        ProposalKind::Update,
        Some(&patch),
        Some("b first"),
    )
    .unwrap();
    store::update(
        &conn,
        &b.id,
        &ItemPatch {
            deps: Some(vec![a.code.clone()]),
            ..Default::default()
        },
    )
    .unwrap();

    let rulings::Ruling::Stale { message } = rulings::accept_proposal(&conn, &p.id).unwrap() else {
        panic!("expected Stale");
    };
    assert!(message.contains("the board changed"), "{message}");
    assert!(message.contains("loop"), "{message}");
    assert!(proposals::get(&conn, &p.id).unwrap().is_none());
    assert!(store::get(&conn, &a.id).unwrap().unwrap().deps.is_empty());
    assert!(events::list_for_item(&conn, &a.id).unwrap().is_empty());
}

#[test]
fn accepting_a_dep_patch_whose_dependency_vanished_refuses() {
    let conn = test_conn();
    let a = with_status(&conn, ItemStatus::Open);
    let gone = with_status(&conn, ItemStatus::Open);
    let patch = ProposalPatch {
        deps: Some(vec![gone.code.clone()]),
        ..Default::default()
    };
    let p =
        proposals::upsert(&conn, "p1", &a.id, ProposalKind::Update, Some(&patch), None).unwrap();
    store::delete(&conn, &gone.id).unwrap();

    let rulings::Ruling::Stale { message } = rulings::accept_proposal(&conn, &p.id).unwrap() else {
        panic!("expected Stale");
    };
    assert!(message.contains(&gone.code), "{message}");
    assert!(proposals::get(&conn, &p.id).unwrap().is_none());
    assert!(store::get(&conn, &a.id).unwrap().unwrap().deps.is_empty());
}

#[test]
fn accepting_a_dep_patch_whose_dependency_was_rejected_refuses() {
    let conn = test_conn();
    let a = with_status(&conn, ItemStatus::Open);
    let dep = with_status(&conn, ItemStatus::Open);
    let patch = ProposalPatch {
        deps: Some(vec![dep.code.clone()]),
        ..Default::default()
    };
    let p =
        proposals::upsert(&conn, "p1", &a.id, ProposalKind::Update, Some(&patch), None).unwrap();
    rulings::reject_item(&conn, &dep.id, "parked").unwrap();

    let rulings::Ruling::Stale { message } = rulings::accept_proposal(&conn, &p.id).unwrap() else {
        panic!("expected Stale");
    };
    assert!(message.contains(&dep.code), "{message}");
    assert!(message.contains("was rejected"), "{message}");
    assert!(proposals::get(&conn, &p.id).unwrap().is_none());
    assert!(store::get(&conn, &a.id).unwrap().unwrap().deps.is_empty());
}

#[test]
fn accepting_a_still_valid_dep_patch_applies_it() {
    let conn = test_conn();
    let a = with_status(&conn, ItemStatus::Open);
    let b = with_status(&conn, ItemStatus::Open);
    let patch = ProposalPatch {
        deps: Some(vec![b.code.clone()]),
        ..Default::default()
    };
    let p =
        proposals::upsert(&conn, "p1", &a.id, ProposalKind::Update, Some(&patch), None).unwrap();

    let rulings::Ruling::Updated { item, .. } = rulings::accept_proposal(&conn, &p.id).unwrap()
    else {
        panic!("expected Updated");
    };
    assert_eq!(item.deps, vec![b.code]);
}

#[test]
fn rejecting_writes_a_note_and_consumes_the_proposal() {
    let conn = test_conn();
    let it = with_status(&conn, ItemStatus::Open);
    let p = pending_update(&conn, &it, Some("split this in two"));

    let event = rulings::reject_proposal(&conn, &p.id).unwrap();
    assert_eq!(event.kind, EventKind::Note);
    assert_eq!(event.actor, EventActor::User);
    assert_eq!(
        event.detail.as_deref(),
        Some("Declined a PM proposal — split this in two")
    );
    assert_eq!(event.item_id, it.id);
    assert!(proposals::get(&conn, &p.id).unwrap().is_none());
    let row = store::get(&conn, &it.id).unwrap().unwrap();
    assert_eq!(row.title, it.title);
}
