//! Step-attempt lifecycle (spec §6.3). One `run_attempt` call drives a single
//! step agent through: spawn → ready → prompt → turn → gate, with a deadline on
//! every wait (spec principle "no wait without a deadline"). It is written
//! entirely against the [`AgentDriver`] trait so the whole state machine is
//! unit-testable with a `MockDriver`.
//!
//! Scope note (S4a): this module owns steps 1–6 of §6.3 — the driver-facing
//! lifecycle and gate evaluation. The git transport that finalizes a `done`
//! attempt (step 7 boundary commit, step 8 ferry-into-run-repo + archive) is
//! the run repository's job and lands with the scheduler in S4b; `run_attempt`
//! returns the gate outcome (plus the parsed verdict) and the scheduler
//! finalizes it. The single re-prompt on a blocked gate (§6.5) lives here
//! because it is part of the attempt, not the run.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::Receiver;
use tokio::time::{interval, sleep_until, Duration, Instant};

use crate::supervisor::StatusEvent;
use crate::workspace::AgentStatus;

// `Receiver<StatusEvent>` is the pre-send subscription threaded into `drive_turn`
// so the turn's Running→Idle transitions (which may flap faster than any poll)
// are already buffered by the time it reads them.

use super::blackboard::{self, Verdict, VerdictError};
use super::budget::{BudgetLimit, EffectiveBudgets, Ledger};
use super::driver::{AgentDriver, SpawnReq};
use super::gates::{self, GateInputs, GateOutcome};
use super::prompts;
use super::spec::Gate;
use super::types::event_type;

/// The deadlines and watchdog cadence for one attempt (spec §11.1). The
/// scheduler resolves these from the spec's budgets ∪ the hardcoded defaults
/// and passes them in; the defaults here are the §11.1 values.
#[derive(Debug, Clone)]
pub struct Deadlines {
    pub spawn_timeout: Duration,
    pub turn_start_timeout: Duration,
    pub stall_timeout: Duration,
    pub nudge_timeout: Duration,
    /// How often the stall watchdog wakes (spec §11.3: 60s).
    pub watchdog_tick: Duration,
}

impl Default for Deadlines {
    fn default() -> Self {
        Self {
            spawn_timeout: Duration::from_secs(180),
            turn_start_timeout: Duration::from_secs(120),
            stall_timeout: Duration::from_secs(600),
            nudge_timeout: Duration::from_secs(300),
            watchdog_tick: Duration::from_secs(60),
        }
    }
}

/// Everything one attempt needs. The prompt is pre-assembled by the scheduler
/// (via [`prompts::step_prompt`], including any retry preamble); the re-prompt
/// text is composed here from the gate reason.
pub struct AttemptParams {
    pub spawn_req: SpawnReq,
    /// The run's blackboard directory (`…/blackboard`).
    pub blackboard: PathBuf,
    /// The `wf_step_exec` row id — for the ledger's per-attempt turn rollup.
    pub exec_id: String,
    pub step_id: String,
    pub attempt: u32,
    pub iteration: u32,
    pub gate: Gate,
    pub prompt: String,
    pub deadlines: Deadlines,
    /// Whether to re-prompt once on a blocked gate before pausing (spec §6.5).
    pub reprompt_on_block: bool,
}

/// The terminal outcome of an attempt (maps onto the §6.2 attempt states).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptOutcome {
    /// Gate satisfied. The scheduler boundary-commits, ferries, archives, and
    /// advances (S4b).
    Done { verdict: Option<Verdict> },
    /// Gate unmet after the allowed re-prompt → run pauses `blocked_gate`.
    Blocked { reason: String },
    /// Approval gate → run pauses `approval`; a human resolves via `wf_approve`.
    AwaitingApproval,
    /// Spawn/turn-start timeout, agent error, or an exhausted stall → the
    /// scheduler applies the retry policy (spec §6.5).
    Error { error: String },
    /// A run-level budget cap was reached at an enforcement point (spec §11.2) →
    /// the scheduler pauses the run `budget_exceeded`. `which` is `turns` /
    /// `tokens` / `wall_clock`.
    BudgetExceeded { which: String },
}

