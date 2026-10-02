//! Session records / turns: reading the persisted chat history and backfilling
//! or appending records the on-disk transcript doesn't carry.

use std::sync::Arc;
use tauri::State;

use crate::error::Result;
use crate::supervisor::Supervisor;

/// The agent's display history: records it inherits through session lineage
/// (tagged `inherited`), then its own.
#[tauri::command]
pub fn read_session_records(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<Vec<crate::workspace::SessionRecord>> {
    supervisor.workspace.read_history_records(&agent_id)
}

/// The records of the sessions the agent's workspace has superseded (a
/// rewind's abandoned branches), one list per session — for its spend.
#[tauri::command]
pub fn read_superseded_records(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<Vec<Vec<crate::workspace::SessionRecord>>> {
    supervisor.workspace.read_superseded_records(&agent_id)
}

/// The user turns of the same history, in the same order.
#[tauri::command]
pub fn read_user_turns(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<Vec<crate::workspace::UserTurn>> {
    supervisor.workspace.read_history_turns(&agent_id)
}

/// Ingest the agent's on-disk transcript into session_records now (lazy
/// backfill when a session is opened with no records yet). Idempotent.
#[tauri::command]
pub fn sync_session(supervisor: State<'_, Arc<Supervisor>>, agent_id: String) -> Result<()> {
    supervisor.sync_session(&agent_id);
    Ok(())
}

/// Persist a runtime-compiled record (`source = 'live_compiled'`) the frontend
/// holds but the on-disk transcript lacks — for example live-only usage and
/// readable Codex reasoning (the rollout stores only encrypted text). Idempotent
/// on `native_id`, so re-sending a turn is a no-op.
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
