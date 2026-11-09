//! The agent-authored walkthrough of a workspace's changes, written through the
//! `tour_set` mailbox op and read by the Code panel's Changes view.

use std::sync::Arc;
use tauri::State;

use crate::error::Result;
use crate::supervisor::Supervisor;

/// The workspace's tour, or null when its agent has not written one. Reloaded
/// on `tour:changed`.
#[tauri::command]
pub fn get_tour(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<Option<crate::workspace::Tour>> {
    supervisor.workspace.get_tour(&agent_id)
}
