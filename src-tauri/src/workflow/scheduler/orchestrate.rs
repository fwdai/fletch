use super::*;

/// An orchestrate child's terminal outcome plus the exec id (for lifecycle
/// forwarding) — the orchestrate analogue of [`ChildResult`].
pub(crate) struct OrchChildResult {
    pub(crate) step_id: String,
    pub(crate) exec_id: String,
    /// The launch generation of the attempt that produced this result — the stage
    /// discards it if a later `retry_child` has superseded this generation.
    pub(crate) generation: u64,
    pub(crate) outcome: ChildOutcome,
    pub(crate) ledger: Ledger,
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
    Budget,
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
pub(crate) async fn run_orchestrate_stage(
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
    cursor: &mut Cursor,
) -> Result<StageFlow> {
    // Resume: a sub-run integration paused mid-merge or on a conflict (§10.3,
    // §12.3). The sub-runs already ran and ferried; continue merging / apply the
    // recorded resolution rather than re-driving the orchestrator.
    if cursor
        .merge
        .as_ref()
        .is_some_and(|m| m.block_index == block_index)
    {
        return resume_subrun_merge(
            ctx,
            run_id,
            run,
            spec,
            orch,
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

    // Enforcement point: before spawning the stage (§11.2).
    if let Some(which) = ledger.exceeded(eff, crate::workflow::now_ms()) {
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

    let orch_step_id = crate::workflow::comms::orch_step_id(block_index);
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
    let orch_req = {
        let conn = ctx.db.lock();
        build_spawn_req(
            &conn,
            ctx.app.as_ref(),
            orch_agent,
            fork_base,
            repo,
            run_repo,
            run_id,
            Some(&orch_exec),
        )
    };
    let spawned = match ctx.driver.spawn(orch_req).await {
        Ok(s) => s,
        Err(e) => {
            let conn = ctx.db.lock();
            finish_step_exec(&conn, &orch_exec, "error", None);
            fail_run(
                &conn,
                ctx.app.as_ref(),
                run_id,
                &format!("orchestrator spawn failed: {e}"),
            );
            return Ok(StageFlow::Stop);
        }
    };
    let orch_agent_id = spawned.agent_id.clone();
    {
        let conn = ctx.db.lock();
        let _ = conn.execute(
            "UPDATE wf_step_exec SET agent_id = ?1, status = 'running', started_at = ?2 WHERE id = ?3",
            rusqlite::params![orch_agent_id, crate::workflow::now_ms(), orch_exec],
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
        fail_run(
            &conn,
            ctx.app.as_ref(),
            run_id,
            &format!("orchestrator not ready: {e}"),
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
    // The `Step` each child (static *or* dynamically spawned) was launched from,
    // keyed by step id — so `retry_child` can rebuild any child, not just the
    // static-body ones (spec §10.2).
    let mut child_specs: HashMap<String, Step> = HashMap::new();
    // The join outcome recorded for each child; restored from prior drives below.
    let mut outcomes: HashMap<String, ChildStatus> = HashMap::new();
    // Rebuild the dynamic children of any prior drive into the registry from the
    // persisted `spawn_child` decisions (which carry their agent + goal), so a
    // resumed stage can still honor `retry_child` for `orchestrate-N::dyn-K`.
    // Spawn order == index order, matching how ids were assigned originally.
    // Bind first so the DB guard drops before the loop — the body re-locks it.
    let prior_dynamic = prior_spawn_decisions(&ctx.db.lock(), run_id, &orch_step_id);
    for (i, (agent, goal)) in prior_dynamic.into_iter().enumerate() {
        let id = format!("{orch_step_id}::dyn-{i}");
        child_specs.insert(
            id.clone(),
            Step {
                id: id.clone(),
                agent,
                goal,
                gate: Gate::Verdict,
                budgets: None,
                comms: orch.comms.clone(),
            },
        );
        // Restore a prior dynamic child's terminal join outcome so the resumed
        // stage doesn't decide the join without it (dynamic children aren't
        // auto-relaunched — the orchestrator re-drives them via retry_child).
        if let Some(status) = latest_exec_status(&ctx.db.lock(), run_id, &id) {
            if let Some(restored) = restored_child_status(&status) {
                outcomes.insert(id, restored);
            }
        }
    }
    // Seed the dynamic-child index from the DB so a resumed stage doesn't reuse an
    // id an earlier drive already created (ids stay unique across resume).
    let mut dyn_count = existing_dyn_child_count(&ctx.db.lock(), run_id, &orch_step_id);

    // Composed sub-runs of this stage (spec §10.3), keyed by sub-run id. Rebuilt
    // from the DB on resume so a restarted stage keeps tracking sub-runs launched
    // by a prior drive (and re-drives any left `pending`/`running`).
    let mut sub_runs: HashMap<String, SubRunState> =
        rebuild_sub_runs(&ctx.db.lock(), run_id, block_index);
    for (sub_id, st) in &sub_runs {
        if st.terminal.is_none() {
            // A live sub-run abandoned by the restart: re-drive its own task.
            spawn_subrun(ctx, sub_id.clone());
        }
    }

    for step in &orch.body {
        // Resume: a static child that terminally finished in a prior drive keeps
        // its join outcome and is not re-run (§6.6, §12.3); an in-flight one
        // (abandoned by the resume) or one that never ran is launched fresh.
        if let Some(status) = latest_exec_status(&ctx.db.lock(), run_id, &step.id) {
            if let Some(restored) = restored_child_status(&status) {
                outcomes.insert(step.id.clone(), restored);
                continue;
            }
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
        child_specs.insert(step.id.clone(), step.clone());
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
            compose_max_sub_runs: orch.compose.as_ref().map(|c| c.max_sub_runs),
        });
        // On a resume after escalation, an answer for the orchestrator is folded in.
        let delivered = {
            let conn = ctx.db.lock();
            crate::workflow::comms::take_pending_deliveries(&conn, run_id, &orch_step_id)
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
            cancel_sub_runs(ctx, &sub_runs).await;
            cancel_run(ctx, run_id).await;
            return Ok(StageFlow::Stop);
        }

        // Orchestrator escalated or asked the human on its last turn → pause
        // `question` (§10.2, §10.4). The backstop mirrors the linear path.
        if crate::workflow::comms::has_unanswered_ask(&ctx.db.lock(), &orch_exec) {
            drain_children(&child_cancels, &mut set).await;
            cancel_sub_runs(ctx, &sub_runs).await;
            pause_question(ctx, run_id, &orch_exec, Some(&orch_agent_id)).await;
            return Ok(StageFlow::Stop);
        }

        // Execute the decisions the orchestrator issued last turn (§10.2).
        let decisions = {
            let conn = ctx.db.lock();
            crate::workflow::comms::take_orchestrator_decisions(&conn, run_id, &orch_exec)
        };
        for d in decisions {
            match d {
                crate::workflow::comms::Decision::StageDone => concluded_early = true,
                crate::workflow::comms::Decision::Compose(plan) => {
                    // Launch the validated sub-run's own driver (spec §10.3),
                    // reserve its budget slice out of the parent ledger, and track
                    // it for the join. A provisioning failure is journaled and the
                    // slice is not reserved — the stage keeps running.
                    match launch_subrun(ctx, run, run_id, run_repo, fork_base, &plan).await {
                        Ok(sub_id) => {
                            ledger.reserve(plan.turns, plan.tokens.unwrap_or(0));
                            {
                                let conn = ctx.db.lock();
                                persist_spent(&conn, run_id, ledger);
                                journal_event(
                                    &conn,
                                    ctx.app.as_ref(),
                                    run_id,
                                    event_type::SUBRUN_LAUNCHED,
                                    Some(&orch_exec),
                                    // The extra fields let a resumed stage rebuild
                                    // its sub-run tracking without a side table.
                                    &json!({
                                        "sub_run_id": sub_id,
                                        "block_index": block_index,
                                        "integrate": if matches!(plan.integrate, Integrate::Merge) { "merge" } else { "none" },
                                        "reserved_turns": plan.turns,
                                        "reserved_tokens": plan.tokens.unwrap_or(0),
                                    }),
                                );
                            }
                            sub_runs.insert(
                                sub_id.clone(),
                                SubRunState {
                                    integrate: plan.integrate,
                                    reserved_turns: plan.turns,
                                    reserved_tokens: plan.tokens.unwrap_or(0),
                                    terminal: None,
                                },
                            );
                            spawn_subrun(ctx, sub_id);
                        }
                        Err(e) => {
                            let conn = ctx.db.lock();
                            journal_event(
                                &conn,
                                ctx.app.as_ref(),
                                run_id,
                                event_type::COMPOSE_DENIED,
                                Some(&orch_exec),
                                &json!({ "reason": format!("sub-run launch failed: {e}") }),
                            );
                        }
                    }
                }
                crate::workflow::comms::Decision::SpawnChild { agent, goal } => {
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
                        child_specs.insert(step.id.clone(), step.clone());
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
                crate::workflow::comms::Decision::SkipChild { step_id, .. } => {
                    // Cancel the child's live task so it stops spending budget and
                    // the stage can conclude without waiting on it, then record it
                    // as satisfied for the join (§10.2).
                    if let Some(flag) = child_cancels.get(&step_id) {
                        flag.store(true, Ordering::SeqCst);
                    }
                    outcomes.insert(step_id, ChildStatus::Skipped);
                }
                crate::workflow::comms::Decision::RetryChild { step_id, guidance } => {
                    // Resolve from the live child registry, so a *dynamic* child
                    // (`orchestrate-N::dyn-K`, absent from `orch.body`) can be
                    // retried too, not just static-body children (§10.2).
                    if let Some(orig) = child_specs.get(&step_id).cloned() {
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

        // Reap finished sub-runs (spec §10.3): reconcile each one's reserved slice
        // against its actual spend, forward its outcome to the orchestrator, and
        // record its join status alongside the children's.
        reap_sub_runs(
            ctx,
            run_id,
            &orch_exec,
            ledger,
            &mut sub_runs,
            &mut outcomes,
        );

        // join `all`: the first child failure fails the stage.
        if matches!(orch.join, Join::All) {
            if let Some(reason) = outcomes.values().find_map(|s| match s {
                ChildStatus::Failure(r) => Some(r.clone()),
                _ => None,
            }) {
                drain_children(&child_cancels, &mut set).await;
                cancel_sub_runs(ctx, &sub_runs).await;
                let _ = ctx.driver.stop(&orch_agent_id).await;
                let conn = ctx.db.lock();
                finish_step_exec(&conn, &orch_exec, "error", None);
                persist_spent(&conn, run_id, ledger);
                fail_run(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    &format!("orchestrate stage failed: {reason}"),
                );
                return Ok(StageFlow::Stop);
            }
        }

        // The join is met only when every child *and* every composed sub-run
        // (spec §6.6, §10.3) is terminal.
        if set.is_empty() && sub_runs.values().all(|s| s.terminal.is_some()) {
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
                fail_run(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    "orchestrate stage failed: all children failed",
                );
                return Ok(StageFlow::Stop);
            }

            if !conclude_sent {
                let prompt = {
                    let inbox = {
                        let conn = ctx.db.lock();
                        crate::workflow::comms::take_orchestrator_inbox(&conn, run_id, &orch_exec)
                    };
                    let mut p = if inbox.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "{}\n\n",
                            crate::workflow::comms::compose_orchestrator_inbox(&inbox)
                        )
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
                Ok(v) if matches!(v.result, crate::workflow::blackboard::VerdictResult::Done)
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
                {
                    let conn = ctx.db.lock();
                    finish_step_exec(&conn, &orch_exec, "done", None);
                    persist_spent(&conn, run_id, ledger);
                }
                // Integrate composed sub-runs at the join (spec §10.3): merge each
                // `integrate: merge` sub-run's ferried ref into the stage line. No
                // merge sub-runs → the stage HEAD is its entry HEAD (`line: None`).
                return begin_subrun_merge(
                    ctx,
                    run_id,
                    run,
                    spec,
                    block_index,
                    run_repo,
                    fork_base,
                    &sub_runs,
                    cursor,
                )
                .await;
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
            crate::workflow::comms::take_orchestrator_inbox(&conn, run_id, &orch_exec)
        };
        if !inbox.is_empty() {
            let prompt = crate::workflow::comms::compose_orchestrator_inbox(&inbox);
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
        if let Some(which) = ledger.exceeded(eff, crate::workflow::now_ms()) {
            drain_children(&child_cancels, &mut set).await;
            cancel_sub_runs(ctx, &sub_runs).await;
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
        // Wait for the next event. With children still live, race a child join
        // against a poll tick; with only sub-runs left (an empty `JoinSet` yields
        // `None` immediately, which would busy-spin), just poll their status.
        if set.is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        } else {
            tokio::select! {
                joined = set.join_next() => {
                    if let Some(j) = joined {
                        handle_orch_child(ctx, run_id, &orch_exec, orch.join, ledger, j, &mut outcomes, &child_cancels, &child_gen);
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {}
            }
        }
    }

    // `stage_done`: wind the remaining children and sub-runs down and mark the
    // stage done. Composed sub-runs are canceled (not integrated) on an early
    // `stage_done` — the orchestrator chose to end without waiting for them.
    drain_children(&child_cancels, &mut set).await;
    cancel_sub_runs(ctx, &sub_runs).await;
    let _ = ctx.driver.archive(&orch_agent_id).await;
    let conn = ctx.db.lock();
    finish_step_exec(&conn, &orch_exec, "done", None);
    persist_spent(&conn, run_id, ledger);
    Ok(StageFlow::Advance { line: None })
}

// ───────────────────────── composed sub-runs (§10.3) ────────────────────────

/// A composed sub-run tracked by its parent orchestrate stage.
pub(crate) struct SubRunState {
    pub(crate) integrate: Integrate,
    /// The reserved budget slice, released and reconciled when the sub-run ends.
    pub(crate) reserved_turns: i64,
    pub(crate) reserved_tokens: i64,
    /// `None` while the sub-run is live; its join outcome once terminal.
    pub(crate) terminal: Option<ChildStatus>,
}

/// Create and provision a composed sub-run (spec §10.3): a synthetic spec from the
/// validated fragment, the fork base resolved from the parent, its own run dir +
/// run repo, and a `wf_run` row with `parent_run_id`. Returns the new sub-run id;
/// the caller reserves the budget slice, spawns the driver, and tracks it.
async fn launch_subrun(
    ctx: &RunCtx,
    parent: &RunEssentials,
    parent_run_id: &str,
    parent_run_repo: &Path,
    fork_base: &str,
    plan: &crate::workflow::comms::ComposePlan,
) -> Result<String> {
    let sub_run_id = format!("run-{}", uuid::Uuid::new_v4());
    let parent_spec: Spec =
        serde_json::from_str(&parent.spec_json).map_err(|e| Error::Other(e.to_string()))?;
    let agents = plan
        .agents
        .clone()
        .unwrap_or_else(|| parent_spec.agents.clone());
    let sub_spec = Spec {
        version: parent_spec.version,
        name: format!("{} — sub-run", parent_spec.name),
        description: None,
        budgets: Some(Budgets {
            turns: Some(plan.turns),
            tokens: plan.tokens,
            ..Default::default()
        }),
        agents,
        workflow: plan.fragment.clone(),
        // A sub-run integrates at the parent's join; it never pushes/PRs itself.
        finalize: None,
    };
    let spec_json = serde_json::to_string(&sub_spec).map_err(|e| Error::Other(e.to_string()))?;
    let budgets_json = serde_json::to_string(&EffectiveBudgets::resolve(&sub_spec))
        .map_err(|e| Error::Other(e.to_string()))?;

    let run_dir = blackboard::run_dir(&sub_run_id)?;
    let task_md = format!("# {}\n\n{}\n", sub_spec.name, plan.task);
    blackboard::provision(&run_dir, &task_md)?;
    let repo = PathBuf::from(&parent.repo_path);
    let sub_run_repo = gitops::provision_run_repo(&repo, &run_dir).await?;

    // Resolve the fork base (§10.3). `run-base` is the parent's original base (a
    // source commit, already in the sub-run's clone). `parent-head` is the stage's
    // current line — a workflow commit that lives only in the parent run repo, so
    // ferry its ref in before the sub-run's step 1 provisions from it.
    let base_sha = match plan.base {
        crate::workflow::comms::ComposeBase::RunBase => parent.base_sha.clone(),
        crate::workflow::comms::ComposeBase::ParentHead => fork_base.to_string(),
    };
    if base_sha.starts_with("refs/") {
        gitops::ferry_ref_as(parent_run_repo, &sub_run_repo, &base_sha, &base_sha).await?;
    }

    let branch = format!(
        "wf/{}-{}",
        slugify(&sub_spec.name),
        &sub_run_id[sub_run_id.len() - 8..]
    );
    let now = crate::workflow::now_ms();
    {
        let conn = ctx.db.lock();
        conn.execute(
            "INSERT INTO wf_run (id, definition_id, parent_run_id, name, spec_json, task,
                 project_id, repo_path, run_dir, branch, base_sha, status, budgets_json,
                 spent_json, created_at, updated_at)
             VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'pending', ?11, '{}', ?12, ?12)",
            rusqlite::params![
                sub_run_id,
                parent_run_id,
                sub_spec.name,
                spec_json,
                plan.task,
                parent.project_id,
                parent.repo_path,
                run_dir.to_string_lossy(),
                branch,
                base_sha,
                budgets_json,
                now,
            ],
        )
        .map_err(|e| Error::Other(e.to_string()))?;
    }
    Ok(sub_run_id)
}

/// Spawn (or, on resume, re-spawn) a composed sub-run's own driver task (spec
/// §6.1, §10.3). In production the sub-run is registered in the run registry so
/// the cancel-cascade reaches it and at most one driver runs; under test (`runs`
/// = `None`) it is driven detached and the stage tracks it by polling
/// `wf_run.status`.
fn spawn_subrun(ctx: &RunCtx, sub_run_id: String) {
    let cancel = Arc::new(AtomicBool::new(false));
    let pending_ask = Arc::new(AtomicBool::new(false));
    if let Some(runs) = &ctx.runs {
        let mut m = runs.lock();
        if m.contains_key(&sub_run_id) {
            return;
        }
        // Sub-runs create live drivers in the same registry as top-level runs,
        // so they must register through the same helper — otherwise a parent
        // that is only driving children would report zero active runs and let
        // the sleep assertion release mid-work. Keyed by the sub-run's own id,
        // distinct from the parent, so each counts independently.
        register_driver(
            &mut m,
            &sub_run_id,
            RunHandle {
                cancel: cancel.clone(),
                respawn: Arc::new(AtomicBool::new(false)),
                pending_ask: pending_ask.clone(),
            },
        );
    }
    let child = RunCtx {
        db: ctx.db.clone(),
        driver: ctx.driver.clone(),
        app: ctx.app.clone(),
        cancel,
        pending_ask,
        deadlines: ctx.deadlines.clone(),
        runs: ctx.runs.clone(),
    };
    let runs = ctx.runs.clone();
    let id = sub_run_id.clone();
    tokio::spawn(async move {
        drive_run(&child, &id).await;
        // Drop the registry entry on exit so the cancel-cascade and any list see
        // the sub-run's driver as gone, and clear its activity via the shared
        // helper (a finished sub-run clears only its own id, never the parent's).
        if let Some(runs) = &runs {
            let mut m = runs.lock();
            deregister_driver(&mut m, &id);
        }
    });
}

/// The join status of a sub-run from its `wf_run.status`, or `None` while it is
/// still `pending`/`running`/`paused`.
pub(crate) fn subrun_terminal_status(conn: &Connection, sub_run_id: &str) -> Option<ChildStatus> {
    let status: Option<String> = conn
        .query_row(
            "SELECT status FROM wf_run WHERE id = ?1",
            [sub_run_id],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    match status.as_deref() {
        Some("done") => Some(ChildStatus::Success),
        Some("failed") => Some(ChildStatus::Failure("sub-run failed".into())),
        Some("canceled") => Some(ChildStatus::Skipped),
        _ => None,
    }
}

/// Rebuild an orchestrate stage's sub-run tracking from the journal on resume
/// (spec §10.3): each `subrun_launched` event for this stage carries the sub-run
/// id, integrate mode, and reserved slice. A sub-run already terminal in the DB is
/// marked so (not reaped or reconciled again); a live one is left `None` for the
/// stage to re-drive and await.
pub(crate) fn rebuild_sub_runs(
    conn: &Connection,
    run_id: &str,
    block_index: usize,
) -> HashMap<String, SubRunState> {
    let mut out = HashMap::new();
    let rows: Vec<String> = conn
        .prepare(
            "SELECT payload_json FROM wf_event
             WHERE run_id = ?1 AND type = ?2
               AND json_extract(payload_json, '$.block_index') = ?3
             ORDER BY seq",
        )
        .and_then(|mut s| {
            s.query_map(
                rusqlite::params![run_id, event_type::SUBRUN_LAUNCHED, block_index as i64],
                |r| r.get::<_, String>(0),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default();
    for body in rows {
        let Ok(v) = serde_json::from_str::<Value>(&body) else {
            continue;
        };
        let Some(sub_id) = v.get("sub_run_id").and_then(|x| x.as_str()) else {
            continue;
        };
        let integrate = if v.get("integrate").and_then(|x| x.as_str()) == Some("merge") {
            Integrate::Merge
        } else {
            Integrate::None
        };
        out.insert(
            sub_id.to_string(),
            SubRunState {
                integrate,
                reserved_turns: v
                    .get("reserved_turns")
                    .and_then(|x| x.as_i64())
                    .unwrap_or(0),
                reserved_tokens: v
                    .get("reserved_tokens")
                    .and_then(|x| x.as_i64())
                    .unwrap_or(0),
                terminal: subrun_terminal_status(conn, sub_id),
            },
        );
    }
    out
}

/// The sub-run's persisted ledger (its actual spend), for reconciliation.
fn subrun_ledger(conn: &Connection, sub_run_id: &str) -> Option<Ledger> {
    let spent: String = conn
        .query_row(
            "SELECT spent_json FROM wf_run WHERE id = ?1",
            [sub_run_id],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten()?;
    let val: Value = serde_json::from_str(&spent).unwrap_or_else(|_| json!({}));
    Some(Ledger::from_json(&val))
}

/// Reconcile any sub-runs that reached a terminal state since the last poll (spec
/// §10.3): release each one's reserved slice, fold its actual spend into the
/// parent ledger (so a slice is replaced by real consumption, never both),
/// journal `subrun_finished`, forward the outcome to the orchestrator, and record
/// its join status alongside the children's.
fn reap_sub_runs(
    ctx: &RunCtx,
    run_id: &str,
    orch_exec: &str,
    ledger: &mut Ledger,
    sub_runs: &mut HashMap<String, SubRunState>,
    outcomes: &mut HashMap<String, ChildStatus>,
) {
    let conn = ctx.db.lock();
    for (sub_id, st) in sub_runs.iter_mut() {
        if st.terminal.is_some() {
            continue;
        }
        let Some(status) = subrun_terminal_status(&conn, sub_id) else {
            continue;
        };
        ledger.release_reservation(st.reserved_turns, st.reserved_tokens);
        if let Some(sub_ledger) = subrun_ledger(&conn, sub_id) {
            fold_child_ledger(ledger, &sub_ledger);
        }
        persist_spent(&conn, run_id, ledger);
        let status_str = match &status {
            ChildStatus::Success => "done",
            ChildStatus::Failure(_) => "failed",
            ChildStatus::Skipped => "canceled",
        };
        journal_event(
            &conn,
            ctx.app.as_ref(),
            run_id,
            event_type::SUBRUN_FINISHED,
            Some(orch_exec),
            &json!({ "sub_run_id": sub_id, "status": status_str }),
        );
        crate::workflow::comms::forward_subrun_finished(
            &conn,
            ctx.app.as_ref(),
            run_id,
            orch_exec,
            sub_id,
            status_str,
        );
        outcomes.insert(sub_id.clone(), status.clone());
        st.terminal = Some(status);
    }
}

/// Cancel every live composed sub-run of a stage (spec §10.3): flag a registered
/// driver so it winds itself down, else (under test) stop its agents and mark it
/// canceled directly.
async fn cancel_sub_runs(ctx: &RunCtx, sub_runs: &HashMap<String, SubRunState>) {
    for (sub_id, st) in sub_runs {
        if st.terminal.is_none() {
            cancel_run_by_id(ctx, sub_id).await;
        }
    }
}

/// Cancel one sub-run by id: prefer flagging its live driver (which stops its
/// agents and marks it canceled), else mark it directly.
async fn cancel_run_by_id(ctx: &RunCtx, run_id: &str) {
    if let Some(runs) = &ctx.runs {
        if let Some(flag) = runs.lock().get(run_id).map(|h| h.cancel.clone()) {
            flag.store(true, Ordering::SeqCst);
            return;
        }
    }
    let agents = live_step_agents(&ctx.db.lock(), run_id);
    for a in agents {
        let _ = ctx.driver.stop(&a).await;
    }
    let conn = ctx.db.lock();
    set_status(&conn, ctx.app.as_ref(), run_id, "canceled", None, None);
}

/// The sub-run's final line `(ref, exec_id)` — the commit its integration merges.
/// `None` when the sub-run produced no commit (nothing to integrate).
fn subrun_final_line(ctx: &RunCtx, sub_run_id: &str) -> Option<(String, String)> {
    let conn = ctx.db.lock();
    let (spec_json, base_sha): (String, String) = conn
        .query_row(
            "SELECT spec_json, base_sha FROM wf_run WHERE id = ?1",
            [sub_run_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .ok()
        .flatten()?;
    let spec: Spec = serde_json::from_str(&spec_json).ok()?;
    let (line_ref, exec) = resume_line_state(
        &conn,
        sub_run_id,
        &spec.workflow,
        spec.workflow.len(),
        &base_sha,
    );
    exec.map(|e| (line_ref, e))
}

/// Integrate composed `integrate: merge` sub-runs at the orchestrate join (spec
/// §10.3): ferry each successful sub-run's final ref into the parent run repo (in
/// launch order), then merge them via the shared merge machinery (§12.3) — a clean
/// run advances the stage line onto the integrated result; a conflict pauses
/// `conflict`. No merge sub-runs → the stage line is unchanged (`line: None`).
#[allow(clippy::too_many_arguments)]
async fn begin_subrun_merge(
    ctx: &RunCtx,
    run_id: &str,
    run: &RunEssentials,
    spec: &Spec,
    block_index: usize,
    run_repo: &Path,
    fork_base: &str,
    sub_runs: &HashMap<String, SubRunState>,
    cursor: &mut Cursor,
) -> Result<StageFlow> {
    // Launch order, so merges are deterministic (child order = spec order §12.3).
    let ordered: Vec<String> = {
        let conn = ctx.db.lock();
        conn.prepare("SELECT id FROM wf_run WHERE parent_run_id = ?1 ORDER BY created_at, rowid")
            .and_then(|mut s| {
                s.query_map([run_id], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap_or_default()
    };
    let mut winners: Vec<(String, String)> = Vec::new();
    for sub_id in ordered {
        let Some(st) = sub_runs.get(&sub_id) else {
            continue;
        };
        if !matches!(st.integrate, Integrate::Merge)
            || !matches!(st.terminal, Some(ChildStatus::Success))
        {
            continue;
        }
        let Some((src_ref, _)) = subrun_final_line(ctx, &sub_id) else {
            continue; // produced no commit
        };
        let sub_run_dir: String = {
            let conn = ctx.db.lock();
            conn.query_row("SELECT run_dir FROM wf_run WHERE id = ?1", [&sub_id], |r| {
                r.get(0)
            })
            .map_err(|e| Error::Other(e.to_string()))?
        };
        let sub_run_repo = gitops::run_repo_path(&PathBuf::from(sub_run_dir));
        let dest = gitops::subrun_ref(&sub_id);
        gitops::ferry_ref_as(&sub_run_repo, run_repo, &src_ref, &dest).await?;
        winners.push((sub_id.clone(), dest));
    }
    if winners.is_empty() {
        return Ok(StageFlow::Advance { line: None });
    }
    let run_dir = PathBuf::from(&run.run_dir);
    let int_wt = gitops::integration_worktree_path(&run_dir, block_index);
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
            &json!({ "count": winners.len(), "kind": "subrun" }),
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

/// Resume a sub-run integration paused mid-merge or on a conflict (spec §10.3,
/// §12.3). Mirrors [`resume_merge_stage`] but the refs are already ferried and the
/// mode-(a) resolver is the orchestrate stage's own agent (there is no `parallel`
/// child list to resolve one from).
#[allow(clippy::too_many_arguments)]
async fn resume_subrun_merge(
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
    eff: &EffectiveBudgets,
    ledger: &mut Ledger,
    test_override: &Option<String>,
    setup_override: &Option<String>,
    cursor: &mut Cursor,
) -> Result<StageFlow> {
    let ms = cursor
        .merge
        .clone()
        .ok_or_else(|| Error::Other("resume_subrun_merge without merge cursor".into()))?;
    let run_dir = PathBuf::from(&run.run_dir);
    let int_wt = gitops::integration_worktree_path(&run_dir, block_index);

    let Some(ci) = ms.conflict.clone() else {
        // Interrupted mid-clean-merge — continue with what remains.
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
    // A conflict awaiting the user's choice must not be silently re-driven.
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
        // (c) The human committed a resolution in the integration worktree.
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
        // (a) Spawn a conflict-resolution step, forked from the pinned snapshot,
        // run by the orchestrate stage's own agent.
        "agent" => {
            let agent_spec = spec.agents.get(&orch.agent).ok_or_else(|| {
                Error::Other(format!(
                    "conflict resolver references unknown agent '{}'",
                    orch.agent
                ))
            })?;
            let resolve_step = Step {
                id: format!("__resolve_{block_index}"),
                agent: orch.agent.clone(),
                goal: format!(
                    "A merge of composed sub-run work produced conflicts in: {}. \
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
                base_sha: &run.base_sha,
                // A parallel/orchestrate merge step is never the run entry.
                launch_attachments: &[],
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

    let test_runner = match crate::workflow::tests_gate::SandboxTestRunner::new(
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
        let child_req = {
            let conn = c.db.lock();
            build_spawn_req(
                &conn,
                c.app.as_ref(),
                &c.agent_spec,
                &c.fork_base,
                &c.repo,
                &c.run_repo,
                &c.run_id,
                Some(&exec_id),
            )
        };
        let spawned = match c.driver.spawn(child_req).await {
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
                rusqlite::params![child_agent_id, crate::workflow::now_ms(), exec_id],
            );
        }

        let prompt = {
            let ctx_prompt = StepPromptCtx {
                run_task: &c.run_task,
                // Parallel/orchestrate children are never the run entry step.
                attachments: &[],
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
                crate::workflow::comms::take_pending_deliveries(&conn, &c.run_id, &c.step.id)
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
                let conn = c.db.lock();
                build_spawn_req(
                    &conn,
                    c.app.as_ref(),
                    &c.agent_spec,
                    &c.fork_base,
                    &c.repo,
                    &c.run_repo,
                    &c.run_id,
                    // The agent is pre-spawned above — that call warned already.
                    None,
                )
            },
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

        let started = crate::workflow::now_ms();
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
        if crate::workflow::comms::has_unanswered_ask(&c.db.lock(), &exec_id) {
            {
                let conn = c.db.lock();
                abandon_exec(&conn, c.app.as_ref(), &c.run_id, &exec_id, "question");
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
                    abandon_exec(&conn, c.app.as_ref(), &c.run_id, &exec_id, "canceled");
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
        if !crate::workflow::comms::has_unanswered_ask(&c.db.lock(), exec_id) {
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
pub(crate) fn handle_orch_child(
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
            crate::workflow::comms::forward_lifecycle(
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
            crate::workflow::comms::forward_lifecycle(
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

/// The `(agent, goal)` of each dynamic child the stage spawned in a prior drive,
/// in spawn order (spec §10.2). Read from the persisted `spawn_child` decisions —
/// joined across every orchestrator exec that shares the stage's `step_id` — so a
/// resumed stage can rebuild those children into its registry and still honor
/// `retry_child` for them. Spawn order matches the `dyn-<k>` index order.
pub(crate) fn prior_spawn_decisions(
    conn: &Connection,
    run_id: &str,
    orch_step_id: &str,
) -> Vec<(String, String)> {
    conn.prepare(
        "SELECT m.body_json FROM wf_message m
           JOIN wf_step_exec e ON m.from_step_exec_id = e.id
         WHERE m.run_id = ?1 AND e.step_id = ?2 AND m.kind = 'decision'
           AND json_extract(m.body_json, '$.decision') = 'spawn_child'
         ORDER BY m.created_at, m.rowid",
    )
    .and_then(|mut s| {
        s.query_map(rusqlite::params![run_id, orch_step_id], |r| {
            r.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
    })
    .map(|bodies| {
        bodies
            .into_iter()
            .filter_map(|b| {
                let v: Value = serde_json::from_str(&b).ok()?;
                let agent = v.get("agent")?.as_str()?.to_string();
                let goal = v.get("goal")?.as_str()?.to_string();
                Some((agent, goal))
            })
            .collect()
    })
    .unwrap_or_default()
}

/// The number of dynamic children an orchestrate stage has already created, from
/// the DB — the next dynamic index, seeded so a resumed stage never reuses an id
/// (§10.2). Dynamic child ids are `orchestrate-<n>::dyn-<k>`, `k` contiguous from
/// 0, so the distinct count is the next `k`.
pub(crate) fn existing_dyn_child_count(conn: &Connection, run_id: &str, orch_step_id: &str) -> u32 {
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
    if ledger.exceeded(eff, crate::workflow::now_ms()).is_some() {
        return OrchStepResult::Budget;
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
        ledger.checkpoint_wall(crate::workflow::now_ms());
        persist_spent(&conn, run_id, ledger);
    }
    match turn {
        attempt::OrchTurn::Ended => {
            if ledger.exceeded(eff, crate::workflow::now_ms()).is_some() {
                return OrchStepResult::Budget;
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
                crate::workflow::comms::queue_engine_ask(
                    &conn,
                    run_id,
                    orch_exec,
                    "The orchestrator stalled. Provide guidance to continue, or cancel the run.",
                );
            }
            pause_question(ctx, run_id, orch_exec, Some(orch_agent_id)).await;
            Ok(StageFlow::Stop)
        }
        OrchStepResult::Budget => {
            let _ = ctx.driver.stop(orch_agent_id).await;
            {
                let conn = ctx.db.lock();
                let _ = conn.execute(
                    "UPDATE wf_step_exec SET status = 'abandoned', ended_at = ?1 WHERE id = ?2",
                    rusqlite::params![crate::workflow::now_ms(), orch_exec],
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
            fail_run(
                &conn,
                ctx.app.as_ref(),
                run_id,
                &format!("orchestrator error: {e}"),
            );
            Ok(StageFlow::Stop)
        }
        OrchStepResult::Ok => Ok(StageFlow::Advance { line: None }),
    }
}

// ─────────────────────────── linear steps & loops (§6.6) ────────────────────
