//! User-message routing: durable turn capture, live injection, and the
//! follow-up queue drained at turn boundaries.

use std::path::Path;
use std::sync::Arc;

use tokio::sync::OwnedMutexGuard;

use crate::agent::injection_mode;
use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::managed_session::ToolUseBehavior;
use crate::message_queue::{decide_delivery, Delivery, PendingMsg};
use crate::workspace::{AgentRecord, AgentStatus};

use super::events::{emit_task, emit_turn_sent, emit_turn_started};
use super::{transition_active, Supervisor};

impl Supervisor {
    /// Route a user message by the provider's injection mode and the agent's
    /// current state (see `message_queue::decide_delivery`):
    /// - at rest, no process → queue for the revive below to deliver,
    /// - idle, queue empty  → deliver now as a new turn (the original path),
    /// - idle, queue full    → flush the leftovers + this message, coalesced,
    /// - busy, claude live    → inject into the running turn over stdin,
    /// - busy, per-turn / tool-gated → queue for the next turn boundary.
    ///
    /// Returns `true` when the message is *held* for a later turn boundary
    /// rather than delivered now — a busy enqueue, a message for a session at
    /// rest, or a flush whose delivery failed and re-queued it (raced with
    /// teardown/respawn). The frontend uses this to badge the optimistic
    /// bubble as "queued" only while it genuinely is; any variant that
    /// actually delivers returns `false`.
    ///
    /// Async because a turn delivered now is checkpointed first (see
    /// `deliver_as_turn`), and a send that arrives meanwhile waits for that
    /// turn to start before it routes (`Supervisor::delivery_locks`).
    ///
    /// The turn the message opens is open from here, not from its hand-off to
    /// the process (`Supervisor::open_turn`): accepted for an agent at rest,
    /// the message is what its user is waiting on through a relaunch of the
    /// session, the checkpoint and the delivery alike. A message that reaches
    /// no turn and has none coming closes it again before this returns.
    pub async fn send_user_message(
        self: Arc<Self>,
        ctx: &Arc<EngineCtx>,
        agent_id: &str,
        turn_id: &str,
        text: &str,
        attachments: &[String],
    ) -> Result<bool> {
        // Resolve the agent up front — this is the existence check. Do it before
        // adopting attachments: adoption writes into the agent's workspace dir
        // (derived by id), so a stale/invalid id would otherwise move the file
        // into an orphan dir nothing sweeps and then fail the send, losing it.
        let record = self.workspace.agent(agent_id)?;
        // An archived agent has no checkout and no process; one being archived
        // is about to have neither. Refuse now, before the turn is persisted or
        // announced, rather than deliver into a teardown or queue a message the
        // archive is about to drop.
        if record.archive.is_some() {
            return Err(Error::Other("agent is archived".into()));
        }
        // One delivery at a time: wait out any in flight for this agent, so the
        // routing below sees the turn it started (`Supervisor::delivery_locks`).
        // Taken after the existence check, so an unknown id never creates a
        // lock; if an archive took the agent meanwhile, `open_route` refuses.
        let _delivering = self.lock_delivery(agent_id).await;
        // Open for the whole routing below: every arm either delivers now or
        // leaves the message queued, and an archive must see neither half-done
        // (`Supervisor::open_route`).
        let _route = self.open_route(agent_id)?;
        // A turn left open with nothing behind it (no turn running, no
        // message held) is nobody's: this message's turn starts now, not then.
        if !self.is_busy(agent_id) && self.message_queue.lock().is_empty(agent_id) {
            self.open_turns.lock().remove(agent_id);
        }
        self.open_turn(ctx, agent_id);
        let queued = self
            .clone()
            .route_user_message(ctx, &record, turn_id, text, attachments)
            .await;
        // Every arm of the routing holds the message rather than dropping it,
        // but holding only helps if something will eventually come along to
        // deliver it. When no process is live at all there is no such boundary
        // — the session is resting after an app restart, or its process exited
        // when it last went idle — so revive it and flush. Without this the
        // message sits in the queue until the user gives up: no turn ever
        // starts, and each retry only grows the backlog.
        if matches!(queued, Ok(true)) && self.needs_revive(agent_id) {
            self.clone().revive_and_flush(ctx, agent_id);
        } else {
            // Delivered (the agent is busy, and this is a no-op), or held with
            // nothing on its way to deliver it, or refused: an agent left at
            // rest owes no turn.
            self.settle_turn(ctx, agent_id);
        }
        queued
    }

