
use rusqlite_migration::{Migrations, M};

use super::*;
use crate::database::get_migrations;

fn test_conn() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    get_migrations().to_latest(&mut conn).unwrap();
    conn
}

fn project(conn: &Connection, id: &str, name: &str) -> String {
    conn.execute(
        "INSERT INTO projects (id, name, created_at) VALUES (?1, ?2, 0)",
        params![id, name],
    )
    .unwrap();
    id.to_string()
}

fn titled(title: &str) -> NewItem {
    NewItem {
        title: title.to_string(),
        ..Default::default()
    }
}

#[test]
fn codes_run_sequentially_from_100_with_a_derived_prefix() {
    let conn = test_conn();
    let p = project(&conn, "p1", "my-cool-app");

    let first = create(&conn, &p, &titled("one")).unwrap();
    let second = create(&conn, &p, &titled("two")).unwrap();

    assert_eq!(first.code, "MCA-100");
    assert_eq!(second.code, "MCA-101");
    let stored: String = conn
        .query_row(
            "SELECT value FROM project_settings WHERE project_id = 'p1' AND key = ?1",
            [PREFIX_KEY],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, "MCA");
}

#[test]
fn code_allocation_is_per_project() {
    let conn = test_conn();
    let a = project(&conn, "pa", "alpha");
    let b = project(&conn, "pb", "beta service");

    let a1 = create(&conn, &a, &titled("a1")).unwrap();
    let b1 = create(&conn, &b, &titled("b1")).unwrap();
    let a2 = create(&conn, &a, &titled("a2")).unwrap();

    assert_eq!(a1.code, "ALP-100");
    assert_eq!(a2.code, "ALP-101");
    assert_eq!(b1.code, "BS-100");
}

#[test]
fn a_number_is_issued_once_ever_even_if_its_item_is_deleted() {
    let conn = test_conn();
    let p = project(&conn, "p1", "fletch");
    let one = create(&conn, &p, &titled("one")).unwrap();
    let two = create(&conn, &p, &titled("two")).unwrap();
    let three = create(&conn, &p, &titled("three")).unwrap();
    assert!(delete(&conn, &two.id).unwrap());
    assert!(delete(&conn, &three.id).unwrap());

    let four = create(&conn, &p, &titled("four")).unwrap();
    assert_eq!(one.code, "FLE-100", "survivors keep their codes");
    assert_eq!(two.code, "FLE-101");
    assert_eq!(three.code, "FLE-102");
    assert_eq!(four.code, "FLE-103", "no gap is ever backfilled");
    let seq: String = conn
        .query_row(
            "SELECT value FROM project_settings WHERE project_id = 'p1' AND key = ?1",
            [SEQ_KEY],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(seq, "104");
}

#[test]
fn a_project_numbered_before_the_counter_existed_picks_up_after_its_rows() {
    // stored counter. The first allocation must clear them rather than
    let conn = test_conn();
    let p = project(&conn, "p1", "fletch");
    conn.execute(
        "INSERT INTO roadmap_items (id, project_id, code, title, horizon, status,
                                        created_at, updated_at)
             VALUES ('legacy', 'p1', 'FLE-142', 'from before', 'later', 'open', 0, 0)",
        [],
    )
    .unwrap();
    assert_eq!(create(&conn, &p, &titled("next")).unwrap().code, "FLE-143");
    assert_eq!(create(&conn, &p, &titled("after")).unwrap().code, "FLE-144");
}

#[test]
fn foreign_codes_do_not_poison_the_next_number() {
    let conn = test_conn();
    let p = project(&conn, "p1", "fletch");
    conn.execute(
        "INSERT INTO roadmap_items (id, project_id, code, title, horizon, status,
                                        created_at, updated_at)
             VALUES ('imported', 'p1', '#207', 'from github', 'later', 'open', 0, 0)",
        [],
    )
    .unwrap();
    assert_eq!(create(&conn, &p, &titled("x")).unwrap().code, "FLE-100");
}

