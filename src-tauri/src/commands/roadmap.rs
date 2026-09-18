//! Tauri wrappers for the roadmap's command surface.
//!
//! Each body is one call into `fletch_core::roadmap::commands`, where the
//! matching `_impl` lives — so a headless host reaches the same code the
//! desktop window does. `generate_handler!` registers the names below
//! unchanged.

use std::sync::Arc;

use tauri::State;

use fletch_core::host::EngineCtx;
use fletch_core::roadmap::brakes::ProjectHold;
use fletch_core::roadmap::commands;
use fletch_core::roadmap::events::ItemEvent;
use fletch_core::roadmap::memory::{Brief, BriefProposal};
use fletch_core::roadmap::order_proposals::OrderProposal;
use fletch_core::roadmap::pr_review;
use fletch_core::roadmap::proposals::Proposal;
use fletch_core::roadmap::types::{ItemPatch, ItemStatus, ItemUpdate, NewItem, RoadmapItem};
use fletch_core::roadmap::Db;

#[tauri::command]
pub async fn roadmap_list_items(
    project_id: String,
    db: State<'_, Db>,
) -> Result<Vec<RoadmapItem>, String> {
    commands::roadmap_list_items_impl(project_id, db.inner()).await
}

#[tauri::command]
pub async fn roadmap_get_item(
    item_id: String,
    db: State<'_, Db>,
) -> Result<Option<RoadmapItem>, String> {
    commands::roadmap_get_item_impl(item_id, db.inner()).await
}

#[tauri::command]
pub async fn roadmap_create_item(
    project_id: String,
    item: NewItem,
    ctx: State<'_, Arc<EngineCtx>>,
    db: State<'_, Db>,
) -> Result<RoadmapItem, String> {
    commands::roadmap_create_item_impl(project_id, item, ctx.inner(), db.inner()).await
}

#[tauri::command]
pub async fn roadmap_update_item(
    id: String,
    patch: ItemPatch,
    expect_status: Option<ItemStatus>,
    queue: Option<bool>,
    ctx: State<'_, Arc<EngineCtx>>,
    db: State<'_, Db>,
) -> Result<ItemUpdate, String> {
    commands::roadmap_update_item_impl(id, patch, expect_status, queue, ctx.inner(), db.inner())
        .await
}

#[tauri::command]
pub async fn roadmap_set_rank(
    item_id: String,
    rank: f64,
    ctx: State<'_, Arc<EngineCtx>>,
    db: State<'_, Db>,
) -> Result<RoadmapItem, String> {
    commands::roadmap_set_rank_impl(item_id, rank, ctx.inner(), db.inner()).await
}

#[tauri::command]
pub async fn roadmap_hand_off_item(
    item_id: String,
    agent_id: String,
    ctx: State<'_, Arc<EngineCtx>>,
    db: State<'_, Db>,
) -> Result<RoadmapItem, String> {
    commands::roadmap_hand_off_item_impl(item_id, agent_id, ctx.inner(), db.inner()).await
}

#[tauri::command]
pub async fn roadmap_item_review(
    item_id: String,
    db: State<'_, Db>,
) -> Result<Option<pr_review::ItemReview>, String> {
    commands::roadmap_item_review_impl(item_id, db.inner()).await
}

#[tauri::command]
pub async fn roadmap_merge_item_pr(item_id: String, db: State<'_, Db>) -> Result<(), String> {
    commands::roadmap_merge_item_pr_impl(item_id, db.inner()).await
}

#[tauri::command]
pub async fn roadmap_note_review_feedback(
    item_id: String,
    threads: usize,
    ctx: State<'_, Arc<EngineCtx>>,
    db: State<'_, Db>,
) -> Result<ItemEvent, String> {
    commands::roadmap_note_review_feedback_impl(item_id, threads, ctx.inner(), db.inner()).await
}

#[tauri::command]
pub async fn roadmap_hold_item(
    item_id: String,
    reason: String,
    ctx: State<'_, Arc<EngineCtx>>,
    db: State<'_, Db>,
) -> Result<RoadmapItem, String> {
    commands::roadmap_hold_item_impl(item_id, reason, ctx.inner(), db.inner()).await
}

#[tauri::command]
pub async fn roadmap_release_item(
    item_id: String,
    ctx: State<'_, Arc<EngineCtx>>,
    db: State<'_, Db>,
) -> Result<RoadmapItem, String> {
    commands::roadmap_release_item_impl(item_id, ctx.inner(), db.inner()).await
}

#[tauri::command]
pub async fn roadmap_get_project_hold(
    project_id: String,
    db: State<'_, Db>,
) -> Result<Option<ProjectHold>, String> {
    commands::roadmap_get_project_hold_impl(project_id, db.inner()).await
}

#[tauri::command]
pub async fn roadmap_hold_project(
    project_id: String,
    reason: String,
    ctx: State<'_, Arc<EngineCtx>>,
    db: State<'_, Db>,
) -> Result<ProjectHold, String> {
    commands::roadmap_hold_project_impl(project_id, reason, ctx.inner(), db.inner()).await
}

