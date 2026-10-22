//! The local review loop's commands, shared by the Tauri commands and the
//! remote dispatcher: start or stop a review of a checkout, read the state
//! of one or every checkout, and dismiss a local thread. The engine does the
//! work (`review::engine`); these are its five front doors.

use std::sync::Arc;

use crate::error::Result;
use crate::host::EngineCtx;
use crate::review::engine::{self, ReviewState, Trigger};
use crate::supervisor::Supervisor;

/// `review_start`: a review of `agent_id`'s checkout `subdir` (the primary
/// when absent) by `custom_agent_id`, or the configured reviewer.
pub async fn review_start_impl(
    ctx: &Arc<EngineCtx>,
    sup: &Arc<Supervisor>,
    agent_id: &str,
    subdir: Option<&str>,
    custom_agent_id: Option<&str>,
) -> Result<ReviewState> {
    engine::start(ctx, sup, agent_id, subdir, custom_agent_id, Trigger::Manual).await
}

/// `review_stop`: interrupt the reviewer turn on that checkout, if any.
pub async fn review_stop_impl(
    ctx: &Arc<EngineCtx>,
    sup: &Arc<Supervisor>,
    agent_id: &str,
    subdir: Option<&str>,
) -> Result<ReviewState> {
    engine::stop(ctx, sup, agent_id, subdir).await
}

/// `get_review`: one checkout's state, looked at now; `None` when it has
/// never been reviewed.
pub async fn get_review_impl(
    ctx: &Arc<EngineCtx>,
    sup: &Arc<Supervisor>,
    agent_id: &str,
    subdir: Option<&str>,
) -> Result<Option<ReviewState>> {
    engine::get(ctx, sup, agent_id, subdir).await
}

/// `get_reviews`: every checkout with a state, for a client connecting.
pub async fn get_reviews_impl(
    ctx: &Arc<EngineCtx>,
    sup: &Arc<Supervisor>,
) -> Result<Vec<ReviewState>> {
    Ok(engine::all(ctx, sup).await)
}

/// `review_dismiss_thread`: the user resolves a local thread, with a reason.
pub async fn review_dismiss_thread_impl(
    ctx: &Arc<EngineCtx>,
    sup: &Arc<Supervisor>,
    agent_id: &str,
    subdir: Option<&str>,
    thread_id: &str,
    reason: Option<&str>,
) -> Result<()> {
    engine::dismiss_thread(ctx, sup, agent_id, subdir, thread_id, reason).await
}
