//! Autopilot: nurse an open PR to mergeable, unattended, on the host.
//!
//! It used to run in the desktop webview, which stops polling when the window
//! is hidden — so autopilot paused exactly when nobody was watching, and a
//! phone could neither see nor steer it. It belongs here by every test the
//! host applies: it must keep running with every window closed, act once
//! however many clients are connected, spend agent turns and publish under the
//! user's name. Clients render [`AutopilotCheckout`] rows and [`LogEntry`]
//! rows and ask the host to flip the switches (`autopilot_set`).
//!
//! - [`readiness`] / [`step`] are the pure decisions, ported from the desktop's
//!   `src/readiness.ts` / `src/autopilot.ts` with every test case.
//! - [`store`] is the durable half: the two switches (same keys and encodings
//!   the desktop wrote, so existing opt-outs survive) and the history log.
//! - [`driver`] is the 10 s loop that reads each enrolled checkout and applies
//!   what [`step::autopilot_step`] decides.
//! - The in-memory table here holds each checkout's cycle. It is also what the
//!   publish-approval gate asks (`rpc::approval`): an enrolled checkout's
//!   `git_push` is pre-approved, since nobody is watching to answer a prompt.
//!
//! On by default — a product decision carried over from the desktop: every
//! project is on until switched off, every agent until paused.

pub mod driver;
pub mod readiness;
pub mod step;
pub mod store;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use parking_lot::Mutex;
use serde::Serialize;

use crate::error::{Error, Result};
use crate::github::PrComments;
use crate::host::EngineCtx;
use crate::supervisor::delegation::{DelegationKind, Key};
use crate::supervisor::Supervisor;
use crate::verify::VerificationReport;
use crate::workspace::AgentRecord;

pub use driver::spawn;
pub use step::{AutopilotState, CyclePhase, GiveUpReason};
pub use store::{LogEntry, Outcome, Switches};

/// One checkout's row changed: an [`AutopilotCheckout`].
pub const EVENT_STATE: &str = "autopilot:state";
/// One [`LogEntry`] was recorded.
pub const EVENT_LOG: &str = "autopilot:event";
/// A switch was written: both opt-out lists, whole ([`Switches`]).
pub const EVENT_SWITCHES: &str = "autopilot:switches";

// ---------------------------------------------------------------------------
// What clients see
// ---------------------------------------------------------------------------

/// The cycle in flight on a checkout, as clients render it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CycleView {
    pub rung: DelegationKind,
    pub attempt: u32,
    pub phase: CyclePhase,
    /// Epoch ms the phase began.
    pub since: i64,
}

/// One checkout's autopilot state: the `autopilot:state` payload and an
/// `autopilot_state` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AutopilotCheckout {
    pub agent_id: String,
    /// `None` for the agent's primary repo.
    pub subdir: Option<String>,
    pub project_id: String,
    /// `project_enabled && !paused` — autopilot is on for this checkout.
    pub enrolled: bool,
    pub paused: bool,
    pub project_enabled: bool,
    pub cycle: Option<CycleView>,
}

