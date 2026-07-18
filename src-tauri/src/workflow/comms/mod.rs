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
use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::error::Result;
use crate::rpc::git::GitDispatcher;
use crate::rpc::{Response, RpcDispatcher, RpcEvent, RpcFuture};

use super::now_ms;
use super::scheduler::WorkflowService;
use super::spec::{CommsCap, Spec};
use super::types::{Message, MessageKind};

mod answer;
mod compose;
mod inbox;
mod route;
mod sender;
#[cfg(test)]
mod tests;

use self::answer::deliver_answer;
use self::route::route;
use self::sender::Poke;

// The public surface consumed from the rest of the `workflow` module tree,
// re-exported so its paths (`comms::X`) are unchanged by the split.
pub(super) use self::compose::{ComposeBase, ComposePlan};
pub(super) use self::inbox::{
    compose_orchestrator_inbox, forward_lifecycle, forward_subrun_finished, queue_engine_ask,
    queue_rejection, take_orchestrator_inbox,
};
pub(super) use self::route::{take_orchestrator_decisions, Decision};
pub(super) use self::sender::orch_step_id;

// ───────────────────────────── caps matrix (pure) ───────────────────────────

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

fn new_msg_id() -> String {
    format!("msg-{}", uuid::Uuid::new_v4())
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
