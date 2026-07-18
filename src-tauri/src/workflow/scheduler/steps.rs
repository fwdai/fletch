use super::*;

pub(crate) fn resolve_agent<'a>(spec: &'a Spec, step: &Step) -> Result<&'a AgentSpec> {
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
pub(crate) async fn cancel_run(ctx: &RunCtx, run_id: &str) {
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
pub(crate) async fn run_loop(
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
                fail_run(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    "nested non-step blocks inside a loop are not supported yet",
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
pub(crate) async fn execute_step(
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
    let test_runner = crate::workflow::tests_gate::SandboxTestRunner::new(
        env.test_override.clone(),
        env.setup_override.clone(),
        step_eff.tests_timeout_secs.max(1) as u64,
    )?;

    let mut attempt_no = next_attempt_no(&ctx.db.lock(), run_id, &step.id, iteration as i64);
    let mut last_failure: Option<String> = None;

    loop {
        // Enforcement point: before every spawn (§11.2). No attempt row is
        // created — the run pauses at the block boundary.
        if let Some(which) = ledger.exceeded(&step_eff, crate::workflow::now_ms()) {
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

        // Launch attachments belong to the run's initial task, so they render on
        // the entry step only — the first top-level block's step (index 0, not
        // inside a loop). Every other step, retry iteration aside, sees none.
        let entry_attachments: &[String] =
            if position.step_index == 0 && position.iteration.is_none() {
                env.launch_attachments
            } else {
                &[]
            };
        let prompt = {
            let ctx_prompt = StepPromptCtx {
                run_task: env.run_task,
                attachments: entry_attachments,
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
                crate::workflow::comms::take_pending_deliveries(&conn, run_id, &step.id)
            };
            if delivered.is_empty() {
                base
            } else {
                format!(
                    "{}\n\n{}",
                    crate::workflow::comms::compose_delivery(&delivered),
                    base
                )
            }
        };

        let params = AttemptParams {
            spawn_req: {
                let conn = ctx.db.lock();
                build_spawn_req(
                    &conn,
                    ctx.app.as_ref(),
                    agent_spec,
                    fork_ref,
                    env.repo,
                    env.run_repo,
                    run_id,
                    Some(&exec_id),
                )
            },
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
            // The run's cancel flag (§6.5): `WorkflowService::cancel` sets it and
            // the attempt's cancel checkpoints/races observe it, so a cancel
            // lands mid-spawn or mid-turn instead of after the block finishes.
            cancel: ctx.cancel.clone(),
            // Shared with the run's `RunHandle` so the comms router and this
            // attempt observe the same pending-ask flag (§10.4).
            pending_ask: ctx.pending_ask.clone(),
        };

        let started = crate::workflow::now_ms();
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
            ledger.checkpoint_wall(crate::workflow::now_ms());
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
        let outcome = if crate::workflow::comms::has_unanswered_ask(&ctx.db.lock(), &exec_id) {
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
                                fail_run(
                                    &conn,
                                    ctx.app.as_ref(),
                                    run_id,
                                    &format!("ferry failed: {e}"),
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
                let worktree = result.worktree.as_ref().unwrap();
                let head = ferry_step(ctx, run_id, &exec_id, &msg, worktree, env.run_repo).await?;
                // Assemble the review evidence while the worktree is intact (spec
                // §9): verification, the ferried diff vs the run base, budget
                // spend, and the step's verdict — journaled so ReviewSurface can
                // render it without re-deriving anything. No lock is held across
                // the (async) verification + git work.
                let evidence = assemble_gate_evidence(
                    env,
                    &step.id,
                    worktree,
                    &head,
                    step_eff.tests_timeout_secs.max(1) as u64,
                    ledger,
                )
                .await;
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
                    event_type::GATE_EVIDENCE,
                    Some(&exec_id),
                    &evidence,
                );
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
                        fail_run(&conn, ctx.app.as_ref(), run_id, &error);
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
                    abandon_exec(&conn, ctx.app.as_ref(), run_id, &exec_id, "budget_exceeded");
                }
                finish_budget_pause(ctx, run_id, Some(&exec_id), ledger);
                return Ok(StepFlow::Halt);
            }
            AttemptOutcome::Canceled => {
                // The run was cancelled mid-attempt (§6.5). `run_attempt` already
                // stopped the agent; abandon the row, archive the chat, and
                // complete the cancel here — the drive loop's between-blocks
                // check never runs again after a Halt, so the terminal status
                // must be written now. Lock scoped so the guard drops before
                // the archive await.
                {
                    let conn = ctx.db.lock();
                    abandon_exec(&conn, ctx.app.as_ref(), run_id, &exec_id, "canceled");
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
                if let Some(agent_id) = &result.agent_id {
                    let _ = ctx.driver.archive(agent_id).await;
                }
                return Ok(StepFlow::Halt);
            }
        }
    }
}

/// Assemble an `approval` gate's review evidence (spec §9), as the `gate_evidence`
/// payload (snake_case, like every IPC payload): the verification report (the
/// shared `Verifier` primitive run install→test→lint in the step worktree —
/// all-`Skipped` when the project configures nothing or the host can't sandbox),
/// the ferried diff versus the run base (shortstat + per-file numstat, both taken
/// in the run repo where the ferried ref and the base commit live), budget spend
/// versus cap, and the step's `verdict.json`. Best-effort: any piece that can't be
/// gathered is omitted / `null` so evidence collection never blocks the pause.
/// Holds no DB lock across its async verification + git work.
async fn assemble_gate_evidence(
    env: &StepEnv<'_>,
    step_id: &str,
    worktree: &Path,
    head_sha: &str,
    tests_timeout_secs: u64,
    ledger: &Ledger,
) -> Value {
    // Reuse the engine-owned verifier. Lint resolves by detection (no project
    // lint override is plumbed to the linear path); a HOME-less host yields no
    // verifier, reported as `null` rather than a fake empty report.
    let verification = match crate::verify::Verifier::new(
        env.test_override.clone(),
        env.setup_override.clone(),
        None,
        tests_timeout_secs,
    ) {
        Ok(v) => serde_json::to_value(v.verify(worktree).await).ok(),
        Err(_) => None,
    };

    let (additions, deletions) = crate::git::diff_shortstat(env.run_repo, env.base_sha, head_sha)
        .await
        .unwrap_or((0, 0));
    let files: Vec<Value> = crate::git::diff_numstat(env.run_repo, env.base_sha, head_sha)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(path, a, d)| json!({ "path": path, "additions": a, "deletions": d }))
        .collect();

    let verdict = blackboard::step_dir(env.blackboard, step_id)
        .ok()
        .and_then(|dir| blackboard::read_verdict(&dir).ok())
        .and_then(|v| serde_json::to_value(v).ok());

    json!({
        "base_sha": env.base_sha,
        "head_sha": head_sha,
        "verification": verification,
        "diff": { "additions": additions, "deletions": deletions, "files": files },
        "budget": {
            "turns_spent": ledger.turns,
            "turns_cap": env.eff.turns,
            "tokens_spent": ledger.tokens,
            "tokens_cap": env.eff.tokens,
            "wall_ms_spent": ledger.wall_ms,
            "wall_clock_cap_mins": env.eff.wall_clock_mins,
        },
        "verdict": verdict,
    })
}

pub(crate) fn gate_mode(gate: &Gate) -> &'static str {
    match gate {
        Gate::Verdict => "verdict",
        Gate::Commit => "commit",
        Gate::Artifact { .. } => "artifact",
        Gate::Tests => "tests",
        Gate::Approval { .. } => "approval",
    }
}

pub(crate) fn slugify(name: &str) -> String {
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
pub(crate) fn check_resumable(conn: &Connection, run_id: &str, action: &str) -> Result<()> {
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

/// The DB half of `wf_reject` (spec §9), factored out so it is unit-testable
/// without the async re-drive. Validates the pause, journals the human decision,
/// abandons the rejected attempt, and either (budget left) queues the reviewer's
/// note as a delivery and returns `true` — telling the caller to re-drive — or
/// (budget spent) pauses `blocked_gate` with the note as detail and returns
/// `false`. Runs entirely under the caller's connection lock.
pub(crate) fn reject_apply(
    conn: &Connection,
    app: Option<&AppHandle>,
    run_id: &str,
    note: &str,
) -> Result<bool> {
    let note = note.trim();
    if note.is_empty() {
        return Err(Error::Other("a rejection note is required".into()));
    }
    let (status, reason) = run_status(conn, run_id)?;
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

    // Record the human decision on the timeline (spec §7.1).
    journal_event(
        conn,
        app,
        run_id,
        event_type::DECISION,
        Some(&exec_id),
        &json!({ "decision": "rejected", "note": note }),
    );

    // Would a fresh attempt immediately hit the run budget? Mirror the drive
    // loop's pre-spawn enforcement point (§11.2) against the frozen caps and the
    // persisted ledger.
    let run = load_run(conn, run_id)?;
    let eff: EffectiveBudgets = serde_json::from_str(&run.budgets_json).unwrap_or_default();
    let spent: Value = serde_json::from_str(&run.spent_json).unwrap_or_else(|_| json!({}));
    let exhausted = Ledger::from_json(&spent)
        .exceeded(&eff, crate::workflow::now_ms())
        .is_some();

    // Either way the rejected attempt is done with: abandon it so it stops
    // counting as awaiting_approval and its ferried (now discarded) ref is never
    // mistaken for the line's fork source (`resume_line_state` only follows `done`
    // execs).
    abandon_exec(conn, app, run_id, &exec_id, "rejected");

    if exhausted {
        journal_event(
            conn,
            app,
            run_id,
            event_type::RUN_PAUSED,
            Some(&exec_id),
            &json!({ "reason": "blocked_gate", "detail": note }),
        );
        set_status(conn, app, run_id, "paused", Some("blocked_gate"), None);
        Ok(false)
    } else {
        crate::workflow::comms::queue_rejection(conn, run_id, &exec_id, note);
        Ok(true)
    }
}
