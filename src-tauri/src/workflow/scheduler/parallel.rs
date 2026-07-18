use super::*;

/// Fold a finished child's budget ledger into the run ledger (§11.2). Each child
/// runs against its own fresh ledger (concurrent children can't share the run's
/// `&mut Ledger`); their spend is summed back here so the next block — and the
/// persisted `spent_json` — reflect the whole stage.
pub(crate) fn fold_child_ledger(run: &mut Ledger, child: &Ledger) {
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
pub(crate) async fn run_parallel_stage(
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

    let run_cancel = ctx.cancel.clone();
    loop {
        // Race the join against a run-level cancel (§6.5): a user cancel must
        // wind the whole stage down promptly, not after every child ran to its
        // natural terminal. Setting `stage_cancel` stops new launches and trips
        // the in-flight children's own cancel races; the loop keeps draining so
        // their teardown (abandon + archive) completes.
        let joined = tokio::select! {
            biased;
            _ = attempt::wait_cancelled(&run_cancel), if !stage_cancel.load(Ordering::SeqCst) => {
                stage_cancel.store(true, Ordering::SeqCst);
                continue;
            }
            j = set.join_next() => match j {
                Some(j) => j,
                None => break,
            },
        };
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
        ledger.checkpoint_wall(crate::workflow::now_ms());
        persist_spent(&conn, run_id, ledger);
    }

    // A run-level cancel supersedes join evaluation (§6.5): the children were
    // wound down above; write the terminal status now — like the drive loop's
    // between-blocks check, nothing after a Stop will.
    if run_cancel.load(Ordering::SeqCst) {
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
        return Ok(StageFlow::Stop);
    }

    match par.join {
        Join::All => {
            if let Some(reason) = stage_failed {
                let conn = ctx.db.lock();
                fail_run(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    &format!("parallel stage failed: {reason}"),
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
                fail_run(
                    &conn,
                    ctx.app.as_ref(),
                    run_id,
                    &format!("all parallel children failed: {}", failures.join("; ")),
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
pub(crate) fn pick_winners(
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
pub(crate) async fn drive_merges(
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
            gitops::MergeResult::Conflict { files } => {
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
        let now = crate::workflow::now_ms();
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
    let test_runner = match crate::workflow::tests_gate::SandboxTestRunner::new(
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
            match &last_failure {
                Some(f) => prompts::retry_prompt(f, &ctx_prompt),
                None => prompts::step_prompt(&ctx_prompt),
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
                    Some(&exec_id),
                )
            },
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

        let started = crate::workflow::now_ms();
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
                    abandon_exec(&conn, c.app.as_ref(), &c.run_id, &exec_id, "canceled");
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

/// Assemble a [`SpawnReq`] for a step / parallel-child agent. The step's
/// skill/MCP deliverables are resolved to by-value snapshots here — at spawn
/// (§3.2), the same semantics as the draft spawn path — so a custom-agent step
/// carries its skills and deliverable MCP servers, and later library edits
/// never touch a spawned step. Anything the definition requested that no
/// longer resolves — skills, or the custom agent itself — is journaled as a
/// `skills_missing` / `mcp_servers_missing` / `custom_agent_missing` warning
/// against `warn_exec_id`;
/// the step still spawns. Pass `warn_exec_id: None` when the req describes an
/// already-spawned agent (`pre_spawned`) whose spawn call warned already. The
/// blackboard write-grant is derived from `owner_run_id` at spawn.
/// An explicit `AgentSpec` override, treating a blank string as unset so a
/// blank `model`/`effort`/`instructions` in a hand-authored or imported spec
/// falls back to the linked custom agent's value instead of spawning with an
/// empty argument. The check is trim-based, so a whitespace-only value (`"   "`,
/// reachable from hand-authored YAML) is also treated as unset; the original
/// (untrimmed) value is returned when it has content. Matches the custom-agent
/// side's normalization in `resolve_step_deliverables`.
fn nonblank(value: &Option<String>) -> Option<String> {
    value.clone().filter(|s| !s.trim().is_empty())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_spawn_req(
    conn: &Connection,
    app: Option<&AppHandle>,
    agent_spec: &AgentSpec,
    fork_base: &str,
    repo: &Path,
    run_repo: &Path,
    run_id: &str,
    warn_exec_id: Option<&str>,
) -> SpawnReq {
    let mcp_server_names: Vec<String> = agent_spec
        .mcp_servers
        .iter()
        .map(|d| d.name.clone())
        .collect();
    let deliverables = crate::workflow::definition::resolve_step_deliverables(
        conn,
        agent_spec.custom_agent.as_deref(),
        &agent_spec.skills,
        &mcp_server_names,
        &agent_spec.base,
    );
    if let Some(exec_id) = warn_exec_id {
        if let Some(ca_id) = &deliverables.missing_custom_agent {
            journal_event(
                conn,
                app,
                run_id,
                event_type::CUSTOM_AGENT_MISSING,
                Some(exec_id),
                &json!({ "custom_agent": ca_id }),
            );
        }
        if !deliverables.missing_skills.is_empty() {
            journal_event(
                conn,
                app,
                run_id,
                event_type::SKILLS_MISSING,
                Some(exec_id),
                &json!({ "skills": deliverables.missing_skills }),
            );
        }
        if !deliverables.missing_mcp_servers.is_empty() {
            journal_event(
                conn,
                app,
                run_id,
                event_type::MCP_SERVERS_MISSING,
                Some(exec_id),
                &json!({ "mcp_servers": deliverables.missing_mcp_servers }),
            );
        }
        // A by-name match that hit multiple same-named rows: the step ran
        // against a deterministic pick (lowest id), but say so — the user may
        // have meant a different one (warn-don't-fail).
        if !deliverables.ambiguous_skills.is_empty() {
            journal_event(
                conn,
                app,
                run_id,
                event_type::SKILLS_AMBIGUOUS,
                Some(exec_id),
                &json!({ "skills": deliverables.ambiguous_skills }),
            );
        }
        if !deliverables.ambiguous_mcp_servers.is_empty() {
            journal_event(
                conn,
                app,
                run_id,
                event_type::MCP_SERVERS_AMBIGUOUS,
                Some(exec_id),
                &json!({ "mcp_servers": deliverables.ambiguous_mcp_servers }),
            );
        }
    }
    SpawnReq {
        repo_path: repo.to_path_buf(),
        provider: agent_spec.base.clone(),
        // The alias's explicit model/effort/instructions win; otherwise inherit
        // the linked custom agent's values (§3.2). Resolved here rather than in
        // the driver so the one place that reads the custom_agents row owns the
        // fallback — and so a live custom-agent step spawns identically to the
        // same alias after YAML export+import (which inlines these onto the
        // AgentSpec via `embed_custom_agents`).
        //
        // A *blank* explicit value (`""`, e.g. from a hand-authored/imported
        // YAML) is treated as unset, so it falls through to the custom agent's
        // value rather than spawning with an empty argument. This matches the
        // empty→None normalization the custom-agent side already applies (see
        // `resolve_step_deliverables` / `embed_custom_agents`).
        model: nonblank(&agent_spec.model).or(deliverables.model.clone()),
        effort: nonblank(&agent_spec.effort).or(deliverables.effort.clone()),
        instructions: nonblank(&agent_spec.instructions).or(deliverables.instructions.clone()),
        custom_agent_id: agent_spec.custom_agent.clone(),
        skills: deliverables.skills,
        mcp_servers: deliverables.mcp_servers,
        fork_base: Some(fork_base.to_string()),
        run_repo: Some(run_repo.to_path_buf()),
        owner_run_id: run_id.to_string(),
    }
}

// ───────────────────────── orchestrate stages (§6.6, §10.2) ─────────────────