#[tauri::command]
pub async fn roadmap_release_project(
    project_id: String,
    ctx: State<'_, Arc<EngineCtx>>,
    db: State<'_, Db>,
) -> Result<(), String> {
    commands::roadmap_release_project_impl(project_id, ctx.inner(), db.inner()).await
}

#[tauri::command]
pub async fn roadmap_reclaim_item(
    item_id: String,
    ctx: State<'_, Arc<EngineCtx>>,
    db: State<'_, Db>,
) -> Result<RoadmapItem, String> {
    commands::roadmap_reclaim_item_impl(item_id, ctx.inner(), db.inner()).await
}

#[tauri::command]
pub async fn roadmap_reject_item(
    item_id: String,
    reason: String,
    ctx: State<'_, Arc<EngineCtx>>,
    db: State<'_, Db>,
) -> Result<RoadmapItem, String> {
    commands::roadmap_reject_item_impl(item_id, reason, ctx.inner(), db.inner()).await
}

#[tauri::command]
pub async fn roadmap_reopen_item(
    item_id: String,
    ctx: State<'_, Arc<EngineCtx>>,
    db: State<'_, Db>,
) -> Result<RoadmapItem, String> {
    commands::roadmap_reopen_item_impl(item_id, ctx.inner(), db.inner()).await
}

#[tauri::command]
pub async fn roadmap_delete_item(
    id: String,
    ctx: State<'_, Arc<EngineCtx>>,
    db: State<'_, Db>,
) -> Result<(), String> {
    commands::roadmap_delete_item_impl(id, ctx.inner(), db.inner()).await
}

#[tauri::command]
pub async fn roadmap_list_item_events(
    item_id: String,
    db: State<'_, Db>,
) -> Result<Vec<ItemEvent>, String> {
    commands::roadmap_list_item_events_impl(item_id, db.inner()).await
}

#[tauri::command]
pub async fn roadmap_latest_events(
    project_id: String,
    db: State<'_, Db>,
) -> Result<Vec<ItemEvent>, String> {
    commands::roadmap_latest_events_impl(project_id, db.inner()).await
}

#[tauri::command]
pub async fn roadmap_list_proposals(
    project_id: String,
    db: State<'_, Db>,
) -> Result<Vec<Proposal>, String> {
    commands::roadmap_list_proposals_impl(project_id, db.inner()).await
}

#[tauri::command]
pub async fn roadmap_accept_proposal(
    proposal_id: String,
    ctx: State<'_, Arc<EngineCtx>>,
    db: State<'_, Db>,
) -> Result<(), String> {
    commands::roadmap_accept_proposal_impl(proposal_id, ctx.inner(), db.inner()).await
}

#[tauri::command]
pub async fn roadmap_reject_proposal(
    proposal_id: String,
    ctx: State<'_, Arc<EngineCtx>>,
    db: State<'_, Db>,
) -> Result<(), String> {
    commands::roadmap_reject_proposal_impl(proposal_id, ctx.inner(), db.inner()).await
}

#[tauri::command]
pub async fn roadmap_get_order_proposal(
    project_id: String,
    db: State<'_, Db>,
) -> Result<Option<OrderProposal>, String> {
    commands::roadmap_get_order_proposal_impl(project_id, db.inner()).await
}

#[tauri::command]
pub async fn roadmap_accept_order_proposal(
    project_id: String,
    ctx: State<'_, Arc<EngineCtx>>,
    db: State<'_, Db>,
) -> Result<(), String> {
    commands::roadmap_accept_order_proposal_impl(project_id, ctx.inner(), db.inner()).await
}

#[tauri::command]
pub async fn roadmap_reject_order_proposal(
    project_id: String,
    ctx: State<'_, Arc<EngineCtx>>,
    db: State<'_, Db>,
) -> Result<(), String> {
    commands::roadmap_reject_order_proposal_impl(project_id, ctx.inner(), db.inner()).await
}

#[tauri::command]
pub async fn roadmap_get_brief(
    project_id: String,
    db: State<'_, Db>,
) -> Result<Option<Brief>, String> {
    commands::roadmap_get_brief_impl(project_id, db.inner()).await
}

#[tauri::command]
pub async fn roadmap_get_brief_proposal(
    project_id: String,
    db: State<'_, Db>,
) -> Result<Option<BriefProposal>, String> {
    commands::roadmap_get_brief_proposal_impl(project_id, db.inner()).await
}

#[tauri::command]
pub async fn roadmap_accept_brief_proposal(
    project_id: String,
    ctx: State<'_, Arc<EngineCtx>>,
    db: State<'_, Db>,
) -> Result<Brief, String> {
    commands::roadmap_accept_brief_proposal_impl(project_id, ctx.inner(), db.inner()).await
}

#[tauri::command]
pub async fn roadmap_reject_brief_proposal(
    project_id: String,
    ctx: State<'_, Arc<EngineCtx>>,
    db: State<'_, Db>,
) -> Result<(), String> {
    commands::roadmap_reject_brief_proposal_impl(project_id, ctx.inner(), db.inner()).await
}