    /// Route an accepted message (`send_user_message`, which holds the agent's
    /// delivery lock and route): deliver it, inject it or queue it, and say
    /// whether it is held for a later boundary.
    async fn route_user_message(
        self: Arc<Self>,
        ctx: &Arc<EngineCtx>,
        record: &AgentRecord,
        turn_id: &str,
        text: &str,
        attachments: &[String],
    ) -> Result<bool> {
        let agent_id = record.id.as_str();
        // Ahead of the routing, so it sees the relaunched process: idle, on a
        // token with the whole margin ahead of the turn.
        self.relaunch_if_login_due(ctx, agent_id).await;
        let mode = injection_mode(&record.provider);

        // Move any pasted attachments out of the app-data staging area into
        // this agent's workspace before the paths are persisted or handed to the
        // agent — a confined agent can't read the staging dir. Dragged/browsed
        // paths pass through unchanged. Runs exactly once per user message: the
        // re-queue/flush paths carry the already-rewritten `PendingMsg`, and a
        // rehydrated pending message was persisted with the workspace path.
        // A box agent's files are also written into its guest, which may boot
        // the box: off the runtime's workers, and an error fails the send.
        let attachments = {
            let (id, paths) = (agent_id.to_string(), attachments.to_vec());
            tokio::task::spawn_blocking(move || crate::attachments::adopt(&id, &paths))
                .await
                .map_err(|e| Error::Other(format!("adopting attachments: {e}")))??
        };
        let attachments = attachments.as_slice();

        let busy = self.is_busy(agent_id);
        // A follow-up that names a granted folder while the relaunch that
        // applies it waits for the turn's end would reach a process that
        // cannot see the folder yet: queue it, like a tool-gated one, and the
        // relaunch's flush delivers it to the new process (or "apply now" ends
        // the turn for it, `apply_changes_now`). Anything else still goes in
        // live — a pending relaunch is no reason to hold plain text back.
        let waits_for_relaunch = self.respawn_pending.lock().contains(agent_id)
            && names_granted_folder(record, attachments);
        let live_closed = waits_for_relaunch
            || self
                .agents
                .lock()
                .get(agent_id)
                .is_some_and(|a| a.is_tool_gated());
        let queue_nonempty = !self.message_queue.lock().is_empty(agent_id);

        let msg = PendingMsg {
            turn_id: turn_id.to_string(),
            text: text.to_string(),
            attachments: attachments.to_vec(),
        };

        // Announce the message to every client before delivering it. The one
        // that sent it already shows an optimistic bubble and dedupes on
        // `turn_id`; the others (a paired phone, or the desktop when the phone
        // sent) would otherwise watch the agent's answer stream in with no
        // prompt above it until the turn-end transcript rebuild. Before
        // delivery, so it lands ahead of the `agent:status` Running flip that
        // delivery raises — a mirroring client renders a turn-opening bubble
        // for `!busy` and must not be told the turn is running first.
        emit_turn_sent(
            ctx.sink.as_ref(),
            agent_id,
            turn_id,
            text,
            attachments,
            busy,
        );

        // Capture the task at send time, not at delivery: a fresh spawn's first
        // prompt is held until the process is up, and every client should read
        // it as the task through that window. Idempotent (`set_agent_task_if_empty`),
        // so `deliver_as_turn`'s own call — kept for the flush and native paths —
        // is a no-op after this one.
        on_first_user_message(
            self.clone(),
            ctx.clone(),
            agent_id.to_string(),
            text.to_string(),
        );

        // No process to take the message and none coming up: it is held for
        // the revive `send_user_message` starts, whose process delivers it as
        // it comes up (`drain_message_queue`), after the turn's checkpoint. A
        // delivery tried here could only fail for want of the process, and
        // only after a checkpoint of its own, whose git in a box agent's
        // checkout boots a stopped box first.
        if !busy && self.needs_revive(agent_id) {
            // A stop from before this message is moot once it is accepted, as
            // at any turn's start (`mark_user_turn_started`); one made while
            // the session comes back still holds it.
            self.interrupted.lock().remove(agent_id);
            // Its row, as a delivery writes it: a client that reads the
            // history meanwhile, or after a revive that fails, still has it.
            if let Err(e) =
                self.workspace
                    .insert_user_turn(agent_id, &msg.turn_id, &msg.text, &msg.attachments)
            {
                tracing::warn!(error = %e, agent_id, "persist outgoing user turn failed");
            }
            self.persist_and_enqueue(agent_id, msg);
            return Ok(true);
        }
        let delivery = decide_delivery(busy, mode, live_closed, queue_nonempty);
        // Whether the message is genuinely held for a later boundary. A path
        // that delivers now returns `false`; a still-busy `Enqueue` returns
        // `true`; a flush whose delivery failed and re-queued the follow-ups
        // also returns `true` (they await the next retry boundary).
        let queued = match delivery {
            Delivery::DeliverNow => {
                if let Err(e) = deliver_as_turn(&self, ctx, agent_id, &msg).await {
                    // We classified the agent idle-and-ready, but a teardown
                    // raced our delivery: an idle agent is torn down under the
                    // `agents` lock while its live status still reads Idle (the
                    // Running flip comes after the busy check), so a concurrent effort/model
                    // respawn can remove it — or kill the process mid-send —
                    // between our `is_busy` check and `live_agent`, surfacing as
                    // AgentNotFound/a send error. Re-queue rather than dropping
                    // the (already-persisted) turn: the respawn's post-restart
                    // flush, or this flush once the restart lands, delivers it
                    // onto the fresh process — which is the intent, since the
                    // message then runs under the new config (CQ3-C). A process
                    // that went for good meanwhile leaves the message to the
                    // revive below, as one that was never there does. The same
                    // re-queue holds a message whose turn start was refused
                    // because the agent went busy outside the delivery lock
                    // while it was checkpointed (see `deliver_as_turn`).
                    tracing::warn!(error = %e, agent_id, "deliver-now failed; re-queueing");
                    self.persist_and_enqueue(agent_id, msg);
                    flush_queued(&self, ctx, agent_id).await?
                } else {
                    false
                }
            }
            Delivery::FlushNow => {
                self.persist_and_enqueue(agent_id, msg);
                flush_queued(&self, ctx, agent_id).await?
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
                    self.persist_and_enqueue(agent_id, msg);
                    flush_queued(&self, ctx, agent_id).await?
                } else {
                    false
                }
            }
            Delivery::Enqueue => {
                self.persist_and_enqueue(agent_id, msg);
                // Same TOCTOU as WriteLive's fallback (CQ3-B): the turn may
                // have ended between the busy check above and this enqueue, so
                // the turn-end Idle drain already ran against an empty queue.
                // If the agent is no longer busy, flush now rather than let the
                // message sit until the user types again. `flush_queued` drains
                // under the queue lock, so if the drain did win the race this is
                // a harmless no-op — never a double send.
                self.is_busy(agent_id) || flush_queued(&self, ctx, agent_id).await?
            }
        };
        Ok(queued)
    }

    /// Whether a *held* message needs its session revived before anything can
    /// deliver it. True only when no live process exists **and** nothing is
    /// already on its way to producing one:
    ///
    /// - a `Spawning` agent is excluded — its own `start_process` drains the
    ///   queue when the process comes up (see `start_process`), and reviving
    ///   underneath it would race that spawn;
    /// - a project mid-deletion is excluded — nothing may be started under it
    ///   (the same guard `respawn_agent_preserving_session` applies), and
    ///   `deliver_as_turn` rejects for that reason too, so the hold there is
    ///   deliberate rather than a missing process.
    ///
    /// A session-preserving respawn briefly presents as "no live process" while
    /// it tears the old one down. Reviving into that window is harmless: the
    /// revive waits on the same `agent_lifecycle` lock the respawn holds, then
    /// finds the agent already restored and returns without spawning.
    pub(super) fn needs_revive(&self, agent_id: &str) -> bool {
        if matches!(self.live_status(agent_id), Some(AgentStatus::Spawning)) {
            return false;
        }
        if self.agents.lock().contains_key(agent_id) {
            return false;
        }
        match self.workspace.agent(agent_id) {
            Ok(record) => !self.deleting_projects.lock().contains(&record.project_id),
            Err(_) => false,
        }
    }

    /// Restart the agent's process in `--resume` mode and deliver whatever is
    /// queued onto it. Fire-and-forget: `send_user_message` has already reported
    /// the message as held, so the caller isn't kept waiting on a spawn — the
    /// `Spawning` status event tells the frontend what's happening and the
    /// flushed turn tells it when the agent picked the message up.
    ///
    /// Safe to fire more than once (a user sending twice in a row): `resume_agent`
    /// serializes on `agent_lifecycle` and no-ops once the agent is live, and
    /// `flush_queued` drains under the queue lock, so a second call finds nothing
    /// left to send rather than double-delivering.
    fn revive_and_flush(self: Arc<Self>, ctx: &Arc<EngineCtx>, agent_id: &str) {
        let ctx = ctx.clone();
        let agent_id = agent_id.to_string();
        crate::host::spawn(async move {
            tracing::info!(
                agent_id,
                "reviving a resting session to deliver a held message"
            );
            if let Err(e) = self.clone().resume_agent(ctx.clone(), &agent_id).await {
                // A launch that failed left the status Error with this reason,
                // so the user sees why; the message stays queued and a later
                // successful revive still delivers it. Its turn is not coming
                // now, which a resume refused before it launched has yet to
                // say.
                tracing::warn!(error = %e, agent_id, "revive for a held message failed");
                self.settle_turn(&ctx, &agent_id);
                return;
            }
            // `start_process` drains the queue itself on the spawn-completion
            // Idle, so this is usually a no-op. It still matters when the revive
            // no-oped because a concurrent respawn had already restored the
            // agent — then nobody else owns this message. Locked for the flush
            // only, never across the spawn (`Supervisor::delivery_locks`).
            let _delivering = self.lock_delivery(&agent_id).await;
            if let Err(e) = flush_queued(&self, &ctx, &agent_id).await {
                tracing::warn!(error = %e, agent_id, "post-revive queue flush failed");
            }
            self.settle_turn(&ctx, &agent_id);
        });
    }

    /// Wait for `agent_id`'s delivery lock (`Supervisor::delivery_locks`), for
    /// a top-level entry point to hold until its turn has started.
    pub(super) async fn lock_delivery(&self, agent_id: &str) -> OwnedMutexGuard<()> {
        let lock = self
            .delivery_locks
            .lock()
            .entry(agent_id.to_string())
            .or_default()
            .clone();
        lock.lock_owned().await
    }

    /// Inject a message into the running turn over the managed agent's open
    /// stdin (claude), with its own row, so it matches the transcript record
    /// the live message produces (the matcher stays 1→1 per live message).
    ///
    /// The row goes in before the write, as for any delivered turn
    /// (`deliver_user_message`): the record the message produces then lies past
    /// the row's watermark whenever it is ingested (`insert_user_turn`).
    /// Returns `Err` if the write fails — the turn ended or the pipe broke in
    /// the race window between the busy check and the write — with the row
    /// withdrawn if this call made it, so the caller can fall back (queue it;
    /// its delivery inserts the row again) without double-handling it or
    /// leaving a turn that never went out.
    fn inject_live(&self, agent_id: &str, msg: &PendingMsg) -> Result<()> {
        // `live_agent` yields `AgentNotFound` when the turn already ended; the
        // send error then propagates so the caller's fallback still fires. Both
        // happen with the `agents` lock released (see `live_agent`). Live
        // injection is claude-only (managed), and claude's model/effort are
        // fixed on its running process — so no per-turn config to pass.
        self.persist_and_send(agent_id, msg, || {
            self.live_agent(agent_id)?
                .send_user_message(&msg.text, &msg.attachments, None, None)
        })?;
        self.reset_native_input(agent_id);
        Ok(())
    }

    /// Write `msg`'s turn row, then hand the message over with `send`. When
    /// the send fails, the row is withdrawn if this call wrote it; one that
    /// was already there (a retry's) stays.
    fn persist_and_send(
        &self,
        agent_id: &str,
        msg: &PendingMsg,
        send: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        let inserted = self
            .workspace
            .insert_user_turn(agent_id, &msg.turn_id, &msg.text, &msg.attachments)
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, agent_id, "persist live-injected user turn failed");
                false
            });
        let sent = send();
        if sent.is_err() && inserted {
            if let Err(e) = self
                .workspace
                .delete_pending_user_turns(std::slice::from_ref(&msg.turn_id))
            {
                tracing::warn!(error = %e, agent_id, "withdraw undelivered user turn failed");
            }
        }
        sent
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
    ) -> Result<()> {
        // Durable capture first — independent of whether the agent accepts.
        if let Err(e) = self
            .workspace
            .insert_user_turn(agent_id, turn_id, text, attachments)
        {
            tracing::warn!(error = %e, agent_id, "persist outgoing user turn failed");
        }
        // Resolve the session's current model/effort from the record at dispatch
        // — the single source of truth. Per-turn runners bake it into this turn's
        // argv; claude (managed) ignores it (config fixed on its process).
        let record = self.workspace.agent(agent_id)?;
        let agent = self.live_agent(agent_id)?;
        agent.send_user_message(
            text,
            attachments,
            record.model.as_deref(),
            record.effort.as_deref(),
        )?;
        self.reset_native_input(agent_id);
        Ok(())
    }

    /// Drop our mirror of a native agent's half-typed input line. A PTY
    /// delivery clears the TUI's real editor before typing, so the mirror has
    /// to be cleared in lockstep or the next Enter would attribute the
    /// abandoned draft to the user's following turn. No-op for every other
    /// transport (only native agents have a tracker entry).
    fn reset_native_input(&self, agent_id: &str) {
        if let Some(tracker) = self.native_inputs.lock().get_mut(agent_id) {
            tracker.clear();
        }
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
        let agent = self.live_agent(agent_id)?;
        agent.answer_tool_use(request_id, updated_input, behavior, message)?;
        // A settled prompt must not come back as a pending card on a client
        // that replays the turn (see `live_turn`).
        if let Some(turn) = self.live_turns.lock().get_mut(agent_id) {
            turn.answered(request_id);
        }
        Ok(())
    }

    /// Enqueue a follow-up both in memory (the live queue) and in the durable
    /// mirror, so it survives a crash/restart. The persist is best-effort: a DB
    /// hiccup is logged but never blocks the in-memory delivery, which is the
    /// hot path. The persisted row is dropped once the message is delivered
    /// (see `flush_queued`) or the agent is torn down (see `detach_runtime`).
    ///
    /// The DB write and the in-memory enqueue are done under one hold of the
    /// queue lock so they land as a unit. Otherwise a concurrent
    /// `flush_queued` cleanup could observe the persisted row (written first)
    /// but not yet the queue entry, and its delete-except-still-queued pass
    /// would delete the row — losing the message on a restart before delivery.
    /// Lock order is always queue → db (`WorkspaceManager` only ever takes the
    /// db lock and never calls back into the queue), so nesting the db lock
    /// under the queue lock here cannot deadlock.
    fn persist_and_enqueue(&self, agent_id: &str, msg: PendingMsg) {
        let mut queue = self.message_queue.lock();
        if let Err(e) = self.workspace.enqueue_pending_message(agent_id, &msg) {
            tracing::warn!(error = %e, agent_id, "persist queued follow-up failed");
        }
        queue.enqueue(agent_id, msg);
    }

    /// Reload queued follow-ups persisted by a previous run into the live
    /// in-memory queue. Called once at startup, before any send. Rehydrated
    /// messages sit idle until the user's next interaction, which routes through
    /// `Delivery::FlushNow` and delivers them coalesced (the existing
    /// idle-with-leftovers path) — no process is auto-spawned here.
    pub fn rehydrate_pending_messages(&self) {
        let pending = match self.workspace.read_all_pending_messages() {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "rehydrate queued follow-ups failed");
                return;
            }
        };
        if pending.is_empty() {
            return;
        }
        let count = pending.len();
        let mut queue = self.message_queue.lock();
        for (agent_id, msg) in pending {
            queue.enqueue(&agent_id, msg);
        }
        tracing::info!(
            count,
            "rehydrated queued follow-up messages from a prior run"
        );
    }
}