/// project, so adding a ticket never jumps the queue.
#[test]
fn a_new_item_ranks_last_within_its_own_project() {
    let conn = test_conn();
    let a = project(&conn, "pa", "alpha");
    let b = project(&conn, "pb", "beta");

    let first = create(&conn, &a, &titled("one")).unwrap();
    let second = create(&conn, &a, &titled("two")).unwrap();
    let other = create(&conn, &b, &titled("theirs")).unwrap();
    assert_eq!(first.rank, 1.0);
    assert_eq!(second.rank, 2.0);
    assert_eq!(other.rank, 1.0, "each project has its own sequence");

    update(
        &conn,
        &second.id,
        &ItemPatch {
            rank: Some(0.5),
            ..Default::default()
        },
    )
    .unwrap();
    let third = create(&conn, &a, &titled("three")).unwrap();
    assert_eq!(third.rank, 2.0, "MAX + 1 over what is left");
    assert_eq!(
        list(&conn, &a)
            .unwrap()
            .iter()
            .map(|i| i.code.clone())
            .collect::<Vec<_>>(),
        vec![second.code, first.code, third.code],
        "the board draws rank order"
    );
}

/// … per project. Seeded through raw inserts because that is what a
/// pre-migration database holds.
#[test]
fn the_migration_backfills_existing_rows_in_board_order() {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    const BEFORE_RANK: usize = 31;
    let before = BEFORE_RANK;
    Migrations::new(
        crate::database::MIGRATIONS[..before]
            .iter()
            .map(|&sql| M::up(sql))
            .collect(),
    )
    .to_latest(&mut conn)
    .unwrap();
    conn.execute_batch(
        "INSERT INTO projects (id, name, created_at) VALUES ('p1', 'one', 0), ('p2', 'two', 0);
             INSERT INTO roadmap_items (id, project_id, code, title, horizon, status,
                                        created_at, updated_at) VALUES
               ('b', 'p1', 'ONE-101', 'second', 'later', 'open', 20, 20),
               ('a', 'p1', 'ONE-100', 'first',  'later', 'open', 10, 10),
               ('c', 'p1', 'ONE-102', 'tied',   'later', 'open', 20, 20),
               ('d', 'p2', 'TWO-100', 'theirs', 'later', 'open', 99, 99);",
    )
    .unwrap();

    get_migrations().to_latest(&mut conn).unwrap();

    let ranks: Vec<(String, f64)> = list(&conn, "p1")
        .unwrap()
        .iter()
        .map(|i| (i.id.clone(), i.rank))
        .collect();
    assert_eq!(
        ranks,
        vec![
            ("a".to_string(), 1.0),
            ("b".to_string(), 2.0),
            ("c".to_string(), 3.0)
        ]
    );
    assert_eq!(list(&conn, "p2").unwrap()[0].rank, 1.0);
    assert_eq!(create(&conn, "p1", &titled("next")).unwrap().rank, 4.0);
}

#[test]
fn setting_a_whole_sequence_renumbers_it_from_one() {
    let conn = test_conn();
    let p = project(&conn, "p1", "fletch");
    let one = create(&conn, &p, &titled("one")).unwrap();
    let two = create(&conn, &p, &titled("two")).unwrap();
    let three = create(&conn, &p, &titled("three")).unwrap();

    let rows = set_ranks(&conn, &[three.id.clone(), one.id.clone(), two.id.clone()]).unwrap();
    assert_eq!(
        rows.iter().map(|r| r.rank).collect::<Vec<_>>(),
        vec![1.0, 2.0, 3.0]
    );
    assert_eq!(
        list(&conn, &p)
            .unwrap()
            .iter()
            .map(|i| i.id.clone())
            .collect::<Vec<_>>(),
        vec![three.id, one.id, two.id]
    );

    let ghost = "no-such-item".to_string();
    assert_eq!(set_ranks(&conn, &[ghost]).unwrap().len(), 0);
}

