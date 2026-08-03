//! The PM's pending ask to reorder a board: the `roadmap_order_proposals` DAO
//! (migration 0032).
//!
//! Why this is its own table rather than a [`super::proposals`] row: that table
//! is *item* scoped — `item_id NOT NULL` is what cascades an ask away with its
//! item and what "one pending ask per item" is built on. An order ask targets
//! the whole board, so it would have to weaken exactly the column carrying that
//! meaning. Four columns keyed by project cost less than that.
//!
//! The stored `codes` are the *complete* order of the project's orderable items
//! (`proposed | open | queued`) — the op refuses a partial list, so the ask is
//! unambiguous: it IS the new backlog order. The set is re-validated when the
//! user rules, because the board moves while an ask is pending (the drainer
//! claims, the PM proposes more), and applying a stale sequence would reorder
//! against an order nobody saw.
//!
//! One pending ask per project; a newer one replaces it, the same way an item's
//! delta is replaced — the user rules on the PM's current position.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;

use super::types::{ItemStatus, RoadmapItem};
use crate::database::now_millis;

/// One pending order ask, as the frontend sees it (`roadmap:order-proposal` and
/// `roadmap_get_order_proposal` carry the same shape). Keyed by project: the
/// board holds at most one.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OrderProposal {
    pub project_id: String,
    /// Every orderable item's code, in the order the PM is asking for.
    pub codes: Vec<String>,
    /// The PM's one-line rationale, quoted on the board's bar.
    pub note: Option<String>,
    pub created_at: i64,
}

const COLUMNS: &str = "project_id, codes_json, note, created_at";

impl OrderProposal {
    fn from_row(r: &Row) -> rusqlite::Result<Self> {
        let raw: String = r.get("codes_json")?;
        let codes: Vec<String> = serde_json::from_str(&raw).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                format!("codes_json: {e}").into(),
            )
        })?;
        Ok(Self {
            project_id: r.get("project_id")?,
            codes,
            note: r.get("note")?,
            created_at: r.get("created_at")?,
        })
    }
}

/// May this item's position be part of a proposed order? The same three
/// statuses a delta may reshape ([`super::proposal_gate`]): an `active` or
/// `in_review` item's place in the queue is decided — it has already been
/// dispatched — and a `done` one left the board.
pub fn is_orderable(item: &RoadmapItem) -> bool {
    matches!(
        item.status,
        ItemStatus::Proposed | ItemStatus::Open | ItemStatus::Queued
    )
}

/// The board's orderable items, in board order (the input's order, which is
/// `store::list`'s rank order).
pub fn orderable(items: &[RoadmapItem]) -> Vec<&RoadmapItem> {
    items.iter().filter(|i| is_orderable(i)).collect()
}

/// Resolve an asked-for sequence to item ids, refusing anything that is not
/// *exactly* the orderable set — and naming what is wrong, so the PM can fix
/// its list in one round trip.
///
/// The exactness is the point: a partial list would be ambiguous (does the
/// unnamed item go first, last, or stay where it was?), and the ask has to mean
/// one thing both when the PM sends it and when the user rules on it, possibly
/// minutes later. That is also why this is shared: the ruling re-runs it against
/// a fresh board read, so a set that drifted refuses instead of applying an
/// order nobody proposed.
pub fn validate_order(codes: &[String], items: &[RoadmapItem]) -> Result<Vec<String>, String> {
    let open = orderable(items);
    let names = |list: &[&str]| list.join(", ");
    if codes.is_empty() {
        let all: Vec<&str> = open.iter().map(|i| i.code.as_str()).collect();
        return Err(format!(
            "`codes` must list every orderable item on this board, in the order you want them \
             built — that is {}",
            if all.is_empty() {
                "nothing: no item here is proposed, open, or queued".to_string()
            } else {
                names(&all)
            }
        ));
    }

    let by_code: HashMap<&str, &RoadmapItem> = items.iter().map(|i| (i.code.as_str(), i)).collect();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut ids = Vec::with_capacity(codes.len());
    let mut unknown: Vec<&str> = Vec::new();
    let mut closed: Vec<String> = Vec::new();
    for code in codes {
        if !seen.insert(code.as_str()) {
            return Err(format!(
                "{code} appears twice — list every code exactly once, in the order you want"
            ));
        }
        match by_code.get(code.as_str()) {
            None => unknown.push(code.as_str()),
            Some(item) if !is_orderable(item) => {
                closed.push(format!("{} is {}", item.code, item.status.as_str()));
            }
            Some(item) => ids.push(item.id.clone()),
        }
    }
    if !unknown.is_empty() {
        return Err(format!(
            "{} is not an item on this board — order only the codes `roadmap_list` returns",
            names(&unknown)
        ));
    }
    if !closed.is_empty() {
        return Err(format!(
            "{} — an item being built or reviewed has already been dispatched, so its place in \
             the order is settled; list only the proposed, open, and queued ones",
            closed.join(", ")
        ));
    }
    let missing: Vec<&str> = open
        .iter()
        .filter(|i| !seen.contains(i.code.as_str()))
        .map(|i| i.code.as_str())
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "the order must name every orderable item, and {} {} missing — send the whole \
             sequence, not the part you want moved",
            names(&missing),
            if missing.len() == 1 { "is" } else { "are" }
        ));
    }
    Ok(ids)
}

