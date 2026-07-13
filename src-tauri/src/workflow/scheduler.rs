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

/// App-state singleton: the active-run registry plus launch / control.
pub struct WorkflowService {
    pub(super) db: Db,
    driver: Arc<dyn AgentDriver>,
    pub(super) app: AppHandle,
    /// Active-run registry. Behind an `Arc` so a drive task can remove its own
    /// entry on exit without borrowing the service.
    pub(super) runs: Arc<Mutex<HashMap<String, RunHandle>>>,
}

pub(super) struct RunHandle {
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

impl WorkflowService {
    pub fn new(db: Db, driver: Arc<dyn AgentDriver>, app: AppHandle) -> Self {
        Self {
            db,
            driver,
            app,
            runs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Launch a run from a snapshot `spec` against `repo_path`. Provisions the
    /// run directory (blackboard + run repo), inserts the `wf_run` row, and
    /// spawns its drive task. Returns the new run id.
    pub async fn launch(
        &self,
        spec: Spec,
        task: String,
        project_id: String,
        repo_path: String,
        definition_id: Option<String>,
        base_branch: Option<String>,
    ) -> Result<String> {
        let run_id = format!("run-{}", uuid::Uuid::new_v4());
        let repo = PathBuf::from(&repo_path);

        // Resolve the base branch to a SHA in the source repo now, so the fork
        // point is fixed and journaled (§12.2).
        let base_ref = base_branch
            .clone()
            .or_else(|| spec.finalize.as_ref().and_then(|f| f.pr_base.clone()))
            .unwrap_or_else(|| "HEAD".to_string());
        let base_sha = crate::git::rev_parse(&repo, &base_ref)
            .await
            .map_err(|e| Error::Other(format!("cannot resolve base '{base_ref}': {e}")))?;

        let run_dir = blackboard::run_dir(&run_id)?;
        let task_md = format!("# {}\n\n{}\n", spec.name, task);
        blackboard::provision(&run_dir, &task_md)?;
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
                     project_id, repo_path, run_dir, branch, base_sha, status, budgets_json,
                     spent_json, created_at, updated_at)
                 VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'pending', ?11, '{}', ?12, ?12)",
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
                    budgets_json,
                    now,
                ],
            )
            .map_err(|e| Error::Other(e.to_string()))?;
        }

        self.spawn_drive(run_id.clone());
        Ok(run_id)
    }

    /// Re-drive every run left `pending`/`running` at startup (spec §6.1); a
    /// `paused` run waits for a user action. Best-effort per run.
    pub fn resume_active_runs(&self) {
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
    /// is live) mark it canceled directly.
    pub async fn cancel(&self, run_id: &str) -> Result<()> {
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

/// Register `run_id` in the active-run map and spawn its drive task. A free
/// function (not a method) so the watchdog can re-invoke it after removing the
/// registry entry.
///
/// A run has at most one live driver (§6.1). If an entry already exists, the
/// driver may be winding down — its paused status written, its entry not yet
/// removed by the watchdog. A command landing in that window (approve, resume,
/// retry) must not be dropped: flag the entry so the watchdog re-drives right
/// after removal.
fn spawn_drive_task(
    db: Db,
    driver: Arc<dyn AgentDriver>,
    app: AppHandle,
    runs: Arc<Mutex<HashMap<String, RunHandle>>>,
    run_id: String,
) {
    let cancel = Arc::new(AtomicBool::new(false));
    let respawn = Arc::new(AtomicBool::new(false));
    let pending_ask = Arc::new(AtomicBool::new(false));
    {
        let mut m = runs.lock();
        if let Some(existing) = m.get(&run_id) {
            existing.respawn.store(true, Ordering::SeqCst);
            return;
        }
        m.insert(
            run_id.clone(),
            RunHandle {
                cancel: cancel.clone(),
                respawn: respawn.clone(),
                pending_ask: pending_ask.clone(),
            },
        );
    }

    let ctx = RunCtx {
        db: db.clone(),
        driver: driver.clone(),
        app: Some(app.clone()),
        cancel,
        pending_ask,
        deadlines: Deadlines::default(),
    };
    let id = run_id.clone();
    let join = tauri::async_runtime::spawn(async move {
        drive_run(&ctx, &id).await;
    });
    // Panic containment (§6.1): a panicked/aborted drive task marks its run
    // failed so it is never left `running` with no live driver.
    tauri::async_runtime::spawn(async move {
        let panicked = join.await.is_err();
        if panicked {
            let conn = db.lock();
            set_status(
                &conn,
                Some(&app),
                &run_id,
                "failed",
                None,
                Some("internal scheduler error"),
            );
        }
        // Read the respawn flag under the same lock as the removal so a
        // request can't slip between the two.
        let respawn_requested = {
            let mut m = runs.lock();
            m.remove(&run_id);
            respawn.load(Ordering::SeqCst)
        };
        if respawn_requested && !panicked {
            spawn_drive_task(db, driver, app, runs, run_id);
        }
    });
}

// ───────────────────────────── the drive loop ───────────────────────────────

/// Everything the drive loop needs, decoupled from the service so it is testable
/// with a `MockDriver`, a temp DB, and no `AppHandle`.
struct RunCtx {
    db: Db,
    driver: Arc<dyn AgentDriver>,
    /// `None` under test — the DB is the source of truth; frontend emits are
    /// skipped.
    app: Option<AppHandle>,
    cancel: Arc<AtomicBool>,
    /// The run's pending-ask flag (§10.4), shared with the [`RunHandle`] so the
    /// comms router can raise it; threaded into each attempt.
    pending_ask: Arc<AtomicBool>,
    deadlines: Deadlines,
}

/// The `wf_run` columns the walker reads.
struct RunEssentials {
    spec_json: String,
    task: String,
    project_id: String,
    repo_path: String,
    run_dir: String,
    branch: String,
    base_sha: String,
    status: String,
    budgets_json: String,
    spent_json: String,
}

/// Drive one run to a terminal or paused state. Any error bubbling out marks the
/// run `failed` with the cause (the panic watchdog covers a hard panic).
async fn drive_run(ctx: &RunCtx, run_id: &str) {
    if let Err(e) = drive_run_inner(ctx, run_id).await {
        let conn = ctx.db.lock();
        set_status(
            &conn,
            ctx.app.as_ref(),
            run_id,
            "failed",
            None,
            Some(&e.to_string()),
        );
    }
}

async fn drive_run_inner(ctx: &RunCtx, run_id: &str) -> Result<()> {
    let run = load_run(&ctx.db.lock(), run_id)?;
    // Defense in depth: a stale respawn or a command racing a terminal write
    // must never restart a finished run.
    if matches!(run.status.as_str(), "done" | "failed" | "canceled") {
        return Ok(());
    }
    let spec: Spec =
        serde_json::from_str(&run.spec_json).map_err(|e| Error::Other(e.to_string()))?;
    ensure_executable(&spec.workflow)?;
    let blocks = &spec.workflow;
    let repo = PathBuf::from(&run.repo_path);
    let run_dir = PathBuf::from(&run.run_dir);
    let run_repo = gitops::run_repo_path(&run_dir);
    let blackboard = blackboard::blackboard_dir(&run_dir);

    // Run repo may need re-provisioning after a restart (idempotent).
    gitops::provision_run_repo(&repo, &run_dir).await?;

    // Tests gate (spec §9.4): resolve the project's test/setup command overrides
    // once (they are project-scoped). The runner is built per step below so it
    // honors the step's effective `tests_timeout_secs`; detection runs per step
    // worktree at gate time.
    let (test_override, setup_override) = {
        let conn = ctx.db.lock();
        (
            project_setting(&conn, &run.project_id, "run.test"),
            project_setting(&conn, &run.project_id, "run.install"),
        )
    };

    let first_time = run.status == "pending";
    {
        let conn = ctx.db.lock();
        let ev = if first_time {
            event_type::RUN_LAUNCHED
        } else {
            event_type::RUN_RESUMED
        };
        journal_event(
            &conn,
            ctx.app.as_ref(),
            run_id,
            ev,
            None,
            &json!({ "base_sha": run.base_sha }),
        );
        set_status(&conn, ctx.app.as_ref(), run_id, "running", None, None);
    }

    // Resume: abandon any attempt left non-terminal by a prior driver (§6.4).
    abandon_stale_attempts(ctx, run_id).await;

    // Budget ledger + frozen caps (§11). `budgets_json` is the launch-frozen
    // effective set; `spent_json` the running ledger (carried across resumes).
    // `start_drive` stamps the wall-clock so pause time between drives isn't
    // charged (§11.3).
    let eff: EffectiveBudgets = serde_json::from_str(&run.budgets_json)
        .unwrap_or_else(|_| EffectiveBudgets::resolve(&spec));
    let spent_val: Value = serde_json::from_str(&run.spent_json).unwrap_or_else(|_| json!({}));
    let mut ledger = Ledger::from_json(&spent_val);
    ledger.start_drive(super::now_ms());

    let mut cursor = get_cursor(&ctx.db.lock(), run_id);
    let mut index = cursor.index as usize;
    // The fork source for the current block: the last *linear step* before the
    // cursor that reached `done`, else the run base. A parallel `integrate: none`
    // stage never advances the line (§12.3), so its done children must not be
    // mistaken for the fork source on resume — hence a block-tree walk rather
    // than "the most recent done exec".
    let (mut last_ref, mut last_exec_id) =
        resume_line_state(&ctx.db.lock(), run_id, blocks, index, &run.base_sha);

    // Run-wide invariants every step attempt reads, bundled so the walker and the
    // loop executor share a single `execute_step` (spec §6.6).
    let env = StepEnv {
        repo: &repo,
        run_repo: &run_repo,
        blackboard: &blackboard,
        eff: &eff,
        test_override: &test_override,
        setup_override: &setup_override,
        run_task: &run.task,
        spec_name: &spec.name,
    };

    while index < blocks.len() {
        if ctx.cancel.load(Ordering::SeqCst) {
            // Bind first so the lock guard drops before the awaits below (a
            // guard held across `.await` would make the drive future `!Send`).
            let agents = live_step_agents(&ctx.db.lock(), run_id);
            for a in agents {
                let _ = ctx.driver.stop(&a).await;
            }
            let conn = ctx.db.lock();
            journal_event(
                &conn,
                ctx.app.as_ref(),
                run_id,
                event_type::RUN_CANCELED,
                None,
                &json!({}),
            );
            set_status(&conn, ctx.app.as_ref(), run_id, "canceled", None, None);
            return Ok(());
        }

        match &blocks[index] {
            Block::Step(step) => {
                let agent_spec = resolve_agent(&spec, step)?;
                let position = Position {
                    step_index: index,
                    step_count: blocks.len(),
                    iteration: None,
                };
                match execute_step(
                    ctx,
                    run_id,
                    &env,
                    step,
                    agent_spec,
                    position,
                    0,
                    &last_ref,
                    false,
                    &mut ledger,
                )
                .await?
                {
                    StepFlow::Done { exec_id, head_ref } => {
                        last_ref = head_ref;
                        last_exec_id = Some(exec_id);
                    }
                    StepFlow::Halt => return Ok(()),
                    // Only a loop's `until` step yields a loop signal.
                    StepFlow::LoopContinue => unreachable!("top-level step is never a loop until"),
                }
            }
            Block::Parallel(par) => {
                match run_parallel_stage(
                    ctx,
                    run_id,
                    &run,
                    &spec,
                    par,
                    index,
                    blocks.len(),
                    &blackboard,
                    &repo,
                    &run_repo,
                    &last_ref,
                    &eff,
                    &mut ledger,
                    &test_override,
                    &setup_override,
                    &mut cursor,
                )
                .await?
                {
                    // `integrate: none` leaves the line unchanged (`line: None`);
                    // `integrate: merge` advances it onto the integrated result.
                    StageFlow::Advance { line } => {
                        if let Some((head_ref, exec_id)) = line {
                            last_ref = head_ref;
                            last_exec_id = Some(exec_id);
                        }
                    }
                    StageFlow::Stop => return Ok(()),
                }
            }
            Block::Loop(lp) => {
                match run_loop(
                    ctx,
                    run_id,
                    &env,
                    &spec,
                    lp,
                    index,
                    &mut cursor,
                    &mut last_ref,
                    &mut last_exec_id,
                    &mut ledger,
                )
                .await?
                {
                    BlockFlow::Advance => {}
                    BlockFlow::Halt => return Ok(()),
                }
            }
            Block::Orchestrate(orch) => {
                match run_orchestrate_stage(
                    ctx,
                    run_id,
                    &run,
                    &spec,
                    orch,
                    index,
                    blocks.len(),
                    &blackboard,
                    &repo,
                    &run_repo,
                    &last_ref,
                    &eff,
                    &mut ledger,
                    &test_override,
                    &setup_override,
                )
                .await?
                {
                    // Orchestrate is `integrate: none` in v1 — the stage HEAD is its
                    // entry HEAD (§12.3), so `line` is always `None` and the fork
                    // source is unchanged. Handled uniformly with the parallel arm.
                    StageFlow::Advance { line } => {
                        if let Some((head_ref, exec_id)) = line {
                            last_ref = head_ref;
                            last_exec_id = Some(exec_id);
                        }
                    }
                    StageFlow::Stop => return Ok(()),
                }
            }
        }

        index += 1;
        cursor.index = index as i64;
        set_cursor(&ctx.db.lock(), run_id, &cursor);
    }

    // Every step done → finalize + mark done.
    finalize_run(ctx, run_id, &run, &spec, &run_repo, last_exec_id.as_deref()).await?;
    let conn = ctx.db.lock();
    journal_event(
        &conn,
        ctx.app.as_ref(),
        run_id,
        event_type::RUN_DONE,
        None,
        &json!({}),
    );
    set_status(&conn, ctx.app.as_ref(), run_id, "done", None, None);
    Ok(())
}

/// Boundary-commit the step's work, pin it, and ferry it into the run repo.
/// Returns the resulting commit SHA. Journals `boundary_commit`.
async fn ferry_step(
    ctx: &RunCtx,
    run_id: &str,
    exec_id: &str,
    message: &str,
    worktree: &Path,
    run_repo: &Path,
) -> Result<String> {
    ferry_committed(
        &ctx.db,
        ctx.app.as_ref(),
        run_id,
        exec_id,
        message,
        worktree,
        run_repo,
    )
    .await
}

/// Boundary-commit + pin + ferry, taking the raw db/app handles so it can also be
/// called from a parallel child's own task (which owns those handles rather than
/// a `&RunCtx`). Journals `boundary_commit`.
async fn ferry_committed(
    db: &Db,
    app: Option<&AppHandle>,
    run_id: &str,
    exec_id: &str,
    message: &str,
    worktree: &Path,
    run_repo: &Path,
) -> Result<String> {
    let bc = gitops::boundary_commit(worktree, message).await?;
    {
        let conn = db.lock();
        journal_event(
            &conn,
            app,
            run_id,
            event_type::BOUNDARY_COMMIT,
            Some(exec_id),
            &json!({ "sha": bc.head }),
        );
    }
    let refname = gitops::pin_step_ref(worktree, exec_id).await?;
    gitops::ferry(worktree, run_repo, &refname).await?;
    Ok(bc.head)
}

/// The atomic commit point for a `done` attempt (§10.4). In ONE DB-lock hold,
/// re-check for a late human `wf_ask` and either finalize the attempt `done`
/// (returning `true`) or leave it uncommitted (`false`) so the caller pauses
/// `question`. This is the load-bearing serialization: the comms router inserts
/// an ask under this same connection mutex, and only after confirming the sender
/// exec is still live — so this section and that insert can't interleave. Either
/// the ask is committed before this runs (we observe it and don't finalize), or
/// we finalize `done` first (and the router then rejects the late ask, whose
/// exec is no longer live). No ordering can both queue an ask and advance the
/// run — closing the window the mailbox drain alone leaves open during `ferry`.
fn commit_done_unless_ask(conn: &Connection, exec_id: &str, head: &str) -> bool {
    if super::comms::has_unanswered_ask(conn, exec_id) {
        false
    } else {
        finish_step_exec(conn, exec_id, "done", Some(head));
        true
    }
}

/// Pause a run `question` for an outstanding human ask: abandon the current
/// attempt (a fresh one runs on `wf_answer`), stop its agent — a human answer can
/// be a long way off and pausing stops processes (§6.5) — and journal the pause.
/// The cursor is left in place so the resumed attempt re-runs the same step with
/// the answer folded in.
async fn pause_question(ctx: &RunCtx, run_id: &str, exec_id: &str, agent_id: Option<&str>) {
    {
        let conn = ctx.db.lock();
        finish_step_exec(&conn, exec_id, "abandoned", None);
    }
    if let Some(a) = agent_id {
        let _ = ctx.driver.stop(a).await;
    }
    let conn = ctx.db.lock();
    journal_event(
        &conn,
        ctx.app.as_ref(),
        run_id,
        event_type::ATTEMPT_ABANDONED,
        Some(exec_id),
        &json!({ "cause": "question" }),
    );
    journal_event(
        &conn,
        ctx.app.as_ref(),
        run_id,
        event_type::RUN_PAUSED,
        Some(exec_id),
        &json!({ "reason": "question" }),
    );
    set_status(
        &conn,
        ctx.app.as_ref(),
        run_id,
        "paused",
        Some("question"),
        None,
    );
}

async fn finalize_run(
    ctx: &RunCtx,
    run_id: &str,
    run: &RunEssentials,
    spec: &Spec,
    run_repo: &Path,
    last_exec_id: Option<&str>,
) -> Result<()> {
    let Some(fin) = spec.finalize.as_ref() else {
        return Ok(());
    };
    if !fin.push {
        return Ok(());
    }
    let Some(exec_id) = last_exec_id else {
        // Nothing was committed; nothing to push.
        return Ok(());
    };
    let final_ref = gitops::step_ref(exec_id);
    let base = fin.pr_base.clone().unwrap_or_else(|| "main".to_string());
    let title = format!("wf: {}", spec.name);
    let outcome = gitops::finalize(
        run_repo,
        &final_ref,
        &run.branch,
        &base,
        &title,
        "",
        fin.open_pr,
    )
    .await?;
    let conn = ctx.db.lock();
    journal_event(
        &conn,
        ctx.app.as_ref(),
        run_id,
        event_type::FINALIZE_PUSHED,
        None,
        &json!({ "branch": outcome.branch }),
    );
    if let Some(url) = outcome.pr_url {
        journal_event(
            &conn,
            ctx.app.as_ref(),
            run_id,
            event_type::FINALIZE_PR,
            None,
            &json!({ "url": url }),
        );
    } else if let Some(err) = outcome.pr_error {
        journal_event(
            &conn,
            ctx.app.as_ref(),
            run_id,
            event_type::FINALIZE_PR,
            None,
            &json!({ "error": err }),
        );
    }
    Ok(())
}

// ───────────────────────────── helpers ──────────────────────────────────────

/// Reject blocks the engine can't yet execute, up front, so a run fails with a
/// clear cause before doing any work rather than part-way through. S8/S9 execute
/// `step` and `parallel` (both `integrate: none` and `integrate: merge`); loop
/// (S7) executes bodies of plain steps; orchestrate (S11) is not wired yet.
fn ensure_executable(blocks: &[Block]) -> Result<()> {
    for b in blocks {
        match b {
            Block::Step(_) => {}
            Block::Parallel(_) => {}
            Block::Loop(lp) => {
                // S7 executes loop bodies of plain steps; nested parallel/loop/
                // orchestrate inside a body isn't wired yet. spec.rs already
                // validates the `until` step + `max`.
                for b in &lp.body {
                    if !matches!(b, Block::Step(_)) {
                        return Err(Error::Other(
                            "nested non-step blocks inside a loop are not supported yet".into(),
                        ));
                    }
                }
            }
            Block::Orchestrate(o) => {
                // S11 executes orchestrate stages with `integrate: none` (the
                // orchestrator supervises note-producing children). Wiring the
                // orchestrator over S9's code-merge machinery is a follow-up.
                if matches!(o.integrate, Integrate::Merge) {
                    return Err(Error::Other(
                        "orchestrate integrate: merge is not supported yet".into(),
                    ));
                }
            }
        }
    }
    Ok(())
}

// ─────────────────────────── parallel stages (§6.6) ─────────────────────────

/// Whether a completed stage advances the run (`Advance`) or halts it (`Stop` —
/// the stage wrote a terminal `failed`/paused status). A merge stage carries the
/// integrated result as the next block's fork source (`line`); an
/// `integrate: none` stage leaves the line unchanged (`line: None`).
enum StageFlow {
    Advance { line: Option<(String, String)> },
    Stop,
}

/// A parallel child's terminal outcome, from the stage's point of view. Every
/// variant carries the child's own budget ledger so the stage can fold its turn
/// / token spend into the run ledger (§11.2).
enum ChildOutcome {
    /// Gate satisfied. `moved_head` records whether the child committed on its
    /// fork (its code is left there under `integrate: none` — §12.3).
    Success {
        moved_head: bool,
        head: Option<String>,
    },
    /// Errored (autonomous retries exhausted), gate blocked, budget-exceeded, or
    /// an unsupported approval gate — anything that isn't a clean `done`.
    Failure { reason: String },
    /// Superseded before finishing by the stage winding down (a join `any`
    /// winner, or a failed `all` stage).
    Canceled,
}

struct ChildResult {
    step_id: String,
    outcome: ChildOutcome,
    ledger: Ledger,
}

/// Everything one parallel child owns to drive itself on its own task (child
/// tasks are spawned into a [`JoinSet`], so they must be `'static`).
struct ChildCtx {
    db: Db,
    driver: Arc<dyn AgentDriver>,
    app: Option<AppHandle>,
    base_deadlines: Deadlines,
    eff: EffectiveBudgets,
    run_id: String,
    run_task: String,
    step: Step,
    agent_spec: AgentSpec,
    fork_base: String,
    blackboard: PathBuf,
    repo: PathBuf,
    run_repo: PathBuf,
    block_index: usize,
    block_count: usize,
    /// Project test/setup command overrides (spec §9.4), resolved once for the
    /// run and cloned per child so a `tests`-gated child resolves its command the
    /// same way a linear step does. The child builds its own `SandboxTestRunner`
    /// (honoring its own `tests_timeout_secs`) in [`drive_child`].
    test_override: Option<String>,
    setup_override: Option<String>,
    /// The stage's integration mode (§12.3). `Merge` children boundary-commit,
    /// pin, and ferry their work into the run repo (like a linear step) so the
    /// stage can merge their refs; `None` children leave code on their fork.
    integrate: Integrate,
    /// An orchestrator note folded into the child's *first* prompt (spec §10.2 —
    /// `retry_child` guidance). Threaded directly rather than via a queued
    /// message so the fresh attempt is guaranteed to carry it. `None` for the
    /// common case.
    extra_note: Option<String>,
    /// The launch generation for this child's `step_id` (spec §10.2). `retry_child`
    /// bumps it and spawns a replacement; the stage ignores results from any
    /// superseded (lower-generation) attempt so a stale finish of the cancelled
    /// task can't win the join. Always `0` outside an orchestrate stage.
    generation: u64,
    /// Set by the stage to wind this child down (loser cancellation, §6.6).
    stage_cancel: Arc<AtomicBool>,
}

/// Fold a finished child's budget ledger into the run ledger (§11.2). Each child
/// runs against its own fresh ledger (concurrent children can't share the run's
/// `&mut Ledger`); their spend is summed back here so the next block — and the
/// persisted `spent_json` — reflect the whole stage.
fn fold_child_ledger(run: &mut Ledger, child: &Ledger) {
    run.turns += child.turns;
    run.tokens += child.tokens;
    for (k, v) in &child.steps {
        let e = run.steps.entry(k.clone()).or_default();
        e.turns += v.turns;
        e.tokens += v.tokens;
    }
    for (k, v) in &child.attempts {
        *run.attempts.entry(k.clone()).or_default() += v;
    }
}

/// Run a `parallel` stage with `integrate: none` (§6.6, §12.3): fork every child
/// from the stage-entry ref, run them concurrently (bounded by `max_concurrent`),
/// and join.
///
/// - `all` — every child must reach `done`; the first failure fails the stage
///   (and so the run), winding the rest down.
/// - `any` — the first `done` child wins and the losers are cancelled + archived;
///   the stage fails only when *every* child fails.
///
/// `integrate: none` means the stage HEAD is its entry HEAD: no child commit is
/// merged, so the run line is unchanged (the caller keeps `last_ref`). A child
/// that moved its own HEAD is journaled `integrate_skipped`. Children run against
/// their own budget ledgers, folded into the run ledger at the end (§11.2).
#[allow(clippy::too_many_arguments)]
async fn run_parallel_stage(
    ctx: &RunCtx,
    run_id: &str,
    run: &RunEssentials,
    spec: &Spec,
    par: &Parallel,
    block_index: usize,
    block_count: usize,
    blackboard: &Path,
    repo: &Path,
    run_repo: &Path,
    fork_base: &str,
    eff: &EffectiveBudgets,
    ledger: &mut Ledger,
    test_override: &Option<String>,
    setup_override: &Option<String>,
    cursor: &mut Cursor,
) -> Result<StageFlow> {
    // Resume a merge stage paused mid-integration (§12.3): the children already
    // ran and ferried, so don't re-run them — continue merging / apply the
    // recorded conflict resolution.
    if matches!(par.integrate, Integrate::Merge)
        && cursor
            .merge
            .as_ref()
            .is_some_and(|m| m.block_index == block_index)
    {
        return resume_merge_stage(
            ctx,
            run_id,
            run,
            spec,
            par,
            block_index,
            block_count,
            blackboard,
            repo,
            run_repo,
            eff,
            ledger,
            test_override,
            setup_override,
            cursor,
        )
        .await;
    }

    // Enforcement point: before spawning the stage (§11.2). Pause at the block
    // boundary if the run budget is already spent, spawning nothing.
    if let Some(which) = ledger.exceeded(eff, super::now_ms()) {
        {
            let conn = ctx.db.lock();
            journal_event(
                &conn,
                ctx.app.as_ref(),
                run_id,
                event_type::BUDGET_EXCEEDED,
                None,
                &json!({ "which": which.as_str() }),
            );
        }
        finish_budget_pause(ctx, run_id, None, ledger);
        return Ok(StageFlow::Stop);
    }

    // The stage-entry SHA, resolved in the run repo (a bare base SHA and a
    // ferried `refs/wf/steps/*` both resolve there). Used only to detect whether
    // a child moved HEAD, so failure to resolve degrades to "no movement".
    let stage_entry_sha = crate::git::rev_parse(run_repo, fork_base).await.ok();

    // Resume: a child that already reached `done` in a prior drive is not re-run
    // (§12.3). If join `any` already has a winner, the stage is complete.
    let mut pending: Vec<&Step> = Vec::new();
    let mut prior_success = false;
    for step in &par.steps {
        if child_already_done(&ctx.db.lock(), run_id, &step.id) {
            prior_success = true;
        } else {
            pending.push(step);
        }
    }
    if matches!(par.join, Join::Any) && prior_success {
        // Resume: no live winner to hand down — begin_merge_stage falls back to
        // the earliest-finished done child.
        return finish_stage_success(
            ctx,
            run_id,
            run,
            spec,
            par,
            block_index,
            run_repo,
            fork_base,
            cursor,
            None,
        )
        .await;
    }
    if pending.is_empty() {
        // Every child already `done` (resume). For `any`, no live winner survived
        // the crash → fall back to the earliest-finished done child.
        return finish_stage_success(
            ctx,
            run_id,
            run,
            spec,
            par,
            block_index,
            run_repo,
            fork_base,
            cursor,
            None,
        )
        .await;
    }

    let stage_cancel = Arc::new(AtomicBool::new(false));
    let concurrency = par
        .max_concurrent
        .map(|m| m as usize)
        .unwrap_or(pending.len())
        .max(1);

    // Build an owned context per pending child up front (also validates agent
    // refs — a bad ref fails the run rather than a single child).
    let mut queue: VecDeque<ChildCtx> = VecDeque::with_capacity(pending.len());
    for step in pending {
        let agent_spec = spec.agents.get(&step.agent).ok_or_else(|| {
            Error::Other(format!(
                "parallel step '{}' references unknown agent '{}'",
                step.id, step.agent
            ))
        })?;
        queue.push_back(ChildCtx {
            db: ctx.db.clone(),
            driver: ctx.driver.clone(),
            app: ctx.app.clone(),
            base_deadlines: ctx.deadlines.clone(),
            eff: eff.clone(),
            run_id: run_id.to_string(),
            run_task: run.task.clone(),
            step: step.clone(),
            agent_spec: agent_spec.clone(),
            fork_base: fork_base.to_string(),
            blackboard: blackboard.to_path_buf(),
            repo: repo.to_path_buf(),
            run_repo: run_repo.to_path_buf(),
            block_index,
            block_count,
            test_override: test_override.clone(),
            setup_override: setup_override.clone(),
            integrate: par.integrate,
            extra_note: None,
            generation: 0,
            stage_cancel: stage_cancel.clone(),
        });
    }

    let mut set: JoinSet<ChildResult> = JoinSet::new();
    let launch = |set: &mut JoinSet<ChildResult>, queue: &mut VecDeque<ChildCtx>| {
        if let Some(c) = queue.pop_front() {
            let entry = stage_entry_sha.clone();
            set.spawn(async move { drive_child(c, entry).await });
        }
    };
    for _ in 0..concurrency {
        launch(&mut set, &mut queue);
    }

    let mut successes = 0usize;
    let mut failures: Vec<String> = Vec::new();
    let mut stage_failed: Option<String> = None;
    // The authoritative join `any` winner: the FIRST child whose success satisfied
    // the join (and cancelled the rest). Captured here rather than reconstructed
    // from timestamps so a same-millisecond raced sibling can't be mistaken for it.
    let mut any_winner: Option<String> = None;

    while let Some(joined) = set.join_next().await {
        let res = match joined {
            Ok(r) => r,
            Err(e) => {
                // A child task panicked — contain it as a child failure (§6.1).
                let reason = format!("child task error: {e}");
                if matches!(par.join, Join::All) && stage_failed.is_none() {
                    stage_failed = Some(reason.clone());
                    stage_cancel.store(true, Ordering::SeqCst);
                }
                failures.push(reason);
                continue;
            }
        };
        // Fold the child's spend into the run ledger regardless of outcome.
        fold_child_ledger(ledger, &res.ledger);
        match res.outcome {
            ChildOutcome::Success { moved_head, head } => {
                successes += 1;
                if moved_head {
                    let conn = ctx.db.lock();
                    journal_event(
                        &conn,
                        ctx.app.as_ref(),
                        run_id,
                        event_type::INTEGRATE_SKIPPED,
                        None,
                        &json!({ "step_id": res.step_id, "sha": head }),
                    );
                }
                // join `any`: first winner → record it and wind the losers down
                // (no new starts). A raced sibling that finishes later is not it.
                if matches!(par.join, Join::Any) {
                    if any_winner.is_none() {
                        any_winner = Some(res.step_id.clone());
                    }
                    stage_cancel.store(true, Ordering::SeqCst);
                }
            }
            ChildOutcome::Failure { reason } => {
                // join `all`: the first failure fails the stage.
                if matches!(par.join, Join::All) && stage_failed.is_none() {
                    stage_failed = Some(reason.clone());
                    stage_cancel.store(true, Ordering::SeqCst);
                }
                failures.push(reason);
            }
            ChildOutcome::Canceled => {}
        }
        // Keep the pipeline full unless we're winding the stage down.
        if !stage_cancel.load(Ordering::SeqCst) {
            launch(&mut set, &mut queue);
        }
    }

    // Persist the folded ledger (children's turns/tokens) before returning.
    {
        let conn = ctx.db.lock();
        ledger.checkpoint_wall(super::now_ms());
        persist_spent(&conn, run_id, ledger);
    }

    match par.join {
        Join::All => {
            if let Some(reason) = stage_failed {
                let conn = ctx.db.lock();
                set_status(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    "failed",
                    None,
                    Some(&format!("parallel stage failed: {reason}")),
                );
                Ok(StageFlow::Stop)
            } else {
                // `all` integrates every child, so no winner hint is needed.
                finish_stage_success(
                    ctx,
                    run_id,
                    run,
                    spec,
                    par,
                    block_index,
                    run_repo,
                    fork_base,
                    cursor,
                    None,
                )
                .await
            }
        }
        Join::Any => {
            if successes > 0 {
                // Hand the authoritative winner down so a raced sibling is never
                // integrated in its place.
                finish_stage_success(
                    ctx,
                    run_id,
                    run,
                    spec,
                    par,
                    block_index,
                    run_repo,
                    fork_base,
                    cursor,
                    any_winner.as_deref(),
                )
                .await
            } else {
                let conn = ctx.db.lock();
                set_status(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    "failed",
                    None,
                    Some(&format!(
                        "all parallel children failed: {}",
                        failures.join("; ")
                    )),
                );
                Ok(StageFlow::Stop)
            }
        }
    }
}

/// A stage whose join condition is met: `integrate: none` advances the run with
/// the line unchanged; `integrate: merge` merges the successful children's
/// ferried refs into the stage accumulator (§12.3) and advances onto the result.
#[allow(clippy::too_many_arguments)]
async fn finish_stage_success(
    ctx: &RunCtx,
    run_id: &str,
    run: &RunEssentials,
    spec: &Spec,
    par: &Parallel,
    block_index: usize,
    run_repo: &Path,
    fork_base: &str,
    cursor: &mut Cursor,
    winner: Option<&str>,
) -> Result<StageFlow> {
    match par.integrate {
        Integrate::None => Ok(StageFlow::Advance { line: None }),
        Integrate::Merge => {
            begin_merge_stage(
                ctx,
                run_id,
                run,
                spec,
                par,
                block_index,
                run_repo,
                fork_base,
                cursor,
                winner,
            )
            .await
        }
    }
}

/// Choose which children's ferried refs the stage integrates (§12.3).
/// `all` merges every done child in spec order. `any` merges exactly ONE branch:
/// the `winner` the join loop recorded when live, else — on resume, when no live
/// winner survived — the child that finished first (least `ended_at`, ties broken
/// by spec order via the stable input). `done` is `(step_id, ref, ended_at)` in
/// spec order.
fn pick_winners(
    mut done: Vec<(String, String, i64)>,
    join: Join,
    winner: Option<&str>,
) -> Vec<(String, String)> {
    if !matches!(join, Join::Any) {
        return done.into_iter().map(|(s, r, _)| (s, r)).collect();
    }
    if let Some(w) = winner {
        if let Some((s, r, _)) = done.into_iter().find(|(s, _, _)| s == w) {
            return vec![(s, r)];
        }
        return Vec::new();
    }
    done.sort_by_key(|(_, _, ended)| *ended);
    done.into_iter().take(1).map(|(s, r, _)| (s, r)).collect()
}

/// Begin a fresh merge stage (§12.3): set up an integration worktree at the
/// stage-entry ref and merge each successful child's ferried ref in spec order.
#[allow(clippy::too_many_arguments)]
async fn begin_merge_stage(
    ctx: &RunCtx,
    run_id: &str,
    run: &RunEssentials,
    spec: &Spec,
    par: &Parallel,
    block_index: usize,
    run_repo: &Path,
    fork_base: &str,
    cursor: &mut Cursor,
    winner: Option<&str>,
) -> Result<StageFlow> {
    let run_dir = PathBuf::from(&run.run_dir);
    let int_wt = gitops::integration_worktree_path(&run_dir, block_index);

    // Successful children with their completion time, in spec order.
    let done: Vec<(String, String, i64)> = {
        let conn = ctx.db.lock();
        par.steps
            .iter()
            .filter_map(|s| {
                done_exec_with_ended_at(&conn, run_id, &s.id)
                    .map(|(e, ended)| (s.id.clone(), gitops::step_ref(&e), ended))
            })
            .collect()
    };
    let winners = pick_winners(done, par.join, winner);
    if winners.is_empty() {
        // No child produced a ferried ref — nothing to integrate; leave the line.
        return Ok(StageFlow::Advance { line: None });
    }

    let base = crate::git::rev_parse(run_repo, fork_base).await?;
    gitops::setup_integration_worktree(run_repo, &int_wt, &base).await?;
    {
        let conn = ctx.db.lock();
        journal_event(
            &conn,
            ctx.app.as_ref(),
            run_id,
            event_type::MERGE_STARTED,
            None,
            &json!({ "count": winners.len() }),
        );
    }
    drive_merges(
        ctx,
        run_id,
        &spec.name,
        block_index,
        run_repo,
        &int_wt,
        winners,
        cursor,
    )
    .await
}

/// Merge each remaining `(step_id, ref)` into the integration worktree in order.
/// A clean merge pins the accumulator and journals `merge_done`; a conflict pins
/// the snapshot, persists the resumable state, journals `merge_conflict`, and
/// pauses the run `conflict`. All merged → finalize the stage.
#[allow(clippy::too_many_arguments)]
async fn drive_merges(
    ctx: &RunCtx,
    run_id: &str,
    spec_name: &str,
    block_index: usize,
    run_repo: &Path,
    int_wt: &Path,
    remaining: Vec<(String, String)>,
    cursor: &mut Cursor,
) -> Result<StageFlow> {
    for (i, (step_id, child_ref)) in remaining.iter().enumerate() {
        let msg = format!("wf({spec_name}): merge {step_id}");
        match gitops::merge_child(int_wt, child_ref, &msg).await? {
            gitops::MergeResult::Clean { head } => {
                gitops::pin_ref(int_wt, &gitops::merge_acc_ref(block_index)).await?;
                let conn = ctx.db.lock();
                journal_event(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    event_type::MERGE_DONE,
                    None,
                    &json!({ "step_id": step_id, "sha": head }),
                );
            }
            gitops::MergeResult::Conflict { files, .. } => {
                gitops::pin_ref(int_wt, &gitops::merge_conflict_ref(block_index)).await?;
                let remaining_after: Vec<(String, String)> = remaining[i + 1..].to_vec();
                cursor.merge = Some(MergeCursor {
                    block_index,
                    remaining: remaining_after,
                    conflict: Some(ConflictInfo {
                        step_id: step_id.clone(),
                        files: files.clone(),
                        conflict_ref: gitops::merge_conflict_ref(block_index),
                        resolution: None,
                    }),
                });
                let conn = ctx.db.lock();
                set_cursor(&conn, run_id, cursor);
                journal_event(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    event_type::MERGE_CONFLICT,
                    None,
                    &json!({
                        "step_id": step_id,
                        "files": files,
                        "worktree": int_wt.to_string_lossy(),
                    }),
                );
                journal_event(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    event_type::RUN_PAUSED,
                    None,
                    &json!({ "reason": "conflict", "files": files }),
                );
                set_status(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    "paused",
                    Some("conflict"),
                    None,
                );
                return Ok(StageFlow::Stop);
            }
        }
    }
    finalize_merge_stage(ctx, run_id, block_index, run_repo, int_wt, cursor).await
}

/// All children merged cleanly: record the integrated result as a synthetic
/// `__merge_<i>` step exec (the next block's fork source, §12.3), tear down the
/// integration worktree, and advance the run onto it.
async fn finalize_merge_stage(
    ctx: &RunCtx,
    run_id: &str,
    block_index: usize,
    run_repo: &Path,
    int_wt: &Path,
    cursor: &mut Cursor,
) -> Result<StageFlow> {
    let head = crate::git::rev_parse(int_wt, "HEAD").await?;
    let exec_id = format!("exec-{}", uuid::Uuid::new_v4());
    {
        let conn = ctx.db.lock();
        let now = super::now_ms();
        let _ = conn.execute(
            "INSERT INTO wf_step_exec
               (id, run_id, step_id, attempt, iteration, status, gate_mode, head_end, started_at, ended_at)
             VALUES (?1, ?2, ?3, 1, 0, 'done', 'merge', ?4, ?5, ?5)",
            rusqlite::params![exec_id, run_id, merge_step_id(block_index), head, now],
        );
    }
    // Pin the integrated result as the stage's step ref, then tear the worktree
    // down (its state is durable in the run repo now). Order matters: the run
    // repo owns the ref + objects, so removing the linked worktree is safe.
    gitops::pin_ref(int_wt, &gitops::step_ref(&exec_id)).await?;
    gitops::remove_integration_worktree(run_repo, int_wt).await;
    cursor.merge = None;
    set_cursor(&ctx.db.lock(), run_id, cursor);
    Ok(StageFlow::Advance {
        line: Some((gitops::step_ref(&exec_id), exec_id)),
    })
}

/// Resume a merge stage paused mid-integration (§12.3). If a conflict is recorded
/// with a chosen resolution, apply it — mode (a) drives a conflict-resolution
/// step forked from the pinned snapshot (gate `commit`); mode (c) reads the human
/// resolution the user committed in the integration worktree — then reset the
/// accumulator onto the resolved commit and merge the remaining children.
#[allow(clippy::too_many_arguments)]
async fn resume_merge_stage(
    ctx: &RunCtx,
    run_id: &str,
    run: &RunEssentials,
    spec: &Spec,
    par: &Parallel,
    block_index: usize,
    block_count: usize,
    blackboard: &Path,
    repo: &Path,
    run_repo: &Path,
    eff: &EffectiveBudgets,
    ledger: &mut Ledger,
    test_override: &Option<String>,
    setup_override: &Option<String>,
    cursor: &mut Cursor,
) -> Result<StageFlow> {
    let ms = cursor
        .merge
        .clone()
        .ok_or_else(|| Error::Other("resume_merge_stage without merge cursor".into()))?;
    let run_dir = PathBuf::from(&run.run_dir);
    let int_wt = gitops::integration_worktree_path(&run_dir, block_index);

    // No recorded conflict → a prior drive was interrupted mid-clean-merge; just
    // continue with whatever remains.
    let Some(ci) = ms.conflict.clone() else {
        return drive_merges(
            ctx,
            run_id,
            &spec.name,
            block_index,
            run_repo,
            &int_wt,
            ms.remaining,
            cursor,
        )
        .await;
    };

    // A conflict awaiting the user's choice must not be silently re-driven; only
    // `wf_resolve_conflict` (which sets `resolution`) may advance it. Re-pause.
    let Some(resolution) = ci.resolution.clone() else {
        let conn = ctx.db.lock();
        set_status(
            &conn,
            ctx.app.as_ref(),
            run_id,
            "paused",
            Some("conflict"),
            None,
        );
        return Ok(StageFlow::Stop);
    };

    let new_acc: String = match resolution.as_str() {
        // (c) The human resolved and committed in the integration worktree. Guard
        // it three ways before continuing: (1) HEAD must have moved off the
        // conflict snapshot and (2) the tree must be clean — else continuing would
        // reset their work away; (3) none of the recorded conflicted files may
        // still hold conflict markers — else the merge would finish with markers in
        // the integrated result. On any failure, clear the choice and re-pause
        // `conflict` with a precise cause so they can fix it and retry.
        "human" => {
            let head = crate::git::rev_parse(&int_wt, "HEAD").await?;
            let snapshot = crate::git::rev_parse(run_repo, &ci.conflict_ref).await.ok();
            let clean = gitops::is_worktree_clean(&int_wt).await.unwrap_or(false);
            let uncommitted = !clean || snapshot.as_deref() == Some(head.as_str());
            let markers =
                gitops::resolution_retains_markers(&int_wt, &ci.conflict_ref, &ci.files).await;
            if uncommitted || markers {
                let detail = if uncommitted {
                    "resolve the conflicts and commit in the integration worktree before continuing"
                } else {
                    "the committed resolution still contains conflict markers — remove them, commit, and continue"
                };
                if let Some(c) = cursor.merge.as_mut().and_then(|m| m.conflict.as_mut()) {
                    c.resolution = None;
                }
                let conn = ctx.db.lock();
                set_cursor(&conn, run_id, cursor);
                journal_event(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    event_type::RUN_PAUSED,
                    None,
                    &json!({ "reason": "conflict", "detail": detail }),
                );
                set_status(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    "paused",
                    Some("conflict"),
                    None,
                );
                return Ok(StageFlow::Stop);
            }
            head
        }
        // (a) Spawn a conflict-resolution step forked from the pinned snapshot.
        "agent" => {
            let agent_alias = par
                .steps
                .iter()
                .find(|s| s.id == ci.step_id)
                .map(|s| s.agent.clone())
                .ok_or_else(|| {
                    Error::Other(format!("conflict step '{}' not in the stage", ci.step_id))
                })?;
            let agent_spec = spec.agents.get(&agent_alias).ok_or_else(|| {
                Error::Other(format!(
                    "conflict resolver references unknown agent '{agent_alias}'"
                ))
            })?;
            let resolve_step = Step {
                id: format!("__resolve_{block_index}"),
                agent: agent_alias,
                goal: format!(
                    "A merge of parallel work produced conflicts in: {}. \
                     Open each file, remove every conflict marker \
                     (<<<<<<<, =======, >>>>>>>), reconcile both sides so the \
                     result is coherent, and commit the resolution.",
                    ci.files.join(", ")
                ),
                gate: Gate::Commit,
                budgets: None,
                comms: vec![],
            };
            let env = StepEnv {
                repo,
                run_repo,
                blackboard,
                eff,
                test_override,
                setup_override,
                run_task: &run.task,
                spec_name: &spec.name,
            };
            let position = Position {
                step_index: block_index,
                step_count: block_count,
                iteration: None,
            };
            match execute_step(
                ctx,
                run_id,
                &env,
                &resolve_step,
                agent_spec,
                position,
                0,
                &ci.conflict_ref,
                false,
                ledger,
            )
            .await?
            {
                StepFlow::Done { head_ref, .. } => {
                    crate::git::rev_parse(run_repo, &head_ref).await?
                }
                // The resolution step paused/failed (blocked gate, budget, error);
                // its status is written. `wf_retry` re-enters this path.
                StepFlow::Halt => return Ok(StageFlow::Stop),
                StepFlow::LoopContinue => unreachable!("resolution step is never a loop until"),
            }
        }
        other => {
            return Err(Error::Other(format!(
                "unknown conflict resolution mode '{other}'"
            )))
        }
    };

    // Reset the accumulator onto the resolved commit and continue merging.
    gitops::setup_integration_worktree(run_repo, &int_wt, &new_acc).await?;
    gitops::pin_ref(&int_wt, &gitops::merge_acc_ref(block_index)).await?;
    cursor.merge = Some(MergeCursor {
        block_index,
        remaining: ms.remaining.clone(),
        conflict: None,
    });
    set_cursor(&ctx.db.lock(), run_id, cursor);
    drive_merges(
        ctx,
        run_id,
        &spec.name,
        block_index,
        run_repo,
        &int_wt,
        ms.remaining,
        cursor,
    )
    .await
}

/// Drive one parallel child to a terminal [`ChildOutcome`] on its own task
/// (§6.6). Mirrors the linear attempt loop (spawn → turn → gate, autonomous
/// retries on error) but: it never ferries (`integrate: none`), classifies the
/// result as success/failure for the join, runs against its own budget ledger,
/// and honours `stage_cancel` — a loser's `run_attempt` stops its own agent and
/// returns `Canceled`, so no agent is ever left running outside the workflow.
async fn drive_child(c: ChildCtx, stage_entry_sha: Option<String>) -> ChildResult {
    let step_eff = c.eff.for_step(c.step.budgets.as_ref());
    let deadlines = deadlines_from(&c.base_deadlines, &step_eff);
    let max_attempts = step_eff.max_attempts;
    let mut child_ledger = Ledger::default();

    let done = |outcome: ChildOutcome, ledger: Ledger| ChildResult {
        step_id: c.step.id.clone(),
        outcome,
        ledger,
    };

    // Tests-gate runner for this child, honoring its own `tests_timeout_secs`
    // and the run's project overrides (spec §9.4) — parity with the linear path.
    // Only a `tests`-gated child consults it; construction fails only if HOME is
    // unavailable, which fails the child rather than silently skipping the gate.
    let test_runner = match super::tests_gate::SandboxTestRunner::new(
        c.test_override.clone(),
        c.setup_override.clone(),
        step_eff.tests_timeout_secs.max(1) as u64,
    ) {
        Ok(r) => r,
        Err(e) => {
            return done(
                ChildOutcome::Failure {
                    reason: format!("could not initialize the tests runner: {e}"),
                },
                child_ledger,
            )
        }
    };

    let mut attempt_no = next_attempt_no(&c.db.lock(), &c.run_id, &c.step.id, 0);
    let mut last_failure: Option<String> = None;

    loop {
        if c.stage_cancel.load(Ordering::SeqCst) {
            return done(ChildOutcome::Canceled, child_ledger);
        }

        let exec_id = format!("exec-{}", uuid::Uuid::new_v4());
        {
            let conn = c.db.lock();
            create_step_exec(
                &conn,
                &exec_id,
                &c.run_id,
                &c.step.id,
                attempt_no,
                0,
                gate_mode(&c.step.gate),
            );
        }

        let prompt = {
            let ctx_prompt = StepPromptCtx {
                run_task: &c.run_task,
                step_id: &c.step.id,
                step_goal: &c.step.goal,
                position: Position {
                    step_index: c.block_index,
                    step_count: c.block_count,
                    iteration: None,
                },
                gate: &c.step.gate,
                turns_per_attempt: c.step.budgets.as_ref().and_then(|b| b.turns_per_attempt),
                comms: &c.step.comms,
            };
            match &last_failure {
                Some(f) => prompts::retry_prompt(f, &ctx_prompt),
                None => prompts::step_prompt(&ctx_prompt),
            }
        };

        let params = AttemptParams {
            spawn_req: build_spawn_req(
                &c.agent_spec,
                &c.fork_base,
                &c.repo,
                &c.run_repo,
                &c.run_id,
            ),
            pre_spawned: None,
            blackboard: c.blackboard.clone(),
            exec_id: exec_id.clone(),
            step_id: c.step.id.clone(),
            attempt: attempt_no as u32,
            iteration: 0,
            gate: c.step.gate.clone(),
            prompt,
            deadlines: deadlines.clone(),
            reprompt_on_block: true,
            cancel: c.stage_cancel.clone(),
            // Parallel children have no human-ask deferral wired yet (comms is a
            // linear-run concern in S10); a never-set flag preserves existing
            // behavior until orchestrator routing lands (S11).
            pending_ask: Arc::new(AtomicBool::new(false)),
        };

        let started = super::now_ms();
        let result = attempt::run_attempt(
            c.driver.as_ref(),
            &test_runner,
            params,
            &mut child_ledger,
            &step_eff,
        )
        .await;

        // Journal the attempt's events + stamp its agent id (linear-path parity).
        {
            let conn = c.db.lock();
            if let Some(agent_id) = &result.agent_id {
                let _ = conn.execute(
                    "UPDATE wf_step_exec SET agent_id = ?1, started_at = ?2 WHERE id = ?3",
                    rusqlite::params![agent_id, started, exec_id],
                );
            }
            for e in &result.events {
                journal_event(
                    &conn,
                    c.app.as_ref(),
                    &c.run_id,
                    e.event_type,
                    Some(&exec_id),
                    &e.payload,
                );
            }
        }

        match result.outcome {
            AttemptOutcome::Done { .. } => {
                let wt = match &result.worktree {
                    Some(wt) => wt.clone(),
                    None => {
                        return done(
                            ChildOutcome::Failure {
                                reason: "done child without a worktree".into(),
                            },
                            child_ledger,
                        )
                    }
                };
                match c.integrate {
                    // `merge` — boundary-commit + pin + ferry into the run repo so
                    // the stage can merge the child's ref (§12.3). A ferry failure
                    // keeps the child out of `done` → drops to the retry policy.
                    Integrate::Merge => {
                        let msg = format!("wf: parallel child {}", c.step.id);
                        match ferry_committed(
                            &c.db,
                            c.app.as_ref(),
                            &c.run_id,
                            &exec_id,
                            &msg,
                            &wt,
                            &c.run_repo,
                        )
                        .await
                        {
                            Ok(head) => {
                                {
                                    let conn = c.db.lock();
                                    finish_step_exec(&conn, &exec_id, "done", Some(&head));
                                }
                                if let Some(agent_id) = &result.agent_id {
                                    let _ = c.driver.archive(agent_id).await;
                                }
                                return done(
                                    ChildOutcome::Success {
                                        moved_head: true,
                                        head: Some(head),
                                    },
                                    child_ledger,
                                );
                            }
                            Err(e) => {
                                last_failure = Some(format!("ferry failed: {e}"));
                                if let Some(agent_id) = &result.agent_id {
                                    let _ = c.driver.stop(agent_id).await;
                                }
                                {
                                    let conn = c.db.lock();
                                    finish_step_exec(&conn, &exec_id, "error", None);
                                }
                                if attempt_no >= max_attempts {
                                    return done(
                                        ChildOutcome::Failure {
                                            reason: format!("ferry failed: {e}"),
                                        },
                                        child_ledger,
                                    );
                                }
                                attempt_no += 1;
                                continue;
                            }
                        }
                    }
                    // `none` — no boundary commit / ferry. Detect whether the child
                    // moved its own HEAD; if so its code is abandoned on the fork →
                    // `integrate_skipped` (journaled by the stage).
                    Integrate::None => {
                        let head = gitops::head_sha(&wt).await.ok();
                        let moved_head = match (&head, &stage_entry_sha) {
                            (Some(h), Some(entry)) => h != entry,
                            _ => false,
                        };
                        {
                            let conn = c.db.lock();
                            finish_step_exec(&conn, &exec_id, "done", head.as_deref());
                        }
                        if let Some(agent_id) = &result.agent_id {
                            let _ = c.driver.archive(agent_id).await;
                        }
                        return done(ChildOutcome::Success { moved_head, head }, child_ledger);
                    }
                }
            }
            AttemptOutcome::Canceled => {
                // A loser: `run_attempt` already stopped the agent (no leak).
                // Abandon the row and archive the chat.
                {
                    let conn = c.db.lock();
                    let _ = conn.execute(
                        "UPDATE wf_step_exec SET status = 'abandoned', ended_at = ?1 WHERE id = ?2",
                        rusqlite::params![super::now_ms(), exec_id],
                    );
                    journal_event(
                        &conn,
                        c.app.as_ref(),
                        &c.run_id,
                        event_type::ATTEMPT_ABANDONED,
                        Some(&exec_id),
                        &json!({ "cause": "canceled" }),
                    );
                }
                if let Some(agent_id) = &result.agent_id {
                    let _ = c.driver.archive(agent_id).await;
                }
                return done(ChildOutcome::Canceled, child_ledger);
            }
            AttemptOutcome::Error { error } => {
                {
                    let conn = c.db.lock();
                    finish_step_exec(&conn, &exec_id, "error", None);
                }
                last_failure = Some(error.clone());
                if attempt_no >= max_attempts {
                    return done(ChildOutcome::Failure { reason: error }, child_ledger);
                }
                attempt_no += 1;
            }
            AttemptOutcome::Blocked { reason } => {
                {
                    let conn = c.db.lock();
                    finish_step_exec(&conn, &exec_id, "blocked", None);
                }
                if let Some(agent_id) = &result.agent_id {
                    let _ = c.driver.stop(agent_id).await;
                    let _ = c.driver.archive(agent_id).await;
                }
                return done(
                    ChildOutcome::Failure {
                        reason: format!("gate unmet: {reason}"),
                    },
                    child_ledger,
                );
            }
            AttemptOutcome::BudgetExceeded { which } => {
                {
                    let conn = c.db.lock();
                    finish_step_exec(&conn, &exec_id, "error", None);
                }
                if let Some(agent_id) = &result.agent_id {
                    let _ = c.driver.stop(agent_id).await;
                    let _ = c.driver.archive(agent_id).await;
                }
                return done(
                    ChildOutcome::Failure {
                        reason: format!("budget_exceeded: {which}"),
                    },
                    child_ledger,
                );
            }
            AttemptOutcome::AwaitingApproval => {
                // Per-child approval pauses aren't modelled in v1 (joins are over
                // success/error, §6.6) — fail the child with a clear cause rather
                // than hang the stage.
                {
                    let conn = c.db.lock();
                    finish_step_exec(&conn, &exec_id, "error", None);
                }
                if let Some(agent_id) = &result.agent_id {
                    let _ = c.driver.archive(agent_id).await;
                }
                return done(
                    ChildOutcome::Failure {
                        reason: "approval gates are not supported inside a parallel stage".into(),
                    },
                    child_ledger,
                );
            }
            AttemptOutcome::AwaitingAnswer => {
                // A parallel child never sets `pending_ask` (human Q&A is a
                // linear-run concern in S10, §10.4), so this is unreachable in
                // practice — fail with a clear cause rather than hang the stage.
                {
                    let conn = c.db.lock();
                    finish_step_exec(&conn, &exec_id, "error", None);
                }
                if let Some(agent_id) = &result.agent_id {
                    let _ = c.driver.archive(agent_id).await;
                }
                return done(
                    ChildOutcome::Failure {
                        reason: "human questions are not supported inside a parallel stage".into(),
                    },
                    child_ledger,
                );
            }
        }
    }
}

/// Assemble a [`SpawnReq`] for a step / parallel-child agent. Resolving the
/// spec's skill / MCP names to snapshots is a documented S4b follow-up; the
/// engine spawns with provider + brief for now (the blackboard write-grant is
/// derived from `owner_run_id` at spawn).
fn build_spawn_req(
    agent_spec: &AgentSpec,
    fork_base: &str,
    repo: &Path,
    run_repo: &Path,
    run_id: &str,
) -> SpawnReq {
    SpawnReq {
        repo_path: repo.to_path_buf(),
        provider: agent_spec.base.clone(),
        model: agent_spec.model.clone(),
        instructions: agent_spec.instructions.clone(),
        custom_agent_id: agent_spec.custom_agent.clone(),
        skills: vec![],
        mcp_servers: vec![],
        fork_base: Some(fork_base.to_string()),
        run_repo: Some(run_repo.to_path_buf()),
        owner_run_id: run_id.to_string(),
    }
}

// ───────────────────────── orchestrate stages (§6.6, §10.2) ─────────────────

/// An orchestrate child's terminal outcome plus the exec id (for lifecycle
/// forwarding) — the orchestrate analogue of [`ChildResult`].
struct OrchChildResult {
    step_id: String,
    exec_id: String,
    /// The launch generation of the attempt that produced this result — the stage
    /// discards it if a later `retry_child` has superseded this generation.
    generation: u64,
    outcome: ChildOutcome,
    ledger: Ledger,
}

/// The stage-visible status of an orchestrate child, for the join decision.
#[derive(Clone)]
enum ChildStatus {
    Success,
    Failure(String),
    /// The orchestrator dropped it with `skip_child` — satisfied, not a failure.
    Skipped,
}

/// Result of waiting for the orchestrator to answer a child's ask (§10.4).
enum AnswerWait {
    Answered,
    Timeout,
    Canceled,
}

/// Outcome of driving one orchestrator turn (§10.2), mapping the turn result onto
/// the stage's control flow.
enum OrchStepResult {
    Ok,
    Stalled,
    Error(String),
    Budget(String),
}

/// Run an orchestrate stage (spec §6.6, §10.2): a stage-lived orchestrator agent
/// supervises children with `integrate: none`. Static `body` children auto-start;
/// the orchestrator may spawn dynamic children (bounded by `children.max`), answer
/// their questions, notify them, and end the stage. The stage completes when the
/// join over all children is met **and** the orchestrator writes its concluding
/// `verdict.json`; `stage_done` ends it early. A stalled orchestrator escalates to
/// the human (§10.2). Children run against their own ledgers, folded into the run
/// ledger (§11.2).
#[allow(clippy::too_many_arguments)]
async fn run_orchestrate_stage(
    ctx: &RunCtx,
    run_id: &str,
    run: &RunEssentials,
    spec: &Spec,
    orch: &Orchestrate,
    block_index: usize,
    block_count: usize,
    blackboard: &Path,
    repo: &Path,
    run_repo: &Path,
    fork_base: &str,
    eff: &EffectiveBudgets,
    ledger: &mut Ledger,
    test_override: &Option<String>,
    setup_override: &Option<String>,
) -> Result<StageFlow> {
    // Enforcement point: before spawning the stage (§11.2).
    if let Some(which) = ledger.exceeded(eff, super::now_ms()) {
        {
            let conn = ctx.db.lock();
            journal_event(
                &conn,
                ctx.app.as_ref(),
                run_id,
                event_type::BUDGET_EXCEEDED,
                None,
                &json!({ "which": which.as_str() }),
            );
        }
        finish_budget_pause(ctx, run_id, None, ledger);
        return Ok(StageFlow::Stop);
    }

    let orch_step_id = super::comms::orch_step_id(block_index);
    // Resume: the orchestrator already concluded in a prior drive → stage done.
    if child_already_done(&ctx.db.lock(), run_id, &orch_step_id) {
        return Ok(StageFlow::Advance { line: None });
    }

    let stage_entry_sha = crate::git::rev_parse(run_repo, fork_base).await.ok();
    let orch_step_eff = eff.for_step(None);
    let orch_deadlines = deadlines_from(&ctx.deadlines, &orch_step_eff);

    // ── Spawn the stage-lived orchestrator; stamp its agent id at spawn so its
    //    mid-turn `wf_decide`/`wf_notify` resolve by agent id (§10.2). ──
    let orch_agent = spec.agents.get(&orch.agent).ok_or_else(|| {
        Error::Other(format!(
            "orchestrate references unknown agent '{}'",
            orch.agent
        ))
    })?;
    let orch_exec = format!("exec-{}", uuid::Uuid::new_v4());
    {
        let conn = ctx.db.lock();
        create_step_exec(&conn, &orch_exec, run_id, &orch_step_id, 1, 0, "verdict");
    }
    let spawned = match ctx
        .driver
        .spawn(build_spawn_req(
            orch_agent, fork_base, repo, run_repo, run_id,
        ))
        .await
    {
        Ok(s) => s,
        Err(e) => {
            let conn = ctx.db.lock();
            finish_step_exec(&conn, &orch_exec, "error", None);
            set_status(
                &conn,
                ctx.app.as_ref(),
                run_id,
                "failed",
                None,
                Some(&format!("orchestrator spawn failed: {e}")),
            );
            return Ok(StageFlow::Stop);
        }
    };
    let orch_agent_id = spawned.agent_id.clone();
    {
        let conn = ctx.db.lock();
        let _ = conn.execute(
            "UPDATE wf_step_exec SET agent_id = ?1, status = 'running', started_at = ?2 WHERE id = ?3",
            rusqlite::params![orch_agent_id, super::now_ms(), orch_exec],
        );
        journal_event(
            &conn,
            ctx.app.as_ref(),
            run_id,
            event_type::ATTEMPT_SPAWNED,
            Some(&orch_exec),
            &json!({ "agent_id": orch_agent_id, "fork_base": fork_base, "role": "orchestrator" }),
        );
    }
    if let Err(e) = attempt::await_agent_ready(
        ctx.driver.as_ref(),
        &orch_agent_id,
        orch_deadlines.spawn_timeout,
    )
    .await
    {
        let conn = ctx.db.lock();
        finish_step_exec(&conn, &orch_exec, "error", None);
        set_status(
            &conn,
            ctx.app.as_ref(),
            run_id,
            "failed",
            None,
            Some(&format!("orchestrator not ready: {e}")),
        );
        return Ok(StageFlow::Stop);
    }
    {
        let conn = ctx.db.lock();
        journal_event(
            &conn,
            ctx.app.as_ref(),
            run_id,
            event_type::ATTEMPT_READY,
            Some(&orch_exec),
            &json!({ "role": "orchestrator" }),
        );
    }

    // ── Launch static body children, then drive the orchestrator's first turn. ──
    let mut set: JoinSet<OrchChildResult> = JoinSet::new();
    // Per-child cancel flags (keyed by step id) so `skip_child`/`retry_child` can
    // wind down one child, and a join-`any` winner or a stage teardown can wind
    // down all of them (§10.2, §6.6).
    let mut child_cancels: HashMap<String, Arc<AtomicBool>> = HashMap::new();
    // Per-step launch generation. `retry_child` bumps it and spawns a replacement;
    // `handle_orch_child` discards any result whose generation is stale, so a
    // cancelled old attempt that still finishes can't win the join (§10.2).
    let mut child_gen: HashMap<String, u64> = HashMap::new();
    // Seed the dynamic-child index from the DB so a resumed stage doesn't reuse an
    // id an earlier drive already created (ids stay unique across resume).
    let mut dyn_count = existing_dyn_child_count(&ctx.db.lock(), run_id, &orch_step_id);
    let mut outcomes: HashMap<String, ChildStatus> = HashMap::new();

    for step in &orch.body {
        // Resume: a static child that already finished is not re-run — its work is
        // durable (parity with the parallel stage, §12.3).
        if child_already_done(&ctx.db.lock(), run_id, &step.id) {
            outcomes.insert(step.id.clone(), ChildStatus::Success);
            continue;
        }
        let agent_spec = spec.agents.get(&step.agent).ok_or_else(|| {
            Error::Other(format!(
                "orchestrate child '{}' references unknown agent '{}'",
                step.id, step.agent
            ))
        })?;
        let cancel = Arc::new(AtomicBool::new(false));
        child_cancels.insert(step.id.clone(), cancel.clone());
        child_gen.insert(step.id.clone(), 0);
        let c = build_orch_child_ctx(
            ctx,
            run_id,
            run,
            agent_spec.clone(),
            step.clone(),
            block_index,
            block_count,
            blackboard,
            repo,
            run_repo,
            fork_base,
            eff,
            test_override,
            setup_override,
            None,
            0,
            cancel,
        );
        let entry = stage_entry_sha.clone();
        set.spawn(async move { drive_orch_child(c, entry).await });
    }

    let init_prompt = {
        let static_children: Vec<String> = orch.body.iter().map(|s| s.id.clone()).collect();
        let dynamic = orch.children.as_ref().map(|c| (c.agent.as_str(), c.max));
        let base = prompts::orchestrator_prompt(&prompts::OrchestratorPromptCtx {
            run_task: &run.task,
            orch_step_id: &orch_step_id,
            goal: &orch.goal,
            position: Position {
                step_index: block_index,
                step_count: block_count,
                iteration: None,
            },
            static_children: &static_children,
            dynamic,
        });
        // On a resume after escalation, an answer for the orchestrator is folded in.
        let delivered = {
            let conn = ctx.db.lock();
            super::comms::take_pending_deliveries(&conn, run_id, &orch_step_id)
        };
        if delivered.is_empty() {
            base
        } else {
            format!("{}\n\n{}", super::comms::compose_delivery(&delivered), base)
        }
    };

    match drive_orch_turn(
        ctx,
        run_id,
        &orch_exec,
        &orch_step_id,
        &orch_agent_id,
        init_prompt,
        "step",
        &orch_deadlines,
        ledger,
        eff,
    )
    .await
    {
        OrchStepResult::Ok => {}
        other => {
            return finish_orch_turn_failure(
                ctx,
                run_id,
                &orch_exec,
                &orch_agent_id,
                &child_cancels,
                &mut set,
                ledger,
                other,
            )
            .await;
        }
    }

    // ── The stage event loop (§6.6): alternate driving the orchestrator with
    //    reaping children until the join is met and it concludes. ──
    let mut concluded_early = false;
    let mut conclude_sent = false;
    let mut conclude_tries = 0u32;
    const MAX_CONCLUDE_TRIES: u32 = 2;

    loop {
        if ctx.cancel.load(Ordering::SeqCst) {
            drain_children(&child_cancels, &mut set).await;
            cancel_run(ctx, run_id).await;
            return Ok(StageFlow::Stop);
        }

        // Orchestrator escalated or asked the human on its last turn → pause
        // `question` (§10.2, §10.4). The backstop mirrors the linear path.
        if super::comms::has_unanswered_ask(&ctx.db.lock(), &orch_exec) {
            drain_children(&child_cancels, &mut set).await;
            pause_question(ctx, run_id, &orch_exec, Some(&orch_agent_id)).await;
            return Ok(StageFlow::Stop);
        }

        // Execute the decisions the orchestrator issued last turn (§10.2).
        let decisions = {
            let conn = ctx.db.lock();
            super::comms::take_orchestrator_decisions(&conn, run_id, &orch_exec)
        };
        for d in decisions {
            match d {
                super::comms::Decision::StageDone => concluded_early = true,
                super::comms::Decision::SpawnChild { agent, goal } => {
                    if let Some(agent_spec) = spec.agents.get(&agent).cloned() {
                        let step = Step {
                            id: format!("{orch_step_id}::dyn-{dyn_count}"),
                            agent: agent.clone(),
                            goal,
                            gate: Gate::Verdict,
                            budgets: None,
                            comms: orch.comms.clone(),
                        };
                        dyn_count += 1;
                        let cancel = Arc::new(AtomicBool::new(false));
                        child_cancels.insert(step.id.clone(), cancel.clone());
                        child_gen.insert(step.id.clone(), 0);
                        let c = build_orch_child_ctx(
                            ctx,
                            run_id,
                            run,
                            agent_spec,
                            step,
                            block_index,
                            block_count,
                            blackboard,
                            repo,
                            run_repo,
                            fork_base,
                            eff,
                            test_override,
                            setup_override,
                            None,
                            0,
                            cancel,
                        );
                        let entry = stage_entry_sha.clone();
                        set.spawn(async move { drive_orch_child(c, entry).await });
                    }
                }
                super::comms::Decision::SkipChild { step_id, .. } => {
                    // Cancel the child's live task so it stops spending budget and
                    // the stage can conclude without waiting on it, then record it
                    // as satisfied for the join (§10.2).
                    if let Some(flag) = child_cancels.get(&step_id) {
                        flag.store(true, Ordering::SeqCst);
                    }
                    outcomes.insert(step_id, ChildStatus::Skipped);
                }
                super::comms::Decision::RetryChild { step_id, guidance } => {
                    if let Some(orig) = orch.body.iter().find(|s| s.id == step_id).cloned() {
                        if let Some(agent_spec) = spec.agents.get(&orig.agent).cloned() {
                            // Cancel the previous attempt's task so it can't race the
                            // retry into the join outcome (§10.2).
                            if let Some(flag) = child_cancels.get(&orig.id) {
                                flag.store(true, Ordering::SeqCst);
                            }
                            outcomes.remove(&orig.id);
                            let cancel = Arc::new(AtomicBool::new(false));
                            child_cancels.insert(orig.id.clone(), cancel.clone());
                            // Bump the generation so the cancelled attempt's result
                            // (if it still finishes) is discarded by the join (§10.2).
                            let gen = child_gen.get(&orig.id).copied().unwrap_or(0) + 1;
                            child_gen.insert(orig.id.clone(), gen);
                            let step = orig.clone();
                            // Thread the guidance straight into the replacement
                            // child's first prompt (not a queued note) so the fresh
                            // attempt is guaranteed to carry it (spec §10.2).
                            let note = (!guidance.trim().is_empty()).then(|| {
                                format!(
                                    "The orchestrator asked you to try this step again \
                                     with this guidance:\n\n{guidance}"
                                )
                            });
                            let c = build_orch_child_ctx(
                                ctx,
                                run_id,
                                run,
                                agent_spec,
                                step,
                                block_index,
                                block_count,
                                blackboard,
                                repo,
                                run_repo,
                                fork_base,
                                eff,
                                test_override,
                                setup_override,
                                note,
                                gen,
                                cancel,
                            );
                            let entry = stage_entry_sha.clone();
                            set.spawn(async move { drive_orch_child(c, entry).await });
                        }
                    }
                }
            }
        }
        if concluded_early {
            break;
        }

        // Reap finished children without blocking (§11.2 fold + §10.1 forward).
        while let Some(joined) = set.try_join_next() {
            handle_orch_child(
                ctx,
                run_id,
                &orch_exec,
                orch.join,
                ledger,
                joined,
                &mut outcomes,
                &child_cancels,
                &child_gen,
            );
        }

        // join `all`: the first child failure fails the stage.
        if matches!(orch.join, Join::All) {
            if let Some(reason) = outcomes.values().find_map(|s| match s {
                ChildStatus::Failure(r) => Some(r.clone()),
                _ => None,
            }) {
                drain_children(&child_cancels, &mut set).await;
                let _ = ctx.driver.stop(&orch_agent_id).await;
                let conn = ctx.db.lock();
                finish_step_exec(&conn, &orch_exec, "error", None);
                persist_spent(&conn, run_id, ledger);
                set_status(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    "failed",
                    None,
                    Some(&format!("orchestrate stage failed: {reason}")),
                );
                return Ok(StageFlow::Stop);
            }
        }

        if set.is_empty() {
            // Every child is terminal. join `any` with no success and ≥1 failure
            // fails the stage; otherwise the orchestrator concludes.
            if matches!(orch.join, Join::Any)
                && !outcomes.values().any(|s| matches!(s, ChildStatus::Success))
                && outcomes
                    .values()
                    .any(|s| matches!(s, ChildStatus::Failure(_)))
            {
                let _ = ctx.driver.stop(&orch_agent_id).await;
                let conn = ctx.db.lock();
                finish_step_exec(&conn, &orch_exec, "error", None);
                persist_spent(&conn, run_id, ledger);
                set_status(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    "failed",
                    None,
                    Some("orchestrate stage failed: all children failed"),
                );
                return Ok(StageFlow::Stop);
            }

            if !conclude_sent {
                let prompt = {
                    let inbox = {
                        let conn = ctx.db.lock();
                        super::comms::take_orchestrator_inbox(&conn, run_id, &orch_exec)
                    };
                    let mut p = if inbox.is_empty() {
                        String::new()
                    } else {
                        format!("{}\n\n", super::comms::compose_orchestrator_inbox(&inbox))
                    };
                    p.push_str(&prompts::orchestrator_conclude_prompt(&orch_step_id));
                    p
                };
                conclude_sent = true;
                conclude_tries += 1;
                match drive_orch_turn(
                    ctx,
                    run_id,
                    &orch_exec,
                    &orch_step_id,
                    &orch_agent_id,
                    prompt,
                    "reprompt",
                    &orch_deadlines,
                    ledger,
                    eff,
                )
                .await
                {
                    OrchStepResult::Ok => continue,
                    other => {
                        return finish_orch_turn_failure(
                            ctx,
                            run_id,
                            &orch_exec,
                            &orch_agent_id,
                            &child_cancels,
                            &mut set,
                            ledger,
                            other,
                        )
                        .await;
                    }
                }
            }

            // Read the orchestrator's concluding verdict — the stage gate (§6.6).
            let step_dir = blackboard::step_dir(blackboard, &orch_step_id)?;
            let verdict = blackboard::read_verdict(&step_dir);
            let done = matches!(
                &verdict,
                Ok(v) if matches!(v.result, super::blackboard::VerdictResult::Done)
            );
            {
                let conn = ctx.db.lock();
                journal_event(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    event_type::GATE_EVALUATED,
                    Some(&orch_exec),
                    &json!({
                        "mode": "verdict",
                        "outcome": if done { "done" } else { "blocked" },
                        "reason": if done { "orchestrator concluded" } else { "orchestrator has not concluded yet" },
                    }),
                );
            }
            if done {
                let _ = ctx.driver.archive(&orch_agent_id).await;
                let conn = ctx.db.lock();
                finish_step_exec(&conn, &orch_exec, "done", None);
                persist_spent(&conn, run_id, ledger);
                return Ok(StageFlow::Advance { line: None });
            }
            if conclude_tries < MAX_CONCLUDE_TRIES {
                conclude_tries += 1;
                match drive_orch_turn(
                    ctx,
                    run_id,
                    &orch_exec,
                    &orch_step_id,
                    &orch_agent_id,
                    prompts::orchestrator_conclude_prompt(&orch_step_id),
                    "reprompt",
                    &orch_deadlines,
                    ledger,
                    eff,
                )
                .await
                {
                    OrchStepResult::Ok => continue,
                    other => {
                        return finish_orch_turn_failure(
                            ctx,
                            run_id,
                            &orch_exec,
                            &orch_agent_id,
                            &child_cancels,
                            &mut set,
                            ledger,
                            other,
                        )
                        .await;
                    }
                }
            }
            // Could not conclude → pause `blocked_gate` for a human retry (§6.5).
            let _ = ctx.driver.stop(&orch_agent_id).await;
            let conn = ctx.db.lock();
            finish_step_exec(&conn, &orch_exec, "blocked", None);
            persist_spent(&conn, run_id, ledger);
            journal_event(
                &conn,
                ctx.app.as_ref(),
                run_id,
                event_type::RUN_PAUSED,
                Some(&orch_exec),
                &json!({ "reason": "blocked_gate", "detail": "orchestrator did not conclude the stage" }),
            );
            set_status(
                &conn,
                ctx.app.as_ref(),
                run_id,
                "paused",
                Some("blocked_gate"),
                None,
            );
            return Ok(StageFlow::Stop);
        }

        // Children still running: prompt the orchestrator if it has inbox to act
        // on, else wait for the next child event or a poll tick.
        let inbox = {
            let conn = ctx.db.lock();
            super::comms::take_orchestrator_inbox(&conn, run_id, &orch_exec)
        };
        if !inbox.is_empty() {
            let prompt = super::comms::compose_orchestrator_inbox(&inbox);
            match drive_orch_turn(
                ctx,
                run_id,
                &orch_exec,
                &orch_step_id,
                &orch_agent_id,
                prompt,
                "message",
                &orch_deadlines,
                ledger,
                eff,
            )
            .await
            {
                OrchStepResult::Ok => continue,
                other => {
                    return finish_orch_turn_failure(
                        ctx,
                        run_id,
                        &orch_exec,
                        &orch_agent_id,
                        &child_cancels,
                        &mut set,
                        ledger,
                        other,
                    )
                    .await;
                }
            }
        }

        // Bound the idle wait by the run wall-clock (§11.3 — "no wait without a
        // deadline"): a wedged stage pauses `budget_exceeded` rather than hanging.
        if let Some(which) = ledger.exceeded(eff, super::now_ms()) {
            drain_children(&child_cancels, &mut set).await;
            let _ = ctx.driver.stop(&orch_agent_id).await;
            {
                let conn = ctx.db.lock();
                finish_step_exec(&conn, &orch_exec, "abandoned", None);
                journal_event(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    event_type::BUDGET_EXCEEDED,
                    Some(&orch_exec),
                    &json!({ "which": which.as_str() }),
                );
            }
            finish_budget_pause(ctx, run_id, Some(&orch_exec), ledger);
            return Ok(StageFlow::Stop);
        }
        tokio::select! {
            joined = set.join_next() => {
                if let Some(j) = joined {
                    handle_orch_child(ctx, run_id, &orch_exec, orch.join, ledger, j, &mut outcomes, &child_cancels, &child_gen);
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {}
        }
    }

    // `stage_done`: wind the remaining children down and mark the stage done.
    drain_children(&child_cancels, &mut set).await;
    let _ = ctx.driver.archive(&orch_agent_id).await;
    let conn = ctx.db.lock();
    finish_step_exec(&conn, &orch_exec, "done", None);
    persist_spent(&conn, run_id, ledger);
    Ok(StageFlow::Advance { line: None })
}

/// Assemble one orchestrate child's owned [`ChildCtx`] (all fields cloned so the
/// child task is `'static`).
#[allow(clippy::too_many_arguments)]
fn build_orch_child_ctx(
    ctx: &RunCtx,
    run_id: &str,
    run: &RunEssentials,
    agent_spec: AgentSpec,
    step: Step,
    block_index: usize,
    block_count: usize,
    blackboard: &Path,
    repo: &Path,
    run_repo: &Path,
    fork_base: &str,
    eff: &EffectiveBudgets,
    test_override: &Option<String>,
    setup_override: &Option<String>,
    extra_note: Option<String>,
    generation: u64,
    cancel: Arc<AtomicBool>,
) -> ChildCtx {
    ChildCtx {
        db: ctx.db.clone(),
        driver: ctx.driver.clone(),
        app: ctx.app.clone(),
        base_deadlines: ctx.deadlines.clone(),
        eff: eff.clone(),
        run_id: run_id.to_string(),
        run_task: run.task.clone(),
        step,
        agent_spec,
        fork_base: fork_base.to_string(),
        blackboard: blackboard.to_path_buf(),
        repo: repo.to_path_buf(),
        run_repo: run_repo.to_path_buf(),
        block_index,
        block_count,
        test_override: test_override.clone(),
        setup_override: setup_override.clone(),
        // Orchestrate children are note-producing in v1 (`integrate: none`), so
        // they never ferry — `drive_orch_child` always takes the none path.
        integrate: Integrate::None,
        extra_note,
        generation,
        stage_cancel: cancel,
    }
}

/// Drive one orchestrate child (spec §6.6, §10.4). Like a parallel child it spawns,
/// runs its turn, and gates under `integrate: none` — but it stamps its agent id
/// at spawn (so a mid-turn `wf_ask` resolves to it), and when it raises a question
/// routed to the orchestrator it parks for the answer and re-attempts with it
/// folded in rather than failing.
async fn drive_orch_child(c: ChildCtx, stage_entry_sha: Option<String>) -> OrchChildResult {
    let step_eff = c.eff.for_step(c.step.budgets.as_ref());
    let deadlines = deadlines_from(&c.base_deadlines, &step_eff);
    let max_attempts = step_eff.max_attempts;
    let mut child_ledger = Ledger::default();
    let mut last_exec = String::new();

    let generation = c.generation;
    let done =
        |step_id: String, exec_id: String, outcome: ChildOutcome, ledger: Ledger| OrchChildResult {
            step_id,
            exec_id,
            generation,
            outcome,
            ledger,
        };

    let test_runner = match super::tests_gate::SandboxTestRunner::new(
        c.test_override.clone(),
        c.setup_override.clone(),
        step_eff.tests_timeout_secs.max(1) as u64,
    ) {
        Ok(r) => r,
        Err(e) => {
            return done(
                c.step.id.clone(),
                last_exec,
                ChildOutcome::Failure {
                    reason: format!("could not initialize the tests runner: {e}"),
                },
                child_ledger,
            )
        }
    };

    let mut attempt_no = next_attempt_no(&c.db.lock(), &c.run_id, &c.step.id, 0);
    let mut last_failure: Option<String> = None;
    // Orchestrator guidance (`retry_child`, §10.2) folded into the first prompt
    // only, then consumed so later attempts don't repeat it.
    let mut extra_note = c.extra_note.clone();

    loop {
        if c.stage_cancel.load(Ordering::SeqCst) {
            return done(
                c.step.id.clone(),
                last_exec,
                ChildOutcome::Canceled,
                child_ledger,
            );
        }

        let exec_id = format!("exec-{}", uuid::Uuid::new_v4());
        last_exec = exec_id.clone();
        {
            let conn = c.db.lock();
            create_step_exec(
                &conn,
                &exec_id,
                &c.run_id,
                &c.step.id,
                attempt_no,
                0,
                gate_mode(&c.step.gate),
            );
        }

        // Spawn and stamp the agent id BEFORE the turn (§10.2).
        let spawned = match c
            .driver
            .spawn(build_spawn_req(
                &c.agent_spec,
                &c.fork_base,
                &c.repo,
                &c.run_repo,
                &c.run_id,
            ))
            .await
        {
            Ok(s) => s,
            Err(e) => {
                let error = format!("spawn failed: {e}");
                {
                    let conn = c.db.lock();
                    finish_step_exec(&conn, &exec_id, "error", None);
                }
                last_failure = Some(error.clone());
                if attempt_no >= max_attempts {
                    return done(
                        c.step.id.clone(),
                        last_exec,
                        ChildOutcome::Failure { reason: error },
                        child_ledger,
                    );
                }
                attempt_no += 1;
                continue;
            }
        };
        let child_agent_id = spawned.agent_id.clone();
        {
            let conn = c.db.lock();
            let _ = conn.execute(
                "UPDATE wf_step_exec SET agent_id = ?1, started_at = ?2 WHERE id = ?3",
                rusqlite::params![child_agent_id, super::now_ms(), exec_id],
            );
        }

        let prompt = {
            let ctx_prompt = StepPromptCtx {
                run_task: &c.run_task,
                step_id: &c.step.id,
                step_goal: &c.step.goal,
                position: Position {
                    step_index: c.block_index,
                    step_count: c.block_count,
                    iteration: None,
                },
                gate: &c.step.gate,
                turns_per_attempt: c.step.budgets.as_ref().and_then(|b| b.turns_per_attempt),
                comms: &c.step.comms,
            };
            let mut base = match &last_failure {
                Some(f) => prompts::retry_prompt(f, &ctx_prompt),
                None => prompts::step_prompt(&ctx_prompt),
            };
            // Orchestrator retry guidance leads the first prompt (§10.2).
            if let Some(note) = extra_note.take() {
                base = format!("{note}\n\n{base}");
            }
            // Fold a delivered answer (to a prior ask) or a notify (§10.4).
            let delivered = {
                let conn = c.db.lock();
                super::comms::take_pending_deliveries(&conn, &c.run_id, &c.step.id)
            };
            if delivered.is_empty() {
                base
            } else {
                format!("{}\n\n{}", super::comms::compose_delivery(&delivered), base)
            }
        };

        let params = AttemptParams {
            spawn_req: build_spawn_req(
                &c.agent_spec,
                &c.fork_base,
                &c.repo,
                &c.run_repo,
                &c.run_id,
            ),
            pre_spawned: Some(spawned),
            blackboard: c.blackboard.clone(),
            exec_id: exec_id.clone(),
            step_id: c.step.id.clone(),
            attempt: attempt_no as u32,
            iteration: 0,
            gate: c.step.gate.clone(),
            prompt,
            deadlines: deadlines.clone(),
            reprompt_on_block: true,
            cancel: c.stage_cancel.clone(),
            pending_ask: Arc::new(AtomicBool::new(false)),
        };

        let started = super::now_ms();
        let result = attempt::run_attempt(
            c.driver.as_ref(),
            &test_runner,
            params,
            &mut child_ledger,
            &step_eff,
        )
        .await;
        {
            let conn = c.db.lock();
            let _ = conn.execute(
                "UPDATE wf_step_exec SET started_at = ?1 WHERE id = ?2 AND started_at IS NULL",
                rusqlite::params![started, exec_id],
            );
            for e in &result.events {
                journal_event(
                    &conn,
                    c.app.as_ref(),
                    &c.run_id,
                    e.event_type,
                    Some(&exec_id),
                    &e.payload,
                );
            }
        }

        // Drain the mailbox so a `wf_ask` from this turn is persisted (§10.4).
        c.driver.settle_rpc(&child_agent_id).await;

        // Asked the orchestrator? Park for the answer, then re-attempt (§10.4).
        if super::comms::has_unanswered_ask(&c.db.lock(), &exec_id) {
            {
                let conn = c.db.lock();
                let _ = conn.execute(
                    "UPDATE wf_step_exec SET status = 'abandoned', ended_at = ?1 WHERE id = ?2",
                    rusqlite::params![super::now_ms(), exec_id],
                );
                journal_event(
                    &conn,
                    c.app.as_ref(),
                    &c.run_id,
                    event_type::ATTEMPT_ABANDONED,
                    Some(&exec_id),
                    &json!({ "cause": "question" }),
                );
            }
            let _ = c.driver.stop(&child_agent_id).await;
            let _ = c.driver.archive(&child_agent_id).await;
            match wait_for_answer(&c, &exec_id, &deadlines).await {
                AnswerWait::Answered => {
                    attempt_no += 1;
                    continue;
                }
                AnswerWait::Canceled => {
                    return done(
                        c.step.id.clone(),
                        last_exec,
                        ChildOutcome::Canceled,
                        child_ledger,
                    )
                }
                AnswerWait::Timeout => {
                    return done(
                        c.step.id.clone(),
                        last_exec,
                        ChildOutcome::Failure {
                            reason: "no answer from the orchestrator".into(),
                        },
                        child_ledger,
                    )
                }
            }
        }

        match result.outcome {
            AttemptOutcome::Done { .. } => {
                let head = match &result.worktree {
                    Some(wt) => gitops::head_sha(wt).await.ok(),
                    None => None,
                };
                let moved_head = match (&head, &stage_entry_sha) {
                    (Some(h), Some(entry)) => h != entry,
                    _ => false,
                };
                {
                    let conn = c.db.lock();
                    finish_step_exec(&conn, &exec_id, "done", head.as_deref());
                }
                let _ = c.driver.archive(&child_agent_id).await;
                return done(
                    c.step.id.clone(),
                    last_exec,
                    ChildOutcome::Success { moved_head, head },
                    child_ledger,
                );
            }
            AttemptOutcome::Canceled => {
                {
                    let conn = c.db.lock();
                    let _ = conn.execute(
                        "UPDATE wf_step_exec SET status = 'abandoned', ended_at = ?1 WHERE id = ?2",
                        rusqlite::params![super::now_ms(), exec_id],
                    );
                    journal_event(
                        &conn,
                        c.app.as_ref(),
                        &c.run_id,
                        event_type::ATTEMPT_ABANDONED,
                        Some(&exec_id),
                        &json!({ "cause": "canceled" }),
                    );
                }
                let _ = c.driver.archive(&child_agent_id).await;
                return done(
                    c.step.id.clone(),
                    last_exec,
                    ChildOutcome::Canceled,
                    child_ledger,
                );
            }
            AttemptOutcome::Error { error } => {
                {
                    let conn = c.db.lock();
                    finish_step_exec(&conn, &exec_id, "error", None);
                }
                last_failure = Some(error.clone());
                if attempt_no >= max_attempts {
                    return done(
                        c.step.id.clone(),
                        last_exec,
                        ChildOutcome::Failure { reason: error },
                        child_ledger,
                    );
                }
                attempt_no += 1;
            }
            AttemptOutcome::Blocked { reason } => {
                {
                    let conn = c.db.lock();
                    finish_step_exec(&conn, &exec_id, "blocked", None);
                }
                let _ = c.driver.stop(&child_agent_id).await;
                let _ = c.driver.archive(&child_agent_id).await;
                return done(
                    c.step.id.clone(),
                    last_exec,
                    ChildOutcome::Failure {
                        reason: format!("gate unmet: {reason}"),
                    },
                    child_ledger,
                );
            }
            AttemptOutcome::BudgetExceeded { which } => {
                {
                    let conn = c.db.lock();
                    finish_step_exec(&conn, &exec_id, "error", None);
                }
                let _ = c.driver.stop(&child_agent_id).await;
                let _ = c.driver.archive(&child_agent_id).await;
                return done(
                    c.step.id.clone(),
                    last_exec,
                    ChildOutcome::Failure {
                        reason: format!("budget_exceeded: {which}"),
                    },
                    child_ledger,
                );
            }
            AttemptOutcome::AwaitingApproval => {
                {
                    let conn = c.db.lock();
                    finish_step_exec(&conn, &exec_id, "error", None);
                }
                let _ = c.driver.archive(&child_agent_id).await;
                return done(
                    c.step.id.clone(),
                    last_exec,
                    ChildOutcome::Failure {
                        reason: "approval gates are not supported inside an orchestrate stage"
                            .into(),
                    },
                    child_ledger,
                );
            }
            AttemptOutcome::AwaitingAnswer => {
                // Children never set `pending_ask`; the `has_unanswered_ask` check
                // above handles asks. Defensive.
                {
                    let conn = c.db.lock();
                    finish_step_exec(&conn, &exec_id, "error", None);
                }
                let _ = c.driver.archive(&child_agent_id).await;
                return done(
                    c.step.id.clone(),
                    last_exec,
                    ChildOutcome::Failure {
                        reason: "unexpected awaiting-answer state".into(),
                    },
                    child_ledger,
                );
            }
        }
    }
}

/// Poll for the orchestrator's answer to this child's ask, bounded by the child's
/// stall timeout (§10.4 — no wait without a deadline). The persisted ask flips to
/// `answered` when the orchestrator replies; the answer itself is folded into the
/// next attempt's prompt.
async fn wait_for_answer(c: &ChildCtx, exec_id: &str, d: &Deadlines) -> AnswerWait {
    let deadline = tokio::time::Instant::now() + d.stall_timeout;
    loop {
        if c.stage_cancel.load(Ordering::SeqCst) {
            return AnswerWait::Canceled;
        }
        if !super::comms::has_unanswered_ask(&c.db.lock(), exec_id) {
            return AnswerWait::Answered;
        }
        if tokio::time::Instant::now() >= deadline {
            return AnswerWait::Timeout;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// Fold a reaped orchestrate child into the stage: sum its ledger, forward its
/// terminal outcome to the orchestrator (§10.1), and record its join status. A
/// join-`any` success winds the losers down.
#[allow(clippy::too_many_arguments)]
fn handle_orch_child(
    ctx: &RunCtx,
    run_id: &str,
    orch_exec: &str,
    join: Join,
    ledger: &mut Ledger,
    joined: std::result::Result<OrchChildResult, tokio::task::JoinError>,
    outcomes: &mut HashMap<String, ChildStatus>,
    child_cancels: &HashMap<String, Arc<AtomicBool>>,
    child_gen: &HashMap<String, u64>,
) {
    let res = match joined {
        Ok(r) => r,
        Err(e) => {
            // A child task panicked — contain it as a failure (§6.1).
            outcomes.insert(
                format!("panicked-{}", outcomes.len()),
                ChildStatus::Failure(format!("child task error: {e}")),
            );
            return;
        }
    };
    // Its spend counts regardless, but a result from an attempt a later
    // `retry_child` superseded must not touch the join outcome or wind losers
    // down — otherwise the stale attempt could decide the stage (§10.2).
    fold_child_ledger(ledger, &res.ledger);
    if child_gen.get(&res.step_id).copied().unwrap_or(0) != res.generation {
        let conn = ctx.db.lock();
        persist_spent(&conn, run_id, ledger);
        return;
    }
    let conn = ctx.db.lock();
    match res.outcome {
        ChildOutcome::Success { moved_head, head } => {
            if moved_head {
                journal_event(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    event_type::INTEGRATE_SKIPPED,
                    None,
                    &json!({ "step_id": res.step_id, "sha": head }),
                );
            }
            super::comms::forward_lifecycle(
                &conn,
                ctx.app.as_ref(),
                run_id,
                orch_exec,
                &res.exec_id,
                "done",
                &format!("child `{}` finished", res.step_id),
            );
            // Don't overwrite a `skip` the orchestrator already recorded.
            outcomes.entry(res.step_id).or_insert(ChildStatus::Success);
            if matches!(join, Join::Any) {
                // First success wins → wind every remaining child down (§6.6).
                cancel_all(child_cancels);
            }
        }
        ChildOutcome::Failure { reason } => {
            super::comms::forward_lifecycle(
                &conn,
                ctx.app.as_ref(),
                run_id,
                orch_exec,
                &res.exec_id,
                "error",
                &format!("child `{}` failed: {reason}", res.step_id),
            );
            outcomes
                .entry(res.step_id)
                .or_insert(ChildStatus::Failure(reason));
        }
        ChildOutcome::Canceled => {}
    }
    persist_spent(&conn, run_id, ledger);
}

/// Raise every child's cancel flag (a join-`any` winner or a stage teardown).
fn cancel_all(child_cancels: &HashMap<String, Arc<AtomicBool>>) {
    for flag in child_cancels.values() {
        flag.store(true, Ordering::SeqCst);
    }
}

/// The number of dynamic children an orchestrate stage has already created, from
/// the DB — the next dynamic index, seeded so a resumed stage never reuses an id
/// (§10.2). Dynamic child ids are `orchestrate-<n>::dyn-<k>`, `k` contiguous from
/// 0, so the distinct count is the next `k`.
fn existing_dyn_child_count(conn: &Connection, run_id: &str, orch_step_id: &str) -> u32 {
    let pattern = format!("{orch_step_id}::dyn-%");
    conn.query_row(
        "SELECT COUNT(DISTINCT step_id) FROM wf_step_exec WHERE run_id = ?1 AND step_id LIKE ?2",
        rusqlite::params![run_id, pattern],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n.max(0) as u32)
    .unwrap_or(0)
}

/// Wind down every remaining orchestrate child: cancel them all and drain the set
/// (each child stops its own agent and returns `Canceled`).
async fn drain_children(
    child_cancels: &HashMap<String, Arc<AtomicBool>>,
    set: &mut JoinSet<OrchChildResult>,
) {
    cancel_all(child_cancels);
    while set.join_next().await.is_some() {}
}

/// Drive one orchestrator turn and fold its budget in (§10.2, §11.2).
#[allow(clippy::too_many_arguments)]
async fn drive_orch_turn(
    ctx: &RunCtx,
    run_id: &str,
    orch_exec: &str,
    orch_step_id: &str,
    orch_agent_id: &str,
    prompt: String,
    kind: &'static str,
    deadlines: &Deadlines,
    ledger: &mut Ledger,
    eff: &EffectiveBudgets,
) -> OrchStepResult {
    if let Some(which) = ledger.exceeded(eff, super::now_ms()) {
        return OrchStepResult::Budget(which.as_str().to_string());
    }
    let (turn, events) =
        attempt::drive_prompt_turn(ctx.driver.as_ref(), orch_agent_id, prompt, kind, deadlines)
            .await;
    {
        let conn = ctx.db.lock();
        for e in &events {
            journal_event(
                &conn,
                ctx.app.as_ref(),
                run_id,
                e.event_type,
                Some(orch_exec),
                &e.payload,
            );
        }
    }
    ledger.charge_turn(orch_step_id, orch_exec);
    ledger.charge_tokens(
        orch_agent_id,
        orch_step_id,
        ctx.driver.turn_usage(orch_agent_id),
    );
    {
        let conn = ctx.db.lock();
        journal_event(
            &conn,
            ctx.app.as_ref(),
            run_id,
            event_type::BUDGET_TICK,
            None,
            &json!({ "turns": ledger.turns, "tokens": ledger.tokens }),
        );
        ledger.checkpoint_wall(super::now_ms());
        persist_spent(&conn, run_id, ledger);
    }
    match turn {
        attempt::OrchTurn::Ended => {
            if let Some(which) = ledger.exceeded(eff, super::now_ms()) {
                return OrchStepResult::Budget(which.as_str().to_string());
            }
            OrchStepResult::Ok
        }
        attempt::OrchTurn::Stalled => OrchStepResult::Stalled,
        attempt::OrchTurn::Error(e) => OrchStepResult::Error(e),
    }
}

/// Handle a non-`Ok` orchestrator turn: wind the children down and pause/fail the
/// run per the cause (§10.2). A stall escalates to the human (`question`).
#[allow(clippy::too_many_arguments)]
async fn finish_orch_turn_failure(
    ctx: &RunCtx,
    run_id: &str,
    orch_exec: &str,
    orch_agent_id: &str,
    child_cancels: &HashMap<String, Arc<AtomicBool>>,
    set: &mut JoinSet<OrchChildResult>,
    ledger: &mut Ledger,
    result: OrchStepResult,
) -> Result<StageFlow> {
    drain_children(child_cancels, set).await;
    match result {
        OrchStepResult::Stalled => {
            // A stalled orchestrator must not hang the stage — escalate to the
            // human (§10.2). `pause_question` abandons the exec, stops the agent,
            // and pauses `question`; a `wf_answer` resumes and re-drives the stage.
            {
                let conn = ctx.db.lock();
                journal_event(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    event_type::WATCHDOG_STALLED,
                    Some(orch_exec),
                    &json!({ "role": "orchestrator", "escalated": true }),
                );
            }
            // Record an engine ask so the human sees why the run paused and can
            // answer it (§10.4) — the orchestrator is unresponsive.
            {
                let conn = ctx.db.lock();
                super::comms::queue_engine_ask(
                    &conn,
                    run_id,
                    orch_exec,
                    "The orchestrator stalled. Provide guidance to continue, or cancel the run.",
                );
            }
            pause_question(ctx, run_id, orch_exec, Some(orch_agent_id)).await;
            Ok(StageFlow::Stop)
        }
        OrchStepResult::Budget(_) => {
            let _ = ctx.driver.stop(orch_agent_id).await;
            {
                let conn = ctx.db.lock();
                let _ = conn.execute(
                    "UPDATE wf_step_exec SET status = 'abandoned', ended_at = ?1 WHERE id = ?2",
                    rusqlite::params![super::now_ms(), orch_exec],
                );
            }
            finish_budget_pause(ctx, run_id, Some(orch_exec), ledger);
            Ok(StageFlow::Stop)
        }
        OrchStepResult::Error(e) => {
            let _ = ctx.driver.stop(orch_agent_id).await;
            let conn = ctx.db.lock();
            finish_step_exec(&conn, orch_exec, "error", None);
            persist_spent(&conn, run_id, ledger);
            set_status(
                &conn,
                ctx.app.as_ref(),
                run_id,
                "failed",
                None,
                Some(&format!("orchestrator error: {e}")),
            );
            Ok(StageFlow::Stop)
        }
        OrchStepResult::Ok => Ok(StageFlow::Advance { line: None }),
    }
}

// ─────────────────────────── linear steps & loops (§6.6) ────────────────────

/// Run-wide invariants every step attempt reads, bundled so the walker and the
/// loop executor share a single [`execute_step`] without a dozen positional args.
struct StepEnv<'a> {
    repo: &'a Path,
    run_repo: &'a Path,
    blackboard: &'a Path,
    eff: &'a EffectiveBudgets,
    test_override: &'a Option<String>,
    setup_override: &'a Option<String>,
    run_task: &'a str,
    spec_name: &'a str,
}

/// What executing one step (through its attempt/retry lifecycle) resolved to.
enum StepFlow {
    /// Gate met and ferried into the run repo. `head_ref` is the fork source for
    /// whatever comes next; `exec_id` is the durable record.
    Done { exec_id: String, head_ref: String },
    /// A loop's `until` step ended without a `done` verdict (revise / blocked /
    /// missing) — the loop iterates again rather than pausing. Returned only when
    /// `is_until` is set.
    LoopContinue,
    /// The run reached a paused or terminal state; the status row is already
    /// written and the drive loop must return.
    Halt,
}

/// Whether a loop block completed (advance to the next) or halted the run.
enum BlockFlow {
    Advance,
    Halt,
}

fn resolve_agent<'a>(spec: &'a Spec, step: &Step) -> Result<&'a AgentSpec> {
    spec.agents.get(&step.agent).ok_or_else(|| {
        Error::Other(format!(
            "step '{}' references unknown agent '{}'",
            step.id, step.agent
        ))
    })
}

/// Cancel handling shared by the loop executor and the top-level walker: stop
/// every live step agent (bind the id list first so the lock guard drops before
/// the awaits — a guard held across `.await` makes the drive future `!Send`),
/// then journal and mark the run canceled.
async fn cancel_run(ctx: &RunCtx, run_id: &str) {
    let agents = live_step_agents(&ctx.db.lock(), run_id);
    for a in agents {
        let _ = ctx.driver.stop(&a).await;
    }
    let conn = ctx.db.lock();
    journal_event(
        &conn,
        ctx.app.as_ref(),
        run_id,
        event_type::RUN_CANCELED,
        None,
        &json!({}),
    );
    set_status(&conn, ctx.app.as_ref(), run_id, "canceled", None, None);
}

/// Execute one loop block (spec §6.6): run the body sequence once per iteration,
/// exiting the moment the `until` step's verdict is `done`, and continuing the
/// outer sequence after `loop.max` iterations (loop exhaustion is not failure —
/// the reviewer's last verdict rides along in the PR body). The iteration counter
/// lives in the run cursor so a resumed run picks up mid-loop rather than
/// restarting at iteration 0. `last_ref` / `last_exec_id` thread the ferried HEAD
/// across iterations so feedback and commits accumulate; the budget `ledger` is
/// threaded through so a loop's turns/tokens count against the run caps (§11).
#[allow(clippy::too_many_arguments)]
async fn run_loop(
    ctx: &RunCtx,
    run_id: &str,
    env: &StepEnv<'_>,
    spec: &Spec,
    lp: &Loop,
    block_index: usize,
    cursor: &mut Cursor,
    last_ref: &mut String,
    last_exec_id: &mut Option<String>,
    ledger: &mut Ledger,
) -> Result<BlockFlow> {
    let key = block_index.to_string();
    let mut iter = cursor.iterations.get(&key).copied().unwrap_or(0);

    while iter < lp.max {
        // Persist the iteration before the body runs so a crash/restart resumes
        // this iteration (spec §6.4), not iteration 0.
        cursor.iterations.insert(key.clone(), iter);
        set_cursor(&ctx.db.lock(), run_id, cursor);
        {
            let conn = ctx.db.lock();
            journal_event(
                &conn,
                ctx.app.as_ref(),
                run_id,
                event_type::LOOP_ITERATION,
                None,
                &json!({ "iteration": iter, "max": lp.max }),
            );
        }

        let mut exit_done = false;
        for (body_index, body_block) in lp.body.iter().enumerate() {
            if ctx.cancel.load(Ordering::SeqCst) {
                cancel_run(ctx, run_id).await;
                return Ok(BlockFlow::Halt);
            }
            // `ensure_executable` already rejected non-step loop bodies.
            let Block::Step(step) = body_block else {
                let conn = ctx.db.lock();
                set_status(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    "failed",
                    None,
                    Some("nested non-step blocks inside a loop are not supported yet"),
                );
                return Ok(BlockFlow::Halt);
            };
            let is_until = step.id == lp.until.step;

            // Resume-skip: a body step already `done` this iteration (its work is
            // ferried and reflected in `last_ref`) must not re-run — attempts are
            // immutable (§6.4). This also promotes a loop-body approval that
            // `wf_approve` marked `done` without bumping the top-level cursor.
            if let Some(exec_id) = done_body_exec(&ctx.db.lock(), run_id, &step.id, iter) {
                *last_ref = gitops::step_ref(&exec_id);
                *last_exec_id = Some(exec_id);
                if is_until {
                    exit_done = true;
                    break;
                }
                continue;
            }

            let agent_spec = resolve_agent(spec, step)?;
            let position = Position {
                step_index: body_index,
                step_count: lp.body.len(),
                iteration: Some(IterationPos {
                    current: iter + 1,
                    max: lp.max,
                }),
            };
            match execute_step(
                ctx, run_id, env, step, agent_spec, position, iter, last_ref, is_until, ledger,
            )
            .await?
            {
                StepFlow::Done { exec_id, head_ref } => {
                    *last_ref = head_ref;
                    *last_exec_id = Some(exec_id);
                    if is_until {
                        // Exit condition met (§6.6): the `until` verdict is
                        // `done`, so the loop is finished. Break out of the body
                        // *now* — any body steps after the `until` step are
                        // remediation for a non-`done` verdict (e.g. `fix` after
                        // `review` in the §5.3 example) and must be skipped when
                        // there is nothing to remediate.
                        exit_done = true;
                        break;
                    }
                }
                // The `until` step returned a non-`done` verdict (revise/blocked).
                // Do NOT restart the body here: the remaining body steps *are* the
                // remediation for that verdict — in the canonical `[review, fix]`
                // loop, `review` is the `until` step and `fix` runs in response to
                // its `revise`. Falling through lets the rest of this iteration's
                // body run; the loop then restarts for the next iteration. (Only
                // the `until` step ever yields `LoopContinue`.)
                StepFlow::LoopContinue => {}
                StepFlow::Halt => return Ok(BlockFlow::Halt),
            }
        }

        if exit_done {
            return Ok(BlockFlow::Advance);
        }
        iter += 1;
    }

    // `loop.max` iterations without a `done` verdict: not a failure — journal it
    // and continue the outer sequence (spec §6.6, open question #1 default).
    let conn = ctx.db.lock();
    journal_event(
        &conn,
        ctx.app.as_ref(),
        run_id,
        event_type::LOOP_MAX_REACHED,
        None,
        &json!({ "iterations": lp.max }),
    );
    Ok(BlockFlow::Advance)
}

/// Drive one step through its full attempt/retry lifecycle (spec §6.3, §6.5),
/// enforcing the budget ledger (§11), the tests gate (§9.4), and comms delivery /
/// pending-ask deferral (§10.4). Shared by the top-level walker and loop bodies.
/// On a met gate it boundary-commits + ferries and returns [`StepFlow::Done`]; a
/// blocked/stalled/errored/over-budget/awaiting terminal writes the paused/failed
/// status and returns [`StepFlow::Halt`]. When `is_until` is set (a loop's exit
/// step) a blocked gate is *not* a pause — a non-`done` verdict is the "iterate
/// again" signal, so it returns [`StepFlow::LoopContinue`] and the single
/// in-attempt re-prompt is suppressed (nagging a reviewer to flip "revise" →
/// "done" would defeat the loop). A pending human `wf_ask` still pauses the run
/// `question` regardless of `is_until` (§10.4).
#[allow(clippy::too_many_arguments)]
async fn execute_step(
    ctx: &RunCtx,
    run_id: &str,
    env: &StepEnv<'_>,
    step: &Step,
    agent_spec: &AgentSpec,
    position: Position,
    iteration: u32,
    fork_ref: &str,
    is_until: bool,
    ledger: &mut Ledger,
) -> Result<StepFlow> {
    // Step-effective budgets: run-level frozen caps with this step's own
    // `budgets` overlaid (§11.1). Feeds the attempt timeouts and retry cap.
    let step_eff = env.eff.for_step(step.budgets.as_ref());
    let max_attempts = step_eff.max_attempts;
    let deadlines = deadlines_from(&ctx.deadlines, &step_eff);
    // Tests-gate runner for this step, honoring its effective `tests_timeout_secs`
    // (spec §9.4, §11.1). A fresh runner per step means setup runs once per step
    // workspace.
    let test_runner = super::tests_gate::SandboxTestRunner::new(
        env.test_override.clone(),
        env.setup_override.clone(),
        step_eff.tests_timeout_secs.max(1) as u64,
    )?;

    let mut attempt_no = next_attempt_no(&ctx.db.lock(), run_id, &step.id, iteration as i64);
    let mut last_failure: Option<String> = None;

    loop {
        // Enforcement point: before every spawn (§11.2). No attempt row is
        // created — the run pauses at the block boundary.
        if let Some(which) = ledger.exceeded(&step_eff, super::now_ms()) {
            {
                let conn = ctx.db.lock();
                journal_event(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    event_type::BUDGET_EXCEEDED,
                    None,
                    &json!({ "which": which.as_str() }),
                );
            }
            finish_budget_pause(ctx, run_id, None, ledger);
            return Ok(StepFlow::Halt);
        }

        let exec_id = format!("exec-{}", uuid::Uuid::new_v4());
        {
            let conn = ctx.db.lock();
            create_step_exec(
                &conn,
                &exec_id,
                run_id,
                &step.id,
                attempt_no,
                iteration as i64,
                gate_mode(&step.gate),
            );
        }

        let prompt = {
            let ctx_prompt = StepPromptCtx {
                run_task: env.run_task,
                step_id: &step.id,
                step_goal: &step.goal,
                position: position.clone(),
                gate: &step.gate,
                turns_per_attempt: step.budgets.as_ref().and_then(|b| b.turns_per_attempt),
                comms: &step.comms,
            };
            let base = match &last_failure {
                Some(f) => prompts::retry_prompt(f, &ctx_prompt),
                None => prompts::step_prompt(&ctx_prompt),
            };
            // Fold any messages queued for this step (a human's `wf_answer`, an
            // orchestrator notify) into this one prompt — coalesced, so many
            // messages cost one turn (§10.4). Marked delivered here so later
            // attempts of the same step don't re-fold them.
            let delivered = {
                let conn = ctx.db.lock();
                super::comms::take_pending_deliveries(&conn, run_id, &step.id)
            };
            if delivered.is_empty() {
                base
            } else {
                format!("{}\n\n{}", super::comms::compose_delivery(&delivered), base)
            }
        };

        let params = AttemptParams {
            spawn_req: build_spawn_req(agent_spec, fork_ref, env.repo, env.run_repo, run_id),
            pre_spawned: None,
            blackboard: env.blackboard.to_path_buf(),
            exec_id: exec_id.clone(),
            step_id: step.id.clone(),
            attempt: attempt_no as u32,
            iteration,
            gate: step.gate.clone(),
            prompt,
            deadlines: deadlines.clone(),
            // A loop's `until` step must not be re-prompted on "revise": that is
            // a legitimate turn end, not an unmet gate to nag about (§6.6).
            reprompt_on_block: !is_until,
            // Linear/loop steps are never cancelled mid-attempt (only parallel
            // losers are, §6.6), so this flag is never set.
            cancel: Arc::new(AtomicBool::new(false)),
            // Shared with the run's `RunHandle` so the comms router and this
            // attempt observe the same pending-ask flag (§10.4).
            pending_ask: ctx.pending_ask.clone(),
        };

        let started = super::now_ms();
        let result =
            attempt::run_attempt(ctx.driver.as_ref(), &test_runner, params, ledger, &step_eff)
                .await;
        // Journal the attempt's events, stamp its agent id, persist the ledger.
        {
            let conn = ctx.db.lock();
            if let Some(agent_id) = &result.agent_id {
                let _ = conn.execute(
                    "UPDATE wf_step_exec SET agent_id = ?1, started_at = ?2 WHERE id = ?3",
                    rusqlite::params![agent_id, started, exec_id],
                );
            }
            for e in &result.events {
                journal_event(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    e.event_type,
                    Some(&exec_id),
                    &e.payload,
                );
            }
            ledger.checkpoint_wall(super::now_ms());
            persist_spent(&conn, run_id, ledger);
        }

        // Drain the agent's RPC mailbox before acting on the gate: a `wf_ask`
        // written during the turn may still be undispatched (§10.4). The agent is
        // idle here, so nothing new arrives after this drain.
        if let Some(agent_id) = &result.agent_id {
            ctx.driver.settle_rpc(agent_id).await;
        }
        // Authoritative pending-ask backstop (§10.4): a persisted, unanswered ask
        // pauses `question` regardless of the gate outcome (which is never acted
        // on while an answer is outstanding).
        let outcome = if super::comms::has_unanswered_ask(&ctx.db.lock(), &exec_id) {
            AttemptOutcome::AwaitingAnswer
        } else {
            result.outcome
        };

        match outcome {
            AttemptOutcome::Done { .. } => {
                let wt = result
                    .worktree
                    .ok_or_else(|| Error::Other("done attempt without a worktree".into()))?;
                // Boundary commit + pin + ferry — the `done` precondition (§6.3
                // steps 7–8). A ferry failure keeps the attempt out of `done` and
                // drops to the retry policy.
                let msg = format!("wf({}): {} attempt {}", env.spec_name, step.id, attempt_no);
                match ferry_step(ctx, run_id, &exec_id, &msg, &wt, env.run_repo).await {
                    Ok(head) => {
                        // Atomic commit point: a late `wf_ask` can be routed during
                        // `ferry`, so re-check under the finalizing lock (§10.4).
                        let committed = commit_done_unless_ask(&ctx.db.lock(), &exec_id, &head);
                        if !committed {
                            pause_question(ctx, run_id, &exec_id, result.agent_id.as_deref()).await;
                            return Ok(StepFlow::Halt);
                        }
                        if let Some(agent_id) = &result.agent_id {
                            let _ = ctx.driver.archive(agent_id).await;
                        }
                        return Ok(StepFlow::Done {
                            head_ref: gitops::step_ref(&exec_id),
                            exec_id,
                        });
                    }
                    Err(e) => {
                        last_failure = Some(format!("ferry failed: {e}"));
                        // Idle but alive; marking the exec `error` hides it from
                        // `live_step_agents`, so stop it here (not archive —
                        // `archive` rejects a not-yet-stopped agent) to keep the
                        // CLI process from leaking past retry/fail.
                        if let Some(agent_id) = &result.agent_id {
                            let _ = ctx.driver.stop(agent_id).await;
                        }
                        let give_up = attempt_no >= max_attempts;
                        {
                            let conn = ctx.db.lock();
                            finish_step_exec(&conn, &exec_id, "error", None);
                            if give_up {
                                set_status(
                                    &conn,
                                    ctx.app.as_ref(),
                                    run_id,
                                    "failed",
                                    None,
                                    Some(&format!("ferry failed: {e}")),
                                );
                            }
                        }
                        if give_up {
                            return Ok(StepFlow::Halt);
                        }
                    }
                }
                attempt_no += 1;
            }
            AttemptOutcome::AwaitingApproval => {
                // Commit the work now so approval only decides whether to advance
                // (§6.3 step 8, §9). The agent is archived; the run pauses until
                // `wf_approve` promotes the attempt and resumes.
                let msg = format!("wf({}): {} attempt {}", env.spec_name, step.id, attempt_no);
                let head = ferry_step(
                    ctx,
                    run_id,
                    &exec_id,
                    &msg,
                    result.worktree.as_ref().unwrap(),
                    env.run_repo,
                )
                .await?;
                {
                    let conn = ctx.db.lock();
                    finish_step_exec(&conn, &exec_id, "awaiting_approval", Some(&head));
                }
                if let Some(agent_id) = &result.agent_id {
                    let _ = ctx.driver.archive(agent_id).await;
                }
                let conn = ctx.db.lock();
                journal_event(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    event_type::RUN_PAUSED,
                    Some(&exec_id),
                    &json!({ "reason": "approval" }),
                );
                set_status(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    "paused",
                    Some("approval"),
                    None,
                );
                return Ok(StepFlow::Halt);
            }
            AttemptOutcome::AwaitingAnswer => {
                // The step asked the human a question (§10.4). No gate, no ferry,
                // no advance — pause `question`; the cursor is left in place so a
                // fresh attempt re-runs this step with the answer folded in.
                pause_question(ctx, run_id, &exec_id, result.agent_id.as_deref()).await;
                return Ok(StepFlow::Halt);
            }
            AttemptOutcome::Blocked { reason } => {
                if is_until {
                    // A loop exit step's non-`done` verdict: record the attempt
                    // and let the loop iterate again. Archive (not stop) so the
                    // reviewer's chat stays replayable from the timeline.
                    {
                        let conn = ctx.db.lock();
                        finish_step_exec(&conn, &exec_id, "blocked", None);
                    }
                    if let Some(agent_id) = &result.agent_id {
                        let _ = ctx.driver.archive(agent_id).await;
                    }
                    return Ok(StepFlow::LoopContinue);
                }
                {
                    let conn = ctx.db.lock();
                    finish_step_exec(&conn, &exec_id, "blocked", None);
                }
                if let Some(agent_id) = &result.agent_id {
                    let _ = ctx.driver.stop(agent_id).await;
                }
                let conn = ctx.db.lock();
                journal_event(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    event_type::RUN_PAUSED,
                    Some(&exec_id),
                    &json!({ "reason": "blocked_gate", "detail": reason }),
                );
                set_status(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    "paused",
                    Some("blocked_gate"),
                    None,
                );
                return Ok(StepFlow::Halt);
            }
            AttemptOutcome::Error { error } => {
                {
                    let conn = ctx.db.lock();
                    finish_step_exec(&conn, &exec_id, "error", None);
                }
                last_failure = Some(error.clone());
                if attempt_no >= max_attempts {
                    // Stall pauses for inspection (resumable); other errors fail
                    // the run (§6.5).
                    let conn = ctx.db.lock();
                    if error == "stalled" {
                        journal_event(
                            &conn,
                            ctx.app.as_ref(),
                            run_id,
                            event_type::RUN_PAUSED,
                            Some(&exec_id),
                            &json!({ "reason": "stalled" }),
                        );
                        set_status(
                            &conn,
                            ctx.app.as_ref(),
                            run_id,
                            "paused",
                            Some("stalled"),
                            None,
                        );
                    } else {
                        set_status(
                            &conn,
                            ctx.app.as_ref(),
                            run_id,
                            "failed",
                            None,
                            Some(&error),
                        );
                    }
                    return Ok(StepFlow::Halt);
                }
                attempt_no += 1;
            }
            AttemptOutcome::BudgetExceeded { .. } => {
                // A run-level cap was hit mid-attempt (§11.2). The attempt already
                // journaled `budget_exceeded`; finish its bookkeeping — stop the
                // agent, abandon the incomplete attempt — and pause. Resume-with-
                // patch (§13) starts a fresh attempt for this step.
                if let Some(agent_id) = &result.agent_id {
                    let _ = ctx.driver.stop(agent_id).await;
                }
                {
                    let conn = ctx.db.lock();
                    let _ = conn.execute(
                        "UPDATE wf_step_exec SET status = 'abandoned', ended_at = ?1 WHERE id = ?2",
                        rusqlite::params![super::now_ms(), exec_id],
                    );
                    journal_event(
                        &conn,
                        ctx.app.as_ref(),
                        run_id,
                        event_type::ATTEMPT_ABANDONED,
                        Some(&exec_id),
                        &json!({ "cause": "budget_exceeded" }),
                    );
                }
                finish_budget_pause(ctx, run_id, Some(&exec_id), ledger);
                return Ok(StepFlow::Halt);
            }
            AttemptOutcome::Canceled => {
                // Linear/loop steps never pass a live cancel flag, so this is
                // unreachable in practice; handle it defensively as an abandonment
                // (the agent is already stopped by `run_attempt`).
                let conn = ctx.db.lock();
                let _ = conn.execute(
                    "UPDATE wf_step_exec SET status = 'abandoned', ended_at = ?1 WHERE id = ?2",
                    rusqlite::params![super::now_ms(), exec_id],
                );
                return Ok(StepFlow::Halt);
            }
        }
    }
}

fn gate_mode(gate: &Gate) -> &'static str {
    match gate {
        Gate::Verdict => "verdict",
        Gate::Commit => "commit",
        Gate::Artifact { .. } => "artifact",
        Gate::Tests => "tests",
        Gate::Approval => "approval",
    }
}

fn slugify(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "run".to_string()
    } else {
        s.chars().take(40).collect()
    }
}

/// The resume/retry guard: only a `paused(blocked_gate|stalled|budget_exceeded)`
/// run may be re-driven. A terminal run must not restart; a `paused(approval)`
/// run must go through `wf_approve`; and a `paused(question)` run must go through
/// `wf_answer` — resuming it without an answer would re-prompt the step with no
/// human response (§10.4). Callers run this before any state mutation so a
/// rejected resume changes nothing.
fn check_resumable(conn: &Connection, run_id: &str, action: &str) -> Result<()> {
    let (status, reason) = run_status(conn, run_id)?;
    if status != "paused" {
        return Err(Error::Other(format!("cannot {action} a {status} run")));
    }
    if reason.as_deref() == Some("approval") {
        return Err(Error::Other(
            "run is awaiting approval — use wf_approve (or wf_cancel)".into(),
        ));
    }
    if reason.as_deref() == Some("question") {
        return Err(Error::Other(
            "run is awaiting an answer — use wf_answer (or wf_cancel)".into(),
        ));
    }
    if reason.as_deref() == Some("conflict") {
        return Err(Error::Other(
            "run is paused on a merge conflict — use wf_resolve_conflict (or wf_cancel)".into(),
        ));
    }
    Ok(())
}

pub(super) fn run_status(conn: &Connection, run_id: &str) -> Result<(String, Option<String>)> {
    conn.query_row(
        "SELECT status, paused_reason FROM wf_run WHERE id = ?1",
        [run_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .map_err(|e| Error::Other(format!("run {run_id} not found: {e}")))
}

/// Read a project setting value, `None` when unset, blank, or the table is
/// absent. Used to resolve the tests-gate command overrides (spec §9.4), which
/// mirror the Run panel's `run.test` / `run.install` keys.
fn project_setting(conn: &Connection, project_id: &str, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM project_settings WHERE project_id = ?1 AND key = ?2",
        rusqlite::params![project_id, key],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
}

fn load_run(conn: &Connection, run_id: &str) -> Result<RunEssentials> {
    conn.query_row(
        "SELECT spec_json, task, project_id, repo_path, run_dir, branch, base_sha, status,
                budgets_json, spent_json
         FROM wf_run WHERE id = ?1",
        [run_id],
        |r| {
            Ok(RunEssentials {
                spec_json: r.get(0)?,
                task: r.get(1)?,
                project_id: r.get(2)?,
                repo_path: r.get(3)?,
                run_dir: r.get(4)?,
                branch: r.get(5)?,
                base_sha: r.get(6)?,
                status: r.get(7)?,
                budgets_json: r.get(8)?,
                spent_json: r.get(9)?,
            })
        },
    )
    .map_err(|e| Error::Other(format!("run {run_id} not found: {e}")))
}

/// Update the run row's status and emit `wf:run` (when an app handle is present).
fn set_status(
    conn: &Connection,
    app: Option<&AppHandle>,
    run_id: &str,
    status: &str,
    paused_reason: Option<&str>,
    error: Option<&str>,
) {
    let _ = conn.execute(
        "UPDATE wf_run SET status = ?1, paused_reason = ?2, error = ?3, updated_at = ?4 WHERE id = ?5",
        rusqlite::params![status, paused_reason, error, super::now_ms(), run_id],
    );
    if let Some(app) = app {
        if let Ok(run) = conn.query_row(
            "SELECT * FROM wf_run WHERE id = ?1",
            [run_id],
            super::types::Run::from_row,
        ) {
            journal::emit_run(app, &run);
        }
    }
}

/// Append a journal event and emit `wf:event` (when an app handle is present).
pub(super) fn journal_event(
    conn: &Connection,
    app: Option<&AppHandle>,
    run_id: &str,
    event_type: &str,
    step_exec_id: Option<&str>,
    payload: &Value,
) {
    match journal::append(conn, run_id, event_type, step_exec_id, payload) {
        Ok(ev) => {
            if let Some(app) = app {
                journal::emit_event(app, &ev);
            }
        }
        Err(e) => tracing::warn!(error = %e, run_id, event_type, "journal append failed"),
    }
}

fn create_step_exec(
    conn: &Connection,
    id: &str,
    run_id: &str,
    step_id: &str,
    attempt: i64,
    iteration: i64,
    gate_mode: &str,
) {
    let _ = conn.execute(
        "INSERT INTO wf_step_exec (id, run_id, step_id, attempt, iteration, status, gate_mode)
         VALUES (?1, ?2, ?3, ?4, ?5, 'spawning', ?6)",
        rusqlite::params![id, run_id, step_id, attempt, iteration, gate_mode],
    );
}

fn finish_step_exec(conn: &Connection, id: &str, status: &str, head_end: Option<&str>) {
    let _ = conn.execute(
        "UPDATE wf_step_exec SET status = ?1, head_end = ?2, ended_at = ?3 WHERE id = ?4",
        rusqlite::params![status, head_end, super::now_ms(), id],
    );
}

/// The next attempt number for a step *within one iteration* — retries increment
/// `attempt`, while each loop iteration is a fresh execution counted separately by
/// the `iteration` column (spec §4). Scoping by iteration keeps `attempt` a true
/// retry count and the §8.3 `attempt-<n>.iter-<i>` archive labels meaningful.
fn next_attempt_no(conn: &Connection, run_id: &str, step_id: &str, iteration: i64) -> i64 {
    conn.query_row(
        "SELECT COALESCE(MAX(attempt), 0) + 1 FROM wf_step_exec
         WHERE run_id = ?1 AND step_id = ?2 AND iteration = ?3",
        rusqlite::params![run_id, step_id, iteration],
        |r| r.get(0),
    )
    .unwrap_or(1)
}

/// The scheduler cursor (spec §6.4): the index into the top-level block sequence
/// plus, for any loop entered, its current iteration keyed by the loop's
/// top-level block index. A run's `spec_json` is immutable after launch, so the
/// index is a stable key. The old `{ "index": N }` shape still deserializes
/// (`iterations` defaults empty).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct Cursor {
    #[serde(default)]
    index: i64,
    #[serde(default)]
    iterations: std::collections::BTreeMap<String, u32>,
    /// In-progress state of a code-producing parallel merge (§12.3). Present only
    /// while a `integrate: merge` stage is mid-merge or paused on a conflict; the
    /// cursor `index` still points at that stage until it finalizes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    merge: Option<MergeCursor>,
}

/// The resumable state of a merge stage (§12.3): which children remain to merge
/// (in spec order) and, if paused, the recorded conflict.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct MergeCursor {
    block_index: usize,
    /// `(step_id, ferried_ref)` children not yet merged, in spec order.
    remaining: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    conflict: Option<ConflictInfo>,
}

/// A recorded merge conflict awaiting resolution (§12.3 modes a/c).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ConflictInfo {
    /// The child whose merge conflicted.
    step_id: String,
    files: Vec<String>,
    /// The committed conflict snapshot a mode-(a) resolution step forks from.
    conflict_ref: String,
    /// Chosen by `wf_resolve_conflict`: `"agent"` (mode a) or `"human"` (mode c).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resolution: Option<String>,
}

/// The synthetic `wf_step_exec.step_id` for a merge stage's integrated result —
/// the fork source the next block (and finalize) reads via [`resume_line_state`].
fn merge_step_id(block_index: usize) -> String {
    format!("__merge_{block_index}")
}

fn get_cursor(conn: &Connection, run_id: &str) -> Cursor {
    let raw: Option<String> = conn
        .query_row(
            "SELECT cursor_json FROM wf_run WHERE id = ?1",
            [run_id],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    raw.and_then(|c| serde_json::from_str::<Cursor>(&c).ok())
        .unwrap_or_default()
}

fn set_cursor(conn: &Connection, run_id: &str, cursor: &Cursor) {
    let json = serde_json::to_string(cursor).unwrap_or_else(|_| "{}".to_string());
    let _ = conn.execute(
        "UPDATE wf_run SET cursor_json = ?1, updated_at = ?2 WHERE id = ?3",
        rusqlite::params![json, super::now_ms(), run_id],
    );
}

/// Whether the top-level block at `index` is a plain `step` (vs a loop/parallel/
/// orchestrate container) — governs whether `wf_approve` advances the cursor
/// (§6.6): a top-level step's approval advances; a loop-body approval is advanced
/// by the loop's resume-skip on re-drive.
fn top_level_block_is_step(conn: &Connection, run_id: &str, index: i64) -> bool {
    let Ok(spec_json) = conn.query_row(
        "SELECT spec_json FROM wf_run WHERE id = ?1",
        [run_id],
        |r| r.get::<_, String>(0),
    ) else {
        return false;
    };
    serde_json::from_str::<Spec>(&spec_json)
        .ok()
        .and_then(|s| s.workflow.get(index as usize).cloned())
        .map(|b| matches!(b, Block::Step(_)))
        .unwrap_or(false)
}

/// The exec id of a body step already `done` this loop iteration, if any — the
/// resume-skip key (spec §6.4): its ferried work is durable, so it must not
/// re-run. Scoped to `(step_id, iteration)`; `attempt` is ignored.
fn done_body_exec(
    conn: &Connection,
    run_id: &str,
    step_id: &str,
    iteration: u32,
) -> Option<String> {
    conn.query_row(
        "SELECT id FROM wf_step_exec
         WHERE run_id = ?1 AND step_id = ?2 AND iteration = ?3 AND status = 'done'
         ORDER BY rowid DESC LIMIT 1",
        rusqlite::params![run_id, step_id, iteration],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

/// Persist the run's ledger snapshot to `spent_json` (§11.2).
fn persist_spent(conn: &Connection, run_id: &str, ledger: &Ledger) {
    let _ = conn.execute(
        "UPDATE wf_run SET spent_json = ?1, updated_at = ?2 WHERE id = ?3",
        rusqlite::params![ledger.to_json().to_string(), super::now_ms(), run_id],
    );
}

/// Pause a run `budget_exceeded` (§11.2): fold in the drive's active wall-clock,
/// persist the ledger, journal `run_paused`, and set the row. The caller has
/// already journaled the `budget_exceeded` event (from the attempt's events or
/// the pre-spawn check) and settled any live agent.
fn finish_budget_pause(ctx: &RunCtx, run_id: &str, exec_id: Option<&str>, ledger: &mut Ledger) {
    ledger.checkpoint_wall(super::now_ms());
    let conn = ctx.db.lock();
    persist_spent(&conn, run_id, ledger);
    journal_event(
        &conn,
        ctx.app.as_ref(),
        run_id,
        event_type::RUN_PAUSED,
        exec_id,
        &json!({ "reason": "budget_exceeded" }),
    );
    set_status(
        &conn,
        ctx.app.as_ref(),
        run_id,
        "paused",
        Some("budget_exceeded"),
        None,
    );
}

/// Attempt deadlines resolved from the step-effective budgets (§11.1). The
/// watchdog cadence comes from `base` — it is not a budget field.
fn deadlines_from(base: &Deadlines, eff: &EffectiveBudgets) -> Deadlines {
    let secs = |n: i64| std::time::Duration::from_secs(n.max(0) as u64);
    Deadlines {
        spawn_timeout: secs(eff.spawn_timeout_secs),
        turn_start_timeout: secs(eff.turn_start_timeout_secs),
        stall_timeout: secs(eff.stall_timeout_secs),
        nudge_timeout: secs(eff.nudge_timeout_secs),
        watchdog_tick: base.watchdog_tick,
    }
}

/// The exec id of the most recent `done` attempt of a specific step.
fn latest_done_exec_for_step(conn: &Connection, run_id: &str, step_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT id FROM wf_step_exec
         WHERE run_id = ?1 AND step_id = ?2 AND status = 'done'
         ORDER BY rowid DESC LIMIT 1",
        rusqlite::params![run_id, step_id],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

/// The step's `done` exec id and its completion time (`ended_at`, 0 if unset).
/// Used to pick the `any`-join winner — the child that finished first (§12.3).
fn done_exec_with_ended_at(
    conn: &Connection,
    run_id: &str,
    step_id: &str,
) -> Option<(String, i64)> {
    conn.query_row(
        "SELECT id, COALESCE(ended_at, 0) FROM wf_step_exec
         WHERE run_id = ?1 AND step_id = ?2 AND status = 'done'
         ORDER BY rowid DESC LIMIT 1",
        rusqlite::params![run_id, step_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
    .ok()
    .flatten()
}

/// Whether a step already has a `done` attempt (a parallel child that finished
/// in an earlier, interrupted drive — resume must not re-run it, §12.3 / S8).
fn child_already_done(conn: &Connection, run_id: &str, step_id: &str) -> bool {
    latest_done_exec_for_step(conn, run_id, step_id).is_some()
}

/// Recompute the line's fork source at resume: the last **top-level `step`**
/// before the cursor that reached `done` (its ferried ref + exec id), else the
/// run base. Parallel `integrate: none` children are deliberately ignored — they
/// never advance the line — which is why this walks the block tree rather than
/// querying "the most recent done exec".
fn resume_line_state(
    conn: &Connection,
    run_id: &str,
    blocks: &[Block],
    cursor: usize,
    base_sha: &str,
) -> (String, Option<String>) {
    let upper = cursor.min(blocks.len());
    for i in (0..upper).rev() {
        match &blocks[i] {
            Block::Step(s) => {
                if let Some(exec_id) = latest_done_exec_for_step(conn, run_id, &s.id) {
                    return (gitops::step_ref(&exec_id), Some(exec_id));
                }
            }
            // A merge stage advances the line via a synthetic `__merge_<i>` exec
            // pinned in the run repo (§12.3); an `integrate: none` stage doesn't.
            Block::Parallel(p) if matches!(p.integrate, Integrate::Merge) => {
                if let Some(exec_id) = latest_done_exec_for_step(conn, run_id, &merge_step_id(i)) {
                    return (gitops::step_ref(&exec_id), Some(exec_id));
                }
            }
            _ => {}
        }
    }
    (base_sha.to_string(), None)
}

/// Live (spawned, non-terminal) step agents for a run — stopped on cancel/pause.
fn live_step_agents(conn: &Connection, run_id: &str) -> Vec<String> {
    conn.prepare(
        "SELECT agent_id FROM wf_step_exec
         WHERE run_id = ?1 AND agent_id IS NOT NULL
           AND status IN ('spawning','running','gating')",
    )
    .and_then(|mut s| {
        s.query_map([run_id], |r| r.get::<_, Option<String>>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()
    })
    .map(|v| v.into_iter().flatten().collect())
    .unwrap_or_default()
}

/// Mark any non-terminal attempt `abandoned("resume")` (spec §6.4). Its agent,
/// if still tracked, is stopped.
async fn abandon_stale_attempts(ctx: &RunCtx, run_id: &str) {
    let stale: Vec<(String, Option<String>)> = {
        let conn = ctx.db.lock();
        conn.prepare(
            "SELECT id, agent_id FROM wf_step_exec
             WHERE run_id = ?1 AND status IN ('spawning','running','gating')",
        )
        .and_then(|mut s| {
            s.query_map([run_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
        })
        .unwrap_or_default()
    };
    for (exec_id, agent_id) in stale {
        if let Some(a) = &agent_id {
            let _ = ctx.driver.stop(a).await;
        }
        let conn = ctx.db.lock();
        let _ = conn.execute(
            "UPDATE wf_step_exec SET status = 'abandoned', ended_at = ?1 WHERE id = ?2",
            rusqlite::params![super::now_ms(), exec_id],
        );
        journal_event(
            &conn,
            ctx.app.as_ref(),
            run_id,
            event_type::ATTEMPT_ABANDONED,
            Some(&exec_id),
            &json!({ "cause": "resume" }),
        );
    }
}

// ───────────────────────────── commands (§13) ───────────────────────────────

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supervisor::StatusEvent;
    use crate::workspace::AgentStatus;
    use std::collections::BTreeMap;
    use std::process::Command as Sh;
    use tokio::sync::broadcast;

    fn sh(dir: &Path, args: &[&str]) {
        let out = Sh::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A real-git stub driver: on spawn it provisions a real `--shared` clone
    /// forking from the run repo (exercising `provision_forking_run_repo`), and
    /// on each prompt the "agent" makes a commit in that workspace. This drives
    /// the full scheduler + gitops + journal path with a mocked agent lifecycle
    /// — the §16 stub-agent integration test, deterministic and process-free.
    struct StubDriver {
        root: PathBuf,
        /// Whether the "agent" commits during its turn. `false` models an agent
        /// that does nothing, so a `commit` gate stays unmet (blocked-gate test).
        commit: bool,
        tx: broadcast::Sender<StatusEvent>,
        state: parking_lot::Mutex<StubState>,
    }
    #[derive(Default)]
    struct StubState {
        statuses: HashMap<String, AgentStatus>,
        worktrees: HashMap<String, PathBuf>,
        count: usize,
    }
    impl StubDriver {
        fn new(root: PathBuf, commit: bool) -> Arc<Self> {
            Arc::new(Self {
                root,
                commit,
                tx: broadcast::channel(256).0,
                state: parking_lot::Mutex::new(StubState::default()),
            })
        }
        fn set(&self, id: &str, s: AgentStatus) {
            self.state.lock().statuses.insert(id.to_string(), s.clone());
            let _ = self.tx.send(StatusEvent {
                agent_id: id.to_string(),
                status: s,
            });
        }
    }
    impl AgentDriver for StubDriver {
        fn spawn(
            &self,
            req: SpawnReq,
        ) -> super::super::driver::BoxFuture<'_, Result<super::super::driver::SpawnedAgent>>
        {
            Box::pin(async move {
                let id = {
                    let mut st = self.state.lock();
                    st.count += 1;
                    format!("stub-{}", st.count)
                };
                let dest = self.root.join(&id);
                let base_ref = req.fork_base.clone().unwrap();
                let spec = crate::sandbox::provision::CheckoutSpec {
                    source_repo: &req.repo_path,
                    base_ref: &base_ref,
                    dest: &dest,
                };
                crate::sandbox::provision::provision_forking_run_repo(
                    &spec,
                    req.run_repo.as_ref().unwrap(),
                )
                .await?;
                sh(&dest, &["config", "user.email", "t@t.t"]);
                sh(&dest, &["config", "user.name", "t"]);
                self.state.lock().worktrees.insert(id.clone(), dest.clone());
                self.set(&id, AgentStatus::Idle);
                Ok(super::super::driver::SpawnedAgent {
                    agent_id: id,
                    worktree: dest,
                })
            })
        }
        fn status(&self, id: &str) -> Option<AgentStatus> {
            self.state.lock().statuses.get(id).cloned()
        }
        fn subscribe(&self) -> broadcast::Receiver<StatusEvent> {
            self.tx.subscribe()
        }
        fn send_message<'a>(
            &'a self,
            id: &'a str,
            _text: String,
        ) -> super::super::driver::BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                let wt = self.state.lock().worktrees.get(id).cloned().unwrap();
                self.set(id, AgentStatus::Running);
                if self.commit {
                    std::fs::write(wt.join(format!("{id}.txt")), "work").unwrap();
                    sh(&wt, &["add", "-A"]);
                    sh(&wt, &["commit", "-qm", "agent work"]);
                }
                self.set(id, AgentStatus::Idle);
                Ok(())
            })
        }
        fn stop<'a>(&'a self, _id: &'a str) -> super::super::driver::BoxFuture<'a, Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn archive<'a>(&'a self, _id: &'a str) -> super::super::driver::BoxFuture<'a, Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn last_activity(&self, _id: &str) -> Option<i64> {
            None
        }
        fn turn_usage(&self, _id: &str) -> Option<super::super::driver::TurnUsage> {
            None
        }
    }

    fn step(id: &str) -> Step {
        Step {
            id: id.to_string(),
            agent: "coder".to_string(),
            goal: format!("do {id}"),
            gate: Gate::Commit,
            budgets: None,
            comms: vec![],
        }
    }

    #[tokio::test]
    async fn linear_two_step_run_reaches_done_and_pushes() {
        let tmp = tempfile::tempdir().unwrap();

        // Bare "remote" + a source repo that points origin at it.
        let bare = tmp.path().join("origin.git");
        std::fs::create_dir_all(&bare).unwrap();
        sh(&bare, &["init", "-q", "--bare", "-b", "main"]);
        let source = tmp.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        sh(&source, &["init", "-q", "-b", "main"]);
        sh(&source, &["config", "user.email", "t@t.t"]);
        sh(&source, &["config", "user.name", "t"]);
        std::fs::write(source.join("README"), "base").unwrap();
        sh(&source, &["add", "-A"]);
        sh(&source, &["commit", "-qm", "base"]);
        sh(
            &source,
            &["remote", "add", "origin", bare.to_str().unwrap()],
        );
        let base_sha = {
            let out = Sh::new("git")
                .current_dir(&source)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };

        let run_dir = tmp.path().join("rundir");
        std::fs::create_dir_all(blackboard::blackboard_dir(&run_dir)).unwrap();

        // Spec: two commit-gated steps + a finalize push (no PR — no GitHub).
        let mut agents = BTreeMap::new();
        agents.insert(
            "coder".to_string(),
            super::super::spec::AgentSpec {
                base: "codex".to_string(),
                model: None,
                instructions: None,
                skills: vec![],
                custom_agent: None,
            },
        );
        let spec = Spec {
            version: 1,
            name: "demo".to_string(),
            description: None,
            budgets: None,
            agents,
            workflow: vec![Block::Step(step("plan")), Block::Step(step("build"))],
            finalize: Some(super::super::spec::Finalize {
                push: true,
                open_pr: false,
                pr_base: Some("main".to_string()),
            }),
        };
        let spec_json = serde_json::to_string(&spec).unwrap();

        let db = crate::database::init(tmp.path()).unwrap();
        let run_id = "run-demo";
        let branch = "wf/demo-abcdef12";
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO wf_run (id, name, spec_json, task, project_id, repo_path, run_dir,
                    branch, base_sha, status, budgets_json, spent_json, created_at, updated_at)
                 VALUES (?1,'demo',?2,'the task','p',?3,?4,?5,?6,'pending','{}','{}',0,0)",
                rusqlite::params![
                    run_id,
                    spec_json,
                    source.to_string_lossy(),
                    run_dir.to_string_lossy(),
                    branch,
                    base_sha,
                ],
            )
            .unwrap();
        }

        let driver = StubDriver::new(tmp.path().join("workspaces"), true);
        let ctx = RunCtx {
            db: db.clone(),
            driver,
            app: None,
            cancel: Arc::new(AtomicBool::new(false)),
            pending_ask: Arc::new(AtomicBool::new(false)),
            deadlines: Deadlines::default(),
        };
        drive_run(&ctx, run_id).await;

        // Run reached done.
        let status: String = db
            .lock()
            .query_row("SELECT status FROM wf_run WHERE id=?1", [run_id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(status, "done", "run should be done");

        // Two step attempts, both done.
        let done_count: i64 = db
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM wf_step_exec WHERE run_id=?1 AND status='done'",
                [run_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(done_count, 2, "both steps done");

        // The branch was pushed to the bare remote, two commits above base
        // (step 2 building on step 1).
        let pushed = Sh::new("git")
            .current_dir(&bare)
            .args(["rev-parse", &format!("refs/heads/{branch}")])
            .output()
            .unwrap();
        assert!(pushed.status.success(), "branch pushed");
        let count = Sh::new("git")
            .current_dir(&bare)
            .args(["rev-list", "--count", &format!("refs/heads/{branch}")])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&count.stdout).trim(),
            "3",
            "base + 2 step commits"
        );

        // A finalize_pushed event was journaled.
        let fin: i64 = db
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM wf_event WHERE run_id=?1 AND type=?2",
                rusqlite::params![run_id, event_type::FINALIZE_PUSHED],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fin, 1, "finalize_pushed journaled");
    }

    /// A single commit-gated step, no finalize — for the cancel / blocked tests.
    fn scaffold_one_step(tmp: &Path, run_id: &str, branch: &str) -> (Db, PathBuf) {
        let source = tmp.join("source");
        std::fs::create_dir_all(&source).unwrap();
        sh(&source, &["init", "-q", "-b", "main"]);
        sh(&source, &["config", "user.email", "t@t.t"]);
        sh(&source, &["config", "user.name", "t"]);
        std::fs::write(source.join("README"), "base").unwrap();
        sh(&source, &["add", "-A"]);
        sh(&source, &["commit", "-qm", "base"]);
        let base_sha = {
            let o = Sh::new("git")
                .current_dir(&source)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap();
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        let run_dir = tmp.join("rundir");
        std::fs::create_dir_all(blackboard::blackboard_dir(&run_dir)).unwrap();
        let mut agents = BTreeMap::new();
        agents.insert(
            "coder".to_string(),
            super::super::spec::AgentSpec {
                base: "codex".to_string(),
                model: None,
                instructions: None,
                skills: vec![],
                custom_agent: None,
            },
        );
        let spec = Spec {
            version: 1,
            name: "demo".to_string(),
            description: None,
            budgets: None,
            agents,
            workflow: vec![Block::Step(step("only"))],
            finalize: None,
        };
        let spec_json = serde_json::to_string(&spec).unwrap();
        let db = crate::database::init(tmp).unwrap();
        db.lock()
            .execute(
                "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                    base_sha,status,budgets_json,spent_json,created_at,updated_at)
                 VALUES (?1,'demo',?2,'t','p',?3,?4,?5,?6,'pending','{}','{}',0,0)",
                rusqlite::params![
                    run_id,
                    spec_json,
                    source.to_string_lossy(),
                    run_dir.to_string_lossy(),
                    branch,
                    base_sha,
                ],
            )
            .unwrap();
        (db, tmp.join("ws"))
    }

    #[tokio::test]
    async fn cancel_marks_run_canceled_and_runs_no_step() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, ws) = scaffold_one_step(tmp.path(), "run-cancel", "wf/c-1");
        let ctx = RunCtx {
            db: db.clone(),
            driver: StubDriver::new(ws, true),
            app: None,
            cancel: Arc::new(AtomicBool::new(true)), // pre-canceled
            pending_ask: Arc::new(AtomicBool::new(false)),
            deadlines: Deadlines::default(),
        };
        drive_run(&ctx, "run-cancel").await;
        let status: String = db
            .lock()
            .query_row("SELECT status FROM wf_run WHERE id='run-cancel'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(status, "canceled");
    }

    #[tokio::test]
    async fn terminal_run_is_not_redriven() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, ws) = scaffold_one_step(tmp.path(), "run-term", "wf/t-1");
        db.lock()
            .execute("UPDATE wf_run SET status='failed' WHERE id='run-term'", [])
            .unwrap();
        let ctx = RunCtx {
            db: db.clone(),
            driver: StubDriver::new(ws, true),
            app: None,
            cancel: Arc::new(AtomicBool::new(false)),
            pending_ask: Arc::new(AtomicBool::new(false)),
            deadlines: Deadlines::default(),
        };
        drive_run(&ctx, "run-term").await;
        let (status, execs): (String, i64) = db
            .lock()
            .query_row(
                "SELECT status, (SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-term')
                 FROM wf_run WHERE id='run-term'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "failed", "terminal status must be preserved");
        assert_eq!(execs, 0, "no step may run on a terminal run");
    }

    #[tokio::test]
    async fn unmet_commit_gate_pauses_blocked() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, ws) = scaffold_one_step(tmp.path(), "run-blocked", "wf/b-1");
        // commit=false → the agent makes no commit → the commit gate stays unmet
        // through the attempt's one re-prompt, so the run pauses `blocked_gate`.
        let ctx = RunCtx {
            db: db.clone(),
            driver: StubDriver::new(ws, false),
            app: None,
            cancel: Arc::new(AtomicBool::new(false)),
            pending_ask: Arc::new(AtomicBool::new(false)),
            deadlines: Deadlines::default(),
        };
        drive_run(&ctx, "run-blocked").await;
        let (status, reason): (String, Option<String>) = db
            .lock()
            .query_row(
                "SELECT status, paused_reason FROM wf_run WHERE id='run-blocked'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "paused");
        assert_eq!(reason.as_deref(), Some("blocked_gate"));
    }

    /// A stub whose "agent" raises the run's pending-ask flag on its very first
    /// turn (standing in for a `wf_ask` routed to the human) and commits on every
    /// later turn. It shares the run's `pending_ask` Arc and records every prompt
    /// it is sent, so the test can prove the deferral, the pause, and the
    /// answer-fold on resume.
    struct AskStub {
        root: PathBuf,
        db: Db,
        run_id: String,
        pending_ask: Arc<AtomicBool>,
        /// Whether turn 1 also raises the in-memory flag (fast path). `false`
        /// exercises the scheduler's DB backstop: the ask is persisted but the
        /// poke is "missed", and the run must still pause `question`.
        set_flag: bool,
        /// When set, the ask isn't persisted during the turn — it only becomes
        /// visible when `settle_rpc` drains the mailbox. Proves the scheduler
        /// drains *before* the backstop check (§10.4).
        persist_in_settle: bool,
        tx: broadcast::Sender<StatusEvent>,
        state: parking_lot::Mutex<AskStubState>,
    }
    #[derive(Default)]
    struct AskStubState {
        statuses: HashMap<String, AgentStatus>,
        worktrees: HashMap<String, PathBuf>,
        spawns: usize,
        turns: usize,
        prompts: Vec<String>,
        ask_persisted: bool,
    }
    impl AskStub {
        fn new(
            root: PathBuf,
            db: Db,
            run_id: &str,
            pending_ask: Arc<AtomicBool>,
            set_flag: bool,
            persist_in_settle: bool,
        ) -> Arc<Self> {
            Arc::new(Self {
                root,
                db,
                run_id: run_id.to_string(),
                pending_ask,
                set_flag,
                persist_in_settle,
                tx: broadcast::channel(256).0,
                state: parking_lot::Mutex::new(AskStubState::default()),
            })
        }
        /// Persist a queued ask against the run's live attempt (agent_id is still
        /// NULL mid-turn, so resolution is by run) — exactly as the router does.
        fn persist_ask(&self) {
            let mut st = self.state.lock();
            if st.ask_persisted {
                return;
            }
            st.ask_persisted = true;
            let conn = self.db.lock();
            let exec: String = conn
                .query_row(
                    "SELECT id FROM wf_step_exec WHERE run_id = ?1
                     AND status IN ('spawning','running','gating')
                     ORDER BY rowid DESC LIMIT 1",
                    [&self.run_id],
                    |r| r.get(0),
                )
                .unwrap();
            conn.execute(
                "INSERT INTO wf_message (id, run_id, from_step_exec_id, to_step_exec_id,
                    kind, body_json, status, created_at)
                 VALUES ('ask-msg-1', ?1, ?2, NULL, 'ask', '{\"question\":\"which db?\"}',
                    'queued', 0)",
                rusqlite::params![self.run_id, exec],
            )
            .unwrap();
        }
        fn set(&self, id: &str, s: AgentStatus) {
            self.state.lock().statuses.insert(id.to_string(), s.clone());
            let _ = self.tx.send(StatusEvent {
                agent_id: id.to_string(),
                status: s,
            });
        }
    }
    impl AgentDriver for AskStub {
        fn spawn(
            &self,
            req: SpawnReq,
        ) -> super::super::driver::BoxFuture<'_, Result<super::super::driver::SpawnedAgent>>
        {
            Box::pin(async move {
                let id = {
                    let mut st = self.state.lock();
                    st.spawns += 1;
                    format!("ask-{}", st.spawns)
                };
                let dest = self.root.join(&id);
                let base_ref = req.fork_base.clone().unwrap();
                let spec = crate::sandbox::provision::CheckoutSpec {
                    source_repo: &req.repo_path,
                    base_ref: &base_ref,
                    dest: &dest,
                };
                crate::sandbox::provision::provision_forking_run_repo(
                    &spec,
                    req.run_repo.as_ref().unwrap(),
                )
                .await?;
                sh(&dest, &["config", "user.email", "t@t.t"]);
                sh(&dest, &["config", "user.name", "t"]);
                self.state.lock().worktrees.insert(id.clone(), dest.clone());
                self.set(&id, AgentStatus::Idle);
                Ok(super::super::driver::SpawnedAgent {
                    agent_id: id,
                    worktree: dest,
                })
            })
        }
        fn status(&self, id: &str) -> Option<AgentStatus> {
            self.state.lock().statuses.get(id).cloned()
        }
        fn subscribe(&self) -> broadcast::Receiver<StatusEvent> {
            self.tx.subscribe()
        }
        fn send_message<'a>(
            &'a self,
            id: &'a str,
            text: String,
        ) -> super::super::driver::BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                let (turn, wt) = {
                    let mut st = self.state.lock();
                    st.turns += 1;
                    st.prompts.push(text);
                    (st.turns, st.worktrees.get(id).cloned().unwrap())
                };
                self.set(id, AgentStatus::Running);
                if turn == 1 {
                    // First turn: ask the human (defer the gate) — no commit.
                    // Unless the ask is deferred to `settle_rpc` (mailbox-drain
                    // test), persist it now and raise the poke only when
                    // `set_flag`; otherwise the DB backstop must catch it.
                    if !self.persist_in_settle {
                        self.persist_ask();
                        if self.set_flag {
                            self.pending_ask.store(true, Ordering::SeqCst);
                        }
                    }
                } else {
                    // Later turns: do the work so the commit gate is met.
                    std::fs::write(wt.join(format!("{id}.txt")), "work").unwrap();
                    sh(&wt, &["add", "-A"]);
                    sh(&wt, &["commit", "-qm", "work"]);
                }
                self.set(id, AgentStatus::Idle);
                Ok(())
            })
        }
        fn settle_rpc<'a>(&'a self, _id: &'a str) -> super::super::driver::BoxFuture<'a, ()> {
            Box::pin(async move {
                // Models the real drain: a wf_ask the agent wrote during the turn
                // is only dispatched (persisted) when the mailbox is settled.
                if self.persist_in_settle {
                    self.persist_ask();
                }
            })
        }
        fn stop<'a>(&'a self, _id: &'a str) -> super::super::driver::BoxFuture<'a, Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn archive<'a>(&'a self, _id: &'a str) -> super::super::driver::BoxFuture<'a, Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn last_activity(&self, _id: &str) -> Option<i64> {
            None
        }
        fn turn_usage(&self, _id: &str) -> Option<super::super::driver::TurnUsage> {
            None
        }
    }

    #[tokio::test]
    async fn ask_pauses_question_then_answer_resumes_to_done() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, ws) = scaffold_one_step(tmp.path(), "run-ask", "wf/ask-1");
        let pending_ask = Arc::new(AtomicBool::new(false));
        let driver = AskStub::new(ws, db.clone(), "run-ask", pending_ask.clone(), true, false);
        let ctx = RunCtx {
            db: db.clone(),
            driver: driver.clone(),
            app: None,
            cancel: Arc::new(AtomicBool::new(false)),
            pending_ask: pending_ask.clone(),
            deadlines: Deadlines::default(),
        };

        // ── First drive: the step asks; the run pauses `question`. ──
        drive_run(&ctx, "run-ask").await;
        let (status, reason): (String, Option<String>) = db
            .lock()
            .query_row(
                "SELECT status, paused_reason FROM wf_run WHERE id='run-ask'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "paused");
        assert_eq!(reason.as_deref(), Some("question"));

        // The asking attempt was abandoned, and its gate was never evaluated
        // (deferred, §10.4).
        let (exec_id, exec_status): (String, String) = db
            .lock()
            .query_row(
                "SELECT id, status FROM wf_step_exec WHERE run_id='run-ask' ORDER BY rowid LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(exec_status, "abandoned");
        let gates: i64 = db
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM wf_event WHERE run_id='run-ask' AND type='gate_evaluated'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            gates, 0,
            "gate must not be evaluated while an ask is pending"
        );

        // ── The human answers (queued for the asking step) and the flag clears,
        // mimicking the fresh RunHandle a real resume creates. ──
        db.lock()
            .execute(
                "INSERT INTO wf_message (id, run_id, from_step_exec_id, to_step_exec_id, kind,
                    body_json, status, created_at)
                 VALUES ('ans-1','run-ask',NULL,?1,'answer',?2,'queued',0)",
                rusqlite::params![exec_id, r#"{"text":"use Postgres"}"#],
            )
            .unwrap();
        pending_ask.store(false, Ordering::SeqCst);

        // ── Resume: a fresh attempt runs, the answer is folded into its prompt,
        // and the run completes. ──
        drive_run(&ctx, "run-ask").await;
        let status: String = db
            .lock()
            .query_row("SELECT status FROM wf_run WHERE id='run-ask'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(status, "done");

        // The answer reached the agent, coalesced into the resumed attempt's
        // single prompt.
        let prompts = driver.state.lock().prompts.clone();
        assert_eq!(prompts.len(), 2, "one ask turn + one resumed turn");
        assert!(
            prompts[1].contains("use Postgres"),
            "answer folded into resumed prompt: {}",
            prompts[1]
        );
        // The queued answer was marked delivered (not re-folded).
        let undelivered: i64 = db
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM wf_message WHERE id='ans-1' AND status='queued'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(undelivered, 0, "answer should be marked delivered");
    }

    #[tokio::test]
    async fn queued_ask_backstop_pauses_even_when_poke_is_missed() {
        // The in-memory pending-ask poke can be lost (the RPC op races the
        // driver's wind-down). The persisted ask is authoritative: even with the
        // flag never set, the scheduler must pause `question` rather than act on
        // the gate outcome (§10.4).
        let tmp = tempfile::tempdir().unwrap();
        let (db, ws) = scaffold_one_step(tmp.path(), "run-ask2", "wf/ask2-1");
        let pending_ask = Arc::new(AtomicBool::new(false));
        // set_flag = false → the ask is persisted, but the flag is never raised.
        let driver = AskStub::new(
            ws,
            db.clone(),
            "run-ask2",
            pending_ask.clone(),
            false,
            false,
        );
        let ctx = RunCtx {
            db: db.clone(),
            driver,
            app: None,
            cancel: Arc::new(AtomicBool::new(false)),
            pending_ask,
            deadlines: Deadlines::default(),
        };

        drive_run(&ctx, "run-ask2").await;

        let (status, reason): (String, Option<String>) = db
            .lock()
            .query_row(
                "SELECT status, paused_reason FROM wf_run WHERE id='run-ask2'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "paused");
        assert_eq!(
            reason.as_deref(),
            Some("question"),
            "the persisted ask must pause the run even though the poke was missed"
        );
        // No boundary commit was ferried — the gate outcome was not acted on.
        let commits: i64 = db
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM wf_event WHERE run_id='run-ask2' AND type='boundary_commit'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(commits, 0, "no ferry while an answer is outstanding");
    }

    #[tokio::test]
    async fn mailbox_drain_surfaces_a_late_ask_before_the_check() {
        // The tightest race: the agent wrote a wf_ask during its turn, but it is
        // still undispatched when the turn ends — it only becomes persisted when
        // the scheduler drains the mailbox (settle_rpc). If the scheduler checked
        // for a pending ask *without* draining first, it would miss it and act on
        // the gate. persist_in_settle models exactly that ordering.
        let tmp = tempfile::tempdir().unwrap();
        let (db, ws) = scaffold_one_step(tmp.path(), "run-ask3", "wf/ask3-1");
        let pending_ask = Arc::new(AtomicBool::new(false));
        // No in-turn persist, no flag — the ask surfaces only via settle_rpc.
        let driver = AskStub::new(ws, db.clone(), "run-ask3", pending_ask.clone(), false, true);
        let ctx = RunCtx {
            db: db.clone(),
            driver,
            app: None,
            cancel: Arc::new(AtomicBool::new(false)),
            pending_ask,
            deadlines: Deadlines::default(),
        };

        drive_run(&ctx, "run-ask3").await;

        let (status, reason): (String, Option<String>) = db
            .lock()
            .query_row(
                "SELECT status, paused_reason FROM wf_run WHERE id='run-ask3'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "paused");
        assert_eq!(
            reason.as_deref(),
            Some("question"),
            "draining the mailbox before the check must surface the late ask"
        );
    }

    #[tokio::test]
    async fn resume_abandons_a_stale_attempt_then_retries_to_done() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, ws) = scaffold_one_step(tmp.path(), "run-resume", "wf/r-1");
        // A prior driver died mid-attempt, leaving a non-terminal step_exec
        // (spec §6.4). Resume must abandon it and start a fresh attempt.
        db.lock()
            .execute(
                "INSERT INTO wf_step_exec (id, run_id, step_id, attempt, iteration, status,
                    gate_mode, agent_id)
                 VALUES ('exec-stale','run-resume','only',1,0,'running','commit','ghost')",
                [],
            )
            .unwrap();
        let ctx = RunCtx {
            db: db.clone(),
            driver: StubDriver::new(ws, true),
            app: None,
            cancel: Arc::new(AtomicBool::new(false)),
            pending_ask: Arc::new(AtomicBool::new(false)),
            deadlines: Deadlines::default(),
        };
        drive_run(&ctx, "run-resume").await;

        // The stale attempt was abandoned...
        let stale: String = db
            .lock()
            .query_row(
                "SELECT status FROM wf_step_exec WHERE id='exec-stale'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stale, "abandoned");
        // ...a fresh attempt ran to done and the run completed.
        let done: i64 = db
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-resume' AND status='done'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(done, 1);
        let status: String = db
            .lock()
            .query_row("SELECT status FROM wf_run WHERE id='run-resume'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(status, "done");
    }

    // ── budgets (spec §11.2) ─────────────────────────────────────────────────

    /// Scaffold a commit-gated linear run of `step_ids`, no finalize, with an
    /// explicit `budgets_json`. Mirrors `scaffold_one_step` but parametric.
    fn scaffold_steps(
        tmp: &Path,
        run_id: &str,
        branch: &str,
        step_ids: &[&str],
        budgets_json: &str,
    ) -> (Db, PathBuf) {
        let source = tmp.join("source");
        std::fs::create_dir_all(&source).unwrap();
        sh(&source, &["init", "-q", "-b", "main"]);
        sh(&source, &["config", "user.email", "t@t.t"]);
        sh(&source, &["config", "user.name", "t"]);
        std::fs::write(source.join("README"), "base").unwrap();
        sh(&source, &["add", "-A"]);
        sh(&source, &["commit", "-qm", "base"]);
        let base_sha = {
            let o = Sh::new("git")
                .current_dir(&source)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap();
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        let run_dir = tmp.join("rundir");
        std::fs::create_dir_all(blackboard::blackboard_dir(&run_dir)).unwrap();
        let mut agents = BTreeMap::new();
        agents.insert(
            "coder".to_string(),
            super::super::spec::AgentSpec {
                base: "codex".to_string(),
                model: None,
                instructions: None,
                skills: vec![],
                custom_agent: None,
            },
        );
        let spec = Spec {
            version: 1,
            name: "demo".to_string(),
            description: None,
            budgets: None,
            agents,
            workflow: step_ids.iter().map(|id| Block::Step(step(id))).collect(),
            finalize: None,
        };
        let spec_json = serde_json::to_string(&spec).unwrap();
        let db = crate::database::init(tmp).unwrap();
        db.lock()
            .execute(
                "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                    base_sha,status,budgets_json,spent_json,created_at,updated_at)
                 VALUES (?1,'demo',?2,'t','p',?3,?4,?5,?6,'pending',?7,'{}',0,0)",
                rusqlite::params![
                    run_id,
                    spec_json,
                    source.to_string_lossy(),
                    run_dir.to_string_lossy(),
                    branch,
                    base_sha,
                    budgets_json,
                ],
            )
            .unwrap();
        (db, tmp.join("ws"))
    }

    fn eff_json(turns: i64) -> String {
        serde_json::to_string(&EffectiveBudgets {
            turns,
            ..Default::default()
        })
        .unwrap()
    }

    #[tokio::test]
    async fn zero_turn_budget_pauses_before_any_spawn() {
        // Enforcement point: before every spawn (§11.2). A run with no turn
        // budget pauses at the block boundary, having spawned nothing.
        let tmp = tempfile::tempdir().unwrap();
        let (db, ws) = scaffold_steps(tmp.path(), "run-b0", "wf/b0", &["only"], &eff_json(0));
        let ctx = RunCtx {
            db: db.clone(),
            driver: StubDriver::new(ws, true),
            app: None,
            cancel: Arc::new(AtomicBool::new(false)),
            pending_ask: Arc::new(AtomicBool::new(false)),
            deadlines: Deadlines::default(),
        };
        drive_run(&ctx, "run-b0").await;

        let (status, reason): (String, Option<String>) = db
            .lock()
            .query_row(
                "SELECT status, paused_reason FROM wf_run WHERE id='run-b0'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "paused");
        assert_eq!(reason.as_deref(), Some("budget_exceeded"));
        let execs: i64 = db
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-b0'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(execs, 0, "no attempt was spawned");
    }

    #[tokio::test]
    async fn budget_exceeded_pauses_then_resume_with_patch_completes() {
        // A turn budget of 1 lets step 1's turn run and be counted, then trips
        // the turn-end enforcement point and pauses the two-step run. A resume
        // with a budget patch (simulating `wf_resume(budget_patch)`) lifts the
        // cap and the run drives to done from the paused position.
        let tmp = tempfile::tempdir().unwrap();
        let (db, ws) = scaffold_steps(tmp.path(), "run-b1", "wf/b1", &["s1", "s2"], &eff_json(1));
        let ctx = RunCtx {
            db: db.clone(),
            driver: StubDriver::new(ws, true),
            app: None,
            cancel: Arc::new(AtomicBool::new(false)),
            pending_ask: Arc::new(AtomicBool::new(false)),
            deadlines: Deadlines::default(),
        };
        drive_run(&ctx, "run-b1").await;

        // Paused for budget, one turn spent, a budget_exceeded event journaled.
        let (status, reason): (String, Option<String>) = db
            .lock()
            .query_row(
                "SELECT status, paused_reason FROM wf_run WHERE id='run-b1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "paused");
        assert_eq!(reason.as_deref(), Some("budget_exceeded"));
        let spent: String = db
            .lock()
            .query_row("SELECT spent_json FROM wf_run WHERE id='run-b1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let ledger = Ledger::from_json(&serde_json::from_str(&spent).unwrap());
        assert_eq!(ledger.turns, 1, "one turn charged before the pause");
        let exceeded: i64 = db
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM wf_event WHERE run_id='run-b1' AND type=?1",
                [event_type::BUDGET_EXCEEDED],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(exceeded, 1, "budget_exceeded journaled");

        // Resume with +10 turns (what `wf_resume`'s patch does), then re-drive.
        {
            let conn = db.lock();
            let bj: String = conn
                .query_row(
                    "SELECT budgets_json FROM wf_run WHERE id='run-b1'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let mut e: EffectiveBudgets = serde_json::from_str(&bj).unwrap();
            e.apply_patch(&Budgets {
                turns: Some(10),
                tokens: None,
                wall_clock_mins: None,
                turns_per_attempt: None,
                max_attempts: None,
                spawn_timeout_secs: None,
                turn_start_timeout_secs: None,
                stall_timeout_secs: None,
                nudge_timeout_secs: None,
                tests_timeout_secs: None,
            });
            conn.execute(
                "UPDATE wf_run SET budgets_json=?1 WHERE id='run-b1'",
                [serde_json::to_string(&e).unwrap()],
            )
            .unwrap();
        }
        drive_run(&ctx, "run-b1").await;

        let status: String = db
            .lock()
            .query_row("SELECT status FROM wf_run WHERE id='run-b1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(status, "done", "resume-with-patch drove the run to done");
        let done: i64 = db
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-b1' AND status='done'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(done, 2, "both steps completed after the patch");
    }

    #[test]
    fn check_resumable_gates_resume_and_retry() {
        // The guard runs before `resume` applies any budget patch, so a rejected
        // resume leaves state untouched. Terminal and approval-paused runs are
        // rejected; resumable pauses pass.
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::database::init(tmp.path()).unwrap();
        let insert = |id: &str, status: &str, reason: Option<&str>| {
            db.lock()
                .execute(
                    "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,
                        branch,base_sha,status,paused_reason,budgets_json,spent_json,
                        created_at,updated_at)
                     VALUES (?1,'n','{}','t','p','/r','/d','wf/x','sha',?2,?3,'{}','{}',0,0)",
                    rusqlite::params![id, status, reason],
                )
                .unwrap();
        };
        insert("r-done", "done", None);
        insert("r-appr", "paused", Some("approval"));
        insert("r-ques", "paused", Some("question"));
        insert("r-budg", "paused", Some("budget_exceeded"));
        insert("r-blk", "paused", Some("blocked_gate"));

        let conn = db.lock();
        assert!(check_resumable(&conn, "r-done", "resume").is_err());
        assert!(check_resumable(&conn, "r-appr", "resume").is_err());
        // A question-paused run must go through `wf_answer`, not a bare resume —
        // otherwise the step re-runs with no human response folded in (§10.4).
        assert!(check_resumable(&conn, "r-ques", "resume").is_err());
        assert!(check_resumable(&conn, "r-budg", "resume").is_ok());
        assert!(check_resumable(&conn, "r-blk", "retry").is_ok());
    }

    #[test]
    fn commit_done_unless_ask_ties_the_gate_to_the_ask_check() {
        // The atomic commit point (§10.4): finalize `done` only when no ask is
        // queued for the exec; a pending ask blocks the commit so the caller can
        // pause `question` instead of advancing.
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::database::init(tmp.path()).unwrap();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                base_sha,status,budgets_json,spent_json,created_at,updated_at)
             VALUES ('r','n','{}','t','p','/r','/d','wf/x','sha','running','{}','{}',0,0)",
            [],
        )
        .unwrap();
        let mk_exec = |id: &str| {
            conn.execute(
                "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode)
                 VALUES (?1,'r','s',1,0,'running','verdict')",
                [id],
            )
            .unwrap();
        };

        // No ask → commits `done` with the ferried head.
        mk_exec("e-clean");
        assert!(commit_done_unless_ask(&conn, "e-clean", "sha1"));
        let (status, head): (String, Option<String>) = conn
            .query_row(
                "SELECT status, head_end FROM wf_step_exec WHERE id='e-clean'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "done");
        assert_eq!(head.as_deref(), Some("sha1"));

        // A queued ask against the exec → does NOT commit; the exec stays live so
        // the caller pauses `question`.
        mk_exec("e-ask");
        conn.execute(
            "INSERT INTO wf_message (id,run_id,from_step_exec_id,to_step_exec_id,kind,
                body_json,status,created_at)
             VALUES ('m1','r','e-ask',NULL,'ask','{}','queued',0)",
            [],
        )
        .unwrap();
        assert!(!commit_done_unless_ask(&conn, "e-ask", "sha2"));
        let status: String = conn
            .query_row(
                "SELECT status FROM wf_step_exec WHERE id='e-ask'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            status, "running",
            "must not finalize while an ask is pending"
        );
    }

    // ───────────────────────── parallel stages (S8) ─────────────────────────

    /// Markers embedded in a child's goal so [`MatrixDriver`] can script its
    /// per-child behaviour off the (goal-bearing) step prompt.
    const FAIL: &str = "PZFAIL";
    const HANG: &str = "PZHANG";
    /// A child that creates the *same* file as its siblings so the second merge
    /// of an `integrate: merge` stage conflicts (add/add). §12.3.
    const CONFLICT: &str = "PZCONFLICT";
    /// The shared file `CONFLICT` children (and the resolver) all touch.
    const CONFLICT_FILE: &str = "conflict.txt";

    #[derive(Clone, Copy)]
    enum Beh {
        Success,
        Fail,
        Hang,
        /// Write `CONFLICT_FILE` with unique content so siblings collide.
        Conflict,
        /// A conflict-resolution step: overwrite `CONFLICT_FILE` to a single
        /// resolved value (removing the markers) and commit.
        Resolve,
    }

    /// A real-git stub like [`StubDriver`] with per-child behaviour keyed off the
    /// step goal: a child whose goal contains `PZFAIL` runs turns but never
    /// commits (its `commit` gate stays unmet → failure); `PZHANG` starts a turn
    /// and never ends it (until the stage cancels it); anything else commits
    /// (success, moving HEAD → `integrate_skipped`).
    struct MatrixDriver {
        root: PathBuf,
        tx: broadcast::Sender<StatusEvent>,
        state: parking_lot::Mutex<MatrixState>,
    }
    #[derive(Default)]
    struct MatrixState {
        statuses: HashMap<String, AgentStatus>,
        worktrees: HashMap<String, PathBuf>,
        /// Behaviour fixed on the agent's first prompt (the step prompt carries
        /// the goal marker; a later reprompt does not, so it must not re-derive).
        behavior: HashMap<String, Beh>,
        archived: Vec<String>,
        stopped: Vec<String>,
        count: usize,
    }
    impl MatrixDriver {
        fn new(root: PathBuf) -> Arc<Self> {
            Arc::new(Self {
                root,
                tx: broadcast::channel(256).0,
                state: parking_lot::Mutex::new(MatrixState::default()),
            })
        }
        fn set(&self, id: &str, s: AgentStatus) {
            self.state.lock().statuses.insert(id.to_string(), s.clone());
            let _ = self.tx.send(StatusEvent {
                agent_id: id.to_string(),
                status: s,
            });
        }
    }
    impl AgentDriver for MatrixDriver {
        fn spawn(
            &self,
            req: SpawnReq,
        ) -> super::super::driver::BoxFuture<'_, Result<super::super::driver::SpawnedAgent>>
        {
            Box::pin(async move {
                let id = {
                    let mut st = self.state.lock();
                    st.count += 1;
                    format!("m-{}", st.count)
                };
                let dest = self.root.join(&id);
                let base_ref = req.fork_base.clone().unwrap();
                let spec = crate::sandbox::provision::CheckoutSpec {
                    source_repo: &req.repo_path,
                    base_ref: &base_ref,
                    dest: &dest,
                };
                crate::sandbox::provision::provision_forking_run_repo(
                    &spec,
                    req.run_repo.as_ref().unwrap(),
                )
                .await?;
                sh(&dest, &["config", "user.email", "t@t.t"]);
                sh(&dest, &["config", "user.name", "t"]);
                self.state.lock().worktrees.insert(id.clone(), dest.clone());
                self.set(&id, AgentStatus::Idle);
                Ok(super::super::driver::SpawnedAgent {
                    agent_id: id,
                    worktree: dest,
                })
            })
        }
        fn status(&self, id: &str) -> Option<AgentStatus> {
            self.state.lock().statuses.get(id).cloned()
        }
        fn subscribe(&self) -> broadcast::Receiver<StatusEvent> {
            self.tx.subscribe()
        }
        fn send_message<'a>(
            &'a self,
            id: &'a str,
            text: String,
        ) -> super::super::driver::BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                let (wt, beh) = {
                    let mut st = self.state.lock();
                    let wt = st.worktrees.get(id).cloned().unwrap();
                    let beh = *st.behavior.entry(id.to_string()).or_insert_with(|| {
                        // The resolution step's prompt names conflict markers; a
                        // `CONFLICT` child creates the shared file; others follow
                        // their goal marker.
                        if text.contains("conflict marker") {
                            Beh::Resolve
                        } else if text.contains(CONFLICT) {
                            Beh::Conflict
                        } else if text.contains(HANG) {
                            Beh::Hang
                        } else if text.contains(FAIL) {
                            Beh::Fail
                        } else {
                            Beh::Success
                        }
                    });
                    (wt, beh)
                };
                self.set(id, AgentStatus::Running);
                match beh {
                    Beh::Hang => return Ok(()), // turn never ends — only a cancel unblocks it
                    Beh::Fail => {}             // no commit → commit gate stays unmet
                    Beh::Success => {
                        std::fs::write(wt.join(format!("{id}.txt")), "work").unwrap();
                        sh(&wt, &["add", "-A"]);
                        sh(&wt, &["commit", "-qm", "child work"]);
                    }
                    Beh::Conflict => {
                        // Unique content in a shared file → add/add conflict when a
                        // sibling's ref is merged after this one.
                        std::fs::write(wt.join(CONFLICT_FILE), format!("from {id}\n")).unwrap();
                        sh(&wt, &["add", "-A"]);
                        sh(&wt, &["commit", "-qm", "conflicting work"]);
                    }
                    Beh::Resolve => {
                        // Overwrite the conflicted file with a single resolved value
                        // (markers gone) and commit — satisfies the `commit` gate.
                        std::fs::write(wt.join(CONFLICT_FILE), "resolved\n").unwrap();
                        sh(&wt, &["add", "-A"]);
                        sh(&wt, &["commit", "-qm", "resolve conflict"]);
                    }
                }
                self.set(id, AgentStatus::Idle);
                Ok(())
            })
        }
        fn stop<'a>(&'a self, id: &'a str) -> super::super::driver::BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                self.state.lock().stopped.push(id.to_string());
                Ok(())
            })
        }
        fn archive<'a>(&'a self, id: &'a str) -> super::super::driver::BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                self.state.lock().archived.push(id.to_string());
                Ok(())
            })
        }
        fn last_activity(&self, _id: &str) -> Option<i64> {
            None
        }
        fn turn_usage(&self, _id: &str) -> Option<super::super::driver::TurnUsage> {
            None
        }
    }

    /// A commit-gated parallel child; `marker` (`""` / `FAIL` / `HANG`) selects
    /// the driver behaviour.
    fn cstep(id: &str, marker: &str) -> Step {
        Step {
            id: id.to_string(),
            agent: "coder".to_string(),
            goal: format!("child {id} {marker}"),
            gate: Gate::Commit,
            budgets: None,
            comms: vec![],
        }
    }

    /// A run whose whole workflow is one `parallel { integrate: none }` block.
    /// Returns the db, the workspace root for the driver, and the base SHA.
    fn scaffold_parallel(
        tmp: &Path,
        run_id: &str,
        join: Join,
        children: &[Step],
    ) -> (Db, PathBuf, String) {
        scaffold_parallel_integrate(tmp, run_id, join, Integrate::None, children)
    }

    #[allow(clippy::too_many_lines)]
    fn scaffold_parallel_integrate(
        tmp: &Path,
        run_id: &str,
        join: Join,
        integrate: Integrate,
        children: &[Step],
    ) -> (Db, PathBuf, String) {
        let source = tmp.join(format!("src-{run_id}"));
        std::fs::create_dir_all(&source).unwrap();
        sh(&source, &["init", "-q", "-b", "main"]);
        sh(&source, &["config", "user.email", "t@t.t"]);
        sh(&source, &["config", "user.name", "t"]);
        std::fs::write(source.join("README"), "base").unwrap();
        sh(&source, &["add", "-A"]);
        sh(&source, &["commit", "-qm", "base"]);
        let base_sha = {
            let o = Sh::new("git")
                .current_dir(&source)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap();
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        let run_dir = tmp.join(format!("rd-{run_id}"));
        std::fs::create_dir_all(blackboard::blackboard_dir(&run_dir)).unwrap();
        let mut agents = BTreeMap::new();
        agents.insert(
            "coder".to_string(),
            super::super::spec::AgentSpec {
                base: "codex".to_string(),
                model: None,
                instructions: None,
                skills: vec![],
                custom_agent: None,
            },
        );
        let spec = Spec {
            version: 1,
            name: "par".to_string(),
            description: None,
            budgets: None,
            agents,
            workflow: vec![Block::Parallel(Parallel {
                join,
                integrate,
                max_concurrent: None,
                steps: children.to_vec(),
            })],
            finalize: None,
        };
        let spec_json = serde_json::to_string(&spec).unwrap();
        let db = crate::database::init(tmp).unwrap();
        db.lock()
            .execute(
                "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                    base_sha,status,budgets_json,spent_json,created_at,updated_at)
                 VALUES (?1,'par',?2,'t','p',?3,?4,'wf/par-x',?5,'pending','{}','{}',0,0)",
                rusqlite::params![
                    run_id,
                    spec_json,
                    source.to_string_lossy(),
                    run_dir.to_string_lossy(),
                    base_sha,
                ],
            )
            .unwrap();
        (db, tmp.join(format!("ws-{run_id}")), base_sha)
    }

    fn par_ctx(db: Db, driver: Arc<MatrixDriver>) -> RunCtx {
        RunCtx {
            db,
            driver,
            app: None,
            cancel: Arc::new(AtomicBool::new(false)),
            pending_ask: Arc::new(AtomicBool::new(false)),
            deadlines: Deadlines::default(),
        }
    }

    fn run_status_str(db: &Db, run_id: &str) -> String {
        db.lock()
            .query_row("SELECT status FROM wf_run WHERE id=?1", [run_id], |r| {
                r.get(0)
            })
            .unwrap()
    }

    fn count_children(db: &Db, run_id: &str, status: &str) -> i64 {
        db.lock()
            .query_row(
                "SELECT COUNT(*) FROM wf_step_exec WHERE run_id=?1 AND status=?2",
                rusqlite::params![run_id, status],
                |r| r.get(0),
            )
            .unwrap()
    }

    #[tokio::test]
    async fn parallel_all_success_reaches_done_and_journals_integrate_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let children = vec![cstep("a", ""), cstep("b", ""), cstep("c", "")];
        let (db, ws, _base) = scaffold_parallel(tmp.path(), "run-pa", Join::All, &children);
        let ctx = par_ctx(db.clone(), MatrixDriver::new(ws));
        drive_run(&ctx, "run-pa").await;

        assert_eq!(run_status_str(&db, "run-pa"), "done");
        assert_eq!(count_children(&db, "run-pa", "done"), 3, "every child done");
        // `integrate: none` — each committing child left its work on its fork.
        let skipped: i64 = db
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM wf_event WHERE run_id='run-pa' AND type=?1",
                [event_type::INTEGRATE_SKIPPED],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(skipped, 3, "one integrate_skipped per committing child");
    }

    #[tokio::test]
    async fn parallel_all_fails_when_a_child_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let children = vec![cstep("ok", ""), cstep("bad", FAIL)];
        let (db, ws, _b) = scaffold_parallel(tmp.path(), "run-af", Join::All, &children);
        let ctx = par_ctx(db.clone(), MatrixDriver::new(ws));
        drive_run(&ctx, "run-af").await;

        let (status, err): (String, Option<String>) = db
            .lock()
            .query_row(
                "SELECT status, error FROM wf_run WHERE id='run-af'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "failed");
        assert!(
            err.unwrap_or_default().contains("parallel stage failed"),
            "failure names its cause"
        );
    }

    #[tokio::test]
    async fn parallel_any_first_success_wins_and_cancels_the_loser() {
        let tmp = tempfile::tempdir().unwrap();
        // One fast success + one hanging child; `any` → success wins and the
        // hanging loser is cancelled + archived (§6.6).
        let children = vec![cstep("win", ""), cstep("slow", HANG)];
        let (db, ws, _b) = scaffold_parallel(tmp.path(), "run-any", Join::Any, &children);
        let driver = MatrixDriver::new(ws);
        let ctx = par_ctx(db.clone(), driver.clone());
        drive_run(&ctx, "run-any").await;

        assert_eq!(run_status_str(&db, "run-any"), "done");
        assert_eq!(count_children(&db, "run-any", "done"), 1, "one winner");
        assert_eq!(count_children(&db, "run-any", "abandoned"), 1, "one loser");

        // The loser was stopped and archived (its chat stays replayable) — the
        // spawn-race fix guarantees the agent id was known when it was cancelled.
        let loser: Option<String> = db
            .lock()
            .query_row(
                "SELECT agent_id FROM wf_step_exec
                 WHERE run_id='run-any' AND status='abandoned'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let loser = loser.expect("cancelled loser has an agent id");
        assert!(
            driver.state.lock().stopped.contains(&loser),
            "loser stopped"
        );
        assert!(
            driver.state.lock().archived.contains(&loser),
            "loser archived"
        );
    }

    #[tokio::test]
    async fn parallel_any_fails_only_when_all_children_fail() {
        let tmp = tempfile::tempdir().unwrap();
        let children = vec![cstep("x", FAIL), cstep("y", FAIL)];
        let (db, ws, _b) = scaffold_parallel(tmp.path(), "run-anf", Join::Any, &children);
        let ctx = par_ctx(db.clone(), MatrixDriver::new(ws));
        drive_run(&ctx, "run-anf").await;

        let (status, err): (String, Option<String>) = db
            .lock()
            .query_row(
                "SELECT status, error FROM wf_run WHERE id='run-anf'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "failed");
        assert!(err
            .unwrap_or_default()
            .contains("all parallel children failed"));
    }

    #[tokio::test]
    async fn resume_parallel_redrives_only_unfinished_children() {
        let tmp = tempfile::tempdir().unwrap();
        // A prior drive finished `done_child` before dying; resume must not
        // re-run it and must drive the remaining child to done (§12.3 / S8).
        let children = vec![cstep("done_child", ""), cstep("todo_child", "")];
        let (db, ws, _b) = scaffold_parallel(tmp.path(), "run-rp", Join::All, &children);
        db.lock()
            .execute("UPDATE wf_run SET status='running' WHERE id='run-rp'", [])
            .unwrap();
        db.lock()
            .execute(
                "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode,head_end)
                 VALUES ('exec-prior','run-rp','done_child',1,0,'done','commit','deadbeef')",
                [],
            )
            .unwrap();
        let ctx = par_ctx(db.clone(), MatrixDriver::new(ws));
        drive_run(&ctx, "run-rp").await;

        let done_child_execs: i64 = db
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-rp' AND step_id='done_child'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            done_child_execs, 1,
            "the done child must not be re-executed"
        );
        let todo_done: i64 = db
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM wf_step_exec
                 WHERE run_id='run-rp' AND step_id='todo_child' AND status='done'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(todo_done, 1, "the unfinished child ran to done");
        assert_eq!(run_status_str(&db, "run-rp"), "done");
    }

    // ──────────────────── code-producing parallel: merge (S9) ───────────────

    /// `pick_winners`: `all` keeps every child in spec order; `any` uses the live
    /// winner hint when present, else the earliest finisher (ties → spec order).
    #[test]
    fn pick_winners_selects_the_right_branch() {
        // (step_id, ref, ended_at) in spec order: a is spec-first, b finished first.
        let done = || {
            vec![
                ("a".to_string(), "ref-a".to_string(), 200),
                ("b".to_string(), "ref-b".to_string(), 100),
            ]
        };

        // `all` → every child, spec order, untouched.
        assert_eq!(
            pick_winners(done(), Join::All, None),
            vec![
                ("a".to_string(), "ref-a".to_string()),
                ("b".to_string(), "ref-b".to_string())
            ]
        );

        // `any` + live winner hint → exactly that child, even if spec-later.
        assert_eq!(
            pick_winners(done(), Join::Any, Some("b")),
            vec![("b".to_string(), "ref-b".to_string())]
        );

        // `any`, no hint (resume) → earliest finisher (b @100), not spec-first (a).
        assert_eq!(
            pick_winners(done(), Join::Any, None),
            vec![("b".to_string(), "ref-b".to_string())]
        );

        // `any`, no hint, tied ended_at → stable fallback to spec order (a).
        let tied = vec![
            ("a".to_string(), "ref-a".to_string(), 100),
            ("b".to_string(), "ref-b".to_string(), 100),
        ];
        assert_eq!(
            pick_winners(tied, Join::Any, None),
            vec![("a".to_string(), "ref-a".to_string())]
        );
    }

    fn sh_out(dir: &Path, args: &[&str]) -> String {
        let out = Sh::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .expect("git");
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    fn count_events(db: &Db, run_id: &str, ty: &str) -> i64 {
        db.lock()
            .query_row(
                "SELECT COUNT(*) FROM wf_event WHERE run_id=?1 AND type=?2",
                rusqlite::params![run_id, ty],
                |r| r.get(0),
            )
            .unwrap()
    }

    /// Record a resolution choice on the paused merge cursor — what
    /// `wf_resolve_conflict` does, exercised directly so the test can also stage
    /// the human's edit before re-driving.
    fn set_resolution(db: &Db, run_id: &str, mode: &str) {
        let conn = db.lock();
        let mut cur = get_cursor(&conn, run_id);
        cur.merge
            .as_mut()
            .unwrap()
            .conflict
            .as_mut()
            .unwrap()
            .resolution = Some(mode.to_string());
        set_cursor(&conn, run_id, &cur);
    }

    /// The tree of the merge stage's integrated result, as a newline-joined file
    /// list, read from the run repo (§12.1).
    fn merge_tree(tmp: &Path, db: &Db, run_id: &str) -> String {
        let run_repo = tmp.join(format!("rd-{run_id}")).join("repo");
        let merge_ref = {
            let conn = db.lock();
            gitops::step_ref(&latest_done_exec_for_step(&conn, run_id, &merge_step_id(0)).unwrap())
        };
        sh_out(&run_repo, &["ls-tree", "--name-only", "-r", &merge_ref])
    }

    /// §16: clean merges in spec order integrate every child's work and the run
    /// advances onto the merged result.
    #[tokio::test]
    async fn merge_stage_integrates_children_and_reaches_done() {
        let tmp = tempfile::tempdir().unwrap();
        // Disjoint files → two clean merges.
        let children = vec![cstep("a", ""), cstep("b", "")];
        let (db, ws, _b) = scaffold_parallel_integrate(
            tmp.path(),
            "run-mg",
            Join::All,
            Integrate::Merge,
            &children,
        );
        let ctx = par_ctx(db.clone(), MatrixDriver::new(ws));
        drive_run(&ctx, "run-mg").await;

        assert_eq!(run_status_str(&db, "run-mg"), "done");
        assert_eq!(
            count_events(&db, "run-mg", event_type::MERGE_DONE),
            2,
            "one merge_done per child, in spec order"
        );
        let files = merge_tree(tmp.path(), &db, "run-mg");
        assert!(
            files.contains("m-1.txt") && files.contains("m-2.txt"),
            "both children's work is present in the integrated tree: {files}"
        );
    }

    /// §16: an induced conflict pauses the run `conflict` and names the file.
    #[tokio::test]
    async fn merge_conflict_pauses_with_file_list() {
        let tmp = tempfile::tempdir().unwrap();
        let children = vec![cstep("a", CONFLICT), cstep("b", CONFLICT)];
        let (db, ws, _b) = scaffold_parallel_integrate(
            tmp.path(),
            "run-mc",
            Join::All,
            Integrate::Merge,
            &children,
        );
        let ctx = par_ctx(db.clone(), MatrixDriver::new(ws));
        drive_run(&ctx, "run-mc").await;

        let (status, reason): (String, Option<String>) = db
            .lock()
            .query_row(
                "SELECT status, paused_reason FROM wf_run WHERE id='run-mc'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "paused");
        assert_eq!(reason.as_deref(), Some("conflict"));

        let payload: String = db
            .lock()
            .query_row(
                "SELECT payload_json FROM wf_event WHERE run_id='run-mc' AND type=?1",
                [event_type::MERGE_CONFLICT],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            payload.contains(CONFLICT_FILE),
            "the conflict names its file: {payload}"
        );
        // The resumable conflict state is persisted on the cursor.
        let cur = get_cursor(&db.lock(), "run-mc");
        assert!(
            cur.merge
                .as_ref()
                .and_then(|m| m.conflict.as_ref())
                .is_some(),
            "conflict recorded for resume"
        );
    }

    /// §16 mode (a): an agent conflict-resolution step drives the run to done.
    #[tokio::test]
    async fn merge_conflict_resolved_by_agent_reaches_done() {
        let tmp = tempfile::tempdir().unwrap();
        let children = vec![cstep("a", CONFLICT), cstep("b", CONFLICT)];
        let (db, ws, _b) = scaffold_parallel_integrate(
            tmp.path(),
            "run-ma",
            Join::All,
            Integrate::Merge,
            &children,
        );
        let ctx = par_ctx(db.clone(), MatrixDriver::new(ws));
        drive_run(&ctx, "run-ma").await;
        assert_eq!(run_status_str(&db, "run-ma"), "paused");

        // `wf_resolve_conflict(run, "agent")` then re-drive.
        set_resolution(&db, "run-ma", "agent");
        drive_run(&ctx, "run-ma").await;

        assert_eq!(run_status_str(&db, "run-ma"), "done");
        // The resolution step ran (its `__resolve_0` exec is done) and the
        // integrated file carries the resolved value, not markers.
        let resolved: i64 = db
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM wf_step_exec
                 WHERE run_id='run-ma' AND step_id='__resolve_0' AND status='done'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(resolved, 1, "the conflict-resolution step ran to done");
        let run_repo = tmp.path().join("rd-run-ma").join("repo");
        let merge_ref = {
            let conn = db.lock();
            gitops::step_ref(
                &latest_done_exec_for_step(&conn, "run-ma", &merge_step_id(0)).unwrap(),
            )
        };
        let body = sh_out(
            &run_repo,
            &["show", &format!("{merge_ref}:{CONFLICT_FILE}")],
        );
        assert!(body.contains("resolved"), "markers resolved: {body}");
        assert!(!body.contains("<<<<<<<"), "no leftover markers: {body}");
    }

    /// §16 mode (c): the human resolves in the integration worktree and the run
    /// resumes to done.
    #[tokio::test]
    async fn merge_conflict_resolved_by_human_reaches_done() {
        let tmp = tempfile::tempdir().unwrap();
        let children = vec![cstep("a", CONFLICT), cstep("b", CONFLICT)];
        let (db, ws, _b) = scaffold_parallel_integrate(
            tmp.path(),
            "run-mh",
            Join::All,
            Integrate::Merge,
            &children,
        );
        let ctx = par_ctx(db.clone(), MatrixDriver::new(ws));
        drive_run(&ctx, "run-mh").await;
        assert_eq!(run_status_str(&db, "run-mh"), "paused");

        // The human resolves in the run repo's integration worktree and commits.
        let int_wt = tmp.path().join("rd-run-mh").join("integrate-0");
        std::fs::write(int_wt.join(CONFLICT_FILE), "human-resolved\n").unwrap();
        sh(&int_wt, &["add", "-A"]);
        sh(&int_wt, &["commit", "-qm", "human resolution"]);

        // `wf_resolve_conflict(run, "human")` then re-drive.
        set_resolution(&db, "run-mh", "human");
        drive_run(&ctx, "run-mh").await;

        assert_eq!(run_status_str(&db, "run-mh"), "done");
        let run_repo = tmp.path().join("rd-run-mh").join("repo");
        let merge_ref = {
            let conn = db.lock();
            gitops::step_ref(
                &latest_done_exec_for_step(&conn, "run-mh", &merge_step_id(0)).unwrap(),
            )
        };
        let body = sh_out(
            &run_repo,
            &["show", &format!("{merge_ref}:{CONFLICT_FILE}")],
        );
        assert!(
            body.contains("human-resolved"),
            "human resolution integrated: {body}"
        );
    }

    /// A slow sibling can race past `any`'s stage-cancel and land its own `done`
    /// exec. The merge must integrate exactly ONE branch — the child that
    /// FINISHED FIRST, not the first in spec order. Pre-seed both children `done`
    /// with `b` (spec-second) finishing *before* `a` (spec-first), then assert the
    /// integrated tree carries `b`'s work and drops `a`'s.
    #[tokio::test]
    async fn merge_any_integrates_the_child_that_finished_first() {
        let tmp = tempfile::tempdir().unwrap();
        let children = vec![cstep("a", ""), cstep("b", "")];
        let (db, _ws, _base) = scaffold_parallel_integrate(
            tmp.path(),
            "run-ma1",
            Join::Any,
            Integrate::Merge,
            &children,
        );

        // Provision the run repo and ferry two real child commits into it, then
        // mark both children `done` — the raced state a live `any` stage can leave
        // behind. `b` finished first (smaller `ended_at`) so `b` is the winner,
        // even though `a` is earlier in spec order.
        let source = tmp.path().join("src-run-ma1");
        let run_dir = tmp.path().join("rd-run-ma1");
        let run_repo = gitops::provision_run_repo(&source, &run_dir).await.unwrap();
        for (child, exec, ended) in [("a", "exec-a", 200_i64), ("b", "exec-b", 100_i64)] {
            let ws = tmp.path().join(format!("wsx-{child}"));
            sh(
                tmp.path(),
                &[
                    "clone",
                    "-q",
                    "--shared",
                    source.to_str().unwrap(),
                    ws.to_str().unwrap(),
                ],
            );
            sh(&ws, &["config", "user.email", "t@t.t"]);
            sh(&ws, &["config", "user.name", "t"]);
            std::fs::write(ws.join(format!("{child}.txt")), "work").unwrap();
            gitops::boundary_commit(&ws, "child").await.unwrap();
            let r = gitops::pin_step_ref(&ws, exec).await.unwrap();
            gitops::ferry(&ws, &run_repo, &r).await.unwrap();
            db.lock()
                .execute(
                    "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode,head_end,ended_at)
                     VALUES (?1,'run-ma1',?2,1,0,'done','commit','x',?3)",
                    rusqlite::params![exec, child, ended],
                )
                .unwrap();
        }
        db.lock()
            .execute("UPDATE wf_run SET status='running' WHERE id='run-ma1'", [])
            .unwrap();

        let ctx = par_ctx(db.clone(), MatrixDriver::new(tmp.path().join("ws-run-ma1")));
        drive_run(&ctx, "run-ma1").await;

        assert_eq!(run_status_str(&db, "run-ma1"), "done");
        assert_eq!(
            count_events(&db, "run-ma1", event_type::MERGE_DONE),
            1,
            "exactly one branch merged under `any`"
        );
        let files = merge_tree(tmp.path(), &db, "run-ma1");
        assert!(
            files.contains("b.txt") && !files.contains("a.txt"),
            "the first-finished child (b) is integrated, not the spec-first (a): {files}"
        );
    }

    /// Human resolution must be committed: if the user edits the integration
    /// worktree but continues without committing, the run refuses (re-pauses)
    /// rather than resetting their edits away and merging on from a marker tree.
    #[tokio::test]
    async fn merge_human_resolution_requires_a_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let children = vec![cstep("a", CONFLICT), cstep("b", CONFLICT)];
        let (db, ws, _b) = scaffold_parallel_integrate(
            tmp.path(),
            "run-mhu",
            Join::All,
            Integrate::Merge,
            &children,
        );
        let ctx = par_ctx(db.clone(), MatrixDriver::new(ws));
        drive_run(&ctx, "run-mhu").await;
        assert_eq!(run_status_str(&db, "run-mhu"), "paused");

        // Human edits the conflicted file but does NOT commit, then continues.
        let int_wt = tmp.path().join("rd-run-mhu").join("integrate-0");
        std::fs::write(int_wt.join(CONFLICT_FILE), "edited but uncommitted\n").unwrap();
        set_resolution(&db, "run-mhu", "human");
        drive_run(&ctx, "run-mhu").await;

        // Refused: still paused(conflict), not advanced; the choice is cleared so
        // the user must commit and retry, and the edit is left in place (not reset).
        assert_eq!(run_status_str(&db, "run-mhu"), "paused");
        let cur = get_cursor(&db.lock(), "run-mhu");
        assert!(
            cur.merge
                .and_then(|m| m.conflict)
                .and_then(|c| c.resolution)
                .is_none(),
            "resolution cleared — the user must commit first"
        );
        let body = std::fs::read_to_string(int_wt.join(CONFLICT_FILE)).unwrap();
        assert!(
            body.contains("edited but uncommitted"),
            "the uncommitted edit is preserved, not discarded: {body}"
        );
    }

    /// A committed human "resolution" that still contains conflict markers must be
    /// rejected — otherwise the merge would finish with markers in the integrated
    /// result. The run re-pauses; it does not reach `done`.
    #[tokio::test]
    async fn merge_human_resolution_with_markers_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let children = vec![cstep("a", CONFLICT), cstep("b", CONFLICT)];
        let (db, ws, _b) = scaffold_parallel_integrate(
            tmp.path(),
            "run-mhm",
            Join::All,
            Integrate::Merge,
            &children,
        );
        let ctx = par_ctx(db.clone(), MatrixDriver::new(ws));
        drive_run(&ctx, "run-mhm").await;
        assert_eq!(run_status_str(&db, "run-mhm"), "paused");

        // Human commits a *partial* resolution: they stripped the outer
        // <<<<<<< / >>>>>>> bounds but left the ======= divider behind.
        let int_wt = tmp.path().join("rd-run-mhm").join("integrate-0");
        std::fs::write(
            int_wt.join(CONFLICT_FILE),
            "from one side\n=======\nfrom the other side\n",
        )
        .unwrap();
        sh(&int_wt, &["add", "-A"]);
        sh(&int_wt, &["commit", "-qm", "partial resolution"]);
        set_resolution(&db, "run-mhm", "human");
        drive_run(&ctx, "run-mhm").await;

        // Refused: still paused(conflict), not done; the choice is cleared.
        assert_eq!(run_status_str(&db, "run-mhm"), "paused");
        let reason: Option<String> = db
            .lock()
            .query_row(
                "SELECT paused_reason FROM wf_run WHERE id='run-mhm'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reason.as_deref(), Some("conflict"));
        let cur = get_cursor(&db.lock(), "run-mhm");
        assert!(
            cur.merge
                .and_then(|m| m.conflict)
                .and_then(|c| c.resolution)
                .is_none(),
            "resolution cleared — the user must strip the markers and retry"
        );
    }

    /// Resolving by *renaming* a still-conflicted file must not slip markers past
    /// the guard: the scan covers paths the resolution changed since the snapshot,
    /// not just the originally-conflicted paths, so the renamed file is caught.
    #[tokio::test]
    async fn merge_human_resolution_that_renames_a_marker_file_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let children = vec![cstep("a", CONFLICT), cstep("b", CONFLICT)];
        let (db, ws, _b) = scaffold_parallel_integrate(
            tmp.path(),
            "run-mhr",
            Join::All,
            Integrate::Merge,
            &children,
        );
        let ctx = par_ctx(db.clone(), MatrixDriver::new(ws));
        drive_run(&ctx, "run-mhr").await;
        assert_eq!(run_status_str(&db, "run-mhr"), "paused");

        // Human "resolves" by renaming the conflicted file — its markers ride
        // along to the new path, and the old path disappears.
        let int_wt = tmp.path().join("rd-run-mhr").join("integrate-0");
        sh(&int_wt, &["mv", CONFLICT_FILE, "renamed.txt"]);
        sh(&int_wt, &["commit", "-qm", "rename instead of resolving"]);
        set_resolution(&db, "run-mhr", "human");
        drive_run(&ctx, "run-mhr").await;

        assert_eq!(run_status_str(&db, "run-mhr"), "paused");
        let cur = get_cursor(&db.lock(), "run-mhr");
        assert!(
            cur.merge
                .and_then(|m| m.conflict)
                .and_then(|c| c.resolution)
                .is_none(),
            "the renamed marker file is detected — resolution cleared"
        );
    }

    // ───────────────────────────── loop blocks (S7) ─────────────────────────

    /// A real-git stub whose "agent" writes a configured `verdict.json` into the
    /// until-step's blackboard dir each turn (instead of committing code) — the
    /// verdict-gated shape a loop's exit step needs. Spawns a real `--shared`
    /// clone forking from the run repo so a `done` verdict still ferries.
    struct VerdictStub {
        root: PathBuf,
        blackboard: PathBuf,
        step_id: String,
        verdict: String,
        /// When true, each turn also makes a commit in the agent's workspace so a
        /// `commit`-gated body step (e.g. `fix`) advances HEAD. A verdict-gated
        /// `until` step ignores its own commit (that attempt never ferries).
        commit: bool,
        tx: broadcast::Sender<StatusEvent>,
        state: parking_lot::Mutex<StubState>,
    }
    impl VerdictStub {
        fn new(
            root: PathBuf,
            blackboard: PathBuf,
            step_id: &str,
            verdict: &str,
            commit: bool,
        ) -> Arc<Self> {
            Arc::new(Self {
                root,
                blackboard,
                step_id: step_id.to_string(),
                verdict: verdict.to_string(),
                commit,
                tx: broadcast::channel(256).0,
                state: parking_lot::Mutex::new(StubState::default()),
            })
        }
        fn set(&self, id: &str, s: AgentStatus) {
            self.state.lock().statuses.insert(id.to_string(), s.clone());
            let _ = self.tx.send(StatusEvent {
                agent_id: id.to_string(),
                status: s,
            });
        }
    }
    impl AgentDriver for VerdictStub {
        fn spawn(
            &self,
            req: SpawnReq,
        ) -> super::super::driver::BoxFuture<'_, Result<super::super::driver::SpawnedAgent>>
        {
            Box::pin(async move {
                let id = {
                    let mut st = self.state.lock();
                    st.count += 1;
                    format!("stub-{}", st.count)
                };
                let dest = self.root.join(&id);
                let base_ref = req.fork_base.clone().unwrap();
                let spec = crate::sandbox::provision::CheckoutSpec {
                    source_repo: &req.repo_path,
                    base_ref: &base_ref,
                    dest: &dest,
                };
                crate::sandbox::provision::provision_forking_run_repo(
                    &spec,
                    req.run_repo.as_ref().unwrap(),
                )
                .await?;
                self.state.lock().worktrees.insert(id.clone(), dest.clone());
                self.set(&id, AgentStatus::Idle);
                Ok(super::super::driver::SpawnedAgent {
                    agent_id: id,
                    worktree: dest,
                })
            })
        }
        fn status(&self, id: &str) -> Option<AgentStatus> {
            self.state.lock().statuses.get(id).cloned()
        }
        fn subscribe(&self) -> broadcast::Receiver<StatusEvent> {
            self.tx.subscribe()
        }
        fn send_message<'a>(
            &'a self,
            id: &'a str,
            _text: String,
        ) -> super::super::driver::BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                self.set(id, AgentStatus::Running);
                // Written after the attempt has subscribed and archived any stale
                // verdict — the ordering the real supervisor produces.
                let dir = blackboard::step_dir(&self.blackboard, &self.step_id).unwrap();
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("verdict.json"), &self.verdict).unwrap();
                if self.commit {
                    let wt = self.state.lock().worktrees.get(id).cloned().unwrap();
                    sh(&wt, &["config", "user.email", "t@t.t"]);
                    sh(&wt, &["config", "user.name", "t"]);
                    std::fs::write(wt.join(format!("{id}.txt")), "work").unwrap();
                    sh(&wt, &["add", "-A"]);
                    sh(&wt, &["commit", "-qm", "agent work"]);
                }
                self.set(id, AgentStatus::Idle);
                Ok(())
            })
        }
        fn stop<'a>(&'a self, _id: &'a str) -> super::super::driver::BoxFuture<'a, Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn archive<'a>(&'a self, _id: &'a str) -> super::super::driver::BoxFuture<'a, Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn last_activity(&self, _id: &str) -> Option<i64> {
            None
        }
        fn turn_usage(&self, _id: &str) -> Option<super::super::driver::TurnUsage> {
            None
        }
    }

    /// Scaffold a run whose whole workflow is one loop with a verdict-gated
    /// `review` exit step. With `with_fix`, a commit-gated `fix` step follows it
    /// in the body (the canonical `[review, fix]` shape) so tests can assert what
    /// happens to a body step *after* the `until` step. Returns the db, the
    /// workspaces root the stub provisions under, and the blackboard dir.
    fn scaffold_loop(
        tmp: &Path,
        run_id: &str,
        branch: &str,
        max: u32,
        with_fix: bool,
    ) -> (Db, PathBuf, PathBuf) {
        let source = tmp.join("source");
        std::fs::create_dir_all(&source).unwrap();
        sh(&source, &["init", "-q", "-b", "main"]);
        sh(&source, &["config", "user.email", "t@t.t"]);
        sh(&source, &["config", "user.name", "t"]);
        std::fs::write(source.join("README"), "base").unwrap();
        sh(&source, &["add", "-A"]);
        sh(&source, &["commit", "-qm", "base"]);
        let base_sha = {
            let o = Sh::new("git")
                .current_dir(&source)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap();
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        let run_dir = tmp.join("rundir");
        let blackboard = blackboard::blackboard_dir(&run_dir);
        std::fs::create_dir_all(&blackboard).unwrap();

        let mut agents = BTreeMap::new();
        agents.insert(
            "coder".to_string(),
            super::super::spec::AgentSpec {
                base: "codex".to_string(),
                model: None,
                instructions: None,
                skills: vec![],
                custom_agent: None,
            },
        );
        let review = Step {
            id: "review".to_string(),
            agent: "coder".to_string(),
            goal: "review the work".to_string(),
            gate: Gate::Verdict,
            budgets: None,
            comms: vec![],
        };
        let mut body = vec![Block::Step(review)];
        if with_fix {
            body.push(Block::Step(Step {
                id: "fix".to_string(),
                agent: "coder".to_string(),
                goal: "address the feedback".to_string(),
                gate: Gate::Commit,
                budgets: None,
                comms: vec![],
            }));
        }
        let spec = Spec {
            version: 1,
            name: "demo".to_string(),
            description: None,
            budgets: None,
            agents,
            workflow: vec![Block::Loop(Loop {
                max,
                until: super::super::spec::Until {
                    step: "review".to_string(),
                    verdict: super::super::spec::LoopVerdict::Done,
                },
                body,
            })],
            finalize: None,
        };
        let spec_json = serde_json::to_string(&spec).unwrap();
        let db = crate::database::init(tmp).unwrap();
        db.lock()
            .execute(
                "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                    base_sha,status,budgets_json,spent_json,created_at,updated_at)
                 VALUES (?1,'demo',?2,'t','p',?3,?4,?5,?6,'pending','{}','{}',0,0)",
                rusqlite::params![
                    run_id,
                    spec_json,
                    source.to_string_lossy(),
                    run_dir.to_string_lossy(),
                    branch,
                    base_sha,
                ],
            )
            .unwrap();
        (db, tmp.join("ws"), blackboard)
    }

    fn count(db: &Db, sql: &str) -> i64 {
        db.lock().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn loop_ctx(db: Db, driver: Arc<VerdictStub>) -> RunCtx {
        RunCtx {
            db,
            driver,
            app: None,
            cancel: Arc::new(AtomicBool::new(false)),
            pending_ask: Arc::new(AtomicBool::new(false)),
            deadlines: Deadlines::default(),
        }
    }

    #[tokio::test]
    async fn loop_exits_on_first_done_verdict() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, ws, bb) = scaffold_loop(tmp.path(), "run-loop-done", "wf/ld-1", 3, false);
        let driver = VerdictStub::new(
            ws,
            bb,
            "review",
            r#"{"result":"done","summary":"lgtm"}"#,
            false,
        );
        drive_run(&loop_ctx(db.clone(), driver), "run-loop-done").await;

        // Exactly one iteration ran (iteration 0), its review is done, the loop
        // never hit its max, and the run completed.
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-loop-done'"
            ),
            1,
            "one review attempt only"
        );
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-loop-done' \
                 AND iteration=0 AND status='done'"
            ),
            1
        );
        assert_eq!(
            count(
                &db,
                &format!(
                    "SELECT COUNT(*) FROM wf_event WHERE run_id='run-loop-done' AND type='{}'",
                    event_type::LOOP_MAX_REACHED
                )
            ),
            0,
            "loop_max_reached must NOT fire on an early done"
        );
        assert_eq!(run_status_str(&db, "run-loop-done"), "done");
    }

    #[tokio::test]
    async fn loop_revises_until_max_then_continues() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, ws, bb) = scaffold_loop(tmp.path(), "run-loop-max", "wf/lm-1", 3, false);
        // "revise" every turn → the loop runs all `max` iterations, then continues
        // (exhaustion is not failure — spec §6.6).
        let driver = VerdictStub::new(
            ws,
            bb,
            "review",
            r#"{"result":"revise","summary":"again"}"#,
            false,
        );
        drive_run(&loop_ctx(db.clone(), driver), "run-loop-max").await;

        // One blocked review per iteration, at iterations 0..3.
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-loop-max' AND status='blocked'"
            ),
            3
        );
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(DISTINCT iteration) FROM wf_step_exec WHERE run_id='run-loop-max'"
            ),
            3,
            "iterations 0,1,2"
        );
        assert_eq!(
            count(
                &db,
                &format!(
                    "SELECT COUNT(*) FROM wf_event WHERE run_id='run-loop-max' AND type='{}'",
                    event_type::LOOP_MAX_REACHED
                )
            ),
            1
        );
        assert_eq!(
            run_status_str(&db, "run-loop-max"),
            "done",
            "loop exhaustion continues to done"
        );
    }

    #[tokio::test]
    async fn resume_mid_loop_restores_the_iteration_counter() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, ws, bb) = scaffold_loop(tmp.path(), "run-loop-resume", "wf/lr-1", 3, false);
        // A prior driver died during iteration 1: the cursor records it and a
        // non-terminal attempt is left behind. Resume must pick up at iteration 1
        // (not restart at 0) and run only iterations 1 and 2 before max.
        {
            let conn = db.lock();
            conn.execute(
                "UPDATE wf_run SET cursor_json=?1 WHERE id='run-loop-resume'",
                [r#"{"index":0,"iterations":{"0":1}}"#],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO wf_step_exec (id, run_id, step_id, attempt, iteration, status,
                    gate_mode, agent_id)
                 VALUES ('exec-stale','run-loop-resume','review',1,1,'running','verdict','ghost')",
                [],
            )
            .unwrap();
        }
        let driver = VerdictStub::new(
            ws,
            bb,
            "review",
            r#"{"result":"revise","summary":"again"}"#,
            false,
        );
        drive_run(&loop_ctx(db.clone(), driver), "run-loop-resume").await;

        // The counter was restored: nothing ran at iteration 0.
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-loop-resume' AND iteration=0"
            ),
            0,
            "resume must not restart the loop at iteration 0"
        );
        // The stale attempt was abandoned; fresh reviews ran at iterations 1 and 2.
        let stale: String = db
            .lock()
            .query_row(
                "SELECT status FROM wf_step_exec WHERE id='exec-stale'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stale, "abandoned");
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-loop-resume' \
                 AND iteration=2 AND status='blocked'"
            ),
            1,
            "the final iteration ran"
        );
        assert_eq!(
            count(
                &db,
                &format!(
                    "SELECT COUNT(*) FROM wf_event WHERE run_id='run-loop-resume' AND type='{}'",
                    event_type::LOOP_MAX_REACHED
                )
            ),
            1
        );
        assert_eq!(run_status_str(&db, "run-loop-resume"), "done");
    }

    /// `until` not last (§6.6): with body `[review, fix]` and a `revise` review
    /// each iteration, the trailing `fix` runs *within the same iteration* before
    /// the loop restarts — the remaining body is the remediation for a non-`done`
    /// verdict, not something to skip.
    #[tokio::test]
    async fn loop_runs_trailing_body_after_a_revise() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, ws, bb) = scaffold_loop(tmp.path(), "run-loop-fix", "wf/lf-1", 2, true);
        let driver = VerdictStub::new(
            ws,
            bb,
            "review",
            r#"{"result":"revise","summary":"again"}"#,
            true,
        );
        drive_run(&loop_ctx(db.clone(), driver), "run-loop-fix").await;

        // `fix` ran and completed in BOTH iterations (0 and 1) — a revise does not
        // short-circuit the rest of the body.
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-loop-fix' \
                 AND step_id='fix' AND status='done'"
            ),
            2,
            "fix runs once per revise iteration"
        );
        assert_eq!(
            count(
                &db,
                &format!(
                    "SELECT COUNT(*) FROM wf_event WHERE run_id='run-loop-fix' AND type='{}'",
                    event_type::LOOP_MAX_REACHED
                )
            ),
            1
        );
        assert_eq!(run_status_str(&db, "run-loop-fix"), "done");
    }

    /// `until` not last (§6.6): a `done` review exits the loop *immediately*,
    /// skipping the trailing `fix` — there is nothing to remediate.
    #[tokio::test]
    async fn loop_skips_trailing_body_on_done() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, ws, bb) = scaffold_loop(tmp.path(), "run-loop-skip", "wf/ls-1", 3, true);
        let driver = VerdictStub::new(
            ws,
            bb,
            "review",
            r#"{"result":"done","summary":"lgtm"}"#,
            true,
        );
        drive_run(&loop_ctx(db.clone(), driver), "run-loop-skip").await;

        // review is done at iteration 0 → the loop exits and `fix` never spawns.
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-loop-skip' AND step_id='fix'"
            ),
            0,
            "a done review skips the trailing fix"
        );
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-loop-skip' \
                 AND step_id='review' AND status='done'"
            ),
            1
        );
        assert_eq!(run_status_str(&db, "run-loop-skip"), "done");
    }

    // ─────────────────────── orchestrate stages (S11) ───────────────────────

    #[derive(Clone, Copy, PartialEq)]
    enum OrchMode {
        /// Writes a `done` verdict on the concluding prompt.
        Conclude,
        /// Never writes a verdict — the stage should gate on it and pause.
        NeverConclude,
        /// Stalls its turn forever — the engine escalates to the human.
        Stall,
    }

    /// A real-git stub for orchestrate stages (spec §10.2). Children commit (their
    /// `commit` gate); the orchestrator writes its concluding `verdict.json` on the
    /// conclude prompt, never writes one, or stalls — per [`OrchMode`]. Roles are
    /// told apart by the prompt text the engine composes.
    struct OrchDriver {
        root: PathBuf,
        blackboard: PathBuf,
        mode: OrchMode,
        /// When set, a `wf_decide` body the orchestrator "issues" on its first turn
        /// (persisted as a queued decision the way the router would), plus the DB
        /// and run id needed to write it. Lets a test script skip/retry decisions.
        first_decision: Option<(Db, String, serde_json::Value)>,
        tx: broadcast::Sender<StatusEvent>,
        state: parking_lot::Mutex<StubState>,
    }
    impl OrchDriver {
        fn new(root: PathBuf, blackboard: PathBuf, mode: OrchMode) -> Arc<Self> {
            Arc::new(Self {
                root,
                blackboard,
                mode,
                first_decision: None,
                tx: broadcast::channel(256).0,
                state: parking_lot::Mutex::new(StubState::default()),
            })
        }
        fn new_scripted(
            root: PathBuf,
            blackboard: PathBuf,
            mode: OrchMode,
            db: Db,
            run_id: &str,
            decision: serde_json::Value,
        ) -> Arc<Self> {
            Arc::new(Self {
                root,
                blackboard,
                mode,
                first_decision: Some((db, run_id.to_string(), decision)),
                tx: broadcast::channel(256).0,
                state: parking_lot::Mutex::new(StubState::default()),
            })
        }
        fn set(&self, id: &str, s: AgentStatus) {
            self.state.lock().statuses.insert(id.to_string(), s.clone());
            let _ = self.tx.send(StatusEvent {
                agent_id: id.to_string(),
                status: s,
            });
        }
        /// Persist `decision` as a queued `decision` message from the orchestrator
        /// exec — exactly what `route_decide` does when the orchestrator calls
        /// `wf_decide` (its exec is resolved by `agent_id`, stamped at spawn).
        fn inject_decision(&self, orch_agent_id: &str) {
            let Some((db, run_id, body)) = &self.first_decision else {
                return;
            };
            let conn = db.lock();
            let exec: Option<String> = conn
                .query_row(
                    "SELECT id FROM wf_step_exec WHERE run_id = ?1 AND agent_id = ?2",
                    rusqlite::params![run_id, orch_agent_id],
                    |r| r.get(0),
                )
                .ok();
            if let Some(exec) = exec {
                conn.execute(
                    "INSERT INTO wf_message (id, run_id, from_step_exec_id, to_step_exec_id,
                        kind, body_json, status, created_at)
                     VALUES (?1, ?2, ?3, NULL, 'decision', ?4, 'queued', 0)",
                    rusqlite::params![
                        format!("dec-{}", uuid::Uuid::new_v4()),
                        run_id,
                        exec,
                        body.to_string(),
                    ],
                )
                .unwrap();
            }
        }
    }
    impl AgentDriver for OrchDriver {
        fn spawn(
            &self,
            req: SpawnReq,
        ) -> super::super::driver::BoxFuture<'_, Result<super::super::driver::SpawnedAgent>>
        {
            Box::pin(async move {
                let id = {
                    let mut st = self.state.lock();
                    st.count += 1;
                    format!("o-{}", st.count)
                };
                let dest = self.root.join(&id);
                let base_ref = req.fork_base.clone().unwrap();
                let spec = crate::sandbox::provision::CheckoutSpec {
                    source_repo: &req.repo_path,
                    base_ref: &base_ref,
                    dest: &dest,
                };
                crate::sandbox::provision::provision_forking_run_repo(
                    &spec,
                    req.run_repo.as_ref().unwrap(),
                )
                .await?;
                self.state.lock().worktrees.insert(id.clone(), dest.clone());
                self.set(&id, AgentStatus::Idle);
                Ok(super::super::driver::SpawnedAgent {
                    agent_id: id,
                    worktree: dest,
                })
            })
        }
        fn status(&self, id: &str) -> Option<AgentStatus> {
            self.state.lock().statuses.get(id).cloned()
        }
        fn subscribe(&self) -> broadcast::Receiver<StatusEvent> {
            self.tx.subscribe()
        }
        fn send_message<'a>(
            &'a self,
            id: &'a str,
            text: String,
        ) -> super::super::driver::BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                let is_initial = text.contains("Workflow orchestrator");
                let is_orch = is_initial
                    || text.contains("All children are done")
                    || text.contains("Updates from your children");
                let is_nudge = text.contains("gone quiet");
                self.set(id, AgentStatus::Running);
                if is_nudge {
                    // Keep the (stalling) turn running so the watchdog escalates.
                    return Ok(());
                }
                if is_orch {
                    // Issue any scripted decision on the opening turn.
                    if is_initial {
                        self.inject_decision(id);
                    }
                    match self.mode {
                        OrchMode::Stall => return Ok(()), // never goes Idle → stall
                        OrchMode::Conclude => {
                            if text.contains("All children are done") {
                                let dir = self.blackboard.join("orchestrate-0");
                                std::fs::create_dir_all(&dir).unwrap();
                                std::fs::write(
                                    dir.join("verdict.json"),
                                    r#"{"result":"done","summary":"concluded"}"#,
                                )
                                .unwrap();
                            }
                        }
                        OrchMode::NeverConclude => {}
                    }
                    self.set(id, AgentStatus::Idle);
                } else if text.contains("HANGCHILD") {
                    // A child that never finishes its turn — only a cancel (e.g.
                    // `skip_child`) can wind it down.
                    return Ok(());
                } else {
                    // A child: satisfy its `commit` gate.
                    let wt = self.state.lock().worktrees.get(id).cloned().unwrap();
                    sh(&wt, &["config", "user.email", "t@t.t"]);
                    sh(&wt, &["config", "user.name", "t"]);
                    std::fs::write(wt.join(format!("{id}.txt")), "work").unwrap();
                    sh(&wt, &["add", "-A"]);
                    sh(&wt, &["commit", "-qm", "child work"]);
                    self.set(id, AgentStatus::Idle);
                }
                Ok(())
            })
        }
        fn stop<'a>(&'a self, _id: &'a str) -> super::super::driver::BoxFuture<'a, Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn archive<'a>(&'a self, _id: &'a str) -> super::super::driver::BoxFuture<'a, Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn last_activity(&self, _id: &str) -> Option<i64> {
            None
        }
        fn turn_usage(&self, _id: &str) -> Option<super::super::driver::TurnUsage> {
            None
        }
    }

    /// A run whose whole workflow is one orchestrate block (agent `orch`) with a
    /// single static, commit-gated child `impl` whose goal is `child_goal` (use
    /// `HANGCHILD` for a child that never finishes its turn). Returns the db, the
    /// workspace root, the blackboard dir, and the base SHA.
    fn scaffold_orchestrate(
        tmp: &Path,
        run_id: &str,
        child_goal: &str,
    ) -> (Db, PathBuf, PathBuf, String) {
        let source = tmp.join("source");
        std::fs::create_dir_all(&source).unwrap();
        sh(&source, &["init", "-q", "-b", "main"]);
        sh(&source, &["config", "user.email", "t@t.t"]);
        sh(&source, &["config", "user.name", "t"]);
        std::fs::write(source.join("README"), "base").unwrap();
        sh(&source, &["add", "-A"]);
        sh(&source, &["commit", "-qm", "base"]);
        let base_sha = {
            let o = Sh::new("git")
                .current_dir(&source)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap();
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        let run_dir = tmp.join("rundir");
        let blackboard = blackboard::blackboard_dir(&run_dir);
        std::fs::create_dir_all(&blackboard).unwrap();

        let mut agents = BTreeMap::new();
        for a in ["orch", "coder"] {
            agents.insert(
                a.to_string(),
                super::super::spec::AgentSpec {
                    base: "codex".to_string(),
                    model: None,
                    instructions: None,
                    skills: vec![],
                    custom_agent: None,
                },
            );
        }
        let child = Step {
            id: "impl".to_string(),
            agent: "coder".to_string(),
            goal: child_goal.to_string(),
            gate: Gate::Commit,
            budgets: None,
            comms: vec![],
        };
        let spec = Spec {
            version: 1,
            name: "orch".to_string(),
            description: None,
            // Short stall/nudge so the stall test escalates in ~2s of real time
            // (no `start_paused` — the real-git provisioning needs the IO reactor).
            // Harmless to the non-stalling tests: their turns end before any tick.
            budgets: Some(Budgets {
                turns: None,
                tokens: None,
                wall_clock_mins: None,
                turns_per_attempt: None,
                max_attempts: None,
                spawn_timeout_secs: None,
                turn_start_timeout_secs: None,
                stall_timeout_secs: Some(1),
                nudge_timeout_secs: Some(1),
                tests_timeout_secs: None,
            }),
            agents,
            workflow: vec![Block::Orchestrate(Orchestrate {
                agent: "orch".to_string(),
                goal: "lead the stage".to_string(),
                children: None,
                body: vec![child],
                join: Join::All,
                integrate: Integrate::None,
                comms: vec![],
                compose: None,
            })],
            finalize: None,
        };
        let spec_json = serde_json::to_string(&spec).unwrap();
        // Freeze the effective budgets from the spec so the short stall/nudge
        // actually take effect (a bare '{}' would deserialize to the defaults).
        let budgets_json = serde_json::to_string(&EffectiveBudgets::resolve(&spec)).unwrap();
        let db = crate::database::init(tmp).unwrap();
        db.lock()
            .execute(
                "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                    base_sha,status,budgets_json,spent_json,created_at,updated_at)
                 VALUES (?1,'orch',?2,'t','p',?3,?4,'wf/orch-x',?5,'pending',?6,'{}',0,0)",
                rusqlite::params![
                    run_id,
                    spec_json,
                    source.to_string_lossy(),
                    run_dir.to_string_lossy(),
                    base_sha,
                    budgets_json,
                ],
            )
            .unwrap();
        (db, tmp.join("ws"), blackboard, base_sha)
    }

    fn orch_ctx(db: Db, driver: Arc<OrchDriver>) -> RunCtx {
        RunCtx {
            db,
            driver,
            app: None,
            cancel: Arc::new(AtomicBool::new(false)),
            pending_ask: Arc::new(AtomicBool::new(false)),
            // Tick the stall watchdog fast so the stall test resolves quickly.
            deadlines: Deadlines {
                watchdog_tick: std::time::Duration::from_millis(100),
                ..Deadlines::default()
            },
        }
    }

    #[tokio::test]
    async fn orchestrate_concludes_after_children_and_reaches_done() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, ws, bb, _base) =
            scaffold_orchestrate(tmp.path(), "run-orch", "implement the slice");
        let ctx = orch_ctx(db.clone(), OrchDriver::new(ws, bb, OrchMode::Conclude));
        drive_run(&ctx, "run-orch").await;

        assert_eq!(run_status_str(&db, "run-orch"), "done");
        // The child ran, and the orchestrator concluded — both terminal `done`.
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-orch' \
                 AND step_id='impl' AND status='done'"
            ),
            1,
            "the child completed"
        );
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-orch' \
                 AND step_id='orchestrate-0' AND status='done'"
            ),
            1,
            "the orchestrator concluded"
        );
        // The stage gate is the orchestrator's own verdict.
        let concluded = count(
            &db,
            "SELECT COUNT(*) FROM wf_event WHERE run_id='run-orch' AND type='gate_evaluated' \
             AND json_extract(payload_json,'$.outcome')='done' \
             AND step_exec_id IN (SELECT id FROM wf_step_exec WHERE step_id='orchestrate-0')",
        );
        assert!(
            concluded >= 1,
            "orchestrator's concluding verdict gated the stage"
        );
    }

    #[tokio::test]
    async fn orchestrate_pauses_blocked_when_the_orchestrator_never_concludes() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, ws, bb, _base) = scaffold_orchestrate(tmp.path(), "run-nc", "implement the slice");
        let ctx = orch_ctx(db.clone(), OrchDriver::new(ws, bb, OrchMode::NeverConclude));
        drive_run(&ctx, "run-nc").await;

        // The child finished, but the stage does NOT complete without the
        // orchestrator's concluding verdict — it pauses `blocked_gate` (§6.6).
        let (status, reason): (String, Option<String>) = db
            .lock()
            .query_row(
                "SELECT status, paused_reason FROM wf_run WHERE id='run-nc'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "paused");
        assert_eq!(reason.as_deref(), Some("blocked_gate"));
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-nc' \
                 AND step_id='impl' AND status='done'"
            ),
            1,
            "the child still ran to completion"
        );
    }

    #[tokio::test]
    async fn orchestrator_stall_escalates_to_the_human() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, ws, bb, _base) =
            scaffold_orchestrate(tmp.path(), "run-stall", "implement the slice");
        let ctx = orch_ctx(db.clone(), OrchDriver::new(ws, bb, OrchMode::Stall));
        drive_run(&ctx, "run-stall").await;

        // A stalled orchestrator does not hang the stage — the engine escalates to
        // the human, pausing the run `question` (§10.2).
        let (status, reason, error): (String, Option<String>, Option<String>) = db
            .lock()
            .query_row(
                "SELECT status, paused_reason, error FROM wf_run WHERE id='run-stall'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, "paused", "run error: {error:?}");
        assert_eq!(reason.as_deref(), Some("question"));
        let stalled = count(
            &db,
            "SELECT COUNT(*) FROM wf_event WHERE run_id='run-stall' AND type='watchdog_stalled'",
        );
        assert!(stalled >= 1, "the orchestrator stall was journaled");
    }

    #[tokio::test]
    async fn resume_does_not_rerun_a_completed_static_child() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, ws, bb, _base) =
            scaffold_orchestrate(tmp.path(), "run-resume", "implement the slice");
        // Simulate a prior drive that paused before the orchestrator concluded: the
        // static child already finished `done`.
        db.lock()
            .execute(
                "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode,agent_id)
                 VALUES ('impl-prior','run-resume','impl',1,0,'done','commit','prior-agent')",
                [],
            )
            .unwrap();
        let ctx = orch_ctx(db.clone(), OrchDriver::new(ws, bb, OrchMode::Conclude));
        drive_run(&ctx, "run-resume").await;

        assert_eq!(run_status_str(&db, "run-resume"), "done");
        // The already-done child is not executed a second time (§12.3 parity).
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-resume' AND step_id='impl'"
            ),
            1,
            "the completed child must not re-run on resume"
        );
    }

    #[test]
    fn dyn_child_index_is_seeded_from_existing_execs() {
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::database::init(tmp.path()).unwrap();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                base_sha,status,budgets_json,spent_json,created_at,updated_at)
             VALUES ('r','n','{}','t','p','/r','/d','wf/x','sha','running','{}','{}',0,0)",
            [],
        )
        .unwrap();
        assert_eq!(existing_dyn_child_count(&conn, "r", "orchestrate-0"), 0);
        for k in 0..2 {
            conn.execute(
                "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode)
                 VALUES (?1,'r',?2,1,0,'done','verdict')",
                rusqlite::params![format!("e{k}"), format!("orchestrate-0::dyn-{k}")],
            )
            .unwrap();
        }
        // A non-dynamic child and a different stage's children must not count.
        conn.execute(
            "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode)
             VALUES ('x','r','impl',1,0,'done','commit')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode)
             VALUES ('y','r','orchestrate-1::dyn-0',1,0,'done','verdict')",
            [],
        )
        .unwrap();
        assert_eq!(
            existing_dyn_child_count(&conn, "r", "orchestrate-0"),
            2,
            "the next dynamic index skips the two already created"
        );
    }

    #[tokio::test]
    async fn skip_child_cancels_the_child_so_the_stage_can_conclude() {
        let tmp = tempfile::tempdir().unwrap();
        // The child hangs its turn — only a cancel ends it. On its opening turn the
        // orchestrator issues `skip_child`; the engine must cancel that child (not
        // let it stall out) so the stage concludes on the orchestrator's verdict.
        let (db, ws, bb, _base) =
            scaffold_orchestrate(tmp.path(), "run-skip", "implement HANGCHILD");
        let driver = OrchDriver::new_scripted(
            ws,
            bb,
            OrchMode::Conclude,
            db.clone(),
            "run-skip",
            serde_json::json!({ "decision": "skip_child", "step_id": "impl", "reason": "unneeded" }),
        );
        drive_run(&orch_ctx(db.clone(), driver), "run-skip").await;

        assert_eq!(run_status_str(&db, "run-skip"), "done");
        // The child was cancelled (abandoned), not left to stall out (`error`) or
        // to complete (`done`).
        let bad = count(
            &db,
            "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-skip' AND step_id='impl' \
             AND status IN ('error','done')",
        );
        assert_eq!(
            bad, 0,
            "skip_child must cancel the child, not let it stall or finish"
        );
    }

    #[test]
    fn stale_retry_result_is_discarded_by_generation() {
        // A superseded attempt (older generation) that still finishes must not
        // decide the join; only the current generation's result counts (§10.2).
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::database::init(tmp.path()).unwrap();
        db.lock()
            .execute(
                "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                    base_sha,status,budgets_json,spent_json,created_at,updated_at)
                 VALUES ('r','n','{}','t','p','/r','/d','wf/x','sha','running','{}','{}',0,0)",
                [],
            )
            .unwrap();
        for id in ["orch-exec", "c-old", "c-new"] {
            db.lock()
                .execute(
                    "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode)
                     VALUES (?1,'r','impl',1,0,'abandoned','verdict')",
                    [id],
                )
                .unwrap();
        }
        let ctx = RunCtx {
            db: db.clone(),
            driver: StubDriver::new(tmp.path().join("ws"), true),
            app: None,
            cancel: Arc::new(AtomicBool::new(false)),
            pending_ask: Arc::new(AtomicBool::new(false)),
            deadlines: Deadlines::default(),
        };
        let mut ledger = Ledger::default();
        let mut outcomes: HashMap<String, ChildStatus> = HashMap::new();
        let child_cancels: HashMap<String, Arc<AtomicBool>> = HashMap::new();
        // Current generation for `impl` is 1 (a retry superseded generation 0).
        let mut child_gen: HashMap<String, u64> = HashMap::new();
        child_gen.insert("impl".to_string(), 1);

        let result = |exec: &str, generation: u64| OrchChildResult {
            step_id: "impl".to_string(),
            exec_id: exec.to_string(),
            generation,
            outcome: ChildOutcome::Success {
                moved_head: false,
                head: None,
            },
            ledger: Ledger::default(),
        };

        // A stale (generation 0) success is ignored — records no join outcome.
        handle_orch_child(
            &ctx,
            "r",
            "orch-exec",
            Join::Any,
            &mut ledger,
            Ok(result("c-old", 0)),
            &mut outcomes,
            &child_cancels,
            &child_gen,
        );
        assert!(
            !outcomes.contains_key("impl"),
            "a superseded attempt must not decide the join"
        );

        // The current-generation result records the outcome.
        handle_orch_child(
            &ctx,
            "r",
            "orch-exec",
            Join::Any,
            &mut ledger,
            Ok(result("c-new", 1)),
            &mut outcomes,
            &child_cancels,
            &child_gen,
        );
        assert!(matches!(outcomes.get("impl"), Some(ChildStatus::Success)));
    }
}
