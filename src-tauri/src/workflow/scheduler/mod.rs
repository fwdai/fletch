//! The run scheduler (spec §6). One tokio task per active run walks the block
//! tree, drives each step through [`attempt::run_attempt`], ferries the `done`
//! commit into the run repo (§12.1), advances the cursor, and finalizes. S4b
//! covered **linear** runs, S8 added **parallel** stages, and S7 adds **loop**
//! blocks (§6.6): the walker dispatches each top-level block, and a `loop` runs
//! its body sequence per iteration until the `until` step's verdict is `done` or
//! `loop.max` is reached. Orchestrate execution arrives in S11 (a block of that
//! kind fails the run with a clear cause rather than being silently skipped).
//!
//! `WorkflowService` (app state) owns the registry of active runs and the
//! launch / control commands. Panic containment (§6.1): the service awaits each
//! drive task's `JoinHandle`; a panicked or errored task marks its run
//! `failed("internal scheduler error")` so a run is never left `running` with no
//! live driver.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use tauri::AppHandle;
use tokio::task::JoinSet;

use crate::error::{Error, Result};
use crate::supervisor::Supervisor;

use super::attempt::{self, AttemptOutcome, AttemptParams, Deadlines};
use super::blackboard;
use super::budget::{EffectiveBudgets, Ledger};
use super::driver::{AgentDriver, SpawnReq};
use super::gitops;
use super::journal;
use super::prompts::{self, IterationPos, Position, StepPromptCtx};
use super::spec::{
    AgentSpec, Block, Budgets, Gate, Integrate, Join, Loop, Orchestrate, Parallel, Spec, Step,
};
use super::types::event_type;

type Db = Arc<Mutex<Connection>>;

mod cleanup;
mod context;
mod drive;
mod orchestrate;
mod parallel;
mod persistence;
mod steps;
#[cfg(test)]
mod tests;

pub(crate) use cleanup::*;
pub(crate) use context::*;
pub(crate) use drive::*;
pub(crate) use orchestrate::*;
pub(crate) use parallel::*;
pub(crate) use persistence::*;
pub(crate) use steps::*;

/// App-state singleton: the active-run registry plus launch / control.
pub struct WorkflowService {
    pub(super) db: Db,
    driver: Arc<dyn AgentDriver>,
    pub(super) app: AppHandle,
    /// Active-run registry. Behind an `Arc` so a drive task can remove its own
    /// entry on exit without borrowing the service.
    pub(super) runs: Arc<Mutex<HashMap<String, RunHandle>>>,
    /// Serializes workflow creation/deletion. Project deletion holds this from
    /// its run preflight through the project commit, so no new run can appear
    /// between cleanup and deletion.
    pub(crate) lifecycle: tokio::sync::Mutex<()>,
}

pub(crate) struct RunHandle {
    cancel: Arc<AtomicBool>,
    /// Set when a spawn request arrives while this driver is winding down (its
    /// paused status already written, registry entry not yet removed). The
    /// watchdog re-drives after removing the entry instead of dropping the
    /// request — an approve that raced the wind-down would otherwise leave the
    /// run paused forever with nothing left to approve.
    respawn: Arc<AtomicBool>,
    /// Raised by the comms router (§10.4) when a live step raises a `wf_ask`
    /// routed to the human: the running attempt observes it at turn end and
    /// returns `AwaitingAnswer`, so the run pauses `question` without gating.
    /// Shared with that run's in-flight attempt (`AttemptParams::pending_ask`).
    pub(super) pending_ask: Arc<AtomicBool>,
}

/// Filesystem half of an atomic project deletion. Run directories are renamed
/// aside before the database transaction; they can therefore be restored if
/// the transaction fails, or swept after it commits. Startup recovery handles
/// the same two states if the app exits between either boundary.
pub(crate) struct ProjectRunCleanup {
    run_ids: Vec<String>,
    staged_dirs: Vec<StagedRunDir>,
    finalized: bool,
}

impl ProjectRunCleanup {
    pub(crate) fn run_ids(&self) -> &[String] {
        &self.run_ids
    }

    pub(crate) fn restore(mut self) {
        restore_staged_run_dirs(&self.staged_dirs);
        self.finalized = true;
    }

