use super::*;

pub(crate) fn run_status(conn: &Connection, run_id: &str) -> Result<(String, Option<String>)> {
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
pub(crate) fn project_setting(conn: &Connection, project_id: &str, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM project_settings WHERE project_id = ?1 AND key = ?2",
        rusqlite::params![project_id, key],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
}

pub(crate) fn load_run(conn: &Connection, run_id: &str) -> Result<RunEssentials> {
    conn.query_row(
        "SELECT spec_json, task, project_id, repo_path, run_dir, branch, base_sha, base_branch,
                status, budgets_json, spent_json, issue_ref
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
                base_branch: r.get(7)?,
                status: r.get(8)?,
                budgets_json: r.get(9)?,
                spent_json: r.get(10)?,
                issue_ref: r.get(11)?,
            })
        },
    )
    .map_err(|e| Error::Other(format!("run {run_id} not found: {e}")))
}

/// Update the run row's status and emit `wf:run` (when an app handle is present).
pub(crate) fn set_status(
    conn: &Connection,
    app: Option<&AppHandle>,
    run_id: &str,
    status: &str,
    paused_reason: Option<&str>,
    error: Option<&str>,
) {
    let _ = conn.execute(
        "UPDATE wf_run SET status = ?1, paused_reason = ?2, error = ?3, updated_at = ?4 WHERE id = ?5",
        rusqlite::params![status, paused_reason, error, crate::workflow::now_ms(), run_id],
    );
    if let Some(app) = app {
        if let Ok(run) = conn.query_row(
            "SELECT * FROM wf_run WHERE id = ?1",
            [run_id],
            crate::workflow::types::Run::from_row,
        ) {
            journal::emit_run(app, &run);
        }
    }
}

/// Mark a run `failed`: journal `run_failed {error}` first, then update the row.
/// A failure is an append-only timeline event (§6.1, §7.1) — the observability
/// goal (§1.2) requires every terminal outcome to be a journal event, not only a
/// materialized-row change. Mirrors the `run_done` journal+status pair, so a
/// panic, ferry failure, or stage failure all leave a `run_failed` in the
/// timeline with the same human-readable cause stored on `wf_run.error`.
pub(crate) fn fail_run(conn: &Connection, app: Option<&AppHandle>, run_id: &str, error: &str) {
    journal_event(
        conn,
        app,
        run_id,
        event_type::RUN_FAILED,
        None,
        &json!({ "error": error }),
    );
    set_status(conn, app, run_id, "failed", None, Some(error));
}

