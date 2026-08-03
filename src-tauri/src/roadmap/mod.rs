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
pub mod proposals;
pub mod store;
pub mod types;

use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::Connection;
use tauri::{AppHandle, Emitter};

use events::{EventActor, EventKind, ItemEvent};
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
