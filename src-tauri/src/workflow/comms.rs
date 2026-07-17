//! The comms router: host-brokered messaging between a
//! step agent and the workflow. Step agents call the `wf_report` / `wf_ask` /
//! `wf_notify` RPC ops through their private mailbox; this module validates each
//! against the step's declared caps, persists it to `wf_message`, journals it,
//! and — for a `wf_ask` with no orchestrator (v1) — signals the run to pause for
//! the human.
//!
//! **The engine is the only prompter (§10.4).** The router never sends a prompt
//! or touches the message queue directly. It appends to the recipient's inbox
//! (`wf_message`) and pokes the run's driver; the scheduler is the sole prompter,
//! folding pending inbox messages into one engine-composed prompt
//! ([`compose_delivery`]) at a turn boundary. That keeps turn accounting
//! deterministic and gate evaluation on the right turn.
//!
//! The routing/persistence/journaling core is written as free functions taking
//! `Option<&AppHandle>` (mirroring the scheduler's testable seam), so the whole
//! matrix is exercised against a temp DB with no live app. [`WorkflowService`]
//! adds only the two things that need the live registry: poking the driver on an
//! ask, and resuming the run on `wf_answer`.
//!
//! Scope (S10): `report` / `ask` / `notify` + human Q&A. The orchestrator role
//! (`decide` / `compose`, auto-forwarding lifecycle events, routing `ask` to an
//! orchestrator) lands in S11; until then an `ask` with no orchestrator routes to
//! the human, which is the complete, useful v1 behavior.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::error::{Error, Result};
use crate::rpc::git::GitDispatcher;
use crate::rpc::{Response, RpcDispatcher, RpcEvent, RpcFuture};

use std::collections::BTreeMap;

use super::budget::{EffectiveBudgets, Ledger};
use super::now_ms;
use super::scheduler::{self, WorkflowService};
use super::spec::{self, AgentSpec, Block, Budgets, CommsCap, Integrate, Orchestrate, Spec};
use super::types::{event_type, Message, MessageKind};

// ───────────────────────────── caps matrix (pure) ───────────────────────────

/// The step-exec `step_id` prefix of a stage-lived orchestrator (spec §10.2).
/// `orchestrate-<block-index>`: the block has no id of its own, so the engine
/// synthesizes a stable one from its position in the immutable spec. Children of
/// an orchestrate stage resolve their caps from that block (below).
pub(super) const ORCH_PREFIX: &str = "orchestrate-";

/// The cap an RPC op requires, if it is a *cap-gated* comms op (spec §10.1).
/// `wf_decide` / `wf_compose` are gated by the orchestrator *role* (not a cap),
/// so they return `None` here and are checked separately in [`route`].
pub(super) fn cap_for_op(op: &str) -> Option<CommsCap> {
    match op {
        "wf_report" => Some(CommsCap::Report),
        "wf_ask" => Some(CommsCap::Ask),
        "wf_notify" => Some(CommsCap::Notify),
        _ => None,
    }
}

/// Is `op` one this router owns? (Everything else falls through to git.) Includes
/// the orchestrator-only `wf_decide` (spec §10.2) and `wf_compose` (spec §10.3),
/// which are gated by role, not a cap.
pub(super) fn is_comms_op(op: &str) -> bool {
    cap_for_op(op).is_some() || op == "wf_decide" || op == "wf_compose"
}

/// Credentialed publish ops a run-owned agent must never reach (§15): the
/// engine's `wf/`-guarded finalize is the only push path for workflow runs.
pub(super) fn is_publish_op(op: &str) -> bool {
    matches!(op, "git_push" | "open_pr")
}

/// Human-readable verb for a rejection message.
fn op_verb(op: &str) -> &'static str {
    match op {
        "wf_report" => "report",
        "wf_ask" => "ask",
        "wf_notify" => "notify",
        _ => "use that op",
    }
}

/// Validate a comms op against a step's declared caps (spec §10.1). A step with
/// `notify` never exists (spec validation forbids it), so `wf_notify` from a step
/// is always rejected here — the deterministic engine, not the agent, is the
/// authority.
pub(super) fn check_cap(op: &str, caps: &[CommsCap]) -> std::result::Result<(), String> {
    match cap_for_op(op) {
        None => Err(format!("unknown comms op: {op}")),
        Some(cap) if caps.contains(&cap) => Ok(()),
        Some(_) => Err(format!(
            "this step is not permitted to {} (declare `{}` in its comms caps)",
            op_verb(op),
            op_verb(op)
        )),
    }
}

// ───────────────────────────── delivery (pure) ──────────────────────────────

/// Compose the single engine-owned prompt preamble that carries messages
/// delivered to a step while it waited (spec §10.4). Many messages coalesce into
/// one preamble, so one turn covers all of them.
pub(super) fn compose_delivery(msgs: &[Message]) -> String {
    let mut s = String::from(
        "## Messages from the workflow\n\n\
         While this step was waiting, the workflow delivered the following. \
         Read them, then continue your work.\n\n",
    );
    for m in msgs {
        match m.kind {
            MessageKind::Answer => {
                let text = body_str(&m.body, "text");
                s.push_str(&format!("- **Answer to your question:** {text}\n"));
            }
            MessageKind::Notify => {
                let text = body_str(&m.body, "message");
                s.push_str(&format!("- **Notice:** {text}\n"));
            }
            // Only answer/notify are ever queued *for* a step; other kinds are
            // outbound. Ignore defensively rather than render an empty bullet.
            _ => {}
        }
    }
    s
}

fn body_str<'a>(body: &'a Value, key: &str) -> &'a str {
    body.get(key).and_then(|v| v.as_str()).unwrap_or("")
}

// ───────────────────────────── persistence ──────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn insert_message(
    conn: &Connection,
    id: &str,
    run_id: &str,
    from: Option<&str>,
    to: Option<&str>,
    kind: &str,
    body: &Value,
    status: &str,
    delivered: bool,
) -> rusqlite::Result<()> {
    let now = now_ms();
    conn.execute(
        "INSERT INTO wf_message
           (id, run_id, from_step_exec_id, to_step_exec_id, kind, body_json, status,
            created_at, delivered_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            run_id,
            from,
            to,
            kind,
            body.to_string(),
            status,
            now,
            if delivered { Some(now) } else { None },
        ],
    )?;
    Ok(())
}

