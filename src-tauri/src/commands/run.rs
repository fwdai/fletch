//! Run panel: start/stop/state of the per-agent run process, ad-hoc
//! verification, run-config detection, and project env-variable overrides.

use std::sync::Arc;
use tauri::State;

use crate::error::Result;
use crate::host::EngineCtx;
use crate::run_session::RunStateSnapshot;
use crate::supervisor::Supervisor;

use super::files::expand_tilde;

/// Start the Run-panel process for an agent.
/// Runs setup-then-run on first start, then run only on subsequent.
#[tauri::command]
pub fn run_start(
    supervisor: State<'_, Arc<Supervisor>>,
    ctx: State<'_, Arc<EngineCtx>>,
    agent_id: String,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.run_start(ctx.inner().clone(), &agent_id)
}

/// Stop the Run-panel process for an agent. Idempotent.
#[tauri::command]
pub fn run_stop(
    supervisor: State<'_, Arc<Supervisor>>,
    ctx: State<'_, Arc<EngineCtx>>,
    agent_id: String,
) -> Result<()> {
    supervisor.run_stop(ctx.inner().clone(), &agent_id)
}

/// Snapshot of the Run-panel state and accumulated log buffer for
/// rehydrating the panel on mount.
#[tauri::command]
pub fn run_state(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<RunStateSnapshot> {
    Ok(supervisor.run_state(&agent_id))
}

/// Run the project's deterministic checks — install → test → lint — in an
/// agent's checkout and return a [`crate::verify::VerificationReport`]. The
/// body lives in the engine (`commands::run_verification_impl`), where the
/// host's autopilot calls it too.
#[tauri::command]
pub async fn run_verification(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    subdir: Option<String>,
) -> Result<crate::verify::VerificationReport> {
    fletch_core::commands::run_verification_impl(supervisor.inner(), &agent_id, subdir.as_deref())
        .await
}

/// Detect the run configuration for an agent's primary repo, ranked by
/// confidence. The panel renders the first entry and layers persisted
/// overrides on top.
#[tauri::command]
pub fn detect_run_config(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<Vec<crate::run_detect::DetectedConfig>> {
    supervisor.detect_run_config(&agent_id)
}

/// Detect the run configuration for a project by repo path (as the sidebar
/// keys its groups), bundled with the resolved project_id. Powers the
/// Project Settings surface, which can open for a repo that has no live agent.
#[tauri::command]
pub fn project_run_config(
    supervisor: State<'_, Arc<Supervisor>>,
    repo_path: String,
) -> Result<crate::supervisor::ProjectRunConfig> {
    supervisor.project_run_config(&repo_path)
}

/// Discover a project's env keys (in the *source* repo, where gitignored env
/// files live), for the Run & Environment settings list: the `KEY=value` pairs
/// in `.env`, plus the keys only *declared* by `.env.example`/`.env.sample` —
/// bare names, because example values are placeholders and must never be
/// treated as real values. Missing/unreadable files → empty. `.env` values are
/// returned so the UI can show them masked and flag overrides that differ; it
/// never writes them anywhere.
#[tauri::command]
pub fn read_env_file_keys(repo_path: String) -> Result<crate::run_env::EnvFileKeys> {
    Ok(crate::run_env::discover_env_keys(&expand_tilde(&repo_path)))
}

/// Read a project variable's override value (keychain-backed) so the settings
/// UI can pre-fill the edit field. `None` when no override is set.
#[tauri::command]
pub fn get_env_override(project_id: String, key: String) -> Option<String> {
    crate::run_env::override_get(&crate::run_env::override_secret_key(&project_id, &key))
}

/// Store a project variable's override value in the override store (OS keychain
/// on release macOS; in-memory session store on dev / non-macOS) so a
/// user-chosen value (e.g. a disposable per-agent DB URL) can diverge from
/// `.env` without ever being written to the database.
#[tauri::command]
pub fn set_env_override(project_id: String, key: String, value: String) -> Result<()> {
    crate::run_env::override_set(
        &crate::run_env::override_secret_key(&project_id, &key),
        &value,
    )
}

/// Remove a project variable's override; resolution falls back to the `.env`
/// value (mirror).
#[tauri::command]
pub fn clear_env_override(project_id: String, key: String) -> Result<()> {
    crate::run_env::override_delete(&crate::run_env::override_secret_key(&project_id, &key))
}
