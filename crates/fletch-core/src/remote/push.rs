//! Push-notification triggers: the out-of-app signals the desktop already
//! raises, plus the ship loop's, mirrored to paired phones as APNs alerts.
//!
//! The contract is `docs/remote-protocol.md` → "Push notifications". The host's
//! whole job is deciding *when* and building the NOTIFY frame; the relay holds
//! the APNs key and does the sending, so nothing here talks to Apple and no
//! transcript text leaves the Mac — the body is the agent's name, at most
//! joined by a check name, a reviewer's login or a PR number.
//!
//! The first two rules mirror `signalAway` in `src/store/eventListeners.ts`,
//! which is the desktop's own chime/notification gate, so a phone and the Mac
//! agree about what is worth interrupting a user for:
//!
//! - `turn_complete` on a `running → idle` transition the user did not cause.
//! - `needs_input` on the first held `can_use_tool` prompt for an agent, one
//!   per batch of parallel prompts.
//!
//! The ship-loop rules read the events the host-side PR watcher
//! (`supervisor::pr_watch`) emits, so they fire with the window shut:
//!
//! - `checks_settled` when a PR's CI rollup lands on `passing` or `failing`
//!   from anything else (a `failing → failing` with a different set of names is
//!   not a second alert; `failing → passing` is).
//! - `review_comment` when unresolved review threads gain one not seen before.
//! - `pr_merged` / `pr_closed` when a PR this process saw open leaves `open`,
//!   so a cold start's first stale snapshot of a merged PR alerts nobody.
//! - `autopilot_gave_up` when the host's autopilot gives up on a rung (an
//!   `autopilot:event` with outcome `give-up`), under the same opt-out.
//! - None of them while the desktop's own window has focus: the user is right
//!   here.
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
//! So these triggers are an [`EventSink`] in the host's fanout instead.
//! A sink runs *inside* `emit`, on the emitting thread, exactly where the Tauri
//! `listen_any` tap used to run, and `agent:status` is emitted before
//! `drain_message_queue` in the same frame — so the flag still means something
//! when the trigger reads it. Hence this module contains no `async`.

use std::collections::{HashMap, HashSet};
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
/// Settings key: alert on the ship loop at all — `checks_settled`,
/// `review_comment`, `pr_merged`, `pr_closed` and `autopilot_gave_up` under
/// one switch. Opt-out,
/// same mechanics as `notify_turn_complete`.
pub const PR_ACTIVITY_SETTING: &str = "notify_pr_activity";

static TURN_COMPLETE: AtomicBool = AtomicBool::new(true);
static PR_ACTIVITY: AtomicBool = AtomicBool::new(true);

pub fn parse_turn_complete(raw: Option<&str>) -> bool {
    raw != Some("false")
}

pub fn set_turn_complete(enabled: bool) {
    TURN_COMPLETE.store(enabled, Ordering::Relaxed);
}

fn turn_complete_enabled() -> bool {
    TURN_COMPLETE.load(Ordering::Relaxed)
}

pub fn parse_pr_activity(raw: Option<&str>) -> bool {
    raw != Some("false")
}

pub fn set_pr_activity(enabled: bool) {
    PR_ACTIVITY.store(enabled, Ordering::Relaxed);
}

fn pr_activity_enabled() -> bool {
    PR_ACTIVITY.load(Ordering::Relaxed)
}

/// The `kind`s and their titles, verbatim from the protocol doc.
const KIND_TURN_COMPLETE: &str = "turn_complete";
const KIND_NEEDS_INPUT: &str = "needs_input";
const KIND_CHECKS_SETTLED: &str = "checks_settled";
const KIND_REVIEW_COMMENT: &str = "review_comment";
const KIND_PR_MERGED: &str = "pr_merged";
const KIND_PR_CLOSED: &str = "pr_closed";
const KIND_AUTOPILOT_GAVE_UP: &str = "autopilot_gave_up";
const TITLE_TURN_COMPLETE: &str = "Turn complete";
const TITLE_NEEDS_INPUT: &str = "Needs your input";
const TITLE_CHECKS_PASSED: &str = "Checks passed";
const TITLE_CHECKS_FAILED: &str = "Checks failed";
const TITLE_REVIEW_COMMENT: &str = "New review comment";
const TITLE_PR_MERGED: &str = "PR merged";
const TITLE_PR_CLOSED: &str = "PR closed";
/// Followed by ` · <rung>`.
const TITLE_AUTOPILOT_GAVE_UP: &str = "Autopilot gave up";

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
    /// The rollup each PR's last `pr:checks_changed` carried, keyed by the
    /// event's agent/subdir plus its PR number, so a `failing → failing` with a
    /// different set of names is not a second alert while `failing → passing`
    /// is — and the next PR on the same checkout starts with no memory. Entries
    /// go when their PR leaves `open`.
    last_rollup: Mutex<HashMap<String, String>>,
    /// The state each checkout's last `pr:state_changed` carried, keyed like
    /// the client stores (`agent` for the primary repo, `agent::subdir` for a
    /// secondary) so one repo's merge is not read as another's. A merge or close
    /// alerts only when this saw the PR open: a cold start's first event may be
    /// a stale snapshot of a PR that merged last week.
    last_pr_state: Mutex<HashMap<String, String>>,
}

