//! The local review loop, driven from the Git panel: start or stop a review
//! of a checkout, read its state, dismiss a finding. Same bodies as the remote
//! ops (`fletch_core::commands::review`).

use std::sync::Arc;
use tauri::State;

use crate::error::Result;
use crate::host::EngineCtx;
use crate::supervisor::Supervisor;
use fletch_core::review::ReviewState;

#[tauri::command]
pub async fn review_start(
    supervisor: State<'_, Arc<Supervisor>>,
    ctx: State<'_, Arc<EngineCtx>>,
    agent_id: String,
    subdir: Option<String>,
    custom_agent_id: Option<String>,
) -> Result<ReviewState> {
    fletch_core::commands::review_start_impl(
        ctx.inner(),
        supervisor.inner(),
        &agent_id,
        subdir.as_deref(),
        custom_agent_id.as_deref(),
    )
    .await
}

#[tauri::command]
pub async fn review_stop(
    supervisor: State<'_, Arc<Supervisor>>,
    ctx: State<'_, Arc<EngineCtx>>,
    agent_id: String,
    subdir: Option<String>,
) -> Result<ReviewState> {
    fletch_core::commands::review_stop_impl(
        ctx.inner(),
        supervisor.inner(),
        &agent_id,
        subdir.as_deref(),
    )
    .await
}

#[tauri::command]
pub async fn get_review(
    supervisor: State<'_, Arc<Supervisor>>,
    ctx: State<'_, Arc<EngineCtx>>,
    agent_id: String,
    subdir: Option<String>,
) -> Result<Option<ReviewState>> {
    fletch_core::commands::get_review_impl(
        ctx.inner(),
        supervisor.inner(),
        &agent_id,
        subdir.as_deref(),
    )
    .await
}

#[tauri::command]
pub async fn get_reviews(
    supervisor: State<'_, Arc<Supervisor>>,
    ctx: State<'_, Arc<EngineCtx>>,
) -> Result<Vec<ReviewState>> {
    fletch_core::commands::get_reviews_impl(ctx.inner(), supervisor.inner()).await
}

#[tauri::command]
pub async fn review_dismiss_thread(
    supervisor: State<'_, Arc<Supervisor>>,
    ctx: State<'_, Arc<EngineCtx>>,
    agent_id: String,
    subdir: Option<String>,
    thread_id: String,
    reason: Option<String>,
) -> Result<()> {
    fletch_core::commands::review_dismiss_thread_impl(
        ctx.inner(),
        supervisor.inner(),
        &agent_id,
        subdir.as_deref(),
        &thread_id,
        reason.as_deref(),
    )
    .await
}
