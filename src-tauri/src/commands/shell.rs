//! Agent shell PTY: open, close, write stdin, and resize.

use std::sync::Arc;
use tauri::State;

use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::supervisor::Supervisor;

/// Open an interactive shell PTY in the agent's primary checkout — inside its
/// box when a box holds the checkout. Idempotent: if a shell is already
/// running for this agent, does nothing. Async + `spawn_blocking` because
/// opening into a stopped box boots it first.
#[tauri::command]
pub async fn open_agent_shell(
    supervisor: State<'_, Arc<Supervisor>>,
    ctx: State<'_, Arc<EngineCtx>>,
    agent_id: String,
) -> Result<()> {
    let (sup, ctx) = (supervisor.inner().clone(), ctx.inner().clone());
    tauri::async_runtime::spawn_blocking(move || sup.open_agent_shell(ctx, &agent_id))
        .await
        .map_err(|e| Error::Other(format!("open shell: {e}")))?
}

/// Kill the shell PTY for an agent.
/// Idempotent: if no shell is running, does nothing.
#[tauri::command]
pub fn close_agent_shell(supervisor: State<'_, Arc<Supervisor>>, agent_id: String) -> Result<()> {
    supervisor.close_agent_shell(&agent_id)
}

/// Write bytes to the agent's shell PTY stdin.
#[tauri::command]
pub fn write_to_shell(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    data: String,
) -> Result<()> {
    supervisor.write_to_shell(&agent_id, data.as_bytes())
}

/// Resize the agent's shell PTY.
#[tauri::command]
pub fn resize_shell(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    cols: u16,
    rows: u16,
) -> Result<()> {
    supervisor.resize_shell(&agent_id, cols, rows)
}