/// Store the project's pending order ask, replacing any it already has. Returns
/// the stored row for emitting after the lock drops. Must be called with the
/// connection lock held.
pub fn upsert(
    conn: &Connection,
    project_id: &str,
    codes: &[String],
    note: Option<&str>,
) -> rusqlite::Result<OrderProposal> {
    let codes_json = serde_json::to_string(codes)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    conn.execute(
        "INSERT INTO roadmap_order_proposals (project_id, codes_json, note, created_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(project_id) DO UPDATE SET
           codes_json = excluded.codes_json,
           note = excluded.note,
           created_at = excluded.created_at",
        params![project_id, codes_json, note, now_millis()],
    )?;
    get(conn, project_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

/// The project's pending order ask, if any — at most one by construction.
pub fn get(conn: &Connection, project_id: &str) -> rusqlite::Result<Option<OrderProposal>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM roadmap_order_proposals WHERE project_id = ?1"),
        [project_id],
        OrderProposal::from_row,
    )
    .optional()
}

/// Remove the ask — the ruling took it, or it went stale. Returns whether a row
/// was removed, so a caller doesn't announce a deletion that didn't happen.
pub fn delete(conn: &Connection, project_id: &str) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "DELETE FROM roadmap_order_proposals WHERE project_id = ?1",
        [project_id],
    )?;
    Ok(n > 0)
}

#[cfg(test)]
mod tests {
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

    /// A board row in `status`, straight through the DAO so the codes are real.
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

    /// The exact-set rule, one row per way an ask can be wrong. Each refusal
    /// names the offending code, because the PM's only way to fix its list is
    /// the error text.
    #[test]
    fn only_the_exact_orderable_set_resolves() {
        let conn = test_conn();
        let a = item(&conn, ItemStatus::Open); // FLE-100
        let b = item(&conn, ItemStatus::Queued); // FLE-101
        let c = item(&conn, ItemStatus::Proposed); // FLE-102
        let building = item(&conn, ItemStatus::Active); // FLE-103
        let shipped = item(&conn, ItemStatus::Done); // FLE-104
        let items = crate::roadmap::store::list(&conn, "p1").unwrap();

        // The whole orderable set, in an order of the PM's choosing: resolved to
        // ids in exactly that order, which is what the ruling ranks.
        assert_eq!(
            validate_order(&codes(&[&c.code, &a.code, &b.code]), &items).unwrap(),
            vec![c.id.clone(), a.id.clone(), b.id.clone()]
        );

        for (ask, needle) in [
            // Nothing at all.
            (codes(&[]), "must list every orderable item"),
            // One orderable item left out.
            (codes(&[&a.code, &b.code]), "FLE-102"),
            // A code that isn't on this board.
            (
                codes(&[&a.code, &b.code, &c.code, "FLE-999"]),
                "not an item on this board",
            ),
            // An item that has already been dispatched, or shipped.
            (
                codes(&[&a.code, &b.code, &c.code, &building.code]),
                "FLE-103 is active",
            ),
            (
                codes(&[&a.code, &b.code, &c.code, &shipped.code]),
                "FLE-104 is done",
            ),
            // The same code twice — an order that names a position twice is not
            // an order.
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
}
