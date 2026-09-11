//! Tauri command surface for the roadmap. Domain work lives in sibling modules.

use tauri::AppHandle;

use super::assignments;
use super::brakes::{self, ProjectHold};
use super::client_events::{self, emit_item, emit_item_event, emit_project_hold};
use super::drainer;
use super::events::{self, EventActor, EventKind, ItemEvent};
use super::memory::{self, Brief, BriefProposal};
use super::merge_sweep;
use super::mutations::{create_checked, update_and_record};
use super::order_proposals::{self, OrderProposal};
use super::pr_review;
use super::proposals::{self, Proposal};
use super::rulings;
use super::store;
use super::types::{ItemPatch, ItemStatus, ItemUpdate, NewItem, RoadmapItem};
use super::Db;

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
        if outcome.item.status == ItemStatus::InReview {
            merge_sweep::nudge();
        }
    }
    Ok(outcome)
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
    merge_sweep::merge_hold_gate(&db, &item_id)?;
    let (repo, number) = pr_review::target(&db, &item_id).ok_or(
        "this item has no pull request to merge — it may have shipped or come back to the board",
    )?;
    crate::github::pr_merge_number(&repo, number)
        .await
        .map_err(|e| e.to_string())?;
    merge_sweep::nudge();
    Ok(())
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