impl PushTriggers {
    pub(super) fn new(state: Arc<RemoteState>, focused: FocusCheck, notify: NotifySink) -> Self {
        Self {
            state,
            focused,
            notify,
            running: Mutex::new(HashSet::new()),
            pending_input: Mutex::new(HashSet::new()),
            last_rollup: Mutex::new(HashMap::new()),
            last_pr_state: Mutex::new(HashMap::new()),
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
        self.alert(
            agents,
            agent_id,
            KIND_TURN_COMPLETE,
            TITLE_TURN_COMPLETE,
            None,
        );
    }

    /// Route one event off the sink. These six names are the whole surface;
    /// every other event the engine emits passes through untouched.
    pub(super) fn on_event(&self, agents: &dyn AgentLookup, event: &str, payload: &Value) {
        match event {
            "agent:status" => {
                if let Ok(status) = StatusPayload::deserialize(payload) {
                    self.on_status(agents, &status.agent_id, &status.status);
                }
            }
            "agent:event" => self.on_agent_event(agents, payload),
            "pr:checks_changed" => self.on_checks_changed(agents, payload),
            "pr:threads_changed" => self.on_threads_changed(agents, payload),
            "pr:state_changed" => self.on_pr_state_changed(agents, payload),
            "autopilot:event" => self.on_autopilot_event(agents, payload),
            _ => {}
        }
    }

    /// One `autopilot:event`. Only a `give-up` alerts: it is the one thing
    /// autopilot did that someone has to act on, and the rest is the loop
    /// working as it should.
    fn on_autopilot_event(&self, agents: &dyn AgentLookup, payload: &Value) {
        let Ok(payload) = AutopilotEventPayload::deserialize(payload) else {
            return;
        };
        if payload.outcome != "give-up" || !pr_activity_enabled() {
            return;
        }
        let title = format!("{TITLE_AUTOPILOT_GAVE_UP} · {}", payload.rung);
        let reason = payload.reason.as_deref().map(|r| r.replace('-', " "));
        self.alert(
            agents,
            &payload.agent_id,
            KIND_AUTOPILOT_GAVE_UP,
            &title,
            reason.as_deref(),
        );
    }

    /// One `pr:checks_changed`. Alerts when the rollup *settles* — lands on
    /// `passing` or `failing` from anything else, an unseen PR included. The
    /// watcher also emits for a changed set of failing names under the same
    /// rollup; that is the phone's list to refresh, not a second alert.
    fn on_checks_changed(&self, agents: &dyn AgentLookup, payload: &Value) {
        let Ok(payload) = ChecksChangedPayload::deserialize(payload) else {
            return;
        };
        // Keyed by PR, not just by checkout: the next PR on the same branch
        // settling green is its own news, not a repeat of the last one's.
        let key = format!(
            "{}#{}",
            checkout_key(&payload.agent_id, payload.subdir.as_deref()),
            payload.number
        );
        let rollup = payload.checks.rollup;
        let previous = self.last_rollup.lock().insert(key, rollup.clone());
        let settled = matches!(rollup.as_str(), "passing" | "failing");
        if !settled || previous.as_deref() == Some(rollup.as_str()) {
            return;
        }
        if !pr_activity_enabled() {
            return;
        }
        let (title, detail) = if rollup == "passing" {
            (TITLE_CHECKS_PASSED, None)
        } else {
            (
                TITLE_CHECKS_FAILED,
                payload.checks.required_failing.first().map(String::as_str),
            )
        };
        self.alert(
            agents,
            &payload.agent_id,
            KIND_CHECKS_SETTLED,
            title,
            detail,
        );
    }

    /// One `pr:threads_changed`. The watcher already did the diffing — the
    /// event names the new ids — so this only needs the first one's author.
    fn on_threads_changed(&self, agents: &dyn AgentLookup, payload: &Value) {
        let Ok(payload) = ThreadsChangedPayload::deserialize(payload) else {
            return;
        };
        let Some(first) = payload.new_thread_ids.first() else {
            return;
        };
        if !pr_activity_enabled() {
            return;
        }
        let author = payload
            .comments
            .unresolved
            .iter()
            .find(|thread| &thread.id == first)
            .map(|thread| thread.author.as_str());
        self.alert(
            agents,
            &payload.agent_id,
            KIND_REVIEW_COMMENT,
            TITLE_REVIEW_COMMENT,
            author,
        );
    }

    /// One `pr:state_changed`. The event fires from several paths (a turn end,
    /// a push, the watcher) and each reports the state it found, not a
    /// transition — so the transition is reconstructed here, per checkout, and
    /// only an `open → merged|closed` this process witnessed is a merge or a
    /// close. Becoming open is never an alert: the watcher reports every open
    /// PR on its first look, a host restart included.
    ///
    /// Two repos of one agent merging are two alerts, but they share the
    /// agent's `collapseId`, so the phone shows the later banner in place of
    /// the earlier one rather than stacking them.
    fn on_pr_state_changed(&self, agents: &dyn AgentLookup, payload: &Value) {
        let Ok(payload) = PrStateChangedPayload::deserialize(payload) else {
            return;
        };
        let checkout = checkout_key(&payload.agent_id, payload.subdir.as_deref());
        let previous = {
            let mut last = self.last_pr_state.lock();
            match &payload.state {
                Some(state) => last.insert(checkout.clone(), state.state.clone()),
                None => last.remove(&checkout),
            }
        };
        // A PR that is no longer open takes its rollup memory with it, so the
        // map stays bounded by the PRs still being watched. Only this
        // checkout's: the agent's other repos are still open.
        if payload.state.as_ref().map(|s| s.state.as_str()) != Some("open") {
            self.last_rollup.lock().retain(|key, _| {
                !matches!(key.strip_prefix(checkout.as_str()), Some(rest) if rest.starts_with('#'))
            });
        }
        let Some(state) = payload.state else {
            return;
        };
        if previous.as_deref() != Some("open") {
            return;
        }
        let (kind, title) = match state.state.as_str() {
            "merged" => (KIND_PR_MERGED, TITLE_PR_MERGED),
            "closed" => (KIND_PR_CLOSED, TITLE_PR_CLOSED),
            _ => return,
        };
        if !pr_activity_enabled() {
            return;
        }
        let number = format!("#{}", state.number);
        self.alert(agents, &payload.agent_id, kind, title, Some(&number));
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
            None,
        );
    }

