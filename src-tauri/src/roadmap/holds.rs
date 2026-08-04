//! Holds: the brake on autonomous progress, at item and project scope
//! (migration 0033).
//!
//! Why a hold is not an unqueue: the queue drains without anyone watching, so
//! "stop, we need to agree on direction first" has to be sayable *with its
//! reason*, by whoever notices — including the PM agent, mid-review, while the
//! user is asleep. An unqueue is a status move that loses the reason and can only
//! be made by the user; a hold is a reason attached to a scope, and it is the one
//! direct write the PM is licensed for, precisely because it can only ever reduce
//! autonomy (invariant 2 in .context/roadmap-pm-plan.md).
//!
//! **Releasing is the user's alone.** There is no RPC op for it, and nothing in
//! this module is reachable from the agent surface except [`hold_item`] and
//! [`hold_project`]. That asymmetry is the whole safety property: an agent that
//! could lift its own brake has no brake.
//!
//! Two scopes, two storage shapes, one vocabulary:
//! - **Item**: three nullable columns on the row itself, so every reader that
//!   already holds the row (the drainer's queue filter, the card, the strip)
//!   needs no join. The row carries the *current* reason; the durable trail
//!   ([`super::events`], kinds `held`/`released`) carries the history of holds.
//! - **Project**: one [`ProjectHold`] row per project. A whole board being
//!   stopped is not a fact about any item, so it has no item to hang off — and it
//!   is what the drainer checks before dispatching anything at all.
//!
//! Both writes are validated through [`clean_reason`], so the PM's op and the
//! user's command cannot disagree about what a usable reason is.

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;

use super::events::EventActor;
use super::types::{enum_col, RoadmapItem};
use crate::database::now_millis;

/// Longest reason either scope will store. A hold's reason is a line on a card,
/// a line in a banner, and a line in the PM's next listing — past a couple of
/// sentences it stops being "why we stopped" and starts being the argument,
/// which belongs in the conversation.
pub const MAX_REASON: usize = 300;

/// Normalize a hold reason, or say exactly what's wrong with it.
///
/// A hold with no reason is the one hold that must not exist: the user's only way
/// out is the Release button, and a button whose card says nothing is a mystery
/// they have to reconstruct. Counted in characters, not bytes — the cap is about
/// how much a human will read, and a byte limit would refuse a shorter reason for
/// containing an em-dash.
pub fn clean_reason(reason: &str) -> Result<String, String> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err("`reason` is required — say what has to be agreed before this moves".into());
    }
    let length = reason.chars().count();
    if length > MAX_REASON {
        return Err(format!(
            "`reason` is {length} characters — keep it under {MAX_REASON}. Say what has to be \
             agreed; the argument for it belongs in the conversation"
        ));
    }
    Ok(reason.to_string())
}

// ───────────────────────────── item scope ───────────────────────────────

/// Place (or replace) an item's hold and return the stored row, or `None` when
/// the row is gone. Must be called with the connection lock held, in the same
/// guard as the `held` event that records it.
///
/// Replacing rather than refusing a second hold: the second reason is the current
/// one — the PM found a better way to say it, or a new problem superseded the old
/// — and the trail keeps the line that is being replaced.
///
/// Deliberately its own SQL rather than an [`super::types::ItemPatch`] field: the
/// hold columns are absent from the patch surface, so no generic edit (and in
/// particular no PM patch, whose whole point is that it cannot advance state) can
/// stop or start the queue as a side effect.
pub fn hold_item(
    conn: &Connection,
    item_id: &str,
    reason: &str,
    by: EventActor,
) -> rusqlite::Result<Option<RoadmapItem>> {
    let now = now_millis();
    conn.execute(
        "UPDATE roadmap_items
            SET hold_reason = ?1, held_by = ?2, held_at = ?3, updated_at = ?3
          WHERE id = ?4",
        params![reason, by.as_str(), now, item_id],
    )?;
    super::store::get(conn, item_id)
}

/// Lift an item's hold, returning the stored row and the reason that was lifted —
/// the detail the `released` event carries, so the trail says what was resolved
/// rather than only that something was.
///
/// `None` when the row is gone. A row that wasn't held comes back with `None` for
/// the reason and is otherwise untouched: the caller's intent ("this should not be
/// held") is satisfied either way, and inventing an error for it would make the
/// strip's one-click release fail on a hold someone else lifted a moment earlier.
pub fn release_item(
    conn: &Connection,
    item_id: &str,
) -> rusqlite::Result<Option<(RoadmapItem, Option<String>)>> {
    let Some(current) = super::store::get(conn, item_id)? else {
        return Ok(None);
    };
    let lifted = current.hold_reason.clone();
    if lifted.is_none() {
        return Ok(Some((current, None)));
    }
    conn.execute(
        "UPDATE roadmap_items
            SET hold_reason = NULL, held_by = NULL, held_at = NULL, updated_at = ?1
          WHERE id = ?2",
        params![now_millis(), item_id],
    )?;
    Ok(super::store::get(conn, item_id)?.map(|row| (row, lifted)))
}

