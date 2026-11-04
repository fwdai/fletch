//! Tauri wrappers for the workflow engine's command surface.
//!
//! Each body is one call into `fletch_core::workflow`, where the matching
//! `_impl` lives — so a headless host reaches the same code the desktop window
//! does. `generate_handler!` registers the names below unchanged; the prose
//! describing what each one does sits on the `_impl`.

use std::sync::Arc;

use tauri::State;

use fletch_core::supervisor::Supervisor;
use fletch_core::workflow::definition::{self, Definition};
use fletch_core::workflow::service::{self, WorkflowService};
use fletch_core::workflow::spec::Spec;
use fletch_core::workflow::types::{Event, Run, RunDetail};
use fletch_core::workflow::yaml::ImportReport;
use fletch_core::workflow::{self, Db};

// ───────────────────────────── reads ────────────────────────────────────────

/// Every run, newest-updated first; optionally scoped to one project.
#[tauri::command]
pub async fn wf_list_runs(
    project_id: Option<String>,
    db: State<'_, Db>,
) -> Result<Vec<Run>, String> {
    workflow::wf_list_runs_impl(project_id, db.inner()).await
}

/// A run plus its attempts. `None` if the run doesn't exist.
#[tauri::command]
pub async fn wf_get_run(run_id: String, db: State<'_, Db>) -> Result<Option<RunDetail>, String> {
    workflow::wf_get_run_impl(run_id, db.inner()).await
}

/// A page of a run's journal: events strictly after `after_seq`, oldest first.
#[tauri::command]
pub async fn wf_events(
    run_id: String,
    after_seq: i64,
    limit: i64,
    db: State<'_, Db>,
) -> Result<Vec<Event>, String> {
    workflow::wf_events_impl(run_id, after_seq, limit, db.inner()).await
}

// ───────────────────────────── definitions ──────────────────────────────────

/// Save (insert or update) a workflow definition after full validation.
#[tauri::command]
pub async fn wf_def_save(
    spec: Spec,
    id: Option<String>,
    hue: Option<i64>,
    db: State<'_, Db>,
) -> Result<Definition, String> {
    definition::wf_def_save_impl(spec, id, hue, db.inner()).await
}

/// Every stored definition.
#[tauri::command]
pub async fn wf_def_list(db: State<'_, Db>) -> Result<Vec<Definition>, String> {
    definition::wf_def_list_impl(db.inner()).await
}

/// Delete a definition by id.
#[tauri::command]
pub async fn wf_def_delete(id: String, db: State<'_, Db>) -> Result<(), String> {
    definition::wf_def_delete_impl(id, db.inner()).await
}

/// A definition as portable YAML.
#[tauri::command]
pub async fn wf_def_export_yaml(id: String, db: State<'_, Db>) -> Result<String, String> {
    definition::wf_def_export_yaml_impl(id, db.inner()).await
}

/// Import a definition from YAML, reporting what was created or skipped.
#[tauri::command]
pub async fn wf_def_import_yaml(
    yaml_text: String,
    db: State<'_, Db>,
) -> Result<ImportReport, String> {
    definition::wf_def_import_yaml_impl(yaml_text, db.inner()).await
}

// ───────────────────────────── runs ─────────────────────────────────────────

/// Launch a run from a launch-time `spec` snapshot.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn wf_launch(
    spec: Spec,
    task: String,
    project_id: String,
    repo_path: String,
    definition_id: Option<String>,
    base_branch: Option<String>,
    attachments: Vec<String>,
    issue_ref: Option<String>,
    service: State<'_, Arc<WorkflowService>>,
    supervisor: State<'_, Arc<Supervisor>>,
) -> Result<String, String> {
    service::wf_launch_impl(
        spec,
        task,
        project_id,
        repo_path,
        definition_id,
        base_branch,
        attachments,
        issue_ref,
        service.inner(),
        supervisor.inner(),
    )
    .await
}

/// Cancel a live run.
#[tauri::command]
pub async fn wf_cancel(
    run_id: String,
    service: State<'_, Arc<WorkflowService>>,
) -> Result<(), String> {
    service::wf_cancel_impl(run_id, service.inner()).await
}

/// Delete a terminal run and everything it owns.
#[tauri::command]
pub async fn wf_delete_run(
    run_id: String,
    service: State<'_, Arc<WorkflowService>>,
    supervisor: State<'_, Arc<Supervisor>>,
) -> Result<(), String> {
    service::wf_delete_run_impl(run_id, service.inner(), supervisor.inner()).await
}
