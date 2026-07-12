//! The run scheduler (spec §6). One tokio task per active run walks the block
//! tree, drives each step through [`attempt::run_attempt`], ferries the `done`
//! commit into the run repo (§12.1), advances the cursor, and finalizes. S4b
//! covers **linear** runs — a top-level sequence of `step` blocks; loop /
//! parallel / orchestrate execution arrive in S7 / S8 / S11 (a non-step block
//! fails the run with a clear cause rather than being silently skipped).
//!
//! `WorkflowService` (app state) owns the registry of active runs and the
//! launch / control commands. Panic containment (§6.1): the service awaits each
//! drive task's `JoinHandle`; a panicked or errored task marks its run
//! `failed("internal scheduler error")` so a run is never left `running` with no
//! live driver.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::error::{Error, Result};

use super::attempt::{self, AttemptOutcome, AttemptParams, Deadlines};
use super::blackboard;
use super::budget::{EffectiveBudgets, Ledger};
use super::driver::{AgentDriver, SpawnReq};
use super::gitops;
use super::journal;
use super::prompts::{self, Position, StepPromptCtx};
use super::spec::{Block, Budgets, Gate, Spec, Step};
use super::types::event_type;

type Db = Arc<Mutex<Connection>>;

/// App-state singleton: the active-run registry plus launch / control.
pub struct WorkflowService {
    db: Db,
    driver: Arc<dyn AgentDriver>,
    app: AppHandle,
    /// Active-run registry. Behind an `Arc` so a drive task can remove its own
    /// entry on exit without borrowing the service.
    runs: Arc<Mutex<HashMap<String, RunHandle>>>,
}