#[test]
fn create_defaults_and_round_trips_json_arrays() {
    let conn = test_conn();
    let p = project(&conn, "p1", "fletch");

    let bare = create(&conn, &p, &titled("bare")).unwrap();
    assert_eq!(bare.horizon, Horizon::Later, "an unplaced item is backlog");
    assert_eq!(bare.status, ItemStatus::Open);
    assert_eq!(bare.source, ItemSource::User);
    assert!(bare.accept.is_empty() && bare.deps.is_empty());
    assert_eq!(bare.why, "");

    let full = create(
        &conn,
        &p,
        &NewItem {
            title: "shaped".into(),
            why: "because".into(),
            horizon: Some(Horizon::Now),
            status: Some(ItemStatus::Proposed),
            area: Some("runtime".into()),
            source: Some(ItemSource::Pm),
            accept: vec!["survives a quit".into(), "reattaches".into()],
            deps: vec![bare.code.clone()],
            workflow_def_id: Some("wf-pipeline".into()),
            issue_url: None,
        },
    )
    .unwrap();

    let rows = list(&conn, &p).unwrap();
    assert_eq!(rows.len(), 2);
    let stored = rows.iter().find(|i| i.id == full.id).unwrap();
    assert_eq!(stored, &full);
    assert_eq!(stored.accept, vec!["survives a quit", "reattaches"]);
    assert_eq!(stored.deps, vec![bare.code]);
    assert_eq!(stored.status, ItemStatus::Proposed);
    assert_eq!(stored.workflow_def_id.as_deref(), Some("wf-pipeline"));
    assert_eq!(bare.workflow_def_id, None);
}

