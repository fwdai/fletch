//! The project roadmap: the per-project list of items the Roadmap tab renders.
//!
//! Layout mirrors the workflow module: [`types`] is the row and its enums,
//! [`store`] is the DAO (called with the connection lock held), and this file is
//! the typed `#[tauri::command]` surface plus the row-level events the frontend
//! syncs on. [`deps`] holds the one rule every dep write on every surface has to
//! pass — no loops — because a loop wedges the queue silently and forever.
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
//! - `roadmap:project-hold` / `roadmap:project-hold-released` — the whole board
//!   stopped, or resumed ([`holds`]). Its own pair rather than a `roadmap:item`
//!   ride, because a project hold is a fact about the board and belongs to no
//!   row; an *item's* hold travels on the row itself, which carries the trio.
//! - `roadmap:brief` — the project's product brief, after the user rules a change
//!   in ([`memory`]). Board scoped like the two above: the brief is a fact about
//!   the *product*, not about any item.
//! - `roadmap:brief-proposal` / `roadmap:brief-proposal-deleted` — the PM's
//!   pending ask to replace that brief, arriving or being ruled on.
//! - `roadmap:queue-note` — transient: why an item isn't moving on its own.
//!   From [`drainer`] (a queued item's blocker) and [`merge_sweep`] (a PR that
//!   closed without merging). Nothing persists it; see the drainer's docs.
//!   Failures and transitions, by contrast, persist as [`events`] rows.
//!
//! Autonomous dispatch lives in [`drainer`]: `queued` items become running
//! workflows there, and every mutation on this surface [`drainer::nudge`]s it so
//! a queue action doesn't wait out the tick interval.
//!
//! [`holds`] is the brake on that dispatch, and the one place where the two
//! doors are deliberately asymmetric: the PM may *place* a hold (its only
//! state-stopping write, invariant 2), and only the typed commands here can lift
//! one. Every release is therefore a user action by construction.
//!
//! [`merge_sweep`] closes the loop at the other end: it watches the PRs of
//! `in_review` items and moves them to `done` when they merge on GitHub. It is
//! host-side rather than in the webview precisely so a queue keeps draining
//! with the window shut.
//!
//! [`review`] closes it a third time, upwards: every run the drainer settles is
//! handed to the project-manager chat as a review turn, so the agent that wrote
//! the brief is the agent that reads what came back.
//!
//! [`memory`] is the other axis entirely: not what will be built, but what the
//! product *is* — the brief the PM keeps across sessions, injected into its
//! instructions at spawn and changed only by the user's ruling. A seam by design;
//! its own docs say what may be replaced behind it.
//!
//! [`pr_review`] is the sweep's foreground half: while a board is on screen it
//! answers the *review* questions about an `in_review` item's PR (CI, conflicts,
//! unresolved threads) so the user can judge, merge, or send the feedback back
//! to an agent without leaving the board.

pub mod deps;
pub mod drainer;
pub mod events;
pub mod holds;
pub mod memory;
pub mod merge_sweep;
pub mod order;
pub mod pr_review;
pub mod proposals;
pub mod review;
pub mod store;
pub mod types;

use std::borrow::Cow;
use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::Connection;
use tauri::{AppHandle, Emitter};

use events::{EventActor, EventKind, ItemEvent};
use holds::ProjectHold;
use memory::{Brief, BriefProposal};
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

/// Notify the frontend that the whole board is held (or that the reason changed —
/// one row per project, replaced in place); carries the full row, so the banner
/// appears mid-conversation when the PM pulls the brake.
pub(crate) fn emit_project_hold(app: &AppHandle, hold: &ProjectHold) {
    let _ = app.emit("roadmap:project-hold", hold);
}

/// Notify the frontend that the board is running again. Addressed by project,
/// because that is the hold's key — the board has nothing else to drop.
fn emit_project_hold_released(app: &AppHandle, project_id: &str) {
    let _ = app.emit("roadmap:project-hold-released", project_id);
}

/// Notify the frontend that the project's product brief changed — which only
/// happens when the user rules a PM ask in ([`memory`]). Carries the whole
/// document, because that is what the tab renders.
fn emit_brief(app: &AppHandle, brief: &Brief) {
    let _ = app.emit("roadmap:brief", brief);
}

/// Notify the frontend that the PM parked (or replaced) an ask to rewrite the
/// brief; carries the full row, so the tab's decision bar appears
/// mid-conversation.
pub(crate) fn emit_brief_proposal(app: &AppHandle, proposal: &BriefProposal) {
    let _ = app.emit("roadmap:brief-proposal", proposal);
}

/// Notify the frontend that the brief ask is gone — ruled on either way.
/// Addressed by project, because that is the ask's key: one per board.
fn emit_brief_proposal_deleted(app: &AppHandle, project_id: &str) {
    let _ = app.emit("roadmap:brief-proposal-deleted", project_id);
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
///
/// The row starts its durable history here with a `created` event, the mirror of
/// the propose RPC's `proposed`. Without it a hand-built board has no history at
/// all, and every consumer that reads "what changed since?" off the event trail
/// — the PM's standup digest above all — would call a board the user filled in
/// by hand unchanged (see .context/roadmap-pm-plan.md, B4).
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
    let (created, event) = {
        let conn = db.lock();
        create_checked(&conn, &project_id, &item)?
    };
    emit_item(&app, &created);
    emit_item_event(&app, &event);
    // A row can arrive already `queued` (or as a dependency another queued item
    // is waiting on), so every mutation re-checks the queue rather than trying
    // to guess which ones matter.
    drainer::nudge();
    Ok(created)
}

