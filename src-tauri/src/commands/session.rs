//! Session records / turns: reading the persisted chat history and backfilling
//! or appending records the on-disk transcript doesn't carry.

use std::sync::Arc;
use tauri::State;

use crate::error::Result;
use crate::supervisor::Supervisor;

#[tauri::command]
pub fn read_session_records(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<Vec<crate::workspace::SessionRecord>> {
    supervisor.workspace.read_session_records(&agent_id)
}

#[tauri::command]
pub fn read_user_turns(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<Vec<crate::workspace::UserTurn>> {
    supervisor.workspace.read_user_turns(&agent_id)
}

/// Ingest the agent's on-disk transcript into session_records now (lazy
/// backfill when a session is opened with no records yet). Idempotent.
#[tauri::command]
pub fn sync_session(supervisor: State<'_, Arc<Supervisor>>, agent_id: String) -> Result<()> {
    supervisor.sync_session(&agent_id);
    Ok(())
}

/// Persist a runtime-compiled record (`source = 'live_compiled'`) the frontend
/// holds but the on-disk transcript lacks — currently cursor's per-turn token
/// usage, which it emits only on its live `result` event. Idempotent on
/// `native_id` (use the event's `request_id`), so re-sending a turn is a no-op.
/// Returns whether a new row was inserted.
#[tauri::command]
pub fn append_live_record(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    provider: String,
    native_id: String,
    body: serde_json::Value,
) -> Result<bool> {
    let inserted = supervisor.workspace.append_session_records(
        &agent_id,
        &provider,
        "live_compiled",
        None,
        &[(native_id.as_str(), &body)],
    )?;
    Ok(inserted > 0)
}
