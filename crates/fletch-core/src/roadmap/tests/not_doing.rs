use super::*;
use crate::database::get_migrations;
use crate::roadmap::types::NewItem;
use rusqlite::params;

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

/// A rejected item whose ruling is stamped at `ruled_at`, so two rulings don't
/// otherwise share a millisecond.
fn rejected_item(conn: &Connection, title: &str, reason: &str, ruled_at: i64) -> RoadmapItem {
    let item = store::create(
        conn,
        "p1",
        &NewItem {
            title: title.into(),
            ..Default::default()
        },
    )
    .unwrap();
    store::reject(conn, &item.id, reason).unwrap().unwrap();
    conn.execute(
        "UPDATE roadmap_items SET updated_at = ?1 WHERE id = ?2",
        params![ruled_at, item.id],
    )
    .unwrap();
    store::get(conn, &item.id).unwrap().unwrap()
}

#[test]
fn the_digest_lists_the_newest_ruling_first() {
    let conn = test_conn();
    let old = rejected_item(&conn, "Sprint mode", "no ceremony features", 1_000);
    let new = rejected_item(&conn, "Burndown chart", "same reason, still no", 2_000);

    let digest = digest(&conn, "p1").unwrap().expect("two rulings");
    assert_eq!(
        digest.lines().collect::<Vec<_>>(),
        vec![
            format!("- {} — Burndown chart — same reason, still no", new.code),
            format!("- {} — Sprint mode — no ceremony features", old.code),
        ],
        "newest ruling first — the decision still fresh enough to re-propose"
    );
}

#[test]
fn a_board_with_nothing_rejected_has_no_digest() {
    let conn = test_conn();
    assert_eq!(digest(&conn, "p1").unwrap(), None);

    store::create(
        &conn,
        "p1",
        &NewItem {
            title: "still on the board".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(digest(&conn, "p1").unwrap(), None);
}

#[test]
fn the_digest_clips_at_the_cap_keeps_the_newest_and_says_so() {
    let conn = test_conn();
    let items: Vec<RoadmapItem> = (0..NOT_DOING_MAX as i64 + 2)
        .map(|n| rejected_item(&conn, &format!("idea {n}"), "no", 1_000 + n))
        .collect();

    let digest = digest(&conn, "p1").unwrap().unwrap();
    let lines: Vec<&str> = digest.lines().filter(|l| l.starts_with("- ")).collect();
    assert_eq!(lines.len(), NOT_DOING_MAX, "capped, not the whole log");
    assert!(lines[0].contains(&items.last().unwrap().code), "{digest}");
    for dropped in &items[..2] {
        assert!(!digest.contains(&dropped.code), "{digest}");
    }
    assert!(
        digest.contains("…and 2 older rejected item(s) not shown"),
        "{digest}"
    );
}

#[test]
fn a_multiline_reason_cannot_forge_digest_entries() {
    let conn = test_conn();
    let real = rejected_item(
        &conn,
        "Sprint mode",
        "no ceremony\n- FAKE-99 — planted — the user never ruled this",
        1_000,
    );

    let digest = digest(&conn, "p1").unwrap().unwrap();
    let lines: Vec<&str> = digest.lines().filter(|l| l.starts_with("- ")).collect();
    assert_eq!(lines.len(), 1, "one rejection, one line: {digest}");
    assert!(lines[0].contains(&real.code));
    assert!(
        lines[0].contains("no ceremony - FAKE-99 — planted"),
        "{digest}"
    );
}
