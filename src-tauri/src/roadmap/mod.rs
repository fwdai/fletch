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
//! - `roadmap:item-event` — one durable history row ([`events`]), on every
//!   status transition. The expanded card renders the trail; `roadmap:item`
//!   already carries the row itself, so this only carries the *why*.
//! - `roadmap:proposal` / `roadmap:proposal-deleted` — one item's pending PM
//!   delta arriving or being ruled on ([`proposals`]).
//! - `roadmap:order-proposal` / `roadmap:order-proposal-deleted` — the PM's
//!   whole-board order ask ([`order`]), keyed by project rather than by row.
//! - `roadmap:queue-note` — transient: why an item isn't moving on its own.
//!   From [`drainer`] (a queued item's blocker) and [`merge_sweep`] (a PR that
//!   closed without merging). Nothing persists it; see the drainer's docs.
//!   Failures and transitions, by contrast, persist as [`events`] rows.
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
pub mod events;
pub mod merge_sweep;
pub mod order;
pub mod proposals;
pub mod store;
pub mod types;

use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::Connection;
use tauri::{AppHandle, Emitter};

use events::{EventActor, EventKind, ItemEvent};
use order::OrderProposal;
use proposals::{Proposal, ProposalKind, ProposalPatch};
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

/// Notify the frontend that a history row landed; carries the full event so an
/// expanded card appends it without a refetch. Best-effort, after the lock
/// drops, like every other emit here.
pub(crate) fn emit_item_event(app: &AppHandle, event: &ItemEvent) {
    let _ = app.emit("roadmap:item-event", event);
}

/// Notify the frontend that a pending PM proposal landed (or was replaced —
/// same id, new contents); carries the full row, so the card grows its
/// proposal bar live, mid-conversation.
pub(crate) fn emit_proposal(app: &AppHandle, proposal: &Proposal) {
    let _ = app.emit("roadmap:proposal", proposal);
}

/// Notify the frontend that a proposal is gone — ruled on, or stale. The item
/// itself is announced separately (`roadmap:item` / `roadmap:item-deleted`)
/// when the ruling changed it.
fn emit_proposal_deleted(app: &AppHandle, id: &str) {
    let _ = app.emit("roadmap:proposal-deleted", id);
}

/// Notify the frontend that the PM parked (or replaced) a whole-board order ask;
/// carries the full row, so the board's order bar appears mid-conversation.
pub(crate) fn emit_order_proposal(app: &AppHandle, proposal: &OrderProposal) {
    let _ = app.emit("roadmap:order-proposal", proposal);
}

/// Notify the frontend that the order ask is gone — ruled on, or stale.
/// Addressed by project, because that is the ask's key: one per board.
fn emit_order_proposal_deleted(app: &AppHandle, project_id: &str) {
    let _ = app.emit("roadmap:order-proposal-deleted", project_id);
}

/// Every item on a project's roadmap in board order (`rank`, then `created_at`).
/// Includes `done` items — the board hides them from the horizons and counts them
/// as "shipped".
#[tauri::command]
pub async fn roadmap_list_items(
    project_id: String,
    db: tauri::State<'_, Db>,
) -> Result<Vec<RoadmapItem>, String> {
    let conn = db.lock();
    store::list(&conn, &project_id).map_err(|e| e.to_string())
}

/// One item by id, or `None` when it's gone. The board never needs this (it
/// holds the whole project), but a *run* does: `wf_run.roadmap_item_id` names an
/// item by id, and the run monitor's roadmap chip has to render its code and
/// title without loading someone else's board.
#[tauri::command]
pub async fn roadmap_get_item(
    item_id: String,
    db: tauri::State<'_, Db>,
) -> Result<Option<RoadmapItem>, String> {
    let conn = db.lock();
    store::get(&conn, &item_id).map_err(|e| e.to_string())
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
    let (outcome, event) = {
        let conn = db.lock();
        update_and_record(&conn, &id, &patch, expect_status).map_err(|e| e.to_string())?
    };
    let outcome = outcome.ok_or_else(|| format!("roadmap item {id} no longer exists"))?;
    // A miss changed nothing, so there is nothing to announce and nothing new
    // for either background task to look at.
    if outcome.applied {
        emit_item(&app, &outcome.item);
        if let Some(event) = &event {
            emit_item_event(&app, event);
        }
        drainer::nudge();
        // The sweep parks while nothing is in review, and the drainer's
        // settlement is otherwise its only alarm clock — a patch that leaves a
        // row in review through this surface must ring it too. Gated on the
        // status so a title edit doesn't cut a sleeping sweep's interval short.
        if outcome.item.status == types::ItemStatus::InReview {
            merge_sweep::nudge();
        }
    }
    Ok(outcome)
}

