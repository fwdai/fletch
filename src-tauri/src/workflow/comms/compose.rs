//! Dynamic composition (spec §10.3): validate a `wf_compose` request — fragment,
//! depth, caps-escalation (§15), `max_sub_runs`, budget-fit — and queue the
//! resulting plan for the orchestrate stage to launch, plus the accounting the
//! fit-checks read.

use std::collections::BTreeMap;

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::rpc::Response;
use crate::workflow::budget::{EffectiveBudgets, Ledger};
use crate::workflow::scheduler;
use crate::workflow::spec::{self, AgentSpec, Block, Budgets, CommsCap, Integrate, Spec};
use crate::workflow::types::event_type;

use super::sender::{orchestrate_block, Poke, Sender, ORCH_PREFIX};
use super::{insert_message, load_spec, new_msg_id};

/// Which commit a composed sub-run forks from (spec §10.3).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::workflow) enum ComposeBase {
    /// The orchestrate stage's entry HEAD (the parent's current line).
    ParentHead,
    /// The run's original base commit.
    RunBase,
}

/// A validated `wf_compose` request (spec §10.3), normalized into the fields the
/// scheduler needs to create and drive the sub-run. Serialized into the queued
/// `decision` message so a resume rebuilds it exactly.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(in crate::workflow) struct ComposePlan {
    pub task: String,
    pub fragment: Vec<Block>,
    /// Sub-run agent map; `None` inherits the parent's agents (spec §10.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<BTreeMap<String, AgentSpec>>,
    /// Reserved run-level turn slice (required, §10.3).
    pub turns: i64,
    /// Reserved run-level token slice, if the parent run has a token cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<i64>,
    pub integrate: Integrate,
    pub base: ComposeBase,
    /// The orchestrate stage this sub-run belongs to (its top-level block index),
    /// so it integrates at that stage's join.
    pub block_index: usize,
}

