//! Live board sync pulses to the webview (`roadmap:*` Tauri events).
//!
//! Distinct from [`super::events`] (durable history). Best-effort after the lock drops.

use crate::host::{emit, EventSink};

use super::brakes::ProjectHold;
use super::events::ItemEvent;
use super::memory::{Brief, BriefProposal};
use super::order_proposals::OrderProposal;
use super::proposals::Proposal;
use super::types::RoadmapItem;

pub(crate) fn emit_item(sink: &dyn EventSink, item: &RoadmapItem) {
    emit(sink, "roadmap:item", item);
}

pub(super) fn emit_item_deleted(sink: &dyn EventSink, id: &str) {
    emit(sink, "roadmap:item-deleted", id);
}

pub(crate) fn emit_item_event(sink: &dyn EventSink, event: &ItemEvent) {
    emit(sink, "roadmap:item-event", event);
}

pub(crate) fn emit_proposal(sink: &dyn EventSink, proposal: &Proposal) {
    emit(sink, "roadmap:proposal", proposal);
}

pub(super) fn emit_proposal_deleted(sink: &dyn EventSink, id: &str) {
    emit(sink, "roadmap:proposal-deleted", id);
}

pub(crate) fn emit_order_proposal(sink: &dyn EventSink, proposal: &OrderProposal) {
    emit(sink, "roadmap:order-proposal", proposal);
}

pub(super) fn emit_order_proposal_deleted(sink: &dyn EventSink, project_id: &str) {
    emit(sink, "roadmap:order-proposal-deleted", project_id);
}

pub(crate) fn emit_project_hold(sink: &dyn EventSink, hold: &ProjectHold) {
    emit(sink, "roadmap:project-hold", hold);
}

pub(super) fn emit_project_hold_released(sink: &dyn EventSink, project_id: &str) {
    emit(sink, "roadmap:project-hold-released", project_id);
}

pub(super) fn emit_brief(sink: &dyn EventSink, brief: &Brief) {
    emit(sink, "roadmap:brief", brief);
}

pub(crate) fn emit_brief_proposal(sink: &dyn EventSink, proposal: &BriefProposal) {
    emit(sink, "roadmap:brief-proposal", proposal);
}

pub(super) fn emit_brief_proposal_deleted(sink: &dyn EventSink, project_id: &str) {
    emit(sink, "roadmap:brief-proposal-deleted", project_id);
}
