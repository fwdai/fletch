use super::*;

/// The single place a driver handle enters the run registry: insert it and mark
/// the run active in one step, under the caller's lock. Every registration site
/// (main runs via [`spawn_drive_task`], orchestrate sub-runs via [`spawn_subrun`],
/// and the respawn path) goes through here, so a site can't insert a driver yet
/// forget to tell the [`ActivityMonitor`] — the invariant "a run with a live
/// driver counts as active" is enforced by construction, not convention.
///
/// Caller must hold the registry lock and must have checked the run has no live
/// handle (a run has at most one live driver, §6.1). Marking active *under* the
/// lock is what upholds the ordering guarantee against [`deregister_driver`]: a
/// clear done under the lock strictly precedes any later re-insert.
pub(crate) fn register_driver(m: &mut HashMap<String, RunHandle>, run_id: &str, handle: RunHandle) {
    m.insert(run_id.to_string(), handle);
    crate::power::ActivityMonitor::global().set_run_active(run_id, true);
}

/// The single place a driver handle leaves the registry: remove it and clear the
/// run's activity in one step, under the caller's lock. Clearing under the lock
/// is what lets a concurrent re-registration's `true` win — the clear strictly
/// precedes any subsequent insert, so the newest driver's state is the final
/// one. Activity is a per-run-id boolean, so deregistering a finished sub-run
/// never clears the parent run's own activity (distinct ids).
pub(crate) fn deregister_driver(m: &mut HashMap<String, RunHandle>, run_id: &str) {
    m.remove(run_id);
    crate::power::ActivityMonitor::global().set_run_active(run_id, false);
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
pub(crate) fn spawn_drive_task(
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
        // A registered drive task is exactly a run in `pending`/`running` (a
        // `paused`/terminal run has no live driver), so registry membership is
        // the faithful, no-poll signal for "workflow work is active" that the
        // sleep assertion + menu-bar status line consume. `register_driver`
        // marks it active under this lock; inert until the app arms the monitor
        // at setup, so tests that spin drive tasks touch no power state.
        register_driver(
            &mut m,
            &run_id,
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
        runs: Some(runs.clone()),
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
            fail_run(&conn, Some(&app), &run_id, "internal scheduler error");
        }
        // Deregister under the same lock as the respawn-flag read so a request
        // can't slip between the two. The clear always happens here; if a
        // respawn is due, `spawn_drive_task` below re-registers (re-marking the
        // run active) — the monitor's release debounce absorbs the microsecond
        // gap, so the sleep assertion never flaps across a respawn. Routing
        // through `deregister_driver` keeps the clear strictly ordered before
        // any concurrent re-insert (round-1 invariant).
        let respawn_requested = {
            let mut m = runs.lock();
            deregister_driver(&mut m, &run_id);
            respawn.load(Ordering::SeqCst) && !panicked
        };
        if respawn_requested {
            spawn_drive_task(db, driver, app, runs, run_id);
        }
    });
}

// ───────────────────────────── the drive loop ───────────────────────────────

/// Drive one run to a terminal or paused state. Any error bubbling out marks the
/// run `failed` with the cause (the panic watchdog covers a hard panic).
pub(crate) async fn drive_run(ctx: &RunCtx, run_id: &str) {
    if let Err(e) = drive_run_inner(ctx, run_id).await {
        let conn = ctx.db.lock();
        fail_run(&conn, ctx.app.as_ref(), run_id, &e.to_string());
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
    ledger.start_drive(crate::workflow::now_ms());

    let mut cursor = get_cursor(&ctx.db.lock(), run_id);
    let mut index = cursor.index as usize;
    // The fork source for the current block: the last *linear step* before the
    // cursor that reached `done`, else the run base. A parallel `integrate: none`
    // stage never advances the line (§12.3), so its done children must not be
    // mistaken for the fork source on resume — hence a block-tree walk rather
    // than "the most recent done exec".
    let (mut last_ref, mut last_exec_id) =
        resume_line_state(&ctx.db.lock(), run_id, blocks, index, &run.base_sha);

    // Launch attachments (durable, read-only): delivered to the entry step's
    // prompt only. Re-read every drive, so a resume redelivers with no state to
    // reconcile.
    let launch_attachments = blackboard::read_attachments(&run_dir);

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
        base_sha: &run.base_sha,
        launch_attachments: &launch_attachments,
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
                    &mut cursor,
                )
                .await?
                {
                    // With no composed `integrate: merge` sub-run the stage HEAD is
                    // its entry HEAD (§12.3) and `line` is `None`; a merged sub-run
                    // advances the line onto the integrated result (§10.3), handled
                    // uniformly with the parallel arm.
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
pub(crate) async fn ferry_step(
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
pub(crate) async fn ferry_committed(
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
pub(crate) fn commit_done_unless_ask(conn: &Connection, exec_id: &str, head: &str) -> bool {
    if crate::workflow::comms::has_unanswered_ask(conn, exec_id) {
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
pub(crate) async fn pause_question(
    ctx: &RunCtx,
    run_id: &str,
    exec_id: &str,
    agent_id: Option<&str>,
) {
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
    // The spec's explicit `pr_base` wins; otherwise fall back to the branch the
    // run was launched from, so a run forked off `develop` opens its PR against
    // `develop` rather than `main`. Empty `base_branch` means none was selected.
    let base = fin
        .pr_base
        .clone()
        .filter(|b| !b.is_empty())
        .or_else(|| Some(run.base_branch.clone()).filter(|b| !b.is_empty()))
        .unwrap_or_else(|| "main".to_string());
    let title = format!("wf: {}", spec.name);
    // A run started from a Home-inbox issue carries its number; append a
    // `Closes #<n>` trailer so merging the finalized PR closes the issue. The
    // run repo is inherently the issue's repo (single-repo), so no subdir
    // gating — unlike the multi-repo agent path. Idempotent; `None` (a normal
    // launch) leaves the empty body untouched.
    let close_issue = run.issue_ref.as_deref().and_then(|s| s.trim().parse().ok());
    let body = crate::github::with_closes_trailer("", close_issue);
    let outcome = gitops::finalize(
        run_repo,
        &final_ref,
        &run.branch,
        &base,
        &title,
        &body,
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
/// (S7) executes bodies of plain steps; orchestrate (S11) runs `integrate: none`
/// only. `spec::validate` rejects both unsupported shapes at save/import now, so
/// this is a backstop for definitions persisted before that rule existed.
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
        abandon_exec(&conn, ctx.app.as_ref(), run_id, &exec_id, "resume");
    }
}

// ───────────────────────────── commands (§13) ───────────────────────────────
