//! Durable item history (`roadmap_item_events`, migration 0030).
//!
//! Standing conditions persist; self-resolving wait-on-dep stays on transient
//! `roadmap:queue-note` only. Write in the same lock scope as the item mutation.


use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;

use super::types::{enum_col, ItemStatus};
use crate::database::now_millis;

crate::db_enum! {
    EventActor {
        User    => "user",
        Pm      => "pm",
        Drainer => "drainer",
        Sweep   => "sweep",
    }
}

crate::db_enum! {
    EventKind {
        Created     => "created",
        Proposed    => "proposed",
        Accepted    => "accepted",
        Edited      => "edited",
        Queued      => "queued",
        Unqueued    => "unqueued",
        Dispatched  => "dispatched",
        PrOpened    => "pr_opened",
        RunFailed   => "run_failed",
        RunCanceled => "run_canceled",
        RunDeleted  => "run_deleted",
        Shipped     => "shipped",
        PrClosed    => "pr_closed",
        Blocked     => "blocked",
        Held        => "held",
        Released    => "released",
        Rejected    => "rejected",
        Reopened    => "reopened",
        Note        => "note",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ItemEvent {
    pub id: String,
    pub item_id: String,
    pub project_id: String,
    pub actor: EventActor,
    pub kind: EventKind,
    pub detail: Option<String>,
    pub created_at: i64,
}

const COLUMNS: &str = "id, item_id, project_id, actor, kind, detail, created_at";

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

pub fn list_for_item(conn: &Connection, item_id: &str) -> rusqlite::Result<Vec<ItemEvent>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM roadmap_item_events WHERE item_id = ?1
          ORDER BY created_at DESC, rowid DESC"
    ))?;
    let rows = stmt.query_map([item_id], ItemEvent::from_row)?;
    rows.collect()
}

pub fn latest_per_item(conn: &Connection, project_id: &str) -> rusqlite::Result<Vec<ItemEvent>> {
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

pub fn latest_by_item(
    conn: &Connection,
    project_id: &str,
) -> rusqlite::Result<HashMap<String, ItemEvent>> {
    Ok(latest_per_item(conn, project_id)?
        .into_iter()
        .map(|e| (e.item_id.clone(), e))
        .collect())
}

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
#[path = "tests/events.rs"]
mod tests;