/// The one write behind [`roadmap_update_item`]: apply the (possibly
/// conditional) patch and, when it actually lands, record the history event the
/// transition implies — both under the caller's single lock scope, so the event
/// can never describe a write that lost a race.
///
/// The event is `Some` exactly when the update applied: a missed precondition
/// wrote nothing, so there is no history to invent for it — the applied /
/// not-applied contract is untouched. Outer `None` means the row is gone.
fn update_and_record(
    conn: &Connection,
    id: &str,
    patch: &ItemPatch,
    expect_status: Option<ItemStatus>,
) -> rusqlite::Result<(Option<ItemUpdate>, Option<ItemEvent>)> {
    let updated = match expect_status {
        Some(expected) => store::update_where_status(conn, id, expected, patch)?,
        None => store::update(conn, id, patch)?,
    };
    match updated {
        Some(item) => {
            let kind = events::transition_kind(expect_status, patch.status);
            let event = events::record(
                conn,
                &item.id,
                &item.project_id,
                // This surface is the frontend's door; the other writers (PM
                // RPC, drainer, sweep) record under their own actors.
                EventActor::User,
                kind,
                None,
            )?;
            Ok((
                Some(ItemUpdate {
                    applied: true,
                    item,
                }),
                Some(event),
            ))
        }
        // Missed the precondition (or the row is gone). Read the current row
        // under the same guard the failed update ran in, so what the caller
        // gets back is the state that beat it.
        None => match expect_status {
            Some(_) => Ok((
                store::get(conn, id)?.map(|item| ItemUpdate {
                    applied: false,
                    item,
                }),
                None,
            )),
            None => Ok((None, None)),
        },
    }
}

