//! The routing core (spec §10.1, §10.2): validate one comms op against its
//! sender's caps, persist and journal it, and hand the caller the resulting
//! poke. Free of the run registry and `AppHandle` so the whole matrix is
//! unit-testable against a temp DB.

use rusqlite::{params, Connection};
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::rpc::Response;
use crate::workflow::now_ms;
use crate::workflow::scheduler;
use crate::workflow::spec::Spec;
use crate::workflow::types::event_type;

use super::answer::deliver_orchestrator_answer;
use super::compose::{route_compose, ComposePlan};
use super::sender::{
    live_orchestrator, orchestrate_block, resolve_sender, Poke, Sender, ORCH_PREFIX,
};
use super::{check_cap, insert_message, load_spec, new_msg_id};

/// The validated, persisted, journaled handling of one comms op. Free of the run
/// registry and `AppHandle` so it is unit-testable; the caller performs the
/// `Poke`. `app` is `None` under test.
pub(super) fn route(
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
pub(in crate::workflow) enum Decision {
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
pub(in crate::workflow) fn take_orchestrator_decisions(
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
