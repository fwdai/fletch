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
use fletch_core::workflow::scheduler::{self, WorkflowService};
use fletch_core::workflow::spec::{Budgets, Spec};
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

/// A run plus its attempts and messages. `None` if the run doesn't exist.
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

// ───────────────────────────── scheduler ────────────────────────────────────

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
    base_sha: Option<String>,
    attachments: Vec<String>,
    issue_ref: Option<String>,
    service: State<'_, Arc<WorkflowService>>,
    supervisor: State<'_, Arc<Supervisor>>,
) -> Result<String, String> {
    scheduler::wf_launch_impl(
        spec,
        task,
        project_id,
        repo_path,
        definition_id,
        base_branch,
        base_sha,
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
    scheduler::wf_cancel_impl(run_id, service.inner()).await
}

/// Resume a paused run, optionally raising the budget with a patch.
#[tauri::command]
pub async fn wf_resume(
    run_id: String,
    budget_patch: Option<Budgets>,
    service: State<'_, Arc<WorkflowService>>,
) -> Result<(), String> {
    scheduler::wf_resume_impl(run_id, budget_patch, service.inner()).await
}

/// Retry the current step of a paused run.
#[tauri::command]
pub async fn wf_retry(
    run_id: String,
    service: State<'_, Arc<WorkflowService>>,
) -> Result<(), String> {
    scheduler::wf_retry_impl(run_id, service.inner()).await
}

/// Approve a run paused on an approval gate.
#[tauri::command]
pub async fn wf_approve(
    run_id: String,
    service: State<'_, Arc<WorkflowService>>,
) -> Result<(), String> {
    scheduler::wf_approve_impl(run_id, service.inner()).await
}

/// Reject a run paused on an approval gate, re-prompting the step with `note`.
#[tauri::command]
pub async fn wf_reject(
    run_id: String,
    note: String,
    service: State<'_, Arc<WorkflowService>>,
) -> Result<(), String> {
    scheduler::wf_reject_impl(run_id, note, service.inner()).await
}

/// The unified diff of `from_sha..to_sha` in a run's own repository.
#[tauri::command]
pub async fn wf_run_diff(
    run_id: String,
    from_sha: String,
    to_sha: String,
    path: Option<String>,
    service: State<'_, Arc<WorkflowService>>,
) -> Result<String, String> {
    scheduler::wf_run_diff_impl(run_id, from_sha, to_sha, path, service.inner()).await
}

/// Resolve a merge conflict; `mode` is `"agent"` or `"human"`.
#[tauri::command]
pub async fn wf_resolve_conflict(
    run_id: String,
    mode: String,
    service: State<'_, Arc<WorkflowService>>,
) -> Result<(), String> {
    scheduler::wf_resolve_conflict_impl(run_id, mode, service.inner()).await
}

/// Delete a terminal run and everything it owns.
#[tauri::command]
pub async fn wf_delete_run(
    run_id: String,
    service: State<'_, Arc<WorkflowService>>,
    supervisor: State<'_, Arc<Supervisor>>,
) -> Result<(), String> {
    scheduler::wf_delete_run_impl(run_id, service.inner(), supervisor.inner()).await
}

// ───────────────────────────── comms ────────────────────────────────────────

/// Answer a paused `question` and resume the run.
#[tauri::command]
pub async fn wf_answer(
    project_id: String,
    run_id: String,
    message_id: String,
    body: String,
    service: State<'_, Arc<WorkflowService>>,
) -> Result<(), String> {
    fletch_core::workflow::comms::wf_answer_impl(
        project_id,
        run_id,
        message_id,
        body,
        service.inner(),
    )
    .await
}
