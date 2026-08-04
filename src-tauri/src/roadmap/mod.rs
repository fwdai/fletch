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
//!
//! [`review`] closes it a third time, upwards: every run the drainer settles is
//! handed to the project-manager chat as a review turn, so the agent that wrote
//! the brief is the agent that reads what came back.
//!
//! [`pr_review`] is the sweep's foreground half: while a board is on screen it
//! answers the *review* questions about an `in_review` item's PR (CI, conflicts,
//! unresolved threads) so the user can judge, merge, or send the feedback back
//! to an agent without leaving the board.

pub mod deps;
pub mod drainer;
pub mod events;
pub mod merge_sweep;
pub mod order;
pub mod pr_review;
pub mod proposals;
pub mod review;
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
        update_and_record(&conn, &id, &patch, expect_status)?
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
///
/// A patch carrying `deps` is checked against [`deps`] *first*, in this same
/// guard: this command is the item dialog's door, and a dep list that closes a
/// loop would leave the queue skipping the whole chain forever. The refusal is
/// an `Err` the dialog renders in its error slot, not a silent drop.
fn update_and_record(
    conn: &Connection,
    id: &str,
    patch: &ItemPatch,
    expect_status: Option<ItemStatus>,
) -> Result<(Option<ItemUpdate>, Option<ItemEvent>), String> {
    if let Some(new_deps) = &patch.deps {
        // A row that is already gone falls through to the normal "no longer
        // exists" path below rather than being refused for its deps.
        if let Some(current) = store::get(conn, id).map_err(|e| e.to_string())? {
            check_dep_edit(conn, &current, new_deps)?;
        }
    }
    let updated = match expect_status {
        Some(expected) => store::update_where_status(conn, id, expected, patch),
        None => store::update(conn, id, patch),
    }
    .map_err(|e| e.to_string())?;
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
#[tauri::command]
pub async fn roadmap_merge_item_pr(
    item_id: String,
    db: tauri::State<'_, Db>,
) -> Result<(), String> {
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
    /// The patch landed; emit the row and the `edited` event. Boxed: a ruling
    /// is almost always this variant, but the enum's size is set by it, and
    /// the row + event pair dwarfs the other arms.
    Updated {
        item: Box<RoadmapItem>,
        event: Box<ItemEvent>,
    },
    /// The item was deleted at the PM's ask; emit the deletion.
    Discarded { item_id: String },
    /// The board outran the ask — the item went `active`+ since the PM
    /// proposed, or the dep list it asked for no longer resolves (or would now
    /// close a loop). The proposal was deleted without applying, and the message
    /// says why.
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
        let (outcome, _) = update_and_record(&conn, &b.id, &deps_patch(&[&a.code]), None).unwrap();
        assert_eq!(outcome.unwrap().item.deps, vec![a.code.clone()]);

        // a after b now closes the loop — refused, and the row is untouched.
        let err = update_and_record(&conn, &a.id, &deps_patch(&[&b.code]), None).unwrap_err();
        assert!(err.contains("loop"), "{err}");
        assert!(
            err.contains(&format!("{} → {} → {}", a.code, b.code, a.code)),
            "the refusal names the loop: {err}"
        );
        assert!(store::get(&conn, &a.id).unwrap().unwrap().deps.is_empty());

        // A code that isn't on the board at all is refused too.
        let err = update_and_record(&conn, &a.id, &deps_patch(&["MCA-999"]), None).unwrap_err();
        assert!(err.contains("MCA-999"), "{err}");

        // Self-reference, and the same rule on the create path.
        let err = update_and_record(&conn, &a.id, &deps_patch(&[&a.code]), None).unwrap_err();
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
        let (outcome, _) = update_and_record(&conn, &a.id, &patch, None).unwrap();
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
