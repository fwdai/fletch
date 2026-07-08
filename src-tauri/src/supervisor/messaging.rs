//! User-message routing: durable turn capture, live injection, and the
//! follow-up queue drained at turn boundaries.

use std::sync::Arc;
use tauri::{AppHandle, Manager};

use crate::agent::{injection_mode, Agent};
use crate::error::{Error, Result};
use crate::managed_session::ToolUseBehavior;
use crate::message_queue::{decide_delivery, Delivery, PendingMsg};
use crate::workspace::AgentStatus;

use super::events::{emit_task, emit_turn_started};
use super::{transition_active, Supervisor};

impl Supervisor {
    /// Route a user message by the provider's injection mode and the agent's
    /// current state (see `message_queue::decide_delivery`):
    /// - idle, queue empty  → deliver now as a new turn (the original path),
    /// - idle, queue full    → flush the leftovers + this message, coalesced,
    /// - busy, claude live    → inject into the running turn over stdin,
    /// - busy, per-turn / tool-gated → queue for the next turn boundary.
    ///
    /// Returns `true` when the message is *held* for a later turn boundary
    /// rather than delivered now — a busy enqueue, or a flush whose delivery
    /// failed and re-queued it (raced with teardown/respawn). The frontend uses
    /// this to badge the optimistic bubble as "queued" only while it genuinely
    /// is; any variant that actually delivers returns `false`.
    pub fn send_user_message(
        self: Arc<Self>,
        app: &AppHandle,
        agent_id: &str,
        turn_id: &str,
        text: &str,
        attachments: &[String],
        thinking: Option<&str>,
    ) -> Result<bool> {
        let mode = injection_mode(&self.workspace.agent(agent_id)?.provider);
        let busy = self.is_busy(agent_id);
        let tool_gated = self
            .agents
            .lock()
            .get(agent_id)
            .is_some_and(Agent::is_tool_gated);
        let queue_nonempty = !self.message_queue.lock().is_empty(agent_id);

        let msg = PendingMsg {
            turn_id: turn_id.to_string(),
            text: text.to_string(),
            attachments: attachments.to_vec(),
            thinking: thinking.map(str::to_string),
        };

        let delivery = decide_delivery(busy, mode, tool_gated, queue_nonempty);
        // Whether the message is genuinely held for a later boundary. A path
        // that delivers now returns `false`; a still-busy `Enqueue` returns
        // `true`; a flush whose delivery failed and re-queued the follow-ups
        // also returns `true` (they await the next retry boundary).
        let queued = match delivery {
            Delivery::DeliverNow => {
                deliver_as_turn(&self, app, agent_id, &msg)?;
                false
            }
            Delivery::FlushNow => {
                self.message_queue.lock().enqueue(agent_id, msg);
                flush_queued(&self, app, agent_id)?
            }
            Delivery::WriteLive => {
                if let Err(e) = self.inject_live(agent_id, &msg) {
                    // The turn ended (or the pipe broke) in the race window
                    // between the busy check and the write. Deliver as a fresh
                    // turn *now* rather than only re-queueing: the turn-end Idle
                    // drain may already have run against an empty queue, so a
                    // bare re-enqueue would strand the follow-up until the next
                    // user message (CQ3-A).
                    tracing::warn!(error = %e, agent_id, "live inject failed; delivering as a new turn");
                    self.message_queue.lock().enqueue(agent_id, msg);
                    flush_queued(&self, app, agent_id)?
                } else {
                    false
                }
            }
            Delivery::Enqueue => {
                self.message_queue.lock().enqueue(agent_id, msg);
                // Same TOCTOU as WriteLive's fallback (CQ3-B): the turn may
                // have ended between the busy check above and this enqueue, so
                // the turn-end Idle drain already ran against an empty queue.
                // If the agent is no longer busy, flush now rather than let the
                // message sit until the user types again. `flush_queued` drains
                // under the queue lock, so if the drain did win the race this is
                // a harmless no-op — never a double send.
                self.is_busy(agent_id) || flush_queued(&self, app, agent_id)?
            }
        };
        Ok(queued)
    }