/// Fire-and-forget handler for the user's first message: persists it
/// as the agent's `task`. No branch is created here — the checkout stays
/// detached until the first push, when the agent names its branch (see
/// `open_pr`/`git_push`).
pub(super) fn on_first_user_message(
    sup: Arc<Supervisor>,
    ctx: Arc<EngineCtx>,
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
            emit_task(ctx.sink.as_ref(), &agent_id, trimmed.clone());
        }
        Ok(false) => {} // task already set
        Err(e) => {
            tracing::warn!(error = %e, agent_id = %agent_id, "set_agent_task_if_empty failed");
        }
    }
}

pub(super) fn mark_user_turn_started(
    sup: &Supervisor,
    ctx: &Arc<EngineCtx>,
    agent_id: &str,
    turn_id: Option<&str>,
) {
    // A new turn is starting, so any prior stop is moot: clear the interrupt
    // flag so this turn's natural completion flushes queued follow-ups.
    sup.interrupted.lock().remove(agent_id);
    sup.heuristic_idle.lock().remove(agent_id);
    if let Some(activity) = sup.activities.lock().get_mut(agent_id) {
        activity.reset_for_new_turn();
    }
    // Stamp the turn's start with the one timestamp the open turn carries
    // (`Supervisor::open_turn`): when its message was accepted, or now for a
    // turn nothing opened before this. The persisted row, the `turn:started`
    // event and every status share it, so the live timer and the footer
    // measure the identical span, the wait for the process included. Native
    // PTY turns have no fletch-origin row (no turn_id), so they carry no
    // persisted timing — but still emit the event so their live timer has an
    // anchor. Emitted here even for a turn announced when it opened: a client
    // that drops the anchor at the rest of a relaunch takes it up again.
    let started_at = *sup
        .open_turns
        .lock()
        .entry(agent_id.to_string())
        .or_insert_with(|| chrono::Utc::now().timestamp_millis());
    if let Some(turn_id) = turn_id {
        if let Err(e) = sup.workspace.mark_user_turn_started(turn_id, started_at) {
            tracing::warn!(error = %e, agent_id, "stamp user turn start failed");
        }
    }
    emit_turn_started(ctx.sink.as_ref(), agent_id, started_at);
    transition_active(sup, ctx, agent_id, AgentStatus::Running);
}