/// Append a journal event and emit `wf:event` (when an app handle is present).
pub(crate) fn journal_event(
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

pub(crate) fn create_step_exec(
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

pub(crate) fn finish_step_exec(conn: &Connection, id: &str, status: &str, head_end: Option<&str>) {
    let _ = conn.execute(
        "UPDATE wf_step_exec SET status = ?1, head_end = ?2, ended_at = ?3 WHERE id = ?4",
        rusqlite::params![status, head_end, crate::workflow::now_ms(), id],
    );
}

/// Mark a step exec `abandoned` (ended now) and journal the reason as an
/// `ATTEMPT_ABANDONED` event (§8.3). The caller holds `conn`.
pub(crate) fn abandon_exec(
    conn: &Connection,
    app: Option<&AppHandle>,
    run_id: &str,
    exec_id: &str,
    cause: &str,
) {
    let _ = conn.execute(
        "UPDATE wf_step_exec SET status = 'abandoned', ended_at = ?1 WHERE id = ?2",
        rusqlite::params![crate::workflow::now_ms(), exec_id],
    );
    journal_event(
        conn,
        app,
        run_id,
        event_type::ATTEMPT_ABANDONED,
        Some(exec_id),
        &json!({ "cause": cause }),
    );
}

/// The next attempt number for a step *within one iteration* — retries increment
/// `attempt`, while each loop iteration is a fresh execution counted separately by
/// the `iteration` column (spec §4). Scoping by iteration keeps `attempt` a true
/// retry count and the §8.3 `attempt-<n>.iter-<i>` archive labels meaningful.
pub(crate) fn next_attempt_no(
    conn: &Connection,
    run_id: &str,
    step_id: &str,
    iteration: i64,
) -> i64 {
    conn.query_row(
        "SELECT COALESCE(MAX(attempt), 0) + 1 FROM wf_step_exec
         WHERE run_id = ?1 AND step_id = ?2 AND iteration = ?3",
        rusqlite::params![run_id, step_id, iteration],
        |r| r.get(0),
    )
    .unwrap_or(1)
}

/// The synthetic `wf_step_exec.step_id` for a merge stage's integrated result —
/// the fork source the next block (and finalize) reads via [`resume_line_state`].
pub(crate) fn merge_step_id(block_index: usize) -> String {
    format!("__merge_{block_index}")
}

pub(crate) fn get_cursor(conn: &Connection, run_id: &str) -> Cursor {
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

pub(crate) fn set_cursor(conn: &Connection, run_id: &str, cursor: &Cursor) {
    let json = serde_json::to_string(cursor).unwrap_or_else(|_| "{}".to_string());
    let _ = conn.execute(
        "UPDATE wf_run SET cursor_json = ?1, updated_at = ?2 WHERE id = ?3",
        rusqlite::params![json, crate::workflow::now_ms(), run_id],
    );
}

/// Whether the top-level block at `index` is a plain `step` (vs a loop/parallel/
/// orchestrate container) — governs whether `wf_approve` advances the cursor
/// (§6.6): a top-level step's approval advances; a loop-body approval is advanced
/// by the loop's resume-skip on re-drive.
pub(crate) fn top_level_block_is_step(conn: &Connection, run_id: &str, index: i64) -> bool {
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
pub(crate) fn done_body_exec(
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
pub(crate) fn persist_spent(conn: &Connection, run_id: &str, ledger: &Ledger) {
    let _ = conn.execute(
        "UPDATE wf_run SET spent_json = ?1, updated_at = ?2 WHERE id = ?3",
        rusqlite::params![
            ledger.to_json().to_string(),
            crate::workflow::now_ms(),
            run_id
        ],
    );
}

/// Pause a run `budget_exceeded` (§11.2): fold in the drive's active wall-clock,
/// persist the ledger, journal `run_paused`, and set the row. The caller has
/// already journaled the `budget_exceeded` event (from the attempt's events or
/// the pre-spawn check) and settled any live agent.
pub(crate) fn finish_budget_pause(
    ctx: &RunCtx,
    run_id: &str,
    exec_id: Option<&str>,
    ledger: &mut Ledger,
) {
    ledger.checkpoint_wall(crate::workflow::now_ms());
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
pub(crate) fn deadlines_from(base: &Deadlines, eff: &EffectiveBudgets) -> Deadlines {
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
pub(crate) fn latest_done_exec_for_step(
    conn: &Connection,
    run_id: &str,
    step_id: &str,
) -> Option<String> {
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
pub(crate) fn done_exec_with_ended_at(
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
pub(crate) fn child_already_done(conn: &Connection, run_id: &str, step_id: &str) -> bool {
    latest_done_exec_for_step(conn, run_id, step_id).is_some()
}

/// The status of a step's most recent attempt, if any — the resume signal for an
/// orchestrate child. Distinguishes a terminally-finished child (whose join
/// outcome must be restored, not recomputed) from an in-flight one.
pub(crate) fn latest_exec_status(conn: &Connection, run_id: &str, step_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT status FROM wf_step_exec
         WHERE run_id = ?1 AND step_id = ?2
         ORDER BY rowid DESC LIMIT 1",
        rusqlite::params![run_id, step_id],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

/// Map a child's most-recent exec status to the join outcome to restore on resume
/// (spec §6.6): a terminally-finished child keeps its result; an in-flight one
/// (`abandoned` by the resume, or none) returns `None` so the caller re-runs or
/// leaves it to the orchestrator. Prevents a resumed stage from deciding the join
/// on incomplete child outcomes.
pub(crate) fn restored_child_status(exec_status: &str) -> Option<ChildStatus> {
    match exec_status {
        "done" => Some(ChildStatus::Success),
        "error" | "blocked" => Some(ChildStatus::Failure("failed in a previous drive".into())),
        _ => None,
    }
}

/// Recompute the line's fork source at resume: the last **top-level `step`**
/// before the cursor that reached `done` (its ferried ref + exec id), else the
/// run base. Parallel `integrate: none` children are deliberately ignored — they
/// never advance the line — which is why this walks the block tree rather than
/// querying "the most recent done exec".
pub(crate) fn resume_line_state(
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
            // A completed loop advances the line to its most recent `done` body
            // step across iterations and retries (§6.6). Without this arm a
            // resume past the loop refetches an earlier block's ref (or the run
            // base) and every commit the loop ferried silently drops off the
            // branch — and a finalize-on-resume skips the push entirely.
            Block::Loop(l) => {
                let ids: Vec<String> = l
                    .body
                    .iter()
                    .filter_map(|b| match b {
                        Block::Step(s) => Some(s.id.clone()),
                        _ => None,
                    })
                    .collect();
                if let Some(exec_id) = latest_done_exec_for_steps(conn, run_id, &ids) {
                    return (gitops::step_ref(&exec_id), Some(exec_id));
                }
            }
            // An orchestrate stage advances the line only when a composed
            // sub-run merged (§10.3) — recorded via the same synthetic
            // `__merge_<i>` exec as a parallel merge stage. `integrate: none`
            // children never move it.
            Block::Orchestrate(_) => {
                if let Some(exec_id) = latest_done_exec_for_step(conn, run_id, &merge_step_id(i)) {
                    return (gitops::step_ref(&exec_id), Some(exec_id));
                }
            }
            _ => {}
        }
    }
    (base_sha.to_string(), None)
}

/// The most recent `done` exec among a set of step ids (a loop's body): the
/// line's tip after a completed loop. `rowid DESC` picks the latest completion
/// across iterations and retries, exactly like [`latest_done_exec_for_step`].
fn latest_done_exec_for_steps(
    conn: &Connection,
    run_id: &str,
    step_ids: &[String],
) -> Option<String> {
    if step_ids.is_empty() {
        return None;
    }
    let placeholders = vec!["?"; step_ids.len()].join(",");
    let sql = format!(
        "SELECT id FROM wf_step_exec
         WHERE run_id = ? AND status = 'done' AND step_id IN ({placeholders})
         ORDER BY rowid DESC LIMIT 1"
    );
    let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(step_ids.len() + 1);
    params.push(&run_id);
    for s in step_ids {
        params.push(s);
    }
    conn.query_row(&sql, params.as_slice(), |r| r.get(0))
        .optional()
        .ok()
        .flatten()
}

/// The ids of a run's composed sub-runs (spec §10.3), for the cancel-cascade.
pub(crate) fn child_run_ids(conn: &Connection, parent_run_id: &str) -> Vec<String> {
    conn.prepare("SELECT id FROM wf_run WHERE parent_run_id = ?1")
        .and_then(|mut s| {
            s.query_map([parent_run_id], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default()
}

/// Live (spawned, non-terminal) step agents for a run — stopped on cancel/pause.
pub(crate) fn live_step_agents(conn: &Connection, run_id: &str) -> Vec<String> {
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
