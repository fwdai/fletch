//! Tauri IPC command handlers — the thin frontend-facing surface.

use std::path::PathBuf;
use std::sync::Arc;
use serde_json::Value;
use tauri::{AppHandle, State};

use crate::error::Result;
use crate::names;
use crate::supervisor::Supervisor;
use crate::workspace::{AgentRecord, AgentView, TrackedRepo, Workspace};

#[tauri::command]
pub fn get_workspace(supervisor: State<'_, Arc<Supervisor>>) -> Option<Workspace> {
    supervisor.current_workspace()
}

/// Allocate a fresh name from the shared place pool for a draft agent.
/// Frontend passes the names already taken (real agents + other drafts) so
/// the picker avoids collisions.
#[tauri::command]
pub fn allocate_draft_name(used: Vec<String>) -> String {
    names::allocate(&used.into_iter().collect())
}

#[tauri::command]
pub fn add_workspace_repo(
    supervisor: State<'_, Arc<Supervisor>>,
    repo_path: String,
) -> Result<Workspace> {
    supervisor.add_workspace_repo(PathBuf::from(repo_path))
}

#[tauri::command]
pub fn remove_workspace_repo(
    supervisor: State<'_, Arc<Supervisor>>,
    repo_path: String,
) -> Result<Workspace> {
    supervisor.remove_workspace_repo(PathBuf::from(repo_path))
}

#[tauri::command]
pub async fn spawn_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    view: Option<AgentView>,
    repo_path: String,
) -> Result<AgentRecord> {
    let sup = supervisor.inner().clone();
    sup.spawn_agent(app, view.unwrap_or_default(), PathBuf::from(repo_path))
        .await
}

#[tauri::command]
pub fn write_to_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    agent_id: String,
    data: String,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.write_to_agent(&app, &agent_id, data.as_bytes())
}

#[tauri::command]
pub fn send_user_message(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    agent_id: String,
    text: String,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.send_user_message(&app, &agent_id, &text)
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
pub async fn resume_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    agent_id: String,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.resume_agent(app, &agent_id).await
}

#[tauri::command]
pub async fn switch_view(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    agent_id: String,
    view: AgentView,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.switch_view(app, &agent_id, view).await
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
pub async fn discard_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.discard_agent(&agent_id).await
}

#[tauri::command]
pub async fn archive_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    agent_id: String,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.archive_agent(app, &agent_id).await
}

#[tauri::command]
pub async fn restore_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    agent_id: String,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.restore_agent(app, &agent_id).await
}

#[tauri::command]
pub fn read_session_transcript(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<Vec<Value>> {
    supervisor.read_session_transcript(&agent_id)
}

#[tauri::command]
pub async fn add_repo_to_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    agent_id: String,
    repo_path: String,
) -> Result<TrackedRepo> {
    let sup = supervisor.inner().clone();
    sup.add_repo_to_agent(app, &agent_id, PathBuf::from(repo_path))
        .await
}
