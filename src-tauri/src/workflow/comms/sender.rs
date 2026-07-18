//! Sender resolution: mapping a run-owned agent's mailbox to the live step
//! attempt behind it, and resolving that step's declared comms caps and the
//! active orchestrator (spec §10.1, §10.2).

use rusqlite::{Connection, OptionalExtension};

use crate::error::{Error, Result};
use crate::workflow::spec::{Block, CommsCap, Orchestrate, Spec};

// ───────────────────────────── caps matrix (pure) ───────────────────────────

/// The step-exec `step_id` prefix of a stage-lived orchestrator (spec §10.2).
/// `orchestrate-<block-index>`: the block has no id of its own, so the engine
/// synthesizes a stable one from its position in the immutable spec. Children of
/// an orchestrate stage resolve their caps from that block (below).
pub(super) const ORCH_PREFIX: &str = "orchestrate-";

// ───────────────────────────── sender resolution ────────────────────────────

/// The run/step context of the agent that issued a comms op.
pub(super) struct Sender {
    pub(super) run_id: String,
    pub(super) step_exec_id: String,
    pub(super) step_id: String,
    pub(super) caps: Vec<CommsCap>,
}

/// Find a `Step` anywhere in a block tree, so a sender's caps resolve regardless
/// of where the step sits (top level, loop body, parallel/orchestrate children).
fn step_caps(spec: &Spec, step_id: &str) -> Option<Vec<CommsCap>> {
    fn walk<'a>(blocks: &'a [Block], id: &str) -> Option<&'a [CommsCap]> {
        for b in blocks {
            match b {
                Block::Step(s) if s.id == id => return Some(&s.comms),
                Block::Step(_) => {}
                Block::Loop(l) => {
                    if let Some(c) = walk(&l.body, id) {
                        return Some(c);
                    }
                }
                Block::Parallel(p) => {
                    if let Some(s) = p.steps.iter().find(|s| s.id == id) {
                        return Some(&s.comms);
                    }
                }
                Block::Orchestrate(o) => {
                    if let Some(s) = o.body.iter().find(|s| s.id == id) {
                        return Some(&s.comms);
                    }
                }
            }
        }
        None
    }
    walk(&spec.workflow, step_id).map(<[CommsCap]>::to_vec)
}

/// Resolve the live step attempt behind a run-owned agent's mailbox. Keyed by
/// `run_id` (which the dispatcher captures from the agent's `owner_run_id` at
/// spawn) rather than `agent_id`: the scheduler only stamps `wf_step_exec.
/// agent_id` *after* the turn completes, so during the turn — exactly when a
/// comms op fires — that column is still NULL and an agent-id lookup would miss.
/// When the row is already linked to `agent_id` (a future/parallel case) that
/// row is preferred; otherwise the run's single in-flight attempt is used, and
/// concurrent in-flight attempts (parallel comms, unsupported in v1) are an
/// explicit error rather than a silent misattribution.
pub(super) fn resolve_sender(conn: &Connection, run_id: &str, agent_id: &str) -> Result<Sender> {
    let live: Vec<(String, String, Option<String>)> = conn
        .prepare(
            "SELECT id, step_id, agent_id FROM wf_step_exec
             WHERE run_id = ?1 AND status IN ('spawning','running','gating')
             ORDER BY rowid DESC",
        )
        .and_then(|mut s| {
            s.query_map([run_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|e| Error::Other(e.to_string()))?;

    let (step_exec_id, step_id) =
        if let Some((id, step, _)) = live.iter().find(|(_, _, a)| a.as_deref() == Some(agent_id)) {
            (id.clone(), step.clone())
        } else {
            match live.as_slice() {
                [] => return Err(Error::Other("no live step for this run".into())),
                [(id, step, _)] => (id.clone(), step.clone()),
                _ => {
                    return Err(Error::Other(
                        "cannot attribute a comms op among concurrent steps \
                     (parallel comms is not supported yet)"
                            .into(),
                    ))
                }
            }
        };

    let spec_json: String = conn
        .query_row(
            "SELECT spec_json FROM wf_run WHERE id = ?1",
            [run_id],
            |r| r.get(0),
        )
        .map_err(|e| Error::Other(e.to_string()))?;
    let spec: Spec = serde_json::from_str(&spec_json).map_err(|e| Error::Other(e.to_string()))?;
    let caps = resolve_caps(conn, &spec, run_id, &step_id)?;

    Ok(Sender {
        run_id: run_id.to_string(),
        step_exec_id,
        step_id,
        caps,
    })
}

/// The declared comms caps of the sender (spec §10.1, §10.2):
/// - the **orchestrator** (its `step_id` starts with [`ORCH_PREFIX`]) gets every
///   cap ("orchestrator gets all", §5.1);
/// - a **child of the active orchestrate stage** takes the orchestrate block's
///   `comms` (its children's caps) — this covers both static-body children and
///   dynamically spawned ones, whose synthetic ids aren't in the spec;
/// - any other step resolves its own declared caps from the block tree.
pub(super) fn resolve_caps(
    conn: &Connection,
    spec: &Spec,
    run_id: &str,
    step_id: &str,
) -> Result<Vec<CommsCap>> {
    if step_id.starts_with(ORCH_PREFIX) {
        return Ok(vec![CommsCap::Report, CommsCap::Ask, CommsCap::Notify]);
    }
    if let Some((_, orch_step_id)) = live_orchestrator(conn, run_id) {
        if let Some(orch) = orchestrate_block(spec, &orch_step_id) {
            return Ok(orch.comms.clone());
        }
    }
    step_caps(spec, step_id)
        .ok_or_else(|| Error::Other(format!("step '{step_id}' not found in run spec")))
}

/// The live orchestrator's `(step_exec_id, step_id)` for a run, if a stage is
/// active. At most one orchestrate stage runs at a time (nested orchestrate is
/// forbidden, §5.2; sequential stages don't overlap), so a single live exec whose
/// `step_id` starts with [`ORCH_PREFIX`] identifies it.
pub(super) fn live_orchestrator(conn: &Connection, run_id: &str) -> Option<(String, String)> {
    conn.query_row(
        "SELECT id, step_id FROM wf_step_exec
         WHERE run_id = ?1 AND status IN ('spawning','running','gating')
           AND step_id LIKE 'orchestrate-%'
         ORDER BY rowid DESC LIMIT 1",
        [run_id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )
    .optional()
    .ok()
    .flatten()
}

/// The `Orchestrate` block an orchestrator `step_id` (`orchestrate-<idx>`) refers
/// to, by parsing the index and indexing the immutable top-level spec sequence.
pub(super) fn orchestrate_block<'a>(spec: &'a Spec, orch_step_id: &str) -> Option<&'a Orchestrate> {
    let idx: usize = orch_step_id.strip_prefix(ORCH_PREFIX)?.parse().ok()?;
    match spec.workflow.get(idx)? {
        Block::Orchestrate(o) => Some(o),
        _ => None,
    }
}

/// The synthetic `step_id` for the orchestrator of the top-level block at `index`.
pub(in crate::workflow) fn orch_step_id(block_index: usize) -> String {
    format!("{ORCH_PREFIX}{block_index}")
}

// ───────────────────────────── routing core (testable) ──────────────────────

/// What the router decided a comms op needs the caller to do next.
pub(super) enum Poke {
    /// Nothing beyond the response (report / rejection).
    None,
    /// A `wf_ask` to the human was queued for `run_id`: raise its pending-ask
    /// flag so the attempt pauses `question` at turn end (§10.4).
    AskQueued { run_id: String },
}
