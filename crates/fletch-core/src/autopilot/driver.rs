//! The autopilot driver: one 10 s tick over every enrolled checkout, applying
//! what [`autopilot_step`] decides. Modelled on `roadmap::drainer`: a `Notify`
//! nudge, each pass on its own task so a panic cannot end the loop, one pass at
//! a time.
//!
//! The world is read through [`World`], so a pass is testable without git or
//! GitHub. What it reads, and how often:
//!
//! - **git state** (local) only when the step could use it: no cycle in flight
//!   and the agent free, or a cycle awaiting evidence. A `working` cycle is
//!   judged on the agent's status alone.
//! - **PR state + CI** (`get_pr_live`, ETag-conditional REST) at most once per
//!   tick, under the same condition.
//! - **review threads** (GraphQL, a rate point each) only for an open PR, and
//!   at most once a minute per checkout — except once more when a cycle starts
//!   awaiting evidence, so a comment round is never judged on threads read
//!   before it ran.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::sync::Notify;

use super::readiness::{CommitMode, LadderContext, ReadinessInput};
use super::step::{autopilot_step, AutopilotInput, CyclePhase, Effect};
use super::store::{self, LogEntry, Outcome};
use super::{checkouts, emit_row, record, row, Table, ThreadsRead};
use crate::error::Result;
use crate::git_state::GitState;
use crate::github::{PrComments, PrLive, PrStatus};
use crate::host::EngineCtx;
use crate::supervisor::delegation::{DelegationKind, Key};
use crate::supervisor::Supervisor;
use crate::verify::VerificationReport;
use crate::workspace::{AgentRecord, AgentStatus};

/// How often to evaluate enrolled checkouts. Every action costs an agent turn,
/// so there is nothing to gain from reacting inside a second.
const TICK: Duration = Duration::from_secs(10);

/// Review threads are re-read at most this often per checkout.
const THREADS_EVERY_MS: i64 = 60_000;

/// Passes between sweeps of log rows whose agent is gone (~an hour).
const PRUNE_EVERY: u64 = 360;

fn signal() -> &'static Notify {
    static SIGNAL: OnceLock<Notify> = OnceLock::new();
    SIGNAL.get_or_init(Notify::new)
}