    /// Build and send one alert, unless the user is already looking at it or no
    /// device could receive it. The body is the agent's name, joined with
    /// ` · ` to `detail` when a kind has one (a check name, a reviewer, a PR
    /// number).
    fn alert(
        &self,
        agents: &dyn AgentLookup,
        agent_id: &str,
        kind: &str,
        title: &str,
        detail: Option<&str>,
    ) {
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
        let name = agents
            .agent_name(agent_id)
            .unwrap_or_else(|| UNNAMED_AGENT.to_string());
        let body = match detail {
            Some(detail) => format!("{name} · {detail}"),
            None => name,
        };
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

/// One checkout's key from an event's `subdir` (`None` = the primary repo).
fn checkout_key(agent_id: &str, subdir: Option<&str>) -> String {
    crate::supervisor::pr_map_key(agent_id, subdir.unwrap_or_default(), subdir.is_none())
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

/// The triggers hanging off the engine's emits. See the module doc for why
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

/// `supervisor::events::PrChecksChangedPayload`, the part of it this needs.
#[derive(Deserialize)]
struct ChecksChangedPayload {
    agent_id: String,
    subdir: Option<String>,
    #[serde(default)]
    number: u32,
    checks: ChecksSummary,
}

#[derive(Deserialize)]
struct ChecksSummary {
    rollup: String,
    #[serde(default)]
    required_failing: Vec<String>,
}

/// `supervisor::events::PrThreadsChangedPayload`, the part of it this needs.
#[derive(Deserialize)]
struct ThreadsChangedPayload {
    agent_id: String,
    #[serde(default)]
    comments: ThreadsSummary,
    #[serde(default)]
    new_thread_ids: Vec<String>,
}

#[derive(Deserialize, Default)]
struct ThreadsSummary {
    #[serde(default)]
    unresolved: Vec<ThreadSummary>,
}

#[derive(Deserialize)]
struct ThreadSummary {
    id: String,
    author: String,
}

/// `supervisor::events::PrStateChangedPayload`, the part of it this needs.
#[derive(Deserialize)]
struct PrStateChangedPayload {
    agent_id: String,
    /// Absent from a host that predates it, which only ever meant the primary.
    #[serde(default)]
    subdir: Option<String>,
    state: Option<PrStateSummary>,
}

#[derive(Deserialize)]
struct PrStateSummary {
    number: u64,
    state: String,
}

/// `autopilot::LogEntry`, the part of it this needs.
#[derive(Deserialize)]
struct AutopilotEventPayload {
    agent_id: String,
    outcome: String,
    rung: String,
    #[serde(default)]
    reason: Option<String>,
}