// ─────────────────────────── project scope ──────────────────────────────

/// One `roadmap_project_holds` row as the frontend sees it — the whole board
/// stopped, with the reason and who stopped it.
///
/// No item id: a project hold is a fact about the board, which is why it has its
/// own table and its own events (`roadmap:project-hold` /
/// `roadmap:project-hold-released`) rather than riding `roadmap:item`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProjectHold {
    pub project_id: String,
    pub reason: String,
    pub held_by: EventActor,
    pub created_at: i64,
}

const HOLD_COLUMNS: &str = "project_id, reason, held_by, created_at";

impl ProjectHold {
    fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            project_id: r.get("project_id")?,
            reason: r.get("reason")?,
            held_by: enum_col(r, "held_by", EventActor::from_db)?,
            created_at: r.get("created_at")?,
        })
    }
}

/// The project's hold, or `None` when the board is running.
pub fn get_project(conn: &Connection, project_id: &str) -> rusqlite::Result<Option<ProjectHold>> {
    conn.query_row(
        &format!("SELECT {HOLD_COLUMNS} FROM roadmap_project_holds WHERE project_id = ?1"),
        [project_id],
        ProjectHold::from_row,
    )
    .optional()
}

/// Place (or replace) the project's hold and return the stored row. One per
/// board, same reasoning as an item's: the newer reason is the current one.
///
/// `created_at` is rewritten by a replacement on purpose — the banner says "held
/// 20m ago" about the reason it is showing, and carrying the first hold's
/// timestamp under a reason written since would date the wrong fact.
pub fn hold_project(
    conn: &Connection,
    project_id: &str,
    reason: &str,
    by: EventActor,
) -> rusqlite::Result<ProjectHold> {
    let created_at = now_millis();
    conn.execute(
        &format!(
            "INSERT INTO roadmap_project_holds ({HOLD_COLUMNS}) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(project_id) DO UPDATE SET
               reason = excluded.reason,
               held_by = excluded.held_by,
               created_at = excluded.created_at"
        ),
        params![project_id, reason, by.as_str(), created_at],
    )?;
    Ok(ProjectHold {
        project_id: project_id.to_string(),
        reason: reason.to_string(),
        held_by: by,
        created_at,
    })
}

/// Lift the project's hold. Reports whether a row was actually removed, so the
/// caller doesn't announce a release that didn't happen.
pub fn release_project(conn: &Connection, project_id: &str) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "DELETE FROM roadmap_project_holds WHERE project_id = ?1",
        [project_id],
    )?;
    Ok(n > 0)
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
    /// refusal for going *over* it, not for reaching it.
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

    /// An item hold is the trio, and a release clears all three and hands back
    /// the reason it lifted (the `released` event's detail).
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
        // The status is untouched: a hold stops progress, it doesn't move the row.
        assert_eq!(held.status, it.status);

        let (released, lifted) = release_item(&conn, &it.id).unwrap().unwrap();
        assert_eq!(lifted.as_deref(), Some("direction unclear"));
        assert!(!released.is_held());
        assert_eq!(released.held_by, None);
        assert_eq!(released.held_at, None);
    }

    /// A second hold replaces the reason rather than refusing: the newer one is
    /// the current one. (The trail keeps both — see `roadmap::mod`'s tests.)
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

    /// Releasing something that isn't held is a no-op that still reports the row —
    /// the strip's one-click release must not fail because someone else got there
    /// first. A row that is gone is `None`, not an error.
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

    /// One hold per board, newer replacing older, and it goes away with the
    /// project it belongs to.
    #[test]
    fn a_project_hold_round_trips_replaces_and_cascades() {
        let conn = test_conn();
        assert!(get_project(&conn, "p1").unwrap().is_none());

        let held = hold_project(&conn, "p1", "re-planning the quarter", EventActor::Pm).unwrap();
        assert_eq!(get_project(&conn, "p1").unwrap(), Some(held.clone()));
        assert_eq!(held.held_by, EventActor::Pm);

        let again =
            hold_project(&conn, "p1", "waiting on the design call", EventActor::User).unwrap();
        assert_eq!(get_project(&conn, "p1").unwrap(), Some(again.clone()));
        assert_eq!(again.reason, "waiting on the design call");
        assert!(again.created_at >= held.created_at);

        assert!(release_project(&conn, "p1").unwrap());
        assert!(get_project(&conn, "p1").unwrap().is_none());
        assert!(
            !release_project(&conn, "p1").unwrap(),
            "a second release removes nothing"
        );

        // A deleted project can't leave a hold nothing can release.
        hold_project(&conn, "p1", "again", EventActor::User).unwrap();
        conn.execute("DELETE FROM projects WHERE id = 'p1'", [])
            .unwrap();
        assert!(get_project(&conn, "p1").unwrap().is_none());
    }

    /// Another board's hold is invisible: the drainer checks per project, and a
    /// hold that leaked across projects would freeze a queue nobody stopped.
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
}
