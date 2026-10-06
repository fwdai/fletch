//! Tauri wrappers for the project context layer's human-facing surface: thin
//! over `fletch_core::commands::context_*_impl`, which the remote dispatcher
//! calls too, so the Context tab edits the host it is driving
//! (docs/remote-protocol.md, "Project context").

use std::sync::Arc;

use tauri::State;

use crate::error::Result;
use crate::host::EngineCtx;
use fletch_core::commands::{self as engine, ContextOverview, ProposalVerdict};
use fletch_core::context::{
    AssertionInput, CompileQuery, DismissReason, EntityInput, Id, LinkChange,
};

/// Everything the Context tab shows: the toggles, the graph, the pending
/// proposals and the stats.
#[tauri::command]
pub fn context_overview(
    ctx: State<'_, Arc<EngineCtx>>,
    project_id: String,
) -> Result<ContextOverview> {
    engine::context_overview_impl(&ctx, &project_id)
}

/// What an agent would be served for `query`, rendered as markdown.
#[tauri::command]
pub fn context_preview(
    ctx: State<'_, Arc<EngineCtx>>,
    project_id: String,
    query: CompileQuery,
) -> Result<String> {
    engine::context_preview_impl(&ctx, &project_id, query)
}

/// Create an entity, or revise one when `input.id` is set.
#[tauri::command]
pub fn context_record_entity(
    ctx: State<'_, Arc<EngineCtx>>,
    project_id: String,
    input: EntityInput,
) -> Result<Id> {
    engine::context_record_entity_impl(&ctx, &project_id, input)
}

/// Record a confirmed assertion; `input.supersedes` changes a decision.
#[tauri::command]
pub fn context_record_assertion(
    ctx: State<'_, Arc<EngineCtx>>,
    project_id: String,
    input: AssertionInput,
) -> Result<Id> {
    engine::context_record_assertion_impl(&ctx, &project_id, input)
}

#[tauri::command]
pub fn context_retract(
    ctx: State<'_, Arc<EngineCtx>>,
    project_id: String,
    assertion_id: String,
    reason: String,
) -> Result<()> {
    engine::context_retract_impl(&ctx, &project_id, &assertion_id, &reason)
}

#[tauri::command]
pub fn context_archive_entity(
    ctx: State<'_, Arc<EngineCtx>>,
    project_id: String,
    entity_id: String,
) -> Result<()> {
    engine::context_archive_entity_impl(&ctx, &project_id, &entity_id)
}

#[tauri::command]
pub fn context_merge_entities(
    ctx: State<'_, Arc<EngineCtx>>,
    project_id: String,
    from: String,
    into: String,
) -> Result<()> {
    engine::context_merge_entities_impl(&ctx, &project_id, &from, &into)
}

#[tauri::command]
pub fn context_link(
    ctx: State<'_, Arc<EngineCtx>>,
    project_id: String,
    change: LinkChange,
) -> Result<()> {
    engine::context_link_impl(&ctx, &project_id, change)
}

/// Accept (answers the recorded id) or dismiss (needs a reason) a proposal.
#[tauri::command]
pub fn context_rule_proposal(
    ctx: State<'_, Arc<EngineCtx>>,
    project_id: String,
    proposal_id: String,
    verdict: ProposalVerdict,
    dismiss_reason: Option<DismissReason>,
) -> Result<Option<Id>> {
    engine::context_rule_proposal_impl(&ctx, &project_id, &proposal_id, verdict, dismiss_reason)
}
