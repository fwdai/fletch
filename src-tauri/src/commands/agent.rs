//! Agent lifecycle: spawn / fork, input (write, message, tool-use answers),
//! terminal control, run-state transitions, and per-agent repo attach.

use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, State};

use crate::error::Result;
use crate::managed_session::ToolUseBehavior;
use crate::supervisor::{SpawnRequest, Supervisor};
use crate::workspace::{AgentRecord, AgentView, TrackedRepo};

// Args mirror the frontend `invoke("spawn_agent", ...)` payload one-to-one;
// they're the IPC wire surface, not collapsible into a struct here.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn spawn_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    view: Option<AgentView>,
    repo_path: String,
    provider: Option<String>,
    name: Option<String>,
    effort: Option<String>,
    model: Option<String>,
    instructions: Option<String>,
    custom_agent_id: Option<String>,
    skills: Option<Vec<crate::agent_profile::SkillSnapshot>>,
    mcp_servers: Option<Vec<crate::agent_profile::McpServerSnapshot>>,
    fork_base: Option<String>,
    issue_ref: Option<String>,
) -> Result<AgentRecord> {
    let sup = supervisor.inner().clone();
    sup.spawn_agent(
        app,
        SpawnRequest {
            view: view.unwrap_or_default(),
            repo_path: PathBuf::from(repo_path),
            provider: provider.unwrap_or_else(|| "claude".to_string()),
            name,
            effort,
            model,
            instructions,
            // Forked-conversation context is set only by the fork path.
            forked_context: None,
            custom_agent_id,
            skills: skills.unwrap_or_default(),
            mcp_servers: mcp_servers.unwrap_or_default(),
            fork_base,
            // User-initiated spawns fork from the source repo, not a run repo,
            // and are never run-owned; the scheduler sets both for a step spawn.
            run_repo: None,
            owner_run_id: None,
            // Carrying another workspace's working tree is a fork-only path.
            carry_from: None,
            // Set when the spawn originates from a Home-inbox issue.
            issue_ref,
        },
    )
    .await
}

/// Fork an existing workspace into a new one, seeding its worktree (`code`) and
/// conversation (`context`) independently. `context = up_to_message` carries the
/// parent conversation through the navigable prompt at a 0-based ordinal (the
/// same ordinal the chat's turn list uses; git-action turns excluded).
///
/// `context_digest` is the frontend-rendered prose for the carried range — built
/// there so it renders uniformly across every provider's chat adapter and always
/// matches the history the child shows. `null`/empty when nothing is carried.
///
/// `snapshot_max_seq` is the highest `session_records.seq` the frontend saw when
/// it built the digest; the copy is capped at it so a sync that appends to the
/// parent between the two reads can't seed the child with turns the brief omitted.
#[tauri::command]
pub async fn fork_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    parent_id: String,
    code: crate::supervisor::ForkCode,
    context: crate::supervisor::ForkContext,
    context_digest: Option<String>,
    snapshot_max_seq: Option<i64>,
) -> Result<AgentRecord> {
    let sup = supervisor.inner().clone();
    sup.fork_agent(
        app,
        &parent_id,
        code,
        context,
        context_digest,
        snapshot_max_seq,
    )
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

/// Returns `true` when the follow-up was enqueued for a later turn boundary
/// rather than delivered now (see `Supervisor::send_user_message`).
#[tauri::command]
pub fn send_user_message(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    agent_id: String,
    turn_id: String,
    text: String,
    attachments: Vec<String>,
    thinking: Option<String>,
) -> Result<bool> {
    let sup = supervisor.inner().clone();
    sup.send_user_message(
        &app,
        &agent_id,
        &turn_id,
        &text,
        &attachments,
        thinking.as_deref(),
    )
}

#[tauri::command]
pub fn answer_tool_use(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    request_id: String,
    updated_input: serde_json::Value,
    behavior: ToolUseBehavior,
    message: Option<String>,
) -> Result<()> {
    supervisor
        .inner()
        .answer_tool_use(&agent_id, &request_id, updated_input, behavior, message)
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
pub async fn discard_agent(supervisor: State<'_, Arc<Supervisor>>, agent_id: String) -> Result<()> {
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
