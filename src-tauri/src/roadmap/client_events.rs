//! Live board sync pulses to the webview (`roadmap:*` Tauri events).
//!
//! Distinct from [`super::events`] (durable history). Best-effort after the lock drops.

use tauri::{AppHandle, Emitter};

use super::brakes::ProjectHold;
use super::events::ItemEvent;
use super::memory::{Brief, BriefProposal};
use super::order_proposals::OrderProposal;
use super::proposals::Proposal;
use super::types::RoadmapItem;

pub(crate) fn emit_item(app: &AppHandle, item: &RoadmapItem) {
    let _ = app.emit("roadmap:item", item);
}

pub(super) fn emit_item_deleted(app: &AppHandle, id: &str) {
    let _ = app.emit("roadmap:item-deleted", id);
}

pub(crate) fn emit_item_event(app: &AppHandle, event: &ItemEvent) {
    let _ = app.emit("roadmap:item-event", event);
}

pub(crate) fn emit_proposal(app: &AppHandle, proposal: &Proposal) {
    let _ = app.emit("roadmap:proposal", proposal);
}

pub(super) fn emit_proposal_deleted(app: &AppHandle, id: &str) {
    let _ = app.emit("roadmap:proposal-deleted", id);
}

pub(crate) fn emit_order_proposal(app: &AppHandle, proposal: &OrderProposal) {
    let _ = app.emit("roadmap:order-proposal", proposal);
}

pub(super) fn emit_order_proposal_deleted(app: &AppHandle, project_id: &str) {
    let _ = app.emit("roadmap:order-proposal-deleted", project_id);
}

pub(crate) fn emit_project_hold(app: &AppHandle, hold: &ProjectHold) {
    let _ = app.emit("roadmap:project-hold", hold);
}

pub(super) fn emit_project_hold_released(app: &AppHandle, project_id: &str) {
    let _ = app.emit("roadmap:project-hold-released", project_id);
}

pub(super) fn emit_brief(app: &AppHandle, brief: &Brief) {
    let _ = app.emit("roadmap:brief", brief);
}

pub(crate) fn emit_brief_proposal(app: &AppHandle, proposal: &BriefProposal) {
    let _ = app.emit("roadmap:brief-proposal", proposal);
}

pub(super) fn emit_brief_proposal_deleted(app: &AppHandle, project_id: &str) {
    let _ = app.emit("roadmap:brief-proposal-deleted", project_id);
}
