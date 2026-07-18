//! The orchestrator's inbox (spec §10.1, §10.4): take and render the messages
//! queued for the orchestrator's attention, auto-forward children's and
//! sub-runs' terminal outcomes, and queue engine-authored asks/rejections.

use rusqlite::{params, Connection};
use serde_json::json;
use tauri::AppHandle;

use crate::workflow::now_ms;
use crate::workflow::scheduler;
use crate::workflow::types::{event_type, Message, MessageKind};

use super::{body_str, insert_message, new_msg_id};

/// One message queued for the orchestrator's attention, with the sending child's
/// step id resolved for a readable prompt.
pub(in crate::workflow) struct InboxItem {
    pub from_step_id: String,
    pub message: Message,
}

/// Take the messages queued for the orchestrator (children's `wf_report`s, their
/// `wf_ask`s, and engine lifecycle notices) that it has not yet been shown, in
/// order (spec §10.1). Marks each shown via `delivered_at` — an **ask stays
/// `queued`** (its `status` is what `has_unanswered_ask` reads, so the child keeps
/// deferring until the orchestrator answers), it is just not shown twice.
pub(in crate::workflow) fn take_orchestrator_inbox(
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
pub(in crate::workflow) fn compose_orchestrator_inbox(items: &[InboxItem]) -> String {
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
pub(in crate::workflow) fn forward_lifecycle(
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
pub(in crate::workflow) fn forward_subrun_finished(
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
pub(in crate::workflow) fn queue_engine_ask(conn: &Connection, run_id: &str, orch_exec: &str, question: &str) {
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
pub(in crate::workflow) fn queue_rejection(conn: &Connection, run_id: &str, step_exec_id: &str, note: &str) {
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
