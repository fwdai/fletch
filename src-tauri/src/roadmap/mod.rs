//! Roadmap Tauri commands + row-level emits. DAO is [`store`] (lock held).
//!
//! `roadmap_items` is deliberately absent from `database::validate` CRUD: codes
//! must be allocated under the connection lock, `*_json` marshalled here, and
//! every mutation must emit — a raw `db_insert` would skip all three.
//!
//! Emits are best-effort after the lock drops; a failed emit never rolls back
//! persistence. Hold *release* is user-only (invariant 2); the PM may place holds
//! but cannot lift them. Dep writes must stay acyclic ([`deps`]) or the queue wedges.

pub mod assignments;
pub mod autonomy;
pub mod brakes;
pub mod client_events;
pub mod deps;
pub mod drainer;
pub mod events;
pub mod memory;
pub mod merge_sweep;
pub mod order_proposals;
pub mod pr_review;
pub mod proposals;
pub mod review;
pub mod rulings;
pub mod store;
pub mod types;

use std::borrow::Cow;
use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::Connection;
use tauri::AppHandle;

use events::{EventActor, EventKind, ItemEvent};
use brakes::ProjectHold;
use memory::{Brief, BriefProposal};
use order_proposals::OrderProposal;
use proposals::{Proposal, ProposalKind, ProposalPatch};
use types::{ItemPatch, ItemStatus, ItemUpdate, NewItem, RoadmapItem};

/// Shared with `rpc::roadmap`; same mutex handle.
pub type Db = Arc<Mutex<Connection>>;

pub(crate) use client_events::{
    emit_brief_proposal, emit_item, emit_item_event, emit_order_proposal, emit_project_hold,
    emit_proposal,
};

pub(crate) use autonomy::{accept_landing, Landing};
pub(crate) use brakes::{hold_with_event as hold_item, release_with_event as release_item};
pub(crate) use rulings::proposal_gate;