/// The one write behind [`roadmap_create_item`]: check the row's deps against
/// the board, then insert it and open its history in one transaction — all in
/// the caller's single lock scope, so an item can never exist without the line
/// saying who put it there.
///
/// The dep check first: a new row can carry deps (the dialog's chips), so the
/// codes have to resolve — a dep naming nothing reads as *satisfied* to the
/// drainer, which silently means "no dependency at all", the opposite of what
/// was typed. It cannot close a loop: nothing can depend on a row that has no
/// code yet.
fn create_checked(
    conn: &Connection,
    project_id: &str,
    item: &NewItem,
) -> Result<(RoadmapItem, ItemEvent), String> {
    if !item.deps.is_empty() {
        let board = store::list(conn, project_id).map_err(|e| e.to_string())?;
        deps::validate_new(&deps::graph_of(&board), &item.deps)?;
    }
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let created = store::create(&tx, project_id, item).map_err(|e| e.to_string())?;
    let event = events::record(
        &tx,
        &created.id,
        project_id,
        // This surface is the frontend's door: a row that arrives here was typed
        // by the user, even when it lands straight into `queued`.
        EventActor::User,
        EventKind::Created,
        None,
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok((created, event))
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
///
/// `queue` is the accept path's one extra gesture: "Accept & queue", one click
/// instead of accept-then-queue. It is meaningful *only* on an accept
/// (`expect_status: proposed`, `status: open`) and ignored everywhere else — see
/// [`accept_landing`], which is also where the project's autoqueue dial is read,
/// so the button and the dial are one implementation and a hold overrules both.
#[tauri::command]
pub async fn roadmap_update_item(
    id: String,
    patch: ItemPatch,
    expect_status: Option<ItemStatus>,
    queue: Option<bool>,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<ItemUpdate, String> {
    let (outcome, event) = {
        let conn = db.lock();
        update_and_record(&conn, &id, &patch, expect_status, queue.unwrap_or(false))?
    };
    let outcome = outcome.ok_or_else(|| format!("roadmap item {id} no longer exists"))?;
    // A miss changed nothing, so there is nothing to announce and nothing new
    // for either background task to look at.
    if outcome.applied {
        emit_item(&app, &outcome.item);
        if let Some(event) = &event {
            emit_item_event(&app, event);
        }
        // Unconditional, and load-bearing for the dial: an autoqueued accept
        // leaves the row `queued`, so this is the call that turns one click into a
        // run without waiting out the tick.
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
///
/// A patch carrying `deps` is checked against [`deps`] *first*, in this same
/// guard: this command is the item dialog's door, and a dep list that closes a
/// loop would leave the queue skipping the whole chain forever. The refusal is
/// an `Err` the dialog renders in its error slot, not a silent drop.
///
/// An *accept* is the one transition this function may redirect: [`accept_landing`]
/// decides whether the row lands `open` or `queued`, so the autonomy dial applies
/// to every accept surface there is — the card's bar, the batch bar, anything
/// added later — without one line of frontend logic deciding it. The event still
/// says `accepted` (from the transition the caller *asked* for), with what became
/// of it as the detail.
fn update_and_record(
    conn: &Connection,
    id: &str,
    patch: &ItemPatch,
    expect_status: Option<ItemStatus>,
    queue: bool,
) -> Result<(Option<ItemUpdate>, Option<ItemEvent>), String> {
    if let Some(new_deps) = &patch.deps {
        // A row that is already gone falls through to the normal "no longer
        // exists" path below rather than being refused for its deps.
        if let Some(current) = store::get(conn, id).map_err(|e| e.to_string())? {
            check_dep_edit(conn, &current, new_deps)?;
        }
    }
    // Where an accept lands, resolved before the write and in the same guard as
    // it: the dial, the button, and the two holds are all read here.
    let landing = is_accept(expect_status, patch)
        .then(|| landing_for(conn, id, queue))
        .flatten();
    // An accept writes the status its landing names — `open` for all but a
    // queueing one, which is the status the caller already sent. Only the
    // `status` field is ever rewritten; everything else lands exactly as sent.
    let mut effective = Cow::Borrowed(patch);
    if let Some(landing) = landing {
        effective.to_mut().status = Some(landing.status());
    }
    let updated = match expect_status {
        Some(expected) => store::update_where_status(conn, id, expected, &effective),
        None => store::update(conn, id, &effective),
    }
    .map_err(|e| e.to_string())?;
    match updated {
        Some(item) => {
            // The transition the *caller* performed, not the status the row
            // landed in: an autoqueued accept is one ruling by one actor, and
            // `accepted` is the name of that ruling. What the dial then did with
            // it is the detail — see [`Landing::detail`].
            let kind = events::transition_kind(expect_status, patch.status);
            let event = events::record(
                conn,
                &item.id,
                &item.project_id,
                // This surface is the frontend's door; the other writers (PM
                // RPC, drainer, sweep) record under their own actors.
                EventActor::User,
                kind,
                landing.and_then(Landing::detail),
            )
            .map_err(|e| e.to_string())?;
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
                store::get(conn, id)
                    .map_err(|e| e.to_string())?
                    .map(|item| ItemUpdate {
                        applied: false,
                        item,
                    }),
                None,
            )),
            None => Ok((None, None)),
        },
    }
}

/// Is this patch *the accept* — the `proposed → open` ruling that turns a PM
/// suggestion into a roadmap item? The only transition the autonomy dial touches.
fn is_accept(expect_status: Option<ItemStatus>, patch: &ItemPatch) -> bool {
    expect_status == Some(ItemStatus::Proposed) && patch.status == Some(ItemStatus::Open)
}

/// Where an accepted item lands.
///
/// The autonomy dial's entire user-visible effect, as three outcomes rather than a
/// bool, because the third one has to be *sayable*: a user who pressed "Accept &
/// queue" on a held row and got an `open` item is owed the reason.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Landing {
    /// `proposed → open`. The accept alone — what every accept did before the
    /// dial existed, and still the default.
    Board,
    /// `proposed → queued`. Either the project's autoqueue dial is on, or this
    /// click was "Accept & queue".
    Queue,
    /// Queueing was asked for and a hold stands, so it lands `open` after all.
    HeldBack,
}

impl Landing {
    /// The status the row actually lands in.
    fn status(self) -> ItemStatus {
        match self {
            Landing::Queue => ItemStatus::Queued,
            Landing::Board | Landing::HeldBack => ItemStatus::Open,
        }
    }

    /// What the `accepted` line says beyond "accepted", or `None` when the accept
    /// is the whole story.
    ///
    /// One event, not two. An `accepted` + `queued` pair would read as two rulings
    /// where the user made one, and would make this the only write in the module
    /// whose single transition emits two history rows; the trail's job is to name
    /// the *gesture*, and "Accepted — auto-queued" names it exactly. Both readers
    /// of the trail are served by that: the card's history line renders the detail
    /// inline, and the PM's `last_event` projection (and the standup that reads the
    /// newest event) sees one line it can quote verbatim.
    fn detail(self) -> Option<&'static str> {
        match self {
            Landing::Board => None,
            Landing::Queue => Some("auto-queued"),
            Landing::HeldBack => Some("left off the queue — this is held"),
        }
    }
}

/// The dial, the button, and the brake — one rule, pure over the three facts.
///
/// Holds trump the dial, always (invariant 2: a hold may only ever *reduce*
/// autonomy). So `held` can turn a queueing accept into a plain one, and nothing
/// can turn a plain accept into a queueing one behind the user's back. The row
/// still becomes a roadmap item — refusing the accept itself would be a hold
/// blocking the user's own ruling, which is the opposite of what it is for.
pub(crate) fn accept_landing(queue_requested: bool, autoqueue: bool, held: bool) -> Landing {
    if !queue_requested && !autoqueue {
        return Landing::Board;
    }
    if held {
        return Landing::HeldBack;
    }
    Landing::Queue
}

/// [`accept_landing`] against the database: read the project's dial and the brake
/// for the row being accepted. Called with the connection lock held, in the same
/// guard as the write it decides.
///
/// `None` when the row is gone — the caller's write then misses on its own and
/// reports the row as it is.
fn landing_for(conn: &Connection, id: &str, queue: bool) -> Option<Landing> {
    let item = store::get(conn, id).ok().flatten()?;
    Some(accept_landing(
        queue,
        drainer::autoqueue(conn, &item.project_id),
        // One gate for both scopes, fail-closed inside it ([`holds::gate`]): this
        // must never be the door that queues something the drainer would then
        // refuse to dispatch.
        holds::gate(conn, &item).is_some(),
    ))
}

/// Check a dep list an item that already exists is about to be given: the codes
/// must resolve, and the graph the write leaves behind must be acyclic
/// ([`deps::validate_edit`]).
///
/// Skipped when the list is the one the item already has. Both writers send whole
/// lists — the dialog posts its form, a PM patch can include an unchanged `deps`
/// — and re-stating an item's own deps cannot make the graph worse. Refusing a
/// retitle because of a loop that was already there would strand the row instead
/// of helping fix it; the drainer's durable `blocked` event is what surfaces
/// those.
fn check_dep_edit(
    conn: &Connection,
    item: &RoadmapItem,
    new_deps: &[String],
) -> Result<(), String> {
    if item.deps == new_deps {
        return Ok(());
    }
    let board = store::list(conn, &item.project_id).map_err(|e| e.to_string())?;
    deps::validate_edit(&deps::graph_of(&board), &item.code, new_deps)
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

/// One `in_review` item's live review state: the CI rollup, the unresolved
/// review threads, and the PR's branch pair ([`pr_review`]).
///
/// Read-only and degrading by design. `None` means "there is nothing to read
/// here" — the item isn't under review, has no PR number, or its project has no
/// repo — and every *field* of a `Some` degrades independently, so a GraphQL
/// budget that ran out leaves the CI rollup on screen. The board polls this
/// while it is mounted; nothing here writes, emits, or nudges.
#[tauri::command]
pub async fn roadmap_item_review(
    item_id: String,
    db: tauri::State<'_, Db>,
) -> Result<Option<pr_review::ItemReview>, String> {
    // The lock is taken and dropped by `target`, before the network reads — a
    // board-cadence poll must never hold the app's one connection across an
    // HTTP round-trip.
    let Some((repo, number)) = pr_review::target(&db, &item_id) else {
        return Ok(None);
    };
    Ok(Some(pr_review::fetch(&repo, number).await))
}

/// Merge an `in_review` item's pull request, from its card.
///
/// The same host merge path the Git panel's Merge button takes
/// ([`crate::github::pr_merge_number`] — auto-merge with the documented
/// direct-merge fallback), addressed by number because the project's checkout is
/// on its base branch, not on the PR's.
///
/// **This does not ship the item.** `in_review → done` has exactly one writer,
/// the merge sweep, and that stays true (invariant 1): a merge is a request to
/// GitHub, and only GitHub's answer is evidence it landed. What this does is
/// [`merge_sweep::nudge`] afterwards, so the sweep asks within a beat instead of
/// waiting out its two-minute tick — reality catches up in a moment rather than
/// a coffee break, without anyone else learning to write `done`.
///
/// **A hold refuses the click** ([`merge_hold_gate`]). This button is the one
/// place the app itself arms an auto-merge, which is how a hold placed afterwards
/// used to be outrun by GitHub — the merge fires later, the sweep ships the item,
/// and the work behind it goes. Saying "this is held, and here is the reason" is
/// the answer the user asked for; releasing first is one click away, and it is
/// theirs alone.
#[tauri::command]
pub async fn roadmap_merge_item_pr(
    item_id: String,
    db: tauri::State<'_, Db>,
) -> Result<(), String> {
    merge_hold_gate(&db, &item_id)?;
    let (repo, number) = pr_review::target(&db, &item_id).ok_or(
        "this item has no pull request to merge — it may have shipped or come back to the board",
    )?;
    // Loud on failure, unlike the read above: this is a click, and a refused
    // merge (gate closed, no permission, revoked token) is the answer the user
    // asked for. It lands on the board's error bar.
    crate::github::pr_merge_number(&repo, number)
        .await
        .map_err(|e| e.to_string())?;
    merge_sweep::nudge();
    Ok(())
}

/// Refuse a Merge click while a hold stands, naming the reason.
///
/// The same [`holds::gate`] every autonomous writer consults, asked here for the
/// one *user* action that would otherwise hand a held item to GitHub — and
/// therefore, a beat later, to the sweep. A refusal rather than a silent no-op
/// because this is a button press: the user is owed the reason, and the board's
/// error bar is where it lands.
///
/// A row that is gone falls through to the missing-PR message below, which is the
/// more accurate complaint about it.
fn merge_hold_gate(db: &Db, item_id: &str) -> Result<(), String> {
    let conn = db.lock();
    let Some(item) = store::get(&conn, item_id).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    match holds::gate(&conn, &item) {
        Some(reason) => Err(format!(
            "{} is held — {reason}. Release the hold before merging its pull request.",
            item.code
        )),
        None => Ok(()),
    }
}

/// Record that this item's review feedback went to an agent — the durable half
/// of "Fix review feedback" on an `in_review` card.
///
/// A `note`, not an `agent_id` stamp, and deliberately *not*
/// [`roadmap_hand_off_item`]: that gate refuses anything past `open` because an
/// item's builder is singular, and this item already has one (the run that
/// opened the PR). The fix agent belongs to the *pull request*, not to the item,
/// so the item's history gains a line and nothing else about it changes — it
/// stays `in_review`, and the sweep still rules on shipment.
#[tauri::command]
pub async fn roadmap_note_review_feedback(
    item_id: String,
    threads: usize,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<ItemEvent, String> {
    let event = {
        let conn = db.lock();
        let item = store::get(&conn, &item_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("roadmap item {item_id} no longer exists"))?;
        // Gated for the same reason the note is worth writing at all: it claims
        // a PR was handed to an agent, and only an item under review has one.
        if item.status != ItemStatus::InReview {
            return Err(format!(
                "{} is {} — only an item under review has feedback to send",
                item.code,
                item.status.as_str()
            ));
        }
        events::record(
            &conn,
            &item.id,
            &item.project_id,
            EventActor::User,
            EventKind::Note,
            Some(&pr_review::feedback_detail(threads)),
        )
        .map_err(|e| e.to_string())?
    };
    emit_item_event(&app, &event);
    Ok(event)
}

// ───────────────────────────── holds ────────────────────────────────────

/// Stop autonomous progress on one item until the user lifts it — the user's own
/// brake, the same write the PM's `roadmap_hold` op makes ([`holds`]).
///
/// The status is deliberately untouched, at any status. A hold is not an unqueue:
/// the item keeps its place in the queue (and its rank), it simply isn't
/// dispatchable while the reason stands, which is what makes releasing a
/// one-click undo rather than a re-queue. Allowed on `active`+ rows too — a run
/// already in flight is exactly when "wait, we agreed something else" is worth
/// recording, and the hold is then what stops the *next* dispatch of that item.
///
/// Records a `held` event carrying the reason, attributed to the user. Holding an
/// already-held item replaces the reason and writes a second `held`, so the trail
/// answers "what has this been held for" while the row carries only the reason in
/// force.
#[tauri::command]
pub async fn roadmap_hold_item(
    item_id: String,
    reason: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<RoadmapItem, String> {
    let reason = holds::clean_reason(&reason)?;
    let (item, event) = {
        let conn = db.lock();
        hold_item(&conn, &item_id, &reason, EventActor::User)?
    };
    emit_item(&app, &item);
    emit_item_event(&app, &event);
    Ok(item)
}

/// The one write behind [`roadmap_hold_item`] and the PM's op: place the hold and
/// record it in the caller's single lock scope, so an item that says it is held
/// always carries the line saying who stopped it and why.
pub(crate) fn hold_item(
    conn: &Connection,
    item_id: &str,
    reason: &str,
    by: EventActor,
) -> Result<(RoadmapItem, ItemEvent), String> {
    let item = holds::hold_item(conn, item_id, reason, by)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("roadmap item {item_id} no longer exists"))?;
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

/// Lift an item's hold — **the user's alone**. There is no RPC op for this: an
/// agent that could release its own brake has no brake (see [`holds`]).
///
/// Records a `released` event whose detail is the reason being lifted, so the
/// trail reads as a pair — why we stopped, and that we resumed — rather than as
/// an unexplained resumption. Nudges the drainer, because this item may be the
/// head of the queue.
#[tauri::command]
pub async fn roadmap_release_item(
    item_id: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<RoadmapItem, String> {
    let (item, event) = {
        let conn = db.lock();
        release_item(&conn, &item_id)?
    };
    emit_item(&app, &item);
    // No event when the row wasn't held: releasing something nobody stopped is a
    // no-op, and a `released` line for it would be a fact that never happened.
    if let Some(event) = &event {
        emit_item_event(&app, event);
        drainer::nudge();
    }
    Ok(item)
}

/// The one write behind [`roadmap_release_item`]: clear the hold and record what
/// was lifted, in the caller's single lock scope.
fn release_item(
    conn: &Connection,
    item_id: &str,
) -> Result<(RoadmapItem, Option<ItemEvent>), String> {
    let (item, lifted) = holds::release_item(conn, item_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("roadmap item {item_id} no longer exists"))?;
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

/// The project's hold, or `None` when the board is running — the board load's
/// fourth companion to [`roadmap_list_items`]; live changes arrive on
/// `roadmap:project-hold` / `roadmap:project-hold-released`.
#[tauri::command]
pub async fn roadmap_get_project_hold(
    project_id: String,
    db: tauri::State<'_, Db>,
) -> Result<Option<ProjectHold>, String> {
    let conn = db.lock();
    holds::get_project(&conn, &project_id).map_err(|e| e.to_string())
}

/// Stop the whole board: nothing dispatches until the user lifts it.
///
/// No history event, unlike an item hold — there is no one item a board-wide
/// stop belongs to, and writing a `held` line onto every row would bury the trail
/// under a fact that isn't about any of them. The hold row *is* the durable
/// record (invariant 3), and it is what the banner and the strip read.
#[tauri::command]
pub async fn roadmap_hold_project(
    project_id: String,
    reason: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<ProjectHold, String> {
    let reason = holds::clean_reason(&reason)?;
    let hold = {
        let conn = db.lock();
        holds::hold_project(&conn, &project_id, &reason, EventActor::User)
            .map_err(|e| e.to_string())?
    };
    emit_project_hold(&app, &hold);
    Ok(hold)
}

/// Let the board run again — the user's alone, like every release. Nudges the
/// drainer, because a queue that was frozen may have work waiting.
#[tauri::command]
pub async fn roadmap_release_project(
    project_id: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<(), String> {
    {
        let conn = db.lock();
        holds::release_project(&conn, &project_id).map_err(|e| e.to_string())?;
    }
    // Announced even when nothing was removed: the caller's board should end up
    // showing no banner either way, and a second release is not an error.
    emit_project_hold_released(&app, &project_id);
    drainer::nudge();
    Ok(())
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

// ───────────────────────── the decision log ─────────────────────────────

/// Rule an item off the board — the decision that used to be a delete, kept as
/// a row instead. The item becomes `rejected` with `close_reason` = why, its
/// trail intact and a `rejected` line appended, so "we are not doing this, and
/// here is why" survives to be read — by the card, by the PM's next session,
/// and by whoever wonders in three months. Reopening
/// ([`roadmap_reopen_item`]) is the undo.
///
/// The reason is mandatory: the row this leaves behind *is* the decision log,
/// and a rejection that can't say why is just a slower delete. Allowed from the
/// pre-work statuses only (`proposed | open | queued` — a queued item leaves
/// the queue by being rejected); an `active`/`in_review` item has an agent on
/// it, and a `done` one already shipped — neither can be un-decided from here.
///
/// A rejection supersedes a pause: any hold is cleared (the trail keeps the
/// hold's history), the agent stamp comes off, and a pending PM proposal on the
/// item is consumed — a dead item's ask shouldn't haunt anything.
#[tauri::command]
pub async fn roadmap_reject_item(
    item_id: String,
    reason: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<RoadmapItem, String> {
    let (item, event, pending) = {
        let conn = db.lock();
        reject_item(&conn, &item_id, &reason)?
    };
    emit_item(&app, &item);
    emit_item_event(&app, &event);
    if let Some(p) = &pending {
        emit_proposal_deleted(&app, &p.id);
    }
    // A rejected item leaves the queue, and a dependant waiting on it is now
    // wedged rather than waiting — the drainer should say which within a beat,
    // not a tick.
    drainer::nudge();
    Ok(item)
}

/// The one write behind [`roadmap_reject_item`]: check the gate, consume any
/// pending proposal, flip the row, and record the ruling — one lock scope, so a
/// row that says it is rejected always carries the line saying why.
fn reject_item(
    conn: &Connection,
    item_id: &str,
    reason: &str,
) -> Result<(RoadmapItem, ItemEvent, Option<Proposal>), String> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(
            "`reason` is required — the rejected row is the decision log, and the reason \
             is the decision"
                .into(),
        );
    }
    let current = store::get(conn, item_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("roadmap item {item_id} no longer exists"))?;
    match current.status {
        ItemStatus::Proposed | ItemStatus::Open | ItemStatus::Queued => {}
        ItemStatus::Active | ItemStatus::InReview => {
            return Err(format!(
                "{} is {} — an agent is on it; cancel or settle the run before ruling \
                 this off the board",
                current.code,
                current.status.as_str()
            ))
        }
        ItemStatus::Done => {
            return Err(format!(
                "{} is done — shipped work can't be un-decided",
                current.code
            ))
        }
        ItemStatus::Rejected => {
            return Err(format!("{} is already rejected", current.code));
        }
    }
    // Consumed under the same lock as the ruling, like the delete path: the
    // board would otherwise count a ghost proposal forever.
    let pending = proposals::for_item(conn, item_id).map_err(|e| e.to_string())?;
    if let Some(p) = &pending {
        proposals::delete(conn, &p.id).map_err(|e| e.to_string())?;
    }
    let item = store::reject(conn, item_id, reason)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("roadmap item {item_id} no longer exists"))?;
    let event = events::record(
        conn,
        &item.id,
        &item.project_id,
        EventActor::User,
        EventKind::Rejected,
        Some(reason),
    )
    .map_err(|e| e.to_string())?;
    Ok((item, event, pending))
}

/// Put a rejected item back on the board, at `open` — the decision log's undo,
/// and the only exit from `rejected`.
///
/// Records a `reopened` event whose detail quotes the reason being shed
/// ("was rejected — …"), so the trail reads as a pair — why we ruled it off,
/// and that we changed our mind — rather than as an unexplained resurrection.
#[tauri::command]
pub async fn roadmap_reopen_item(
    item_id: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<RoadmapItem, String> {
    let (item, event) = {
        let conn = db.lock();
        reopen_item(&conn, &item_id)?
    };
    emit_item(&app, &item);
    // No event when the row wasn't rejected: like a release on an unheld row,
    // reopening something already open is a no-op, and a `reopened` line for it
    // would be a fact that never happened.
    if let Some(event) = &event {
        emit_item_event(&app, event);
        // A dependant wedged on this item's rejection is merely waiting again.
        drainer::nudge();
    }
    Ok(item)
}

/// The one write behind [`roadmap_reopen_item`]: the guarded flip and its
/// record, in the caller's single lock scope. The precondition rides
/// [`store::reopen`]'s own `WHERE` (`status = rejected`), so a stale click —
/// the item was already reopened, or was never rejected — misses cleanly:
/// nothing is written, nothing is stamped, and the caller gets the row as it
/// actually is.
fn reopen_item(
    conn: &Connection,
    item_id: &str,
) -> Result<(RoadmapItem, Option<ItemEvent>), String> {
    // Read before the flip: the detail the `reopened` event quotes is the
    // reason the write is about to clear.
    let current = store::get(conn, item_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("roadmap item {item_id} no longer exists"))?;
    let Some(item) = store::reopen(conn, item_id).map_err(|e| e.to_string())? else {
        return Ok((current, None));
    };
    // `close_reason` is `Some` exactly when the row is rejected, but a row
    // written before that invariant held costs a shorter line, not a panic.
    let detail = match &current.close_reason {
        Some(reason) => format!("was rejected — {reason}"),
        None => "was rejected".to_string(),
    };
    let event = events::record(
        conn,
        &item.id,
        &item.project_id,
        EventActor::User,
        EventKind::Reopened,
        Some(&detail),
    )
    .map_err(|e| e.to_string())?;
    Ok((item, Some(event)))
}

/// Delete an item. Silent when the row is already gone — the caller's intent
/// ("this should not be on the board") is satisfied either way.
///
/// This is typo cleanup, not a ruling: the write for "we decided against this"
/// is [`roadmap_reject_item`], which keeps the row and the reason. Deletion
/// records no history event on purpose — `roadmap_item_events` cascades with
/// the row, so a deleted item takes its trail with it, which is exactly right
/// for a row that should never have existed and exactly wrong for a decision.
///
/// One thing does have to outlive the row: a *routed issue's* refusal. A ghost
/// the issue funnel created carries the tracker URL it came from
/// ([`RoadmapItem::issue_url`]), and discarding it is the user saying "not this
/// one" — a decision the inbox must respect after a reload, when the row it was
/// expressed on is gone. So the tombstone is written here, in the same lock scope
/// as the delete, from the row as it was (see [`store::decline_issue`]). Only for
/// a still-`proposed` row: removing an item the user already *accepted* is a
/// different decision, and re-offering that issue later is correct.
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
        // Read before the delete for the same reason: the routing record this row
        // may carry is only legible while the row exists.
        let doomed = store::get(&conn, &id).map_err(|e| e.to_string())?;
        let removed = store::delete(&conn, &id).map_err(|e| e.to_string())?;
        if removed {
            if let Some(row) = doomed.filter(|r| r.status == ItemStatus::Proposed) {
                if let Some(url) = row.issue_url.as_deref() {
                    store::decline_issue(&conn, &row.project_id, url).map_err(|e| e.to_string())?;
                }
            }
        }
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

/// The newest event of every item on a project's board, newest first — one read
/// for the board-wide "what does this item's trail say now" question the "Needs
/// you" strip asks (a `blocked` item whose trail moved on is not blocked). Read
/// only; live rows arrive on `roadmap:item-event` like every other trail row.
///
/// Board-scoped rather than per item on purpose: [`roadmap_list_item_events`] is
/// the lazy per-card fetch, and the strip must see items nobody expanded.
#[tauri::command]
pub async fn roadmap_latest_events(
    project_id: String,
    db: tauri::State<'_, Db>,
) -> Result<Vec<ItemEvent>, String> {
    let conn = db.lock();
    events::latest_per_item(&conn, &project_id).map_err(|e| e.to_string())
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
    /// The row changed; emit it and its event. Both kinds of ask land here — an
    /// update's patch, and a discard's rejection (the item stays on the board as
    /// the decision log, `rejected` with the PM's note as its `close_reason`).
    /// Boxed: a ruling is almost always this variant, but the enum's size is
    /// set by it, and the row + event pair dwarfs the other arm.
    Updated {
        item: Box<RoadmapItem>,
        event: Box<ItemEvent>,
    },
    /// The board outran the ask — the item went `active`+ since the PM
    /// proposed, or the dep list it asked for no longer resolves (or would now
    /// close a loop). The proposal was deleted without applying, and the message
    /// says why.
    Stale { message: String },
}

/// May a proposal still be applied to this item? The set lives on the status
/// itself ([`ItemStatus::is_rulable`]) — one predicate, shared with the PM-side
/// refusal in `rpc::roadmap`, so the two gates can never drift apart. Only the
/// message stays here: this one is read by the user, on the card's bar.
///
/// A held item still passes. It is paused, not sealed — see the predicate's docs.
fn proposal_gate(item: &RoadmapItem) -> Result<(), String> {
    if item.status.is_rulable() {
        return Ok(());
    }
    // Name the actual objection, not a generic one: "being built or reviewed"
    // said of a shipped or rejected item would make the bar's refusal read as a
    // bug rather than an answer.
    let why = match item.status {
        ItemStatus::Done => "shipped work can't be reshaped by proposal",
        ItemStatus::Rejected => {
            "an item ruled off the board can't be reshaped by proposal — reopen it first"
        }
        _ => "an item being built or reviewed can't be reshaped by proposal",
    };
    Err(format!("{} is {} — {why}", item.code, item.status.as_str()))
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
/// have gone `active` since the PM asked) and the dep graph ([`deps`]), then
/// apply-and-record or drop the stale ask. The proposal row is gone on every
/// path — a ruling consumes it, and a dead ask shouldn't haunt the card.
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
            // Re-checked here, not just when the PM asked: an item the patch
            // depends on can have been deleted, and another item can have taken
            // a dep on *this* one, since — either of which turns a dep list that
            // was fine into a dangling reference or a loop. Same policy as a
            // stale ask: refuse, say why, and consume the proposal rather than
            // leaving a bar that fails every time it is clicked.
            if let Some(new_deps) = &patch.deps {
                if let Err(why) = check_dep_edit(conn, &item, new_deps) {
                    proposals::delete(conn, proposal_id).map_err(|e| e.to_string())?;
                    return Ok(Ruling::Stale {
                        message: format!("the board changed since the PM asked — {why}"),
                    });
                }
            }
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
            // An accepted discard used to delete the row — "an item ruled off
            // the board needs no trail". The decision log reverses that: ruled-
            // off items ARE the trail, so the row stays, `rejected`, with the
            // PM's rationale as its `close_reason`. The op requires a note, so
            // the fallback is for a stored ask that predates the requirement.
            let reason = proposal
                .note
                .as_deref()
                .unwrap_or("discarded at the PM's ask");
            let rejected = store::reject(conn, &item.id, reason)
                .map_err(|e| e.to_string())?
                .ok_or("the item this proposal targets no longer exists")?;
            let event = events::record(
                conn,
                &rejected.id,
                &rejected.project_id,
                // The ruling writes history, not the ask — same doctrine as the
                // Update arm above.
                EventActor::User,
                EventKind::Rejected,
                Some(&ruling_detail("Rejected", proposal.note.as_deref())),
            )
            .map_err(|e| e.to_string())?;
            proposals::delete(conn, proposal_id).map_err(|e| e.to_string())?;
            Ok(Ruling::Updated {
                item: Box::new(rejected),
                event: Box::new(event),
            })
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
            // A patch can change horizon or deps, which can unblock (or
            // re-order) whatever is queued behind this item; a discard takes a
            // row out of the queue and wedges its dependants, which the drainer
            // should say within a beat.
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

// ────────────────────────── product brief ───────────────────────────────

/// The project's product brief, or `None` when the PM hasn't been given one —
/// the Product brief tab's load ([`memory`]). Live changes arrive on
/// `roadmap:brief`.
#[tauri::command]
pub async fn roadmap_get_brief(
    project_id: String,
    db: tauri::State<'_, Db>,
) -> Result<Option<Brief>, String> {
    let conn = db.lock();
    memory::load(&conn, &project_id).map_err(|e| e.to_string())
}

/// The PM's pending ask to replace that brief, if any — fetched with the brief;
/// live rows arrive on `roadmap:brief-proposal`.
#[tauri::command]
pub async fn roadmap_get_brief_proposal(
    project_id: String,
    db: tauri::State<'_, Db>,
) -> Result<Option<BriefProposal>, String> {
    let conn = db.lock();
    memory::get_proposal(&conn, &project_id).map_err(|e| e.to_string())
}

/// Accept the PM's proposed brief — the user's "yes", and the *only* thing that
/// writes product memory.
///
/// One lock scope for both writes ([`memory::accept`]), so the brief can never be
/// replaced while its ask survives to be applied a second time. No item events: a
/// brief belongs to no item, and a line on every row would bury the trail under a
/// fact that isn't about any of them — the brief row itself is the durable object
/// (invariant 3), exactly as for a project hold and an order ask.
#[tauri::command]
pub async fn roadmap_accept_brief_proposal(
    project_id: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<Brief, String> {
    let applied = {
        let conn = db.lock();
        memory::accept(&conn, &project_id).map_err(|e| e.to_string())?
    };
    // The ask is consumed on every path, so the tab's bar clears either way.
    emit_brief_proposal_deleted(&app, &project_id);
    let brief = applied.ok_or("this brief update has already been ruled on")?;
    emit_brief(&app, &brief);
    Ok(brief)
}

/// Decline the proposed brief — the standing one is untouched.
///
/// Writes no history for the same reason a declined order ask doesn't: there is no
/// item the refusal is about. What the PM learns from a decline is that the brief
/// it reads next session is unchanged, which is the honest record.
#[tauri::command]
pub async fn roadmap_reject_brief_proposal(
    project_id: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<(), String> {
    {
        let conn = db.lock();
        memory::delete_proposal(&conn, &project_id).map_err(|e| e.to_string())?;
    }
    emit_brief_proposal_deleted(&app, &project_id);
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

    /// The app's one connection, as the commands take it.
    fn test_db(conn: Connection) -> Db {
        Arc::new(Mutex::new(conn))
    }

    /// The Merge button is the one *user* action that can hand a held item to
    /// GitHub — it arms an auto-merge, and the sweep ships whatever merges. So it
    /// refuses while either scope's hold stands, and names the reason.
    #[test]
    fn merging_is_refused_while_a_hold_stands() {
        let conn = test_conn();
        let it = with_status(&conn, ItemStatus::InReview);
        let other = with_status(&conn, ItemStatus::InReview);
        let db = test_db(conn);

        assert!(merge_hold_gate(&db, &it.id).is_ok(), "nothing stops it");
        {
            let conn = db.lock();
            holds::hold_item(&conn, &it.id, "we agreed something else", EventActor::Pm).unwrap();
        }
        let refused = merge_hold_gate(&db, &it.id).unwrap_err();
        assert!(refused.contains(&it.code), "{refused}");
        assert!(refused.contains("we agreed something else"), "{refused}");
        assert!(refused.contains("Release the hold"), "{refused}");
        // One item's hold is not the board's: the other card still merges.
        assert!(merge_hold_gate(&db, &other.id).is_ok());

        // The board-wide brake refuses every card, including one with no hold of
        // its own — this is the scope the merge path never used to read.
        {
            let conn = db.lock();
            holds::hold_project(&conn, "p1", "re-planning the quarter", EventActor::User).unwrap();
        }
        let board = merge_hold_gate(&db, &other.id).unwrap_err();
        assert!(board.contains("re-planning the quarter"), "{board}");

        // A row that is gone is the missing-PR complaint's business, not the hold's.
        assert!(merge_hold_gate(&db, "no-such-item").is_ok());
    }

    fn status_patch(to: ItemStatus) -> ItemPatch {
        ItemPatch {
            status: Some(to),
            ..Default::default()
        }
    }

    /// A hand-built item opens its own history. Without this line a board the
    /// user typed in has no events at all, so every "what moved since we last
    /// spoke?" reader — the PM's standup digest above all — calls it unchanged.
    #[test]
    fn creating_an_item_records_created() {
        let conn = test_conn();
        let (item, event) = create_checked(
            &conn,
            "p1",
            &NewItem {
                title: "hand-written".into(),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(event.item_id, item.id);
        assert_eq!(event.project_id, "p1");
        assert_eq!(event.kind, EventKind::Created);
        assert_eq!(event.actor, EventActor::User);
        assert_eq!(event.detail, None);
        // Exactly one, and it is the row's whole history so far.
        assert_eq!(events::list_for_item(&conn, &item.id).unwrap(), vec![event]);
    }

    /// The insert and its history line ride one transaction, so a failed event
    /// write can't leave a row with no provenance. Forced by pointing the write
    /// at a project id the FK refuses.
    #[test]
    fn a_failed_create_leaves_no_row_behind() {
        let conn = test_conn();
        assert!(create_checked(
            &conn,
            "no-such-project",
            &NewItem {
                title: "orphan".into(),
                ..Default::default()
            },
        )
        .is_err());
        assert!(store::list(&conn, "no-such-project").unwrap().is_empty());
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
                update_and_record(&conn, &it.id, &status_patch(to), Some(from), false).unwrap();
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
            false,
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
        let (_, event) = update_and_record(&conn, &it.id, &patch, None, false).unwrap();
        assert_eq!(event.unwrap().kind, EventKind::Edited);
    }

    // ───────────────────────── the autonomy dial ────────────────────────

    /// Turn a project's dial on.
    fn set_autoqueue(conn: &Connection, on: bool) {
        conn.execute(
            "INSERT OR REPLACE INTO project_settings (project_id, key, value)
             VALUES ('p1', 'roadmap.autoqueue', ?1)",
            rusqlite::params![if on { "1" } else { "0" }],
        )
        .unwrap();
    }

    /// Accept one proposed row, with or without the "Accept & queue" flag, and
    /// report where it landed and what the trail says about it. Exactly the call
    /// [`roadmap_update_item`] makes.
    fn accept(conn: &Connection, id: &str, queue: bool) -> (ItemStatus, EventKind, Option<String>) {
        let (outcome, event) = update_and_record(
            conn,
            id,
            &status_patch(ItemStatus::Open),
            Some(ItemStatus::Proposed),
            queue,
        )
        .unwrap();
        let outcome = outcome.expect("the row is there");
        assert!(outcome.applied);
        let event = event.expect("an applied accept records itself");
        (outcome.item.status, event.kind, event.detail)
    }

    /// The whole rule, without a database: the button, the dial, and the brake.
    #[test]
    fn the_dial_and_the_button_land_in_the_same_place() {
        // Neither asked: today's accept, unchanged.
        assert_eq!(accept_landing(false, false, false), Landing::Board);
        // Either one queues it — one implementation, two doors.
        assert_eq!(accept_landing(true, false, false), Landing::Queue);
        assert_eq!(accept_landing(false, true, false), Landing::Queue);
        assert_eq!(accept_landing(true, true, false), Landing::Queue);
        // A hold beats both, and beats them the same way (invariant 2: a hold can
        // only ever reduce autonomy).
        assert_eq!(accept_landing(true, false, true), Landing::HeldBack);
        assert_eq!(accept_landing(false, true, true), Landing::HeldBack);
        // A hold on a row nobody asked to queue changes nothing — the accept
        // itself is the user's ruling and is never refused.
        assert_eq!(accept_landing(false, false, true), Landing::Board);

        // Each landing's status and its one line of trail.
        assert_eq!(Landing::Board.status(), ItemStatus::Open);
        assert_eq!(Landing::Queue.status(), ItemStatus::Queued);
        assert_eq!(Landing::HeldBack.status(), ItemStatus::Open);
        assert_eq!(Landing::Board.detail(), None);
        assert!(Landing::Queue.detail().is_some());
        assert!(Landing::HeldBack.detail().is_some());
    }

    /// With the dial on, accepting is the only touch before a run starts: the row
    /// lands `queued`, and one `accepted` event says how it got there.
    #[test]
    fn autoqueue_lands_an_accepted_item_in_the_queue() {
        let conn = test_conn();
        set_autoqueue(&conn, true);
        let it = with_status(&conn, ItemStatus::Proposed);

        let (status, kind, detail) = accept(&conn, &it.id, false);
        assert_eq!(status, ItemStatus::Queued);
        // Still `accepted`: one ruling by one actor, whatever the dial did with
        // it. The detail is what names the dial's part.
        assert_eq!(kind, EventKind::Accepted);
        assert_eq!(detail.as_deref(), Some("auto-queued"));
        // One line, not an `accepted` + `queued` pair.
        assert_eq!(events::list_for_item(&conn, &it.id).unwrap().len(), 1);
    }

    /// With the dial off, the accept is unchanged — and the same code path takes
    /// "Accept & queue" (`queue: true`) to `queued`, so the button and the dial
    /// can never disagree about where an accepted item goes.
    #[test]
    fn the_dial_off_accepts_to_the_board_unless_the_click_asked_to_queue() {
        let conn = test_conn();
        let plain = with_status(&conn, ItemStatus::Proposed);
        assert_eq!(
            accept(&conn, &plain.id, false),
            (ItemStatus::Open, EventKind::Accepted, None)
        );

        let queued = with_status(&conn, ItemStatus::Proposed);
        let (status, kind, detail) = accept(&conn, &queued.id, true);
        assert_eq!(status, ItemStatus::Queued);
        assert_eq!(kind, EventKind::Accepted);
        assert_eq!(detail.as_deref(), Some("auto-queued"));
    }

    /// Holds trump the dial. A held row (and every row of a held board) is
    /// accepted onto the roadmap and left `open` — the brake the PM can pull is
    /// exactly what makes high-autonomy mode safe, so it must survive the mode
    /// that needs it. The trail says why, because otherwise the user who pressed
    /// "Accept & queue" has an `open` item and no explanation.
    #[test]
    fn a_hold_keeps_an_accepted_item_off_the_queue() {
        let conn = test_conn();
        set_autoqueue(&conn, true);

        let held = with_status(&conn, ItemStatus::Proposed);
        holds::hold_item(
            &conn,
            &held.id,
            "confirm the direction first",
            EventActor::Pm,
        )
        .unwrap();
        let (status, kind, detail) = accept(&conn, &held.id, true);
        assert_eq!(status, ItemStatus::Open, "the item's own hold stopped it");
        assert_eq!(kind, EventKind::Accepted);
        assert!(detail.unwrap().contains("held"));

        // The board-wide brake does the same for a row with no hold of its own.
        let ordinary = with_status(&conn, ItemStatus::Proposed);
        holds::hold_project(&conn, "p1", "re-planning the quarter", EventActor::Pm).unwrap();
        assert_eq!(accept(&conn, &ordinary.id, false).0, ItemStatus::Open);

        // Released, the dial applies again — nothing about the accept path
        // remembers the hold.
        assert!(holds::release_project(&conn, "p1").unwrap());
        let after = with_status(&conn, ItemStatus::Proposed);
        assert_eq!(accept(&conn, &after.id, false).0, ItemStatus::Queued);
    }

    /// The dial reaches exactly one transition. A queue action, an unqueue, or a
    /// form edit is not an accept, so neither the setting nor the flag may touch
    /// it — otherwise "Accept & queue"'s flag, sent by any caller, would become a
    /// second way to queue anything.
    #[test]
    fn nothing_but_an_accept_is_redirected() {
        let conn = test_conn();
        set_autoqueue(&conn, true);

        // An unqueue with the flag set stays an unqueue.
        let queued = with_status(&conn, ItemStatus::Queued);
        let (outcome, event) = update_and_record(
            &conn,
            &queued.id,
            &status_patch(ItemStatus::Open),
            Some(ItemStatus::Queued),
            true,
        )
        .unwrap();
        assert_eq!(outcome.unwrap().item.status, ItemStatus::Open);
        let event = event.unwrap();
        assert_eq!(event.kind, EventKind::Unqueued);
        assert_eq!(event.detail, None);

        // And a plain edit is still an edit, with no status invented for it.
        let open = with_status(&conn, ItemStatus::Open);
        let (outcome, event) = update_and_record(
            &conn,
            &open.id,
            &ItemPatch {
                title: Some("retitled".into()),
                ..Default::default()
            },
            None,
            true,
        )
        .unwrap();
        assert_eq!(outcome.unwrap().item.status, ItemStatus::Open);
        assert_eq!(event.unwrap().kind, EventKind::Edited);

        assert!(is_accept(
            Some(ItemStatus::Proposed),
            &status_patch(ItemStatus::Open)
        ));
        assert!(!is_accept(None, &status_patch(ItemStatus::Open)));
        assert!(!is_accept(
            Some(ItemStatus::Proposed),
            &status_patch(ItemStatus::Queued)
        ));
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

    // ───────────────────────────── holds ────────────────────────────────

    /// The item brake, end to end through the command layer's one write: the
    /// trio lands on the row, a `held` line lands on the trail carrying the
    /// reason, and the status doesn't move — a hold stops progress, it doesn't
    /// take the item off the queue.
    #[test]
    fn holding_an_item_records_the_reason_and_moves_nothing() {
        let conn = test_conn();
        let it = with_status(&conn, ItemStatus::Queued);

        let (held, event) = hold_item(&conn, &it.id, "confirm the scope", EventActor::Pm).unwrap();
        assert_eq!(held.hold_reason.as_deref(), Some("confirm the scope"));
        assert_eq!(held.held_by, Some(EventActor::Pm));
        assert!(held.held_at.is_some());
        assert_eq!(held.status, ItemStatus::Queued, "a hold is not an unqueue");
        assert_eq!(event.kind, EventKind::Held);
        assert_eq!(event.actor, EventActor::Pm);
        assert_eq!(event.detail.as_deref(), Some("confirm the scope"));
        assert_eq!(events::list_for_item(&conn, &it.id).unwrap(), vec![event]);

        // A release names what it lifted, so the trail reads as a pair.
        let (released, event) = release_item(&conn, &it.id).unwrap();
        assert!(!released.is_held());
        assert_eq!(released.held_by, None);
        assert_eq!(released.held_at, None);
        assert_eq!(released.status, ItemStatus::Queued);
        let event = event.expect("lifting a real hold is history");
        assert_eq!(event.kind, EventKind::Released);
        // Always the user: there is no release op, so nothing else can get here.
        assert_eq!(event.actor, EventActor::User);
        assert_eq!(event.detail.as_deref(), Some("confirm the scope"));
    }

    /// A second hold replaces the reason on the row and *adds* to the trail —
    /// the row answers "why is it held", the trail answers "what has it been
    /// held for".
    #[test]
    fn re_holding_replaces_the_reason_and_keeps_both_lines() {
        let conn = test_conn();
        let it = with_status(&conn, ItemStatus::Open);
        hold_item(&conn, &it.id, "first reason", EventActor::Pm).unwrap();
        let (held, _) = hold_item(&conn, &it.id, "sharper reason", EventActor::User).unwrap();

        assert_eq!(held.hold_reason.as_deref(), Some("sharper reason"));
        assert_eq!(held.held_by, Some(EventActor::User));
        let trail = events::list_for_item(&conn, &it.id).unwrap();
        assert_eq!(trail.len(), 2, "the superseded reason survives as history");
        assert_eq!(trail[0].detail.as_deref(), Some("sharper reason"));
        assert_eq!(trail[1].detail.as_deref(), Some("first reason"));
        assert!(trail.iter().all(|e| e.kind == EventKind::Held));
    }

    /// Releasing something nobody held writes no line: a `released` event for a
    /// hold that never existed would be a fact that didn't happen. The row still
    /// comes back, so the strip's one-click release can't fail because someone
    /// else lifted it a moment earlier.
    #[test]
    fn releasing_an_unheld_item_is_a_quiet_no_op() {
        let conn = test_conn();
        let it = with_status(&conn, ItemStatus::Open);
        let (row, event) = release_item(&conn, &it.id).unwrap();
        assert_eq!(row.id, it.id);
        assert!(event.is_none());
        assert!(events::list_for_item(&conn, &it.id).unwrap().is_empty());
        assert!(release_item(&conn, "no-such-item").is_err());
    }

    /// The board brake: the hold row is the whole record (no per-item history),
    /// and it survives a fresh read — which is what makes it a durable object
    /// rather than a toast.
    #[test]
    fn holding_a_project_records_the_row_and_no_item_history() {
        let conn = test_conn();
        let it = with_status(&conn, ItemStatus::Queued);

        let hold =
            holds::hold_project(&conn, "p1", "waiting on the design call", EventActor::Pm).unwrap();
        assert_eq!(hold.reason, "waiting on the design call");
        assert_eq!(hold.held_by, EventActor::Pm);
        assert_eq!(holds::get_project(&conn, "p1").unwrap(), Some(hold));
        assert!(
            events::list_for_item(&conn, &it.id).unwrap().is_empty(),
            "a board-wide stop belongs to no row"
        );

        assert!(holds::release_project(&conn, "p1").unwrap());
        assert!(holds::get_project(&conn, "p1").unwrap().is_none());
    }

    /// The gate-unification pin (the follow-up B5 was named for): `rulable` in
    /// the RPC and `proposal_gate` here read the same predicate, and **a hold is
    /// not part of it**. A held item is paused, not sealed — the reason to stop
    /// autonomous progress is usually that the item's shape is wrong, so the
    /// proposal that fixes it has to be rulable while the hold stands.
    #[test]
    fn a_hold_pauses_an_item_without_sealing_it_to_proposals() {
        use ItemStatus::{Active, Done, InReview, Open, Proposed, Queued};
        // One set, both gates.
        for status in [Proposed, Open, Queued] {
            assert!(status.is_rulable(), "{}", status.as_str());
            assert!(proposal_gate(&with_status(&test_conn(), status)).is_ok());
        }
        for status in [Active, InReview, Done] {
            assert!(!status.is_rulable(), "{}", status.as_str());
            let refusal = proposal_gate(&with_status(&test_conn(), status)).unwrap_err();
            assert!(refusal.contains(status.as_str()), "{refusal}");
        }

        // And the ruling still applies on a held row.
        let conn = test_conn();
        let it = with_status(&conn, ItemStatus::Queued);
        let p = pending_update(&conn, &it, Some("this is what it should have said"));
        hold_item(&conn, &it.id, "the scope is wrong", EventActor::Pm).unwrap();

        let Ruling::Updated { item, .. } = accept_proposal(&conn, &p.id).unwrap() else {
            panic!("a held item is paused, not sealed");
        };
        assert_eq!(item.title, "reshaped");
        assert!(item.is_held(), "and the hold outlives the ruling");
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
        assert!(reclaim(&conn, &bare.id)
            .unwrap_err()
            .contains("with an agent"));

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
            assert_eq!(
                row.agent_id.as_deref(),
                Some("w1"),
                "a refusal writes nothing"
            );
            assert!(events::list_for_item(&conn, &it.id).unwrap().is_empty());
        }
        assert!(reclaim(&conn, "no-such-item").is_err());
    }

    // ────────────────────────── the decision log ────────────────────────

    /// The ruling that used to be a delete: every pre-work status may be ruled
    /// off the board, and the row that stays says why — trimmed, on the row
    /// *and* as the `rejected` line's detail, so the card and the trail quote
    /// the same sentence.
    #[test]
    fn rejecting_keeps_the_row_and_the_reason_from_any_prework_status() {
        let conn = test_conn();
        for status in [ItemStatus::Proposed, ItemStatus::Open, ItemStatus::Queued] {
            let it = with_status(&conn, status);
            let (item, event, pending) =
                reject_item(&conn, &it.id, "  out of scope for v1  ").unwrap();
            assert_eq!(item.status, ItemStatus::Rejected);
            assert_eq!(item.close_reason.as_deref(), Some("out of scope for v1"));
            assert!(pending.is_none(), "nothing was pending on this item");
            assert_eq!(event.kind, EventKind::Rejected);
            assert_eq!(event.actor, EventActor::User);
            assert_eq!(event.detail.as_deref(), Some("out of scope for v1"));
            assert_eq!(events::list_for_item(&conn, &it.id).unwrap(), vec![event]);
        }
    }

    /// A rejection supersedes a pause: the hold trio and the agent stamp come
    /// off with the status flip. The trail keeps the hold's history — the row
    /// only ever carries the state in force, and `rejected` is now that state.
    #[test]
    fn rejecting_clears_a_hold_and_the_agent_stamp() {
        let conn = test_conn();
        let it = with_status(&conn, ItemStatus::Open);
        holds::hold_item(&conn, &it.id, "direction unclear", EventActor::Pm).unwrap();
        store::update(
            &conn,
            &it.id,
            &ItemPatch {
                agent_id: Some(Some("w1".into())),
                ..Default::default()
            },
        )
        .unwrap();

        let (item, _, _) = reject_item(&conn, &it.id, "not doing this after all").unwrap();
        assert!(!item.is_held());
        assert_eq!(item.held_by, None);
        assert_eq!(item.held_at, None);
        assert_eq!(item.agent_id, None);
        assert_eq!(
            item.close_reason.as_deref(),
            Some("not doing this after all")
        );
    }

    /// A blank reason refuses the whole rejection: the rejected row is the
    /// decision log, and a rejection that can't say why is just a slower
    /// delete. Nothing is written on refusal.
    #[test]
    fn a_blank_reason_refuses_the_rejection() {
        let conn = test_conn();
        let it = with_status(&conn, ItemStatus::Open);
        let err = reject_item(&conn, &it.id, "   ").unwrap_err();
        assert!(err.contains("reason"), "{err}");
        let row = store::get(&conn, &it.id).unwrap().unwrap();
        assert_eq!(row.status, ItemStatus::Open);
        assert_eq!(row.close_reason, None);
        assert!(events::list_for_item(&conn, &it.id).unwrap().is_empty());
    }

    /// The gate, naming the status it refuses for: an `active`/`in_review`
    /// item has an agent on it, a `done` one already shipped, and a `rejected`
    /// one has already been ruled on. A refusal writes nothing.
    #[test]
    fn rejecting_refuses_inflight_shipped_and_already_rejected_items() {
        let conn = test_conn();
        for status in [
            ItemStatus::Active,
            ItemStatus::InReview,
            ItemStatus::Done,
            ItemStatus::Rejected,
        ] {
            let it = with_status(&conn, status);
            let err = reject_item(&conn, &it.id, "changed our minds").unwrap_err();
            assert!(err.contains(&it.code), "{err}");
            assert!(err.contains(status.as_str()), "{err}");
            let row = store::get(&conn, &it.id).unwrap().unwrap();
            assert_eq!(row.status, status, "a refusal writes nothing");
            assert_eq!(row.close_reason, None);
            assert!(events::list_for_item(&conn, &it.id).unwrap().is_empty());
        }
        assert!(reject_item(&conn, "no-such-item", "why").is_err());
    }

    /// A dead item's ask shouldn't haunt anything: rejecting consumes the
    /// item's pending proposal, and hands it back so the caller can announce
    /// the deletion the board is watching for.
    #[test]
    fn rejecting_consumes_the_items_pending_proposal() {
        let conn = test_conn();
        let it = with_status(&conn, ItemStatus::Open);
        let p = pending_update(&conn, &it, Some("reshape it"));

        let (_, _, pending) = reject_item(&conn, &it.id, "no longer relevant").unwrap();
        assert_eq!(
            pending.map(|p| p.id),
            Some(p.id.clone()),
            "returned so the caller can emit `roadmap:proposal-deleted`"
        );
        assert!(proposals::get(&conn, &p.id).unwrap().is_none());
    }

    /// The decision log's undo: reopening lands the item back at `open` (not
    /// back in the queue — re-queueing is a fresh decision), sheds the reason,
    /// and quotes it in the trail so the pair reads honestly.
    #[test]
    fn reopening_returns_a_rejected_item_to_the_board() {
        let conn = test_conn();
        let it = with_status(&conn, ItemStatus::Queued);
        reject_item(&conn, &it.id, "parked for the rewrite").unwrap();

        let (item, event) = reopen_item(&conn, &it.id).unwrap();
        assert_eq!(item.status, ItemStatus::Open);
        assert_eq!(
            item.close_reason, None,
            "an item back in play owes nobody an epitaph"
        );
        let event = event.expect("a real reopen records itself");
        assert_eq!(event.kind, EventKind::Reopened);
        assert_eq!(event.actor, EventActor::User);
        assert_eq!(
            event.detail.as_deref(),
            Some("was rejected — parked for the rewrite")
        );
        // The trail keeps the pair: why it left the board, and that it's back.
        assert_eq!(events::list_for_item(&conn, &it.id).unwrap().len(), 2);

        // A second click races the first and loses cleanly: no second stamp.
        let (again, event) = reopen_item(&conn, &it.id).unwrap();
        assert_eq!(again.status, ItemStatus::Open);
        assert!(event.is_none());
    }

    /// Reopening something nobody rejected is a no-op that doesn't stamp: the
    /// precondition rides the write's own `WHERE`, so a stale click can't move
    /// a row that isn't rejected or invent history for a reopen that never
    /// happened.
    #[test]
    fn reopening_an_unrejected_item_is_a_noop_that_records_nothing() {
        let conn = test_conn();
        for status in [ItemStatus::Open, ItemStatus::Queued, ItemStatus::Done] {
            let it = with_status(&conn, status);
            let (item, event) = reopen_item(&conn, &it.id).unwrap();
            assert_eq!(item.status, status, "the row comes back as it actually is");
            assert!(event.is_none());
            assert!(events::list_for_item(&conn, &it.id).unwrap().is_empty());
        }
        assert!(reopen_item(&conn, "no-such-item").is_err());
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

    /// Accepting a discard keeps the row as the decision log: `rejected`, with
    /// the PM's rationale as its `close_reason` and a `rejected` line in the
    /// trail — where it used to delete the row and everything it knew.
    #[test]
    fn accepting_a_discard_keeps_the_row_as_rejected() {
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

        let Ruling::Updated { item, event } = accept_proposal(&conn, &p.id).unwrap() else {
            panic!("expected Updated");
        };
        assert_eq!(item.id, it.id, "the row survives the ruling");
        assert_eq!(item.status, ItemStatus::Rejected);
        assert_eq!(item.close_reason.as_deref(), Some("superseded by MCA-101"));
        // The ruling writes history, not the ask — the user rejected, carrying
        // the PM's rationale.
        assert_eq!(event.kind, EventKind::Rejected);
        assert_eq!(event.actor, EventActor::User);
        assert_eq!(
            event.detail.as_deref(),
            Some("Rejected a PM proposal — superseded by MCA-101")
        );
        assert!(proposals::get(&conn, &p.id).unwrap().is_none());
        // Ruling twice is refused, not replayed.
        assert!(accept_proposal(&conn, &p.id).is_err());
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

    /// A dep list the dialog sends is checked before it lands: an unknown code
    /// (which the drainer would read as "satisfied") and a loop (which would
    /// wedge the queue forever) are both refused, with nothing written.
    #[test]
    fn a_dep_edit_is_refused_when_it_closes_a_loop_or_names_nothing() {
        let conn = test_conn();
        let a = with_status(&conn, ItemStatus::Open);
        let b = with_status(&conn, ItemStatus::Open);

        let deps_patch = |codes: &[&str]| ItemPatch {
            deps: Some(codes.iter().map(|c| (*c).to_string()).collect()),
            ..Default::default()
        };

        // b after a: an ordinary edge, applied.
        let (outcome, _) =
            update_and_record(&conn, &b.id, &deps_patch(&[&a.code]), None, false).unwrap();
        assert_eq!(outcome.unwrap().item.deps, vec![a.code.clone()]);

        // a after b now closes the loop — refused, and the row is untouched.
        let err =
            update_and_record(&conn, &a.id, &deps_patch(&[&b.code]), None, false).unwrap_err();
        assert!(err.contains("loop"), "{err}");
        assert!(
            err.contains(&format!("{} → {} → {}", a.code, b.code, a.code)),
            "the refusal names the loop: {err}"
        );
        assert!(store::get(&conn, &a.id).unwrap().unwrap().deps.is_empty());

        // A code that isn't on the board at all is refused too.
        let err =
            update_and_record(&conn, &a.id, &deps_patch(&["MCA-999"]), None, false).unwrap_err();
        assert!(err.contains("MCA-999"), "{err}");

        // Self-reference, and the same rule on the create path.
        let err =
            update_and_record(&conn, &a.id, &deps_patch(&[&a.code]), None, false).unwrap_err();
        assert!(err.contains("depend on itself"), "{err}");
        let err = create_checked(
            &conn,
            "p1",
            &NewItem {
                title: "fresh".into(),
                deps: vec!["MCA-999".into()],
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(err.contains("MCA-999"), "{err}");
        // A create naming a real code is fine — a new row can't be depended on.
        assert!(create_checked(
            &conn,
            "p1",
            &NewItem {
                title: "fresh".into(),
                deps: vec![a.code.clone()],
                ..Default::default()
            },
        )
        .is_ok());
    }

    /// Re-stating the deps an item already has is not a refusal — even when they
    /// are part of a loop written before this check existed. The dialog posts the
    /// whole form, so blocking a retitle would strand the row; the drainer's
    /// durable `blocked` event is what surfaces a loop like that.
    #[test]
    fn resending_an_items_own_deps_is_not_refused() {
        let conn = test_conn();
        let a = with_status(&conn, ItemStatus::Open);
        let b = with_status(&conn, ItemStatus::Open);
        for (row, dep) in [(&a, &b.code), (&b, &a.code)] {
            store::update(
                &conn,
                &row.id,
                &ItemPatch {
                    deps: Some(vec![dep.clone()]),
                    ..Default::default()
                },
            )
            .unwrap();
        }

        let patch = ItemPatch {
            title: Some("retitled".into()),
            deps: Some(vec![b.code.clone()]),
            ..Default::default()
        };
        let (outcome, _) = update_and_record(&conn, &a.id, &patch, None, false).unwrap();
        assert_eq!(outcome.unwrap().item.title, "retitled");
    }

    /// A dep patch that was fine when the PM asked can be a loop by the time the
    /// user clicks: the board moved. Re-validated at ruling time, refused, and
    /// the ask is consumed rather than left to fail on every click.
    #[test]
    fn accepting_a_dep_patch_that_became_a_loop_refuses_and_clears() {
        let conn = test_conn();
        let a = with_status(&conn, ItemStatus::Open);
        let b = with_status(&conn, ItemStatus::Open);
        let patch = ProposalPatch {
            deps: Some(vec![b.code.clone()]),
            ..Default::default()
        };
        let p = proposals::upsert(
            &conn,
            "p1",
            &a.id,
            ProposalKind::Update,
            Some(&patch),
            Some("b first"),
        )
        .unwrap();
        // Meanwhile the user (or an earlier ruling) made b depend on a.
        store::update(
            &conn,
            &b.id,
            &ItemPatch {
                deps: Some(vec![a.code.clone()]),
                ..Default::default()
            },
        )
        .unwrap();

        let Ruling::Stale { message } = accept_proposal(&conn, &p.id).unwrap() else {
            panic!("expected Stale");
        };
        assert!(message.contains("the board changed"), "{message}");
        assert!(message.contains("loop"), "{message}");
        assert!(proposals::get(&conn, &p.id).unwrap().is_none());
        // Nothing applied, and no history invented for a write that didn't land.
        assert!(store::get(&conn, &a.id).unwrap().unwrap().deps.is_empty());
        assert!(events::list_for_item(&conn, &a.id).unwrap().is_empty());
    }

    /// The other half of the same re-check: the item the ask depends on was
    /// deleted since. Applying it blind would leave a dangling code the drainer
    /// silently treats as satisfied — so it refuses and says what vanished.
    #[test]
    fn accepting_a_dep_patch_whose_dependency_vanished_refuses() {
        let conn = test_conn();
        let a = with_status(&conn, ItemStatus::Open);
        let gone = with_status(&conn, ItemStatus::Open);
        let patch = ProposalPatch {
            deps: Some(vec![gone.code.clone()]),
            ..Default::default()
        };
        let p = proposals::upsert(&conn, "p1", &a.id, ProposalKind::Update, Some(&patch), None)
            .unwrap();
        store::delete(&conn, &gone.id).unwrap();

        let Ruling::Stale { message } = accept_proposal(&conn, &p.id).unwrap() else {
            panic!("expected Stale");
        };
        assert!(message.contains(&gone.code), "{message}");
        assert!(proposals::get(&conn, &p.id).unwrap().is_none());
        assert!(store::get(&conn, &a.id).unwrap().unwrap().deps.is_empty());
    }

    /// A dep patch that is still valid applies as any other patch does — the
    /// re-check is a gate, not a second refusal path for good asks.
    #[test]
    fn accepting_a_still_valid_dep_patch_applies_it() {
        let conn = test_conn();
        let a = with_status(&conn, ItemStatus::Open);
        let b = with_status(&conn, ItemStatus::Open);
        let patch = ProposalPatch {
            deps: Some(vec![b.code.clone()]),
            ..Default::default()
        };
        let p = proposals::upsert(&conn, "p1", &a.id, ProposalKind::Update, Some(&patch), None)
            .unwrap();

        let Ruling::Updated { item, .. } = accept_proposal(&conn, &p.id).unwrap() else {
            panic!("expected Updated");
        };
        assert_eq!(item.deps, vec![b.code]);
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
