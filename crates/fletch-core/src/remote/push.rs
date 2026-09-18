//! Push-notification triggers: the two out-of-app signals the desktop already
//! raises, mirrored to paired phones as APNs alerts.
//!
//! The contract is `docs/remote-protocol.md` → "Push notifications". The host's
//! whole job is deciding *when* and building the NOTIFY frame; the relay holds
//! the APNs key and does the sending, so nothing here talks to Apple and no
//! transcript text leaves the Mac — the body is the agent's name.
//!
//! The rules mirror `signalAway` in `src/store/eventListeners.ts`, which is the
//! desktop's own chime/notification gate, so a phone and the Mac agree about
//! what is worth interrupting a user for:
//!
//! - `turn_complete` on a `running → idle` transition the user did not cause.
//! - `needs_input` on the first held `can_use_tool` prompt for an agent, one
//!   per batch of parallel prompts.
//! - Neither while the desktop's own window has focus: the user is right here.
//!
//! ## Why a sink and not a subscriber task
//!
//! Telling a user stop from a natural end means reading `Supervisor::interrupted`
//! at the instant of the Idle transition, and that instant is short:
//! `supervisor::transition_active` emits `agent:status` and then, with no await
//! in between, calls `drain_message_queue`, which *removes* the agent from
//! `interrupted` (`supervisor/mod.rs`, `messaging.rs`). A subscriber on a
//! channel — the engine's broadcast, or `Supervisor::subscribe_status` — is a
//! separate task and may be polled on another worker thread, so it can only
//! ever race that removal: it would report every stop as a natural completion.
//!
//! So these two triggers are an [`EventSink`] in the host's fanout instead.
//! A sink runs *inside* `emit`, on the emitting thread, exactly where the Tauri
//! `listen_any` tap used to run, and `agent:status` is emitted before
//! `drain_message_queue` in the same frame — so the flag still means something
//! when the trigger reads it. Hence this module contains no `async`.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value};

use super::RemoteState;
use crate::host::{EngineCtx, EventSink, Sink};
use crate::supervisor::Supervisor;
use crate::workspace::{AgentStatus, AgentView};

/// Settings key: alert on `turn_complete` at all. Opt-out — only `"false"`
/// silences it; `needs_input` is always sent. Shared with the desktop chime and
/// banner (the frontend reads the same key), so one switch covers every alert
/// surface. Mirrored in memory because the taps run without a DB handle.
pub const TURN_COMPLETE_SETTING: &str = "notify_turn_complete";
static TURN_COMPLETE: AtomicBool = AtomicBool::new(true);

pub fn parse_turn_complete(raw: Option<&str>) -> bool {
    raw != Some("false")
}

pub fn set_turn_complete(enabled: bool) {
    TURN_COMPLETE.store(enabled, Ordering::Relaxed);
}

fn turn_complete_enabled() -> bool {
    TURN_COMPLETE.load(Ordering::Relaxed)
}

/// The two `kind`s and their titles, verbatim from the protocol doc.
const KIND_TURN_COMPLETE: &str = "turn_complete";
const KIND_NEEDS_INPUT: &str = "needs_input";
const TITLE_TURN_COMPLETE: &str = "Turn complete";
const TITLE_NEEDS_INPUT: &str = "Needs your input";

/// Doc cap on the tokens in one NOTIFY. The relay caps device links per host at
/// 8 too, so this can only bite if that ever grows; truncating is defensive.
const MAX_TOKENS: usize = 8;
/// Doc cap on `title` and `body`, in characters.
const MAX_TEXT_CHARS: usize = 200;

/// Body for an agent whose record has gone (archived mid-turn, say). The alert
/// is still worth sending — the phone shows the agent by id.
const UNNAMED_AGENT: &str = "Agent";

/// What a trigger needs to know about an agent. A trait for the same reason
/// [`super::dispatch::Dispatch`] is one: it lets the rules below be tested
/// without standing up a `Supervisor` and a database, and it is the entire
/// surface this module has on the rest of the app.
pub(super) trait AgentLookup: Send + Sync {
    /// The name a user would recognize — the alert's whole body.
    fn agent_name(&self, agent_id: &str) -> Option<String>;
    /// Whether the turn that is ending was stopped by the user, rather than
    /// finishing on its own. See the module doc on why this must be read
    /// synchronously.
    fn was_interrupted(&self, agent_id: &str) -> bool;
    /// Whether the agent runs in the native PTY view. Its status is a heuristic
    /// read off terminal quiet, so `running → idle` there is not a turn ending
    /// — and the desktop never notifies for it either (`signalAway` fires from
    /// the structured event stream, which a native agent does not have).
    fn is_native(&self, agent_id: &str) -> bool;
}

