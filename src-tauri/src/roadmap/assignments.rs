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
    let current = store::get(conn, item_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("roadmap item {item_id} no longer exists"))?;
    match current.status {
        ItemStatus::Proposed | ItemStatus::Open => {}
        status => {
            return Err(format!(
                "{} is {} — an item that is queued or already being built can't be handed \
                 to an agent; take it back to the board first",
                current.code,
                status.as_str()
            ))
        }
    }
    let name: Option<String> = conn
        .query_row(
            "SELECT name FROM workspaces WHERE id = ?1",
            [agent_id],
            |r| r.get(0),
        )
        .ok();
    let patch = ItemPatch {
        agent_id: Some(Some(agent_id.to_string())),
        ..Default::default()
    };
    let item = store::update(conn, item_id, &patch)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("roadmap item {item_id} no longer exists"))?;
    let detail = match &name {
        Some(name) => format!("Handed to agent {name}"),
        None => "Handed to an agent".to_string(),
    };
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

/// Clear agent stamp only; status unchanged.
pub(super) fn reclaim(conn: &Connection, item_id: &str) -> Result<(RoadmapItem, ItemEvent), String> {
    let current = store::get(conn, item_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("roadmap item {item_id} no longer exists"))?;
    let Some(agent_id) = current.agent_id.clone() else {
        return Err(format!("{} isn't with an agent", current.code));
    };
    match current.status {
        ItemStatus::Proposed | ItemStatus::Open => {}
        status => {
            return Err(format!(
                "{} is {} — it's already being built; deal with it from the run",
                current.code,
                status.as_str()
            ))
        }
    }
    let name: Option<String> = conn
        .query_row(
            "SELECT name FROM workspaces WHERE id = ?1",
            [&agent_id],
            |r| r.get(0),
        )
        .ok();
    let item = store::update(
        conn,
        item_id,
        &ItemPatch {
            agent_id: Some(None),
            ..Default::default()
        },
    )
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("roadmap item {item_id} no longer exists"))?;
    let detail = match &name {
        Some(name) => format!("Taken back from agent {name}"),
        None => "Taken back from an agent".to_string(),
    };
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
