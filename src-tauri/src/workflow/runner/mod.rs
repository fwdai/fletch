//! The kernel runner: the simple sequential success path for a workflow run.
//!
//! One workspace per run (the run repository's own working tree), one agent per
//! step, no ferry and no per-step clone: each step's agent *adopts* the shared
//! checkout ([`SpawnReq::existing_workspace`]), so consecutive steps see each
//! other's work as plain history instead of a re-cloned fork. A step's boundary
//! commit and its `refs/wf/steps/<exec>` pin land directly in that workspace.
//!
//! Scope is deliberate: plain top-level steps, `commit` / `verdict` gates, one
//! per-step wall timeout, and failure-on-anything-unexpected. Retries, loops,
//! parallel stages, approvals, budgets and resume all still belong to the
//! clone-per-step engine (`scheduler/`), which keeps serving every spec this
//! module rejects — see `docs/workflow-kernel-layers.md` for the layer ledger
//! and what each future layer replaces.
//!
//! The journal is the contract with the UI: the kernel emits the same event
//! types and the same `wf_step_exec` status values as the old engine, so the run
//! monitor works unchanged. Every phase is journaled — there is no state a
//! reader of the timeline cannot see.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

use rusqlite::Connection;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::Receiver;

use crate::error::{Error, Result};
use crate::supervisor::StatusEvent;
use crate::workspace::AgentStatus;

use super::blackboard::{self, VerdictError, VerdictResult};
use super::driver::{AgentDriver, SpawnReq};
use super::gitops;
use super::prompts::{self, Position, StepPromptCtx};
use super::scheduler::{
    abandon_exec, build_spawn_req, cancel_run, create_step_exec, fail_run, finalize_run,
    finish_step_exec, gate_mode, journal_event, load_run, resolve_agent, set_status, RunCtx,
};
use super::spec::{Block, Gate, Spec, Step};
use super::types::event_type;

#[cfg(test)]
mod tests;

/// The kernel's only guard: a wall-clock ceiling on one step, covering spawn →
/// ready → prompt → turn end. Generous, because a step doing real work
/// legitimately runs for a long time and the kernel has no notion of progress;
/// the old engine's stall clock and nudge are deliberately not reproduced (a
/// single honest ceiling beats four tunable ones until real hangs prove
/// otherwise).
#[cfg(not(test))]
const STEP_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Under test the timeout *arm* is what's interesting, not the wait. Short
/// enough to assert on, long enough for a stub's real git work.
#[cfg(test)]
const STEP_TIMEOUT: Duration = Duration::from_secs(2);

/// What a non-terminal kernel run is told when the app restarts under it. The
/// kernel keeps no resume state (no cursor reads, no mid-run recovery), so
/// pretending otherwise would silently re-run finished steps in a workspace that
/// already contains their commits.
const NO_RESUME: &str = "kernel runs do not resume yet (see docs/workflow-kernel-layers.md)";

/// Whether a spec is within the kernel's competence: a flat sequence of steps,
/// each gated on something the kernel can decide by itself. Any other block kind
/// (parallel, loop, orchestrate) or gate (artifact, tests, approval) belongs to
/// the clone-per-step engine.
pub(crate) fn kernel_eligible(spec: &Spec) -> bool {
    !spec.workflow.is_empty()
        && spec.workflow.iter().all(|b| match b {
            Block::Step(s) => matches!(s.gate, Gate::Commit | Gate::Verdict),
            _ => false,
        })
}

/// The routing decision for one run row, taken from its launch-frozen
/// `spec_json` so every drive of a given run reaches the same engine. An
/// unreadable or unparsable spec routes to the old engine, which owns the
/// reporting of that failure.
pub(crate) fn routes_to_kernel(db: &super::Db, run_id: &str) -> bool {
    let spec_json: Option<String> = db
        .lock()
        .query_row(
            "SELECT spec_json FROM wf_run WHERE id = ?1",
            [run_id],
            |r| r.get(0),
        )
        .ok();
    spec_json
        .and_then(|j| serde_json::from_str::<Spec>(&j).ok())
        .map(|spec| kernel_eligible(&spec))
        .unwrap_or(false)
}