/// Deliver a single message as a fresh turn: checkpoint the workspace, persist
/// the message durably, hand it to the agent, and mark the turn started. The
/// pre-existing send path, now shared by the direct-send and queue-flush routes.
/// Runs under the caller's delivery lock (`Supervisor::delivery_locks`).
async fn deliver_as_turn(
    sup: &Arc<Supervisor>,
    ctx: &Arc<EngineCtx>,
    agent_id: &str,
    msg: &PendingMsg,
) -> Result<()> {
    let project_id = sup.workspace.agent(agent_id)?.project_id;
    // Pin the code as it stands before the agent sees the prompt. Done ahead of
    // the deletion lock, which can't be held across the snapshot's git I/O, so
    // both checks below see whatever changed while it ran.
    sup.checkpoint_turn(agent_id, &msg.turn_id).await;
    let deletion_guard = sup.deleting_projects.lock();
    if deletion_guard.contains(&project_id) {
        return Err(Error::Other("project deletion is in progress".into()));
    }
    // Never start a turn over a running one. The delivery lock orders every
    // turn Fletch starts, but not one the user types into the native TUI
    // (`write_to_agent`), nor a respawn's Spawning; either can land during the
    // checkpoint above. Checked under the lock the Running flip below happens
    // under; the caller then holds the message for the next turn boundary.
    if sup.is_busy(agent_id) {
        return Err(Error::Other("a turn is already in progress".into()));
    }
    // The previous turn is in `session_records` by now; from here the live
    // buffer holds this one (see `live_turn`). Emptied *before* the message
    // goes out: the agent's first event can land on the event callback before
    // this function gets to mark the turn started, and a clear after delivery
    // would take that event with it. A delivery that fails leaves an empty
    // buffer for an agent that is idle, which the records already cover.
    sup.live_turns
        .lock()
        .entry(agent_id.to_string())
        .or_default()
        .begin_turn();
    // Durable capture ahead of the turn-start stamp below, which updates this
    // row. Idempotent (`INSERT OR IGNORE`): a queued message's earlier row and
    // `deliver_user_message`'s own capture are no-ops after it.
    if let Err(e) =
        sup.workspace
            .insert_user_turn(agent_id, &msg.turn_id, &msg.text, &msg.attachments)
    {
        tracing::warn!(error = %e, agent_id, "persist outgoing user turn failed");
    }
    // The turn opens *before* the message goes out. Its first event can reach
    // the event handler before this function resumes, and the handler reads
    // main-line output under an Idle status as a turn the provider started on
    // its own (`make_event_handler`) — with Running already set here, that
    // reading is exact. A terminal event that beats this function back then
    // closes a turn that is open, rather than one not yet marked, which is what
    // would otherwise leave the agent Running with nothing left to end it.
    let resting = sup.live_status(agent_id);
    mark_user_turn_started(sup, ctx, agent_id, Some(&msg.turn_id));
    if let Err(e) = sup.deliver_user_message(agent_id, &msg.turn_id, &msg.text, &msg.attachments) {
        // Nothing was handed over, so nothing ran: back to rest without the
        // turn-end side effects, for the caller's re-queue to retry against.
        sup.revert_turn_start(ctx, agent_id, &msg.turn_id, resting);
        return Err(e);
    }
    sup.remember_turn(agent_id, msg);
    on_first_user_message(
        sup.clone(),
        ctx.clone(),
        agent_id.to_string(),
        msg.text.clone(),
    );
    drop(deletion_guard);
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
///
/// The caller holds the agent's delivery lock (`Supervisor::delivery_locks`).
pub(super) async fn flush_queued(
    sup: &Arc<Supervisor>,
    ctx: &Arc<EngineCtx>,
    agent_id: &str,
) -> Result<bool> {
    // Open from the pop through delivery and the Running flip, so an archive
    // cannot reserve the agent while a queued message is in hand
    // (`Supervisor::open_route`). Refused when one already holds the agent; the
    // follow-ups stay queued for the archive to dispose of as its trigger allows.
    let _route = sup.open_route(agent_id)?;
    let (coalesced, count) = {
        let mut queue = sup.message_queue.lock();
        let members = queue.turn_ids(agent_id);
        let Some(coalesced) = queue.drain_coalesced(agent_id) else {
            return Ok(false);
        };
        if members.len() > 1 {
            supersede_member_turns(sup, agent_id, &coalesced, &members);
        }
        (coalesced, members.len())
    };
    // The follow-ups' turn is owed from here, where they start on their way,
    // not from the far side of the checkpoint. A no-op for a message held
    // since it was accepted, whose turn has been open since then.
    sup.open_turn(ctx, agent_id);
    if count > 1 {
        tracing::debug!(
            agent_id,
            count,
            "flushing coalesced follow-up messages as one turn"
        );
    }
    if let Err(e) = deliver_as_turn(sup, ctx, agent_id, &coalesced).await {
        // Delivery raced with teardown/respawn (e.g. AgentNotFound). Put the
        // follow-ups back rather than dropping them; a later boundary or the
        // post-respawn flush retries. Re-queue at the front to preserve order.
        // The persisted rows are left intact (we only clear on success), so the
        // retry — or a restart before it — still has them.
        tracing::warn!(error = %e, agent_id, "flush delivery failed; re-queueing follow-ups");
        sup.message_queue.lock().requeue_front(agent_id, coalesced);
        return Ok(true);
    }
    // Delivered as a turn (now durable in `session_user_turns`), so drop the
    // pending rows we just delivered. Keep only what is still queued in memory
    // — a follow-up that arrived during the delivery window — so it survives to
    // its own flush. This also clears any rows coalesced away by a prior failed
    // flush, leaving no orphans behind.
    //
    // Snapshot `keep` and run the delete under a single hold of the queue lock,
    // so a concurrent `persist_and_enqueue` (which writes its row and queue
    // entry under the same lock) is fully ordered before or after this pass —
    // never half-visible. Without the shared lock, a row written but not yet
    // queued would be absent from `keep` and wrongly deleted. Lock order is
    // queue → db throughout (see `persist_and_enqueue`), so this can't deadlock.
    {
        let queue = sup.message_queue.lock();
        let keep = queue.turn_ids(agent_id);
        if let Err(e) = sup
            .workspace
            .delete_pending_messages_except(agent_id, &keep)
        {
            tracing::warn!(error = %e, agent_id, "clear delivered pending follow-ups failed");
        }
    }
    Ok(false)
}

/// Make the coalesced message the one turn its members became. Each member
/// whose earlier delivery failed left a row of its own, but its text now goes
/// out inside the coalesced prompt; left in place, that row would read as an
/// earlier turn and claim the coalesced record its text prefixes. The last
/// member's own row, whose id the coalesced message takes, holds only its own
/// text. So every member's pending row goes and the coalesced row is written
/// fresh, at the drain rather than after delivery: a delivery that then fails
/// re-queues the coalesced message under its own id, and the next flush no
/// longer knows the members to clean up.
///
/// Called under the queue lock (queue → db, see `persist_and_enqueue`).
fn supersede_member_turns(
    sup: &Supervisor,
    agent_id: &str,
    coalesced: &PendingMsg,
    members: &[String],
) {
    if let Err(e) = sup.workspace.delete_pending_user_turns(members) {
        tracing::warn!(error = %e, agent_id, "withdraw coalesced follow-up turns failed");
    }
    if let Err(e) = sup.workspace.insert_user_turn(
        agent_id,
        &coalesced.turn_id,
        &coalesced.text,
        &coalesced.attachments,
    ) {
        tracing::warn!(error = %e, agent_id, "persist coalesced user turn failed");
    }
}

/// Whether `attachments` name one of the folders granted to `record`'s agent
/// (a folder chip sent with the message, `workspace::folder_grants`), spelled
/// as stored or any way that resolves to it: a client that sent the path it
/// asked for, not the canonical one, must not have the message reach a process
/// launched before the folder was.
fn names_granted_folder(record: &AgentRecord, attachments: &[String]) -> bool {
    attachments.iter().any(|a| {
        crate::workspace::folder_grants::find(&record.folder_grants, Path::new(a)).is_some()
    })
}

/// At a turn-end Idle transition, flush any queued follow-up messages as the
/// next turn — but only on a *natural* completion. Order of the guards matters:
///
/// 1. A pending session-preserving respawn owns the flush (and the interrupt
///    check): it tears down and restarts the agent, then flushes once it's
///    ready (see `respawn_agent_preserving_session`). Flushing here would race
///    that teardown and `AgentNotFound` could drop the queue. The flag is still
///    set at this point — `transition_active` calls us synchronously right after
///    `drain_pending_respawn`, before its spawned task clears it.
/// 2. A user stop converges on this same Idle (the dying process emits its
///    result), so when the interrupt flag is set we clear it and keep the queue
///    intact (A2-A: stop never auto-sends).
///
/// Spawns the flush because `transition_active` holds only `&Supervisor`, and
/// the delivery needs an owned `Arc` (recovered from the engine ctx, like
/// `drain_pending_respawn`).
pub(super) fn drain_message_queue(sup: &Supervisor, ctx: &Arc<EngineCtx>, agent_id: &str) {
    if sup.respawn_pending.lock().contains(agent_id) {
        return;
    }
    if sup.interrupted.lock().remove(agent_id) {
        // Stopped while a message was held for a process still coming up: its
        // turn is not coming either.
        sup.settle_turn(ctx, agent_id);
        return;
    }
    let held = !sup.message_queue.lock().is_empty(agent_id);
    // Nothing held and no turn open: the common turn end, with nothing to do.
    if !held && sup.turn_since(agent_id).is_none() {
        return;
    }
    let Some(sup_arc) = ctx.supervisor() else {
        return;
    };
    let ctx = ctx.clone();
    let agent_id = agent_id.to_string();
    if !held {
        // An agent that has come to rest with nothing held owes no turn, yet
        // one is open: left by a send that was refused while the process was
        // still coming up, when it could not settle a busy agent. Closed under
        // the delivery lock, so after whoever holds it now. If that is a send
        // mid-delivery (it relaunched the process from inside itself), it has
        // by then run its turn, queued its message or settled for itself, and
        // the look below reads what it left; if it is anything else that takes
        // the lock (a checkpoint, a rewind, a move), the turn is still closed
        // once it is done.
        crate::host::spawn(async move {
            let _delivering = sup_arc.lock_delivery(&agent_id).await;
            if sup_arc.message_queue.lock().is_empty(&agent_id) {
                sup_arc.settle_turn(&ctx, &agent_id);
            }
        });
        return;
    }
    crate::host::spawn(async move {
        let _delivering = sup_arc.lock_delivery(&agent_id).await;
        if let Err(e) = flush_queued(&sup_arc, &ctx, &agent_id).await {
            tracing::warn!(error = %e, agent_id, "flush queued follow-up messages failed");
        }
        sup_arc.settle_turn(&ctx, &agent_id);
    });
}

/// If a session-preserving respawn (binary swap or a mid-session model/effort
/// change) was deferred for this agent because it was mid-turn (see
/// `respawn_agent_preserving_session`), now that it's Idle restart it so it
/// re-reads the record. No-op unless the agent is flagged. We recover the
/// `Arc<Supervisor>` from the engine ctx because `transition_active` only
/// holds `&Supervisor`, and the respawn needs an owned `Arc` for its spawned
/// task.
pub(super) fn drain_pending_respawn(sup: &Supervisor, ctx: &Arc<EngineCtx>, agent_id: &str) {
    if !sup.respawn_pending.lock().contains(agent_id) {
        return;
    }
    let Some(sup_arc) = ctx.supervisor() else {
        return;
    };
    let ctx = ctx.clone();
    let agent_id = agent_id.to_string();
    crate::host::spawn(async move {
        // Fire-and-forget at the turn boundary: a failed restart is logged and
        // set on the agent's status inside the call.
        let _ = sup_arc
            .respawn_agent_preserving_session(&ctx, &agent_id)
            .await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::error::Error;
    use crate::git::checkpoint;
    use crate::pty_session::{PtySession, PtySpawn};
    use crate::sandbox::KillHandle;
    use crate::supervisor::checkpoints::CAPTURE_TIMEOUT;
    use crate::supervisor::tests::{
        committed_repo, record_in_checkouts, record_with_status, test_supervisor,
    };
    use std::time::Duration;

    /// Only a message that names a granted folder waits for the relaunch that
    /// applies it; plain text, or a file, goes in live as it always did.
    #[test]
    fn only_a_message_naming_a_granted_folder_waits_for_its_relaunch() {
        let mut record = record_with_status("grants", AgentStatus::Running);
        record.folder_grants = vec![crate::workspace::FolderGrant {
            path: "/Users/u/Projects/lib".into(),
            writable: false,
        }];
        assert!(!names_granted_folder(&record, &[]));
        assert!(!names_granted_folder(
            &record,
            &["/tmp/shot.png".to_string()]
        ));
        assert!(names_granted_folder(
            &record,
            &[
                "/tmp/shot.png".to_string(),
                "/Users/u/Projects/lib".to_string()
            ]
        ));
    }

    /// A client that names the folder by the path it picked — a link to the
    /// granted folder — still has its message wait for the relaunch.
    #[test]
    #[cfg(unix)]
    fn a_folder_named_through_a_link_still_waits_for_its_relaunch() {
        let td = tempfile::tempdir().unwrap();
        let real = std::fs::canonicalize(td.path()).unwrap().join("lib");
        std::fs::create_dir(&real).unwrap();
        let link = td.path().join("lib-link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let mut record = record_with_status("grants-link", AgentStatus::Running);
        record.folder_grants = vec![crate::workspace::FolderGrant {
            path: real,
            writable: false,
        }];
        assert!(names_granted_folder(
            &record,
            &[link.to_string_lossy().into_owned()]
        ));
    }

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
            .deliver_user_message("yosemite", "turn-1", "hello", &[])
            .unwrap_err();
        assert!(matches!(err, Error::AgentNotFound(_)));

        // Canonical store untouched.
        let records = sup.workspace.read_session_records("yosemite").unwrap();
        assert!(
            records.is_empty(),
            "failed delivery must not write the canonical store, got {records:?}",
        );

        // Outgoing turn captured, pending (no transcript yet) → renders standalone.
        let turns = sup.workspace.read_history_turns("yosemite").unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].turn_id, "turn-1");
        assert_eq!(turns[0].text, "hello");
        assert_eq!(turns[0].native_id, None);
    }

    /// The regression this guards: after an app restart no process is live and
    /// the supervisor holds no runtime status at all, so a held message has
    /// nobody to deliver it. That state must ask for a revive — otherwise the
    /// message sits queued forever while the composer spins.
    #[test]
    fn a_resting_session_with_no_live_process_asks_for_a_revive() {
        let sup = test_supervisor();
        let mut record = record_with_status("yosemite", AgentStatus::Idle);
        sup.workspace.add_agent(&mut record).unwrap();

        assert!(
            sup.live_status("yosemite").is_none(),
            "no runtime state yet"
        );
        assert!(sup.needs_revive("yosemite"));
    }

    /// The same holds once the agent's process has exited within this run: the
    /// live status lingers at Idle but the `agents` map no longer has a handle.
    #[test]
    fn an_exited_process_still_asks_for_a_revive() {
        let sup = test_supervisor();
        let mut record = record_with_status("yosemite", AgentStatus::Idle);
        sup.workspace.add_agent(&mut record).unwrap();
        sup.statuses
            .lock()
            .insert("yosemite".to_string(), AgentStatus::Idle);

        assert!(sup.needs_revive("yosemite"));
    }

    /// A spawn already in flight owns the delivery — `start_process` drains the
    /// queue when it completes. Reviving here would race that spawn.
    #[test]
    fn a_spawning_agent_does_not_ask_for_a_revive() {
        let sup = test_supervisor();
        let mut record = record_with_status("yosemite", AgentStatus::Spawning);
        sup.workspace.add_agent(&mut record).unwrap();
        sup.statuses
            .lock()
            .insert("yosemite".to_string(), AgentStatus::Spawning);

        assert!(!sup.needs_revive("yosemite"));
    }

    /// Nothing may be started under a project being deleted — the hold there is
    /// deliberate (`deliver_as_turn` rejects for that reason), not a missing
    /// process.
    #[test]
    fn an_agent_in_a_deleting_project_does_not_ask_for_a_revive() {
        let sup = test_supervisor();
        let mut record = record_with_status("yosemite", AgentStatus::Idle);
        sup.workspace.add_agent(&mut record).unwrap();
        sup.deleting_projects
            .lock()
            .insert(sup.workspace.agent("yosemite").unwrap().project_id);

        assert!(!sup.needs_revive("yosemite"));
    }

    /// An unknown agent has nothing to revive.
    #[test]
    fn an_unknown_agent_does_not_ask_for_a_revive() {
        let sup = test_supervisor();
        assert!(!sup.needs_revive("nowhere"));
    }

    const TURN: &str = "8e2f4c6a-1d3b-4a5c-9e7f-0b1d2c3e4f5a";

    fn pending(turn_id: &str) -> PendingMsg {
        PendingMsg {
            turn_id: turn_id.to_string(),
            text: "hello".to_string(),
            attachments: vec![],
        }
    }

    /// An idle agent in a fresh checkout, with no process: delivery gets as
    /// far as handing the prompt over, which fails `AgentNotFound` — so
    /// whatever happened before the hand-off is observable. It has no session
    /// to resume, so the revive a held message asks for fails at once rather
    /// than launching a real agent.
    async fn idle_agent_in(dir: &std::path::Path) -> (Arc<Supervisor>, std::path::PathBuf) {
        let checkout = committed_repo(dir, "repo").await;
        let sup = Arc::new(test_supervisor());
        let mut record = record_in_checkouts(&sup, "yosemite", std::slice::from_ref(&checkout));
        record.session_id = None;
        sup.workspace.add_agent(&mut record).unwrap();
        (sup, checkout)
    }

    /// The `turn:sent` payloads emitted so far, in order.
    fn turns_sent(sink: &crate::host::sink::RecordingSink) -> Vec<serde_json::Value> {
        sink.events()
            .into_iter()
            .filter(|(name, _)| name == "turn:sent")
            .map(|(_, payload)| payload)
            .collect()
    }

    #[tokio::test]
    async fn a_turn_is_checkpointed_before_the_agent_is_handed_the_prompt() {
        let td = tempfile::tempdir().unwrap();
        let (sup, checkout) = idle_agent_in(td.path()).await;
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();

        let err = deliver_as_turn(&sup, &ctx, "yosemite", &pending(TURN))
            .await
            .unwrap_err();

        assert!(matches!(err, Error::AgentNotFound(_)), "got {err}");
        assert!(checkpoint::resolve(&checkout, TURN)
            .await
            .unwrap()
            .is_some());
    }

    /// The turn is open before the prompt is handed over, so a provider event
    /// that lands first finds it Running; a hand-off that fails puts the agent
    /// back exactly as it rested, announced as idle, with no turn-end side
    /// effects for a turn that never ran.
    #[tokio::test]
    async fn a_turn_opens_before_the_hand_off_and_closes_quietly_when_it_fails() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _checkout) = idle_agent_in(td.path()).await;
        let (ctx, sink, _dir) = crate::host::ctx::test_ctx();
        assert_eq!(sup.live_status("yosemite"), None);

        let err = deliver_as_turn(&sup, &ctx, "yosemite", &pending(TURN))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::AgentNotFound(_)), "got {err}");

        // Back to how it rested — no runtime entry at all, as before.
        assert_eq!(sup.live_status("yosemite"), None);
        let statuses: Vec<String> = sink
            .events()
            .into_iter()
            .filter_map(|(name, payload)| match name.as_str() {
                "turn:started" => Some("started".to_string()),
                "agent:status" => payload["status"].as_str().map(str::to_string),
                _ => None,
            })
            .collect();
        assert_eq!(statuses, ["started", "running", "idle"]);
        // The turn row was captured, but its start stamp was withdrawn with the
        // status: the turn never ran, so an unrelated Idle must not close it and
        // the retry's own stamp — which only writes a null — must land.
        let turns = sup.workspace.read_history_turns("yosemite").unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].started_at, None);
        sup.workspace.mark_user_turn_started(TURN, 4242).unwrap();
        let turns = sup.workspace.read_history_turns("yosemite").unwrap();
        assert_eq!(turns[0].started_at, Some(4242));
    }

    // ── the open turn ─────────────────────────────────────────────────────

    /// What each `agent:status` emitted so far said: the status, and the turn
    /// it reported open.
    fn statuses_said(sink: &crate::host::sink::RecordingSink) -> Vec<(String, Option<i64>)> {
        sink.events()
            .into_iter()
            .filter(|(name, _)| name == "agent:status")
            .map(|(_, p)| {
                (
                    p["status"].as_str().unwrap().to_string(),
                    p["turn_since"].as_i64(),
                )
            })
            .collect()
    }

    /// The `started_at` of each `turn:started` emitted so far.
    fn turns_started(sink: &crate::host::sink::RecordingSink) -> Vec<i64> {
        sink.events()
            .into_iter()
            .filter(|(name, _)| name == "turn:started")
            .map(|(_, p)| p["started_at"].as_i64().unwrap())
            .collect()
    }

    /// `yosemite` at rest with a record, nothing live.
    fn resting() -> Arc<Supervisor> {
        let sup = Arc::new(test_supervisor());
        let mut record = record_with_status("yosemite", AgentStatus::Idle);
        sup.workspace.add_agent(&mut record).unwrap();
        sup
    }

    /// The user's turn, as the engine reports it: open from the moment the
    /// message is accepted, through the launch of the process it is held for
    /// and that process coming to rest, and closed only when the turn has run.
    /// One start time the whole way, which is the status every client reads
    /// "working" from.
    #[tokio::test]
    async fn a_held_message_keeps_one_turn_open_from_acceptance_to_the_end_of_its_run() {
        let (ctx, sink, _dir) = crate::host::ctx::test_ctx();
        let sup = resting();

        let since = sup.open_turn(&ctx, "yosemite");
        // The session relaunches, rests, and only then takes the message.
        let launch = sup.begin_launch(&ctx, "yosemite");
        assert!(sup.settle_launch(&ctx, &launch, AgentStatus::Idle, None));
        assert_eq!(
            sup.agent_record("yosemite").unwrap().turn_since,
            Some(since)
        );
        sup.set_status(&ctx, "yosemite", AgentStatus::Running, None);
        sup.set_status(&ctx, "yosemite", AgentStatus::Idle, None);

        assert_eq!(
            statuses_said(&sink),
            [
                ("spawning".to_string(), Some(since)),
                ("idle".to_string(), Some(since)),
                ("running".to_string(), Some(since)),
                ("idle".to_string(), None),
            ]
        );
        // Announced once, where it opened: going Running did not open another.
        assert_eq!(turns_started(&sink), [since]);
        assert_eq!(sup.turn_since("yosemite"), None);
        assert_eq!(sup.agent_record("yosemite").unwrap().turn_since, None);
    }

    /// A turn nothing announced beforehand (the provider's own, a native one)
    /// is open for as long as the agent runs.
    #[tokio::test]
    async fn a_turn_nothing_opened_is_open_while_the_agent_runs() {
        let (ctx, sink, _dir) = crate::host::ctx::test_ctx();
        let sup = resting();
        sup.statuses
            .lock()
            .insert("yosemite".to_string(), AgentStatus::Idle);

        sup.set_status(&ctx, "yosemite", AgentStatus::Running, None);
        let since = sup.turn_since("yosemite").expect("open while running");
        sup.set_status(&ctx, "yosemite", AgentStatus::Idle, None);

        assert_eq!(
            statuses_said(&sink),
            [
                ("running".to_string(), Some(since)),
                ("idle".to_string(), None)
            ]
        );
    }

    /// Going down ends the turn, whatever it was waiting on.
    #[tokio::test]
    async fn a_launch_that_fails_closes_the_turn_it_was_for() {
        let (ctx, sink, _dir) = crate::host::ctx::test_ctx();
        let sup = resting();

        sup.open_turn(&ctx, "yosemite");
        let launch = sup.begin_launch(&ctx, "yosemite");
        assert!(sup.settle_launch(
            &ctx,
            &launch,
            AgentStatus::Error,
            Some("no session".to_string())
        ));

        assert_eq!(statuses_said(&sink).last().unwrap().1, None);
        assert_eq!(sup.turn_since("yosemite"), None);
    }

    /// A turn opened for a message no delivery is coming for is closed, and
    /// the clients told, by announcing the resting status again without it. A
    /// busy agent's turn is left for its status to end.
    #[tokio::test]
    async fn a_turn_with_no_delivery_coming_is_settled_and_a_busy_one_is_left() {
        let (ctx, sink, _dir) = crate::host::ctx::test_ctx();
        let sup = resting();

        let since = sup.open_turn(&ctx, "yosemite");
        let launch = sup.begin_launch(&ctx, "yosemite");
        sup.settle_turn(&ctx, "yosemite");
        assert_eq!(sup.turn_since("yosemite"), Some(since));

        assert!(sup.settle_launch(&ctx, &launch, AgentStatus::Idle, None));
        sup.settle_turn(&ctx, "yosemite");
        assert_eq!(sup.turn_since("yosemite"), None);
        assert_eq!(
            statuses_said(&sink).last().unwrap(),
            &("idle".to_string(), None)
        );

        // Nothing left to close: said once.
        let said = statuses_said(&sink).len();
        sup.settle_turn(&ctx, "yosemite");
        assert_eq!(statuses_said(&sink).len(), said);
    }

    /// `yosemite` with its process just come to rest from a spawn, and a turn
    /// open that a send refused during that spawn left behind: the agent was
    /// busy, so the send could not settle it, and nothing was queued. The
    /// context knows the supervisor, as it does once the engine has booted.
    fn rested_with_a_turn_left_open(ctx: &Arc<EngineCtx>) -> (Arc<Supervisor>, i64) {
        let sup = resting();
        ctx.set_supervisor(sup.clone());
        let launch = sup.begin_launch(ctx, "yosemite");
        let since = sup.open_turn(ctx, "yosemite");
        // The refused send's own settle: a no-op on a busy agent.
        sup.settle_turn(ctx, "yosemite");
        assert!(sup.settle_launch(ctx, &launch, AgentStatus::Idle, None));
        (sup, since)
    }

    /// Wait for the task a drain spawned to have closed `yosemite`'s turn.
    async fn turn_closed(sup: &Supervisor) -> bool {
        for _ in 0..200 {
            if sup.turn_since("yosemite").is_none() {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        false
    }

    /// A send refused while the agent was still coming up (its attachments
    /// could not be written into the guest, say) leaves the turn it opened.
    /// The process coming to rest with nothing held closes it and says so,
    /// rather than leave an idle agent reading "working" for good.
    #[tokio::test]
    async fn a_turn_left_by_a_send_refused_mid_spawn_closes_when_the_process_rests() {
        let (ctx, sink, _dir) = crate::host::ctx::test_ctx();
        let (sup, _since) = rested_with_a_turn_left_open(&ctx);

        // `start_process`, as the spawn completes.
        drain_message_queue(&sup, &ctx, "yosemite");

        assert!(turn_closed(&sup).await);
        assert_eq!(
            statuses_said(&sink).last().unwrap(),
            &("idle".to_string(), None)
        );
    }

    /// The delivery lock is not only a send's: a checkpoint, a rewind, an
    /// account switch and a workspace move take it too, and none of them
    /// settles a turn. One holding it as the process rests only delays the
    /// close until it is done.
    #[tokio::test]
    async fn a_turn_left_open_is_closed_once_an_unrelated_holder_of_the_delivery_lock_is_done() {
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        let (sup, since) = rested_with_a_turn_left_open(&ctx);
        let unrelated = sup.lock_delivery("yosemite").await;

        drain_message_queue(&sup, &ctx, "yosemite");
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert_eq!(sup.turn_since("yosemite"), Some(since), "held: not yet");

        drop(unrelated);
        assert!(turn_closed(&sup).await);
    }

    /// The holder is a send mid-delivery: it relaunched the process from
    /// inside itself (its login was due) and has since queued its message for
    /// the revive it asked for. Its turn is owed, and stays open.
    #[tokio::test]
    async fn a_turn_whose_send_was_mid_delivery_stays_open_when_that_send_queued_its_message() {
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        let (sup, since) = rested_with_a_turn_left_open(&ctx);
        let sending = sup.lock_delivery("yosemite").await;

        drain_message_queue(&sup, &ctx, "yosemite");
        sup.persist_and_enqueue("yosemite", pending(TURN));
        drop(sending);

        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        assert_eq!(sup.turn_since("yosemite"), Some(since));
    }

    /// The turn's row starts when its message was accepted, not when it was
    /// handed over: the time a turn is recorded to have run is the time its
    /// user waited, and it is the clock the live count shares.
    #[tokio::test]
    async fn a_turn_is_recorded_as_starting_when_its_message_was_accepted() {
        let (ctx, sink, _dir) = crate::host::ctx::test_ctx();
        let sup = resting();
        sup.statuses
            .lock()
            .insert("yosemite".to_string(), AgentStatus::Idle);
        sup.workspace
            .insert_user_turn("yosemite", TURN, "fix the build", &[])
            .unwrap();
        sup.open_turns.lock().insert("yosemite".to_string(), 4242);

        mark_user_turn_started(&sup, &ctx, "yosemite", Some(TURN));

        let turns = sup.workspace.read_history_turns("yosemite").unwrap();
        assert_eq!(turns[0].started_at, Some(4242));
        assert_eq!(turns_started(&sink), [4242]);
        assert_eq!(statuses_said(&sink), [("running".to_string(), Some(4242))]);
    }

    /// The whole path for a session with no process: the message is accepted,
    /// its turn opens at once, the delivery fails for want of a process, and
    /// the revive it asks for cannot launch (the session has nothing to
    /// resume). The turn is then closed and the clients told, rather than
    /// left reading "working" for an agent at rest.
    #[tokio::test]
    async fn a_message_whose_revive_cannot_launch_does_not_leave_its_turn_open() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _checkout) = idle_agent_in(td.path()).await;
        let (ctx, sink, _dir) = crate::host::ctx::test_ctx();

        let held = sup
            .clone()
            .send_user_message(&ctx, "yosemite", TURN, "hello", &[])
            .await
            .unwrap();
        assert!(held, "no process to deliver to: the message is held");

        // The revive runs on its own task.
        for _ in 0..200 {
            if sup.turn_since("yosemite").is_none() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(sup.turn_since("yosemite"), None);

        // Opened where the message was accepted, ahead of anything else it
        // caused, and every status up to the close carried that one start.
        let events = sink.events();
        let opened = events
            .iter()
            .position(|(name, _)| name == "turn:started")
            .unwrap();
        let first_status = events
            .iter()
            .position(|(name, _)| name == "agent:status")
            .unwrap();
        assert!(opened < first_status);
        let since = turns_started(&sink)[0];
        let said = statuses_said(&sink);
        let (last, before) = said.split_last().unwrap();
        assert!(
            before.iter().all(|(_, turn)| *turn == Some(since)),
            "{said:?}"
        );
        assert_eq!(last.1, None, "{said:?}");
    }

    /// A message to a session at rest, with no process to take it, is held for
    /// the revive it asks for and nothing more: no checkpoint (whose git, in a
    /// box agent's checkout, would boot the stopped box first) and no delivery
    /// that could only fail, so its turn never flaps Running and back.
    #[tokio::test]
    async fn a_message_to_a_session_at_rest_is_held_for_its_revive_untried() {
        let td = tempfile::tempdir().unwrap();
        let (sup, checkout) = idle_agent_in(td.path()).await;
        let (ctx, sink, _dir) = crate::host::ctx::test_ctx();

        let held = sup
            .clone()
            .send_user_message(&ctx, "yosemite", TURN, "hello", &[])
            .await
            .unwrap();

        assert!(held);
        assert_eq!(sup.message_queue.lock().turn_ids("yosemite"), [TURN]);
        assert_eq!(checkpoint::resolve(&checkout, TURN).await.unwrap(), None);
        let said = statuses_said(&sink);
        assert!(
            said.iter().all(|(status, _)| status != "running"),
            "{said:?}"
        );
    }

    /// The rest of that path, for a session whose box the user stopped
    /// mid-turn (`rest_sessions`, which leaves that turn's stop for the next
    /// one to clear): once the revived process is up, the held message is
    /// checkpointed and delivered, there and only there.
    #[tokio::test]
    async fn a_held_message_is_checkpointed_and_delivered_once_its_revived_process_is_up() {
        let td = tempfile::tempdir().unwrap();
        let (sup, checkout) = idle_agent_in(td.path()).await;
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        ctx.set_supervisor(sup.clone());
        mark_user_turn_started(&sup, &ctx, "yosemite", None);
        sup.rest_sessions(&ctx, &["yosemite".to_string()]);

        let held = sup
            .clone()
            .send_user_message(&ctx, "yosemite", TURN, "hello", &[])
            .await
            .unwrap();
        assert!(held);
        // This session has nothing to resume, so the revive the send started
        // fails at once and closes the turn; its launch is played here.
        assert!(turn_closed(&sup).await);
        let launch = sup.begin_launch(&ctx, "yosemite");
        live_process(&sup, td.path());
        assert!(sup.settle_launch(&ctx, &launch, AgentStatus::Idle, None));
        drain_message_queue(&sup, &ctx, "yosemite");

        for _ in 0..200 {
            if sup.live_status("yosemite") == Some(AgentStatus::Running) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(sup.live_status("yosemite"), Some(AgentStatus::Running));
        assert!(sup.message_queue.lock().is_empty("yosemite"));
        assert!(checkpoint::resolve(&checkout, TURN)
            .await
            .unwrap()
            .is_some());
    }

    // ── turn outcomes ─────────────────────────────────────────────────────

    /// `yosemite`'s first turn, persisted and running, as a delivery leaves it.
    fn running_turn(ctx: &Arc<EngineCtx>) -> Arc<Supervisor> {
        let sup = Arc::new(test_supervisor());
        let mut record = record_with_status("yosemite", AgentStatus::Idle);
        sup.workspace.add_agent(&mut record).unwrap();
        sup.statuses
            .lock()
            .insert("yosemite".to_string(), AgentStatus::Idle);
        sup.workspace
            .insert_user_turn("yosemite", TURN, "fix the build", &[])
            .unwrap();
        mark_user_turn_started(&sup, ctx, "yosemite", Some(TURN));
        sup
    }

    fn outcomes(sup: &Supervisor) -> Vec<Option<String>> {
        sup.workspace
            .read_history_turns("yosemite")
            .unwrap()
            .into_iter()
            .map(|t| t.outcome)
            .collect()
    }

    #[tokio::test]
    async fn a_turn_that_runs_to_its_end_is_completed() {
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        let sup = running_turn(&ctx);

        transition_active(&sup, &ctx, "yosemite", AgentStatus::Idle);

        assert_eq!(outcomes(&sup), [Some("completed".to_string())]);
    }

    /// The stop flag is consumed by the turn-end drain, which runs after the
    /// turn is closed — so the close still sees it.
    #[tokio::test]
    async fn a_turn_the_user_stops_is_interrupted() {
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        let sup = running_turn(&ctx);

        sup.clone()
            .stop_agent(ctx.clone(), "yosemite")
            .await
            .unwrap();
        transition_active(&sup, &ctx, "yosemite", AgentStatus::Idle);

        assert_eq!(outcomes(&sup), [Some("interrupted".to_string())]);
        assert!(!sup.interrupted.lock().contains("yosemite"), "drained");
    }

    /// A stop that lands between turns is moot once the next turn starts.
    #[tokio::test]
    async fn a_stop_before_a_turn_starts_does_not_mark_it() {
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        let sup = Arc::new(test_supervisor());
        let mut record = record_with_status("yosemite", AgentStatus::Idle);
        sup.workspace.add_agent(&mut record).unwrap();
        sup.clone()
            .stop_agent(ctx.clone(), "yosemite")
            .await
            .unwrap();
        sup.workspace
            .insert_user_turn("yosemite", TURN, "fix the build", &[])
            .unwrap();
        mark_user_turn_started(&sup, &ctx, "yosemite", Some(TURN));

        transition_active(&sup, &ctx, "yosemite", AgentStatus::Idle);

        assert_eq!(outcomes(&sup), [Some("completed".to_string())]);
    }

    #[tokio::test]
    async fn a_turn_that_ends_in_error_is_failed() {
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        let sup = running_turn(&ctx);

        sup.set_status(
            &ctx,
            "yosemite",
            AgentStatus::Error,
            Some("Agent process exited: boom".into()),
        );

        assert_eq!(outcomes(&sup), [Some("failed".to_string())]);
    }

    /// A message held for a later delivery (here, one sent to a session at
    /// rest; as well, one whose hand-off failed) is not given up on: its row
    /// stays without an outcome, and a later turn end doesn't close it.
    #[tokio::test]
    async fn a_requeued_delivery_has_no_outcome() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _checkout) = idle_agent_in(td.path()).await;
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();

        let held = sup
            .clone()
            .send_user_message(&ctx, "yosemite", TURN, "hello", &[])
            .await
            .unwrap();
        assert!(held);
        assert_eq!(sup.message_queue.lock().turn_ids("yosemite"), [TURN]);

        mark_user_turn_started(&sup, &ctx, "yosemite", None);
        transition_active(&sup, &ctx, "yosemite", AgentStatus::Idle);
        assert_eq!(outcomes(&sup), [None]);
    }

    /// Capture is best-effort: a checkout that can't be snapshotted is skipped
    /// and the turn still goes out (here, as far as the missing process).
    #[tokio::test]
    async fn a_failed_checkpoint_does_not_stop_the_send() {
        let td = tempfile::tempdir().unwrap();
        let (sup, checkout) = idle_agent_in(td.path()).await;
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        std::fs::remove_dir_all(&checkout).unwrap();

        let err = deliver_as_turn(&sup, &ctx, "yosemite", &pending(TURN))
            .await
            .unwrap_err();

        assert!(matches!(err, Error::AgentNotFound(_)), "got {err}");
        let turns = sup.workspace.read_history_turns("yosemite").unwrap();
        assert_eq!(turns.len(), 1, "the turn was persisted past the checkpoint");
    }

    /// A checkout too slow to snapshot can't hold the send past the capture
    /// deadline: the checkpoint is dropped and the turn goes out regardless.
    #[tokio::test]
    async fn a_checkpoint_past_its_deadline_is_skipped_and_the_send_goes_on() {
        let td = tempfile::tempdir().unwrap();
        let (sup, checkout) = idle_agent_in(td.path()).await;
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();

        // Paused, the clock jumps to the next timer whenever the runtime would
        // otherwise wait — the capture deadline, while git is still running.
        tokio::time::pause();
        let started = tokio::time::Instant::now();
        let err = deliver_as_turn(&sup, &ctx, "yosemite", &pending(TURN))
            .await
            .unwrap_err();
        let waited = started.elapsed();
        tokio::time::resume();

        // Cut off at the capture deadline, not git's own 120s per command.
        assert!(
            waited >= CAPTURE_TIMEOUT && waited < CAPTURE_TIMEOUT + Duration::from_secs(1),
            "waited {waited:?}"
        );
        assert!(matches!(err, Error::AgentNotFound(_)), "got {err}");
        let turns = sup.workspace.read_history_turns("yosemite").unwrap();
        assert_eq!(turns.len(), 1, "the turn went out past the timeout");
        assert_eq!(checkpoint::resolve(&checkout, TURN).await.unwrap(), None);
    }

    const FIRST: &str = "0a1b2c3d-0000-4000-8000-000000000001";
    const SECOND: &str = "0a1b2c3d-0000-4000-8000-000000000002";

    /// A send that arrives while a delivery is in flight waits for that turn
    /// to start, then routes as its follow-up rather than a turn of its own.
    #[tokio::test]
    async fn a_send_during_a_delivery_waits_and_follows_its_turn() {
        let td = tempfile::tempdir().unwrap();
        let (sup, checkout) = idle_agent_in(td.path()).await;
        let (ctx, sink, _dir) = crate::host::ctx::test_ctx();

        // The first message's delivery holds the lock while it checkpoints.
        let first_delivery = sup.lock_delivery("yosemite").await;
        let second = tokio::spawn({
            let (sup, ctx) = (sup.clone(), ctx.clone());
            async move {
                sup.send_user_message(&ctx, "yosemite", SECOND, "second", &[])
                    .await
            }
        });
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert!(turns_sent(&sink).is_empty(), "not routed mid-delivery");

        // The first turn starts, and its delivery lets go.
        mark_user_turn_started(&sup, &ctx, "yosemite", Some(FIRST));
        drop(first_delivery);

        assert!(second.await.unwrap().unwrap(), "held for the turn boundary");
        let sent = turns_sent(&sink);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["follow_up"], true);
        assert_eq!(sup.message_queue.lock().turn_ids("yosemite"), [SECOND]);
        assert_eq!(checkpoint::resolve(&checkout, SECOND).await.unwrap(), None);
    }

    /// Two sends racing into an idle agent are routed in send order: the second
    /// waits out the first's routing and queues behind it. (With no process
    /// both are held for the revive, which checkpoints the turn they make;
    /// their order is what this checks.)
    #[tokio::test]
    async fn concurrent_sends_reach_the_agent_in_send_order() {
        let td = tempfile::tempdir().unwrap();
        let (sup, checkout) = idle_agent_in(td.path()).await;
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();

        let (first, second) = tokio::join!(
            sup.clone()
                .send_user_message(&ctx, "yosemite", FIRST, "first", &[]),
            sup.clone()
                .send_user_message(&ctx, "yosemite", SECOND, "second", &[]),
        );

        assert!(first.unwrap() && second.unwrap(), "both held");
        let held = sup
            .message_queue
            .lock()
            .drain_coalesced("yosemite")
            .unwrap();
        assert_eq!(held.text, "first\n\nsecond");
        assert_eq!(held.turn_id, SECOND);
        assert_eq!(checkpoint::resolve(&checkout, FIRST).await.unwrap(), None);
    }

    /// The delivery lock orders the turns Fletch starts, not one the user
    /// types into the native TUI. A flush that finds such a turn running holds
    /// its message for that turn's boundary, with nothing captured or
    /// persisted.
    #[tokio::test]
    async fn a_turn_typed_into_the_native_tui_is_not_superseded() {
        let td = tempfile::tempdir().unwrap();
        let (sup, checkout) = idle_agent_in(td.path()).await;
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        // What `write_to_agent` does when the typed line is submitted.
        mark_user_turn_started(&sup, &ctx, "yosemite", None);

        sup.persist_and_enqueue("yosemite", pending(TURN));
        assert!(flush_queued(&sup, &ctx, "yosemite").await.unwrap(), "held");

        assert_eq!(sup.message_queue.lock().turn_ids("yosemite"), [TURN]);
        assert_eq!(checkpoint::resolve(&checkout, TURN).await.unwrap(), None);
        assert!(sup
            .workspace
            .read_history_turns("yosemite")
            .unwrap()
            .is_empty());
    }

    // ── turn rows and the records their prompts produce ───────────────────

    /// A process for `yosemite` that takes whatever it is handed and ignores
    /// it: a live agent to deliver to, without a real one.
    fn live_process(sup: &Supervisor, dir: &std::path::Path) {
        let pty = PtySession::spawn(
            PtySpawn {
                program: std::path::Path::new("/bin/sh"),
                args: &["-c".to_string(), "cat >/dev/null".to_string()],
                cwd: dir,
                env: &[],
                cols: 80,
                rows: 24,
                env_remove: &[],
                kill_plan: KillHandle::ProcessGroup,
                after_exit: None,
            },
            |_| {},
            |_| {},
        )
        .unwrap();
        sup.agents
            .lock()
            .insert("yosemite".to_string(), Arc::new(Agent::over_pty(pty)));
    }

    fn msg(turn_id: &str, text: &str) -> PendingMsg {
        PendingMsg {
            turn_id: turn_id.to_string(),
            text: text.to_string(),
            attachments: vec![],
        }
    }

    fn said(role: &str, text: &str) -> serde_json::Value {
        serde_json::json!({"type": role, "text": text})
    }

    /// Ingest `records` into `yosemite`'s session and match its turns, as a
    /// turn-end sync does.
    fn ingest(sup: &Supervisor, records: &[(&str, serde_json::Value)]) {
        let records: Vec<_> = records.iter().map(|(id, body)| (*id, body)).collect();
        sup.workspace
            .append_session_records("yosemite", "claude", "transcript", None, &records)
            .unwrap();
        sup.workspace
            .associate_pending_user_turns("yosemite", crate::workspace::tests::test_prompts)
            .unwrap();
    }

    /// `(turn_id, matched record)` of `yosemite`'s turns, in send order.
    fn matched(sup: &Supervisor) -> Vec<(String, Option<String>)> {
        sup.workspace
            .read_history_turns("yosemite")
            .unwrap()
            .into_iter()
            .map(|t| (t.turn_id, t.native_id))
            .collect()
    }

    /// The race this order closes: the agent's record of an injected message
    /// can be ingested the moment the write lands — which, when the row was
    /// written after the write, came before the row and left the turn
    /// unmatched for good.
    #[tokio::test]
    async fn an_injection_ingested_as_it_is_written_still_matches_its_turn() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = idle_agent_in(td.path()).await;
        // The running turn so far, its answer quoting what comes next.
        ingest(&sup, &[("a0", said("assistant", "Reply yes to go on."))]);

        // The hand-off, with the agent's record of it ingested at once.
        sup.persist_and_send("yosemite", &msg(FIRST, "yes"), || {
            ingest(&sup, &[("u1", said("user", "yes"))]);
            Ok(())
        })
        .unwrap();

        assert_eq!(matched(&sup), [(FIRST.to_string(), Some("u1".to_string()))]);
    }

    #[tokio::test]
    async fn live_injections_into_one_turn_each_match_their_own_record_in_order() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = idle_agent_in(td.path()).await;
        live_process(&sup, td.path());
        sup.workspace
            .insert_user_turn("yosemite", TURN, "fix the build", &[])
            .unwrap();

        sup.inject_live("yosemite", &msg(FIRST, "yes")).unwrap();
        sup.inject_live("yosemite", &msg(SECOND, "yes")).unwrap();
        // The turn ends, and its transcript lands in one batch.
        ingest(
            &sup,
            &[
                ("u0", said("user", "fix the build")),
                ("a0", said("assistant", "working on it")),
                ("u1", said("user", "yes")),
                ("a1", said("assistant", "ok")),
                ("u2", said("user", "yes")),
                ("a2", said("assistant", "done")),
            ],
        );

        assert_eq!(
            matched(&sup),
            [
                (TURN.to_string(), Some("u0".to_string())),
                (FIRST.to_string(), Some("u1".to_string())),
                (SECOND.to_string(), Some("u2".to_string())),
            ]
        );
    }

    /// An injection that couldn't be written takes back the row it made: the
    /// caller queues the message, and its delivery writes the row again.
    #[tokio::test]
    async fn a_failed_injection_leaves_no_turn_behind() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = idle_agent_in(td.path()).await;

        let err = sup.inject_live("yosemite", &msg(FIRST, "yes")).unwrap_err();

        assert!(matches!(err, Error::AgentNotFound(_)), "got {err}");
        assert!(matched(&sup).is_empty());
        // A row it didn't make (a retry's) stays.
        sup.workspace
            .insert_user_turn("yosemite", SECOND, "again", &[])
            .unwrap();
        assert!(sup.inject_live("yosemite", &msg(SECOND, "again")).is_err());
        assert_eq!(matched(&sup), [(SECOND.to_string(), None)]);
    }

    /// Coalesced follow-ups go out as one turn under the last message's id,
    /// and that row too is written before the hand-off.
    #[tokio::test]
    async fn a_coalesced_flush_has_its_turn_before_the_agent_has_the_message() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = idle_agent_in(td.path()).await;
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        live_process(&sup, td.path());
        ingest(&sup, &[("a0", said("assistant", "say first\n\nsecond"))]);
        sup.persist_and_enqueue("yosemite", msg(FIRST, "first"));
        sup.persist_and_enqueue("yosemite", msg(SECOND, "second"));

        assert!(!flush_queued(&sup, &ctx, "yosemite").await.unwrap(), "sent");
        ingest(&sup, &[("u1", said("user", "first\n\nsecond"))]);

        assert_eq!(
            matched(&sup),
            [(SECOND.to_string(), Some("u1".to_string()))]
        );
    }

    /// `(turn_id, text)` of `yosemite`'s turn rows, in send order.
    fn rows(sup: &Supervisor) -> Vec<(String, String)> {
        sup.workspace
            .read_history_turns("yosemite")
            .unwrap()
            .into_iter()
            .map(|t| (t.turn_id, t.text))
            .collect()
    }

    /// Hand `msg` to an agent with no process, as a send does: the delivery
    /// fails after writing the row, and the message is re-queued.
    async fn fail_and_requeue(sup: &Arc<Supervisor>, ctx: &Arc<EngineCtx>, msg: PendingMsg) {
        assert!(deliver_as_turn(sup, ctx, "yosemite", &msg).await.is_err());
        sup.persist_and_enqueue("yosemite", msg);
    }

    /// Follow-ups folded into one prompt are one turn: the rows their failed
    /// deliveries left behind don't outlive the coalesced delivery.
    #[tokio::test]
    async fn a_coalesced_flush_leaves_only_the_coalesced_turn() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = idle_agent_in(td.path()).await;
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        fail_and_requeue(&sup, &ctx, msg(FIRST, "first")).await;
        fail_and_requeue(&sup, &ctx, msg(SECOND, "second")).await;
        assert_eq!(rows(&sup).len(), 2);

        live_process(&sup, td.path());
        assert!(!flush_queued(&sup, &ctx, "yosemite").await.unwrap(), "sent");

        assert_eq!(
            rows(&sup),
            [(SECOND.to_string(), "first\n\nsecond".to_string())]
        );
    }

    /// A coalesced flush that fails re-queues the merged message under its own
    /// id, so the next flush can't see the members — their rows are gone
    /// already, and the coalesced row carries their text.
    #[tokio::test]
    async fn a_failed_coalesced_flush_leaves_no_member_rows_behind() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = idle_agent_in(td.path()).await;
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        fail_and_requeue(&sup, &ctx, msg(FIRST, "first")).await;
        sup.persist_and_enqueue("yosemite", msg(SECOND, "second"));

        assert!(flush_queued(&sup, &ctx, "yosemite").await.unwrap(), "held");
        assert_eq!(sup.message_queue.lock().turn_ids("yosemite"), [SECOND]);
        live_process(&sup, td.path());
        assert!(!flush_queued(&sup, &ctx, "yosemite").await.unwrap(), "sent");

        assert_eq!(
            rows(&sup),
            [(SECOND.to_string(), "first\n\nsecond".to_string())]
        );
    }

    #[tokio::test]
    async fn a_single_requeued_follow_up_keeps_its_turn() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = idle_agent_in(td.path()).await;
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        fail_and_requeue(&sup, &ctx, msg(FIRST, "first")).await;

        live_process(&sup, td.path());
        assert!(!flush_queued(&sup, &ctx, "yosemite").await.unwrap(), "sent");

        assert_eq!(rows(&sup), [(FIRST.to_string(), "first".to_string())]);
    }
}