/// One journalable event the attempt produced, in order. The scheduler attaches
/// the `step_exec_id` and appends each to the run journal (S4b); returning them
/// as data keeps the attempt free of DB/transaction concerns and lets S4a tests
/// assert that every transition is journaled with a cause (spec §16).
#[derive(Debug, Clone)]
pub struct AttemptEvent {
    pub event_type: &'static str,
    pub payload: Value,
}

/// The result of driving one attempt.
pub struct AttemptRun {
    /// The spawned agent, if spawn succeeded — for linking its chat / archival.
    pub agent_id: Option<String>,
    /// The spawned agent's checkout path — where the scheduler runs the boundary
    /// commit + ferry after a `done` (§6.3 steps 7–8). `None` if spawn failed.
    pub worktree: Option<PathBuf>,
    pub outcome: AttemptOutcome,
    pub events: Vec<AttemptEvent>,
}

fn ev(event_type: &'static str, payload: Value) -> AttemptEvent {
    AttemptEvent {
        event_type,
        payload,
    }
}

/// Drive one step attempt to a terminal outcome (spec §6.3 steps 1–6).
///
/// `ledger` / `eff` carry budget enforcement (spec §11.2): the attempt checks
/// `ledger.exceeded()` before each prompt send and charges one turn (plus any
/// token usage) at each turn end, bailing with [`AttemptOutcome::BudgetExceeded`]
/// the moment a run-level cap is reached. The scheduler owns the pre-spawn check
/// and persistence of the mutated ledger.
pub async fn run_attempt(
    driver: &dyn AgentDriver,
    params: AttemptParams,
    ledger: &mut Ledger,
    eff: &EffectiveBudgets,
) -> AttemptRun {
    let AttemptParams {
        spawn_req,
        blackboard,
        exec_id,
        step_id,
        attempt,
        iteration,
        gate,
        prompt,
        deadlines,
        reprompt_on_block,
    } = params;

    let fork_base = spawn_req.fork_base.clone();
    let mut events: Vec<AttemptEvent> = Vec::new();

    // ── 1. Spawn ─────────────────────────────────────────────────────────
    let spawned = match driver.spawn(spawn_req).await {
        Ok(s) => s,
        Err(e) => {
            let error = format!("spawn failed: {e}");
            events.push(ev(event_type::ATTEMPT_ERROR, json!({ "error": error })));
            return AttemptRun {
                agent_id: None,
                worktree: None,
                outcome: AttemptOutcome::Error { error },
                events,
            };
        }
    };
    let agent_id = spawned.agent_id.clone();
    let worktree = spawned.worktree.clone();
    events.push(ev(
        event_type::ATTEMPT_SPAWNED,
        json!({ "agent_id": agent_id, "fork_base": fork_base }),
    ));

    // ── 2. Ready — subscribe-then-check until Idle (deadline: spawn_timeout) ──
    match wait_ready(driver, &agent_id, deadlines.spawn_timeout).await {
        Ready::Idle => events.push(ev(event_type::ATTEMPT_READY, json!({}))),
        Ready::Errored => {
            return terminal_error(driver, &agent_id, "agent error before ready", events).await
        }
        Ready::Timeout => return terminal_error(driver, &agent_id, "spawn_timeout", events).await,
        Ready::Closed => {
            return terminal_error(driver, &agent_id, "supervisor stopped", events).await
        }
    }

    // ── 3. Fork point (commit gate only needs it) ────────────────────────
    let step_dir = match blackboard::step_dir(&blackboard, &step_id) {
        Ok(d) => d,
        Err(e) => {
            return terminal_error(driver, &agent_id, &format!("blackboard error: {e}"), events)
                .await
        }
    };
    let head_start = if matches!(gate, Gate::Commit) {
        crate::git::rev_parse(&spawned.worktree, "HEAD").await.ok()
    } else {
        None
    };

    // ── 4–6. Turn(s) → gate, with a single re-prompt on a blocked gate ───
    let mut prompt_text = prompt;
    let mut prompt_kind = "step";
    let mut reprompts_left: u32 = if reprompt_on_block { 1 } else { 0 };

    loop {
        // Enforcement point: before every prompt/message send (spec §11.2). A run
        // that has spent its turn / token / wall-clock budget pauses now rather
        // than driving another turn.
        if let Some(which) = ledger.exceeded(eff, super::now_ms()) {
            return budget_exceeded_run(agent_id, worktree, which, events);
        }

        // Subscribe BEFORE sending so an arbitrarily fast Running→Idle flap is
        // unlosable (spec §6.3 step 4). Snapshot, archive any stale verdict,
        // then send — in that order.
        let mut rx = driver.subscribe();
        let snapshot = driver.status(&agent_id);
        // Move any leftover verdict aside so this turn's gate only ever reads a
        // verdict written after this prompt (spec §8.3). Journaled on the
        // prompt_sent event rather than a dedicated type.
        let stale_archived = match blackboard::archive_stale_verdict(&step_dir, attempt, iteration)
        {
            Ok(archived) => archived.map(|p| p.to_string_lossy().into_owned()),
            Err(e) => {
                tracing::warn!(error = %e, step = %step_id, "stale-verdict archival failed");
                None
            }
        };

        if let Err(e) = driver.send_message(&agent_id, prompt_text.clone()).await {
            return terminal_error(driver, &agent_id, &format!("send failed: {e}"), events).await;
        }
        let mut prompt_payload = json!({ "kind": prompt_kind });
        if let Some(path) = &stale_archived {
            prompt_payload["stale_verdict_archived"] = json!(path);
        }
        events.push(ev(event_type::PROMPT_SENT, prompt_payload));

        // Turn: wait for start (deadline) then end (stall watchdog). `rx` was
        // subscribed *before* the send above, so the flap is already buffered.
        match drive_turn(
            driver,
            &agent_id,
            &mut rx,
            snapshot,
            &deadlines,
            &mut events,
        )
        .await
        {
            TurnEnd::Ended => events.push(ev(event_type::TURN_ENDED, json!({ "status": "idle" }))),
            TurnEnd::AgentErrored => {
                events.push(ev(event_type::TURN_ENDED, json!({ "status": "error" })));
                return terminal_error(driver, &agent_id, "agent errored mid-turn", events).await;
            }
            TurnEnd::TurnStartTimeout => {
                return terminal_error(driver, &agent_id, "turn_start_timeout", events).await
            }
            TurnEnd::Stalled => return terminal_error(driver, &agent_id, "stalled", events).await,
            TurnEnd::Closed => {
                return terminal_error(driver, &agent_id, "supervisor stopped", events).await
            }
        }

        // Turn complete: count it (the universal unit, spec §11.2) and charge any
        // token usage the provider reported, then journal the ledger tick.
        ledger.charge_turn(&step_id, &exec_id);
        ledger.charge_tokens(&agent_id, &step_id, driver.turn_usage(&agent_id));
        events.push(ev(
            event_type::BUDGET_TICK,
            json!({ "turns": ledger.turns, "tokens": ledger.tokens }),
        ));
        // Enforcement point: at every turn end (spec §11.2).
        if let Some(which) = ledger.exceeded(eff, super::now_ms()) {
            return budget_exceeded_run(agent_id, worktree, which, events);
        }

        // Gate — pure evaluation over freshly gathered facts (spec §6.3 step 6).
        let (verdict, verdict_error) = match blackboard::read_verdict(&step_dir) {
            Ok(v) => (Some(v), None),
            Err(VerdictError::Missing) => (None, None),
            Err(VerdictError::Malformed(e)) => (None, Some(e)),
        };
        let head_end = if matches!(gate, Gate::Commit) {
            crate::git::rev_parse(&spawned.worktree, "HEAD").await.ok()
        } else {
            None
        };
        let artifact_present = match &gate {
            Gate::Artifact { path } => artifact_exists(&spawned.worktree, path),
            _ => false,
        };
        let inputs = GateInputs {
            verdict: verdict.as_ref(),
            verdict_error: verdict_error.as_deref(),
            head_start: head_start.as_deref(),
            head_end: head_end.as_deref(),
            artifact_present,
            approved: false,
        };
        let result = gates::evaluate(&gate, &inputs);
        events.push(ev(
            event_type::GATE_EVALUATED,
            json!({
                "mode": gate_mode(&gate),
                "outcome": outcome_label(&result.outcome),
                "reason": result.reason,
            }),
        ));

        match result.outcome {
            GateOutcome::Done => {
                return AttemptRun {
                    agent_id: Some(agent_id),
                    worktree: Some(worktree),
                    outcome: AttemptOutcome::Done { verdict },
                    events,
                }
            }
            GateOutcome::AwaitingApproval => {
                return AttemptRun {
                    agent_id: Some(agent_id),
                    worktree: Some(worktree),
                    outcome: AttemptOutcome::AwaitingApproval,
                    events,
                }
            }
            GateOutcome::Blocked => {
                if reprompts_left > 0 {
                    reprompts_left -= 1;
                    prompt_kind = "reprompt";
                    prompt_text = prompts::reprompt(&gate, &result.reason);
                    continue;
                }
                return AttemptRun {
                    agent_id: Some(agent_id),
                    worktree: Some(worktree),
                    outcome: AttemptOutcome::Blocked {
                        reason: result.reason,
                    },
                    events,
                };
            }
        }
    }
}

