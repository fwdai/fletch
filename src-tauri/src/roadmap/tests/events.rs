
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

fn item(conn: &Connection) -> crate::roadmap::types::RoadmapItem {
    store::create(
        conn,
        "p1",
        &NewItem {
            title: "one".into(),
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn events_round_trip_and_list_newest_first() {
    let conn = test_conn();
    let it = item(&conn);

    let first = record(
        &conn,
        &it.id,
        "p1",
        EventActor::Pm,
        EventKind::Proposed,
        None,
    )
    .unwrap();
    let second = record(
        &conn,
        &it.id,
        "p1",
        EventActor::Drainer,
        EventKind::RunFailed,
        Some("its run failed"),
    )
    .unwrap();

    let listed = list_for_item(&conn, &it.id).unwrap();
    assert_eq!(listed, vec![second.clone(), first]);
    assert_eq!(listed[0].detail.as_deref(), Some("its run failed"));
    assert_eq!(second.actor, EventActor::Drainer);
    assert_eq!(second.kind, EventKind::RunFailed);
}

#[test]
fn the_newest_reads_agree_with_the_head_of_the_trail() {
    let conn = test_conn();
    let a = item(&conn);
    let b = item(&conn);
    assert!(latest_per_item(&conn, "p1").unwrap().is_empty());
    assert!(latest_by_item(&conn, "p1").unwrap().is_empty());

    for kind in [EventKind::Created, EventKind::Queued, EventKind::Dispatched] {
        record(&conn, &a.id, "p1", EventActor::User, kind, None).unwrap();
    }
    let b_note = record(
        &conn,
        &b.id,
        "p1",
        EventActor::Pm,
        EventKind::Note,
        Some("watch this one"),
    )
    .unwrap();

    let a_head = list_for_item(&conn, &a.id).unwrap()[0].clone();
    assert_eq!(a_head.kind, EventKind::Dispatched);
    assert_eq!(latest_per_item(&conn, "p1").unwrap().first(), Some(&b_note));

    let by_item = latest_by_item(&conn, "p1").unwrap();
    assert_eq!(by_item.len(), 2);
    assert_eq!(by_item.get(&a.id), Some(&a_head));
    assert_eq!(by_item.get(&b.id), Some(&b_note));

    conn.execute(
        "INSERT INTO projects (id, name, created_at) VALUES ('p2', 'other', 0)",
        [],
    )
    .unwrap();
    assert!(latest_per_item(&conn, "p2").unwrap().is_empty());
    assert!(latest_by_item(&conn, "p2").unwrap().is_empty());
}

#[test]
fn latest_per_item_returns_the_newest_row_for_every_item() {
    // item whose `blocked` was superseded by a dispatch is not blocked, and
    // an item whose last word is `blocked` is.
    let conn = test_conn();
    let one = item(&conn);
    let two = item(&conn);

    for (id, kind, detail) in [
        (&one.id, EventKind::Queued, None),
        (
            &one.id,
            EventKind::Blocked,
            Some("MCA-100 → MCA-101 → MCA-100"),
        ),
        // Supersedes the block: same millisecond, so only the rowid
        (&one.id, EventKind::Dispatched, Some("build")),
        (&two.id, EventKind::Queued, None),
        (
            &two.id,
            EventKind::Blocked,
            Some("MCA-101 → MCA-100 → MCA-101"),
        ),
    ] {
        record(&conn, id, "p1", EventActor::Drainer, kind, detail).unwrap();
    }

    let latest = latest_per_item(&conn, "p1").unwrap();
    assert_eq!(latest.len(), 2, "one row per item, not one per event");
    let by: std::collections::HashMap<&str, &ItemEvent> =
        latest.iter().map(|e| (e.item_id.as_str(), e)).collect();
    assert_eq!(by[one.id.as_str()].kind, EventKind::Dispatched);
    assert_eq!(by[two.id.as_str()].kind, EventKind::Blocked);
    assert_eq!(
        by[two.id.as_str()].detail.as_deref(),
        Some("MCA-101 → MCA-100 → MCA-101")
    );
}

#[test]
fn latest_per_item_is_scoped_to_one_board() {
    let conn = test_conn();
    conn.execute(
        "INSERT INTO projects (id, name, created_at) VALUES ('p2', 'other', 0)",
        [],
    )
    .unwrap();
    let mine = item(&conn);
    let theirs = store::create(
        &conn,
        "p2",
        &NewItem {
            title: "theirs".into(),
            ..Default::default()
        },
    )
    .unwrap();
    record(
        &conn,
        &mine.id,
        "p1",
        EventActor::User,
        EventKind::Queued,
        None,
    )
    .unwrap();
    record(
        &conn,
        &theirs.id,
        "p2",
        EventActor::Drainer,
        EventKind::Blocked,
        None,
    )
    .unwrap();

    let latest = latest_per_item(&conn, "p1").unwrap();
    assert_eq!(latest.len(), 1);
    assert_eq!(latest[0].item_id, mine.id);
}

#[test]
fn deleting_an_item_takes_its_history() {
    let conn = test_conn();
    let it = item(&conn);
    record(
        &conn,
        &it.id,
        "p1",
        EventActor::User,
        EventKind::Queued,
        None,
    )
    .unwrap();
    assert!(store::delete(&conn, &it.id).unwrap());
    assert!(list_for_item(&conn, &it.id).unwrap().is_empty());
}

#[test]
fn every_kind_is_declared_on_both_sides_of_the_wire() {
    const TS: &str = include_str!("../../../../src/api/types/roadmap.ts");
    // The union's own block, so an unrelated string literal elsewhere in the
    let union = TS
        .split("export type RoadmapEventKind =")
        .nth(1)
        .and_then(|rest| rest.split_once(';'))
        .map(|(block, _)| block)
        .expect("the frontend declares RoadmapEventKind");

    let declared: Vec<&str> = union
        .split('"')
        .skip(1)
        .step_by(2)
        .filter(|s| !s.is_empty())
        .collect();
    for kind in ALL_KINDS {
        assert!(
            declared.contains(&kind.as_str()),
            "the frontend union is missing {:?} — it would render as `undefined`",
            kind.as_str()
        );
    }
    for spelling in &declared {
        assert!(
            EventKind::from_db(spelling).is_some(),
            "the frontend declares {spelling:?}, which no writer can produce"
        );
    }
    assert_eq!(declared.len(), ALL_KINDS.len());
}

#[test]
fn every_kind_is_declared_in_the_pms_instructions() {
    const DOC: &str = include_str!("../../instructions/roadmap.md");
    let list = DOC
        .split("`kind` (`")
        .nth(1)
        .and_then(|rest| rest.split_once("`)"))
        .map(|(block, _)| block)
        .expect("the instructions enumerate the kinds a last_event carries");
    let documented: Vec<String> = list
        .split('|')
        .map(|s| s.split_whitespace().collect::<String>())
        .filter(|s| !s.is_empty())
        .collect();

    for kind in ALL_KINDS {
        assert!(
            documented.iter().any(|d| d == kind.as_str()),
            "instructions/roadmap.md never mentions {:?} — the PM cannot read a line it \
                 doesn't know exists",
            kind.as_str()
        );
    }
    for spelling in &documented {
        assert!(
            EventKind::from_db(spelling).is_some(),
            "instructions/roadmap.md advertises {spelling:?}, which no writer can produce"
        );
    }
    assert_eq!(documented.len(), ALL_KINDS.len());
}

const ALL_KINDS: [EventKind; 19] = [
    EventKind::Created,
    EventKind::Proposed,
    EventKind::Accepted,
    EventKind::Edited,
    EventKind::Queued,
    EventKind::Unqueued,
    EventKind::Dispatched,
    EventKind::PrOpened,
    EventKind::RunFailed,
    EventKind::RunCanceled,
    EventKind::RunDeleted,
    EventKind::Shipped,
    EventKind::PrClosed,
    EventKind::Blocked,
    EventKind::Held,
    EventKind::Released,
    EventKind::Rejected,
    EventKind::Reopened,
    EventKind::Note,
];

#[test]
fn the_user_transitions_map_to_their_kinds() {
    use ItemStatus::{Done, InReview, Open, Proposed, Queued};
    let cases = [
        (Some(Proposed), Some(Open), EventKind::Accepted),
        (Some(Open), Some(Queued), EventKind::Queued),
        (Some(Queued), Some(Open), EventKind::Unqueued),
        (Some(InReview), Some(Done), EventKind::Shipped),
        (None, None, EventKind::Edited),
        (Some(Open), None, EventKind::Edited),
    ];
    for (expected, to, kind) in cases {
        assert_eq!(
            transition_kind(expected, to),
            kind,
            "{expected:?} -> {to:?}"
        );
    }
}