    /// Inject a message into the running turn over the managed agent's open
    /// stdin (claude). On success, persist its row so it matches the transcript
    /// record the live message produces (the matcher stays 1→1 per live
    /// message). Returns `Err` if the write fails — the turn ended or the pipe
    /// broke in the race window between the busy check and the write — leaving
    /// the message untouched so the caller can fall back without double-handling
    /// it.
    fn inject_live(&self, agent_id: &str, msg: &PendingMsg) -> Result<()> {
        self.agents
            .lock()
            .get(agent_id)
            .ok_or_else(|| Error::AgentNotFound(agent_id.to_string()))
            .and_then(|a| {
                a.send_user_message(&msg.text, &msg.attachments, msg.thinking.as_deref())
            })?;
        if let Err(e) =
            self.workspace
                .insert_user_turn(agent_id, &msg.turn_id, &msg.text, &msg.attachments)
        {
            tracing::warn!(error = %e, agent_id, "persist live-injected user turn failed");
        }
        Ok(())
    }

    /// Capture the outgoing user turn durably, then deliver it to the agent.
    ///
    /// Order matters: we persist the `session_user_turns` row *before* the agent
    /// send, idempotently on `turn_id`. So the message survives even if delivery
    /// fails (agent not yet spawned → `AgentNotFound`; the frontend resumes and
    /// retries via `sendWhenAgentReady`, reusing the same `turn_id` → one row).
    /// On reload a never-delivered turn renders standalone so the user can retry.
    ///
    /// This row carries Fletch-origin metadata (text + attachments) that the
    /// transcript can't; it lives outside `session_records`, which stays a pure
    /// 1:1 mirror of the agent's jsonl. At turn-end `sync_session_records`
    /// matches the row to its canonical transcript user-message and fills in
    /// `native_id`. It is never rendered as a message when matched (the
    /// transcript renders the turn; this only hangs attachments) — so no
    /// double-render with the optimistic live render.
    fn deliver_user_message(
        &self,
        agent_id: &str,
        turn_id: &str,
        text: &str,
        attachments: &[String],
        thinking: Option<&str>,
    ) -> Result<()> {
        // Durable capture first — independent of whether the agent accepts.
        if let Err(e) = self
            .workspace
            .insert_user_turn(agent_id, turn_id, text, attachments)
        {
            tracing::warn!(error = %e, agent_id, "persist outgoing user turn failed");
        }
        let agents = self.agents.lock();
        let agent = agents
            .get(agent_id)
            .ok_or_else(|| Error::AgentNotFound(agent_id.to_string()))?;
        agent.send_user_message(text, attachments, thinking)?;
        Ok(())
    }

    /// Deliver the user's answer to a held user-input prompt as a control
    /// response, unblocking the paused turn.
    pub fn answer_tool_use(
        &self,
        agent_id: &str,
        request_id: &str,
        updated_input: serde_json::Value,
        behavior: ToolUseBehavior,
        message: Option<String>,
    ) -> Result<()> {
        let agents = self.agents.lock();
        let agent = agents
            .get(agent_id)
            .ok_or_else(|| Error::AgentNotFound(agent_id.to_string()))?;
        agent.answer_tool_use(request_id, updated_input, behavior, message)
    }
}

/// Fire-and-forget handler for the user's first message: persists it
/// as the agent's `task`. No branch is created here — the checkout stays
/// detached until the first push, when the agent names its branch (see
/// `open_pr`/`git_push`).
pub(super) fn on_first_user_message(
    sup: Arc<Supervisor>,
    app: AppHandle,
    agent_id: String,
    text: String,
) {
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        return;
    }
    if trimmed.starts_with('/') {
        return;
    }

    match sup.workspace.set_agent_task_if_empty(&agent_id, &trimmed) {
        Ok(true) => {
            emit_task(&app, &agent_id, trimmed.clone());
        }
        Ok(false) => {} // task already set
        Err(e) => {
            tracing::warn!(error = %e, agent_id = %agent_id, "set_agent_task_if_empty failed");
        }
    }
}