/// Stop the (still-live) agent and return an `Error` outcome. Errored/timed-out
/// attempts must not leave a CLI process running; the scheduler's retry/abandon
/// path (S4b) also stops agents, and `stop` is safe to call more than once.
async fn terminal_error(
    driver: &dyn AgentDriver,
    agent_id: &str,
    error: &str,
    mut events: Vec<AttemptEvent>,
) -> AttemptRun {
    let _ = driver.stop(agent_id).await;
    events.push(ev(event_type::ATTEMPT_ERROR, json!({ "error": error })));
    AttemptRun {
        agent_id: Some(agent_id.to_string()),
        worktree: None,
        outcome: AttemptOutcome::Error {
            error: error.to_string(),
        },
        events,
    }
}

/// Build the `BudgetExceeded` result. The agent is left alive and idle — the
/// scheduler stops it as part of pausing the run (spec §6.5 "pausing stops
/// processes"), mirroring how a `Blocked` attempt is handled.
fn budget_exceeded_run(
    agent_id: String,
    worktree: PathBuf,
    which: BudgetLimit,
    mut events: Vec<AttemptEvent>,
) -> AttemptRun {
    events.push(ev(
        event_type::BUDGET_EXCEEDED,
        json!({ "which": which.as_str() }),
    ));
    AttemptRun {
        agent_id: Some(agent_id),
        worktree: Some(worktree),
        outcome: AttemptOutcome::BudgetExceeded {
            which: which.as_str().to_string(),
        },
        events,
    }
}

