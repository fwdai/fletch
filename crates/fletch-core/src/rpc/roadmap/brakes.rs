use rusqlite::Connection;
use serde_json::{json, Value};

use crate::roadmap::brakes::{self, ProjectHold};
use crate::roadmap::events::{EventActor, ItemEvent};
use crate::roadmap::store;
use crate::roadmap::types::{ItemStatus, RoadmapItem};
use crate::rpc::Response;

use super::args::{parse_required, wrote, HoldArgs, PROJECT_SCOPE};

pub(super) enum Held {
    Item(Box<RoadmapItem>, Box<ItemEvent>),
    Project(ProjectHold),
}

/// Only ever reduces autonomy, and there is no release op, so an agent can never
/// undo its own brake. Any working status may be held; only `rejected` is
/// refused. Re-holding replaces the reason and records another `held`.
pub(super) fn hold_op(
    conn: &Connection,
    project_id: &str,
    id: &str,
    args: &Value,
) -> (Response, Option<Held>) {
    wrote(
        id,
        "roadmap_hold",
        "held",
        place_hold(conn, project_id, args),
    )
}

fn place_hold(conn: &Connection, project_id: &str, args: &Value) -> Result<(Value, Held), String> {
    let args: HoldArgs = parse_required(args)?;
    let reason = brakes::clean_reason(&args.reason)?;
    let scope = args.scope.trim();
    if scope.is_empty() {
        return Err(format!(
            "`scope` is required — an item code, or {PROJECT_SCOPE:?} for the whole board"
        ));
    }

    if scope == PROJECT_SCOPE {
        // A board-wide stop belongs to no row; the hold row is the record.
        let stored = brakes::hold_project(conn, project_id, &reason, EventActor::Pm)
            .map_err(|e| e.to_string())?;
        return Ok((
            json!({ "held": { "scope": PROJECT_SCOPE } }),
            Held::Project(stored),
        ));
    }

    let items = store::list(conn, project_id).map_err(|e| e.to_string())?;
    let item = items.iter().find(|i| i.code == scope).ok_or_else(|| {
        format!(
            "no item {scope:?} on this board — `roadmap_list` shows what exists, or use \
             {PROJECT_SCOPE:?} to hold the whole board"
        )
    })?;
    // A rejected item has no queue to stop and no card with a Release button:
    // the hold would be invisible, and would ambush the user on reopen.
    if item.status == ItemStatus::Rejected {
        return Err(format!(
            "{} was rejected — a ruled-off item has nothing to pause; ask the user to \
             reopen it first",
            item.code
        ));
    }
    // `hold_item` records the `held` line in the same guard, so a held row never
    // exists without it.
    let (item, event) = crate::roadmap::hold_item(conn, &item.id, &reason, EventActor::Pm)?;
    Ok((
        json!({ "held": { "scope": item.code } }),
        Held::Item(Box::new(item), Box::new(event)),
    ))
}

#[cfg(test)]
#[path = "tests/brakes.rs"]
mod tests;