/// Wake the driver now.
pub(crate) fn nudge() {
    signal().notify_one();
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Everything a pass reads from, or does to, the world beyond the table and
/// the database. One implementation drives the real supervisor; the tests
/// script another.
pub(crate) trait World: Send + Sync + 'static {
    /// Live, non-archived agents.
    fn agents(&self) -> Vec<AgentRecord>;
    /// The agent is mid-turn (running or still spawning).
    fn busy(&self, agent_id: &str) -> bool;
    /// A delegation is live on this checkout.
    fn delegation_live(&self, key: &Key) -> bool;
    /// The checkout's git state; `None` when unreadable or refused.
    fn git(&self, key: &Key) -> impl Future<Output = Option<GitState>> + Send;
    fn pr(&self, key: &Key) -> impl Future<Output = Option<PrLive>> + Send;
    fn threads(&self, key: &Key) -> impl Future<Output = Option<PrComments>> + Send;
    /// Hand a rung to the agent, as `delegate_git` would.
    fn dispatch(
        &self,
        key: &Key,
        action: &str,
        params: &BTreeMap<String, String>,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Run the project's verify commands in this checkout; `None` when they
    /// couldn't run (which must never read as a fix that failed).
    fn verify(&self, key: &Key) -> impl Future<Output = Option<VerificationReport>> + Send;
}

/// The production world: the host's supervisor.
pub(crate) struct Live {
    pub sup: Arc<Supervisor>,
    pub ctx: Arc<EngineCtx>,
}

impl World for Live {
    fn agents(&self) -> Vec<AgentRecord> {
        super::live_agents(&self.sup)
    }

    fn busy(&self, agent_id: &str) -> bool {
        matches!(
            self.sup.status_of(agent_id),
            Some(AgentStatus::Running | AgentStatus::Spawning)
        )
    }

    fn delegation_live(&self, key: &Key) -> bool {
        crate::supervisor::delegation::is_live(key)
    }

    async fn git(&self, key: &Key) -> Option<GitState> {
        // A checkout Fletch refuses to run git in reads as a zero state —
        // "clean, nothing unpushed" — which must not count as a world.
        crate::commands::get_git_state_impl(&self.sup, &key.0, key.1.as_deref())
            .await
            .ok()
            .flatten()
            .filter(|g| g.blocked_config.is_empty())
    }

    async fn pr(&self, key: &Key) -> Option<PrLive> {
        crate::commands::get_pr_live_impl(&self.sup, &key.0, key.1.as_deref())
            .await
            .ok()
            .flatten()
    }

    async fn threads(&self, key: &Key) -> Option<PrComments> {
        crate::commands::get_pr_threads_impl(&self.sup, &key.0, key.1.as_deref())
            .await
            .ok()
            .flatten()
    }

    async fn dispatch(
        &self,
        key: &Key,
        action: &str,
        params: &BTreeMap<String, String>,
    ) -> Result<()> {
        crate::commands::delegate_git_impl(
            &self.sup,
            &self.ctx,
            &key.0,
            key.1.as_deref(),
            action,
            params,
        )
        .await
        .map(|_| ())
    }

    async fn verify(&self, key: &Key) -> Option<VerificationReport> {
        match crate::commands::run_verification_impl(&self.sup, &key.0, key.1.as_deref()).await {
            Ok(report) => {
                crate::supervisor::emit_verification(
                    self.ctx.sink.as_ref(),
                    &key.0,
                    report.clone(),
                );
                Some(report)
            }
            Err(e) => {
                tracing::info!(agent_id = %key.0, error = %e, "autopilot: verification did not run");
                None
            }
        }
    }
}

/// Read the world one checkout's step is judged on.
async fn observe<W: World>(
    world: &W,
    table: &Table,
    key: &Key,
    awaiting_since: Option<i64>,
    cached: Option<ThreadsRead>,
    now: i64,
) -> ReadinessInput {
    let git = world.git(key).await;
    let (pr, checks) = match world.pr(key).await {
        Some(live) => (Some(live.state), live.checks),
        None => (None, None),
    };
    let comments = if pr.as_ref().is_some_and(|p| p.state == PrStatus::Open) {
        let stale = cached.as_ref().map_or(true, |c| {
            now - c.at >= THREADS_EVERY_MS || awaiting_since.is_some_and(|since| c.at < since)
        });
        if stale {
            let comments = world.threads(key).await;
            table.update(key, |t| {
                t.threads = Some(ThreadsRead {
                    at: now,
                    comments: comments.clone(),
                })
            });
            comments
        } else {
            cached.and_then(|c| c.comments)
        }
    } else {
        None
    };
    ReadinessInput {
        git,
        pr,
        checks,
        comments,
    }
}

fn log_row(
    key: &Key,
    now: i64,
    outcome: Outcome,
    rung: DelegationKind,
    attempt: u32,
    reason: Option<super::GiveUpReason>,
) -> LogEntry {
    LogEntry {
        id: uuid::Uuid::new_v4().to_string(),
        agent_id: key.0.clone(),
        subdir: key.1.clone(),
        at: now,
        outcome,
        rung,
        attempt,
        reason,
    }
}

/// One sweep: enroll and drop checkouts against the switches, then decide and
/// apply one effect per enrolled checkout.
pub(crate) async fn run_pass<W: World>(
    table: &'static Table,
    ctx: &Arc<EngineCtx>,
    world: &Arc<W>,
    now: i64,
) {
    let generation = table.generation();
    let switches = match store::read_switches(&ctx.db.lock()) {
        Ok(s) => s,
        Err(e) => {
            // On by default must fail closed: not knowing who opted out is no
            // licence to act on them.
            tracing::warn!(error = %e, "autopilot: switches unreadable — pass skipped");
            return;
        }
    };
    let agents = world.agents();
    let by_id: HashMap<&str, &AgentRecord> = agents.iter().map(|a| (a.id.as_str(), a)).collect();
    let mut keys: BTreeSet<Key> = table.keys().into_iter().collect();
    let mut on: HashMap<Key, &AgentRecord> = HashMap::new();
    for agent in &agents {
        if switches.agent_on(&agent.project_id, &agent.id) {
            for subdir in checkouts(agent) {
                let key = (agent.id.clone(), subdir);
                keys.insert(key.clone());
                on.insert(key, agent);
            }
        }
    }

    // Agents handed a rung earlier in THIS pass. The `running` a dispatch
    // raises can arrive after the next checkout is read, and two checkouts of
    // one agent dispatching together would coalesce into one turn.
    let mut dispatched_to: HashSet<String> = HashSet::new();
    for key in keys {
        // A switch was written since this pass read them: stop rather than
        // act on the old answer. The next pass reads the new one.
        if table.generation() != generation {
            return;
        }
        let Some(agent) = on.get(&key).copied() else {
            // Gone, switched off or paused: forget its state (and its cycle).
            if table.remove(&key) {
                if let Some(record) = by_id.get(key.0.as_str()) {
                    emit_row(ctx, &row(table, &switches, record, key.1.clone()));
                }
            }
            continue;
        };
        // On by default: an unseen checkout is enrolled on its first tick.
        if table.enroll(&key) {
            emit_row(ctx, &row(table, &switches, agent, key.1.clone()));
        }
        let Some(tracked) = table.get(&key) else {
            continue;
        };
        if tracked.verifying {
            continue;
        }
        let busy = world.busy(&key.0) || dispatched_to.contains(&key.0);
        let in_flight = world.delegation_live(&key);
        let cycle = tracked.state.cycle.as_ref();
        let awaiting_since = cycle
            .filter(|c| c.phase == CyclePhase::AwaitingEvidence)
            .map(|c| c.phase_since);
        let needs_world = match cycle {
            None => !busy && !in_flight,
            Some(_) => awaiting_since.is_some(),
        };
        let readiness = if needs_world {
            observe(
                world.as_ref(),
                table,
                &key,
                awaiting_since,
                tracked.threads.clone(),
                now,
            )
            .await
        } else {
            ReadinessInput::default()
        };
        let base = readiness
            .git
            .as_ref()
            .map(|g| g.parent_branch.clone())
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| "main".to_string());
        let ladder = LadderContext {
            base,
            commit_mode: CommitMode::CommitPr,
        };
        let effect = autopilot_step(&AutopilotInput {
            state: Some(&tracked.state),
            readiness: &readiness,
            ladder: &ladder,
            agent_busy: busy,
            delegation_in_flight: in_flight,
            verification: tracked.verdict.as_ref(),
            now,
        });
        if table.generation() != generation {
            return;
        }
        if matches!(effect, Effect::Dispatch { .. }) {
            dispatched_to.insert(key.0.clone());
        }
        apply(table, ctx, world, &switches, agent, &key, effect, now).await;
    }
}