/// Read (and mark delivered) every message queued for delivery to `step_id`'s
/// attempts — a human's `wf_answer`, later an orchestrator notify. Joins through
/// the recipient step-exec so an answer addressed to an earlier (now abandoned)
/// attempt still reaches the step's fresh attempt. Marking them `delivered` here
/// makes the fold idempotent across a step's attempts (spec §10.4).
pub(super) fn take_pending_deliveries(
    conn: &Connection,
    run_id: &str,
    step_id: &str,
) -> Vec<Message> {
    let msgs: Vec<Message> = conn
        .prepare(
            "SELECT m.* FROM wf_message m
               JOIN wf_step_exec e ON m.to_step_exec_id = e.id
             WHERE e.run_id = ?1 AND e.step_id = ?2
               AND m.status = 'queued' AND m.kind IN ('answer', 'notify')
             ORDER BY m.created_at, m.rowid",
        )
        .and_then(|mut stmt| {
            stmt.query_map(params![run_id, step_id], Message::from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default();

    if !msgs.is_empty() {
        let now = now_ms();
        for m in &msgs {
            let _ = conn.execute(
                "UPDATE wf_message SET status = 'delivered', delivered_at = ?1 WHERE id = ?2",
                params![now, m.id],
            );
        }
    }
    msgs
}

/// Does this attempt have a `wf_ask` still awaiting a human answer? The
/// persisted message is the source of truth (spec §8.4 "event-sourced truth"):
/// the in-memory pending-ask poke is best-effort and can be missed if the RPC
/// op races the driver's wind-down, so the scheduler consults this before acting
/// on a turn's gate outcome — a queued ask always pauses the run `question`
/// rather than letting the gate be acted on (§10.4).
pub(super) fn has_unanswered_ask(conn: &Connection, step_exec_id: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM wf_message
         WHERE from_step_exec_id = ?1 AND kind = 'ask' AND status = 'queued' LIMIT 1",
        [step_exec_id],
        |_| Ok(()),
    )
    .optional()
    .unwrap_or(None)
    .is_some()
}

// ───────────────────────────── sender resolution ────────────────────────────

/// The run/step context of the agent that issued a comms op.
struct Sender {
    run_id: String,
    step_exec_id: String,
    step_id: String,
    caps: Vec<CommsCap>,
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
fn resolve_sender(conn: &Connection, run_id: &str, agent_id: &str) -> Result<Sender> {
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
fn resolve_caps(
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
pub(super) fn orch_step_id(block_index: usize) -> String {
    format!("{ORCH_PREFIX}{block_index}")
}

// ───────────────────────────── routing core (testable) ──────────────────────

/// What the router decided a comms op needs the caller to do next.
enum Poke {
    /// Nothing beyond the response (report / rejection).
    None,
    /// A `wf_ask` to the human was queued for `run_id`: raise its pending-ask
    /// flag so the attempt pauses `question` at turn end (§10.4).
    AskQueued { run_id: String },
}

/// The validated, persisted, journaled handling of one comms op. Free of the run
/// registry and `AppHandle` so it is unit-testable; the caller performs the
/// `Poke`. `app` is `None` under test.
fn route(
    conn: &Connection,
    app: Option<&AppHandle>,
    id: &str,
    run_id: &str,
    agent_id: &str,
    op: &str,
    args: &Value,
) -> (Response, Poke) {
    let sender = match resolve_sender(conn, run_id, agent_id) {
        Ok(s) => s,
        Err(e) => return (Response::err(id, e.to_string()), Poke::None),
    };

    // `wf_decide` is gated by the orchestrator *role*, not a cap (spec §10.2):
    // only the stage-lived orchestrator may issue decisions.
    if op == "wf_decide" {
        if !sender_is_orchestrator(&sender) {
            let e = "wf_decide is available to the orchestrator only".to_string();
            journal_denied(conn, app, &sender, op, &e);
            return (Response::err(id, e), Poke::None);
        }
        return route_decide(conn, app, &sender, id, args);
    }

    // `wf_compose` is likewise orchestrator-only (spec §10.3) and additionally
    // requires the stage to have `compose` enabled — checked in `route_compose`.
    if op == "wf_compose" {
        if !sender_is_orchestrator(&sender) {
            let e = "wf_compose is available to the orchestrator only".to_string();
            journal_denied(conn, app, &sender, op, &e);
            return (Response::err(id, e), Poke::None);
        }
        return route_compose(conn, app, &sender, id, args);
    }

    if let Err(e) = check_cap(op, &sender.caps) {
        // Journaled, never a silent drop (§10.1): the timeline shows the denied
        // attempt.
        journal_denied(conn, app, &sender, op, &e);
        return (Response::err(id, e), Poke::None);
    }

    match op {
        "wf_report" => (route_report(conn, app, &sender, id, args), Poke::None),
        "wf_ask" => route_ask(conn, app, &sender, id, args),
        // `wf_notify` is orchestrator-only — `check_cap` already rejected it for a
        // child (no step is granted `notify`, §5.2), so only the orchestrator
        // reaches here.
        "wf_notify" => (route_notify(conn, app, &sender, id, args), Poke::None),
        other => (
            Response::err(id, format!("unknown op: {other}")),
            Poke::None,
        ),
    }
}

/// Whether the sending attempt is the stage-lived orchestrator (spec §10.2).
fn sender_is_orchestrator(sender: &Sender) -> bool {
    sender.step_id.starts_with(ORCH_PREFIX)
}

fn route_report(
    conn: &Connection,
    app: Option<&AppHandle>,
    sender: &Sender,
    id: &str,
    args: &Value,
) -> Response {
    let note = args.get("note").and_then(|v| v.as_str()).unwrap_or("");
    let status = args
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("progress");
    if !matches!(status, "progress" | "done") {
        return Response::err(id, "wf_report `status` must be \"progress\" or \"done\"");
    }
    let body = json!({ "status": status, "note": note });
    let msg_id = new_msg_id();
    // Forwarded to the orchestrator when a stage is active (spec §10.1: "wf_report
    // only adds color"); otherwise recorded on the timeline and delivered (a
    // report never replaces the verdict). The orchestrator picks queued reports
    // up at its next turn via `take_orchestrator_inbox`.
    let orchestrator = child_orchestrator(conn, sender);
    let (to, msg_status, delivered) = match &orchestrator {
        Some((orch_exec, _)) => (Some(orch_exec.as_str()), "queued", false),
        None => (None, "delivered", true),
    };
    if let Err(e) = insert_message(
        conn,
        &msg_id,
        &sender.run_id,
        Some(&sender.step_exec_id),
        to,
        "report",
        &body,
        msg_status,
        delivered,
    ) {
        return Response::err(id, e.to_string());
    }
    journal_routed(conn, app, sender, &msg_id, "report", to);
    Response::ok(id, 0, msg_id, String::new())
}

/// The active orchestrator `(step_exec_id, step_id)` a *child* sender should route
/// to — `None` when the sender is itself the orchestrator or no stage is active.
fn child_orchestrator(conn: &Connection, sender: &Sender) -> Option<(String, String)> {
    if sender_is_orchestrator(sender) {
        return None;
    }
    live_orchestrator(conn, &sender.run_id)
}

fn route_ask(
    conn: &Connection,
    app: Option<&AppHandle>,
    sender: &Sender,
    id: &str,
    args: &Value,
) -> (Response, Poke) {
    let question = args.get("question").and_then(|v| v.as_str()).unwrap_or("");
    if question.trim().is_empty() {
        return (
            Response::err(id, "wf_ask requires a non-empty `question`"),
            Poke::None,
        );
    }
    let mut body = json!({ "question": question });
    if let Some(options) = args.get("options") {
        body["options"] = options.clone();
    }
    let msg_id = new_msg_id();
    // A child's ask routes to the orchestrator when a stage is active (spec §10.1);
    // otherwise (and for the orchestrator's own ask) it routes to the human, which
    // pauses the run `question` (§10.4). Either way the ask stays `queued` — the
    // sender's attempt sees an unanswered ask and defers its gate; it is marked
    // `answered` when the orchestrator (or human) replies.
    let orchestrator = child_orchestrator(conn, sender);
    let to = orchestrator.as_ref().map(|(exec, _)| exec.as_str());
    if let Err(e) = insert_message(
        conn,
        &msg_id,
        &sender.run_id,
        Some(&sender.step_exec_id),
        to,
        "ask",
        &body,
        "queued",
        false,
    ) {
        return (Response::err(id, e.to_string()), Poke::None);
    }
    journal_routed(conn, app, sender, &msg_id, "ask", to);
    // Routed to the orchestrator: the orchestrate loop polls its inbox, so no
    // human pause. Routed to the human: raise the pending-ask flag so the run
    // pauses `question` at turn end.
    let poke = if orchestrator.is_some() {
        Poke::None
    } else {
        Poke::AskQueued {
            run_id: sender.run_id.clone(),
        }
    };
    (Response::ok(id, 0, msg_id, String::new()), poke)
}

/// `wf_notify` (orchestrator only, spec §10.1): push a notice to one running child
/// (`to: <step-id>`) or all of them (`to: "all-children"`). Each notice is queued
/// to the child's live attempt; the child folds it into its next engine-composed
/// prompt (§10.4). Best-effort — a notice to a child with no live attempt is a
/// no-op, journaled with `to: null`.
fn route_notify(
    conn: &Connection,
    app: Option<&AppHandle>,
    sender: &Sender,
    id: &str,
    args: &Value,
) -> Response {
    let message = args.get("message").and_then(|v| v.as_str()).unwrap_or("");
    if message.trim().is_empty() {
        return Response::err(id, "wf_notify requires a non-empty `message`");
    }
    let to = args.get("to").and_then(|v| v.as_str()).unwrap_or("");
    if to.trim().is_empty() {
        return Response::err(
            id,
            "wf_notify requires `to` (a child step-id or \"all-children\")",
        );
    }
    let recipients = live_children(
        conn,
        &sender.run_id,
        if to == "all-children" { None } else { Some(to) },
    );
    if recipients.is_empty() {
        journal_routed(conn, app, sender, &new_msg_id(), "notify", None);
        return Response::ok(id, 0, String::new(), String::new());
    }
    let body = json!({ "message": message });
    for child_exec in &recipients {
        let msg_id = new_msg_id();
        let _ = insert_message(
            conn,
            &msg_id,
            &sender.run_id,
            Some(&sender.step_exec_id),
            Some(child_exec),
            "notify",
            &body,
            "queued",
            false,
        );
        journal_routed(conn, app, sender, &msg_id, "notify", Some(child_exec));
    }
    Response::ok(id, 0, String::new(), String::new())
}

/// Live (non-terminal) child attempts of the active orchestrate stage — every
/// live step exec that is not the orchestrator, optionally filtered to one
/// `step_id`. The recipients of `wf_notify`.
fn live_children(conn: &Connection, run_id: &str, step_id: Option<&str>) -> Vec<String> {
    let want = step_id.map(str::to_string);
    conn.prepare(
        "SELECT id, step_id FROM wf_step_exec
         WHERE run_id = ?1 AND status IN ('spawning','running','gating')
           AND step_id NOT LIKE 'orchestrate-%'
         ORDER BY rowid",
    )
    .and_then(|mut s| {
        s.query_map([run_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map(|it| {
            it.filter_map(std::result::Result::ok)
                .filter(|(_, sid)| want.as_deref().map(|w| w == sid).unwrap_or(true))
                .map(|(id, _)| id)
                .collect()
        })
    })
    .unwrap_or_default()
}

// ───────────────────────────── decisions (§10.2) ────────────────────────────

/// A decision the orchestrator issued that the *scheduler* must execute (spec
/// §10.2). `answer` and `escalate` are completed in the router (a pure DB write /
/// a human pause), so they are not represented here; these are the ones that need
/// the driver + child JoinSet the orchestrate stage owns.
// `Eq` is intentionally omitted: `Compose` carries a `Vec<Block>` fragment, and
// `Block` is only `PartialEq` (specs aren't totally comparable). `PartialEq` is
// all the tests need.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Decision {
    /// Start another dynamic child (already validated ≤ `children.max`).
    SpawnChild { agent: String, goal: String },
    /// Drop a child that is no longer needed.
    SkipChild { step_id: String, reason: String },
    /// Ask a finished child to try again with guidance.
    RetryChild { step_id: String, guidance: String },
    /// End the stage now (early join, spec §6.6).
    StageDone,
    /// Launch a validated composed sub-run (spec §10.3). The plan is fully
    /// validated (fragment, depth, caps, budget-fit) in [`route_compose`]; the
    /// scheduler only creates and drives it.
    Compose(Box<ComposePlan>),
}

/// Which commit a composed sub-run forks from (spec §10.3).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ComposeBase {
    /// The orchestrate stage's entry HEAD (the parent's current line).
    ParentHead,
    /// The run's original base commit.
    RunBase,
}

/// A validated `wf_compose` request (spec §10.3), normalized into the fields the
/// scheduler needs to create and drive the sub-run. Serialized into the queued
/// `decision` message so a resume rebuilds it exactly.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(super) struct ComposePlan {
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

/// Handle a `wf_decide` op (spec §10.2). Validates the decision, journals it, and
/// either completes it here (`answer` / `escalate`) or persists it as a queued
/// `decision` message the orchestrate stage consumes via
/// [`take_orchestrator_decisions`]. Invalid decisions (unknown step, over
/// `children.max`, missing fields) return a structured error and are journaled —
/// the deterministic engine, not the orchestrator, is the authority.
/// Whether `step_id` is a child the sender's orchestrate stage can address with
/// `skip_child`/`retry_child` (§10.2). Validated against the definition-declared
/// child namespace — a static `body` child id, or a dynamic `<orch>::dyn-<k>` id
/// within the `children.max` bound — not spawned `wf_step_exec` rows: static
/// children create their rows inside their own spawned task, so a DB check would
/// race the still-pending spawn and wrongly reject a valid skip on the
/// orchestrator's opening turn. The orchestrator's own step is never a valid
/// target. `sender_step_id` is the orchestrator's id (`orchestrate-<n>`).
fn orchestrate_child_is_declared(spec: &Spec, sender_step_id: &str, step_id: &str) -> bool {
    if step_id == sender_step_id {
        return false;
    }
    let Some(orch) = orchestrate_block(spec, sender_step_id) else {
        return false;
    };
    if orch.body.iter().any(|s| s.id == step_id) {
        return true;
    }
    if let Some(tmpl) = &orch.children {
        if let Some(k) = step_id
            .strip_prefix(&format!("{sender_step_id}::dyn-"))
            .and_then(|k| k.parse::<u32>().ok())
        {
            return k < tmpl.max;
        }
    }
    false
}

/// Reject a `skip_child`/`retry_child` naming a step outside the orchestrate
/// stage's declared child namespace — returns the structured error to send back,
/// or `None` when the target is valid. Loading the spec fails closed with a
/// descriptive error rather than silently allowing an unknown step through.
fn reject_unknown_child(
    conn: &Connection,
    sender: &Sender,
    decision: &str,
    step_id: &str,
) -> Option<String> {
    match load_spec(conn, &sender.run_id) {
        Some(spec) if orchestrate_child_is_declared(&spec, &sender.step_id, step_id) => None,
        Some(_) => Some(format!(
            "wf_decide {decision}: unknown child step `{step_id}`"
        )),
        None => Some(format!(
            "wf_decide {decision}: cannot resolve run spec to validate `{step_id}`"
        )),
    }
}

fn route_decide(
    conn: &Connection,
    app: Option<&AppHandle>,
    sender: &Sender,
    id: &str,
    args: &Value,
) -> (Response, Poke) {
    let decision = args.get("decision").and_then(|v| v.as_str()).unwrap_or("");
    match decision {
        "answer" => {
            let message_id = args
                .get("message_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let body = args.get("body").and_then(|v| v.as_str()).unwrap_or("");
            if message_id.is_empty() || body.trim().is_empty() {
                return (
                    Response::err(
                        id,
                        "wf_decide answer requires `message_id` and a non-empty `body`",
                    ),
                    Poke::None,
                );
            }
            match deliver_orchestrator_answer(conn, app, sender, message_id, body) {
                Ok(()) => {
                    journal_decision(
                        conn,
                        app,
                        sender,
                        &json!({ "decision": "answer", "message_id": message_id }),
                    );
                    (
                        Response::ok(id, 0, String::new(), String::new()),
                        Poke::None,
                    )
                }
                Err(e) => (Response::err(id, e.to_string()), Poke::None),
            }
        }
        "spawn_child" => route_spawn_child(conn, app, sender, id, args),
        "skip_child" => {
            let step_id = args.get("step_id").and_then(|v| v.as_str()).unwrap_or("");
            if step_id.is_empty() {
                return (
                    Response::err(id, "wf_decide skip_child requires `step_id`"),
                    Poke::None,
                );
            }
            if let Some(err) = reject_unknown_child(conn, sender, "skip_child", step_id) {
                return (Response::err(id, err), Poke::None);
            }
            let reason = args
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            queue_decision(
                conn,
                app,
                sender,
                id,
                json!({ "decision": "skip_child", "step_id": step_id, "reason": reason }),
            )
        }
        "retry_child" => {
            let step_id = args.get("step_id").and_then(|v| v.as_str()).unwrap_or("");
            if step_id.is_empty() {
                return (
                    Response::err(id, "wf_decide retry_child requires `step_id`"),
                    Poke::None,
                );
            }
            if let Some(err) = reject_unknown_child(conn, sender, "retry_child", step_id) {
                return (Response::err(id, err), Poke::None);
            }
            let guidance = args
                .get("guidance")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            queue_decision(
                conn,
                app,
                sender,
                id,
                json!({ "decision": "retry_child", "step_id": step_id, "guidance": guidance }),
            )
        }
        "stage_done" => queue_decision(conn, app, sender, id, json!({ "decision": "stage_done" })),
        "escalate" => {
            let question = args.get("question").and_then(|v| v.as_str()).unwrap_or("");
            if question.trim().is_empty() {
                return (
                    Response::err(id, "wf_decide escalate requires a non-empty `question`"),
                    Poke::None,
                );
            }
            // Escalation is the orchestrator asking the human: queue an ask from
            // the orchestrator (to = NULL) so the orchestrate loop's pending-ask
            // backstop pauses the run `question`, and `wf_answer` delivers back to
            // the orchestrator (§10.4).
            let msg_id = new_msg_id();
            if let Err(e) = insert_message(
                conn,
                &msg_id,
                &sender.run_id,
                Some(&sender.step_exec_id),
                None,
                "ask",
                &json!({ "question": question }),
                "queued",
                false,
            ) {
                return (Response::err(id, e.to_string()), Poke::None);
            }
            journal_decision(conn, app, sender, &json!({ "decision": "escalate" }));
            journal_routed(conn, app, sender, &msg_id, "ask", None);
            (
                Response::ok(id, 0, msg_id, String::new()),
                Poke::AskQueued {
                    run_id: sender.run_id.clone(),
                },
            )
        }
        other => (
            Response::err(id, format!("unknown decision: {other}")),
            Poke::None,
        ),
    }
}

/// Journal a decision and persist it as a queued `decision` message for the
/// orchestrate stage to execute (spec §10.2).
fn queue_decision(
    conn: &Connection,
    app: Option<&AppHandle>,
    sender: &Sender,
    id: &str,
    body: Value,
) -> (Response, Poke) {
    let msg_id = new_msg_id();
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
    journal_decision(conn, app, sender, &body);
    (Response::ok(id, 0, msg_id, String::new()), Poke::None)
}

/// Validate + record a `spawn_child` decision (spec §10.2): the agent must be the
/// block's `children` template agent and the count must stay within
/// `children.max`. Over-max or template-less spawns are denied with a structured
/// error and a `child_spawn_denied` journal entry.
fn route_spawn_child(
    conn: &Connection,
    app: Option<&AppHandle>,
    sender: &Sender,
    id: &str,
    args: &Value,
) -> (Response, Poke) {
    let goal = args.get("goal").and_then(|v| v.as_str()).unwrap_or("");
    if goal.trim().is_empty() {
        return (
            Response::err(id, "wf_decide spawn_child requires a non-empty `goal`"),
            Poke::None,
        );
    }
    let Some(spec) = load_spec(conn, &sender.run_id) else {
        return (Response::err(id, "run spec unavailable"), Poke::None);
    };
    let template = orchestrate_block(&spec, &sender.step_id).and_then(|o| o.children.clone());
    let Some(template) = template else {
        let e = "this stage has no dynamic-child template (spawn_child unavailable)".to_string();
        journal_spawn_denied(conn, app, sender, &e);
        return (Response::err(id, e), Poke::None);
    };
    let agent = args
        .get("agent")
        .and_then(|v| v.as_str())
        .unwrap_or(&template.agent);
    if agent != template.agent {
        let e = format!(
            "spawn_child agent must be the template's child agent '{}'",
            template.agent
        );
        journal_spawn_denied(conn, app, sender, &e);
        return (Response::err(id, e), Poke::None);
    }
    let spawned = spawn_child_count(conn, &sender.run_id, &sender.step_id);
    if spawned >= template.max as i64 {
        let e = format!(
            "spawn_child denied: already at the children.max of {} for this stage",
            template.max
        );
        journal_spawn_denied(conn, app, sender, &e);
        return (Response::err(id, e), Poke::None);
    }
    // Approved: journal the request + approval, then queue the decision for the
    // orchestrate stage to actually spawn.
    scheduler::journal_event(
        conn,
        app,
        &sender.run_id,
        event_type::CHILD_SPAWN_REQUESTED,
        Some(&sender.step_exec_id),
        &json!({ "agent": agent, "goal": goal }),
    );
    scheduler::journal_event(
        conn,
        app,
        &sender.run_id,
        event_type::CHILD_SPAWN_APPROVED,
        Some(&sender.step_exec_id),
        &json!({ "agent": agent }),
    );
    let msg_id = new_msg_id();
    if let Err(e) = insert_message(
        conn,
        &msg_id,
        &sender.run_id,
        Some(&sender.step_exec_id),
        None,
        "decision",
        &json!({ "decision": "spawn_child", "agent": agent, "goal": goal }),
        "queued",
        false,
    ) {
        return (Response::err(id, e.to_string()), Poke::None);
    }
    (Response::ok(id, 0, msg_id, String::new()), Poke::None)
}

/// Validate + record a `wf_compose` request (spec §10.3). Runs the fragment
/// through the full [`spec::validate`], enforces the depth cap, the caps-escalation
/// rules (§15), `max_sub_runs`, and the budget-fit check, then queues a
/// [`Decision::Compose`] the orchestrate stage executes. Every rejection is a
/// structured error plus a `compose_denied` journal entry — the deterministic
/// engine is the authority, never the orchestrator.
fn route_compose(
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
/// counted like [`spawn_child_count`] so a burst within one turn stays within
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

/// How many `spawn_child` decisions this orchestrate *stage* has already had
/// approved. Counted across every orchestrator exec that shares the stage's
/// `step_id` (`orchestrate-<n>`), so a resume — which starts a fresh orchestrator
/// exec — cannot re-grant a whole `children.max` batch. Status-agnostic, so
/// consumed decisions still count; counting persisted decisions (created
/// synchronously under this lock) also keeps a burst within one turn race-free.
fn spawn_child_count(conn: &Connection, run_id: &str, orch_step_id: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM wf_message m
           JOIN wf_step_exec e ON m.from_step_exec_id = e.id
         WHERE m.run_id = ?1 AND e.step_id = ?2 AND m.kind = 'decision'
           AND json_extract(m.body_json, '$.decision') = 'spawn_child'",
        params![run_id, orch_step_id],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

/// Consume the queued `decision` messages the orchestrator issued (spec §10.2),
/// marking them delivered. `answer` / `escalate` are already handled in the
/// router, so only [`Decision`] variants the scheduler executes are returned.
pub(super) fn take_orchestrator_decisions(
    conn: &Connection,
    run_id: &str,
    orch_exec: &str,
) -> Vec<Decision> {
    let rows: Vec<(String, Value)> = conn
        .prepare(
            "SELECT id, body_json FROM wf_message
             WHERE run_id = ?1 AND from_step_exec_id = ?2 AND kind = 'decision'
               AND status = 'queued'
             ORDER BY created_at, rowid",
        )
        .and_then(|mut s| {
            s.query_map(params![run_id, orch_exec], |r| {
                let body: String = r.get(1)?;
                Ok((
                    r.get::<_, String>(0)?,
                    serde_json::from_str(&body).unwrap_or(Value::Null),
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default();

    let mut out = Vec::new();
    for (msg_id, body) in rows {
        let _ = conn.execute(
            "UPDATE wf_message SET status = 'delivered', delivered_at = ?1 WHERE id = ?2",
            params![now_ms(), msg_id],
        );
        let d = body.get("decision").and_then(|v| v.as_str()).unwrap_or("");
        let s = |k: &str| {
            body.get(k)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        match d {
            "spawn_child" => out.push(Decision::SpawnChild {
                agent: s("agent"),
                goal: s("goal"),
            }),
            "skip_child" => out.push(Decision::SkipChild {
                step_id: s("step_id"),
                reason: s("reason"),
            }),
            "retry_child" => out.push(Decision::RetryChild {
                step_id: s("step_id"),
                guidance: s("guidance"),
            }),
            "stage_done" => out.push(Decision::StageDone),
            "compose" => {
                if let Some(plan) = body
                    .get("plan")
                    .cloned()
                    .and_then(|p| serde_json::from_value::<ComposePlan>(p).ok())
                {
                    out.push(Decision::Compose(Box::new(plan)));
                }
            }
            _ => {}
        }
    }
    out
}

/// One message queued for the orchestrator's attention, with the sending child's
/// step id resolved for a readable prompt.
pub(super) struct InboxItem {
    pub from_step_id: String,
    pub message: Message,
}

/// Take the messages queued for the orchestrator (children's `wf_report`s, their
/// `wf_ask`s, and engine lifecycle notices) that it has not yet been shown, in
/// order (spec §10.1). Marks each shown via `delivered_at` — an **ask stays
/// `queued`** (its `status` is what `has_unanswered_ask` reads, so the child keeps
/// deferring until the orchestrator answers), it is just not shown twice.
pub(super) fn take_orchestrator_inbox(
    conn: &Connection,
    run_id: &str,
    orch_exec: &str,
) -> Vec<InboxItem> {
    let items: Vec<InboxItem> = conn
        .prepare(
            "SELECT m.*, e.step_id FROM wf_message m
               JOIN wf_step_exec e ON m.from_step_exec_id = e.id
             WHERE m.run_id = ?1 AND m.to_step_exec_id = ?2
               AND m.kind IN ('report', 'ask') AND m.delivered_at IS NULL
             ORDER BY m.created_at, m.rowid",
        )
        .and_then(|mut stmt| {
            stmt.query_map(params![run_id, orch_exec], |r| {
                let from_step_id: String = r.get("step_id")?;
                Ok(InboxItem {
                    from_step_id,
                    message: Message::from_row(r)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default();

    let now = now_ms();
    for item in &items {
        let _ = conn.execute(
            "UPDATE wf_message SET delivered_at = ?1 WHERE id = ?2",
            params![now, item.message.id],
        );
    }
    items
}

/// Compose the one engine-owned prompt preamble that hands the orchestrator its
/// pending inbox (spec §10.1, §10.4). Asks carry their `message_id` so the
/// orchestrator can answer them with `wf_decide`.
pub(super) fn compose_orchestrator_inbox(items: &[InboxItem]) -> String {
    let mut s = String::from(
        "## Updates from your children\n\n\
         Since your last turn, the following arrived. Act on them, then continue \
         leading the stage.\n\n",
    );
    for item in items {
        let m = &item.message;
        let step = &item.from_step_id;
        match m.kind {
            MessageKind::Ask => {
                let q = body_str(&m.body, "question");
                s.push_str(&format!(
                    "- **`{step}` asks** (answer with message_id `{}`): {q}\n",
                    m.id
                ));
                if let Some(opts) = m.body.get("options").and_then(|v| v.as_array()) {
                    let opts: Vec<String> = opts
                        .iter()
                        .filter_map(|o| o.as_str().map(str::to_string))
                        .collect();
                    if !opts.is_empty() {
                        s.push_str(&format!("    options: {}\n", opts.join(", ")));
                    }
                }
            }
            MessageKind::Report => {
                let note = body_str(&m.body, "note");
                let status = body_str(&m.body, "status");
                if m.body.get("lifecycle").and_then(|v| v.as_bool()) == Some(true) {
                    s.push_str(&format!("- **`{step}` {status}** — {note}\n"));
                } else if status == "done" {
                    s.push_str(&format!("- **`{step}` reports done**: {note}\n"));
                } else {
                    s.push_str(&format!("- **`{step}` reports progress**: {note}\n"));
                }
            }
            _ => {}
        }
    }
    s
}

/// Auto-forward a child's terminal outcome to the orchestrator as a lifecycle
/// report (spec §10.1: a child can never *forget* to report completion). Journaled
/// as a routed message; picked up on the orchestrator's next turn.
pub(super) fn forward_lifecycle(
    conn: &Connection,
    app: Option<&AppHandle>,
    run_id: &str,
    orch_exec: &str,
    child_exec: &str,
    status: &str,
    note: &str,
) {
    let msg_id = new_msg_id();
    let _ = insert_message(
        conn,
        &msg_id,
        run_id,
        Some(child_exec),
        Some(orch_exec),
        "report",
        &json!({ "status": status, "note": note, "lifecycle": true }),
        "queued",
        false,
    );
    scheduler::journal_event(
        conn,
        app,
        run_id,
        event_type::MESSAGE_ROUTED,
        Some(child_exec),
        &json!({ "message_id": msg_id, "kind": "report", "from": child_exec, "to": orch_exec, "lifecycle": true }),
    );
}

/// Auto-forward a composed sub-run's terminal outcome to the orchestrator (spec
/// §10.3: "the orchestrator receives subrun_launched / subrun_finished messages").
/// Delivered as a lifecycle report the orchestrator reads on its next turn.
/// Attributed to the orchestrator's own exec so it resolves through the inbox
/// join (a sub-run has no step exec in the parent run); the note names the
/// sub-run.
pub(super) fn forward_subrun_finished(
    conn: &Connection,
    app: Option<&AppHandle>,
    run_id: &str,
    orch_exec: &str,
    sub_run_id: &str,
    status: &str,
) {
    let msg_id = new_msg_id();
    let note = format!("sub-run `{sub_run_id}` finished ({status})");
    let _ = insert_message(
        conn,
        &msg_id,
        run_id,
        Some(orch_exec),
        Some(orch_exec),
        "report",
        &json!({ "status": status, "note": note, "lifecycle": true, "sub_run_id": sub_run_id }),
        "queued",
        false,
    );
    scheduler::journal_event(
        conn,
        app,
        run_id,
        event_type::MESSAGE_ROUTED,
        Some(orch_exec),
        &json!({ "message_id": msg_id, "kind": "report", "from": orch_exec, "to": orch_exec, "lifecycle": true, "sub_run_id": sub_run_id }),
    );
}

/// Queue an engine-authored `ask` to the human on the orchestrator's behalf
/// (spec §10.4) — used when the orchestrator stalls and the engine escalates, so
/// there is a concrete question `wf_answer` can resolve on resume.
pub(super) fn queue_engine_ask(conn: &Connection, run_id: &str, orch_exec: &str, question: &str) {
    let _ = insert_message(
        conn,
        &new_msg_id(),
        run_id,
        Some(orch_exec),
        None,
        "ask",
        &json!({ "question": question }),
        "queued",
        false,
    );
}

/// Queue a human's approval-gate rejection as a delivery to the gated step, so
/// its next attempt re-prompts with the reviewer's note folded into the prompt —
/// the same coalesced-delivery path a `wf_answer` uses (spec §10.4). Addressed to
/// the (now abandoned) approval exec by id; `take_pending_deliveries` joins on the
/// step id, so it reaches the step's fresh attempt. Modeled as a `notify` so no
/// new message kind is needed.
pub(super) fn queue_rejection(conn: &Connection, run_id: &str, step_exec_id: &str, note: &str) {
    let body = json!({
        "message": format!(
            "A human reviewed your work and requested changes before approving it:\n\n{note}\n\n\
             Address this feedback, update the code, and complete the step again."
        )
    });
    let _ = insert_message(
        conn,
        &new_msg_id(),
        run_id,
        None,
        Some(step_exec_id),
        "notify",
        &body,
        "queued",
        false,
    );
}

/// Queue an `answer` to the asking child, mark the originating `ask` `answered`,
/// and journal the route (§10.4). Shared by the orchestrator (§10.2) and human
/// (§14) answer paths, which differ only in their preconditions and in `from`
/// (the answering step exec, or `None` for a human).
fn answer_ask(
    conn: &Connection,
    app: Option<&AppHandle>,
    run_id: &str,
    ask_message_id: &str,
    asking_exec: Option<&str>,
    from: Option<&str>,
    body: &str,
) -> Result<()> {
    let ans_id = new_msg_id();
    insert_message(
        conn,
        &ans_id,
        run_id,
        from,
        asking_exec,
        "answer",
        &json!({ "text": body }),
        "queued",
        false,
    )
    .map_err(|e| Error::Other(e.to_string()))?;
    conn.execute(
        "UPDATE wf_message SET status = 'answered' WHERE id = ?1",
        [ask_message_id],
    )
    .map_err(|e| Error::Other(e.to_string()))?;
    scheduler::journal_event(
        conn,
        app,
        run_id,
        event_type::MESSAGE_ROUTED,
        asking_exec,
        &json!({ "message_id": ans_id, "kind": "answer", "from": from, "to": asking_exec }),
    );
    Ok(())
}

/// Deliver the orchestrator's answer to a child's ask (spec §10.2). Marks the ask
/// `answered` and queues an `answer` to the asking child, which folds it into its
/// next prompt (§10.4). Rejects an answer to an ask not addressed to this
/// orchestrator or already answered.
fn deliver_orchestrator_answer(
    conn: &Connection,
    app: Option<&AppHandle>,
    sender: &Sender,
    message_id: &str,
    body: &str,
) -> Result<()> {
    let (asking_exec, to_exec, status) = conn
        .query_row(
            "SELECT from_step_exec_id, to_step_exec_id, status FROM wf_message
             WHERE id = ?1 AND run_id = ?2 AND kind = 'ask'",
            params![message_id, sender.run_id],
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|e| Error::Other(e.to_string()))?
        .ok_or_else(|| Error::Other("no such question on this run".into()))?;
    if to_exec.as_deref() != Some(sender.step_exec_id.as_str()) {
        return Err(Error::Other("that question was not routed to you".into()));
    }
    if status != "queued" {
        return Err(Error::Other("that question was already answered".into()));
    }
    answer_ask(
        conn,
        app,
        &sender.run_id,
        message_id,
        asking_exec.as_deref(),
        Some(&sender.step_exec_id),
        body,
    )
}

fn journal_decision(conn: &Connection, app: Option<&AppHandle>, sender: &Sender, payload: &Value) {
    scheduler::journal_event(
        conn,
        app,
        &sender.run_id,
        event_type::DECISION,
        Some(&sender.step_exec_id),
        payload,
    );
}

fn journal_spawn_denied(conn: &Connection, app: Option<&AppHandle>, sender: &Sender, reason: &str) {
    scheduler::journal_event(
        conn,
        app,
        &sender.run_id,
        event_type::CHILD_SPAWN_DENIED,
        Some(&sender.step_exec_id),
        &json!({ "reason": reason }),
    );
}

/// Load a run's launch-frozen spec.
fn load_spec(conn: &Connection, run_id: &str) -> Option<Spec> {
    let spec_json: String = conn
        .query_row(
            "SELECT spec_json FROM wf_run WHERE id = ?1",
            [run_id],
            |r| r.get(0),
        )
        .ok()?;
    serde_json::from_str(&spec_json).ok()
}

/// Persist a human's answer to a paused `question` run and journal it. Does not
/// resume — the caller (`WorkflowService::answer`) does that. `app` is `None`
/// under test.
fn deliver_answer(
    conn: &Connection,
    app: Option<&AppHandle>,
    project_id: &str,
    run_id: &str,
    message_id: &str,
    body: &str,
) -> Result<()> {
    // Scope the answer to the caller's project (mirrors `wf_list_runs`): the run
    // must belong to `project_id`. Not a cross-user boundary — this is a
    // single-user desktop app — but it keeps a confused frontend from answering a
    // run outside the project context it's operating in.
    let proj: Option<String> = conn
        .query_row(
            "SELECT project_id FROM wf_run WHERE id = ?1",
            [run_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| Error::Other(e.to_string()))?;
    if proj.as_deref() != Some(project_id) {
        return Err(Error::Other("run not found in this project".into()));
    }

    let (status, reason) = scheduler::run_status(conn, run_id)?;
    if status != "paused" || reason.as_deref() != Some("question") {
        return Err(Error::Other(format!(
            "run is not awaiting an answer (status: {status})"
        )));
    }

    let (asking_exec, ask_status) = conn
        .query_row(
            "SELECT from_step_exec_id, status FROM wf_message
             WHERE id = ?1 AND run_id = ?2 AND kind = 'ask'",
            params![message_id, run_id],
            |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|e| Error::Other(e.to_string()))?
        .ok_or_else(|| Error::Other("no such question on this run".into()))?;
    if ask_status != "queued" {
        return Err(Error::Other("that question was already answered".into()));
    }

    // `from = None` → the answer is journaled as coming from the human.
    answer_ask(
        conn,
        app,
        run_id,
        message_id,
        asking_exec.as_deref(),
        None,
        body,
    )
}

// ───────────────────────────── service glue ─────────────────────────────────

impl WorkflowService {
    /// Handle one `wf_report` / `wf_ask` / `wf_notify` op from a step agent
    /// (§10.4). Synchronous: validate + persist + journal under the DB lock, then
    /// perform the resulting poke. No prompt is sent here — that is the
    /// scheduler's job.
    pub(super) fn handle_comms_op(
        &self,
        id: &str,
        run_id: &str,
        agent_id: &str,
        op: &str,
        args: &Value,
    ) -> (Response, Vec<RpcEvent>) {
        // Validate + persist + journal under the DB lock, dropping it before we
        // touch the run registry (lock discipline: never a map lock across the
        // DB lock's work).
        let (resp, poke) = {
            let conn = self.db.lock();
            route(&conn, Some(&self.app), id, run_id, agent_id, op, args)
        };
        if let Poke::AskQueued { run_id } = poke {
            // Raise the run's pending-ask flag so the in-flight attempt defers its
            // gate and the run pauses `question` at turn end (§10.4).
            if let Some(flag) = self.runs.lock().get(&run_id).map(|h| h.pending_ask.clone()) {
                flag.store(true, Ordering::SeqCst);
            }
        }
        (resp, Vec::new())
    }

    /// Deliver a human's answer to a paused `question` run and resume it
    /// (`wf_answer`, spec §13). The scheduler folds the answer into the fresh
    /// attempt's prompt on resume.
    pub(super) fn answer(
        &self,
        project_id: &str,
        run_id: &str,
        message_id: &str,
        body: &str,
    ) -> Result<()> {
        {
            let conn = self.db.lock();
            deliver_answer(&conn, Some(&self.app), project_id, run_id, message_id, body)?;
        }
        // Resume: the drive loop starts a fresh attempt for the asking step (the
        // asking attempt was abandoned at the pause).
        self.spawn_drive(run_id.to_string());
        Ok(())
    }
}

// ───────────────────────────── journaling ───────────────────────────────────

fn journal_routed(
    conn: &Connection,
    app: Option<&AppHandle>,
    sender: &Sender,
    message_id: &str,
    kind: &str,
    to: Option<&str>,
) {
    scheduler::journal_event(
        conn,
        app,
        &sender.run_id,
        event_type::MESSAGE_ROUTED,
        Some(&sender.step_exec_id),
        &json!({
            "message_id": message_id,
            "kind": kind,
            "from": sender.step_exec_id,
            "to": to,
        }),
    );
}

fn journal_denied(
    conn: &Connection,
    app: Option<&AppHandle>,
    sender: &Sender,
    op: &str,
    reason: &str,
) {
    scheduler::journal_event(
        conn,
        app,
        &sender.run_id,
        event_type::MESSAGE_ROUTED,
        Some(&sender.step_exec_id),
        &json!({
            "kind": "denied",
            "op": op,
            "from": sender.step_exec_id,
            "reason": reason,
        }),
    );
}

fn new_msg_id() -> String {
    format!("msg-{}", uuid::Uuid::new_v4())
}

// ───────────────────────────── RPC dispatcher ───────────────────────────────

/// The RPC dispatcher for a **run-owned** step agent (spec §10). Comms ops
/// (`wf_*`) route to the [`WorkflowService`]; everything else (git, echo, ping)
/// falls through to the standard [`GitDispatcher`], so a step agent keeps the
/// same local-git surface as any other agent. Constructed in
/// `supervisor::lifecycle` when an agent has an `owner_run_id`.
pub struct WorkflowCommsDispatcher {
    app: AppHandle,
    /// The run that owns this agent (its `owner_run_id`), captured at spawn. Comms
    /// ops resolve the sender by run — `wf_step_exec.agent_id` is stamped only
    /// after the turn, so it can't key resolution during the turn.
    run_id: String,
    agent_id: String,
    git: GitDispatcher,
}

impl WorkflowCommsDispatcher {
    pub fn new(app: AppHandle, run_id: String, agent_id: String, git: GitDispatcher) -> Self {
        Self {
            app,
            run_id,
            agent_id,
            git,
        }
    }
}

impl RpcDispatcher for WorkflowCommsDispatcher {
    fn dispatch<'a>(
        &'a self,
        id: &'a str,
        op: &'a str,
        args: &'a Value,
    ) -> RpcFuture<'a, (Response, Vec<RpcEvent>)> {
        Box::pin(async move {
            if is_comms_op(op) {
                match self.app.try_state::<Arc<WorkflowService>>() {
                    Some(svc) => {
                        svc.inner()
                            .handle_comms_op(id, &self.run_id, &self.agent_id, op, args)
                    }
                    None => (
                        Response::err(id, "workflow service unavailable"),
                        Vec::new(),
                    ),
                }
            } else if is_publish_op(op) {
                // §15: run-owned step agents never publish. The engine's
                // finalize is the only push path (and it is `wf/`-namespace
                // guarded); the plain GitDispatcher would push any branch or
                // open a PR with the host's credentials, so deny these
                // outright rather than fall through. `git_fetch` stays
                // available for base refreshes.
                (
                    Response::err(
                        id,
                        "workflow step agents cannot push or open PRs; the run \
                         publishes its wf/ branch when it finalizes",
                    ),
                    Vec::new(),
                )
            } else {
                self.git.dispatch(id, op, args).await
            }
        })
    }
}

// ───────────────────────────── command (§13) ────────────────────────────────

type Svc<'a> = tauri::State<'a, Arc<WorkflowService>>;

/// Answer a paused `question` and resume the run (spec §13, §10.4).
#[tauri::command]
pub async fn wf_answer(
    project_id: String,
    run_id: String,
    message_id: String,
    body: String,
    service: Svc<'_>,
) -> std::result::Result<(), String> {
    service
        .answer(&project_id, &run_id, &message_id, &body)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::spec::{
        AgentSpec, ChildTemplate, ComposeLimits, Gate, Integrate, Join, Orchestrate, Step,
    };
    use crate::workflow::types::{MessageKind, MessageStatus};
    use std::collections::BTreeMap;

    // ── caps matrix (spec §10.1) ──────────────────────────────────────────

    #[test]
    fn publish_ops_are_denied_for_run_owned_agents() {
        // §15: the dispatcher short-circuits these before the GitDispatcher
        // fallthrough — a step agent can never push or open a PR with host
        // credentials. `git_fetch` (and unknown ops) still fall through.
        assert!(is_publish_op("git_push"));
        assert!(is_publish_op("open_pr"));
        assert!(!is_publish_op("git_fetch"));
        assert!(!is_publish_op("wf_report"));
        assert!(!is_publish_op("echo"));
    }

    #[test]
    fn cap_for_op_maps_the_three_comms_ops() {
        assert_eq!(cap_for_op("wf_report"), Some(CommsCap::Report));
        assert_eq!(cap_for_op("wf_ask"), Some(CommsCap::Ask));
        assert_eq!(cap_for_op("wf_notify"), Some(CommsCap::Notify));
        assert_eq!(cap_for_op("git_push"), None);
        assert!(!is_comms_op("git_push"));
        assert!(is_comms_op("wf_ask"));
    }

    #[test]
    fn check_cap_matrix() {
        assert!(check_cap("wf_report", &[CommsCap::Report]).is_ok());
        assert!(check_cap("wf_report", &[CommsCap::Ask]).is_err());
        assert!(check_cap("wf_ask", &[CommsCap::Ask]).is_ok());
        assert!(check_cap("wf_ask", &[]).is_err());
        // notify is never grantable to a step, so it's always rejected here.
        assert!(check_cap("wf_notify", &[CommsCap::Report, CommsCap::Ask]).is_err());
        assert!(check_cap("wf_notify", &[CommsCap::Notify]).is_ok());
        assert!(check_cap("rm_rf", &[CommsCap::Report]).is_err());
    }

    // ── delivery coalescing (spec §10.4) ──────────────────────────────────

    fn msg(kind: MessageKind, body: Value) -> Message {
        Message {
            id: "m".into(),
            run_id: "r".into(),
            from_step_exec_id: None,
            to_step_exec_id: Some("e".into()),
            kind,
            body,
            status: MessageStatus::Queued,
            created_at: 0,
            delivered_at: None,
        }
    }

    #[test]
    fn compose_delivery_coalesces_many_messages_into_one_preamble() {
        let msgs = vec![
            msg(MessageKind::Answer, json!({ "text": "use Postgres" })),
            msg(MessageKind::Notify, json!({ "message": "slice B landed" })),
        ];
        let p = compose_delivery(&msgs);
        assert_eq!(p.matches("## Messages from the workflow").count(), 1);
        assert!(p.contains("use Postgres"));
        assert!(p.contains("slice B landed"));
    }

    #[test]
    fn compose_delivery_renders_answer_body() {
        let p = compose_delivery(&[msg(MessageKind::Answer, json!({ "text": "yes, ship it" }))]);
        assert!(p.contains("Answer to your question:"));
        assert!(p.contains("yes, ship it"));
    }

    // ── routing over a temp DB (spec §10.1, §10.4) ────────────────────────

    /// A DB with one paused run, one live step attempt, and a spec whose single
    /// step declares `caps`. Returns (db, run_id, step_exec_id).
    fn seed(caps: Vec<CommsCap>) -> (Connection, String, String) {
        let td = tempfile::tempdir().unwrap();
        let db = crate::database::init(td.path()).unwrap();
        // Keep the tempdir alive for the connection's lifetime by leaking it —
        // acceptable in a unit test.
        std::mem::forget(td);
        let conn = Arc::try_unwrap(db).ok().unwrap().into_inner();

        let mut agents = BTreeMap::new();
        agents.insert(
            "a".to_string(),
            AgentSpec {
                base: "claude".into(),
                model: None,
                effort: None,
                instructions: None,
                skills: vec![],
                mcp_servers: vec![],
                custom_agent: None,
            },
        );
        let spec = Spec {
            version: 1,
            name: "demo".into(),
            description: None,
            budgets: None,
            agents,
            workflow: vec![Block::Step(Step {
                id: "s1".into(),
                agent: "a".into(),
                goal: "g".into(),
                gate: Gate::Verdict,
                budgets: None,
                comms: caps,
            })],
            finalize: None,
        };
        let spec_json = serde_json::to_string(&spec).unwrap();
        conn.execute(
            "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                base_sha,status,paused_reason,budgets_json,spent_json,created_at,updated_at)
             VALUES ('run','demo',?1,'t','p','/repo','/rd','wf/x','sha','paused','question','{}','{}',0,0)",
            [spec_json],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode,agent_id)
             VALUES ('exec-1','run','s1',1,0,'running','verdict','agent-1')",
            [],
        )
        .unwrap();
        (conn, "run".to_string(), "exec-1".to_string())
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn report_persists_and_journals() {
        let (conn, _run, exec) = seed(vec![CommsCap::Report]);
        let (resp, poke) = route(
            &conn,
            None,
            "req-1",
            "run",
            "agent-1",
            "wf_report",
            &json!({ "status": "progress", "note": "halfway" }),
        );
        assert!(resp.ok, "{resp:?}");
        assert!(matches!(poke, Poke::None));
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM wf_message WHERE kind='report'"),
            1
        );
        // Journaled as message_routed against the sending attempt.
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM wf_event WHERE type='message_routed'"
            ),
            1
        );
        let se: String = conn
            .query_row(
                "SELECT step_exec_id FROM wf_event WHERE type='message_routed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(se, exec);
    }

    #[test]
    fn report_without_cap_is_rejected_and_journaled_denied() {
        let (conn, _run, _exec) = seed(vec![]); // no caps
        let (resp, poke) = route(
            &conn,
            None,
            "req-1",
            "run",
            "agent-1",
            "wf_report",
            &json!({ "note": "x" }),
        );
        assert!(!resp.ok);
        assert!(matches!(poke, Poke::None));
        // No message persisted, but the denial is journaled — never silent.
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM wf_message"), 0);
        let denied: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM wf_event WHERE type='message_routed'
                 AND json_extract(payload_json,'$.kind')='denied'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(denied, 1);
    }

    #[test]
    fn ask_queues_message_and_reports_poke() {
        let (conn, run, _exec) = seed(vec![CommsCap::Ask]);
        let (resp, poke) = route(
            &conn,
            None,
            "req-1",
            "run",
            "agent-1",
            "wf_ask",
            &json!({ "question": "which db?" }),
        );
        assert!(resp.ok);
        match poke {
            Poke::AskQueued { run_id } => assert_eq!(run_id, run),
            Poke::None => panic!("ask should request a poke"),
        }
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM wf_message WHERE kind='ask' AND status='queued'"
            ),
            1
        );
    }

    #[test]
    fn empty_question_is_rejected() {
        let (conn, _run, _exec) = seed(vec![CommsCap::Ask]);
        let (resp, poke) = route(
            &conn,
            None,
            "req-1",
            "run",
            "agent-1",
            "wf_ask",
            &json!({ "question": "   " }),
        );
        assert!(!resp.ok);
        assert!(matches!(poke, Poke::None));
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM wf_message"), 0);
    }

    #[test]
    fn answer_queues_reply_and_marks_ask_answered() {
        let (conn, run, exec) = seed(vec![CommsCap::Ask]);
        // The step asked a question.
        let (resp, _poke) = route(
            &conn,
            None,
            "req-1",
            "run",
            "agent-1",
            "wf_ask",
            &json!({ "question": "which db?" }),
        );
        let ask_id = resp.stdout.clone().unwrap();

        // The human answers.
        deliver_answer(&conn, None, "p", &run, &ask_id, "Postgres").unwrap();

        // The ask is answered; a queued answer targets the asking attempt.
        let ask_status: String = conn
            .query_row(
                "SELECT status FROM wf_message WHERE id=?1",
                [&ask_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ask_status, "answered");
        let (to, status): (String, String) = conn
            .query_row(
                "SELECT to_step_exec_id, status FROM wf_message WHERE kind='answer'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(to, exec);
        assert_eq!(status, "queued");

        // The queued answer is picked up for the step and coalesced.
        let pending = take_pending_deliveries(&conn, &run, "s1");
        assert_eq!(pending.len(), 1);
        assert!(compose_delivery(&pending).contains("Postgres"));
        // …and marked delivered, so it isn't folded twice.
        assert!(take_pending_deliveries(&conn, &run, "s1").is_empty());
    }

    #[test]
    fn answer_rejects_when_run_not_awaiting() {
        let (conn, run, _exec) = seed(vec![CommsCap::Ask]);
        // No ask outstanding, and we'll flip the run to running.
        conn.execute(
            "UPDATE wf_run SET status='running', paused_reason=NULL WHERE id='run'",
            [],
        )
        .unwrap();
        let err = deliver_answer(&conn, None, "p", &run, "nope", "x");
        assert!(err.is_err());
    }

    #[test]
    fn answer_rejects_a_run_outside_the_project() {
        let (conn, run, _exec) = seed(vec![CommsCap::Ask]);
        let (resp, _) = route(
            &conn,
            None,
            "req-1",
            "run",
            "agent-1",
            "wf_ask",
            &json!({ "question": "q" }),
        );
        let ask_id = resp.stdout.unwrap();
        // The run belongs to project "p"; a caller scoped to another project
        // cannot answer it, and nothing is enqueued.
        let err = deliver_answer(&conn, None, "other", &run, &ask_id, "x");
        assert!(err.is_err(), "answer must be scoped to the run's project");
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM wf_message WHERE kind='answer'"),
            0
        );
        // The correct project succeeds.
        deliver_answer(&conn, None, "p", &run, &ask_id, "x").unwrap();
    }

    #[test]
    fn resolves_sender_before_agent_id_is_stamped() {
        // The scheduler stamps `wf_step_exec.agent_id` only after the turn ends,
        // but comms ops fire *during* the turn. Resolution must work off the
        // run's live attempt while that column is still NULL — otherwise every
        // mid-turn wf_ask/wf_report would fail with "no workflow step".
        let (conn, _run, exec) = seed(vec![CommsCap::Ask]);
        conn.execute(
            "UPDATE wf_step_exec SET agent_id = NULL WHERE id = ?1",
            [&exec],
        )
        .unwrap();
        let (resp, poke) = route(
            &conn,
            None,
            "req-1",
            "run",
            "agent-1",
            "wf_ask",
            &json!({ "question": "which db?" }),
        );
        assert!(
            resp.ok,
            "must resolve by run while agent_id is NULL: {resp:?}"
        );
        assert!(matches!(poke, Poke::AskQueued { .. }));
    }

    #[test]
    fn concurrent_live_attempts_are_not_misattributed() {
        // Parallel comms is unsupported in v1: with two in-flight attempts and no
        // agent_id link yet, the router refuses rather than guess a sender.
        let (conn, _run, _exec) = seed(vec![CommsCap::Ask]);
        conn.execute(
            "UPDATE wf_step_exec SET agent_id = NULL WHERE id = 'exec-1'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode)
             VALUES ('exec-2','run','s1',1,0,'running','verdict')",
            [],
        )
        .unwrap();
        let (resp, _poke) = route(
            &conn,
            None,
            "req-1",
            "run",
            "agent-1",
            "wf_ask",
            &json!({ "question": "q" }),
        );
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("concurrent"));
    }

    #[test]
    fn ask_is_rejected_once_the_exec_has_committed() {
        // The other half of the commit-point serialization (§10.4): if the
        // scheduler finalized the attempt first (its exec is terminal), a late
        // ask from that turn is rejected — never queued against a completed step,
        // so the run can't be left advanced with an orphan question.
        let (conn, _run, exec) = seed(vec![CommsCap::Ask]);
        conn.execute(
            "UPDATE wf_step_exec SET status = 'done' WHERE id = ?1",
            [&exec],
        )
        .unwrap();
        let (resp, poke) = route(
            &conn,
            None,
            "req-1",
            "run",
            "agent-1",
            "wf_ask",
            &json!({ "question": "q" }),
        );
        assert!(!resp.ok, "ask on a committed exec must be rejected");
        assert!(matches!(poke, Poke::None));
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM wf_message"),
            0,
            "no orphan ask is queued"
        );
    }

    #[test]
    fn has_unanswered_ask_tracks_queued_then_answered() {
        let (conn, run, exec) = seed(vec![CommsCap::Ask]);
        assert!(!has_unanswered_ask(&conn, &exec), "no ask yet");
        let (resp, _) = route(
            &conn,
            None,
            "req-1",
            "run",
            "agent-1",
            "wf_ask",
            &json!({ "question": "q" }),
        );
        let ask_id = resp.stdout.unwrap();
        assert!(has_unanswered_ask(&conn, &exec), "queued ask is pending");
        deliver_answer(&conn, None, "p", &run, &ask_id, "yes").unwrap();
        assert!(
            !has_unanswered_ask(&conn, &exec),
            "answered ask is no longer pending"
        );
    }

    // ── orchestrator role + decisions (spec §10.2) ────────────────────────

    /// A running orchestrate stage: an `orchestrate-0` block (agent `orch`,
    /// dynamic `coder` children max 2, child caps `[report, ask]`), one live
    /// orchestrator exec, and one live child exec. Returns (conn, run, orch_exec,
    /// child_exec).
    fn seed_orchestrate() -> (Connection, String, String, String) {
        let td = tempfile::tempdir().unwrap();
        let db = crate::database::init(td.path()).unwrap();
        std::mem::forget(td);
        let conn = Arc::try_unwrap(db).ok().unwrap().into_inner();

        let mut agents = BTreeMap::new();
        for a in ["orch", "coder"] {
            agents.insert(
                a.to_string(),
                AgentSpec {
                    base: "claude".into(),
                    model: None,
                    effort: None,
                    instructions: None,
                    skills: vec![],
                    mcp_servers: vec![],
                    custom_agent: None,
                },
            );
        }
        let spec = Spec {
            version: 1,
            name: "demo".into(),
            description: None,
            budgets: None,
            agents,
            workflow: vec![Block::Orchestrate(Orchestrate {
                agent: "orch".into(),
                goal: "lead".into(),
                children: Some(ChildTemplate {
                    agent: "coder".into(),
                    max: 2,
                }),
                body: vec![],
                join: Join::All,
                integrate: Integrate::None,
                comms: vec![CommsCap::Report, CommsCap::Ask],
                compose: None,
            })],
            finalize: None,
        };
        let spec_json = serde_json::to_string(&spec).unwrap();
        conn.execute(
            "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                base_sha,status,budgets_json,spent_json,created_at,updated_at)
             VALUES ('run','demo',?1,'t','p','/repo','/rd','wf/x','sha','running','{}','{}',0,0)",
            [spec_json],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode,agent_id)
             VALUES ('orch-exec','run','orchestrate-0',1,0,'running','verdict','orch-agent')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode,agent_id)
             VALUES ('child-exec','run','child-1',1,0,'running','verdict','child-agent')",
            [],
        )
        .unwrap();
        (
            conn,
            "run".to_string(),
            "orch-exec".to_string(),
            "child-exec".to_string(),
        )
    }

    #[test]
    fn wf_decide_is_orchestrator_only() {
        let (conn, _run, _orch, _child) = seed_orchestrate();
        let (resp, poke) = route(
            &conn,
            None,
            "r1",
            "run",
            "child-agent",
            "wf_decide",
            &json!({ "decision": "stage_done" }),
        );
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("orchestrator"));
        assert!(matches!(poke, Poke::None));
    }

    #[test]
    fn child_caps_come_from_the_orchestrate_block() {
        let (conn, run, _orch, _child) = seed_orchestrate();
        let spec = load_spec(&conn, &run).unwrap();
        // Child inherits the block's [report, ask]; the orchestrator gets all.
        let child_caps = resolve_caps(&conn, &spec, &run, "child-1").unwrap();
        assert_eq!(child_caps, vec![CommsCap::Report, CommsCap::Ask]);
        let orch_caps = resolve_caps(&conn, &spec, &run, "orchestrate-0").unwrap();
        assert!(
            orch_caps.contains(&CommsCap::Notify),
            "orchestrator gets notify"
        );
    }

    #[test]
    fn child_ask_routes_to_the_orchestrator_not_the_human() {
        let (conn, _run, orch, child) = seed_orchestrate();
        let (resp, poke) = route(
            &conn,
            None,
            "r1",
            "run",
            "child-agent",
            "wf_ask",
            &json!({ "question": "which db?" }),
        );
        assert!(resp.ok, "{resp:?}");
        // No human pause — the orchestrator handles it.
        assert!(
            matches!(poke, Poke::None),
            "child ask must not pause the run"
        );
        let to: String = conn
            .query_row(
                "SELECT to_step_exec_id FROM wf_message WHERE kind='ask'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(to, orch);
        assert!(
            has_unanswered_ask(&conn, &child),
            "child defers until answered"
        );
    }

    #[test]
    fn orchestrator_answers_a_child_ask() {
        let (conn, run, orch, child) = seed_orchestrate();
        // The child asks.
        let (resp, _) = route(
            &conn,
            None,
            "r1",
            "run",
            "child-agent",
            "wf_ask",
            &json!({ "question": "which db?", "options": ["pg", "sqlite"] }),
        );
        let ask_id = resp.stdout.unwrap();

        // The orchestrator sees it in its inbox with the message id to answer.
        let inbox = take_orchestrator_inbox(&conn, &run, &orch);
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].from_step_id, "child-1");
        assert_eq!(inbox[0].message.id, ask_id);
        assert!(compose_orchestrator_inbox(&inbox).contains(&ask_id));

        // The orchestrator answers via wf_decide.
        let (resp2, poke2) = route(
            &conn,
            None,
            "r2",
            "run",
            "orch-agent",
            "wf_decide",
            &json!({ "decision": "answer", "message_id": ask_id, "body": "use Postgres" }),
        );
        assert!(resp2.ok, "{resp2:?}");
        assert!(matches!(poke2, Poke::None));

        // The child is no longer waiting; the answer is queued for its next turn.
        assert!(!has_unanswered_ask(&conn, &child));
        let pending = take_pending_deliveries(&conn, &run, "child-1");
        assert_eq!(pending.len(), 1);
        assert!(compose_delivery(&pending).contains("use Postgres"));

        // The decision is journaled.
        let decided: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM wf_event WHERE type='decision'
                 AND json_extract(payload_json,'$.decision')='answer'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(decided, 1);
    }

    #[test]
    fn spawn_child_is_bounded_by_children_max_and_denials_journal() {
        let (conn, run, orch, _child) = seed_orchestrate(); // children.max = 2
        for i in 0..2 {
            let (resp, _) = route(
                &conn,
                None,
                &format!("s{i}"),
                "run",
                "orch-agent",
                "wf_decide",
                &json!({ "decision": "spawn_child", "agent": "coder", "goal": "a slice" }),
            );
            assert!(resp.ok, "spawn {i} should be approved: {resp:?}");
        }
        // The third exceeds children.max → structured error + child_spawn_denied.
        let (resp3, poke3) = route(
            &conn,
            None,
            "s3",
            "run",
            "orch-agent",
            "wf_decide",
            &json!({ "decision": "spawn_child", "agent": "coder", "goal": "one too many" }),
        );
        assert!(!resp3.ok);
        assert!(resp3.error.unwrap().contains("children.max"));
        assert!(matches!(poke3, Poke::None));
        let denied: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM wf_event WHERE type='child_spawn_denied'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(denied, 1);
        let approved: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM wf_event WHERE type='child_spawn_approved'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(approved, 2);

        // The two approvals are consumable by the scheduler as SpawnChild decisions.
        let decisions = take_orchestrator_decisions(&conn, &run, &orch);
        assert_eq!(decisions.len(), 2);
        assert!(decisions
            .iter()
            .all(|d| matches!(d, Decision::SpawnChild { .. })));
    }

    #[test]
    fn spawn_child_agent_must_match_the_template() {
        let (conn, _run, _orch, _child) = seed_orchestrate();
        let (resp, _) = route(
            &conn,
            None,
            "s1",
            "run",
            "orch-agent",
            "wf_decide",
            &json!({ "decision": "spawn_child", "agent": "orch", "goal": "wrong agent" }),
        );
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("template's child agent"));
    }

    #[test]
    fn skip_and_retry_child_reject_steps_outside_the_declared_namespace() {
        // §10.2: an unknown-step decision must return a structured error, not be
        // queued and silently dropped by the stage. Validated against the
        // definition-declared child namespace (static body ids + `::dyn-<k>`
        // within `children.max`), which is race-free vs. the child's own spawn.
        let (conn, run, orch, _child) = seed_orchestrate(); // body: [], children.max = 2

        // A dynamic child within the template bound is a valid target.
        let (ok, _) = route(
            &conn,
            None,
            "d0",
            "run",
            "orch-agent",
            "wf_decide",
            &json!({ "decision": "retry_child", "step_id": "orchestrate-0::dyn-0", "guidance": "again" }),
        );
        assert!(ok.ok, "a declared dynamic child must be accepted: {ok:?}");

        // A dyn index at/over children.max, an undeclared id, and the
        // orchestrator's own step are all rejected with a structured error.
        for (label, step_id) in [
            ("over-max", "orchestrate-0::dyn-2"),
            ("unknown", "ghost"),
            ("self", "orchestrate-0"),
        ] {
            let (resp, poke) = route(
                &conn,
                None,
                label,
                "run",
                "orch-agent",
                "wf_decide",
                &json!({ "decision": "skip_child", "step_id": step_id, "reason": "x" }),
            );
            assert!(!resp.ok, "{label} ({step_id}) must be rejected");
            assert!(
                resp.error.unwrap().contains("unknown child step"),
                "{label} must be a structured unknown-step error"
            );
            assert!(matches!(poke, Poke::None));
        }

        // Only the one valid decision was queued for the stage to consume.
        let decisions = take_orchestrator_decisions(&conn, &run, &orch);
        assert_eq!(decisions.len(), 1);
        assert!(matches!(decisions[0], Decision::RetryChild { .. }));
    }

    #[test]
    fn notify_is_orchestrator_only_and_reaches_children() {
        let (conn, run, _orch, _child) = seed_orchestrate();
        // The orchestrator notifies the child.
        let (resp, _) = route(
            &conn,
            None,
            "n1",
            "run",
            "orch-agent",
            "wf_notify",
            &json!({ "to": "child-1", "message": "slice B landed" }),
        );
        assert!(resp.ok, "{resp:?}");
        let pending = take_pending_deliveries(&conn, &run, "child-1");
        assert_eq!(pending.len(), 1);
        assert!(compose_delivery(&pending).contains("slice B landed"));

        // A child cannot notify (its caps are [report, ask]).
        let (resp2, _) = route(
            &conn,
            None,
            "n2",
            "run",
            "child-agent",
            "wf_notify",
            &json!({ "to": "all-children", "message": "x" }),
        );
        assert!(!resp2.ok);
    }

    #[test]
    fn lifecycle_is_auto_forwarded_to_the_orchestrator() {
        let (conn, run, orch, child) = seed_orchestrate();
        forward_lifecycle(
            &conn,
            None,
            &run,
            &orch,
            &child,
            "done",
            "child `child-1` finished",
        );
        let inbox = take_orchestrator_inbox(&conn, &run, &orch);
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].from_step_id, "child-1");
        let rendered = compose_orchestrator_inbox(&inbox);
        assert!(
            rendered.contains("child-1") && rendered.contains("finished"),
            "{rendered}"
        );
        // Shown once — not re-delivered on the next turn.
        assert!(take_orchestrator_inbox(&conn, &run, &orch).is_empty());
    }

    #[test]
    fn stage_done_and_escalate_decisions() {
        let (conn, run, orch, _child) = seed_orchestrate();
        // stage_done queues a consumable decision.
        let (resp, _) = route(
            &conn,
            None,
            "d1",
            "run",
            "orch-agent",
            "wf_decide",
            &json!({ "decision": "stage_done" }),
        );
        assert!(resp.ok, "{resp:?}");
        // escalate queues an ask to the human and pauses the run.
        let (resp2, poke2) = route(
            &conn,
            None,
            "d2",
            "run",
            "orch-agent",
            "wf_decide",
            &json!({ "decision": "escalate", "question": "which framework?" }),
        );
        assert!(resp2.ok, "{resp2:?}");
        assert!(
            matches!(poke2, Poke::AskQueued { .. }),
            "escalate pauses for the human"
        );
        // The escalation appears as an unanswered ask from the orchestrator.
        assert!(has_unanswered_ask(&conn, &orch));

        let decisions = take_orchestrator_decisions(&conn, &run, &orch);
        assert_eq!(decisions, vec![Decision::StageDone]);
    }

    #[test]
    fn spawn_limit_persists_across_resume() {
        let (conn, _run, _orch, _child) = seed_orchestrate(); // children.max = 2
        for i in 0..2 {
            let (resp, _) = route(
                &conn,
                None,
                &format!("s{i}"),
                "run",
                "orch-agent",
                "wf_decide",
                &json!({ "decision": "spawn_child", "agent": "coder", "goal": "slice" }),
            );
            assert!(resp.ok, "{resp:?}");
        }
        // Resume: the stage gets a fresh orchestrator exec (same `orchestrate-0`
        // step id); the old one is no longer live.
        conn.execute(
            "UPDATE wf_step_exec SET status='abandoned' WHERE id='orch-exec'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode,agent_id)
             VALUES ('orch-exec-2','run','orchestrate-0',2,0,'running','verdict','orch-agent-2')",
            [],
        )
        .unwrap();
        // The resumed orchestrator cannot re-grant a whole new batch — the count is
        // stage-wide, not per-exec.
        let (resp, _) = route(
            &conn,
            None,
            "s3",
            "run",
            "orch-agent-2",
            "wf_decide",
            &json!({ "decision": "spawn_child", "agent": "coder", "goal": "one too many" }),
        );
        assert!(!resp.ok, "spawn limit must persist across resume: {resp:?}");
        assert!(resp.error.unwrap().contains("children.max"));
    }

    // ── dynamic composition, wf_compose (spec §10.3) ──────────────────────

    /// A running orchestrate stage with `compose` enabled. `comms` are the stage's
    /// children caps; `turns` seeds the run budget; `parent` sets `parent_run_id`
    /// (drives the depth check). Returns (conn, orch_exec).
    fn seed_compose(
        limits: Option<ComposeLimits>,
        comms: Vec<CommsCap>,
        turns: i64,
        parent: Option<&str>,
    ) -> (Connection, String) {
        let td = tempfile::tempdir().unwrap();
        let db = crate::database::init(td.path()).unwrap();
        std::mem::forget(td);
        let conn = Arc::try_unwrap(db).ok().unwrap().into_inner();

        let mut agents = BTreeMap::new();
        for a in ["orch", "coder"] {
            agents.insert(
                a.to_string(),
                AgentSpec {
                    base: "claude".into(),
                    model: None,
                    effort: None,
                    instructions: None,
                    skills: vec![],
                    mcp_servers: vec![],
                    custom_agent: None,
                },
            );
        }
        let spec = Spec {
            version: 1,
            name: "demo".into(),
            description: None,
            budgets: None,
            agents,
            workflow: vec![Block::Orchestrate(Orchestrate {
                agent: "orch".into(),
                goal: "lead".into(),
                children: Some(ChildTemplate {
                    agent: "coder".into(),
                    max: 2,
                }),
                body: vec![],
                join: Join::All,
                integrate: Integrate::None,
                comms,
                compose: limits,
            })],
            finalize: None,
        };
        let spec_json = serde_json::to_string(&spec).unwrap();
        let budgets_json = serde_json::to_string(&crate::workflow::budget::EffectiveBudgets {
            turns,
            ..Default::default()
        })
        .unwrap();
        // Satisfy the parent_run_id FK when the run is itself a sub-run.
        if let Some(p) = parent {
            conn.execute(
                "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                    base_sha,status,budgets_json,spent_json,created_at,updated_at)
                 VALUES (?1,'p','{}','t','p','/repo','/rd','wf/p','sha','running','{}','{}',0,0)",
                [p],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO wf_run (id,parent_run_id,name,spec_json,task,project_id,repo_path,run_dir,
                branch,base_sha,status,budgets_json,spent_json,created_at,updated_at)
             VALUES ('run',?1,'demo',?2,'t','p','/repo','/rd','wf/x','sha','running',?3,'{}',0,0)",
            rusqlite::params![parent, spec_json, budgets_json],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode,agent_id)
             VALUES ('orch-exec','run','orchestrate-0',1,0,'running','verdict','orch-agent')",
            [],
        )
        .unwrap();
        (conn, "orch-exec".to_string())
    }

    /// A minimal one-step fragment whose step declares `caps` and uses `agent`.
    fn fragment(caps: Vec<&str>, agent: &str) -> Value {
        json!([{ "step": { "id": "impl", "agent": agent, "goal": "do it", "comms": caps } }])
    }

    fn compose_args(fragment: Value, turns: i64) -> Value {
        json!({
            "task": "a composed slice",
            "fragment": fragment,
            "budgets": { "turns": turns },
            "integrate": "merge",
            "base": "parent-head",
        })
    }

    fn compose(conn: &Connection, agent_id: &str, args: &Value) -> Response {
        route(conn, None, "c1", "run", agent_id, "wf_compose", args).0
    }

    #[test]
    fn wf_compose_is_orchestrator_only() {
        let (conn, _orch) = seed_compose(
            Some(ComposeLimits {
                max_sub_runs: 2,
                max_depth: 2,
            }),
            vec![CommsCap::Report],
            100,
            None,
        );
        // Add a non-orchestrator child exec and send as it. (Its step id must not
        // start with the orchestrate prefix, which marks the orchestrator role.)
        conn.execute(
            "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode,agent_id)
             VALUES ('child-exec','run','child-1',1,0,'running','verdict','child-agent')",
            [],
        )
        .unwrap();
        let resp = compose(
            &conn,
            "child-agent",
            &compose_args(fragment(vec![], "coder"), 10),
        );
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("orchestrator only"));
    }

    #[test]
    fn wf_compose_denied_when_composition_disabled() {
        let (conn, _orch) = seed_compose(None, vec![CommsCap::Report], 100, None);
        let resp = compose(
            &conn,
            "orch-agent",
            &compose_args(fragment(vec![], "coder"), 10),
        );
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("not enabled"));
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM wf_event WHERE type='compose_denied'"
            ),
            1,
            "a rejection is always journaled"
        );
    }

    #[test]
    fn wf_compose_valid_request_queues_a_decision() {
        let (conn, _orch) = seed_compose(
            Some(ComposeLimits {
                max_sub_runs: 2,
                max_depth: 2,
            }),
            vec![CommsCap::Report, CommsCap::Ask],
            100,
            None,
        );
        let resp = compose(
            &conn,
            "orch-agent",
            &compose_args(fragment(vec!["report"], "coder"), 30),
        );
        assert!(resp.ok, "valid compose should be accepted: {resp:?}");
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM wf_message WHERE kind='decision'
                   AND json_extract(body_json,'$.decision')='compose' AND status='queued'"
            ),
            1
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM wf_event WHERE type='compose_requested'"
            ),
            1
        );
        // The scheduler decodes it into a typed Compose decision.
        let decisions = take_orchestrator_decisions(&conn, "run", "orch-exec");
        assert_eq!(decisions.len(), 1);
        assert!(matches!(decisions[0], Decision::Compose(_)));
    }

    #[test]
    fn wf_compose_rejects_over_budget() {
        // Run turn cap is 20; a 50-turn slice can't fit.
        let (conn, _orch) = seed_compose(
            Some(ComposeLimits {
                max_sub_runs: 2,
                max_depth: 2,
            }),
            vec![CommsCap::Report],
            20,
            None,
        );
        let resp = compose(
            &conn,
            "orch-agent",
            &compose_args(fragment(vec![], "coder"), 50),
        );
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("remain in the run budget"));
    }

    #[test]
    fn wf_compose_rejects_over_depth() {
        // The run is itself a sub-run (parent set → depth 1); with max_depth 1 a
        // further sub-run would be depth 2.
        let (conn, _orch) = seed_compose(
            Some(ComposeLimits {
                max_sub_runs: 2,
                max_depth: 1,
            }),
            vec![CommsCap::Report],
            100,
            Some("parent-run"),
        );
        let resp = compose(
            &conn,
            "orch-agent",
            &compose_args(fragment(vec![], "coder"), 10),
        );
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("depth"));
    }

    #[test]
    fn wf_compose_rejects_caps_escalation() {
        // Stage grants children only `report`; a fragment step wanting `ask` is a
        // privilege escalation (spec §15).
        let (conn, _orch) = seed_compose(
            Some(ComposeLimits {
                max_sub_runs: 2,
                max_depth: 2,
            }),
            vec![CommsCap::Report],
            100,
            None,
        );
        let resp = compose(
            &conn,
            "orch-agent",
            &compose_args(fragment(vec!["ask"], "coder"), 10),
        );
        assert!(!resp.ok);
        assert!(resp
            .error
            .unwrap()
            .contains("broader than the stage grants"));
    }

    #[test]
    fn wf_compose_rejects_invalid_fragment() {
        let (conn, _orch) = seed_compose(
            Some(ComposeLimits {
                max_sub_runs: 2,
                max_depth: 2,
            }),
            vec![CommsCap::Report],
            100,
            None,
        );
        // References an agent that isn't in the (inherited) agent map.
        let resp = compose(
            &conn,
            "orch-agent",
            &compose_args(fragment(vec![], "nonexistent"), 10),
        );
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("fragment is invalid"));
    }

    #[test]
    fn wf_compose_rejects_over_max_sub_runs() {
        let (conn, _orch) = seed_compose(
            Some(ComposeLimits {
                max_sub_runs: 1,
                max_depth: 2,
            }),
            vec![CommsCap::Report],
            100,
            None,
        );
        // One sub-run already exists for this parent.
        conn.execute(
            "INSERT INTO wf_run (id,parent_run_id,name,spec_json,task,project_id,repo_path,run_dir,
                branch,base_sha,status,budgets_json,spent_json,created_at,updated_at)
             VALUES ('sub-1','run','s','{}','t','p','/repo','/rd','wf/s','sha','running','{}','{}',0,0)",
            [],
        )
        .unwrap();
        let resp = compose(
            &conn,
            "orch-agent",
            &compose_args(fragment(vec![], "coder"), 10),
        );
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("max_sub_runs"));
    }
}
