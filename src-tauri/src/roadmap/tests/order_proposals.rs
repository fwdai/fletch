    use super::*;
    use crate::database::get_migrations;

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

    fn codes(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn a_newer_ask_replaces_the_pending_one() {
        let conn = test_conn();
        upsert(&conn, "p1", &codes(&["FLE-100", "FLE-101"]), Some("first")).unwrap();
        let second = upsert(&conn, "p1", &codes(&["FLE-101", "FLE-100"]), None).unwrap();

        assert_eq!(get(&conn, "p1").unwrap(), Some(second.clone()));
        assert_eq!(second.codes, codes(&["FLE-101", "FLE-100"]));
        assert_eq!(
            second.note, None,
            "the replacement's note wins, blank or not"
        );
    }

    #[test]
    fn an_ask_round_trips_and_deletes_once() {
        let conn = test_conn();
        assert!(get(&conn, "p1").unwrap().is_none());
        let stored = upsert(&conn, "p1", &codes(&["FLE-100"]), Some("reordered")).unwrap();
        assert_eq!(stored.project_id, "p1");
        assert_eq!(stored.note.as_deref(), Some("reordered"));

        assert!(delete(&conn, "p1").unwrap());
        assert!(!delete(&conn, "p1").unwrap(), "second delete is a no-op");
        assert!(get(&conn, "p1").unwrap().is_none());
    }

    fn item(conn: &Connection, status: ItemStatus) -> RoadmapItem {
        crate::roadmap::store::create(
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

    /// names the offending code, because the PM's only way to fix its list is
    #[test]
    fn only_the_exact_orderable_set_resolves() {
        let conn = test_conn();
        let a = item(&conn, ItemStatus::Open); // FLE-100
        let b = item(&conn, ItemStatus::Queued); // FLE-101
        let c = item(&conn, ItemStatus::Proposed); // FLE-102
        let building = item(&conn, ItemStatus::Active); // FLE-103
        let shipped = item(&conn, ItemStatus::Done); // FLE-104
        let ruled_off = item(&conn, ItemStatus::Rejected); // FLE-105
        let items = crate::roadmap::store::list(&conn, "p1").unwrap();

        assert_eq!(
            validate_order(&codes(&[&c.code, &a.code, &b.code]), &items).unwrap(),
            vec![c.id.clone(), a.id.clone(), b.id.clone()]
        );

        for (ask, needle) in [
            (codes(&[]), "must list every orderable item"),
            (codes(&[&a.code, &b.code]), "FLE-102"),
            (
                codes(&[&a.code, &b.code, &c.code, "FLE-999"]),
                "not an item on this board",
            ),
            (
                codes(&[&a.code, &b.code, &c.code, &building.code]),
                "FLE-103 is active",
            ),
            (
                codes(&[&a.code, &b.code, &c.code, &shipped.code]),
                "FLE-104 is done",
            ),
            // refusal must say "rejected", not claim it was dispatched.
            (
                codes(&[&a.code, &b.code, &c.code, &ruled_off.code]),
                "FLE-105 is rejected",
            ),
            (
                codes(&[&a.code, &b.code, &c.code, &ruled_off.code]),
                "rejected items have no place",
            ),
            (
                codes(&[&a.code, &a.code, &b.code, &c.code]),
                "appears twice",
            ),
        ] {
            let e = validate_order(&ask, &items).expect_err("should have been refused");
            assert!(e.contains(needle), "expected {needle:?} in {e:?}");
        }
    }

    #[test]
    fn deleting_a_project_takes_its_pending_order_ask() {
        let conn = test_conn();
        upsert(&conn, "p1", &codes(&["FLE-100"]), None).unwrap();
        conn.execute("DELETE FROM projects WHERE id = 'p1'", [])
            .unwrap();
        assert!(get(&conn, "p1").unwrap().is_none());
    }