/// Perform one effect, announcing the row it changed and logging the four
/// effects a user would ask about (`wait` is most ticks; `verify` and
/// `await-evidence` are steps inside a cycle whose outcome says how it went).
#[allow(clippy::too_many_arguments)]
async fn apply<W: World>(
    table: &'static Table,
    ctx: &Arc<EngineCtx>,
    world: &Arc<W>,
    switches: &store::Switches,
    agent: &AgentRecord,
    key: &Key,
    effect: Effect,
    now: i64,
) {
    let announce = || emit_row(ctx, &row(table, switches, agent, key.1.clone()));
    let attempt_now = || {
        table
            .get(key)
            .and_then(|t| t.state.cycle.map(|c| c.attempt))
            .unwrap_or(0)
    };
    match effect {
        Effect::Dispatch {
            rung,
            action,
            params,
            signature,
            situation,
        } => {
            table.update(key, |t| {
                t.state.open_cycle(rung, signature, situation, now);
                t.verdict = None;
            });
            announce();
            record(
                ctx,
                &log_row(key, now, Outcome::Dispatch, rung, attempt_now(), None),
            );
            if let Err(e) = world.dispatch(key, action, &params).await {
                // The cycle stays open: the next tick finds the agent free and
                // judges it, which spends a budget slot rather than looping.
                tracing::warn!(agent_id = %key.0, error = %e, "autopilot: rung not delegated");
            }
        }
        Effect::AwaitEvidence => {
            table.update(key, |t| t.state.advance(CyclePhase::AwaitingEvidence, now));
            announce();
        }
        Effect::Verify => {
            // Enter awaiting-evidence first: the evidence clock starts now.
            let identity = table.update(key, |t| {
                t.state.advance(CyclePhase::AwaitingEvidence, now);
                t.verifying = true;
                t.state.cycle.clone()
            });
            announce();
            let (world, key) = (world.clone(), key.clone());
            crate::host::spawn(async move {
                let report = world.verify(&key).await;
                table.update(&key, |t| {
                    t.verifying = false;
                    // Only evidence for the cycle it was produced for counts.
                    if t.state.cycle.as_ref().map(|c| (c.rung, c.attempt))
                        == identity.flatten().map(|c| (c.rung, c.attempt))
                    {
                        t.verdict = report;
                    }
                });
                nudge();
            });
        }
        Effect::Settle { rung } => {
            let attempt = attempt_now();
            record(
                ctx,
                &log_row(key, now, Outcome::Settle, rung, attempt, None),
            );
            table.update(key, |t| t.state.settle(rung));
            announce();
        }
        Effect::Retry { rung, barren } => {
            let attempt = attempt_now();
            record(ctx, &log_row(key, now, Outcome::Retry, rung, attempt, None));
            table.update(key, |t| t.state.retry(rung, barren.as_deref()));
            announce();
        }
        Effect::GiveUp {
            rung,
            reason,
            barren,
        } => {
            // Retry's bookkeeping; the row is the one a returning user looks
            // for (and the phone is pushed, see `remote::push`).
            let attempt = attempt_now();
            record(
                ctx,
                &log_row(key, now, Outcome::GiveUp, rung, attempt, Some(reason)),
            );
            table.update(key, |t| t.state.retry(rung, barren.as_deref()));
            announce();
        }
        Effect::Wait { .. } => {}
    }
}

/// Start the driver: a pass every [`TICK`] and on every [`nudge`].
pub fn spawn(ctx: Arc<EngineCtx>, sup: Arc<Supervisor>) {
    let world = Arc::new(Live {
        sup,
        ctx: ctx.clone(),
    });
    crate::host::spawn(async move {
        let mut passes: u64 = 0;
        loop {
            if passes % PRUNE_EVERY == 0 {
                if let Err(e) = store::prune_orphans(&ctx.db.lock()) {
                    tracing::warn!(error = %e, "autopilot: log prune failed");
                }
            }
            passes = passes.wrapping_add(1);
            let pass = {
                let (ctx, world) = (ctx.clone(), world.clone());
                crate::host::spawn(async move {
                    run_pass(super::global(), &ctx, &world, now_ms()).await;
                })
                .await
            };
            if let Err(e) = pass {
                tracing::error!(error = %e, "autopilot pass panicked — autopilot continues");
            }
            tokio::select! {
                _ = tokio::time::sleep(TICK) => {}
                _ = signal().notified() => {}
            }
        }
    });
}

#[cfg(test)]
#[path = "driver_tests.rs"]
mod tests;
