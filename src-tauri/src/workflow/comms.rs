//! The comms router (TECH_SPEC §10.1, §10.4): host-brokered messaging between a
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

use super::now_ms;
use super::scheduler::{self, WorkflowService};
use super::spec::{Block, CommsCap, Spec};
use super::types::{event_type, Message, MessageKind};

// ───────────────────────────── caps matrix (pure) ───────────────────────────

/// The cap an RPC op requires, if it is a comms op (spec §10.1).
pub(super) fn cap_for_op(op: &str) -> Option<CommsCap> {
    match op {
        "wf_report" => Some(CommsCap::Report),
        "wf_ask" => Some(CommsCap::Ask),
        "wf_notify" => Some(CommsCap::Notify),
        _ => None,
    }
}

/// Is `op` one this router owns? (Everything else falls through to git.)
pub(super) fn is_comms_op(op: &str) -> bool {
    cap_for_op(op).is_some()
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
    let caps = step_caps(&spec, &step_id)
        .ok_or_else(|| Error::Other(format!("step '{step_id}' not found in run spec")))?;

    Ok(Sender {
        run_id: run_id.to_string(),
        step_exec_id,
        step_id,
        caps,
    })
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
    if let Err(e) = check_cap(op, &sender.caps) {
        // Journaled, never a silent drop (§10.1): the timeline shows the denied
        // attempt.
        journal_denied(conn, app, &sender, op, &e);
        return (Response::err(id, e), Poke::None);
    }

    match op {
        "wf_report" => (route_report(conn, app, &sender, id, args), Poke::None),
        "wf_ask" => route_ask(conn, app, &sender, id, args),
        // `wf_notify` is orchestrator-only; check_cap already rejected it for a
        // step. Reachable only if a future block granted the cap — refuse until
        // S11 wires orchestrator delivery.
        "wf_notify" => (
            Response::err(id, "notify is not available in this workflow yet"),
            Poke::None,
        ),
        other => (
            Response::err(id, format!("unknown op: {other}")),
            Poke::None,
        ),
    }
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
    // No orchestrator in v1: a report is recorded on the timeline and otherwise a
    // no-op (it never replaces the verdict). Marked delivered — nothing to route.
    if let Err(e) = insert_message(
        conn,
        &msg_id,
        &sender.run_id,
        Some(&sender.step_exec_id),
        None,
        "report",
        &body,
        "delivered",
        true,
    ) {
        return Response::err(id, e.to_string());
    }
    journal_routed(conn, app, sender, &msg_id, "report", None);
    Response::ok(id, 0, msg_id, String::new())
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
    // Routed to the human (no orchestrator in v1): to_step_exec_id = NULL.
    if let Err(e) = insert_message(
        conn,
        &msg_id,
        &sender.run_id,
        Some(&sender.step_exec_id),
        None,
        "ask",
        &body,
        "queued",
        false,
    ) {
        return (Response::err(id, e.to_string()), Poke::None);
    }
    journal_routed(conn, app, sender, &msg_id, "ask", None);
    (
        Response::ok(id, 0, msg_id, String::new()),
        Poke::AskQueued {
            run_id: sender.run_id.clone(),
        },
    )
}

/// Persist a human's answer to a paused `question` run and journal it. Does not
/// resume — the caller (`WorkflowService::answer`) does that. `app` is `None`
/// under test.
fn deliver_answer(
    conn: &Connection,
    app: Option<&AppHandle>,
    run_id: &str,
    message_id: &str,
    body: &str,
) -> Result<()> {
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

    let ans_id = new_msg_id();
    let ans_body = json!({ "text": body });
    insert_message(
        conn,
        &ans_id,
        run_id,
        None, // from the human
        asking_exec.as_deref(),
        "answer",
        &ans_body,
        "queued",
        false,
    )
    .map_err(|e| Error::Other(e.to_string()))?;
    conn.execute(
        "UPDATE wf_message SET status = 'answered' WHERE id = ?1",
        [message_id],
    )
    .map_err(|e| Error::Other(e.to_string()))?;

    scheduler::journal_event(
        conn,
        app,
        run_id,
        event_type::MESSAGE_ROUTED,
        asking_exec.as_deref(),
        &json!({
            "message_id": ans_id,
            "kind": "answer",
            "from": Value::Null,
            "to": asking_exec,
        }),
    );
    Ok(())
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
    pub(super) fn answer(&self, run_id: &str, message_id: &str, body: &str) -> Result<()> {
        {
            let conn = self.db.lock();
            deliver_answer(&conn, Some(&self.app), run_id, message_id, body)?;
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
    run_id: String,
    message_id: String,
    body: String,
    service: Svc<'_>,
) -> std::result::Result<(), String> {
    service
        .answer(&run_id, &message_id, &body)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::spec::{AgentSpec, Gate, Step};
    use crate::workflow::types::{MessageKind, MessageStatus};
    use std::collections::BTreeMap;

    // ── caps matrix (spec §10.1) ──────────────────────────────────────────

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
                instructions: None,
                skills: vec![],
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
        deliver_answer(&conn, None, &run, &ask_id, "Postgres").unwrap();

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
        let err = deliver_answer(&conn, None, &run, "nope", "x");
        assert!(err.is_err());
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
        deliver_answer(&conn, None, &run, &ask_id, "yes").unwrap();
        assert!(
            !has_unanswered_ask(&conn, &exec),
            "answered ask is no longer pending"
        );
    }
}