/// Move an item in the project's priority order — the board's drag, landing as
/// a single fractional rank (see migration 0032).
///
/// Its own command rather than a `rank` patch through [`roadmap_update_item`]
/// because it deliberately writes **no history event**. A rank nudge is
/// bookkeeping, not a planning fact: it is the same class of write as the
/// drainer's `run_id` write-back (see [`drainer::write_item`] vs
/// `write_item_with_event`), and a trail reading "edited" six times because the
/// user tidied a backlog would bury the lines that matter. A *horizon* move is
/// a planning fact, so that one rides [`roadmap_update_item`] with the rank in
/// the same patch and records itself as an edit.
///
/// The write is unconditional: the row's status is not what this changes, and
/// re-ranking an item the drainer claimed a moment ago is harmless — the
/// drainer reads rank only to pick among `queued` items.
#[tauri::command]
pub async fn roadmap_set_rank(
    item_id: String,
    rank: f64,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<RoadmapItem, String> {
    let item = {
        let conn = db.lock();
        store::update(
            &conn,
            &item_id,
            &ItemPatch {
                rank: Some(rank),
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?
    };
    let item = item.ok_or_else(|| format!("roadmap item {item_id} no longer exists"))?;
    emit_item(&app, &item);
    // The queue dispatches in rank order, so the next pick may have changed.
    drainer::nudge();
    Ok(item)
}

/// Record the manual hand-off: the user sent this item to an agent they spawned
/// themselves ("Send to an agent" on the card), so the item now names that
/// workspace.
///
/// Its own command rather than an `agent_id` patch through
/// [`roadmap_update_item`] because the two say different things. A patch records
/// `edited` with no detail — true, and useless on the card. A hand-off is
/// provenance: it lands as a `note` naming the agent, read off the `workspaces`
/// row here so the trail can't disagree with the sidebar.
///
/// The status is deliberately untouched, and the hand-off is gated to
/// `proposed | open`: an item that is queued, being built, or in review is
/// already dispatched, and stamping a second builder onto it would put two
/// agents on one brief (the queue side of that door is closed too — the
/// drainer skips agent-linked items, and the card hides Queue on them). The
/// hand-off *is* the dispatch, and the user drives it from the agent's chat.
#[tauri::command]
pub async fn roadmap_hand_off_item(
    item_id: String,
    agent_id: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<RoadmapItem, String> {
    let (item, event) = {
        let conn = db.lock();
        hand_off(&conn, &item_id, &agent_id)?
    };
    emit_item(&app, &item);
    emit_item_event(&app, &event);
    Ok(item)
}

/// The one write behind [`roadmap_hand_off_item`]: stamp the workspace and
/// record the note in the caller's single lock scope, so an item that says it
/// was handed off always carries the line explaining when.
fn hand_off(
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
    // A name we can't read is not worth failing the hand-off over — the stamp is
    // the load-bearing half, and the card falls back to the workspace list for
    // the label anyway.
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

/// Take a handed-off item back off its agent ("Take it back" on the card): the
/// `agent_id` is cleared and the row is the queue's to dispatch again.
///
/// The undo half of [`roadmap_hand_off_item`], and its own command for the same
/// reason: clearing the stamp through a patch would record a bare `edited`,
/// while the trail needs to say *which* agent stopped owning the item. Gated to
/// `proposed | open` — the only statuses a hand-off can leave a row in, so
/// anything else means the queue or a run has taken over since and reclaiming
/// would strip provenance off work in flight.
#[tauri::command]
pub async fn roadmap_reclaim_item(
    item_id: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<RoadmapItem, String> {
    let (item, event) = {
        let conn = db.lock();
        reclaim(&conn, &item_id)?
    };
    emit_item(&app, &item);
    emit_item_event(&app, &event);
    // Nothing was blocking the queue from taking this row except the stamp.
    drainer::nudge();
    Ok(item)
}

/// The one write behind [`roadmap_reclaim_item`]: check the gate, clear the
/// stamp, and record the note in the caller's single lock scope.
fn reclaim(conn: &Connection, item_id: &str) -> Result<(RoadmapItem, ItemEvent), String> {
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
    // Same degradation as the hand-off: a name we can't read costs the label,
    // not the write.
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

/// Delete an item. Silent when the row is already gone — the caller's intent
/// ("this should not be on the board") is satisfied either way.
///
/// Deletion records no history event on purpose: `roadmap_item_events` cascades
/// with the row, so a deleted item (including a discarded proposal) takes its
/// trail with it — an item ruled off the board needs no history.
#[tauri::command]
pub async fn roadmap_delete_item(
    id: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<(), String> {
    let (removed, pending) = {
        let conn = db.lock();
        // Any pending PM proposal cascades away with the row; read it first so
        // its disappearance can be announced — the board holds proposals in
        // their own stream and would otherwise count a ghost of one forever.
        let pending = proposals::for_item(&conn, &id).map_err(|e| e.to_string())?;
        let removed = store::delete(&conn, &id).map_err(|e| e.to_string())?;
        (removed, pending.filter(|_| removed))
    };
    if removed {
        emit_item_deleted(&app, &id);
        if let Some(p) = pending {
            emit_proposal_deleted(&app, &p.id);
        }
        // A deleted item can be the dep something queued was waiting on — a
        // stale code counts as satisfied, so the removal can unblock a run.
        drainer::nudge();
    }
    Ok(())
}

/// One item's durable history, newest first. Fetched lazily by the board on
/// first card expand; live rows arrive on `roadmap:item-event`.
#[tauri::command]
pub async fn roadmap_list_item_events(
    item_id: String,
    db: tauri::State<'_, Db>,
) -> Result<Vec<ItemEvent>, String> {
    let conn = db.lock();
    events::list_for_item(&conn, &item_id).map_err(|e| e.to_string())
}

/// Every pending PM proposal on a project's board — the board load's companion
/// to [`roadmap_list_items`]; live rows arrive on `roadmap:proposal`.
#[tauri::command]
pub async fn roadmap_list_proposals(
    project_id: String,
    db: tauri::State<'_, Db>,
) -> Result<Vec<Proposal>, String> {
    let conn = db.lock();
    proposals::list_for_project(&conn, &project_id).map_err(|e| e.to_string())
}

/// What ruling on a proposal did, decided under one lock scope so the check,
/// the write, and the history it records can never disagree.
enum Ruling {
    /// The patch landed; emit the row and the `edited` event. Boxed: a ruling
    /// is almost always this variant, but the enum's size is set by it, and
    /// the row + event pair dwarfs the other arms.
    Updated {
        item: Box<RoadmapItem>,
        event: Box<ItemEvent>,
    },
    /// The item was deleted at the PM's ask; emit the deletion.
    Discarded { item_id: String },
    /// The item outran the ask (it went `active`+ since the PM proposed) — the
    /// proposal was deleted without applying, and the message says why.
    Stale { message: String },
}

/// May a proposal still be applied to this item? Anything from `active` on is
/// being built or judged — its shape belongs to the run now, and reshaping it
/// mid-flight would make the PR answer a brief nobody wrote.
fn proposal_gate(item: &RoadmapItem) -> Result<(), String> {
    match item.status {
        ItemStatus::Proposed | ItemStatus::Open | ItemStatus::Queued => Ok(()),
        status => Err(format!(
            "{} is {} — an item being built or reviewed can't be reshaped by proposal",
            item.code,
            status.as_str()
        )),
    }
}

/// The ruling's history line: the PM's rationale rides along, prefixed with
/// what the user did with it, so the trail reads honestly — who asked, who
/// ruled, and why.
fn ruling_detail(verb: &str, note: Option<&str>) -> String {
    match note {
        Some(note) => format!("{verb} a PM proposal — {note}"),
        None => format!("{verb} a PM proposal"),
    }
}

/// The one write behind [`roadmap_accept_proposal`]: re-read the proposal and
/// its item under the caller's lock, re-check the status gate (the item may
/// have gone `active` since the PM asked), then apply-and-record or drop the
/// stale ask. The proposal row is gone on every path — a ruling consumes it,
/// and a dead ask shouldn't haunt the card.
fn accept_proposal(conn: &Connection, proposal_id: &str) -> Result<Ruling, String> {
    let proposal = proposals::get(conn, proposal_id)
        .map_err(|e| e.to_string())?
        .ok_or("this proposal has already been ruled on")?;
    // The FK guarantees the item outlives its proposal, so a hit here is a row.
    let item = store::get(conn, &proposal.item_id)
        .map_err(|e| e.to_string())?
        .ok_or("the item this proposal targets no longer exists")?;

    if let Err(message) = proposal_gate(&item) {
        proposals::delete(conn, proposal_id).map_err(|e| e.to_string())?;
        return Ok(Ruling::Stale { message });
    }

    match proposal.kind {
        ProposalKind::Update => {
            let patch: ProposalPatch =
                serde_json::from_value(proposal.patch.clone().ok_or("proposal carries no patch")?)
                    .map_err(|e| e.to_string())?;
            let updated = store::update(conn, &item.id, &patch.to_item_patch())
                .map_err(|e| e.to_string())?
                .ok_or("the item this proposal targets no longer exists")?;
            let event = events::record(
                conn,
                &updated.id,
                &updated.project_id,
                // The ruling writes history, not the ask: the user accepting is
                // the edit, with the PM's rationale carried in the detail.
                EventActor::User,
                EventKind::Edited,
                Some(&ruling_detail("Accepted", proposal.note.as_deref())),
            )
            .map_err(|e| e.to_string())?;
            proposals::delete(conn, proposal_id).map_err(|e| e.to_string())?;
            Ok(Ruling::Updated {
                item: Box::new(updated),
                event: Box::new(event),
            })
        }
        ProposalKind::Discard => {
            // No event: the row's deletion cascades its history (and this
            // proposal) away — an item ruled off the board needs no trail,
            // exactly like `roadmap_delete_item`.
            store::delete(conn, &item.id).map_err(|e| e.to_string())?;
            Ok(Ruling::Discarded { item_id: item.id })
        }
    }
}

/// Apply a pending PM proposal — the user's "yes". One lock scope for the
/// whole ruling; see [`accept_proposal`]. A stale ask (the item went `active`
/// since) is deleted and reported as an error the board's bar can show — the
/// `roadmap:proposal-deleted` emit clears it from the card either way.
#[tauri::command]
pub async fn roadmap_accept_proposal(
    proposal_id: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<(), String> {
    let ruling = {
        let conn = db.lock();
        accept_proposal(&conn, &proposal_id)?
    };
    emit_proposal_deleted(&app, &proposal_id);
    match ruling {
        Ruling::Updated { item, event } => {
            emit_item(&app, &item);
            emit_item_event(&app, &event);
            // The patch can change horizon or deps, which can unblock (or
            // re-order) whatever is queued behind this item.
            drainer::nudge();
            Ok(())
        }
        Ruling::Discarded { item_id } => {
            emit_item_deleted(&app, &item_id);
            // A deleted item can be the dep something queued was waiting on.
            drainer::nudge();
            Ok(())
        }
        Ruling::Stale { message } => Err(message),
    }
}

/// Decline a pending PM proposal — the user's "no". The refusal is history the
/// PM's next session should see, so it lands as a durable `note` on the item;
/// the item itself is untouched.
#[tauri::command]
pub async fn roadmap_reject_proposal(
    proposal_id: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<(), String> {
    let event = {
        let conn = db.lock();
        reject_proposal(&conn, &proposal_id)?
    };
    emit_proposal_deleted(&app, &proposal_id);
    emit_item_event(&app, &event);
    Ok(())
}

/// The one write behind [`roadmap_reject_proposal`]: drop the proposal and
/// record the refusal, in the caller's single lock scope.
fn reject_proposal(conn: &Connection, proposal_id: &str) -> Result<ItemEvent, String> {
    let proposal = proposals::get(conn, proposal_id)
        .map_err(|e| e.to_string())?
        .ok_or("this proposal has already been ruled on")?;
    proposals::delete(conn, proposal_id).map_err(|e| e.to_string())?;
    events::record(
        conn,
        &proposal.item_id,
        &proposal.project_id,
        EventActor::User,
        EventKind::Note,
        Some(&ruling_detail("Declined", proposal.note.as_deref())),
    )
    .map_err(|e| e.to_string())
}

/// The project's pending whole-board order ask, if any — the board load's third
/// companion to [`roadmap_list_items`]; live rows arrive on
/// `roadmap:order-proposal`.
#[tauri::command]
pub async fn roadmap_get_order_proposal(
    project_id: String,
    db: tauri::State<'_, Db>,
) -> Result<Option<OrderProposal>, String> {
    let conn = db.lock();
    order::get(&conn, &project_id).map_err(|e| e.to_string())
}

/// What ruling on an order ask did. Same shape as [`Ruling`]: decided under one
/// lock scope, so the re-validation, the rewrite, and what gets announced can
/// never disagree.
enum OrderRuling {
    /// The sequence was applied; emit every row whose rank moved.
    Applied(Vec<RoadmapItem>),
    /// The board's orderable set is no longer the one the PM sequenced — the ask
    /// was deleted without applying, and the message says what changed.
    Stale(String),
}

/// The one write behind [`roadmap_accept_order_proposal`]: re-read the ask and
/// the board under the caller's lock, re-validate that the sequence is still
/// *exactly* the orderable set, then rewrite the ranks or drop the stale ask.
///
/// Re-validation is not paranoia: an item can be claimed by the drainer, shipped
/// by the sweep, or newly proposed by the PM between the ask and the click, and
/// each of those changes what "the whole order" means. Applying a sequence that
/// no longer covers the board would silently leave the new item's rank behind
/// everything — the same "a stale ask self-deletes" policy the item deltas use.
fn accept_order(conn: &Connection, project_id: &str) -> Result<OrderRuling, String> {
    let proposal = order::get(conn, project_id)
        .map_err(|e| e.to_string())?
        .ok_or("this order proposal has already been ruled on")?;
    let items = store::list(conn, project_id).map_err(|e| e.to_string())?;
    match order::validate_order(&proposal.codes, &items) {
        Err(message) => {
            order::delete(conn, project_id).map_err(|e| e.to_string())?;
            Ok(OrderRuling::Stale(format!(
                "the board changed since the PM proposed this order — {message}"
            )))
        }
        Ok(ids) => {
            // One transaction: a half-applied sequence is an order nobody
            // proposed. No per-item events — ranks are bookkeeping, exactly as
            // in [`roadmap_set_rank`].
            let rows = store::set_ranks(conn, &ids).map_err(|e| e.to_string())?;
            order::delete(conn, project_id).map_err(|e| e.to_string())?;
            Ok(OrderRuling::Applied(rows))
        }
    }
}

/// Apply the PM's proposed order — the user's "yes" on the whole sequence. One
/// lock scope for the ruling; see [`accept_order`]. The ask is consumed on every
/// path, so the bar clears whether the order landed or went stale.
#[tauri::command]
pub async fn roadmap_accept_order_proposal(
    project_id: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<(), String> {
    let ruling = {
        let conn = db.lock();
        accept_order(&conn, &project_id)?
    };
    emit_order_proposal_deleted(&app, &project_id);
    match ruling {
        OrderRuling::Applied(rows) => {
            for row in &rows {
                emit_item(&app, row);
            }
            // The queue dispatches in rank order, so the next pick has changed.
            drainer::nudge();
            Ok(())
        }
        OrderRuling::Stale(message) => Err(message),
    }
}

/// Decline the PM's proposed order — the board is untouched.
///
/// Unlike an item delta's refusal, this writes no history: there is no single
/// item the refusal is about, and inventing a `note` on every row in the
/// sequence would bury the lines that matter under bookkeeping.
#[tauri::command]
pub async fn roadmap_reject_order_proposal(
    project_id: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<(), String> {
    {
        let conn = db.lock();
        order::delete(&conn, &project_id).map_err(|e| e.to_string())?;
    }
    emit_order_proposal_deleted(&app, &project_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::get_migrations;
    use events::EventKind;
    use types::NewItem;

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

    fn with_status(conn: &Connection, status: ItemStatus) -> RoadmapItem {
        store::create(
            conn,
            "p1",
            &NewItem {
                title: "it".into(),
                status: Some(status),
                ..Default::default()
            },
        )
        .unwrap()
    }

    fn status_patch(to: ItemStatus) -> ItemPatch {
        ItemPatch {
            status: Some(to),
            ..Default::default()
        }
    }

    /// Every user transition the board performs writes exactly one event of the
    /// right kind, attributed to the user.
    #[test]
    fn each_user_transition_writes_exactly_one_event() {
        use ItemStatus::{Done, InReview, Open, Proposed, Queued};
        let conn = test_conn();
        let cases = [
            (Proposed, Open, EventKind::Accepted),
            (Open, Queued, EventKind::Queued),
            (Queued, Open, EventKind::Unqueued),
            (InReview, Done, EventKind::Shipped),
        ];
        for (from, to, kind) in cases {
            let it = with_status(&conn, from);
            let (outcome, event) =
                update_and_record(&conn, &it.id, &status_patch(to), Some(from)).unwrap();
            assert!(outcome.unwrap().applied);
            let event = event.expect("an applied transition records itself");
            assert_eq!(event.kind, kind);
            assert_eq!(event.actor, EventActor::User);
            assert_eq!(event.detail, None);
            assert_eq!(events::list_for_item(&conn, &it.id).unwrap(), vec![event]);
        }
    }

    /// A missed precondition writes nothing — not the row, and not history. The
    /// applied/not-applied contract is what the frontend's races lean on, and an
    /// event for a write that never happened would be a lie on the card.
    #[test]
    fn a_missed_precondition_records_nothing() {
        let conn = test_conn();
        let it = with_status(&conn, ItemStatus::Active);
        let (outcome, event) = update_and_record(
            &conn,
            &it.id,
            &status_patch(ItemStatus::Open),
            Some(ItemStatus::Queued),
        )
        .unwrap();
        let outcome = outcome.unwrap();
        assert!(!outcome.applied);
        assert_eq!(outcome.item.status, ItemStatus::Active);
        assert!(event.is_none());
        assert!(events::list_for_item(&conn, &it.id).unwrap().is_empty());
    }

    /// Any other applied patch is an `edited` — the catch-all that keeps "every
    /// transition writes exactly one event" true for the form too.
    #[test]
    fn a_plain_edit_records_edited() {
        let conn = test_conn();
        let it = with_status(&conn, ItemStatus::Open);
        let patch = ItemPatch {
            title: Some("retitled".into()),
            ..Default::default()
        };
        let (_, event) = update_and_record(&conn, &it.id, &patch, None).unwrap();
        assert_eq!(event.unwrap().kind, EventKind::Edited);
    }

    /// `roadmap_get_item`'s one read: a live id returns the row, and an id that
    /// never existed (or has been deleted) is `None` rather than an error — the
    /// run monitor's chip renders nothing for an item that left the board.
    #[test]
    fn getting_an_item_by_id_finds_it_or_reports_nothing() {
        let conn = test_conn();
        let it = with_status(&conn, ItemStatus::Open);
        assert_eq!(store::get(&conn, &it.id).unwrap().as_ref(), Some(&it));
        assert!(store::get(&conn, "no-such-item").unwrap().is_none());
        store::delete(&conn, &it.id).unwrap();
        assert!(store::get(&conn, &it.id).unwrap().is_none());
    }

    /// A hand-off stamps the workspace, leaves the status alone, and records a
    /// `note` naming the agent — the line the card's trail shows.
    #[test]
    fn handing_off_stamps_the_agent_and_names_it_in_history() {
        let conn = test_conn();
        conn.execute(
            "INSERT INTO workspaces (id, project_id, name, created_at)
             VALUES ('w1', 'p1', 'blue-heron', 0)",
            [],
        )
        .unwrap();
        let it = with_status(&conn, ItemStatus::Open);

        let (item, event) = hand_off(&conn, &it.id, "w1").unwrap();
        assert_eq!(item.agent_id.as_deref(), Some("w1"));
        assert_eq!(item.status, ItemStatus::Open);
        assert_eq!(event.kind, EventKind::Note);
        assert_eq!(event.actor, EventActor::User);
        assert_eq!(event.detail.as_deref(), Some("Handed to agent blue-heron"));
    }

    /// An unknown agent id still stamps (the row is what the drainer and the
    /// card read); only the label degrades. An unknown *item* is an error —
    /// there is nothing to hand off.
    #[test]
    fn handing_off_degrades_without_a_name_and_refuses_a_dead_item() {
        let conn = test_conn();
        let it = with_status(&conn, ItemStatus::Open);
        let (_, event) = hand_off(&conn, &it.id, "gone").unwrap();
        assert_eq!(event.detail.as_deref(), Some("Handed to an agent"));
        assert!(hand_off(&conn, "no-such-item", "gone").is_err());
    }

    /// Anything from `queued` on is already dispatched: handing it off would
    /// put a second builder on one brief. The refusal names the status, stamps
    /// nothing, and records nothing.
    #[test]
    fn handing_off_refuses_a_dispatched_item() {
        let conn = test_conn();
        for status in [ItemStatus::Queued, ItemStatus::Active, ItemStatus::InReview] {
            let it = with_status(&conn, status);
            let err = hand_off(&conn, &it.id, "w1").unwrap_err();
            assert!(err.contains(status.as_str()), "{err}");
            let row = store::get(&conn, &it.id).unwrap().unwrap();
            assert_eq!(row.agent_id, None, "a refusal must not stamp");
            assert!(events::list_for_item(&conn, &it.id).unwrap().is_empty());
        }
    }

    /// Taking an item back clears the stamp and records a `note` naming the
    /// agent it came back from — the undo of a hand-off, and the only other
    /// writer of `agent_id`.
    #[test]
    fn reclaiming_clears_the_agent_and_names_it_in_history() {
        let conn = test_conn();
        conn.execute(
            "INSERT INTO workspaces (id, project_id, name, created_at)
             VALUES ('w1', 'p1', 'blue-heron', 0)",
            [],
        )
        .unwrap();
        let it = with_status(&conn, ItemStatus::Open);
        hand_off(&conn, &it.id, "w1").unwrap();

        let (item, event) = reclaim(&conn, &it.id).unwrap();
        assert_eq!(item.agent_id, None);
        assert_eq!(item.status, ItemStatus::Open, "the status is untouched");
        assert_eq!(event.kind, EventKind::Note);
        assert_eq!(event.actor, EventActor::User);
        assert_eq!(
            event.detail.as_deref(),
            Some("Taken back from agent blue-heron")
        );
        // Nothing to take back twice.
        assert!(reclaim(&conn, &it.id).is_err());
    }

    /// The gate: an item nobody handed off has nothing to reclaim, and one the
    /// queue has since taken is being built — the run is where that gets dealt
    /// with. A refusal writes nothing.
    #[test]
    fn reclaiming_refuses_an_unhanded_or_dispatched_item() {
        let conn = test_conn();
        let bare = with_status(&conn, ItemStatus::Open);
        assert!(reclaim(&conn, &bare.id).unwrap_err().contains("with an agent"));

        for status in [ItemStatus::Queued, ItemStatus::Active, ItemStatus::InReview] {
            let it = with_status(&conn, status);
            store::update(
                &conn,
                &it.id,
                &ItemPatch {
                    agent_id: Some(Some("w1".into())),
                    ..Default::default()
                },
            )
            .unwrap();
            let err = reclaim(&conn, &it.id).unwrap_err();
            assert!(err.contains(status.as_str()), "{err}");
            let row = store::get(&conn, &it.id).unwrap().unwrap();
            assert_eq!(row.agent_id.as_deref(), Some("w1"), "a refusal writes nothing");
            assert!(events::list_for_item(&conn, &it.id).unwrap().is_empty());
        }
        assert!(reclaim(&conn, "no-such-item").is_err());
    }

    /// A pending update proposal for a test item, straight through the DAO —
    /// the RPC op's validation is exercised in `rpc::roadmap`'s own tests.
    fn pending_update(conn: &Connection, item: &RoadmapItem, note: Option<&str>) -> Proposal {
        let patch = ProposalPatch {
            title: Some("reshaped".into()),
            horizon: Some(types::Horizon::Now),
            ..Default::default()
        };
        proposals::upsert(
            conn,
            &item.project_id,
            &item.id,
            ProposalKind::Update,
            Some(&patch),
            note,
        )
        .unwrap()
    }

    /// Accepting an update applies the patch, records the ruling as an `edited`
    /// event carrying the PM's rationale, and consumes the proposal.
    #[test]
    fn accepting_an_update_applies_records_and_consumes() {
        let conn = test_conn();
        let it = with_status(&conn, ItemStatus::Open);
        let p = pending_update(&conn, &it, Some("scope grew"));

        let ruling = accept_proposal(&conn, &p.id).unwrap();
        let Ruling::Updated { item, event } = ruling else {
            panic!("expected Updated");
        };
        assert_eq!(item.title, "reshaped");
        assert_eq!(item.horizon, types::Horizon::Now);
        assert_eq!(event.kind, EventKind::Edited);
        assert_eq!(event.actor, EventActor::User);
        assert_eq!(
            event.detail.as_deref(),
            Some("Accepted a PM proposal — scope grew")
        );
        assert!(proposals::get(&conn, &p.id).unwrap().is_none());
        // Ruling twice is refused, not replayed.
        assert!(accept_proposal(&conn, &p.id).is_err());
    }

    /// An item that went `active` between the ask and the ruling refuses the
    /// apply and clears the ask: nothing written, no history, no zombie bar.
    #[test]
    fn accepting_against_a_raced_away_item_refuses_and_clears() {
        let conn = test_conn();
        let it = with_status(&conn, ItemStatus::Queued);
        let p = pending_update(&conn, &it, None);
        // The drainer claimed it while the user was reading the diff.
        store::update(&conn, &it.id, &status_patch(ItemStatus::Active)).unwrap();

        let Ruling::Stale { message } = accept_proposal(&conn, &p.id).unwrap() else {
            panic!("expected Stale");
        };
        assert!(message.contains("active"), "{message}");
        assert!(message.contains(&it.code), "{message}");
        assert!(proposals::get(&conn, &p.id).unwrap().is_none());
        // Untouched: no patch, no event.
        let row = store::get(&conn, &it.id).unwrap().unwrap();
        assert_eq!(row.title, it.title);
        assert!(events::list_for_item(&conn, &it.id).unwrap().is_empty());
    }

    /// Accepting a discard deletes the item row; its history and the proposal
    /// itself cascade away with it.
    #[test]
    fn accepting_a_discard_deletes_the_row() {
        let conn = test_conn();
        let it = with_status(&conn, ItemStatus::Open);
        let p = proposals::upsert(
            &conn,
            "p1",
            &it.id,
            ProposalKind::Discard,
            None,
            Some("superseded by MCA-101"),
        )
        .unwrap();

        let Ruling::Discarded { item_id } = accept_proposal(&conn, &p.id).unwrap() else {
            panic!("expected Discarded");
        };
        assert_eq!(item_id, it.id);
        assert!(store::get(&conn, &it.id).unwrap().is_none());
        assert!(proposals::get(&conn, &p.id).unwrap().is_none());
    }

    /// Accepting an order rewrites every orderable row's rank as 1.0, 2.0, …
    /// in the asked sequence, consumes the ask, and writes no history — a rank
    /// is bookkeeping, like the drainer's `run_id` write-back.
    #[test]
    fn accepting_an_order_renumbers_the_board_and_records_nothing() {
        let conn = test_conn();
        let a = with_status(&conn, ItemStatus::Open);
        let b = with_status(&conn, ItemStatus::Queued);
        let c = with_status(&conn, ItemStatus::Proposed);
        order::upsert(
            &conn,
            "p1",
            &[c.code.clone(), a.code.clone(), b.code.clone()],
            Some("auth first"),
        )
        .unwrap();

        let OrderRuling::Applied(rows) = accept_order(&conn, "p1").unwrap() else {
            panic!("expected Applied");
        };
        assert_eq!(
            rows.iter().map(|r| r.code.as_str()).collect::<Vec<_>>(),
            vec![c.code.as_str(), a.code.as_str(), b.code.as_str()]
        );
        assert_eq!(
            store::list(&conn, "p1")
                .unwrap()
                .iter()
                .map(|i| i.code.clone())
                .collect::<Vec<_>>(),
            vec![c.code.clone(), a.code.clone(), b.code.clone()],
            "the board (and the drainer's queue) now reads in the accepted order"
        );
        for it in [&a, &b, &c] {
            assert!(
                events::list_for_item(&conn, &it.id).unwrap().is_empty(),
                "a reorder is bookkeeping, not history"
            );
        }
        // The ask is consumed; ruling twice is refused rather than replayed.
        assert!(order::get(&conn, "p1").unwrap().is_none());
        assert!(accept_order(&conn, "p1").is_err());
    }

    /// The board moved since the PM sequenced it (the drainer claimed one item,
    /// and a new ticket arrived): the ask no longer covers the orderable set, so
    /// it refuses and self-deletes rather than applying a partial order.
    #[test]
    fn accepting_a_stale_order_refuses_and_clears() {
        let conn = test_conn();
        let a = with_status(&conn, ItemStatus::Queued);
        let b = with_status(&conn, ItemStatus::Open);
        order::upsert(&conn, "p1", &[b.code.clone(), a.code.clone()], None).unwrap();
        // The drainer claimed `a` while the user was reading the ask, and the PM
        // proposed something new.
        store::update(&conn, &a.id, &status_patch(ItemStatus::Active)).unwrap();
        let fresh = with_status(&conn, ItemStatus::Proposed);

        let OrderRuling::Stale(message) = accept_order(&conn, "p1").unwrap() else {
            panic!("expected Stale");
        };
        assert!(message.contains("the board changed"), "{message}");
        assert!(message.contains(&a.code), "{message}");
        assert!(order::get(&conn, "p1").unwrap().is_none());
        // Nothing was renumbered.
        assert_eq!(store::get(&conn, &b.id).unwrap().unwrap().rank, b.rank);
        assert_eq!(
            store::get(&conn, &fresh.id).unwrap().unwrap().rank,
            fresh.rank
        );
    }

    /// Declining leaves the board alone and takes the ask with it. No history:
    /// there is no one item a whole-board refusal belongs to.
    #[test]
    fn rejecting_an_order_drops_the_ask_and_touches_nothing() {
        let conn = test_conn();
        let a = with_status(&conn, ItemStatus::Open);
        order::upsert(&conn, "p1", std::slice::from_ref(&a.code), Some("nope")).unwrap();

        assert!(order::delete(&conn, "p1").unwrap());
        assert!(order::get(&conn, "p1").unwrap().is_none());
        assert_eq!(store::get(&conn, &a.id).unwrap().unwrap().rank, a.rank);
        assert!(events::list_for_item(&conn, &a.id).unwrap().is_empty());
    }

    /// Declining leaves the item alone and writes the refusal as a durable
    /// `note`, so the PM's next session sees it was ruled on, and how.
    #[test]
    fn rejecting_writes_a_note_and_consumes_the_proposal() {
        let conn = test_conn();
        let it = with_status(&conn, ItemStatus::Open);
        let p = pending_update(&conn, &it, Some("split this in two"));

        let event = reject_proposal(&conn, &p.id).unwrap();
        assert_eq!(event.kind, EventKind::Note);
        assert_eq!(event.actor, EventActor::User);
        assert_eq!(
            event.detail.as_deref(),
            Some("Declined a PM proposal — split this in two")
        );
        assert_eq!(event.item_id, it.id);
        assert!(proposals::get(&conn, &p.id).unwrap().is_none());
        let row = store::get(&conn, &it.id).unwrap().unwrap();
        assert_eq!(row.title, it.title);
    }
}
