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

use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension, Row};
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
        Created    => "created",
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

/// What one transition writes to the trail: who moved the item, what happened,
/// and the human-readable payload. Travels as one value so a write helper's
/// signature names the transition rather than three loose fields.
pub(crate) struct TrailEntry {
    pub actor: EventActor,
    pub kind: EventKind,
    pub detail: Option<String>,
}

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

/// Every item's newest event on one board, keyed by `item_id`.
///
/// The card renders an item's whole trail ([`list_for_item`]); the PM's
/// projection only ever needs the last line of each — so this is one statement
/// for the whole board rather than a query per row, and it never pulls a
/// hundred rows to look at one.
///
/// The window function is what makes "newest per item" one pass; the ordering
/// inside it is [`list_for_item`]'s, so the line the PM is shown and the line at
/// the top of the card's trail are the same row even for two events written in
/// the same millisecond.
pub fn latest_by_item(
    conn: &Connection,
    project_id: &str,
) -> rusqlite::Result<HashMap<String, ItemEvent>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM (
           SELECT {COLUMNS}, ROW_NUMBER() OVER (
             PARTITION BY item_id ORDER BY created_at DESC, rowid DESC
           ) AS rn
           FROM roadmap_item_events WHERE project_id = ?1
         ) WHERE rn = 1"
    ))?;
    let rows = stmt.query_map([project_id], ItemEvent::from_row)?;
    rows.map(|r| r.map(|e| (e.item_id.clone(), e)))
        .collect::<rusqlite::Result<HashMap<_, _>>>()
}

/// The newest event anywhere on a board — "when did this board last move".
///
/// The standup digest compares exactly this against the PM chat's last turn: if
/// nothing has happened since the two of you spoke, there is nothing to
/// summarize, and asking for a digest anyway trains the user to ignore them.
pub fn latest_for_project(
    conn: &Connection,
    project_id: &str,
) -> rusqlite::Result<Option<ItemEvent>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM roadmap_item_events WHERE project_id = ?1
          ORDER BY created_at DESC, rowid DESC LIMIT 1"
    ))?;
    let mut rows = stmt.query_map([project_id], ItemEvent::from_row)?;
    rows.next().transpose()
}

/// The item's newest event, or `None` for an item with no history yet.
///
/// One row rather than the whole trail because of the one caller that needs it:
/// the drainer's wedged-queue check, which writes a `blocked` line only when the
/// last thing said about the item isn't already that same line (see
/// [`super::drainer::record_wedge`]). Same ordering as [`list_for_item`], so
/// "newest" means one thing in both.
pub fn latest_for_item(conn: &Connection, item_id: &str) -> rusqlite::Result<Option<ItemEvent>> {
    conn.query_row(
        &format!(
            "SELECT {COLUMNS} FROM roadmap_item_events WHERE item_id = ?1
              ORDER BY created_at DESC, rowid DESC LIMIT 1"
        ),
        [item_id],
        ItemEvent::from_row,
    )
    .optional()
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

    /// Both "newest" reads agree with the head of the trail: the per-item map
    /// `roadmap_list` projects from, and the board-wide one the standup compares
    /// against. Same-millisecond writes tie-break on write order, which is what
    /// keeps the PM's `last_event` and the card's top line the same row.
    #[test]
    fn the_newest_reads_agree_with_the_head_of_the_trail() {
        let conn = test_conn();
        let a = item(&conn);
        let b = item(&conn);
        assert!(latest_for_project(&conn, "p1").unwrap().is_none());
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
        // Board-wide: the newest write anywhere, whichever item it landed on.
        assert_eq!(
            latest_for_project(&conn, "p1").unwrap(),
            Some(b_note.clone())
        );

        let by_item = latest_by_item(&conn, "p1").unwrap();
        assert_eq!(by_item.len(), 2);
        assert_eq!(by_item.get(&a.id), Some(&a_head));
        assert_eq!(by_item.get(&b.id), Some(&b_note));

        // Another project's history is invisible to both board-scoped reads.
        conn.execute(
            "INSERT INTO projects (id, name, created_at) VALUES ('p2', 'other', 0)",
            [],
        )
        .unwrap();
        assert!(latest_for_project(&conn, "p2").unwrap().is_none());
        assert!(latest_by_item(&conn, "p2").unwrap().is_empty());
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