impl AgentLookup for Supervisor {
    fn agent_name(&self, agent_id: &str) -> Option<String> {
        self.workspace
            .agent(agent_id)
            .ok()
            .map(|record| record.name)
    }

    fn is_native(&self, agent_id: &str) -> bool {
        self.workspace
            .agent(agent_id)
            .ok()
            .is_some_and(|record| matches!(record.view, AgentView::Native))
    }

    fn was_interrupted(&self, agent_id: &str) -> bool {
        // Read, not consumed: `drain_message_queue` owns the removal, and
        // taking it here would make a stop auto-flush the follow-up queue.
        self.interrupted.lock().contains(agent_id)
    }
}

/// Whether the user is at the Mac. Injectable so the tests can force either
/// answer without a window.
pub(super) type FocusCheck = Box<dyn Fn() -> bool + Send + Sync>;

/// Where a built NOTIFY payload goes. `true` when it was queued for the relay.
/// Injectable for the same reason as [`FocusCheck`]: it is the only observable
/// effect the rules have.
pub(super) type NotifySink = Box<dyn Fn(String) -> bool + Send + Sync>;

/// The trigger state machine. One per process, shared by the taps.
pub(super) struct PushTriggers {
    state: Arc<RemoteState>,
    focused: FocusCheck,
    notify: NotifySink,
    /// Agents currently `Running`, so `running → idle` is an edge rather than a
    /// status reading. A set, not a map of last-seen statuses, so it stays
    /// bounded by the live agents instead of growing with every agent the app
    /// has ever run.
    running: Mutex<HashSet<String>>,
    /// Agents with a `can_use_tool` prompt already held. The mark is what makes
    /// a batch of parallel prompts one alert; it is dropped when the agent
    /// leaves `Running`, which is also how a turn ends.
    pending_input: Mutex<HashSet<String>>,
}

impl PushTriggers {
    pub(super) fn new(state: Arc<RemoteState>, focused: FocusCheck, notify: NotifySink) -> Self {
        Self {
            state,
            focused,
            notify,
            running: Mutex::new(HashSet::new()),
            pending_input: Mutex::new(HashSet::new()),
        }
    }

    /// The production wiring: alerts go out on the host link.
    pub(super) fn for_state(state: Arc<RemoteState>, focused: FocusCheck) -> Self {
        let sink_state = state.clone();
        Self::new(
            state,
            focused,
            Box::new(move |payload| sink_state.send_notify(payload)),
        )
    }

    /// One `agent:status` transition.
    pub(super) fn on_status(&self, agents: &dyn AgentLookup, agent_id: &str, status: &AgentStatus) {
        if matches!(status, AgentStatus::Running) {
            self.running.lock().insert(agent_id.to_string());
            return;
        }
        // Anything else is the agent leaving `Running`, which is also the end of
        // any turn that was holding prompts — so the batch mark goes with it,
        // and the next `can_use_tool` can alert again.
        let was_running = self.running.lock().remove(agent_id);
        self.pending_input.lock().remove(agent_id);
        if !was_running || !matches!(status, AgentStatus::Idle) {
            return;
        }
        if agents.was_interrupted(agent_id) {
            // A stop converges on this same Idle (the dying process flushes its
            // last event); it is not a completion to celebrate.
            return;
        }
        if agents.is_native(agent_id) {
            // A native agent's Idle is "the terminal went quiet", which happens
            // several times in one turn; there is no turn boundary to report.
            return;
        }
        if !turn_complete_enabled() {
            return;
        }
        self.alert(agents, agent_id, KIND_TURN_COMPLETE, TITLE_TURN_COMPLETE);
    }

    /// Route one event off the sink. The two names are the whole surface; every
    /// other event the engine emits passes through untouched.
    pub(super) fn on_event(&self, agents: &dyn AgentLookup, event: &str, payload: &Value) {
        match event {
            "agent:status" => {
                if let Ok(status) = StatusPayload::deserialize(payload) {
                    self.on_status(agents, &status.agent_id, &status.status);
                }
            }
            "agent:event" => self.on_agent_event(agents, payload),
            _ => {}
        }
    }

