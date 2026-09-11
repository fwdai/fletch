
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

fn retitle(to: &str) -> ProposalPatch {
    ProposalPatch {
        title: Some(to.into()),
        ..Default::default()
    }
}

#[test]
fn a_second_proposal_replaces_the_first_and_keeps_its_id() {
    let conn = test_conn();
    let it = item(&conn);

    let first = upsert(
        &conn,
        "p1",
        &it.id,
        ProposalKind::Update,
        Some(&retitle("v1")),
        None,
    )
    .unwrap();
    let second = upsert(
        &conn,
        "p1",
        &it.id,
        ProposalKind::Discard,
        None,
        Some("obsolete"),
    )
    .unwrap();

    assert_eq!(second.id, first.id);
    assert_eq!(second.kind, ProposalKind::Discard);
    assert_eq!(second.patch, None);
    assert_eq!(second.note.as_deref(), Some("obsolete"));
    assert_eq!(list_for_project(&conn, "p1").unwrap(), vec![second]);
}

#[test]
fn the_stored_patch_keys_are_exactly_the_changed_fields() {
    let conn = test_conn();
    let it = item(&conn);
    let patch: ProposalPatch = serde_json::from_str(r#"{"title": "new", "area": null}"#).unwrap();
    assert_eq!(patch.fields(), vec!["title", "area"]);

    let stored = upsert(
        &conn,
        "p1",
        &it.id,
        ProposalKind::Update,
        Some(&patch),
        None,
    )
    .unwrap();
    let obj = stored.patch.as_ref().unwrap().as_object().unwrap();
    assert_eq!(obj.len(), 2);
    assert_eq!(obj["title"], "new");
    assert!(obj["area"].is_null());
    let back: ProposalPatch = serde_json::from_value(stored.patch.unwrap()).unwrap();
    assert_eq!(back.area, Some(None));
    assert_eq!(back.to_item_patch().area, Some(None));
}

#[test]
fn deleting_an_item_takes_its_pending_proposal() {
    let conn = test_conn();
    let it = item(&conn);
    upsert(
        &conn,
        "p1",
        &it.id,
        ProposalKind::Update,
        Some(&retitle("gone with the row")),
        None,
    )
    .unwrap();
    assert!(store::delete(&conn, &it.id).unwrap());
    assert!(list_for_project(&conn, "p1").unwrap().is_empty());
}
