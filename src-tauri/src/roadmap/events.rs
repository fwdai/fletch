//! Durable item history: the `roadmap_item_events` DAO (migration 0030).
//!
//! Why this exists: everything else that explains an item's movement
//! evaporates. The queue note is re-derived every tick and never stored, a
//! toast dies with the render, and a chat line is buried in a transcript no
//! surface re-reads. But "why did this run fail", "when did this ship"
//! (`done_at` is the `shipped` event's timestamp) and "who ruled on this" are
//! facts the decision cards, PM oversight digests, and standup all hang off —
//! so every status transition writes exactly one row here, in the same
//! connection-lock scope as the item write it describes, and the frontend
//! follows along on `roadmap:item-event`.
//!
//! What deliberately does *not* land here: the drainer's re-derived-every-tick
//! conditions (waiting on deps, no workflow, no repo). Persisting one of those
//! per tick would bury the transitions in noise; they stay on the transient
//! `roadmap:queue-note` channel. Only failures and transitions persist.
//!
//! Like `roadmap_items`, the table is absent from the generic CRUD allow-list:
//! an event that didn't ride a typed write path could disagree with the
//! transition it claims to record.

use rusqlite::{params, Connection, Row};
use serde::Serialize;

use super::types::{enum_col, ItemStatus};
use crate::database::now_millis;

crate::db_enum! {
    /// Who moved the item. The four writers of `roadmap_items`, by surface:
    /// the typed commands (`user`), the propose RPC (`pm`), the queue drainer
    /// (`drainer`), and the merge sweep (`sweep`).
    EventActor {
        User    => "user",
        Pm      => "pm",
        Drainer => "drainer",
        Sweep   => "sweep",
    }
}

crate::db_enum! {
    /// What happened. One kind per transition, so a history line never has to
    /// re-derive meaning from a status pair.
    ///
    /// `held` and `released` arrive with the holds slice (B5, see
    /// .context/roadmap-pm-plan.md) — not declared here until they have a writer.
    EventKind {
        Proposed   => "proposed",
        Accepted   => "accepted",
        Discarded  => "discarded",
        Edited     => "edited",
        Queued     => "queued",
        Unqueued   => "unqueued",
        Dispatched => "dispatched",
        PrOpened   => "pr_opened",
        RunFailed  => "run_failed",
        Shipped    => "shipped",
        Abandoned  => "abandoned",
        Blocked    => "blocked",
        Note       => "note",
    }
}

/// One history row, as the frontend sees it (`roadmap:item-event` and
/// `roadmap_list_item_events` carry the same shape).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ItemEvent {
    pub id: String,
    pub item_id: String,
    /// Denormalized off the item so board-scoped listeners filter without a join.
    pub project_id: String,
    pub actor: EventActor,
    pub kind: EventKind,
    /// Human-readable payload: a failure reason, a PR url, a workflow id.
    pub detail: Option<String>,
    pub created_at: i64,
}

const COLUMNS: &str = "id, item_id, project_id, actor, kind, detail, created_at";

impl ItemEvent {
    fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: r.get("id")?,
            item_id: r.get("item_id")?,
            project_id: r.get("project_id")?,
            actor: enum_col(r, "actor", EventActor::from_db)?,
            kind: enum_col(r, "kind", EventKind::from_db)?,
            detail: r.get("detail")?,
            created_at: r.get("created_at")?,
        })
    }
}

/// Append one event and return the stored row, so the caller can emit it after
/// the connection lock drops. Must be called with the lock held, in the same
/// guard scope as the item write it records — that is what keeps the history
/// and the row it describes from ever disagreeing.
pub fn record(
    conn: &Connection,
    item_id: &str,
    project_id: &str,
    actor: EventActor,
    kind: EventKind,
    detail: Option<&str>,
) -> rusqlite::Result<ItemEvent> {
    let id = uuid::Uuid::new_v4().to_string();
    let created_at = now_millis();
    conn.execute(
        &format!("INSERT INTO roadmap_item_events ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"),
        params![
            id,
            item_id,
            project_id,
            actor.as_str(),
            kind.as_str(),
            detail,
            created_at
        ],
    )?;
    Ok(ItemEvent {
        id,
        item_id: item_id.to_string(),
        project_id: project_id.to_string(),
        actor,
        kind,
        detail: detail.map(str::to_string),
        created_at,
    })
}

/// One item's history, newest first — the order the card's disclosure renders.
/// `rowid` breaks ties so two events in the same millisecond keep write order.
pub fn list_for_item(conn: &Connection, item_id: &str) -> rusqlite::Result<Vec<ItemEvent>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM roadmap_item_events WHERE item_id = ?1
          ORDER BY created_at DESC, rowid DESC"
    ))?;
    let rows = stmt.query_map([item_id], ItemEvent::from_row)?;
    rows.collect()
}

/// The history kind a user patch implies, from the transition it performs.
///
/// The typed commands express a transition as `expect_status` (where the row
/// must still be) plus `patch.status` (where it goes), so the four named
/// transitions the frontend performs are recognized from exactly those two —
/// and every other applied patch is an `edited`, including an unconditional one
/// that happens to move status (no UI does that today).
pub(crate) fn transition_kind(expected: Option<ItemStatus>, to: Option<ItemStatus>) -> EventKind {
    match (expected, to) {
        (Some(ItemStatus::Proposed), Some(ItemStatus::Open)) => EventKind::Accepted,
        (Some(ItemStatus::Open), Some(ItemStatus::Queued)) => EventKind::Queued,
        (Some(ItemStatus::Queued), Some(ItemStatus::Open)) => EventKind::Unqueued,
        (Some(ItemStatus::InReview), Some(ItemStatus::Done)) => EventKind::Shipped,
        _ => EventKind::Edited,
    }
}

#[cfg(test)]
mod tests {
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
        // Newest first: the card's inline line is `listed[0]`.
        assert_eq!(listed, vec![second.clone(), first]);
        assert_eq!(listed[0].detail.as_deref(), Some("its run failed"));
        assert_eq!(second.actor, EventActor::Drainer);
        assert_eq!(second.kind, EventKind::RunFailed);
    }

    #[test]
    fn deleting_an_item_takes_its_history() {
        // Deletion writes no event on purpose: a deleted item was ruled off the
        // board, and history for a row nothing can render is a leak, not a record.
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
    fn the_user_transitions_map_to_their_kinds() {
        use ItemStatus::{Done, InReview, Open, Proposed, Queued};
        let cases = [
            (Some(Proposed), Some(Open), EventKind::Accepted),
            (Some(Open), Some(Queued), EventKind::Queued),
            (Some(Queued), Some(Open), EventKind::Unqueued),
            (Some(InReview), Some(Done), EventKind::Shipped),
            // A form edit: no precondition, no status move.
            (None, None, EventKind::Edited),
            // A conditional edit that isn't one of the four transitions.
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
}
