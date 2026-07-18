//! Run panel: start/stop/state of the per-agent run process, ad-hoc
//! verification, run-config detection, and project env-variable overrides.

use std::sync::Arc;
use tauri::{AppHandle, State};

use crate::error::Result;
use crate::run_session::RunStateSnapshot;
use crate::supervisor::Supervisor;

use super::files::{agent_repo_checkout, expand_tilde};

/// Start the Run-panel process for an agent.
/// Runs setup-then-run on first start, then run only on subsequent.
#[tauri::command]
pub fn run_start(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    agent_id: String,
) -> Result<()> {
    let sup = supervisor.inner().clone();
    sup.run_start(app, &agent_id)
}

/// Stop the Run-panel process for an agent. Idempotent.
#[tauri::command]
pub fn run_stop(
    supervisor: State<'_, Arc<Supervisor>>,
    app: AppHandle,
    agent_id: String,
) -> Result<()> {
    supervisor.run_stop(app, &agent_id)
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

/// Default wall-clock budget for an ad-hoc verification run's checks, matching
/// the workflow tests gate's `DEFAULT_TESTS_TIMEOUT_SECS` (15 min). Ad-hoc
/// checkouts have no step budget to draw from.
const VERIFY_TIMEOUT_SECS: u64 = 900;

/// Run the project's deterministic checks — install → test → lint — in an
/// agent's checkout and return a [`crate::verify::VerificationReport`]. Resolves
/// the target repo via `subdir` (primary when `None`) and layers the project's
/// `run.test` / `run.install` / `run.lint` overrides over detection, the same
/// layering the workflow tests gate uses.
#[tauri::command]
pub async fn run_verification(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    subdir: Option<String>,
) -> Result<crate::verify::VerificationReport> {
    let (_repo, checkout) = agent_repo_checkout(&supervisor, &agent_id, subdir.as_deref())?;
    // Project-scoped command overrides (mirrors the tests gate's `run.test` /
    // `run.install`, plus `run.lint`). Empty project_id → detection only.
    let project_id = supervisor
        .workspace
        .agent(&agent_id)
        .map(|r| r.project_id)
        .unwrap_or_default();
    let setting = |key: &str| -> Option<String> {
        if project_id.is_empty() {
            None
        } else {
            supervisor.workspace.project_setting(&project_id, key)
        }
    };
    let verifier = crate::verify::Verifier::new(
        setting("run.test"),
        setting("run.install"),
        setting("run.lint"),
        VERIFY_TIMEOUT_SECS,
    )?;
    let report = verifier.verify(&checkout).await;
    tracing::info!(
        agent_id = %agent_id,
        passed = report.passed(),
        checks = report.checks.len(),
        "ran ad-hoc verification"
    );
    Ok(report)
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

/// Discover the `KEY=value` pairs in a project's `.env` (in the *source* repo,
/// where gitignored env files live), for the Run & Environment settings list.
/// Missing/unreadable `.env` → empty. Values are returned so the UI can show
/// them masked and flag overrides that differ; it never writes them anywhere.
#[tauri::command]
pub fn read_env_file_keys(repo_path: String) -> Result<Vec<crate::run_env::EnvEntry>> {
    Ok(crate::run_env::read_env_file(&expand_tilde(&repo_path)))
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
