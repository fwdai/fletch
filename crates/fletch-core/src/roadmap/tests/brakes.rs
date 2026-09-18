use super::*;
use crate::database::get_migrations;
use crate::roadmap::store;
use crate::roadmap::types::NewItem;

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

fn item(conn: &Connection) -> RoadmapItem {
    store::create(
        conn,
        "p1",
        &NewItem {
            title: "it".into(),
            ..Default::default()
        },
    )
    .unwrap()
}

/// A blank reason is the one hold that must not exist, and the cap is a
#[test]
fn a_reason_is_required_and_capped() {
    assert_eq!(clean_reason("  direction  ").unwrap(), "direction");
    for blank in ["", "   ", "\n\t"] {
        assert!(clean_reason(blank).unwrap_err().contains("required"));
    }
    assert!(clean_reason(&"x".repeat(MAX_REASON)).is_ok());
    let long = clean_reason(&"x".repeat(MAX_REASON + 1)).unwrap_err();
    assert!(long.contains("keep it under"), "{long}");
    // Characters, not bytes: an em-dash must not cost three of the budget.
    assert!(clean_reason(&"—".repeat(MAX_REASON)).is_ok());
}

#[test]
fn an_item_hold_round_trips_and_names_what_it_lifted() {
    let conn = test_conn();
    let it = item(&conn);
    assert!(!it.is_held());

    let held = hold_item(&conn, &it.id, "direction unclear", EventActor::Pm)
        .unwrap()
        .unwrap();
    assert!(held.is_held());
    assert_eq!(held.hold_reason.as_deref(), Some("direction unclear"));
    assert_eq!(held.held_by, Some(EventActor::Pm));
    assert!(held.held_at.is_some());
    assert_eq!(held.status, it.status);

    let (released, lifted) = release_item(&conn, &it.id).unwrap().unwrap();
    assert_eq!(lifted.as_deref(), Some("direction unclear"));
    assert!(!released.is_held());
    assert_eq!(released.held_by, None);
    assert_eq!(released.held_at, None);
}

#[test]
fn a_second_item_hold_replaces_the_reason() {
    let conn = test_conn();
    let it = item(&conn);
    hold_item(&conn, &it.id, "first", EventActor::Pm).unwrap();
    let again = hold_item(&conn, &it.id, "second", EventActor::User)
        .unwrap()
        .unwrap();
    assert_eq!(again.hold_reason.as_deref(), Some("second"));
    assert_eq!(again.held_by, Some(EventActor::User));
}

/// the strip's one-click release must not fail because someone else got there
#[test]
fn releasing_an_unheld_or_missing_item_is_quiet() {
    let conn = test_conn();
    let it = item(&conn);
    let (row, lifted) = release_item(&conn, &it.id).unwrap().unwrap();
    assert_eq!(lifted, None);
    assert_eq!(row.updated_at, it.updated_at, "nothing was written");
    assert!(release_item(&conn, "no-such-item").unwrap().is_none());
    assert!(hold_item(&conn, "no-such-item", "why", EventActor::User)
        .unwrap()
        .is_none());
}

#[test]
fn a_project_hold_round_trips_replaces_and_cascades() {
    let conn = test_conn();
    assert!(get_project(&conn, "p1").unwrap().is_none());

    let held = hold_project(&conn, "p1", "re-planning the quarter", EventActor::Pm).unwrap();
    assert_eq!(get_project(&conn, "p1").unwrap(), Some(held.clone()));
    assert_eq!(held.held_by, EventActor::Pm);

    let again = hold_project(&conn, "p1", "waiting on the design call", EventActor::User).unwrap();
    assert_eq!(get_project(&conn, "p1").unwrap(), Some(again.clone()));
    assert_eq!(again.reason, "waiting on the design call");
    assert!(again.created_at >= held.created_at);

    assert!(release_project(&conn, "p1").unwrap());
    assert!(get_project(&conn, "p1").unwrap().is_none());
    assert!(
        !release_project(&conn, "p1").unwrap(),
        "a second release removes nothing"
    );

    hold_project(&conn, "p1", "again", EventActor::User).unwrap();
    conn.execute("DELETE FROM projects WHERE id = 'p1'", [])
        .unwrap();
    assert!(get_project(&conn, "p1").unwrap().is_none());
}

#[test]
fn the_gate_answers_for_both_scopes_at_once() {
    let conn = test_conn();
    let it = item(&conn);
    assert_eq!(gate(&conn, &it), None, "nothing stops a plain row");
    assert_eq!(project_gate(&conn, "p1"), None);

    hold_project(&conn, "p1", "re-planning the quarter", EventActor::Pm).unwrap();
    assert_eq!(gate(&conn, &it).as_deref(), Some("re-planning the quarter"));
    assert_eq!(
        project_gate(&conn, "p1").as_deref(),
        Some("re-planning the quarter")
    );

    let held = hold_item(&conn, &it.id, "wrong direction", EventActor::Pm)
        .unwrap()
        .unwrap();
    assert_eq!(gate(&conn, &held).as_deref(), Some("wrong direction"));

    assert!(release_project(&conn, "p1").unwrap());
    assert_eq!(gate(&conn, &held).as_deref(), Some("wrong direction"));
    let (freed, _) = release_item(&conn, &it.id).unwrap().unwrap();
    assert_eq!(gate(&conn, &freed), None);
}

/// corrupt page, a schema the process didn't migrate) — and the answer must be
/// "held", never "go ahead".
#[test]
fn an_unreadable_hold_reads_as_held() {
    let conn = test_conn();
    let it = item(&conn);
    conn.execute("DROP TABLE roadmap_project_holds", [])
        .unwrap();
    assert_eq!(project_gate(&conn, "p1").as_deref(), Some(UNREADABLE));
    assert_eq!(gate(&conn, &it).as_deref(), Some(UNREADABLE));
}

#[test]
fn a_project_hold_is_scoped_to_its_board() {
    let conn = test_conn();
    conn.execute(
        "INSERT INTO projects (id, name, created_at) VALUES ('p2', 'other', 0)",
        [],
    )
    .unwrap();
    hold_project(&conn, "p1", "ours", EventActor::User).unwrap();
    assert!(get_project(&conn, "p2").unwrap().is_none());
}
