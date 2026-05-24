//! Tauri IPC command handlers — the thin frontend-facing surface.

use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, State};

use crate::error::Result;
use crate::keys;
use crate::supervisor::Supervisor;
use crate::workspace::{AgentRecord, Workspace};

#[tauri::command]
pub fn get_workspace(supervisor: State<'_, Arc<Supervisor>>) -> Option<Workspace> {
    supervisor.current_workspace()
}

#[tauri::command]
pub fn set_repo(
    supervisor: State<'_, Arc<Supervisor>>,
    repo_path: String,
    base_image: String,
) -> Result<Workspace> {
    supervisor.set_repo(PathBuf::from(repo_path), base_image)
}

#[tauri::command]
pub async fn spawn_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    name: String,
    branch: String,
    task: String,
) -> Result<AgentRecord> {
    let sup = supervisor.inner().clone();
    sup.spawn_agent(app, name, branch, task).await
}

#[tauri::command]
pub fn write_to_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    data: String,
) -> Result<()> {
    supervisor.write_to_agent(&agent_id, data.as_bytes())
}

#[tauri::command]
pub fn resize_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    cols: u16,
    rows: u16,
) -> Result<()> {
    supervisor.resize_agent(&agent_id, cols, rows)
}

#[tauri::command]
pub async fn stop_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    agent_id: String,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.stop_agent(app, &agent_id).await
}

#[tauri::command]
pub async fn discard_worktree(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.discard_worktree(&agent_id).await
}

#[tauri::command]
pub fn get_public_key(supervisor: State<'_, Arc<Supervisor>>) -> Result<String> {
    keys::read_public_key(&supervisor.keys)
}

/// Returns the names of all Tart VMs visible to the bundled `tart` binary.
/// Used by the frontend to populate a "pick a base image" picker — agent
/// VMs (named `algiers-*`) are filtered out so we only show candidates.
#[tauri::command]
pub async fn list_base_images(supervisor: State<'_, Arc<Supervisor>>) -> Result<Vec<String>> {
    let vm = supervisor.vm.clone();
    let names = vm.list_names().await?;
    Ok(names
        .into_iter()
        .filter(|n| !n.starts_with("algiers-"))
        .collect())
}