#[tauri::command]
pub async fn roadmap_list_items(
    project_id: String,
    db: tauri::State<'_, Db>,
) -> Result<Vec<RoadmapItem>, String> {
    let conn = db.lock();
    store::list(&conn, &project_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn roadmap_get_item(
    item_id: String,
    db: tauri::State<'_, Db>,
) -> Result<Option<RoadmapItem>, String> {
    let conn = db.lock();
    store::get(&conn, &item_id).map_err(|e| e.to_string())
}

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
    drainer::nudge();
    Ok(created)
}

/// Dep-check then insert + open event under caller's lock.
fn create_checked(
    conn: &Connection,
    project_id: &str,
    item: &NewItem,
) -> Result<(RoadmapItem, ItemEvent), String> {
    if !item.deps.is_empty() {
        let board = store::list(conn, project_id).map_err(|e| e.to_string())?;
        deps::validate_new(
            &deps::graph_of(&board),
            &deps::rejected_of(&board),
            &item.deps,
        )?;
    }
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let created = store::create(&tx, project_id, item).map_err(|e| e.to_string())?;
    let event = events::record(
        &tx,
        &created.id,
        project_id,
        EventActor::User,
        EventKind::Created,
        None,
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok((created, event))
}

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
    let outcome = outcome.ok_or_else(|| store::missing(&id))?;
    if outcome.applied {
        emit_item(&app, &outcome.item);
        if let Some(event) = &event {
            emit_item_event(&app, event);
        }
        drainer::nudge();
        if outcome.item.status == types::ItemStatus::InReview {
            merge_sweep::nudge();
        }
    }
    Ok(outcome)
}

/// Apply patch + history under one lock; refuse writing `rejected` via generic edit.
fn update_and_record(
    conn: &Connection,
    id: &str,
    patch: &ItemPatch,
    expect_status: Option<ItemStatus>,
    queue: bool,
) -> Result<(Option<ItemUpdate>, Option<ItemEvent>), String> {
    if patch.status == Some(ItemStatus::Rejected) {
        return Err(
            "an item is ruled off the board with roadmap_reject_item, which requires a \
             reason — not a status edit"
                .into(),
        );
    }
            if let Some(new_deps) = &patch.deps {
        if let Some(current) = store::get(conn, id).map_err(|e| e.to_string())? {
            deps::check_edit(conn, &current, new_deps)?;
        }
    }
    let landing = autonomy::is_accept(expect_status, patch)
        .then(|| autonomy::landing_for(conn, id, queue))
        .flatten();
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
            let kind = events::transition_kind(expect_status, patch.status);
            let event = events::record(
                conn,
                &item.id,
                &item.project_id,
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




#[tauri::command]
// Rank-only write: deliberately no history event (migration 0032).
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
    let item = item.ok_or_else(|| store::missing(&item_id))?;
    emit_item(&app, &item);
    drainer::nudge();
    Ok(item)
}

#[tauri::command]
pub async fn roadmap_hand_off_item(
    item_id: String,
    agent_id: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<RoadmapItem, String> {
    let (item, event) = {
        let conn = db.lock();
        assignments::hand_off(&conn, &item_id, &agent_id)?
    };
    emit_item(&app, &item);
    emit_item_event(&app, &event);
    Ok(item)
}


#[tauri::command]
// Drop DB lock before network; never hold the app connection across awaits.
pub async fn roadmap_item_review(
    item_id: String,
    db: tauri::State<'_, Db>,
) -> Result<Option<pr_review::ItemReview>, String> {
    let Some((repo, number)) = pr_review::target(&db, &item_id) else {
        return Ok(None);
    };
    Ok(Some(pr_review::fetch(&repo, number).await))
}

#[tauri::command]
pub async fn roadmap_merge_item_pr(
    item_id: String,
    db: tauri::State<'_, Db>,
) -> Result<(), String> {
    merge_hold_gate(&db, &item_id)?;
    let (repo, number) = pr_review::target(&db, &item_id).ok_or(
        "this item has no pull request to merge — it may have shipped or come back to the board",
    )?;
    crate::github::pr_merge_number(&repo, number)
        .await
        .map_err(|e| e.to_string())?;
    merge_sweep::nudge();
    Ok(())
}

/// Fail closed: user merge must not bypass an item/project hold.
fn merge_hold_gate(db: &Db, item_id: &str) -> Result<(), String> {
    let conn = db.lock();
    let Some(item) = store::get(&conn, item_id).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    match brakes::gate(&conn, &item) {
        Some(reason) => Err(format!(
            "{} is held — {reason}. Release the hold before merging its pull request.",
            item.code
        )),
        None => Ok(()),
    }
}

#[tauri::command]
// Not hand_off: that gate refuses past `open`.
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
            .ok_or_else(|| store::missing(&item_id))?;
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

#[tauri::command]
pub async fn roadmap_hold_item(
    item_id: String,
    reason: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<RoadmapItem, String> {
    let reason = brakes::clean_reason(&reason)?;
    let (item, event) = {
        let conn = db.lock();
        brakes::hold_with_event(&conn, &item_id, &reason, EventActor::User)?
    };
    emit_item(&app, &item);
    emit_item_event(&app, &event);
    Ok(item)
}

#[tauri::command]
pub async fn roadmap_release_item(
    item_id: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<RoadmapItem, String> {
    let (item, event) = {
        let conn = db.lock();
        brakes::release_with_event(&conn, &item_id)?
    };
    emit_item(&app, &item);
    if let Some(event) = &event {
        emit_item_event(&app, event);
        drainer::nudge();
    }
    Ok(item)
}

#[tauri::command]
pub async fn roadmap_get_project_hold(
    project_id: String,
    db: tauri::State<'_, Db>,
) -> Result<Option<ProjectHold>, String> {
    let conn = db.lock();
    brakes::get_project(&conn, &project_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn roadmap_hold_project(
    project_id: String,
    reason: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<ProjectHold, String> {
    let reason = brakes::clean_reason(&reason)?;
    let hold = {
        let conn = db.lock();
        brakes::hold_project(&conn, &project_id, &reason, EventActor::User)
            .map_err(|e| e.to_string())?
    };
    emit_project_hold(&app, &hold);
    Ok(hold)
}

#[tauri::command]
// Release is user-only; no agent RPC path.
pub async fn roadmap_release_project(
    project_id: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<(), String> {
    {
        let conn = db.lock();
        brakes::release_project(&conn, &project_id).map_err(|e| e.to_string())?;
    }
    client_events::emit_project_hold_released(&app, &project_id);
    drainer::nudge();
    Ok(())
}

#[tauri::command]
pub async fn roadmap_reclaim_item(
    item_id: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<RoadmapItem, String> {
    let (item, event) = {
        let conn = db.lock();
        assignments::reclaim(&conn, &item_id)?
    };
    emit_item(&app, &item);
    emit_item_event(&app, &event);
    drainer::nudge();
    Ok(item)
}


#[tauri::command]
pub async fn roadmap_reject_item(
    item_id: String,
    reason: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<RoadmapItem, String> {
    let (item, event, pending) = {
        let conn = db.lock();
        rulings::reject_item(&conn, &item_id, &reason)?
    };
    emit_item(&app, &item);
    emit_item_event(&app, &event);
    if let Some(p) = &pending {
        client_events::emit_proposal_deleted(&app, &p.id);
    }
    drainer::nudge();
    Ok(item)
}


#[tauri::command]
pub async fn roadmap_reopen_item(
    item_id: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<RoadmapItem, String> {
    let (item, event, corrections) = {
        let conn = db.lock();
        rulings::reopen_item(&conn, &item_id)?
    };
    emit_item(&app, &item);
    if let Some(event) = &event {
        emit_item_event(&app, event);
        drainer::nudge();
    }
    for note in &corrections {
        emit_item_event(&app, note);
    }
    Ok(item)
}



#[tauri::command]
// Cascade-delete pending proposal; stale dep codes count as satisfied.
pub async fn roadmap_delete_item(
    id: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<(), String> {
    let (removed, pending) = {
        let conn = db.lock();
        let pending = proposals::for_item(&conn, &id).map_err(|e| e.to_string())?;
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
        client_events::emit_item_deleted(&app, &id);
        if let Some(p) = pending {
            client_events::emit_proposal_deleted(&app, &p.id);
        }
        drainer::nudge();
    }
    Ok(())
}

#[tauri::command]
pub async fn roadmap_list_item_events(
    item_id: String,
    db: tauri::State<'_, Db>,
) -> Result<Vec<ItemEvent>, String> {
    let conn = db.lock();
    events::list_for_item(&conn, &item_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn roadmap_latest_events(
    project_id: String,
    db: tauri::State<'_, Db>,
) -> Result<Vec<ItemEvent>, String> {
    let conn = db.lock();
    events::latest_per_item(&conn, &project_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn roadmap_list_proposals(
    project_id: String,
    db: tauri::State<'_, Db>,
) -> Result<Vec<Proposal>, String> {
    let conn = db.lock();
    proposals::list_for_project(&conn, &project_id).map_err(|e| e.to_string())
}


#[tauri::command]
pub async fn roadmap_accept_proposal(
    proposal_id: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<(), String> {
    let ruling = {
        let conn = db.lock();
        rulings::accept_proposal(&conn, &proposal_id)?
    };
    client_events::emit_proposal_deleted(&app, &proposal_id);
    match ruling {
        rulings::Ruling::Updated { item, event } => {
            emit_item(&app, &item);
            emit_item_event(&app, &event);
            drainer::nudge();
            Ok(())
        }
        rulings::Ruling::Stale { message } => Err(message),
    }
}

#[tauri::command]
pub async fn roadmap_reject_proposal(
    proposal_id: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<(), String> {
    let event = {
        let conn = db.lock();
        rulings::reject_proposal(&conn, &proposal_id)?
    };
    client_events::emit_proposal_deleted(&app, &proposal_id);
    emit_item_event(&app, &event);
    Ok(())
}


#[tauri::command]
pub async fn roadmap_get_order_proposal(
    project_id: String,
    db: tauri::State<'_, Db>,
) -> Result<Option<OrderProposal>, String> {
    let conn = db.lock();
    order_proposals::get(&conn, &project_id).map_err(|e| e.to_string())
}


#[tauri::command]
pub async fn roadmap_accept_order_proposal(
    project_id: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<(), String> {
    let ruling = {
        let conn = db.lock();
        rulings::accept_order(&conn, &project_id)?
    };
    client_events::emit_order_proposal_deleted(&app, &project_id);
    match ruling {
        rulings::OrderRuling::Applied(rows) => {
            for row in &rows {
                emit_item(&app, row);
            }
            drainer::nudge();
            Ok(())
        }
        rulings::OrderRuling::Stale(message) => Err(message),
    }
}

#[tauri::command]
pub async fn roadmap_reject_order_proposal(
    project_id: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<(), String> {
    {
        let conn = db.lock();
        order_proposals::delete(&conn, &project_id).map_err(|e| e.to_string())?;
    }
    client_events::emit_order_proposal_deleted(&app, &project_id);
    Ok(())
}

#[tauri::command]
pub async fn roadmap_get_brief(
    project_id: String,
    db: tauri::State<'_, Db>,
) -> Result<Option<Brief>, String> {
    let conn = db.lock();
    memory::load(&conn, &project_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn roadmap_get_brief_proposal(
    project_id: String,
    db: tauri::State<'_, Db>,
) -> Result<Option<BriefProposal>, String> {
    let conn = db.lock();
    memory::get_proposal(&conn, &project_id).map_err(|e| e.to_string())
}

#[tauri::command]
// Brief + proposal consume under one lock (invariant 3).
pub async fn roadmap_accept_brief_proposal(
    project_id: String,
    app: AppHandle,
    db: tauri::State<'_, Db>,
) -> Result<Brief, String> {
    let applied = {
        let conn = db.lock();
        memory::accept(&conn, &project_id).map_err(|e| e.to_string())?
    };
    client_events::emit_brief_proposal_deleted(&app, &project_id);
    let brief = applied.ok_or("this brief update has already been ruled on")?;
    client_events::emit_brief(&app, &brief);
    Ok(brief)
}

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
    client_events::emit_brief_proposal_deleted(&app, &project_id);
    Ok(())
}

#[cfg(test)]
#[path = "tests/commands.rs"]
mod tests;