#[test]
fn a_wire_borne_null_clears_the_column() {
    let conn = test_conn();
    let p = project(&conn, "p1", "fletch");
    let item = create(
        &conn,
        &p,
        &NewItem {
            title: "labelled".into(),
            area: Some("runtime".into()),
            workflow_def_id: Some("wf-pipeline".into()),
            ..Default::default()
        },
    )
    .unwrap();

    let patch: ItemPatch = serde_json::from_str(r#"{"area": null}"#).unwrap();
    let row = update(&conn, &item.id, &patch).unwrap().unwrap();
    assert_eq!(row.area, None, "the dialog's clear must stick");
    assert_eq!(
        row.workflow_def_id.as_deref(),
        Some("wf-pipeline"),
        "absent keys stay untouched"
    );
}

#[test]
fn update_patches_only_named_fields_and_clears_with_null() {
    let conn = test_conn();
    let p = project(&conn, "p1", "fletch");
    let item = create(
        &conn,
        &p,
        &NewItem {
            title: "move me".into(),
            horizon: Some(Horizon::Later),
            area: Some("runtime".into()),
            accept: vec!["one".into()],
            ..Default::default()
        },
    )
    .unwrap();

    let moved = update(
        &conn,
        &item.id,
        &ItemPatch {
            horizon: Some(Horizon::Now),
            ..Default::default()
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(moved.horizon, Horizon::Now);
    assert_eq!(
        moved.area.as_deref(),
        Some("runtime"),
        "an absent field is left alone"
    );
    assert_eq!(moved.accept, vec!["one"]);
    assert_eq!(moved.code, item.code, "a code never moves");
    assert_eq!(moved.created_at, item.created_at);

    let cleared = update(
        &conn,
        &item.id,
        &ItemPatch {
            area: Some(None),
            accept: Some(vec![]),
            title: Some("retitled".into()),
            ..Default::default()
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(cleared.area, None);
    assert!(cleared.accept.is_empty());
    assert_eq!(cleared.title, "retitled");

    assert!(delete(&conn, &item.id).unwrap());
    assert!(update(&conn, &item.id, &ItemPatch::default())
        .unwrap()
        .is_none());
    assert!(
        !delete(&conn, &item.id).unwrap(),
        "second delete is a no-op"
    );
    assert!(list(&conn, &p).unwrap().is_empty());
}

#[test]
fn a_conditional_update_applies_only_while_the_status_still_holds() {
    // The unqueue race: the drainer claims `queued → active` under the
    // connection lock, and the click that says `queued → open` arrives a
    let conn = test_conn();
    let p = project(&conn, "p1", "fletch");
    let item = create(
        &conn,
        &p,
        &NewItem {
            title: "queue me".into(),
            status: Some(ItemStatus::Queued),
            ..Default::default()
        },
    )
    .unwrap();

    let unqueued = update_where_status(
        &conn,
        &item.id,
        ItemStatus::Queued,
        &ItemPatch {
            status: Some(ItemStatus::Open),
            ..Default::default()
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(unqueued.status, ItemStatus::Open);

    let claimed = update(
        &conn,
        &item.id,
        &ItemPatch {
            status: Some(ItemStatus::Active),
            ..Default::default()
        },
    )
    .unwrap()
    .unwrap();

    assert!(update_where_status(
        &conn,
        &item.id,
        ItemStatus::Queued,
        &ItemPatch {
            status: Some(ItemStatus::Open),
            ..Default::default()
        },
    )
    .unwrap()
    .is_none());
    assert_eq!(get(&conn, &item.id).unwrap().unwrap(), claimed);

    assert!(
        update_where_status(&conn, &item.id, ItemStatus::Queued, &ItemPatch::default())
            .unwrap()
            .is_none()
    );
    assert!(
        update_where_status(&conn, &item.id, ItemStatus::Active, &ItemPatch::default())
            .unwrap()
            .is_some()
    );

    assert!(delete(&conn, &item.id).unwrap());
    assert!(update_where_status(
        &conn,
        &item.id,
        ItemStatus::Active,
        &ItemPatch {
            status: Some(ItemStatus::Open),
            ..Default::default()
        },
    )
    .unwrap()
    .is_none());
}

#[test]
fn a_routed_row_carries_its_issue_url_and_a_hand_typed_one_does_not() {
    let conn = test_conn();
    let p = project(&conn, "p1", "fletch");
    let routed = create(
        &conn,
        &p,
        &NewItem {
            title: "Crash on save".into(),
            why: "https://github.com/o/r/issues/7\nSaving drops the body".into(),
            status: Some(ItemStatus::Proposed),
            source: Some(ItemSource::Github),
            issue_url: Some("https://github.com/o/r/issues/7".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let typed = create(&conn, &p, &titled("my own idea")).unwrap();
    assert_eq!(
        routed.issue_url.as_deref(),
        Some("https://github.com/o/r/issues/7")
    );
    assert_eq!(typed.issue_url, None, "nothing imported this one");

    // the user's to rewrite, and dedup must not notice.
    let edited = update(
        &conn,
        &routed.id,
        &ItemPatch {
            why: Some("Because three people asked".into()),
            ..Default::default()
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        edited.issue_url.as_deref(),
        Some("https://github.com/o/r/issues/7"),
        "an edited rationale is still the same issue"
    );
    let patch: ItemPatch = serde_json::from_str(r#"{"issue_url": "https://evil/1"}"#).unwrap();
    let after = update(&conn, &routed.id, &patch).unwrap().unwrap();
    assert_eq!(
        after.issue_url.as_deref(),
        Some("https://github.com/o/r/issues/7"),
        "provenance is not editable"
    );
    assert_eq!(list(&conn, &p).unwrap().len(), 2);
}

#[test]
fn a_declined_issue_is_remembered_once_per_project() {
    let conn = test_conn();
    let a = project(&conn, "pa", "alpha");
    let b = project(&conn, "pb", "beta");
    let url = "https://github.com/o/r/issues/7";

    decline_issue(&conn, &a, url).unwrap();
    decline_issue(&conn, &a, url).unwrap();
    decline_issue(&conn, &a, "https://github.com/o/r/issues/9").unwrap();
    assert!(declined_issues(&conn, &b).unwrap().is_empty());
    assert_eq!(
        declined_issues(&conn, &a).unwrap(),
        vec![
            url.to_string(),
            "https://github.com/o/r/issues/9".to_string()
        ]
    );

    let raw: String = conn
        .query_row(
            "SELECT value FROM project_settings WHERE project_id = 'pa' AND key = ?1",
            [DECLINED_ISSUES_KEY],
            |r| r.get(0),
        )
        .unwrap();
    assert!(raw.starts_with('['), "{raw}");
    assert!(raw.contains(url));

    // A corrupt value costs the dedup, never a panic.
    conn.execute(
        "UPDATE project_settings SET value = 'not json' WHERE project_id = 'pa'",
        [],
    )
    .unwrap();
    assert!(declined_issues(&conn, &a).unwrap().is_empty());
    decline_issue(&conn, &a, url).unwrap();
    assert_eq!(declined_issues(&conn, &a).unwrap(), vec![url.to_string()]);
}

/// must cross it untouched — no rebuild, no cascade, and the new column
#[test]
fn the_issue_url_migration_leaves_existing_rows_alone() {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    // reason the rank backfill test pins its own: a later migration must not
    const BEFORE_ISSUE_URL: usize = 35;
    Migrations::new(
        crate::database::MIGRATIONS[..BEFORE_ISSUE_URL]
            .iter()
            .map(|&sql| M::up(sql))
            .collect(),
    )
    .to_latest(&mut conn)
    .unwrap();
    conn.execute_batch(
        "INSERT INTO projects (id, name, created_at) VALUES ('p1', 'one', 0);
             INSERT INTO roadmap_items (id, project_id, code, title, why, horizon, status,
                                        rank, created_at, updated_at) VALUES
               ('old', 'p1', 'ONE-100', 'from before',
                'https://github.com/o/r/issues/1\nrouted the old way',
                'later', 'proposed', 1.0, 10, 10);",
    )
    .unwrap();

    get_migrations().to_latest(&mut conn).unwrap();

    let rows = list(&conn, "p1").unwrap();
    assert_eq!(rows.len(), 1, "the row survived the migration");
    assert_eq!(rows[0].code, "ONE-100");
    assert_eq!(
        rows[0].issue_url, None,
        "not backfilled — the legacy URL stays in the `why`, where the \
             frontend's fallback reader finds it"
    );
    assert!(rows[0].why.starts_with("https://github.com/o/r/issues/1"));
    decline_issue(&conn, "p1", "https://github.com/o/r/issues/1").unwrap();
    let fresh = create(
        &conn,
        "p1",
        &NewItem {
            title: "new import".into(),
            issue_url: Some("https://github.com/o/r/issues/2".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        fresh.issue_url.as_deref(),
        Some("https://github.com/o/r/issues/2")
    );
}

#[test]
fn deleting_a_project_takes_its_roadmap() {
    let conn = test_conn();
    let p = project(&conn, "p1", "fletch");
    create(&conn, &p, &titled("one")).unwrap();
    conn.execute("DELETE FROM projects WHERE id = 'p1'", [])
        .unwrap();
    assert!(list(&conn, &p).unwrap().is_empty());
}

#[test]
fn prefixes_are_derived_from_whatever_the_name_offers() {
    assert_eq!(derive_prefix("fletch"), "FLE");
    assert_eq!(derive_prefix("my-cool-app"), "MCA");
    assert_eq!(derive_prefix("A very long product name here"), "AVLP");
    assert_eq!(derive_prefix("q"), "PRJ", "one letter is not a prefix");
    assert_eq!(derive_prefix("  "), "PRJ");
    assert_eq!(
        derive_prefix("🚀"),
        "PRJ",
        "non-ascii yields nothing usable"
    );
    assert_eq!(derive_prefix("2fa"), "2FA");
}