pub(super) fn mark_user_turn_started(
    sup: &Supervisor,
    app: &AppHandle,
    agent_id: &str,
    turn_id: Option<&str>,
) {
    // A new turn is starting, so any prior stop is moot: clear the interrupt
    // flag so this turn's natural completion flushes queued follow-ups.
    sup.interrupted.lock().remove(agent_id);
    if let Some(activity) = sup.activities.lock().get_mut(agent_id) {
        activity.reset_for_new_turn();
    }
    // Stamp the turn's run start with a single timestamp shared by the persisted
    // row and the `turn:started` event, so the live timer and the footer measure
    // from the identical instant. Native PTY turns have no fletch-origin row (no
    // turn_id), so they carry no persisted timing — but still emit the event so
    // their live timer has an anchor.
    let started_at = chrono::Utc::now().timestamp_millis();
    if let Some(turn_id) = turn_id {
        if let Err(e) = sup.workspace.mark_user_turn_started(turn_id, started_at) {
            tracing::warn!(error = %e, agent_id, "stamp user turn start failed");
        }
    }
    emit_turn_started(app, agent_id, started_at);
    transition_active(sup, app, agent_id, AgentStatus::Running);
}

/// Deliver a single message as a fresh turn: persist it durably, hand it to the
/// agent, and mark the turn started. The pre-existing send path, now shared by
/// the direct-send and queue-flush routes.
fn deliver_as_turn(
    sup: &Arc<Supervisor>,
    app: &AppHandle,
    agent_id: &str,
    msg: &PendingMsg,
) -> Result<()> {
    sup.deliver_user_message(
        agent_id,
        &msg.turn_id,
        &msg.text,
        &msg.attachments,
        msg.thinking.as_deref(),
    )?;
    mark_user_turn_started(sup, app, agent_id, Some(&msg.turn_id));
    on_first_user_message(
        sup.clone(),
        app.clone(),
        agent_id.to_string(),
        msg.text.clone(),
    );
    Ok(())
}

/// Coalesce every queued follow-up for an agent into one prompt and deliver it
/// as the next turn. No-op if the queue is empty. Persists a single
/// `session_user_turns` row (the coalesced message's `turn_id`), so the matcher
/// stays 1→1 with the one transcript record the turn produces.
///
/// Returns `true` only when delivery failed and the follow-ups were re-queued
/// (still held for a later boundary); `false` when they were delivered as a
/// turn or the queue was already empty (drained elsewhere). Callers reporting a
/// "queued" state to the frontend key off this so the badge tracks reality.
pub(super) fn flush_queued(sup: &Arc<Supervisor>, app: &AppHandle, agent_id: &str) -> Result<bool> {
    let count = sup.message_queue.lock().len(agent_id);
    let Some(coalesced) = sup.message_queue.lock().drain_coalesced(agent_id) else {
        return Ok(false);
    };
    if count > 1 {
        tracing::debug!(
            agent_id,
            count,
            "flushing coalesced follow-up messages as one turn"
        );
    }
    if let Err(e) = deliver_as_turn(sup, app, agent_id, &coalesced) {
        // Delivery raced with teardown/respawn (e.g. AgentNotFound). Put the
        // follow-ups back rather than dropping them; a later boundary or the
        // post-respawn flush retries. Re-queue at the front to preserve order.
        tracing::warn!(error = %e, agent_id, "flush delivery failed; re-queueing follow-ups");
        sup.message_queue.lock().requeue_front(agent_id, coalesced);
        return Ok(true);
    }
    Ok(false)
}