enum Ready {
    Idle,
    Errored,
    Timeout,
    Closed,
}

/// Wait for the agent to reach `Idle` (ready to prompt), subscribing before
/// reading its status so a fast Spawning→Idle can't be missed.
async fn wait_ready(driver: &dyn AgentDriver, agent_id: &str, timeout: Duration) -> Ready {
    let mut rx = driver.subscribe();
    match driver.status(agent_id) {
        Some(AgentStatus::Idle) => return Ready::Idle,
        Some(AgentStatus::Error) => return Ready::Errored,
        _ => {}
    }
    let deadline = Instant::now() + timeout;
    loop {
        tokio::select! {
            _ = sleep_until(deadline) => return Ready::Timeout,
            r = rx.recv() => match r {
                Ok(evt) if evt.agent_id == agent_id => match evt.status {
                    AgentStatus::Idle => return Ready::Idle,
                    AgentStatus::Error => return Ready::Errored,
                    _ => {}
                },
                Ok(_) => {}
                Err(RecvError::Lagged(_)) => match driver.status(agent_id) {
                    Some(AgentStatus::Idle) => return Ready::Idle,
                    Some(AgentStatus::Error) => return Ready::Errored,
                    _ => {}
                },
                Err(RecvError::Closed) => return Ready::Closed,
            },
        }
    }
}

enum TurnEnd {
    Ended,
    AgentErrored,
    TurnStartTimeout,
    Stalled,
    Closed,
}