/// `autopilot_state` / `autopilot_set`: the rows plus both opt-out lists, so a
/// client can render a project's switch even when it has no agents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AutopilotSnapshot {
    pub checkouts: Vec<AutopilotCheckout>,
    pub disabled_projects: Vec<String>,
    pub paused_agents: Vec<String>,
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// The newest review-thread read of a checkout. Threads are GraphQL (a rate
/// point per read), so the driver reuses this for a minute.
#[derive(Debug, Clone)]
pub(crate) struct ThreadsRead {
    pub at: i64,
    pub comments: Option<PrComments>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Tracked {
    pub state: AutopilotState,
    /// Local verification produced for THIS cycle (cleared when one opens).
    pub verdict: Option<VerificationReport>,
    /// A verification for this checkout is running; the checkout is held until
    /// it lands, as the desktop's pass held on its awaited verify.
    pub verifying: bool,
    pub threads: Option<ThreadsRead>,
}

/// The host's enrolled checkouts. Present = enrolled; absent = not tracked
/// (off, paused, gone, or not ticked yet).
#[derive(Default)]
pub struct Table {
    inner: Mutex<BTreeMap<Key, Tracked>>,
    /// Bumped by every switch write, so a pass that read the switches before
    /// the write stops instead of re-enrolling what was just switched off.
    generation: AtomicU64,
}

fn cycle_view(state: &AutopilotState) -> Option<CycleView> {
    state.cycle.as_ref().map(|c| CycleView {
        rung: c.rung,
        attempt: c.attempt,
        phase: c.phase,
        since: c.phase_since,
    })
}

impl Table {
    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    fn bump(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn keys(&self) -> Vec<Key> {
        self.inner.lock().keys().cloned().collect()
    }

    pub(crate) fn get(&self, key: &Key) -> Option<Tracked> {
        self.inner.lock().get(key).cloned()
    }

    pub fn contains(&self, key: &Key) -> bool {
        self.inner.lock().contains_key(key)
    }

    /// Start tracking `key`; `true` when it was not tracked before.
    pub(crate) fn enroll(&self, key: &Key) -> bool {
        let mut inner = self.inner.lock();
        if inner.contains_key(key) {
            return false;
        }
        inner.insert(
            key.clone(),
            Tracked {
                state: AutopilotState::enrolled(),
                ..Tracked::default()
            },
        );
        true
    }

    pub(crate) fn remove(&self, key: &Key) -> bool {
        self.inner.lock().remove(key).is_some()
    }

    /// Apply `f` to a tracked checkout's state; `None` when it isn't tracked.
    pub(crate) fn update<T>(&self, key: &Key, f: impl FnOnce(&mut Tracked) -> T) -> Option<T> {
        self.inner.lock().get_mut(key).map(f)
    }

    fn cycle_of(&self, key: &Key) -> Option<CycleView> {
        self.inner
            .lock()
            .get(key)
            .and_then(|t| cycle_view(&t.state))
    }
}

/// The host's one table. Global, like the delegation table, because the
/// approval gate that consults it has no supervisor to reach it through.
pub fn global() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(Table::default)
}

/// Whether autopilot pre-authorizes the gated publish `op` on this checkout:
/// `git_push` on an enrolled checkout, and nothing else. Every rung works on a
/// PR that already exists, so `open_pr` — a new, often public artifact under
/// the user's name — always prompts. `repo` is the approval gate's spelling:
/// `None` for the primary checkout.
pub fn pre_authorizes(agent_id: &str, repo: Option<&str>, op: &str) -> bool {
    op == "git_push" && global().contains(&(agent_id.to_string(), repo.map(str::to_string)))
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// Every checkout of an agent, as table-key subdirs: `None` for the primary.
pub(crate) fn checkouts(record: &AgentRecord) -> Vec<Option<String>> {
    record
        .repos
        .iter()
        .enumerate()
        .map(|(i, r)| (i > 0).then(|| r.subdir.clone()))
        .collect()
}

/// The agents autopilot can act on: live, not archived, sidebar-visible (no
/// workflow step agents or planning chats).
pub(crate) fn live_agents(sup: &Supervisor) -> Vec<AgentRecord> {
    sup.workspace
        .current()
        .map(|w| w.agents)
        .unwrap_or_default()
        .into_iter()
        .filter(|a| a.archive.is_none())
        .collect()
}

pub(crate) fn row(
    table: &Table,
    switches: &Switches,
    record: &AgentRecord,
    subdir: Option<String>,
) -> AutopilotCheckout {
    let project_enabled = switches.project_on(&record.project_id);
    let paused = switches.paused(&record.id);
    let key: Key = (record.id.clone(), subdir);
    AutopilotCheckout {
        cycle: table.cycle_of(&key),
        agent_id: key.0,
        subdir: key.1,
        project_id: record.project_id.clone(),
        enrolled: project_enabled && !paused,
        paused,
        project_enabled,
    }
}

pub(crate) fn emit_row(ctx: &EngineCtx, row: &AutopilotCheckout) {
    crate::host::emit(ctx.sink.as_ref(), EVENT_STATE, row);
}

fn read_switches(ctx: &EngineCtx) -> Result<Switches> {
    Ok(store::read_switches(&ctx.db.lock())?)
}

fn snapshot(table: &Table, switches: Switches, agents: &[AgentRecord]) -> AutopilotSnapshot {
    let checkouts = agents
        .iter()
        .flat_map(|a| {
            checkouts(a)
                .into_iter()
                .map(|s| row(table, &switches, a, s))
                .collect::<Vec<_>>()
        })
        .collect();
    AutopilotSnapshot {
        checkouts,
        disabled_projects: switches.disabled_projects,
        paused_agents: switches.paused_agents,
    }
}

// ---------------------------------------------------------------------------
// The ops
// ---------------------------------------------------------------------------

/// `autopilot_state`: one agent's checkouts, or every live agent's.
pub fn autopilot_state_impl(
    ctx: &EngineCtx,
    sup: &Supervisor,
    agent_id: Option<&str>,
) -> Result<AutopilotSnapshot> {
    state(global(), ctx, sup, agent_id)
}

fn state(
    table: &Table,
    ctx: &EngineCtx,
    sup: &Supervisor,
    agent_id: Option<&str>,
) -> Result<AutopilotSnapshot> {
    let switches = read_switches(ctx)?;
    let agents = match agent_id {
        Some(id) => {
            let record = sup.workspace.agent(id)?;
            if record.archive.is_some() {
                Vec::new()
            } else {
                vec![record]
            }
        }
        None => live_agents(sup),
    };
    Ok(snapshot(table, switches, &agents))
}

/// `autopilot_set`: flip a project's switch or pause/resume one agent. Takes
/// effect before it returns: a checkout switched off loses its cycle and its
/// push pre-approval now, not at the next tick.
///
/// Announces every write as `autopilot:switches` first — both lists whole, so
/// every client's switches agree even when no checkout changed (a project
/// with no agents) — then `autopilot:state` for each affected checkout.
pub fn autopilot_set_impl(
    ctx: &EngineCtx,
    sup: &Supervisor,
    project_id: Option<&str>,
    agent_id: Option<&str>,
    enabled: bool,
) -> Result<AutopilotSnapshot> {
    set(global(), ctx, sup, project_id, agent_id, enabled)
}

fn set(
    table: &Table,
    ctx: &EngineCtx,
    sup: &Supervisor,
    project_id: Option<&str>,
    agent_id: Option<&str>,
    enabled: bool,
) -> Result<AutopilotSnapshot> {
    fn clean(s: Option<&str>) -> Option<&str> {
        s.map(str::trim).filter(|s| !s.is_empty())
    }
    let agents = live_agents(sup);
    let affected: Vec<&AgentRecord> = match (clean(project_id), clean(agent_id)) {
        (Some(project_id), None) => {
            store::set_project(&ctx.db.lock(), project_id, enabled)?;
            agents
                .iter()
                .filter(|a| a.project_id == project_id)
                .collect()
        }
        (None, Some(agent_id)) => {
            sup.workspace.agent(agent_id)?;
            store::set_agent_paused(&ctx.db.lock(), agent_id, !enabled, |id| {
                sup.workspace.agent(id).is_ok()
            })?;
            agents.iter().filter(|a| a.id == agent_id).collect()
        }
        _ => {
            return Err(Error::Other(
                "autopilot_set takes exactly one of projectId or agentId".into(),
            ))
        }
    };
    table.bump();
    let switches = read_switches(ctx)?;
    crate::host::emit(ctx.sink.as_ref(), EVENT_SWITCHES, &switches);
    for record in affected {
        let on = switches.agent_on(&record.project_id, &record.id);
        for subdir in checkouts(record) {
            if !on {
                table.remove(&(record.id.clone(), subdir.clone()));
            }
            emit_row(ctx, &row(table, &switches, record, subdir));
        }
    }
    // Switched on: enroll at the next pass rather than in ten seconds.
    driver::nudge();
    Ok(snapshot(table, switches, &agents))
}

/// `autopilot_log`: what autopilot did, newest first. `subdir` names one
/// checkout; the primary's own subdir names the primary, as everywhere else.
pub fn autopilot_log_impl(
    ctx: &EngineCtx,
    sup: &Supervisor,
    agent_id: Option<&str>,
    subdir: Option<&str>,
) -> Result<Vec<LogEntry>> {
    let subdir = subdir.map(str::trim).filter(|s| !s.is_empty());
    let Some(agent_id) = agent_id else {
        return store::read_log(&ctx.db.lock(), store::LogScope::All);
    };
    let Some(subdir) = subdir else {
        return store::read_log(&ctx.db.lock(), store::LogScope::Agent(agent_id));
    };
    // A discarded agent's record is gone, but its rows may linger until the
    // next prune: read those by the name as given.
    let primary = sup
        .workspace
        .agent(agent_id)
        .ok()
        .and_then(|r| r.repos.first().map(|p| p.subdir == subdir))
        .unwrap_or(false);
    let subdir = (!primary).then_some(subdir);
    store::read_log(&ctx.db.lock(), store::LogScope::Checkout(agent_id, subdir))
}

/// Record and announce one log row.
pub(crate) fn record(ctx: &EngineCtx, entry: &LogEntry) {
    if let Err(e) = store::append_log(&ctx.db.lock(), entry) {
        tracing::warn!(error = %e, agent_id = %entry.agent_id, "autopilot: log row not stored");
    }
    crate::host::emit(ctx.sink.as_ref(), EVENT_LOG, entry);
}

#[cfg(test)]
mod tests;
