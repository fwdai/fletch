//! Agent workspace assignments: hand off an item to a workspace, or reclaim it.

use rusqlite::Connection;

use super::events::{self, EventActor, EventKind, ItemEvent};
use super::store;
use super::types::{ItemPatch, ItemStatus, RoadmapItem};

/// Status untouched; workspace stamp under one lock.
pub(super) fn hand_off(
    conn: &Connection,
    item_id: &str,
    agent_id: &str,
) -> Result<(RoadmapItem, ItemEvent), String> {
    stamp(
        conn,
        item_id,
        Stamp::HandOff { agent_id },
        |item, status| {
            format!(
                "{} is {} — an item that is queued or already being built can't be handed \
                 to an agent; take it back to the board first",
                item.code,
                status.as_str()
            )
        },
        |name| match name {
            Some(name) => format!("Handed to agent {name}"),
            None => "Handed to an agent".to_string(),
        },
    )
}

/// Clear agent stamp only; status unchanged.
pub(super) fn reclaim(conn: &Connection, item_id: &str) -> Result<(RoadmapItem, ItemEvent), String> {
    stamp(
        conn,
        item_id,
        Stamp::Reclaim,
        |item, status| {
            format!(
                "{} is {} — it's already being built; deal with it from the run",
                item.code,
                status.as_str()
            )
        },
        |name| match name {
            Some(name) => format!("Taken back from agent {name}"),
            None => "Taken back from an agent".to_string(),
        },
    )
}

enum Stamp<'a> {
    HandOff { agent_id: &'a str },
    Reclaim,
}

fn stamp(
    conn: &Connection,
    item_id: &str,
    op: Stamp<'_>,
    refuse: impl FnOnce(&RoadmapItem, ItemStatus) -> String,
    detail: impl FnOnce(Option<&str>) -> String,
) -> Result<(RoadmapItem, ItemEvent), String> {
    let current = store::require(conn, item_id)?;
    let lookup_id = match &op {
        Stamp::HandOff { agent_id } => *agent_id,
        Stamp::Reclaim => current
            .agent_id
            .as_deref()
            .ok_or_else(|| format!("{} isn't with an agent", current.code))?,
    };
    match current.status {
        ItemStatus::Proposed | ItemStatus::Open => {}
        status => return Err(refuse(&current, status)),
    }
    let name: Option<String> = conn
        .query_row(
            "SELECT name FROM workspaces WHERE id = ?1",
            [lookup_id],
            |r| r.get(0),
        )
        .ok();
    let patch = ItemPatch {
        agent_id: Some(match &op {
            Stamp::HandOff { agent_id } => Some((*agent_id).to_string()),
            Stamp::Reclaim => None,
        }),
        ..Default::default()
    };
    let item = store::update(conn, item_id, &patch)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| store::missing(item_id))?;
    let detail = detail(name.as_deref());
    let event = events::record(
        conn,
        &item.id,
        &item.project_id,
        EventActor::User,
        EventKind::Note,
        Some(&detail),
    )
    .map_err(|e| e.to_string())?;
    Ok((item, event))
}