/// Drive one kernel run to `done`, `failed` or `canceled`. Any error bubbling
/// out fails the run with its cause — the kernel never leaves a run `running`
/// with no live driver.
pub(crate) async fn run_kernel(ctx: &RunCtx, run_id: &str) {
    if let Err(e) = run_kernel_inner(ctx, run_id).await {
        let conn = ctx.db.lock();
        fail_run(&conn, ctx.app.as_ref(), run_id, &e.to_string());
    }
}

async fn run_kernel_inner(ctx: &RunCtx, run_id: &str) -> Result<()> {
    let run = load_run(&ctx.db.lock(), run_id)?;
    // A stale respawn or a command racing a terminal write must not restart a
    // finished run.
    if matches!(run.status.as_str(), "done" | "failed" | "canceled") {
        return Ok(());
    }
    if run.status != "pending" {
        refuse_resume(ctx, run_id).await;
        return Ok(());
    }

    let spec: Spec =
        serde_json::from_str(&run.spec_json).map_err(|e| Error::Other(e.to_string()))?;
    // The routing site checked this; re-check so a hand-edited row can't smuggle
    // an unsupported block past the one gate that understands it.
    if !kernel_eligible(&spec) {
        return Err(Error::Other(
            "spec is not kernel-eligible (only plain steps with commit/verdict gates)".into(),
        ));
    }

    let repo = PathBuf::from(&run.repo_path);
    let run_dir = PathBuf::from(&run.run_dir);
    let blackboard = blackboard::blackboard_dir(&run_dir);
    // The run repo is a non-bare `--shared` clone with `origin` rewritten, so
    // its working tree doubles as the run's single workspace and finalize
    // already pushes from here. Detach at the run base: the clone opens on the
    // source's default branch, which is not necessarily the fork point.
    let workspace = gitops::provision_run_repo(&repo, &run_dir).await?;
    crate::git::run_git(
        &workspace,
        &["checkout", "--detach", &run.base_sha],
        "checkout run base",
    )
    .await?;

    {
        let conn = ctx.db.lock();
        journal_event(
            &conn,
            ctx.app.as_ref(),
            run_id,
            event_type::RUN_LAUNCHED,
            None,
            &json!({
                "base_sha": run.base_sha,
                "runner": "kernel",
                "workspace": workspace.to_string_lossy(),
            }),
        );
        set_status(&conn, ctx.app.as_ref(), run_id, "running", None, None);
    }

    let steps: Vec<&Step> = spec
        .workflow
        .iter()
        .filter_map(|b| match b {
            Block::Step(s) => Some(s),
            _ => None,
        })
        .collect();
    // Launch attachments belong to the run's initial task, so only the entry
    // step renders them.
    let attachments = blackboard::read_attachments(&run_dir);
    // The fork base each step is journaled against, and the ref finalize pushes:
    // the run base until a step pins one.
    let mut last_ref = run.base_sha.clone();
    let mut last_exec_id: Option<String> = None;

    for (index, step) in steps.iter().enumerate() {
        if ctx.cancel.load(Ordering::SeqCst) {
            cancel_run(ctx, run_id).await;
            return Ok(());
        }

        let agent_spec = resolve_agent(&spec, step)?;
        let exec_id = format!("exec-{}", uuid::Uuid::new_v4());
        {
            let conn = ctx.db.lock();
            create_step_exec(
                &conn,
                &exec_id,
                run_id,
                &step.id,
                1,
                0,
                gate_mode(&step.gate),
            );
        }

        let prompt = prompts::step_prompt(&StepPromptCtx {
            run_task: &run.task,
            attachments: if index == 0 { &attachments } else { &[] },
            step_id: &step.id,
            step_goal: &step.goal,
            position: Position {
                step_index: index,
                step_count: steps.len(),
                iteration: None,
            },
            gate: &step.gate,
            turns_per_attempt: step.budgets.as_ref().and_then(|b| b.turns_per_attempt),
            comms: &step.comms,
        });

        let spawn_req = {
            let conn = ctx.db.lock();
            let mut req = build_spawn_req(
                &conn,
                ctx.app.as_ref(),
                agent_spec,
                &last_ref,
                &repo,
                &workspace,
                run_id,
                Some(&exec_id),
            );
            // Adoption is what makes this a kernel run: the step agent works in
            // the run repo's tree rather than cloning its own, so the whole run
            // shares one checkout and one line of commits. `fork_base` stays as
            // the journal's record of where this step started.
            req.existing_workspace = Some(workspace.clone());
            req
        };

        // One deadline and one cancel race around the whole step, rather than a
        // deadline per wait: the kernel cannot tell a slow step from a stuck one,
        // so it only promises not to hang forever.
        let end = tokio::select! {
            biased;
            r = tokio::time::timeout(
                STEP_TIMEOUT,
                drive_step(ctx, run_id, &exec_id, spawn_req, prompt),
            ) => r.unwrap_or_else(|_| StepEnd::Failed {
                agent_id: agent_id_of(&ctx.db.lock(), &exec_id),
                error: format!("step timed out after {}s", STEP_TIMEOUT.as_secs()),
            }),
            _ = super::attempt::wait_cancelled(&ctx.cancel) => StepEnd::Canceled,
        };

        let agent_id = match end {
            StepEnd::TurnEnded { agent_id } => agent_id,
            StepEnd::Failed { agent_id, error } => {
                fail_step(ctx, run_id, &exec_id, agent_id.as_deref(), "error", &error).await;
                return Ok(());
            }
            StepEnd::Canceled => {
                cancel_step(ctx, run_id, &exec_id).await;
                return Ok(());
            }
        };

        // Gate, v0: a `commit` gate is satisfied by the turn ending (the boundary
        // commit below captures whatever the agent did); a `verdict` gate needs
        // `result: "done"`. Anything else fails the run — re-prompting and
        // retrying are later layers, and a silent advance past an unmet gate
        // would be worse than a loud stop.
        let gate = evaluate_gate(&step.gate, &blackboard, &step.id);
        journal(
            ctx,
            run_id,
            &exec_id,
            event_type::GATE_EVALUATED,
            json!({
                "mode": gate_mode(&step.gate),
                "outcome": if gate.is_ok() { "done" } else { "blocked" },
                "reason": gate.clone().err().unwrap_or_default(),
            }),
        );
        if let Err(reason) = gate {
            fail_step(ctx, run_id, &exec_id, Some(&agent_id), "blocked", &reason).await;
            return Ok(());
        }

        // Durability: commit the step's work in the shared workspace and pin it.
        // No ferry — the workspace *is* the run repo, so the pin is already
        // where finalize and the diff surface read from.
        let message = format!("wf({}): {}", spec.name, step.id);
        let head = match commit_step(&workspace, &exec_id, &message).await {
            Ok(head) => head,
            Err(e) => {
                fail_step(
                    ctx,
                    run_id,
                    &exec_id,
                    Some(&agent_id),
                    "error",
                    &format!("boundary commit failed: {e}"),
                )
                .await;
                return Ok(());
            }
        };
        {
            let conn = ctx.db.lock();
            journal_event(
                &conn,
                ctx.app.as_ref(),
                run_id,
                event_type::BOUNDARY_COMMIT,
                Some(&exec_id),
                &json!({ "sha": head, "message": message }),
            );
            finish_step_exec(&conn, &exec_id, "done", Some(&head));
        }
        // Archive, never delete: the step's chat stays replayable from the
        // timeline after the agent is gone.
        let _ = ctx.driver.archive(&agent_id).await;

        last_ref = gitops::step_ref(&exec_id);
        last_exec_id = Some(exec_id);
    }

    finalize_run(
        ctx,
        run_id,
        &run,
        &spec,
        &workspace,
        last_exec_id.as_deref(),
    )
    .await?;
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

/// How one step's agent-facing half ended. The gate, the commit and the run's
/// terminal status are the caller's business.
enum StepEnd {
    /// The agent's turn ended cleanly; the gate has not been consulted yet.
    TurnEnded {
        agent_id: String,
    },
    /// The step never reached a turn end. `agent_id` is `None` only when the
    /// spawn itself failed — otherwise the agent exists and must be stopped.
    Failed {
        agent_id: Option<String>,
        error: String,
    },
    Canceled,
}

/// Spawn the step's agent into the shared workspace, wait for it to be ready,
/// send the step prompt, and wait for the turn to end. Every transition is
/// journaled as it happens (not batched at the end) so the monitor can mount the
/// step's chat and show progress while the turn is still in flight.
async fn drive_step(
    ctx: &RunCtx,
    run_id: &str,
    exec_id: &str,
    spawn_req: SpawnReq,
    prompt: String,
) -> StepEnd {
    let fork_base = spawn_req.fork_base.clone();
    let spawned = match ctx.driver.spawn(spawn_req).await {
        Ok(s) => s,
        Err(e) => {
            return StepEnd::Failed {
                agent_id: None,
                error: format!("spawn failed: {e}"),
            }
        }
    };
    // The adopted workspace is the run repo, so `SpawnedAgent::worktree` is not
    // consulted: the kernel commits where it provisioned, not where the driver
    // reports.
    let agent_id = spawned.agent_id;
    // Link the agent before the first event, so a listener that refetches on
    // that event already finds the step's chat mounted.
    stamp_spawned(&ctx.db.lock(), exec_id, &agent_id);
    journal(
        ctx,
        run_id,
        exec_id,
        event_type::ATTEMPT_SPAWNED,
        json!({ "agent_id": agent_id, "fork_base": fork_base }),
    );

    // The per-step ceiling is the real deadline; passing it here too keeps this
    // wait from outliving the step under a driver that never reports readiness.
    if let Err(e) =
        super::attempt::await_agent_ready(ctx.driver.as_ref(), &agent_id, STEP_TIMEOUT).await
    {
        return StepEnd::Failed {
            agent_id: Some(agent_id),
            error: e,
        };
    }
    journal(ctx, run_id, exec_id, event_type::ATTEMPT_READY, json!({}));

    // Subscribe BEFORE sending (the [`AgentDriver`] contract): a Running→Idle
    // flap can be faster than any status read, and this buffers it.
    let mut rx = ctx.driver.subscribe();
    if let Err(e) = ctx.driver.send_message(&agent_id, prompt).await {
        return StepEnd::Failed {
            agent_id: Some(agent_id),
            error: format!("send failed: {e}"),
        };
    }
    journal(
        ctx,
        run_id,
        exec_id,
        event_type::PROMPT_SENT,
        json!({ "kind": "step" }),
    );

    match await_turn_end(ctx.driver.as_ref(), &agent_id, &mut rx).await {
        Ok(()) => {
            journal(
                ctx,
                run_id,
                exec_id,
                event_type::TURN_ENDED,
                json!({ "status": "idle" }),
            );
            StepEnd::TurnEnded { agent_id }
        }
        Err(error) => {
            journal(
                ctx,
                run_id,
                exec_id,
                event_type::TURN_ENDED,
                json!({ "status": "error" }),
            );
            StepEnd::Failed {
                agent_id: Some(agent_id),
                error,
            }
        }
    }
}

/// Wait for the turn to end, i.e. for the agent to go `Idle`. `rx` must have been
/// subscribed before the prompt was sent. The step's wall timeout is the only
/// deadline: there is no turn-start clock and no stall watchdog here.
async fn await_turn_end(
    driver: &dyn AgentDriver,
    agent_id: &str,
    rx: &mut Receiver<StatusEvent>,
) -> std::result::Result<(), String> {
    loop {
        match rx.recv().await {
            Ok(e) if e.agent_id == agent_id => match e.status {
                AgentStatus::Idle => return Ok(()),
                AgentStatus::Error | AgentStatus::Stopped => {
                    return Err("agent errored mid-turn".into())
                }
                AgentStatus::Running | AgentStatus::Spawning => {}
            },
            Ok(_) => {}
            // A lagged receiver dropped transitions; the authoritative status
            // still tells us whether the turn is over.
            Err(RecvError::Lagged(_)) => match driver.status(agent_id) {
                Some(AgentStatus::Idle) => return Ok(()),
                Some(AgentStatus::Error | AgentStatus::Stopped) => {
                    return Err("agent errored mid-turn".into())
                }
                _ => {}
            },
            Err(RecvError::Closed) => return Err("supervisor stopped".into()),
        }
    }
}

/// The gate verdict as `Ok(())` (advance) or `Err(reason)` (fail the run). Pure:
/// a `commit` gate is decided by the turn having ended, a `verdict` gate by the
/// file the step wrote.
fn evaluate_gate(
    gate: &Gate,
    blackboard: &std::path::Path,
    step_id: &str,
) -> std::result::Result<(), String> {
    match gate {
        // The boundary commit records whatever the turn produced, including
        // nothing; the kernel does not second-guess an agent that says it's done.
        Gate::Commit => Ok(()),
        Gate::Verdict => {
            let dir = blackboard::step_dir(blackboard, step_id)
                .map_err(|e| format!("blackboard error: {e}"))?;
            match blackboard::read_verdict(&dir) {
                Ok(v) if v.result == VerdictResult::Done => Ok(()),
                Ok(v) => Err(format!(
                    "verdict.json result is \"{}\"{}",
                    match v.result {
                        VerdictResult::Done => "done",
                        VerdictResult::Revise => "revise",
                        VerdictResult::Blocked => "blocked",
                    },
                    if v.summary.trim().is_empty() {
                        String::new()
                    } else {
                        format!(": {}", v.summary.trim())
                    }
                )),
                Err(VerdictError::Missing) => Err("no verdict.json was written".into()),
                Err(VerdictError::Malformed(e)) => Err(e),
            }
        }
        // `kernel_eligible` rejects every other gate before a run starts.
        _ => Err(format!(
            "gate '{}' is not supported by the kernel",
            gate_mode(gate)
        )),
    }
}

/// Boundary-commit the shared workspace and pin the result as the step's ref.
/// Returns the resulting HEAD.
async fn commit_step(workspace: &std::path::Path, exec_id: &str, message: &str) -> Result<String> {
    let bc = gitops::boundary_commit(workspace, message).await?;
    gitops::pin_step_ref(workspace, exec_id).await?;
    Ok(bc.head)
}

/// End the run on a step that could not finish: stop its agent (an idle-but-alive
/// CLI must not outlive the run), close the exec row with `status`, and fail the
/// run with the cause on both the timeline and the run row.
async fn fail_step(
    ctx: &RunCtx,
    run_id: &str,
    exec_id: &str,
    agent_id: Option<&str>,
    status: &str,
    error: &str,
) {
    if let Some(a) = agent_id {
        let _ = ctx.driver.stop(a).await;
    }
    let conn = ctx.db.lock();
    journal_event(
        &conn,
        ctx.app.as_ref(),
        run_id,
        event_type::ATTEMPT_ERROR,
        Some(exec_id),
        &json!({ "error": error }),
    );
    finish_step_exec(&conn, exec_id, status, None);
    fail_run(&conn, ctx.app.as_ref(), run_id, error);
}

/// Complete a cancel observed while a step was live: stop the agent, abandon its
/// exec, archive the chat, and write the run's terminal status. The step loop
/// never runs again after this, so the status must be written here.
async fn cancel_step(ctx: &RunCtx, run_id: &str, exec_id: &str) {
    let agent_id = agent_id_of(&ctx.db.lock(), exec_id);
    if let Some(a) = &agent_id {
        let _ = ctx.driver.stop(a).await;
    }
    {
        let conn = ctx.db.lock();
        abandon_exec(&conn, ctx.app.as_ref(), run_id, exec_id, "canceled");
    }
    if let Some(a) = &agent_id {
        let _ = ctx.driver.archive(a).await;
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

/// A kernel run found non-terminal at startup: there is no mid-run recovery yet,
/// so say so on the timeline and fail rather than re-running steps whose commits
/// are already in the workspace. Any exec the previous driver left open is
/// abandoned (and its agent stopped) so nothing claims to still be live.
async fn refuse_resume(ctx: &RunCtx, run_id: &str) {
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
        abandon_exec(&conn, ctx.app.as_ref(), run_id, &exec_id, NO_RESUME);
    }
    let conn = ctx.db.lock();
    fail_run(&conn, ctx.app.as_ref(), run_id, NO_RESUME);
}

/// Link the live agent to its exec row and mark the row `running` — what lets the
/// monitor resolve and mount the step's chat mid-turn.
fn stamp_spawned(conn: &Connection, exec_id: &str, agent_id: &str) {
    let _ = conn.execute(
        "UPDATE wf_step_exec SET agent_id = ?1, status = 'running', started_at = ?2
         WHERE id = ?3",
        rusqlite::params![agent_id, super::now_ms(), exec_id],
    );
}

/// The agent stamped on an exec row, for the paths that lost the handle to the
/// step future (timeout, cancel).
fn agent_id_of(conn: &Connection, exec_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT agent_id FROM wf_step_exec WHERE id = ?1",
        [exec_id],
        |r| r.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
}

fn journal(
    ctx: &RunCtx,
    run_id: &str,
    exec_id: &str,
    event_type: &str,
    payload: serde_json::Value,
) {
    let conn = ctx.db.lock();
    journal_event(
        &conn,
        ctx.app.as_ref(),
        run_id,
        event_type,
        Some(exec_id),
        &payload,
    );
}
