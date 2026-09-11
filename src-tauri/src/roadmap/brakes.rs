//! Autonomy brakes at item and project scope (migration 0033).
//!
//! PM may place a hold; only typed user commands can release (invariant 2).
//! Item holds live on the row; project holds are their own table.

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;

use super::events::{self, EventActor, EventKind, ItemEvent};
use super::types::{enum_col, RoadmapItem};
use crate::database::now_millis;

/// Max reason length for holds and reject rulings (chars, not bytes).
pub const MAX_REASON: usize = 300;

/// Which user-facing copy [`clean_reason_for`] should use.
#[derive(Debug, Clone, Copy)]
pub enum ReasonKind {
    Hold,
    Reject,
}

/// Non-empty trimmed reason under [`MAX_REASON`] (hold wording).
pub fn clean_reason(reason: &str) -> Result<String, String> {
    clean_reason_for(reason, ReasonKind::Hold)
}

/// Shared trim / empty / length gate; messages depend on [`ReasonKind`].
pub fn clean_reason_for(reason: &str, kind: ReasonKind) -> Result<String, String> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(match kind {
            ReasonKind::Hold => {
                "`reason` is required — say what has to be agreed before this moves".into()
            }
            ReasonKind::Reject => {
                "`reason` is required — the rejected row is the decision log, and the reason \
                 is the decision"
                    .into()
            }
        });
    }
    let length = reason.chars().count();
    if length > MAX_REASON {
        return Err(match kind {
            ReasonKind::Hold => format!(
                "`reason` is {length} characters — keep it under {MAX_REASON}. Say what has to be \
                 agreed; the argument for it belongs in the conversation"
            ),
            ReasonKind::Reject => format!(
                "`reason` is {length} characters — keep it under {MAX_REASON}. Say the ruling; the \
                 argument for it belongs in the conversation"
            ),
        });
    }
    Ok(reason.to_string())
}

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

/// Place hold and record `held` under the caller's lock.
pub(crate) fn hold_with_event(
    conn: &Connection,
    item_id: &str,
    reason: &str,
    by: EventActor,
) -> Result<(RoadmapItem, ItemEvent), String> {
    let item = hold_item(conn, item_id, reason, by)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| super::store::missing(item_id))?;
    let event = events::record(
        conn,
        &item.id,
        &item.project_id,
        by,
        EventKind::Held,
        Some(reason),
    )
    .map_err(|e| e.to_string())?;
    Ok((item, event))
}

/// Clear hold and record `released` when something was lifted.
pub(crate) fn release_with_event(
    conn: &Connection,
    item_id: &str,
) -> Result<(RoadmapItem, Option<ItemEvent>), String> {
    let (item, lifted) = release_item(conn, item_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| super::store::missing(item_id))?;
    let Some(lifted) = lifted else {
        return Ok((item, None));
    };
    let event = events::record(
        conn,
        &item.id,
        &item.project_id,
        EventActor::User,
        EventKind::Released,
        Some(&lifted),
    )
    .map_err(|e| e.to_string())?;
    Ok((item, Some(event)))
}

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

pub fn get_project(conn: &Connection, project_id: &str) -> rusqlite::Result<Option<ProjectHold>> {
    conn.query_row(
        &format!("SELECT {HOLD_COLUMNS} FROM roadmap_project_holds WHERE project_id = ?1"),
        [project_id],
        ProjectHold::from_row,
    )
    .optional()
}

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

pub fn release_project(conn: &Connection, project_id: &str) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "DELETE FROM roadmap_project_holds WHERE project_id = ?1",
        [project_id],
    )?;
    Ok(n > 0)
}

const UNREADABLE: &str = "the board's hold could not be read";

/// Fail-closed: item or project hold reason, if any.
pub fn gate(conn: &Connection, item: &RoadmapItem) -> Option<String> {
    item.hold_reason
        .clone()
        .or_else(|| project_gate(conn, &item.project_id))
}

pub fn project_gate(conn: &Connection, project_id: &str) -> Option<String> {
    match get_project(conn, project_id) {
        Ok(hold) => hold.map(|h| h.reason),
        Err(e) => {
            tracing::warn!(
                project_id,
                error = %e,
                "roadmap: cannot read the project hold — treating the board as held"
            );
            Some(UNREADABLE.to_string())
        }
    }
}

#[cfg(test)]
#[path = "tests/brakes.rs"]
mod tests;