/// Drive one turn: wait for the turn to start (deadline `turn_start_timeout`),
/// then for it to end (`Idle`), running the stall watchdog concurrently once
/// the turn is under way (spec §6.3 step 5, §11.3). `rx` was subscribed before
/// the prompt was sent, so a Running→Idle flap is already buffered here.
async fn drive_turn(
    driver: &dyn AgentDriver,
    agent_id: &str,
    rx: &mut Receiver<StatusEvent>,
    snapshot: Option<AgentStatus>,
    d: &Deadlines,
    events: &mut Vec<AttemptEvent>,
) -> TurnEnd {
    let mut seen_running = matches!(snapshot, Some(AgentStatus::Running));
    let start_deadline = Instant::now() + d.turn_start_timeout;

    // Stall tracking (tokio clock so it's deterministic under `start_paused`).
    let mut last_activity = driver.last_activity(agent_id);
    let mut last_progress = Instant::now();
    let mut nudged = false;
    let mut nudge_deadline = Instant::now();
    let mut ticker = interval(d.watchdog_tick);
    ticker.tick().await; // consume the immediate first tick

    loop {
        tokio::select! {
            // Turn-start deadline — active only until the turn is under way.
            _ = sleep_until(start_deadline), if !seen_running => {
                return TurnEnd::TurnStartTimeout;
            }
            r = rx.recv() => match r {
                Ok(evt) if evt.agent_id == agent_id => match evt.status {
                    AgentStatus::Running => {
                        if !seen_running {
                            seen_running = true;
                            last_progress = Instant::now();
                        }
                    }
                    // Idle ends the turn. If we never observed Running (an
                    // ultra-fast flap where only Idle was left buffered), the
                    // turn still ran and ended — let the gate judge the result.
                    AgentStatus::Idle => return TurnEnd::Ended,
                    AgentStatus::Error | AgentStatus::Stopped => return TurnEnd::AgentErrored,
                    AgentStatus::Spawning => {}
                },
                Ok(_) => {}
                Err(RecvError::Lagged(_)) => match driver.status(agent_id) {
                    Some(AgentStatus::Idle) => return TurnEnd::Ended,
                    Some(AgentStatus::Error) | Some(AgentStatus::Stopped) => {
                        return TurnEnd::AgentErrored
                    }
                    Some(AgentStatus::Running) => seen_running = true,
                    _ => {}
                },
                Err(RecvError::Closed) => return TurnEnd::Closed,
            },
            // Stall watchdog — only meaningful once the turn is running.
            _ = ticker.tick(), if seen_running => {
                let now = driver.last_activity(agent_id);
                if now != last_activity {
                    last_activity = now;
                    last_progress = Instant::now();
                    nudged = false;
                }
                if !nudged && last_progress.elapsed() >= d.stall_timeout {
                    events.push(ev(event_type::WATCHDOG_STALLED, json!({})));
                    let _ = driver.send_message(agent_id, prompts::nudge()).await;
                    events.push(ev(event_type::PROMPT_SENT, json!({ "kind": "nudge" })));
                    nudged = true;
                    nudge_deadline = Instant::now() + d.nudge_timeout;
                } else if nudged && Instant::now() >= nudge_deadline {
                    return TurnEnd::Stalled;
                }
            }
        }
    }
}

/// Existence check for the `artifact` gate. `spec.rs` already rejects absolute
/// paths and `..` at spec time; this re-validates defensively (the path is a
/// filesystem probe against the agent's worktree).
fn artifact_exists(worktree: &Path, rel: &str) -> bool {
    let candidate = Path::new(rel);
    if candidate.is_absolute()
        || candidate
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return false;
    }
    worktree.join(candidate).exists()
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

