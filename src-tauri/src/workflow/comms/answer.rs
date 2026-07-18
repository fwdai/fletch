//! Answering asks (spec §10.2, §10.4, §14): the shared queue-an-answer path plus
//! the orchestrator and human answer entrypoints.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;
use tauri::AppHandle;

use crate::error::{Error, Result};
use crate::workflow::scheduler;
use crate::workflow::types::event_type;

use super::sender::Sender;
use super::{insert_message, new_msg_id};

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
pub(super) fn deliver_orchestrator_answer(
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

/// Persist a human's answer to a paused `question` run and journal it. Does not
/// resume — the caller (`WorkflowService::answer`) does that. `app` is `None`
/// under test.
pub(super) fn deliver_answer(
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
