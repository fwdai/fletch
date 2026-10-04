//! The host-owned settings: thin wrappers over `fletch_core::commands`'
//! settings `_impl`s, which the remote dispatcher calls too — so the Settings
//! pane and the project page take one code path whether the window is driving
//! this Mac or a paired host. Each setter persists the key, updates the
//! engine's in-memory mirror, and emits `settings:changed` /
//! `project_settings:changed` (docs/remote-protocol.md, "Settings").

use std::collections::BTreeMap;
use std::sync::Arc;
use tauri::State;

use crate::error::Result;
use crate::host::EngineCtx;
use crate::supervisor::Supervisor;
use fletch_core::commands as engine;

/// The host-owned global settings, and nothing else — no secret, no
/// client-side preference.
#[tauri::command]
pub fn get_settings(ctx: State<'_, Arc<EngineCtx>>) -> Result<BTreeMap<String, String>> {
    engine::get_settings_impl(&ctx)
}

/// Whether finishing a turn alerts at all (chime, banner, phone push).
#[tauri::command]
pub fn set_notify_turn_complete(ctx: State<'_, Arc<EngineCtx>>, enabled: bool) -> Result<()> {
    engine::set_notify_turn_complete_impl(&ctx, enabled)
}

/// Whether the ship loop alerts the phone at all.
#[tauri::command]
pub fn set_notify_pr_activity(ctx: State<'_, Arc<EngineCtx>>, enabled: bool) -> Result<()> {
    engine::set_notify_pr_activity_impl(&ctx, enabled)
}

/// Days a workspace may sit idle before the sweep archives it; `0` is off.
#[tauri::command]
pub fn set_auto_archive_idle_days(ctx: State<'_, Arc<EngineCtx>>, days: u32) -> Result<()> {
    engine::set_auto_archive_idle_days_impl(&ctx, days)
}

/// Flip code-indexing consent; turning it on warms the index in the
/// background.
#[tauri::command]
pub fn set_code_indexing_enabled(
    ctx: State<'_, Arc<EngineCtx>>,
    supervisor: State<'_, Arc<Supervisor>>,
    enabled: bool,
) -> Result<()> {
    engine::set_code_indexing_enabled_impl(&ctx, &supervisor, enabled)
}

/// Change the sandbox engine stamped onto new agents, after a live probe.
#[tauri::command]
pub async fn set_sandbox_engine(ctx: State<'_, Arc<EngineCtx>>, engine: String) -> Result<()> {
    engine::set_sandbox_engine_impl(&ctx, &engine).await
}

/// The docker launch knobs, written together; blank clears one.
#[tauri::command]
pub fn set_docker_launch_settings(
    ctx: State<'_, Arc<EngineCtx>>,
    image: Option<String>,
    memory: Option<String>,
    cpus: Option<String>,
) -> Result<()> {
    engine::set_docker_launch_settings_impl(&ctx, engine::LaunchKnobs::new(image, memory, cpus))
}

/// The podman twin of [`set_docker_launch_settings`].
#[tauri::command]
pub fn set_podman_launch_settings(
    ctx: State<'_, Arc<EngineCtx>>,
    image: Option<String>,
    memory: Option<String>,
    cpus: Option<String>,
) -> Result<()> {
    engine::set_podman_launch_settings_impl(&ctx, engine::LaunchKnobs::new(image, memory, cpus))
}

/// Set or clear a per-agent custom binary path, then respawn that provider's
/// live agents so they exec it.
#[tauri::command]
pub async fn set_agent_bin_override(
    ctx: State<'_, Arc<EngineCtx>>,
    supervisor: State<'_, Arc<Supervisor>>,
    id: String,
    path: Option<String>,
) -> Result<()> {
    engine::set_agent_bin_override_impl(&ctx, &supervisor, &id, path.as_deref()).await
}

/// The prefix prepended to every agent branch; answers the stored form.
#[tauri::command]
pub fn set_branch_prefix(ctx: State<'_, Arc<EngineCtx>>, prefix: String) -> Result<String> {
    engine::set_branch_prefix_impl(&ctx, &prefix)
}

/// Whether pull requests Fletch opens start as drafts.
#[tauri::command]
pub fn set_draft_prs(ctx: State<'_, Arc<EngineCtx>>, enabled: bool) -> Result<()> {
    engine::set_draft_prs_impl(&ctx, enabled)
}

/// Turn the publish-approval prompt on or off.
#[tauri::command]
pub fn set_publish_confirmation(ctx: State<'_, Arc<EngineCtx>>, enabled: bool) -> Result<()> {
    engine::set_publish_confirmation_impl(&ctx, enabled)
}

/// How long a publish-approval prompt waits; `0` waits until answered.
#[tauri::command]
pub fn set_publish_approval_wait(ctx: State<'_, Arc<EngineCtx>>, secs: u64) -> Result<()> {
    engine::set_publish_approval_wait_impl(&ctx, secs)
}

/// "Remove agent attribution", from each agent's next spawn or resume.
#[tauri::command]
pub fn set_agent_attribution_removed(ctx: State<'_, Arc<EngineCtx>>, removed: bool) -> Result<()> {
    engine::set_agent_attribution_removed_impl(&ctx, removed)
}

/// A project's client-writable settings (the allowlist in
/// `fletch_core::commands::project_settings`).
#[tauri::command]
pub fn get_project_settings(
    ctx: State<'_, Arc<EngineCtx>>,
    project_id: String,
) -> Result<BTreeMap<String, String>> {
    engine::get_project_settings_impl(&ctx, &project_id)
}

/// Write one allowlisted project key; `null` deletes the row.
#[tauri::command]
pub fn set_project_setting(
    ctx: State<'_, Arc<EngineCtx>>,
    project_id: String,
    key: String,
    value: Option<String>,
) -> Result<()> {
    engine::set_project_setting_impl(&ctx, &project_id, &key, value.as_deref())
}
