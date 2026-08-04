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
    /// No variant without a writer. `held` and `released` arrive with the holds
    /// slice (B5, see .context/roadmap-pm-plan.md); `discarded` was declared and
    /// never written — discarding an item deletes the row (its history cascades
    /// away with it) and declining a PM proposal writes a `note` — so it is gone
    /// rather than left as a kind the frontend must label and nothing produces.
    EventKind {
        Created    => "created",
        Proposed   => "proposed",
        Accepted   => "accepted",
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

/// The newest event of every item on one project's board, newest first — one
/// read for a board-wide question, where [`list_for_item`] would be one query
/// per card (and the board only ever loads the trails of cards someone
/// expanded).
///
/// Two consumers, one query: the board's "Needs you" strip asks "is this item's
/// *latest* word `blocked`?" (a `blocked` event a later transition superseded is
/// history, not a decision), and the PM's `roadmap_list` projection quotes the
/// last line of every item's trail ([`latest_by_item`] is this keyed by item).
///
/// A window function rather than `MAX(created_at)` so the tiebreak is the same
/// `created_at DESC, rowid DESC` [`list_for_item`] uses: two events written in
/// the same millisecond must resolve to the one written last, not to either.
pub fn latest_per_item(conn: &Connection, project_id: &str) -> rusqlite::Result<Vec<ItemEvent>> {
    // `rowid` rides along so the outer ordering can tie-break same-millisecond
    // writes by write order too — the head of this list is "the newest event
    // anywhere on the board" (the standup digest reads it), and that must be
    // one row, not whichever item id sorts first.
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM (
           SELECT {COLUMNS}, rowid AS rid,
                  ROW_NUMBER() OVER (
                    PARTITION BY item_id ORDER BY created_at DESC, rowid DESC
                  ) AS rn
             FROM roadmap_item_events
            WHERE project_id = ?1
         )
          WHERE rn = 1
          ORDER BY created_at DESC, rid DESC"
    ))?;
    let rows = stmt.query_map([project_id], ItemEvent::from_row)?;
    rows.collect()
}

/// [`latest_per_item`], keyed by `item_id` — the shape a per-row lookup wants.
pub fn latest_by_item(
    conn: &Connection,
    project_id: &str,
) -> rusqlite::Result<HashMap<String, ItemEvent>> {
    Ok(latest_per_item(conn, project_id)?
        .into_iter()
        .map(|e| (e.item_id.clone(), e))
        .collect())
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
        // Board-wide: the list is newest-first, so its head is the newest write
        // anywhere — the fact the standup digest reads off element zero.
        assert_eq!(latest_per_item(&conn, "p1").unwrap().first(), Some(&b_note));

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
        assert!(latest_per_item(&conn, "p2").unwrap().is_empty());
        assert!(latest_by_item(&conn, "p2").unwrap().is_empty());
    }

    #[test]
    fn latest_per_item_returns_the_newest_row_for_every_item() {
        // The strip's question is "what does this item's trail say *now*": an
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
            // tiebreak makes this the newest.
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
        // The strip is per project; another project's wedge is not this board's
        // decision (and the item id wouldn't resolve to a row here anyway).
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