    fn finish(mut self, app: &AppHandle) {
        // The DB commit has happened; never let Drop restore directories for
        // rows that no longer exist. Failed removals are startup-recoverable.
        self.finalized = true;
        for dir in &self.staged_dirs {
            dir.remove_staged();
        }
        for run_id in &self.run_ids {
            journal::emit_run_deleted(app, run_id);
        }
    }
}

impl Drop for ProjectRunCleanup {
    fn drop(&mut self) {
        if !self.finalized {
            restore_staged_run_dirs(&self.staged_dirs);
        }
    }
}

impl WorkflowService {
    pub fn new(db: Db, driver: Arc<dyn AgentDriver>, app: AppHandle) -> Self {
        Self {
            db,
            driver,
            app,
            runs: Arc::new(Mutex::new(HashMap::new())),
            lifecycle: tokio::sync::Mutex::new(()),
        }
    }

    /// Launch a run from a snapshot `spec` against `repo_path`. Provisions the
    /// run directory (blackboard + run repo), inserts the `wf_run` row, and
    /// spawns its drive task. Returns the new run id.
    #[allow(clippy::too_many_arguments)]
    pub async fn launch(
        &self,
        spec: Spec,
        task: String,
        project_id: String,
        repo_path: String,
        definition_id: Option<String>,
        base_branch: Option<String>,
        // An explicit fork-point commit (promote-to-workflow): the run forks from
        // this SHA instead of a branch tip. Wins over `base_branch` for the fork
        // point, and leaves `base_branch` empty so finalization falls back to
        // `finalize.pr_base`/`main` rather than treating a raw SHA as a PR base.
        base_sha_override: Option<String>,
        // Launch-time file attachments (absolute paths). Persisted durably in a
        // host-only sidecar and rendered as `Attached file: {path}` lines into the
        // entry step's prompt only (see `drive_run_inner` / `execute_step`) — the
        // same form a chat message delivers them, scoped to the first agent.
        attachments: Vec<String>,
        // The GitHub issue this run was started from (Home-inbox "Start work" →
        // Pipeline), as a bare issue number. Threaded onto the run row so the
        // finalize open-PR path can append a `Closes #<n>` trailer. `None` for a
        // normal launch — backward-compatible with today's behavior.
        issue_ref: Option<String>,
    ) -> Result<String> {
        let _lifecycle_guard = self.lifecycle.lock().await;
        let run_id = format!("run-{}", uuid::Uuid::new_v4());
        let repo = PathBuf::from(&repo_path);

        // Resolve the fork point to a SHA in the source repo now, so it is fixed
        // and journaled (§12.2). An explicit override (promotion) resolves
        // directly; otherwise the caller's branch, the spec's pr_base, then HEAD.
        let base_ref = base_sha_override
            .clone()
            .or_else(|| base_branch.clone())
            .or_else(|| spec.finalize.as_ref().and_then(|f| f.pr_base.clone()))
            .unwrap_or_else(|| "HEAD".to_string());
        let base_sha = crate::git::rev_parse(&repo, &base_ref)
            .await
            .map_err(|e| Error::Other(format!("cannot resolve base '{base_ref}': {e}")))?;
        // Persist the caller-selected branch name (not the "HEAD"/pr_base/SHA
        // fallback), so finalization can open the PR against the branch the run
        // forked from when the spec doesn't pin `finalize.pr_base` (§12.2). A
        // SHA-forked promotion carries no branch name.
        let base_branch = if base_sha_override.is_some() {
            String::new()
        } else {
            base_branch.unwrap_or_default()
        };

        let run_dir = blackboard::run_dir(&run_id)?;
        let task_md = format!("# {}\n\n{}\n", spec.name, task);
        blackboard::provision(&run_dir, &task_md)?;
        // Durable, read-only: persisted once here, re-read on every drive, and
        // rendered only into the entry step's prompt — so recovery is trivial
        // (no ephemeral delivery state) and non-entry steps never see them.
        blackboard::write_attachments(&run_dir, &attachments)?;
        gitops::provision_run_repo(&repo, &run_dir).await?;

        let branch = format!("wf/{}-{}", slugify(&spec.name), &run_id[run_id.len() - 8..]);
        let spec_json = serde_json::to_string(&spec).map_err(|e| Error::Other(e.to_string()))?;
        // Freeze the effective budgets (§11.1 defaults ∪ spec) at launch — the
        // immutable-except-by-resume-patch source of truth for enforcement (§11.2).
        let budgets_json = serde_json::to_string(&EffectiveBudgets::resolve(&spec))
            .map_err(|e| Error::Other(e.to_string()))?;

        let now = super::now_ms();
        {
            let conn = self.db.lock();
            conn.execute(
                "INSERT INTO wf_run (id, definition_id, parent_run_id, name, spec_json, task,
                     project_id, repo_path, run_dir, branch, base_sha, base_branch, status,
                     budgets_json, spent_json, created_at, updated_at, issue_ref)
                 VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'pending', ?12, '{}', ?13, ?13, ?14)",
                rusqlite::params![
                    run_id,
                    definition_id,
                    spec.name,
                    spec_json,
                    task,
                    project_id,
                    repo_path,
                    run_dir.to_string_lossy(),
                    branch,
                    base_sha,
                    base_branch,
                    budgets_json,
                    now,
                    issue_ref,
                ],
            )
            .map_err(|e| Error::Other(e.to_string()))?;
        }

        self.spawn_drive(run_id.clone());
        Ok(run_id)
    }

    /// Re-drive every run left `pending`/`running` at startup (spec §6.1); a
    /// `paused` run waits for a user action. Best-effort per run. Also
    /// reconciles run dirs a crashed `wf_delete_run` left staged (§13), so a
    /// surviving run is valid again before anyone opens it.
    pub fn resume_active_runs(&self) {
        {
            let conn = self.db.lock();
            if let Ok(root) = blackboard::runs_root() {
                recover_staged_run_dirs(&conn, &root);
            }
        }
        let ids: Vec<String> = {
            let conn = self.db.lock();
            conn.prepare("SELECT id FROM wf_run WHERE status IN ('pending','running')")
                .and_then(|mut s| {
                    s.query_map([], |r| r.get::<_, String>(0))?
                        .collect::<std::result::Result<Vec<_>, _>>()
                })
                .unwrap_or_default()
        };
        for id in ids {
            self.spawn_drive(id);
        }
    }

    /// Cancel a run: flag it, stop the live attempt's agent, and (if no driver
    /// is live) mark it canceled directly. Cancelling a parent cascades to its
    /// composed sub-runs (spec §6.1, §10.3) — depth-first so the whole tree winds
    /// down.
    pub async fn cancel(&self, run_id: &str) -> Result<()> {
        // Cascade to sub-runs first, so a child can't outlive a parent that has
        // already been marked canceled.
        let children = {
            let conn = self.db.lock();
            child_run_ids(&conn, run_id)
        };
        for child in children {
            Box::pin(self.cancel(&child)).await?;
        }

        let handle = self.runs.lock().get(run_id).map(|h| h.cancel.clone());
        match handle {
            Some(cancel) => cancel.store(true, Ordering::SeqCst),
            None => {
                // No live driver — a paused/pending run. Mark it canceled and
                // stop any lingering run-owned agent.
                self.stop_live_step_agents(run_id).await;
                let conn = self.db.lock();
                set_status(&conn, Some(&self.app), run_id, "canceled", None, None);
            }
        }
        Ok(())
    }

    /// Delete a terminal run (`wf_delete_run`, spec §13): cascade over composed
    /// sub-runs, discard every run-owned step-agent workspace (`owner_run_id`)
    /// through the app path — a DB cascade would orphan checkouts and dangle
    /// supervisor state (0019) — remove `~/.fletch/runs/<id>/` (blackboard +
    /// run repo), and delete the run's rows (`wf_step_exec` / `wf_event` /
    /// `wf_message` cascade off `wf_run`). Chats of deleted runs are gone; the
    /// UI's confirm dialog says so. The whole tree is checked before anything
    /// is touched, so a rejected delete changes nothing.
    ///
    /// Deletion of the tree itself is best-effort per subtree: workspace
    /// discards and dir removals are irreversible, so a mid-tree failure can't
    /// roll back — instead a failed run keeps itself *and its ancestors* fully
    /// intact (its `wf_run` row is what a retry rediscovers the cleanup
    /// through, and the `parent_run_id` FK forbids deleting a parent under a
    /// surviving child), sibling subtrees still get cleaned, and every failure
    /// is aggregated into one error that says deleting again finishes the job.
    pub async fn delete_run(&self, supervisor: &Arc<Supervisor>, run_id: &str) -> Result<()> {
        let _lifecycle_guard = self.lifecycle.lock().await;
        self.delete_run_locked(supervisor, run_id).await
    }

    async fn delete_run_locked(&self, supervisor: &Arc<Supervisor>, run_id: &str) -> Result<()> {
        let order = {
            let conn = self.db.lock();
            let mut order = Vec::new();
            run_tree_post_order(&conn, run_id, &mut order);
            check_deletable(&conn, &order)?;
            order
        };
        // A terminal run normally has no live driver, but a drive task can
        // still be winding down (status written, registry entry not yet
        // removed). Deleting its rows out from under it would race — reject.
        {
            let runs = self.runs.lock();
            if let Some(id) = order.iter().find(|id| runs.contains_key(id.as_str())) {
                return Err(Error::Other(format!(
                    "run {id} is still winding down — try again in a moment"
                )));
            }
        }

        let mut errors = Vec::new();
        self.delete_tree(supervisor, run_id, &mut errors).await;
        if errors.is_empty() {
            return Ok(());
        }
        Err(Error::Other(format!(
            "run deletion incomplete: {}. Every run that failed (and its \
             parents) is untouched — delete again to finish.",
            errors.join("; ")
        )))
    }

    /// Preflight and stage every workflow run owned by a project. The caller
    /// holds `lifecycle` through the subsequent project/run DB transaction, so
    /// the checked set cannot change before that single commit point.
    pub(crate) fn prepare_project_runs_locked(
        &self,
        project_id: &str,
    ) -> Result<ProjectRunCleanup> {
        let all_ids = {
            let conn = self.db.lock();
            let all_ids = conn
                .prepare("SELECT id FROM wf_run WHERE project_id = ?1 ORDER BY created_at, id")?
                .query_map([project_id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            check_deletable(&conn, &all_ids)?;
            all_ids
        };

        // Match delete_run's winding-down guard, but check the whole project
        // before touching any tree so rejection is side-effect free.
        {
            let runs = self.runs.lock();
            if let Some(id) = all_ids.iter().find(|id| runs.contains_key(id.as_str())) {
                return Err(Error::Other(format!(
                    "run {id} is still winding down — try again in a moment"
                )));
            }
        }

        let staged_dirs = stage_run_dirs(&all_ids)?;
        Ok(ProjectRunCleanup {
            run_ids: all_ids,
            staged_dirs,
            finalized: false,
        })
    }

    pub(crate) fn finish_project_runs(&self, cleanup: ProjectRunCleanup) {
        cleanup.finish(&self.app);
    }

    /// Delete `run_id`'s subtree, children first. Returns whether the whole
    /// subtree is gone; a failure is pushed onto `errors` and keeps this run's
    /// rows (deleted last, so the run stays discoverable for a retry) and, via
    /// the `false` return, every ancestor's.
    async fn delete_tree(
        &self,
        supervisor: &Arc<Supervisor>,
        run_id: &str,
        errors: &mut Vec<String>,
    ) -> bool {
        let children = {
            let conn = self.db.lock();
            child_run_ids(&conn, run_id)
        };
        let mut children_deleted = true;
        for child in children {
            if !Box::pin(self.delete_tree(supervisor, &child, errors)).await {
                children_deleted = false;
            }
        }
        if !children_deleted {
            // A surviving child row still points at this run (`parent_run_id`),
            // and a half-gutted parent would strand the retry path — leave this
            // run entirely alone.
            return false;
        }

        // Discard every owned workspace before giving up, rather than bailing on
        // the first failure: a single wedged agent must not strand the others,
        // which would leave the surviving (kept-for-retry) run showing some
        // agents already gone and others intact. A successful discard removes the
        // workspace row, so `agents_for_run` no longer lists it — a re-delete
        // only sees the ones that failed, and retries just those.
        let agent_ids: Vec<String> = supervisor
            .workspace
            .agents_for_run(run_id)
            .into_iter()
            .map(|a| a.id)
            .collect();
        let all_discarded = discard_all(&agent_ids, run_id, errors, |id| {
            let sup = supervisor.clone();
            async move { sup.discard_agent(&id).await }
        })
        .await;
        if !all_discarded {
            // A workspace survives; keep this run's row (and, via `false`, its
            // ancestors') so a re-delete rediscovers and finishes the cleanup.
            return false;
        }
        let deleted = {
            let conn = self.db.lock();
            delete_run_data(&conn, run_id)
        };
        if let Err(e) = deleted {
            errors.push(format!("run {run_id}: {e}"));
            return false;
        }
        journal::emit_run_deleted(&self.app, run_id);
        true
    }

    /// Resume a paused run (`wf_resume`): optionally raise the budget (§11.2,
    /// §13), then re-drive from the cursor. A fresh attempt is started for a
    /// blocked / stalled / budget-exceeded step by the drive loop. A patch that
    /// lifts the tripped cap is what lets a `budget_exceeded` run make progress;
    /// resuming without one simply re-pauses at the same cap.
    pub fn resume(&self, run_id: &str, budget_patch: Option<Budgets>) -> Result<()> {
        {
            let conn = self.db.lock();
            // Validate resumability BEFORE touching the budget — a rejected
            // resume (terminal or approval-paused run) must not mutate the
            // otherwise-immutable `budgets_json`.
            check_resumable(&conn, run_id, "resume")?;
            if let Some(patch) = budget_patch {
                let budgets_json: String = conn
                    .query_row(
                        "SELECT budgets_json FROM wf_run WHERE id = ?1",
                        [run_id],
                        |r| r.get(0),
                    )
                    .map_err(|e| Error::Other(format!("run {run_id} not found: {e}")))?;
                let mut eff: EffectiveBudgets = serde_json::from_str(&budgets_json)
                    .map_err(|e| Error::Other(format!("bad budgets_json: {e}")))?;
                eff.apply_patch(&patch);
                let patched =
                    serde_json::to_string(&eff).map_err(|e| Error::Other(e.to_string()))?;
                conn.execute(
                    "UPDATE wf_run SET budgets_json = ?1, updated_at = ?2 WHERE id = ?3",
                    rusqlite::params![patched, super::now_ms(), run_id],
                )
                .map_err(|e| Error::Other(e.to_string()))?;
            }
        }
        self.spawn_drive(run_id.to_string());
        Ok(())
    }

    /// User-initiated retry after `paused(blocked_gate|stalled)` — same as
    /// resume; the drive loop starts a fresh attempt (one beyond `max_attempts`,
    /// since this is an explicit human decision, §6.5).
    pub fn retry(&self, run_id: &str) -> Result<()> {
        self.resume_paused(run_id, "retry")
    }

    /// Shared resume/retry guard: only a `paused(blocked_gate|stalled|
    /// budget_exceeded)` run may be re-driven. A terminal run must not restart,
    /// and a `paused(approval)` run must go through `wf_approve` — re-driving it
    /// would start a fresh attempt for a step whose result is already ferried.
    fn resume_paused(&self, run_id: &str, action: &str) -> Result<()> {
        {
            let conn = self.db.lock();
            check_resumable(&conn, run_id, action)?;
        }
        self.spawn_drive(run_id.to_string());
        Ok(())
    }

    /// Approve an `awaiting_approval` step: the boundary commit was already
    /// ferried at the pause, so approval promotes that attempt to `done` (so the
    /// next fork and the finalize push include it), advances the cursor, and
    /// resumes.
    pub fn approve(&self, run_id: &str) -> Result<()> {
        {
            let conn = self.db.lock();
            let (status, reason) = run_status(&conn, run_id)?;
            if status != "paused" || reason.as_deref() != Some("approval") {
                return Err(Error::Other(format!(
                    "run is not awaiting approval (status: {status})"
                )));
            }
            let exec_id: String = conn
                .query_row(
                    "SELECT id FROM wf_step_exec WHERE run_id = ?1 AND status = 'awaiting_approval'
                     ORDER BY rowid DESC LIMIT 1",
                    [run_id],
                    |r| r.get(0),
                )
                .map_err(|e| Error::Other(format!("no awaiting_approval attempt: {e}")))?;
            conn.execute(
                "UPDATE wf_step_exec SET status = 'done' WHERE id = ?1",
                [&exec_id],
            )
            .map_err(|e| Error::Other(e.to_string()))?;
            journal_event(
                &conn,
                Some(&self.app),
                run_id,
                event_type::DECISION,
                Some(&exec_id),
                &json!({ "decision": "approved" }),
            );
            // Advance the cursor only when the approved step is a top-level step:
            // its `done` ref is the next block's fork source. An approval inside a
            // loop body must NOT bump the top-level index (that would skip the rest
            // of the loop) — the loop's resume-skip promotes it on re-drive (§6.6).
            let mut cursor = get_cursor(&conn, run_id);
            if top_level_block_is_step(&conn, run_id, cursor.index) {
                cursor.index += 1;
                set_cursor(&conn, run_id, &cursor);
            }
        }
        self.spawn_drive(run_id.to_string());
        Ok(())
    }

    /// Reject an `awaiting_approval` step (`wf_reject`, spec §9): journal the
    /// human decision, then give the step one more attempt within budget — the
    /// mirror of [`Self::approve`]. The rejected attempt is abandoned (its ferried
    /// work discarded) and the reviewer's `note` is queued as a delivery so the
    /// fresh attempt re-prompts with it (the same fold `wf_answer` uses, §10.4);
    /// the cursor is left in place so the walker re-runs the step. When the run
    /// budget is already spent there is no attempt to give — pause `blocked_gate`
    /// carrying the note as detail, exactly like an exhausted blocked-gate retry
    /// (§6.5).
    pub fn reject(&self, run_id: &str, note: &str) -> Result<()> {
        let re_drive = {
            let conn = self.db.lock();
            reject_apply(&conn, Some(&self.app), run_id, note)?
        };
        if re_drive {
            self.spawn_drive(run_id.to_string());
        }
        Ok(())
    }

    /// Diff `from_sha..to_sha` in the run's own repository (`wf_run_diff`, spec
    /// §9). Both refs are objects in `~/.fletch/runs/<id>/repo` — the ferried step
    /// ref and the run base — so no working checkout is involved.
    pub async fn run_diff(
        &self,
        run_id: &str,
        from_sha: &str,
        to_sha: &str,
        path: Option<&str>,
    ) -> Result<String> {
        let run_dir: String = {
            let conn = self.db.lock();
            conn.query_row("SELECT run_dir FROM wf_run WHERE id = ?1", [run_id], |r| {
                r.get(0)
            })
            .map_err(|e| Error::Other(format!("run {run_id} not found: {e}")))?
        };
        let run_repo = gitops::run_repo_path(Path::new(&run_dir));
        crate::git::diff_refs(&run_repo, from_sha, to_sha, path).await
    }

    /// Resolve a `paused(conflict)` run (`wf_resolve_conflict`, §12.3). `mode` is
    /// `"agent"` (spawn a conflict-resolution step forked from the snapshot) or
    /// `"human"` (the user resolved in the run repo's integration worktree). The
    /// choice is recorded on the merge cursor and the run re-driven; the merge
    /// stage's resume path applies it. Mode `"orchestrator"` (§12.3 b) arrives
    /// with S11.
    pub fn resolve_conflict(&self, run_id: &str, mode: &str) -> Result<()> {
        if !matches!(mode, "agent" | "human") {
            return Err(Error::Other(format!(
                "unknown conflict resolution mode '{mode}' (expected 'agent' or 'human')"
            )));
        }
        {
            let conn = self.db.lock();
            let (status, reason) = run_status(&conn, run_id)?;
            if status != "paused" || reason.as_deref() != Some("conflict") {
                return Err(Error::Other(format!(
                    "run is not paused on a conflict (status: {status})"
                )));
            }
            let mut cursor = get_cursor(&conn, run_id);
            let ci = cursor
                .merge
                .as_mut()
                .and_then(|m| m.conflict.as_mut())
                .ok_or_else(|| Error::Other("no recorded merge conflict to resolve".into()))?;
            ci.resolution = Some(mode.to_string());
            set_cursor(&conn, run_id, &cursor);
        }
        self.spawn_drive(run_id.to_string());
        Ok(())
    }

    pub(super) fn spawn_drive(&self, run_id: String) {
        spawn_drive_task(
            self.db.clone(),
            self.driver.clone(),
            self.app.clone(),
            self.runs.clone(),
            run_id,
        );
    }

    async fn stop_live_step_agents(&self, run_id: &str) {
        let agent_ids = live_step_agents(&self.db.lock(), run_id);
        for a in agent_ids {
            let _ = self.driver.stop(&a).await;
        }
    }
}

type Svc<'a> = tauri::State<'a, Arc<WorkflowService>>;

/// Launch a run from a launch-time `spec` snapshot (spec §13).
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
    // Set when the launch originates from a Home-inbox issue (Pipeline mode);
    // carried onto the run row so its finalized PR closes the issue. `None` for
    // a normal launch.
    issue_ref: Option<String>,
    service: Svc<'_>,
    supervisor: tauri::State<'_, Arc<Supervisor>>,
) -> std::result::Result<String, String> {
    // A run targets one repo, so its project is authoritatively that repo's.
    // Resolve it from `repo_path` here rather than trusting the caller's
    // snapshot: a path-normalization mismatch or a stale workspace snapshot
    // could otherwise pass an empty id and orphan the run from project-scoped
    // queries. Fall back to the caller's value only if resolution fails.
    let project_id = supervisor
        .workspace
        .project_id_for_repo(&repo_path)
        .unwrap_or(project_id);
    service
        .launch(
            spec,
            task,
            project_id,
            repo_path,
            definition_id,
            base_branch,
            base_sha,
            attachments,
            issue_ref,
        )
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn wf_cancel(run_id: String, service: Svc<'_>) -> std::result::Result<(), String> {
    service.cancel(&run_id).await.map_err(|e| e.to_string())
}

/// Resume a paused run (§13), optionally raising the budget with a patch
/// ("+N turns / +N tokens / +N minutes") for a `budget_exceeded` pause.
#[tauri::command]
pub async fn wf_resume(
    run_id: String,
    budget_patch: Option<Budgets>,
    service: Svc<'_>,
) -> std::result::Result<(), String> {
    service
        .resume(&run_id, budget_patch)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn wf_retry(run_id: String, service: Svc<'_>) -> std::result::Result<(), String> {
    service.retry(&run_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn wf_approve(run_id: String, service: Svc<'_>) -> std::result::Result<(), String> {
    service.approve(&run_id).map_err(|e| e.to_string())
}

/// Reject a run paused on an approval gate (spec §9): re-prompt the step with the
/// `note` for one more attempt within budget, else pause `blocked_gate`.
#[tauri::command]
pub async fn wf_reject(
    run_id: String,
    note: String,
    service: Svc<'_>,
) -> std::result::Result<(), String> {
    service.reject(&run_id, &note).map_err(|e| e.to_string())
}

/// The unified diff of `from_sha..to_sha` in a run's own repository (spec §9) —
/// the review surface diffs a ferried step ref against the run base, both objects
/// in `~/.fletch/runs/<id>/repo`. `path` scopes it to one file; omit for the whole
/// diff. Read-only.
#[tauri::command]
pub async fn wf_run_diff(
    run_id: String,
    from_sha: String,
    to_sha: String,
    path: Option<String>,
    service: Svc<'_>,
) -> std::result::Result<String, String> {
    service
        .run_diff(&run_id, &from_sha, &to_sha, path.as_deref())
        .await
        .map_err(|e| e.to_string())
}

/// Delete a terminal run and everything it owns (spec §13): run-owned step
/// agents (and their chats), the run directory, and the run's rows.
#[tauri::command]
pub async fn wf_delete_run(
    run_id: String,
    service: Svc<'_>,
    supervisor: tauri::State<'_, Arc<Supervisor>>,
) -> std::result::Result<(), String> {
    service
        .delete_run(supervisor.inner(), &run_id)
        .await
        .map_err(|e| e.to_string())
}

/// Resolve a merge conflict (§12.3): `mode` is `"agent"` or `"human"`.
#[tauri::command]
pub async fn wf_resolve_conflict(
    run_id: String,
    mode: String,
    service: Svc<'_>,
) -> std::result::Result<(), String> {
    service
        .resolve_conflict(&run_id, &mode)
        .map_err(|e| e.to_string())
}
