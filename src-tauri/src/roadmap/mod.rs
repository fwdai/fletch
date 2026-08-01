//! The project roadmap: the per-project list of items the Roadmap tab renders.
//!
//! Layout mirrors the workflow module: [`types`] is the row and its enums,
//! [`store`] is the DAO (called with the connection lock held), and this file is
//! the typed `#[tauri::command]` surface plus the row-level events the frontend
//! syncs on.
//!
//! `roadmap_items` is deliberately absent from the generic CRUD allow-list
//! (`database::validate`), exactly like the `wf_*` tables. Codes must be
//! allocated under the connection lock, the `*_json` columns must be marshalled,
//! and every mutation must announce itself — a frontend `db_insert` would skip
//! all three.
//!
//! Events (all best-effort; a failed emit never affects what was persisted):
//! - `roadmap:item` — the full row, on every create/update. The frontend upserts
//!   by `id`, the same shape `wf:run` uses, so any writer (this surface, the PM
//!   agent's RPC, the queue drainer) updates the board without a refetch.
//! - `roadmap:item-deleted` — the deleted row's id, so the board drops it
//!   instead of upserting it.
//! - `roadmap:queue-note` — transient, from [`drainer`]: why a queued item isn't
//!   moving. Nothing persists it; see that module's docs for why.
//!
//! Autonomous dispatch lives in [`drainer`]: `queued` items become running
//! workflows there, and every mutation on this surface [`drainer::nudge`]s it so
//! a queue action doesn't wait out the tick interval.
//!
//! [`merge_sweep`] closes the loop at the other end: it watches the PRs of
//! `in_review` items and moves them to `done` when they merge on GitHub. It is
//! host-side rather than in the webview precisely so a queue keeps draining
//! with the window shut.

pub mod drainer;
pub mod merge_sweep;
pub mod store;
pub mod types;

use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::Connection;
use tauri::{AppHandle, Emitter};

use types::{ItemPatch, ItemStatus, ItemUpdate, NewItem, RoadmapItem};

/// The app's single connection. Public because the PM agent's RPC dispatcher
/// (`rpc::roadmap`) writes the same table from outside this module and takes
/// the same handle.
pub type Db = Arc<Mutex<Connection>>;

/// Notify the frontend that an item row changed; carries the full row.
pub(crate) fn emit_item(app: &AppHandle, item: &RoadmapItem) {
    let _ = app.emit("roadmap:item", item);
}

/// Notify the frontend that an item was deleted, so the board drops the row.
fn emit_item_deleted(app: &AppHandle, id: &str) {
    let _ = app.emit("roadmap:item-deleted", id);
}

/// Every item on a project's roadmap, oldest first. Includes `done` items — the
/// board hides them from the horizons and counts them as "shipped".
#[tauri::command]
pub async fn roadmap_list_items(
    project_id: String,
    db: tauri::State<'_, Db>,
) -> Result<Vec<RoadmapItem>, String> {
    let conn = db.lock();
    store::list(&conn, &project_id).map_err(|e| e.to_string())
}

/// Add an item to a project's roadmap. The `code` is allocated here, not passed
/// in: it's the item's identity for the rest of its life, and only the DB knows
/// which numbers are taken.
#[tauri::command]
pub async fn roadmap_create_item(
    project_id: String,
    item: NewItem,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<RoadmapItem, String> {
    if item.title.trim().is_empty() {
        return Err("a roadmap item needs a title".into());
    }
    let created = {
        let conn = db.lock();
        store::create(&conn, &project_id, &item).map_err(|e| e.to_string())?
    };
    emit_item(&app, &created);
    // A row can arrive already `queued` (or as a dependency another queued item
    // is waiting on), so every mutation re-checks the queue rather than trying
    // to guess which ones matter.
    drainer::nudge();
    Ok(created)
}

/// Patch an item. Absent fields are left alone; an explicit `null` clears a
/// nullable one (see [`ItemPatch`]). `code` and `project_id` are not patchable —
/// a code that moved would break every reference to it.
///
/// `expect_status` makes the write *conditional*: the patch lands only while the
/// row still says that, and a miss returns `applied: false` with the row as it
/// actually is — nothing written, nothing emitted. That is how a status
/// transition sent from a client snapshot stays safe against the drainer, which
/// claims `queued → active` under this same lock: an unqueue that arrives a
/// moment late is dropped instead of flipping a live run's item back onto the
/// board (which would orphan the run — see [`drainer`]). Omitting it keeps the
/// unconditional behaviour every other caller wants (a retitle, a horizon move).
#[tauri::command]
pub async fn roadmap_update_item(
    id: String,
    patch: ItemPatch,
    expect_status: Option<ItemStatus>,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<ItemUpdate, String> {
    let outcome = {
        let conn = db.lock();
        match expect_status {
            Some(expected) => {
                match store::update_where_status(&conn, &id, expected, &patch)
                    .map_err(|e| e.to_string())?
                {
                    Some(item) => Some(ItemUpdate {
                        applied: true,
                        item,
                    }),
                    // Missed the precondition. Read the current row under the
                    // same guard the failed update ran in, so what the caller
                    // gets back is the state that beat it.
                    None => store::get(&conn, &id)
                        .map_err(|e| e.to_string())?
                        .map(|item| ItemUpdate {
                            applied: false,
                            item,
                        }),
                }
            }
            None => store::update(&conn, &id, &patch)
                .map_err(|e| e.to_string())?
                .map(|item| ItemUpdate {
                    applied: true,
                    item,
                }),
        }
    };
    let outcome = outcome.ok_or_else(|| format!("roadmap item {id} no longer exists"))?;
    // A miss changed nothing, so there is nothing to announce and nothing new
    // for the drainer to look at.
    if outcome.applied {
        emit_item(&app, &outcome.item);
        drainer::nudge();
    }
    Ok(outcome)
}

/// Delete an item. Silent when the row is already gone — the caller's intent
/// ("this should not be on the board") is satisfied either way.
#[tauri::command]
pub async fn roadmap_delete_item(
    id: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<(), String> {
    let removed = {
        let conn = db.lock();
        store::delete(&conn, &id).map_err(|e| e.to_string())?
    };
    if removed {
        emit_item_deleted(&app, &id);
        // A deleted item can be the dep something queued was waiting on — a
        // stale code counts as satisfied, so the removal can unblock a run.
        drainer::nudge();
    }
    Ok(())
}
