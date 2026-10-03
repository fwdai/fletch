//! Agent lifecycle: spawn / fork, input (write, message, tool-use answers),
//! terminal control, run-state transitions, and per-agent repo attach.

use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;

use crate::error::Result;
use crate::host::EngineCtx;
use crate::managed_session::ToolUseBehavior;
use crate::supervisor::Supervisor;
use crate::workspace::{AgentRecord, AgentView, TrackedRepo};

// Args mirror the frontend `invoke("spawn_agent", ...)` payload one-to-one;
// they're the IPC wire surface, not collapsible into a struct here.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn spawn_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    ctx: State<'_, Arc<EngineCtx>>,
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
    // Tags a workspace owned by a surface other than the sidebar — today only
    // the Roadmap tab's project-manager chat (`workspace::PURPOSE_ROADMAP_PM`).
    // Absent for a normal spawn.
    purpose: Option<String>,
    // The first prompt, persisted as the task at creation so the sidebar row
    // never shows an empty task while the process starts. Absent when the
    // caller has no prompt yet.
    task: Option<String>,
) -> Result<AgentRecord> {
    fletch_core::commands::spawn_agent_impl(
        supervisor.inner().clone(),
        ctx.inner().clone(),
        view,
        repo_path,
        provider,
        name,
        effort,
        model,
        instructions,
        custom_agent_id,
        skills,
        mcp_servers,
        fork_base,
        issue_ref,
        purpose,
        task,
    )
    .await
}

/// Fork an existing workspace into a new one at an anchor — through the turn
/// `turn_id` names, or the end of the parent's session when `null` — seeding
/// its worktree (`code`) and conversation (`context`) independently. Every
/// fork continues the parent conversation by reference (the child shows the
/// parent's history through session lineage); `context = full` has the
/// child's agent resume it natively, `context = summary` briefs its fresh
/// agent with a summary of it.
///
/// `transcript` is the frontend-rendered text of the parent conversation up to
/// the anchor — built there so it renders uniformly across every provider's
/// chat adapter — which the spawn summarizes. `null` for a full fork.
#[tauri::command]
pub async fn fork_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    ctx: State<'_, Arc<EngineCtx>>,
    parent_id: String,
    turn_id: Option<String>,
    code: crate::supervisor::ForkCode,
    context: crate::supervisor::ForkContext,
    transcript: Option<String>,
) -> Result<AgentRecord> {
    let sup = supervisor.inner().clone();
    sup.fork_agent(
        ctx.inner().clone(),
        &parent_id,
        turn_id.as_deref(),
        code,
        context,
        transcript,
    )
    .await
}

/// Rewind an agent to just before the turn `turn_id` names, in place: its
/// conversation, its code, or both (`scope`). `transcript` is the
/// frontend-rendered conversation before that turn, which the rewound
/// session's agent is briefed with a summary of when its provider can't
/// resume the conversation natively; `null` when the conversation isn't
/// rewound. Resolves once the rewound agent is up.
#[tauri::command]
pub async fn rewind_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    ctx: State<'_, Arc<EngineCtx>>,
    agent_id: String,
    turn_id: String,
    scope: crate::supervisor::RewindScope,
    transcript: Option<String>,
) -> Result<crate::supervisor::RewindOutcome> {
    supervisor
        .inner()
        .rewind(ctx.inner(), &agent_id, &turn_id, scope, transcript)
        .await
}

/// What rewinding an agent's code to before turn `turn_id` would do — the
/// commits that would leave each branch — for the confirmation; an error says
/// why its code can't be rewound.
#[tauri::command]
pub async fn preview_rewind_code(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    turn_id: String,
) -> Result<crate::supervisor::RestoreReport> {
    supervisor
        .inner()
        .preview_rewind_code(&agent_id, &turn_id)
        .await
}

/// Undo an agent's last code restore, from the undo point its checkouts hold.
#[tauri::command]
pub async fn undo_code_restore(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<()> {
    supervisor.inner().undo_code_restore(&agent_id).await
}

/// Let an agent's last code restore's undo point go, keeping the restored code.
#[tauri::command]
pub async fn discard_code_undo(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<()> {
    supervisor.inner().discard_code_undo(&agent_id).await
}

/// Whether an agent's last code restore can still be undone.
#[tauri::command]
pub async fn has_code_undo(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<bool> {
    supervisor.inner().has_code_undo(&agent_id).await
}

#[tauri::command]
pub fn write_to_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    ctx: State<'_, Arc<EngineCtx>>,
    agent_id: String,
    data: String,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.write_to_agent(ctx.inner(), &agent_id, data.as_bytes())
}

/// Returns `true` when the follow-up was enqueued for a later turn boundary
/// rather than delivered now (see `Supervisor::send_user_message`).
#[tauri::command]
pub async fn send_user_message(
    supervisor: State<'_, Arc<Supervisor>>,
    ctx: State<'_, Arc<EngineCtx>>,
    agent_id: String,
    turn_id: String,
    text: String,
    attachments: Vec<String>,
) -> Result<bool> {
    let sup = supervisor.inner().clone();
    sup.send_user_message(ctx.inner(), &agent_id, &turn_id, &text, &attachments)
        .await
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
    ctx: State<'_, Arc<EngineCtx>>,
    agent_id: String,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.resume_agent(ctx.inner().clone(), &agent_id).await
}

#[tauri::command]
pub async fn switch_view(
    supervisor: State<'_, Arc<Supervisor>>,
    ctx: State<'_, Arc<EngineCtx>>,
    agent_id: String,
    view: AgentView,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.switch_view(ctx.inner().clone(), &agent_id, view).await
}

#[tauri::command]
pub async fn stop_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    ctx: State<'_, Arc<EngineCtx>>,
    agent_id: String,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.stop_agent(ctx.inner().clone(), &agent_id).await
}

#[tauri::command]
pub async fn set_agent_effort(
    supervisor: State<'_, Arc<Supervisor>>,
    ctx: State<'_, Arc<EngineCtx>>,
    agent_id: String,
    effort: Option<String>,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.set_agent_effort(ctx.inner(), &agent_id, effort.as_deref())
        .await
}

#[tauri::command]
pub async fn set_agent_model(
    supervisor: State<'_, Arc<Supervisor>>,
    ctx: State<'_, Arc<EngineCtx>>,
    agent_id: String,
    model: Option<String>,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.set_agent_model(ctx.inner(), &agent_id, model.as_deref())
        .await
}

#[tauri::command]
pub async fn discard_agent(supervisor: State<'_, Arc<Supervisor>>, agent_id: String) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.discard_agent(&agent_id).await
}

#[tauri::command]
pub async fn archive_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    ctx: State<'_, Arc<EngineCtx>>,
    agent_id: String,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.archive_agent(ctx.inner().clone(), &agent_id).await
}

#[tauri::command]
pub async fn restore_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    ctx: State<'_, Arc<EngineCtx>>,
    agent_id: String,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.restore_agent(ctx.inner().clone(), &agent_id).await
}

#[tauri::command]
pub async fn add_repo_to_agent(
    supervisor: State<'_, Arc<Supervisor>>,
    ctx: State<'_, Arc<EngineCtx>>,
    agent_id: String,
    repo_path: String,
) -> Result<TrackedRepo> {
    let sup = supervisor.inner().clone();
    sup.add_repo_to_agent(ctx.inner().clone(), &agent_id, PathBuf::from(repo_path))
        .await
}