/// At a turn-end Idle transition, flush any queued follow-up messages as the
/// next turn — but only on a *natural* completion. Order of the guards matters:
///
/// 1. A pending binary-swap respawn owns the flush (and the interrupt check):
///    it tears down and restarts the agent, then flushes once it's ready (see
///    `respawn_agent_for_bin`). Flushing here would race that teardown and
///    `AgentNotFound` could drop the queue. The flag is still set at this point
///    — `transition_active` calls us synchronously right after
///    `drain_pending_bin_respawn`, before its spawned task clears it.
/// 2. A user stop converges on this same Idle (the dying process emits its
///    result), so when the interrupt flag is set we clear it and keep the queue
///    intact (A2-A: stop never auto-sends).
///
/// Spawns the flush because `transition_active` holds only `&Supervisor`, and
/// the delivery needs an owned `Arc` (recovered from Tauri state, like
/// `drain_pending_bin_respawn`).
pub(super) fn drain_message_queue(sup: &Supervisor, app: &AppHandle, agent_id: &str) {
    if sup.respawn_pending.lock().contains(agent_id) {
        return;
    }
    if sup.interrupted.lock().remove(agent_id) {
        return;
    }
    if sup.message_queue.lock().is_empty(agent_id) {
        return;
    }
    let Some(sup_arc) = app
        .try_state::<Arc<Supervisor>>()
        .map(|s| s.inner().clone())
    else {
        return;
    };
    let app = app.clone();
    let agent_id = agent_id.to_string();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = flush_queued(&sup_arc, &app, &agent_id) {
            tracing::warn!(error = %e, agent_id, "flush queued follow-up messages failed");
        }
    });
}

/// If a binary-path change was deferred for this agent because it was
/// mid-turn (see `respawn_agent_for_bin`), now that it's Idle restart it onto
/// the new binary. No-op unless the agent is flagged. We recover the managed
/// `Arc<Supervisor>` from Tauri state because `transition_active` only holds
/// `&Supervisor`, and the respawn needs an owned `Arc` for its spawned task.
pub(super) fn drain_pending_bin_respawn(sup: &Supervisor, app: &AppHandle, agent_id: &str) {
    if !sup.respawn_pending.lock().contains(agent_id) {
        return;
    }
    let Some(sup_arc) = app
        .try_state::<Arc<Supervisor>>()
        .map(|s| s.inner().clone())
    else {
        return;
    };
    let app = app.clone();
    let agent_id = agent_id.to_string();
    tauri::async_runtime::spawn(async move {
        sup_arc.respawn_agent_for_bin(&app, &agent_id).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supervisor::tests::{record_with_status, test_supervisor};

    #[test]
    fn delivery_to_unready_agent_leaves_canonical_store_clean_but_captures_turn() {
        // A freshly spawned agent has a session row but isn't in the live agents
        // map yet (the frontend retries the send until it's ready). A failed
        // delivery must not touch the canonical transcript store — but the
        // outgoing user turn IS captured durably so it isn't lost and can be
        // retried.
        let sup = test_supervisor();
        let mut record = record_with_status("yosemite", AgentStatus::Spawning);
        sup.workspace.add_agent(&mut record).unwrap();

        let err = sup
            .deliver_user_message("yosemite", "turn-1", "hello", &[], None)
            .unwrap_err();
        assert!(matches!(err, Error::AgentNotFound(_)));

        // Canonical store untouched.
        let records = sup.workspace.read_session_records("yosemite").unwrap();
        assert!(
            records.is_empty(),
            "failed delivery must not write the canonical store, got {records:?}",
        );

        // Outgoing turn captured, pending (no transcript yet) → renders standalone.
        let turns = sup.workspace.read_user_turns("yosemite").unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].turn_id, "turn-1");
        assert_eq!(turns[0].text, "hello");
        assert_eq!(turns[0].native_id, None);
    }
}
