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
/// disks; a no-op while one runs, so it doubles as Retry.
#[tauri::command]
pub fn box_image_install() -> Result<()> {
    engine::box_image_install_impl()
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

/// Write one validated box setting; `null` restores its default.
#[tauri::command]
pub fn set_box_setting(
    ctx: State<'_, Arc<EngineCtx>>,
    key: String,
    value: Option<String>,
) -> Result<()> {
    engine::set_box_setting_impl(&ctx, &key, value.as_deref())
}