struct RunHandle {
    cancel: Arc<AtomicBool>,
    /// Set when a spawn request arrives while this driver is winding down (its
    /// paused status already written, registry entry not yet removed). The
    /// watchdog re-drives after removing the entry instead of dropping the
    /// request — an approve that raced the wind-down would otherwise leave the
    /// run paused forever with nothing left to approve.
    respawn: Arc<AtomicBool>,
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
            let cursor = get_cursor_index(&conn, run_id);
            set_cursor_index(&conn, run_id, cursor + 1);
        }
        self.spawn_drive(run_id.to_string());
        Ok(())
    }

    fn spawn_drive(&self, run_id: String) {
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
            },
        );
    }

    let ctx = RunCtx {
        db: db.clone(),
        driver: driver.clone(),
        app: Some(app.clone()),
        cancel,
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
    let steps = extract_linear_steps(&spec)?;
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

    let mut index = get_cursor_index(&ctx.db.lock(), run_id) as usize;
    // The fork source for the next step: the last done step's ref, else base_sha.
    let mut last_ref =
        latest_done_ref(&ctx.db.lock(), run_id).unwrap_or_else(|| run.base_sha.clone());
    let mut last_exec_id: Option<String> = latest_done_exec(&ctx.db.lock(), run_id);

    while index < steps.len() {
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

        let step = &steps[index];
        let agent_spec = spec.agents.get(&step.agent).ok_or_else(|| {
            Error::Other(format!(
                "step '{}' references unknown agent '{}'",
                step.id, step.agent
            ))
        })?;
        // Step-effective budgets: run-level frozen caps with this step's own
        // `budgets` overlaid (§11.1). Feeds the attempt timeouts and retry cap.
        let step_eff = eff.for_step(step.budgets.as_ref());
        let max_attempts = step_eff.max_attempts;
        let deadlines = deadlines_from(&ctx.deadlines, &step_eff);
        // Tests-gate runner for this step, honoring its effective
        // `tests_timeout_secs` (spec §9.4, §11.1). Only the `tests` gate consults
        // it; a fresh runner per step means setup runs once per step workspace.
        let test_runner = super::tests_gate::SandboxTestRunner::new(
            test_override.clone(),
            setup_override.clone(),
            step_eff.tests_timeout_secs.max(1) as u64,
        )?;

        let mut attempt_no = next_attempt_no(&ctx.db.lock(), run_id, &step.id);
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
                finish_budget_pause(ctx, run_id, None, &mut ledger);
                return Ok(());
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
                    gate_mode(&step.gate),
                );
            }

            let prompt = {
                let ctx_prompt = StepPromptCtx {
                    run_task: &run.task,
                    step_id: &step.id,
                    step_goal: &step.goal,
                    position: Position {
                        step_index: index,
                        step_count: steps.len(),
                        iteration: None,
                    },
                    gate: &step.gate,
                    turns_per_attempt: step.budgets.as_ref().and_then(|b| b.turns_per_attempt),
                };
                match &last_failure {
                    Some(f) => prompts::retry_prompt(f, &ctx_prompt),
                    None => prompts::step_prompt(&ctx_prompt),
                }
            };

            let params = AttemptParams {
                spawn_req: SpawnReq {
                    repo_path: repo.clone(),
                    provider: agent_spec.base.clone(),
                    model: agent_spec.model.clone(),
                    instructions: agent_spec.instructions.clone(),
                    custom_agent_id: agent_spec.custom_agent.clone(),
                    // Follow-up (documented in the S4b PR): resolving the
                    // spec's agent skill/MCP names to snapshots — the linear
                    // engine spawns with the provider + brief for now. The
                    // blackboard write-grant is derived from `owner_run_id`
                    // at spawn (supervisor::lifecycle).
                    skills: vec![],
                    mcp_servers: vec![],
                    fork_base: Some(last_ref.clone()),
                    run_repo: Some(run_repo.clone()),
                    owner_run_id: run_id.to_string(),
                },
                blackboard: blackboard.clone(),
                exec_id: exec_id.clone(),
                step_id: step.id.clone(),
                attempt: attempt_no as u32,
                iteration: 0,
                gate: step.gate.clone(),
                prompt,
                deadlines: deadlines.clone(),
                reprompt_on_block: true,
            };

            let started = super::now_ms();
            let result = attempt::run_attempt(
                ctx.driver.as_ref(),
                &test_runner,
                params,
                &mut ledger,
                &step_eff,
            )
            .await;
            // Journal the attempt's events and stamp its agent id.
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
                // Persist the ledger this attempt spent (turns/tokens charged in
                // `run_attempt`), folding in the drive's active wall-clock so a
                // resume reads a current spend snapshot (§11.2).
                ledger.checkpoint_wall(super::now_ms());
                persist_spent(&conn, run_id, &ledger);
            }

            match result.outcome {
                AttemptOutcome::Done { .. } => {
                    let wt = result
                        .worktree
                        .ok_or_else(|| Error::Other("done attempt without a worktree".into()))?;
                    // Boundary commit + pin + ferry — the `done` precondition
                    // (§6.3 steps 7–8). A ferry failure keeps the attempt out of
                    // `done` and drops to the retry policy.
                    let msg = format!("wf({}): {} attempt {}", spec.name, step.id, attempt_no);
                    let ferry = ferry_step(ctx, run_id, &exec_id, &msg, &wt, &run_repo).await;
                    match ferry {
                        Ok(head) => {
                            {
                                let conn = ctx.db.lock();
                                finish_step_exec(&conn, &exec_id, "done", Some(&head));
                            }
                            if let Some(agent_id) = &result.agent_id {
                                let _ = ctx.driver.archive(agent_id).await;
                            }
                            last_ref = gitops::step_ref(&exec_id);
                            last_exec_id = Some(exec_id.clone());
                            index += 1;
                            set_cursor_index(&ctx.db.lock(), run_id, index as i64);
                            break;
                        }
                        Err(e) => {
                            last_failure = Some(format!("ferry failed: {e}"));
                            // The attempt reached `done`, so its agent is idle but
                            // still alive. Marking the exec `error` hides it from
                            // `live_step_agents` (which only sees in-flight execs),
                            // so stop+archive it here — otherwise the CLI process
                            // leaks past the retry/fail and its chat is orphaned.
                            if let Some(agent_id) = &result.agent_id {
                                let _ = ctx.driver.stop(agent_id).await;
                                let _ = ctx.driver.archive(agent_id).await;
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
                                return Ok(());
                            }
                        }
                    }
                    attempt_no += 1;
                }
                AttemptOutcome::AwaitingApproval => {
                    // Commit the work now so approval only decides whether to
                    // advance (§6.3 step 8, §9). The agent is archived; the run
                    // pauses until `wf_approve` bumps the cursor and resumes.
                    let msg = format!("wf({}): {} attempt {}", spec.name, step.id, attempt_no);
                    let head = ferry_step(
                        ctx,
                        run_id,
                        &exec_id,
                        &msg,
                        result.worktree.as_ref().unwrap(),
                        &run_repo,
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
                    return Ok(());
                }
                AttemptOutcome::Blocked { reason } => {
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
                    return Ok(());
                }
                AttemptOutcome::Error { error } => {
                    {
                        let conn = ctx.db.lock();
                        finish_step_exec(&conn, &exec_id, "error", None);
                    }
                    last_failure = Some(error.clone());
                    if attempt_no >= max_attempts {
                        // Stall pauses for inspection (resumable); other errors
                        // fail the run (§6.5).
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
                        return Ok(());
                    }
                    attempt_no += 1;
                }
                AttemptOutcome::BudgetExceeded { .. } => {
                    // A run-level cap was hit mid-attempt (§11.2). The attempt
                    // already journaled `budget_exceeded`; finish its bookkeeping
                    // — stop the agent, abandon the incomplete attempt — and pause.
                    // Resume-with-patch (§13) starts a fresh attempt for this step.
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
                    finish_budget_pause(ctx, run_id, Some(&exec_id), &mut ledger);
                    return Ok(());
                }
            }
        }
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
    let bc = gitops::boundary_commit(worktree, message).await?;
    {
        let conn = ctx.db.lock();
        journal_event(
            &conn,
            ctx.app.as_ref(),
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

/// Top-level steps of a linear run. A non-step block fails the run (loop /
/// parallel / orchestrate execution is S7 / S8 / S11).
fn extract_linear_steps(spec: &Spec) -> Result<Vec<Step>> {
    spec.workflow
        .iter()
        .map(|b| match b {
            Block::Step(s) => Ok(s.clone()),
            Block::Loop(_) => Err(Error::Other(
                "loop blocks are not supported yet (S7)".into(),
            )),
            Block::Parallel(_) => Err(Error::Other(
                "parallel blocks are not supported yet (S8)".into(),
            )),
            Block::Orchestrate(_) => Err(Error::Other(
                "orchestrate blocks are not supported yet (S11)".into(),
            )),
        })
        .collect()
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
/// run may be re-driven. A terminal run must not restart, and a
/// `paused(approval)` run must go through `wf_approve`. Callers run this before
/// any state mutation so a rejected resume changes nothing.
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
    Ok(())
}

fn run_status(conn: &Connection, run_id: &str) -> Result<(String, Option<String>)> {
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
fn journal_event(
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
    gate_mode: &str,
) {
    let _ = conn.execute(
        "INSERT INTO wf_step_exec (id, run_id, step_id, attempt, iteration, status, gate_mode)
         VALUES (?1, ?2, ?3, ?4, 0, 'spawning', ?5)",
        rusqlite::params![id, run_id, step_id, attempt, gate_mode],
    );
}

fn finish_step_exec(conn: &Connection, id: &str, status: &str, head_end: Option<&str>) {
    let _ = conn.execute(
        "UPDATE wf_step_exec SET status = ?1, head_end = ?2, ended_at = ?3 WHERE id = ?4",
        rusqlite::params![status, head_end, super::now_ms(), id],
    );
}

fn next_attempt_no(conn: &Connection, run_id: &str, step_id: &str) -> i64 {
    conn.query_row(
        "SELECT COALESCE(MAX(attempt), 0) + 1 FROM wf_step_exec WHERE run_id = ?1 AND step_id = ?2",
        rusqlite::params![run_id, step_id],
        |r| r.get(0),
    )
    .unwrap_or(1)
}

fn get_cursor_index(conn: &Connection, run_id: &str) -> i64 {
    let cursor: Option<String> = conn
        .query_row(
            "SELECT cursor_json FROM wf_run WHERE id = ?1",
            [run_id],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    cursor
        .and_then(|c| serde_json::from_str::<Value>(&c).ok())
        .and_then(|v| v.get("index").and_then(|i| i.as_i64()))
        .unwrap_or(0)
}

fn set_cursor_index(conn: &Connection, run_id: &str, index: i64) {
    let _ = conn.execute(
        "UPDATE wf_run SET cursor_json = ?1, updated_at = ?2 WHERE id = ?3",
        rusqlite::params![
            json!({ "index": index }).to_string(),
            super::now_ms(),
            run_id
        ],
    );
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

/// The exec id of the most recent `done` step (the fork source for the next).
fn latest_done_exec(conn: &Connection, run_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT id FROM wf_step_exec WHERE run_id = ?1 AND status = 'done' ORDER BY rowid DESC LIMIT 1",
        [run_id],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

fn latest_done_ref(conn: &Connection, run_id: &str) -> Option<String> {
    latest_done_exec(conn, run_id).map(|id| gitops::step_ref(&id))
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
#[tauri::command]
pub async fn wf_launch(
    spec: Spec,
    task: String,
    project_id: String,
    repo_path: String,
    definition_id: Option<String>,
    base_branch: Option<String>,
    service: Svc<'_>,
) -> std::result::Result<String, String> {
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
        insert("r-budg", "paused", Some("budget_exceeded"));
        insert("r-blk", "paused", Some("blocked_gate"));

        let conn = db.lock();
        assert!(check_resumable(&conn, "r-done", "resume").is_err());
        assert!(check_resumable(&conn, "r-appr", "resume").is_err());
        assert!(check_resumable(&conn, "r-budg", "resume").is_ok());
        assert!(check_resumable(&conn, "r-blk", "retry").is_ok());
    }
}
