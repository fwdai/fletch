//! The box sandbox: thin wrappers over `fletch_core::commands::box_*_impl`,
//! which the remote dispatcher calls too.
//! Reads answer the probe, the image and one agent's box; writes install or
//! cancel the image, stop a box, extend a project's egress allow-list, and
//! write the five box settings — through `set_box_setting`, never the generic
//! `set_setting`, because the engine reads these.

use std::sync::Arc;
use tauri::State;

use crate::error::Result;
use crate::host::EngineCtx;
use fletch_core::commands as engine;

/// Can this host boot a box: helper, dylibs, hypervisor, arch, image, flag.
#[tauri::command]
pub fn box_runtime_probe() -> engine::BoxRuntimeProbe {
    engine::box_runtime_probe_impl()
}

/// The pinned image's install state; `box:image` carries the changes.
#[tauri::command]
pub fn box_image_status() -> engine::BoxImageStatus {
    engine::box_image_status_impl()
}

/// Start installing the pinned image in the background, then the tools
/// disks; a no-op while one runs, so it doubles as Retry. Async: the refusal
/// for a machine that cannot boot a box runs the runtime probe's processes.
#[tauri::command]
pub async fn box_image_install() -> Result<()> {
    tauri::async_runtime::spawn_blocking(engine::box_image_install_impl)
        .await
        .map_err(|e| crate::error::Error::Other(e.to_string()))?
}

#[tauri::command]
pub fn box_image_cancel() -> Result<()> {
    engine::box_image_cancel_impl()
}

/// Build a provider's tools disk now, past the backoff after failed builds;
/// the Retry of a failed `box:tools`.
#[tauri::command]
pub fn box_tools_retry(provider: String) -> Result<()> {
    engine::box_tools_retry_impl(&provider)
}

/// The registry's phase for one agent's box.
#[tauri::command]
pub fn box_state(agent_id: String) -> fletch_core::sandbox::boxes::registry::BoxStateReport {
    engine::box_state_impl(&agent_id)
}

/// The workspace types a new agent may be given, for the composer's choice.
#[tauri::command]
pub async fn available_workspace_types(
    ctx: State<'_, Arc<EngineCtx>>,
    repo_path: String,
    provider: String,
) -> Result<Vec<fletch_core::sandbox::WorkspaceTypeOption>> {
    engine::available_workspace_types_impl(&ctx, &repo_path, &provider).await
}

/// Every VM that exists, running or not, hidden workspaces included.
#[tauri::command]
pub async fn box_list(ctx: State<'_, Arc<EngineCtx>>) -> Result<Vec<engine::BoxVm>> {
    engine::box_list_impl(&ctx)
}

/// Stop the agent's box now; answers which rung of the ladder worked.
#[tauri::command]
pub async fn box_stop(agent_id: String) -> Result<String> {
    engine::box_stop_impl(&agent_id).await
}

/// Add one host pattern to a project's egress allow-list; answers the list.
#[tauri::command]
pub fn box_egress_allow(
    ctx: State<'_, Arc<EngineCtx>>,
    project_id: String,
    pattern: String,
) -> Result<Vec<String>> {
    engine::box_egress_allow_impl(&ctx, &project_id, &pattern)
}

/// The egress allow-list, layer by layer; with a project, its own too.
#[tauri::command]
pub fn box_egress_overview(
    ctx: State<'_, Arc<EngineCtx>>,
    project_id: Option<String>,
) -> Result<engine::EgressOverview> {
    engine::box_egress_overview_impl(&ctx, project_id.as_deref())
}

/// Replace the user's global egress changes over the template.
#[tauri::command]
pub fn set_box_egress_global(
    ctx: State<'_, Arc<EngineCtx>>,
    delta: fletch_core::sandbox::boxes::allowlist::Delta,
) -> Result<fletch_core::sandbox::boxes::allowlist::Delta> {
    engine::set_box_egress_global_impl(&ctx, &delta)
}

/// The hosts boxes are held waiting to reach, oldest first.
#[tauri::command]
pub fn box_egress_pending() -> Vec<fletch_core::sandbox::boxes::egress_prompt::PendingEgressPrompt>
{
    engine::box_egress_pending_impl()
}

/// Answer one held host: allow it for the project or everywhere, block it on
/// the box, or open the box's network for the turn.
#[tauri::command]
pub fn box_egress_answer(
    ctx: State<'_, Arc<EngineCtx>>,
    id: String,
    answer: engine::EgressAnswer,
) -> Result<()> {
    engine::box_egress_answer_impl(&ctx, &id, answer)
}

/// What boxes reached or were refused lately, grouped by host; with a
/// project, its boxes only.
#[tauri::command]
pub fn box_egress_activity(
    ctx: State<'_, Arc<EngineCtx>>,
    project_id: Option<String>,
    hours: Option<u64>,
) -> Result<Vec<fletch_core::sandbox::boxes::egress_log::HostActivity>> {
    engine::box_egress_activity_impl(&ctx, project_id.as_deref(), hours)
}

/// Open an agent's box's network until its turn ends, or close it again.
#[tauri::command]
pub fn box_egress_open_turn(agent_id: String, open: Option<bool>) -> Result<()> {
    engine::box_egress_open_turn_impl(&agent_id, open.unwrap_or(true))
}

/// Write one validated box setting; `null` restores its default.
#[tauri::command]
pub fn set_box_setting(
    ctx: State<'_, Arc<EngineCtx>>,
    key: String,
    value: Option<String>,
) -> Result<()> {
    engine::set_box_setting_impl(&ctx, &key, value.as_deref())
}