/// Validate + record a `wf_compose` request (spec §10.3). Runs the fragment
/// through the full [`spec::validate`], enforces the depth cap, the caps-escalation
/// rules (§15), `max_sub_runs`, and the budget-fit check, then queues a
/// [`Decision::Compose`](super::Decision::Compose) the orchestrate stage executes. Every rejection is a
/// structured error plus a `compose_denied` journal entry — the deterministic
/// engine is the authority, never the orchestrator.
pub(super) fn route_compose(
    conn: &Connection,
    app: Option<&AppHandle>,
    sender: &Sender,
    id: &str,
    args: &Value,
) -> (Response, Poke) {
    let deny = |conn: &Connection, msg: String| -> (Response, Poke) {
        journal_compose_denied(conn, app, sender, &msg);
        (Response::err(id, msg), Poke::None)
    };

    // The stage must be an orchestrate block with `compose` enabled.
    let Some(spec) = load_spec(conn, &sender.run_id) else {
        return (Response::err(id, "run spec unavailable"), Poke::None);
    };
    let Some(orch) = orchestrate_block(&spec, &sender.step_id) else {
        return deny(
            conn,
            "wf_compose is only valid within an orchestrate stage".into(),
        );
    };
    let Some(limits) = orch.compose.clone() else {
        return deny(
            conn,
            "dynamic composition is not enabled for this stage".into(),
        );
    };
    let allowed_caps = orch.comms.clone();

    // ── Parse the request (spec §10.3). ──
    let task = args
        .get("task")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if task.is_empty() {
        return deny(conn, "wf_compose requires a non-empty `task`".into());
    }
    let Some(frag_val) = args.get("fragment") else {
        return deny(conn, "wf_compose requires a `fragment` block list".into());
    };
    let fragment: Vec<Block> = match serde_json::from_value(frag_val.clone()) {
        Ok(f) => f,
        Err(e) => {
            return deny(
                conn,
                format!("wf_compose `fragment` is not a valid block list: {e}"),
            )
        }
    };
    let agents: Option<BTreeMap<String, AgentSpec>> = match args.get("agents") {
        Some(v) if !v.is_null() => match serde_json::from_value(v.clone()) {
            Ok(a) => Some(a),
            Err(e) => {
                return deny(
                    conn,
                    format!("wf_compose `agents` is not a valid agent map: {e}"),
                )
            }
        },
        _ => None,
    };
    let Some(budgets) = args.get("budgets") else {
        return deny(
            conn,
            "wf_compose requires `budgets` with a `turns` slice (§10.3)".into(),
        );
    };
    let turns = budgets.get("turns").and_then(|v| v.as_i64()).unwrap_or(0);
    if turns <= 0 {
        return deny(
            conn,
            "wf_compose `budgets.turns` must be a positive number".into(),
        );
    }
    let req_tokens = budgets
        .get("tokens")
        .and_then(|v| v.as_i64())
        .filter(|t| *t > 0);
    let integrate = match args
        .get("integrate")
        .and_then(|v| v.as_str())
        .unwrap_or("none")
    {
        "none" => Integrate::None,
        "merge" => Integrate::Merge,
        other => {
            return deny(
                conn,
                format!("wf_compose `integrate` must be \"none\" or \"merge\", got \"{other}\""),
            )
        }
    };
    let base = match args
        .get("base")
        .and_then(|v| v.as_str())
        .unwrap_or("parent-head")
    {
        "parent-head" => ComposeBase::ParentHead,
        "run-base" => ComposeBase::RunBase,
        other => {
            return deny(
                conn,
                format!(
                    "wf_compose `base` must be \"parent-head\" or \"run-base\", got \"{other}\""
                ),
            )
        }
    };

    // ── Validate the fragment with the full spec.rs rules (§5.2), by wrapping it
    //    in a synthetic Spec that also carries the reserved budget slice. ──
    let eff_agents = agents.clone().unwrap_or_else(|| spec.agents.clone());
    let sub_spec = Spec {
        version: spec.version,
        name: format!("{} — composed", spec.name),
        description: None,
        budgets: Some(Budgets {
            turns: Some(turns),
            tokens: req_tokens,
            ..Budgets::default()
        }),
        agents: eff_agents,
        workflow: fragment.clone(),
        finalize: None,
    };
    if let Err(errs) = spec::validate(&sub_spec) {
        return deny(
            conn,
            format!("wf_compose fragment is invalid: {}", errs.join("; ")),
        );
    }

    // ── Depth (spec §10.3): parent depth + 1 ≤ max_depth, absolute cap 2. ──
    let new_depth = run_depth(conn, &sender.run_id) + 1;
    let max_depth = (limits.max_depth as i64).min(2);
    if new_depth > max_depth {
        return deny(
            conn,
            format!(
                "wf_compose denied: composition depth {new_depth} exceeds the limit of {max_depth}"
            ),
        );
    }

    // ── Caps escalation (spec §15): the fragment can't grant caps broader than
    //    this stage's children caps, and can't enable compose at max depth. ──
    if let Some(msg) = caps_escalation(&fragment, &allowed_caps, new_depth >= max_depth) {
        return deny(conn, msg);
    }

    // ── max_sub_runs: already-launched sub-runs + queued compose requests. ──
    let launched = subrun_count(conn, &sender.run_id);
    let queued = pending_compose_count(conn, &sender.run_id, &sender.step_id);
    if launched + queued >= limits.max_sub_runs as i64 {
        return deny(
            conn,
            format!(
                "wf_compose denied: already at the max_sub_runs of {} for this stage",
                limits.max_sub_runs
            ),
        );
    }

    // ── Budget-fit (spec §10.3): the slice must fit the parent's remaining budget,
    //    net of slices already reserved by queued (not-yet-launched) composes. ──
    let (eff, ledger) = load_budget(conn, &sender.run_id);
    let (pending_turns, pending_tokens) =
        pending_compose_reservations(conn, &sender.run_id, &sender.step_id);
    let avail_turns = ledger.remaining_turns(&eff) - pending_turns;
    if turns > avail_turns {
        return deny(
            conn,
            format!(
                "wf_compose denied: requested {turns} turns but only {} remain in the run budget",
                avail_turns.max(0)
            ),
        );
    }
    if let (Some(cap_left), Some(req)) = (ledger.remaining_tokens(&eff), req_tokens) {
        let avail = cap_left - pending_tokens;
        if req > avail {
            return deny(
                conn,
                format!(
                    "wf_compose denied: requested {req} tokens but only {} remain in the run budget",
                    avail.max(0)
                ),
            );
        }
    }

    // ── Approved: journal the request and queue the plan for the stage loop. ──
    let block_index = orch_index(&sender.step_id).unwrap_or(0);
    let plan = ComposePlan {
        task,
        fragment,
        agents,
        turns,
        tokens: req_tokens,
        integrate,
        base,
        block_index,
    };
    scheduler::journal_event(
        conn,
        app,
        &sender.run_id,
        event_type::COMPOSE_REQUESTED,
        Some(&sender.step_exec_id),
        &json!({
            "turns": turns,
            "tokens": req_tokens,
            "integrate": if matches!(integrate, Integrate::Merge) { "merge" } else { "none" },
            "depth": new_depth,
        }),
    );
    let msg_id = new_msg_id();
    let body = json!({ "decision": "compose", "plan": plan });
    if let Err(e) = insert_message(
        conn,
        &msg_id,
        &sender.run_id,
        Some(&sender.step_exec_id),
        None,
        "decision",
        &body,
        "queued",
        false,
    ) {
        return (Response::err(id, e.to_string()), Poke::None);
    }
    (Response::ok(id, 0, msg_id, String::new()), Poke::None)
}