    /// One `agent:event` payload. Only held `can_use_tool` control requests are
    /// of interest; everything else is transcript.
    pub(super) fn on_agent_event(&self, agents: &dyn AgentLookup, payload: &Value) {
        let Ok(payload) = AgentEventPayload::deserialize(payload) else {
            return;
        };
        let held = payload.event.is_some_and(|event| {
            event.kind.as_deref() == Some("control_request")
                && event
                    .request
                    .is_some_and(|r| r.subtype.as_deref() == Some("can_use_tool"))
        });
        if !held {
            return;
        }
        // `insert` is false when a prompt is already held: a turn can forward
        // several at once, and one alert for the batch beats one per prompt.
        if !self.pending_input.lock().insert(payload.agent_id.clone()) {
            return;
        }
        self.alert(
            agents,
            &payload.agent_id,
            KIND_NEEDS_INPUT,
            TITLE_NEEDS_INPUT,
        );
    }

    /// Build and send one alert, unless the user is already looking at it or no
    /// device could receive it.
    fn alert(&self, agents: &dyn AgentLookup, agent_id: &str, kind: &str, title: &str) {
        if (self.focused)() {
            tracing::debug!(agent_id, kind, "remote: push skipped, the Mac has focus");
            return;
        }
        let tokens: Vec<serde_json::Value> = self
            .state
            .devices()
            .list()
            .iter()
            .filter_map(|d| d.push_target())
            .take(MAX_TOKENS)
            .map(|(token, environment)| json!({ "token": token, "environment": environment }))
            .collect();
        if tokens.is_empty() {
            return;
        }
        let body = agents
            .agent_name(agent_id)
            .unwrap_or_else(|| UNNAMED_AGENT.to_string());
        let payload = json!({
            "tokens": tokens,
            "title": clamp(title),
            "body": clamp(&body),
            "kind": kind,
            "agentId": agent_id,
            // One alert per agent on the phone: a later one for the same agent
            // replaces the banner rather than stacking under it.
            "collapseId": agent_id,
        });
        if !(self.notify)(payload.to_string()) {
            tracing::debug!(
                agent_id,
                kind,
                "remote: push dropped, no relay link to send it on"
            );
        }
    }
}

/// Trim to the doc's 200-character cap, on a character boundary.
fn clamp(text: &str) -> String {
    text.chars().take(MAX_TEXT_CHARS).collect()
}

/// The tap, as a sink for the host's fanout. Built from `events::install_taps`,
/// so push follows the same "in unconditionally, short-circuits when nothing is
/// listening" rule as event forwarding: with no relay link and no registered
/// token, `alert` gives up before it builds anything.
pub(super) fn tap(ctx: &Arc<EngineCtx>, state: Arc<RemoteState>) -> Sink {
    Arc::new(PushTap {
        triggers: PushTriggers::for_state(state, host_focus(ctx.clone())),
        ctx: ctx.clone(),
    })
}

/// The two triggers hanging off the engine's emits. See the module doc for why
/// this is a sink and not a task.
struct PushTap {
    triggers: PushTriggers,
    ctx: Arc<EngineCtx>,
}

impl EventSink for PushTap {
    fn emit_value(&self, event: &str, payload: Value) -> Result<(), String> {
        with_supervisor(&self.ctx, |agents| {
            self.triggers.on_event(agents, event, &payload)
        });
        // A tap, not a destination: "nobody could be reached" is not a thing it
        // can report, and the one caller that reads an emit error is asking
        // about the user's window (see `host::sink::FanoutSink`).
        Ok(())
    }
}

/// Run `f` with the supervisor, if there is one. Resolved per event rather than
/// captured, so the taps can be installed before (or without) the supervisor
/// being published on the ctx — the same lookup `supervisor::messaging` does.
fn with_supervisor(ctx: &EngineCtx, f: impl FnOnce(&Supervisor)) {
    if let Some(sup) = ctx.supervisor() {
        f(sup.as_ref());
    }
}

/// The focus check: is the user looking at this host right now. Whatever the
/// host answers — a desktop asks its main window; a headless host says no, so a
/// trigger is sent rather than silently swallowed.
fn host_focus(ctx: Arc<EngineCtx>) -> FocusCheck {
    Box::new(move || (ctx.focus)())
}

/// `supervisor::events::AgentStatusPayload`, the part of it this needs.
#[derive(Deserialize)]
struct StatusPayload {
    agent_id: String,
    status: AgentStatus,
}

/// `supervisor::events::AgentEventPayload`. The inner event is a provider's own
/// JSON, so only the two discriminants the trigger reads are named and every
/// one of them is optional — the overwhelming majority of these payloads are
/// transcript events with none of these keys.
#[derive(Deserialize)]
struct AgentEventPayload {
    agent_id: String,
    event: Option<RawEvent>,
}

#[derive(Deserialize)]
struct RawEvent {
    #[serde(rename = "type")]
    kind: Option<String>,
    request: Option<RawRequest>,
}

#[derive(Deserialize)]
struct RawRequest {
    subtype: Option<String>,
}