fn outcome_label(outcome: &GateOutcome) -> &'static str {
    match outcome {
        GateOutcome::Done => "done",
        GateOutcome::Blocked => "blocked",
        GateOutcome::AwaitingApproval => "awaiting_approval",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::driver::{MockDriver, TurnBehavior, TurnUsage};

    fn params(gate: Gate, blackboard: PathBuf, deadlines: Deadlines) -> AttemptParams {
        AttemptParams {
            spawn_req: SpawnReq {
                repo_path: PathBuf::from("/r"),
                provider: "claude".into(),
                model: None,
                instructions: None,
                custom_agent_id: None,
                skills: vec![],
                mcp_servers: vec![],
                fork_base: Some("base-sha".into()),
                run_repo: None,
                owner_run_id: "run-1".into(),
            },
            blackboard,
            exec_id: "exec-1".into(),
            step_id: "plan".into(),
            attempt: 1,
            iteration: 0,
            gate,
            prompt: "do the thing".into(),
            deadlines,
            reprompt_on_block: true,
        }
    }

    /// Run an attempt with an effectively unbounded budget — the default for the
    /// lifecycle tests that don't exercise enforcement.
    async fn run(driver: &dyn AgentDriver, params: AttemptParams) -> AttemptRun {
        let mut ledger = Ledger::default();
        run_attempt(driver, params, &mut ledger, &EffectiveBudgets::default()).await
    }

    fn fast() -> Deadlines {
        Deadlines {
            spawn_timeout: Duration::from_secs(180),
            turn_start_timeout: Duration::from_secs(120),
            stall_timeout: Duration::from_secs(600),
            nudge_timeout: Duration::from_secs(300),
            watchdog_tick: Duration::from_secs(10),
        }
    }

    fn types_present(events: &[AttemptEvent], t: &str) -> bool {
        events.iter().any(|e| e.event_type == t)
    }

    #[tokio::test(start_paused = true)]
    async fn happy_path_verdict_done() {
        let bb = tempfile::tempdir().unwrap();
        let d = MockDriver::new();
        d.set_ready_on_spawn(true);
        d.set_turn_behavior(TurnBehavior::Complete);
        // The agent writes a done verdict into its step dir during the turn.
        let step_dir = blackboard::step_dir(bb.path(), "plan").unwrap();
        d.set_complete_verdict(step_dir, r#"{"result":"done","summary":"ok"}"#);

        let run = run(
            d.as_ref(),
            params(Gate::Verdict, bb.path().to_path_buf(), fast()),
        )
        .await;
        assert!(
            matches!(run.outcome, AttemptOutcome::Done { .. }),
            "{:?}",
            run.outcome
        );
        assert!(types_present(&run.events, event_type::ATTEMPT_SPAWNED));
        assert!(types_present(&run.events, event_type::ATTEMPT_READY));
        assert!(types_present(&run.events, event_type::PROMPT_SENT));
        assert!(types_present(&run.events, event_type::TURN_ENDED));
        assert!(types_present(&run.events, event_type::GATE_EVALUATED));
    }

    #[tokio::test(start_paused = true)]
    async fn spawn_timeout_when_never_ready() {
        let bb = tempfile::tempdir().unwrap();
        let d = MockDriver::new();
        // ready_on_spawn stays false → agent parks in Spawning forever.
        let run = run(
            d.as_ref(),
            params(Gate::Verdict, bb.path().to_path_buf(), fast()),
        )
        .await;
        match run.outcome {
            AttemptOutcome::Error { error } => assert_eq!(error, "spawn_timeout"),
            other => panic!("expected spawn_timeout, got {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn turn_start_timeout_when_prompt_never_wakes_agent() {
        let bb = tempfile::tempdir().unwrap();
        let d = MockDriver::new();
        d.set_ready_on_spawn(true);
        d.set_turn_behavior(TurnBehavior::Silent); // prompt lands, no turn
        let run = run(
            d.as_ref(),
            params(Gate::Verdict, bb.path().to_path_buf(), fast()),
        )
        .await;
        match run.outcome {
            AttemptOutcome::Error { error } => assert_eq!(error, "turn_start_timeout"),
            other => panic!("expected turn_start_timeout, got {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn fast_flap_is_caught_by_subscribe_before_send() {
        // The Running→Idle transitions fire *inside* send_message, i.e. after
        // run_attempt subscribed. A driver that subscribed after sending would
        // miss them and hit turn_start_timeout; catching the flap proves the
        // discipline.
        let bb = tempfile::tempdir().unwrap();
        let d = MockDriver::new();
        d.set_ready_on_spawn(true);
        d.set_turn_behavior(TurnBehavior::Complete);
        let step_dir = blackboard::step_dir(bb.path(), "plan").unwrap();
        d.set_complete_verdict(step_dir, r#"{"result":"done","summary":"flap"}"#);

        let run = run(
            d.as_ref(),
            params(Gate::Verdict, bb.path().to_path_buf(), fast()),
        )
        .await;
        assert!(
            matches!(run.outcome, AttemptOutcome::Done { .. }),
            "{:?}",
            run.outcome
        );
    }

    #[tokio::test(start_paused = true)]
    async fn stall_nudges_then_abandons() {
        let bb = tempfile::tempdir().unwrap();
        let d = MockDriver::new();
        d.set_ready_on_spawn(true);
        d.set_turn_behavior(TurnBehavior::RunningOnly); // turn starts, never ends
        let run = run(
            d.as_ref(),
            params(Gate::Verdict, bb.path().to_path_buf(), fast()),
        )
        .await;
        match run.outcome {
            AttemptOutcome::Error { error } => assert_eq!(error, "stalled"),
            other => panic!("expected stalled, got {other:?}"),
        }
        // Nudged once before abandoning, and the stalled agent was stopped.
        assert!(types_present(&run.events, event_type::WATCHDOG_STALLED));
        let nudges = d
            .sent_messages()
            .into_iter()
            .filter(|(_, t)| t.contains("gone quiet"))
            .count();
        assert_eq!(nudges, 1, "exactly one nudge");
        assert!(d.was_stopped("mock-agent-1"));
    }

    #[tokio::test(start_paused = true)]
    async fn agent_error_mid_turn_fails_the_attempt() {
        let bb = tempfile::tempdir().unwrap();
        let d = MockDriver::new();
        d.set_ready_on_spawn(true);
        d.set_turn_behavior(TurnBehavior::ErrorOut);
        let run = run(
            d.as_ref(),
            params(Gate::Verdict, bb.path().to_path_buf(), fast()),
        )
        .await;
        assert!(
            matches!(run.outcome, AttemptOutcome::Error { .. }),
            "{:?}",
            run.outcome
        );
    }

    #[tokio::test(start_paused = true)]
    async fn spawn_failure_returns_error_without_agent() {
        let bb = tempfile::tempdir().unwrap();
        let d = MockDriver::new();
        d.fail_next_spawn("no repo");
        let run = run(
            d.as_ref(),
            params(Gate::Verdict, bb.path().to_path_buf(), fast()),
        )
        .await;
        assert!(run.agent_id.is_none());
        assert!(matches!(run.outcome, AttemptOutcome::Error { .. }));
    }

    #[tokio::test(start_paused = true)]
    async fn blocked_gate_reprompts_once_then_pauses() {
        let bb = tempfile::tempdir().unwrap();
        let d = MockDriver::new();
        d.set_ready_on_spawn(true);
        d.set_turn_behavior(TurnBehavior::Complete);
        // The agent writes a "revise" verdict each turn → gate stays blocked.
        let step_dir = blackboard::step_dir(bb.path(), "plan").unwrap();
        d.set_complete_verdict(step_dir, r#"{"result":"revise","summary":"more work"}"#);

        let run = run(
            d.as_ref(),
            params(Gate::Verdict, bb.path().to_path_buf(), fast()),
        )
        .await;
        match &run.outcome {
            AttemptOutcome::Blocked { reason } => assert!(reason.contains("revise")),
            other => panic!("expected Blocked, got {other:?}"),
        }
        // step prompt + exactly one re-prompt.
        let prompt_kinds: Vec<String> = run
            .events
            .iter()
            .filter(|e| e.event_type == event_type::PROMPT_SENT)
            .map(|e| e.payload["kind"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(prompt_kinds, vec!["step", "reprompt"]);
    }

    #[tokio::test(start_paused = true)]
    async fn approval_gate_awaits_human() {
        let bb = tempfile::tempdir().unwrap();
        let d = MockDriver::new();
        d.set_ready_on_spawn(true);
        d.set_turn_behavior(TurnBehavior::Complete);
        let run = run(
            d.as_ref(),
            params(Gate::Approval, bb.path().to_path_buf(), fast()),
        )
        .await;
        assert!(
            matches!(run.outcome, AttemptOutcome::AwaitingApproval),
            "{:?}",
            run.outcome
        );
    }

    #[tokio::test(start_paused = true)]
    async fn stale_verdict_does_not_satisfy_a_new_attempt() {
        // A leftover done-verdict from a previous iteration must be archived
        // before the prompt, so a silent agent (writes nothing) is Blocked, not
        // Done (spec §8.3).
        let bb = tempfile::tempdir().unwrap();
        let step_dir = blackboard::step_dir(bb.path(), "plan").unwrap();
        std::fs::create_dir_all(&step_dir).unwrap();
        std::fs::write(
            step_dir.join("verdict.json"),
            r#"{"result":"done","summary":"stale"}"#,
        )
        .unwrap();

        let d = MockDriver::new();
        d.set_ready_on_spawn(true);
        d.set_turn_behavior(TurnBehavior::Complete); // completes but writes no verdict
        let run = run(
            d.as_ref(),
            params(Gate::Verdict, bb.path().to_path_buf(), fast()),
        )
        .await;
        assert!(
            matches!(run.outcome, AttemptOutcome::Blocked { .. }),
            "{:?}",
            run.outcome
        );
        // The stale verdict was archived, journaled on the prompt_sent event.
        assert!(run
            .events
            .iter()
            .any(|e| e.event_type == event_type::PROMPT_SENT
                && e.payload.get("stale_verdict_archived").is_some()));
    }

    // ── budget enforcement (spec §11.2) ──────────────────────────────────────

    #[tokio::test(start_paused = true)]
    async fn turn_budget_pauses_at_turn_end() {
        let bb = tempfile::tempdir().unwrap();
        let d = MockDriver::new();
        d.set_ready_on_spawn(true);
        // A "revise" verdict would normally trigger the re-prompt, but the turn
        // budget of 1 stops the attempt at the first turn end instead.
        d.set_turn_behavior(TurnBehavior::Complete);
        let step_dir = blackboard::step_dir(bb.path(), "plan").unwrap();
        d.set_complete_verdict(step_dir, r#"{"result":"revise","summary":"more"}"#);

        let mut ledger = Ledger::default();
        let eff = EffectiveBudgets {
            turns: 1,
            ..Default::default()
        };
        let run =
            run_attempt(d.as_ref(), params(Gate::Verdict, bb.path().to_path_buf(), fast()), &mut ledger, &eff)
                .await;
        match run.outcome {
            AttemptOutcome::BudgetExceeded { which } => assert_eq!(which, "turns"),
            other => panic!("expected BudgetExceeded(turns), got {other:?}"),
        }
        assert_eq!(ledger.turns, 1, "the completed turn was counted");
        // The turn ran (so it's charged) but the gate was never reached.
        assert!(types_present(&run.events, event_type::TURN_ENDED));
        assert!(types_present(&run.events, event_type::BUDGET_TICK));
        assert!(!types_present(&run.events, event_type::GATE_EVALUATED));
    }

    #[tokio::test(start_paused = true)]
    async fn token_budget_pauses_at_turn_end() {
        let bb = tempfile::tempdir().unwrap();
        let d = MockDriver::new();
        d.set_ready_on_spawn(true);
        d.set_turn_behavior(TurnBehavior::Complete);
        // The first spawned agent is deterministically "mock-agent-1"; give it a
        // cumulative usage above the cap so the turn-end charge trips tokens.
        d.set_usage(
            "mock-agent-1",
            TurnUsage {
                input_tokens: 400,
                output_tokens: 200,
            },
        );

        let mut ledger = Ledger::default();
        let eff = EffectiveBudgets {
            turns: 100,
            tokens: Some(500),
            ..Default::default()
        };
        let run =
            run_attempt(d.as_ref(), params(Gate::Verdict, bb.path().to_path_buf(), fast()), &mut ledger, &eff)
                .await;
        match run.outcome {
            AttemptOutcome::BudgetExceeded { which } => assert_eq!(which, "tokens"),
            other => panic!("expected BudgetExceeded(tokens), got {other:?}"),
        }
        assert_eq!(ledger.tokens, 600);
    }

    #[tokio::test(start_paused = true)]
    async fn exhausted_budget_pauses_before_any_send() {
        // Models a resume where the ledger is already at the cap: the pre-send
        // enforcement point fires before the first prompt is ever sent.
        let bb = tempfile::tempdir().unwrap();
        let d = MockDriver::new();
        d.set_ready_on_spawn(true);
        d.set_turn_behavior(TurnBehavior::Complete);

        let mut ledger = Ledger::default();
        ledger.charge_turn("plan", "prev");
        ledger.charge_turn("plan", "prev");
        let eff = EffectiveBudgets {
            turns: 2,
            ..Default::default()
        };
        let run =
            run_attempt(d.as_ref(), params(Gate::Verdict, bb.path().to_path_buf(), fast()), &mut ledger, &eff)
                .await;
        assert!(
            matches!(run.outcome, AttemptOutcome::BudgetExceeded { .. }),
            "{:?}",
            run.outcome
        );
        // Spawned + ready, but no prompt was sent and no turn ran.
        assert!(types_present(&run.events, event_type::ATTEMPT_READY));
        assert!(!types_present(&run.events, event_type::PROMPT_SENT));
        assert!(d.sent_messages().is_empty());
    }
}