/// The composition depth of a run: 0 for a top-level run, +1 per `parent_run_id`
/// hop (spec §10.3). Bounded by the absolute depth cap, so the walk is short.
fn run_depth(conn: &Connection, run_id: &str) -> i64 {
    let mut depth = 0i64;
    let mut cur = run_id.to_string();
    // The absolute cap is 2, so a valid chain is ≤ 3 rows; the guard bounds a
    // corrupt self-referential chain regardless.
    for _ in 0..8 {
        let parent: Option<String> = conn
            .query_row(
                "SELECT parent_run_id FROM wf_run WHERE id = ?1",
                [&cur],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten();
        match parent {
            Some(p) => {
                depth += 1;
                cur = p;
            }
            None => break,
        }
    }
    depth
}

/// Sub-runs already created under `parent_run_id` (any status): they count toward
/// `max_sub_runs` for the life of the run, like `children.max` bounds spawns.
fn subrun_count(conn: &Connection, parent_run_id: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM wf_run WHERE parent_run_id = ?1",
        [parent_run_id],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

/// Queued (not-yet-launched) `compose` decisions for this orchestrate stage —
/// counted like [`spawn_child_count`](super::Decision) so a burst within one turn stays within
/// `max_sub_runs` before the stage loop has drained any of them.
fn pending_compose_count(conn: &Connection, run_id: &str, orch_step_id: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM wf_message m
           JOIN wf_step_exec e ON m.from_step_exec_id = e.id
         WHERE m.run_id = ?1 AND e.step_id = ?2 AND m.kind = 'decision' AND m.status = 'queued'
           AND json_extract(m.body_json, '$.decision') = 'compose'",
        params![run_id, orch_step_id],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

/// The turn/token slices of queued (not-yet-launched) composes for this stage, so
/// the budget-fit check accounts for reservations the stage loop hasn't applied to
/// the ledger yet (§10.3). Launched sub-runs' reservations are already in the
/// ledger's `reserved_*`, so they are not re-counted here.
fn pending_compose_reservations(conn: &Connection, run_id: &str, orch_step_id: &str) -> (i64, i64) {
    conn.prepare(
        "SELECT m.body_json FROM wf_message m
           JOIN wf_step_exec e ON m.from_step_exec_id = e.id
         WHERE m.run_id = ?1 AND e.step_id = ?2 AND m.kind = 'decision' AND m.status = 'queued'
           AND json_extract(m.body_json, '$.decision') = 'compose'",
    )
    .and_then(|mut s| {
        s.query_map(params![run_id, orch_step_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()
    })
    .map(|bodies| {
        bodies.into_iter().fold((0i64, 0i64), |(t, k), b| {
            let plan = serde_json::from_str::<Value>(&b)
                .ok()
                .and_then(|v| v.get("plan").cloned());
            let turns = plan
                .as_ref()
                .and_then(|p| p.get("turns"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let tokens = plan
                .as_ref()
                .and_then(|p| p.get("tokens"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            (t + turns, k + tokens)
        })
    })
    .unwrap_or((0, 0))
}

/// The parent run's frozen budgets and current ledger, for the compose fit-check.
fn load_budget(conn: &Connection, run_id: &str) -> (EffectiveBudgets, Ledger) {
    let (budgets_json, spent_json): (String, String) = conn
        .query_row(
            "SELECT budgets_json, spent_json FROM wf_run WHERE id = ?1",
            [run_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap_or_else(|_| ("{}".into(), "{}".into()));
    let eff: EffectiveBudgets = serde_json::from_str(&budgets_json).unwrap_or_default();
    let spent: Value = serde_json::from_str(&spent_json).unwrap_or_else(|_| json!({}));
    (eff, Ledger::from_json(&spent))
}

/// Reject a fragment that would broaden comms caps beyond the parent block's
/// children caps, or enable `compose` at the maximum depth (spec §15). Returns the
/// first violation message, or `None` if the fragment is within bounds.
fn caps_escalation(fragment: &[Block], allowed: &[CommsCap], at_max_depth: bool) -> Option<String> {
    fn broader(caps: &[CommsCap], allowed: &[CommsCap]) -> Option<CommsCap> {
        caps.iter().find(|c| !allowed.contains(c)).copied()
    }
    fn cap_name(c: CommsCap) -> &'static str {
        match c {
            CommsCap::Report => "report",
            CommsCap::Ask => "ask",
            CommsCap::Notify => "notify",
        }
    }
    for block in fragment {
        match block {
            Block::Step(s) => {
                if let Some(c) = broader(&s.comms, allowed) {
                    return Some(format!(
                        "wf_compose denied: fragment step '{}' declares comms cap '{}' \
                         broader than the stage grants",
                        s.id,
                        cap_name(c)
                    ));
                }
            }
            Block::Parallel(p) => {
                for s in &p.steps {
                    if let Some(c) = broader(&s.comms, allowed) {
                        return Some(format!(
                            "wf_compose denied: fragment step '{}' declares comms cap '{}' \
                             broader than the stage grants",
                            s.id,
                            cap_name(c)
                        ));
                    }
                }
            }
            Block::Loop(l) => {
                if let Some(m) = caps_escalation(&l.body, allowed, at_max_depth) {
                    return Some(m);
                }
            }
            Block::Orchestrate(o) => {
                if let Some(c) = broader(&o.comms, allowed) {
                    return Some(format!(
                        "wf_compose denied: fragment orchestrate '{}' grants children comms \
                         cap '{}' broader than the stage grants",
                        o.agent,
                        cap_name(c)
                    ));
                }
                if at_max_depth && o.compose.is_some() {
                    return Some(format!(
                        "wf_compose denied: fragment orchestrate '{}' enables composition at \
                         the maximum depth",
                        o.agent
                    ));
                }
                for s in &o.body {
                    if let Some(c) = broader(&s.comms, allowed) {
                        return Some(format!(
                            "wf_compose denied: fragment step '{}' declares comms cap '{}' \
                             broader than the stage grants",
                            s.id,
                            cap_name(c)
                        ));
                    }
                }
            }
        }
    }
    None
}

/// The top-level block index an orchestrator `step_id` (`orchestrate-<idx>`) refers
/// to.
fn orch_index(orch_step_id: &str) -> Option<usize> {
    orch_step_id.strip_prefix(ORCH_PREFIX)?.parse().ok()
}

/// Journal a `wf_compose` rejection (spec §10.3): never a silent drop.
fn journal_compose_denied(
    conn: &Connection,
    app: Option<&AppHandle>,
    sender: &Sender,
    reason: &str,
) {
    scheduler::journal_event(
        conn,
        app,
        &sender.run_id,
        event_type::COMPOSE_DENIED,
        Some(&sender.step_exec_id),
        &json!({ "reason": reason }),
    );
}
