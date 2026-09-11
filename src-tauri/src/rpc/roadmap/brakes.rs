use rusqlite::Connection;
use serde_json::{json, Value};

use crate::roadmap::brakes::{self, ProjectHold};
use crate::roadmap::events::{EventActor, ItemEvent};
use crate::roadmap::store;
use crate::roadmap::types::{ItemStatus, RoadmapItem};
use crate::rpc::Response;

use super::args::{parse_required, HoldArgs, PROJECT_SCOPE};

/// What a hold stopped, so the dispatcher can announce the right thing once the
/// lock drops: an item's hold rides its row (which carries the trio) plus the
/// `held` line, a project's is the table row itself.
pub(super) enum Held {
    Item(Box<RoadmapItem>, Box<ItemEvent>),
    Project(ProjectHold),
}

/// `roadmap_hold`: stop autonomous progress on one item, or on the whole board,
/// until the user signs off.
///
/// The PM's second (and last) direct write, and it is allowed for the same reason
/// the first is — the conservative direction of **invariant 2** (see
/// .context/roadmap-pm-plan.md): a hold can only ever *reduce* autonomy. It
/// dispatches nothing, queues nothing, edits nothing; it takes something the app
/// would have done on its own and makes it wait for a human. Every ask that would
/// *advance* state stays a proposal the user rules on.
///
/// The asymmetry is the safety property: there is **no release op**. Only the
/// typed commands (`roadmap_release_item` / `roadmap_release_project`) lift a
/// hold, so every release is a user action by construction and an agent can never
/// undo its own brake.
///
/// Like `roadmap_note`, the target may be at any *working* status: the moment a
/// hold is most worth placing is usually mid-run, on the `active` item whose PR
/// is about to answer the wrong question — exactly the item a proposal is
/// refused on. Only `rejected` is refused (see the gate below).
/// Holding an already-held scope replaces the reason and records another `held`,
/// so the trail keeps what was superseded.
pub(super) fn hold_op(
    conn: &Connection,
    project_id: &str,
    id: &str,
    args: &Value,
) -> (Response, Option<Held>) {
    let err = |msg: String| (Response::err(id, format!("roadmap_hold: {msg}")), None);
    let args: HoldArgs = match parse_required(args) {
        Ok(a) => a,
        Err(e) => return err(e),
    };
    let reason = match brakes::clean_reason(&args.reason) {
        Ok(reason) => reason,
        Err(e) => return err(e),
    };
    let scope = args.scope.trim();
    if scope.is_empty() {
        return err(format!(
            "`scope` is required — an item code, or {PROJECT_SCOPE:?} for the whole board"
        ));
    }

    if scope == PROJECT_SCOPE {
        // No item event: a board-wide stop belongs to no row (see
        // `roadmap::roadmap_hold_project`). The hold row is the durable record.
        let stored = match brakes::hold_project(conn, project_id, &reason, EventActor::Pm) {
            Ok(hold) => hold,
            Err(e) => return err(e.to_string()),
        };
        let payload = json!({ "held": { "scope": PROJECT_SCOPE } });
        return match serde_json::to_string(&payload) {
            Ok(stdout) => (
                Response::ok(id, 0, stdout, String::new()),
                Some(Held::Project(stored)),
            ),
            // The board is stopped either way: say so, and still announce it.
            Err(e) => (
                Response::err(id, format!("roadmap_hold: held, but {e}")),
                Some(Held::Project(stored)),
            ),
        };
    }

    let items = match store::list(conn, project_id) {
        Ok(items) => items,
        Err(e) => return err(e.to_string()),
    };
    let Some(item) = items.iter().find(|i| i.code == scope) else {
        return err(format!(
            "no item {scope:?} on this board — `roadmap_list` shows what exists, or use \
             {PROJECT_SCOPE:?} to hold the whole board"
        ));
    };
    // A rejected item has no queue to stop and no card with a Release button:
    // the hold would be invisible, and would ambush the user on reopen.
    if item.status == ItemStatus::Rejected {
        return err(format!(
            "{} was rejected — a ruled-off item has nothing to pause; ask the user to \
             reopen it first",
            item.code
        ));
    }
    // One write path for both doors: the command layer's `hold_item` places the
    // hold and records the `held` line in this same guard, so a held row can
    // never exist without the line saying who stopped it.
    let (item, event) = match crate::roadmap::hold_item(conn, &item.id, &reason, EventActor::Pm) {
        Ok(pair) => pair,
        Err(e) => return err(e),
    };

    let payload = json!({ "held": { "scope": item.code } });
    let held = Some(Held::Item(Box::new(item), Box::new(event)));
    match serde_json::to_string(&payload) {
        Ok(stdout) => (Response::ok(id, 0, stdout, String::new()), held),
        Err(e) => (
            Response::err(id, format!("roadmap_hold: held, but {e}")),
            held,
        ),
    }
}

#[cfg(test)]
#[path = "tests/brakes.rs"]
mod tests;
