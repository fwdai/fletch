//! Per-agent activity tracking — the abstraction that decides whether a
//! turn is open.
//!
//! Each running agent owns one `Activity` instance. The supervisor
//! feeds it whatever signal the agent's output channel produces (PTY
//! bytes for native view, stream-json events for custom view) and asks
//! "has the turn ended?". The answer follows the output in both
//! directions: a terminal event closes the turn, and main-line output
//! after that opens one again — the provider may start a turn Fletch
//! never asked for (Claude answering a finished background task), and
//! the status has to say so. The state machine is provider-agnostic;
//! only the constructors know provider-specific event shapes. Adding a
//! provider means one more constructor, not touching the supervisor.

use std::time::{Duration, Instant};

use serde_json::Value;

/// Native-mode silence threshold. Claude's TUI emits continuous
/// redraw bytes (spinners, progress lines) while it's working, so a
/// pause this long indicates the prompt is sitting idle.
const NATIVE_SILENCE_THRESHOLD: Duration = Duration::from_millis(2500);

pub trait Activity: Send {
    /// Feed a PTY chunk to the detector. Default does nothing — only
    /// native-mode impls care about raw bytes.
    fn observe_bytes(&mut self, _bytes: &[u8]) {}

    /// Feed a structured event to the detector. Returns true when the event
    /// is main-line turn output — proof that a turn is open *right now*,
    /// whoever started it. Terminal events, sidechain (sub-agent) events and
    /// control-plane chatter return false. Default does nothing and reports
    /// nothing — only managed-mode impls care.
    fn observe_event(&mut self, _event: &Value) -> bool {
        false
    }

    /// Whether the current turn has ended: the provider said so (managed) or
    /// the output went quiet (native), and nothing has opened a turn since.
    fn turn_ended(&self) -> bool;

    /// The provider's own report that the current turn failed, when it made
    /// one: codex's `turn.failed`, claude's `is_error` result, opencode's
    /// `error` event, pi's error stop. These turns end in band and the process
    /// may even exit cleanly, so without this the turn would read as a
    /// success. The text is the provider's message, kept on the host — the
    /// supervisor classifies it, never forwards it. Default: no such report
    /// (native PTY output has no structured failure).
    fn turn_failure(&self) -> Option<&str> {
        None
    }

    /// Called when a new user turn is submitted. Resets the detector
    /// so any prior explicit-end flag or stale silence doesn't
    /// immediately mark the new turn as ended.
    fn reset_for_new_turn(&mut self);
}

/// The message of an event that reports a failed turn, or `None` for any
/// other event. One per provider, like `is_turn_end`.
type FailureOf = fn(&Value) -> Option<String>;

fn str_at<'a>(event: &'a Value, pointer: &str) -> Option<&'a str> {
    event.pointer(pointer).and_then(Value::as_str)
}

fn event_type(event: &Value) -> Option<&str> {
    event.get("type").and_then(Value::as_str)
}

/// Custom view: agents that stream structured events and signal end-of-turn
/// with one specific event. The turn-end *signal* is the only thing that
/// varies between providers, so it's injected as a predicate; the rest of the
/// state machine is shared: the terminal event closes the turn, any later
/// main-line output opens it again. Construct via the provider helpers below.
pub struct ManagedActivity {
    /// The terminal event has been seen and no main-line output followed it.
    ended: bool,
    /// Returns true for the event that marks the end of a turn.
    is_turn_end: fn(&Value) -> bool,
    /// Returns the failure message of an event that reports one.
    failure_of: FailureOf,
    /// The failure the provider reported for the current turn, if any.
    failure: Option<String>,
}

/// Sub-agent (sidechain) events carry the spawning Task/Agent tool's id in a
/// top-level `parent_tool_use_id`. They belong to a nested turn, not the main
/// one: a sub-agent's `result` must not end the main turn, and its output must
/// not open one. (The frontend mirrors this via `parent_tool_use_id` routing
/// in reduce.ts.)
fn is_sidechain(event: &Value) -> bool {
    event
        .get("parent_tool_use_id")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty())
}

/// Control-plane events that say nothing about whether a turn is open:
/// Claude's `system` family — `init` at process start, the `task_*` lifecycle
/// of background sub-agents and background Bash, hook output. These arrive
/// between turns as a matter of course, so they must not open one. The turn a
/// provider starts on its own announces itself with ordinary `user` /
/// `assistant` output, which does.
fn is_control_plane(event: &Value) -> bool {
    event.get("type").and_then(|v| v.as_str()) == Some("system")
}

impl ManagedActivity {
    fn new(is_turn_end: fn(&Value) -> bool, failure_of: FailureOf) -> Self {
        Self {
            ended: false,
            is_turn_end,
            failure_of,
            failure: None,
        }
    }

    /// Claude (`--print --output-format stream-json`) ends a turn with a
    /// `result` event; one with `is_error: true` (subtype
    /// `error_during_execution`, `error_max_turns`, …) is a failed turn whose
    /// `result` text says why. Cursor emits Claude-shaped stream-json, so it
    /// shares this detector.
    pub fn claude() -> Self {
        Self::new(
            |event| event_type(event) == Some("result"),
            |event| {
                if event_type(event) != Some("result")
                    || event.get("is_error").and_then(Value::as_bool) != Some(true)
                {
                    return None;
                }
                Some(
                    str_at(event, "/result")
                        .or_else(|| str_at(event, "/subtype"))
                        .unwrap_or("result reported an error")
                        .to_string(),
                )
            },
        )
    }

    /// Codex (`codex exec --json`) ends a turn with `turn.completed`, or with
    /// `turn.failed` when the model call errored (usage limit, auth, 4xx) —
    /// after which the process exits non-zero. Both are the turn's explicit
    /// end; the supervisor uses that to tell a failed turn from a crash. (The
    /// per-turn process exit is handled separately and does not feed this
    /// detector.) The failure's message rides `turn.failed` (and the `error`
    /// event that precedes it): `error.message`, or a bare `message`.
    pub fn codex() -> Self {
        Self::new(
            |event| {
                matches!(
                    event_type(event),
                    Some("turn.completed") | Some("turn.failed")
                )
            },
            |event| {
                if !matches!(event_type(event), Some("turn.failed") | Some("error")) {
                    return None;
                }
                Some(
                    str_at(event, "/error/message")
                        .or_else(|| str_at(event, "/message"))
                        .unwrap_or("turn failed")
                        .to_string(),
                )
            },
        )
    }

    /// OpenCode (`opencode run --format json`) emits one or more `step_finish`
    /// events per turn (one per reasoning/tool step); only the final one
    /// carries `part.reason == "stop"`. Intermediate steps use `tool-calls`
    /// and must not be treated as turn-end. A top-level `error` event
    /// (`error.name` + `error.data.message`, e.g. `UnknownError`) is the turn
    /// failing; the process then exits 1 with nothing on stderr.
    pub fn opencode() -> Self {
        Self::new(
            |event| {
                event_type(event) == Some("step_finish")
                    && str_at(event, "/part/reason") == Some("stop")
            },
            |event| {
                if event_type(event) != Some("error") {
                    return None;
                }
                let name = str_at(event, "/error/name");
                let message =
                    str_at(event, "/error/data/message").or_else(|| str_at(event, "/message"));
                Some(match (name, message) {
                    (Some(n), Some(m)) => format!("{n}: {m}"),
                    (Some(n), None) => n.to_string(),
                    (None, Some(m)) => m.to_string(),
                    (None, None) => "OpenCode reported an error".to_string(),
                })
            },
        )
    }

    /// Pi (`pi -p --mode json`) emits a `turn_end` per assistant step (so it
    /// fires mid-turn after a tool call); the whole turn ends with a single
    /// `agent_end`, which is the signal we key on. A failed model call is a
    /// `message_end` whose assistant message stopped with reason `error`
    /// (`errorMessage` says why); pi then still emits `agent_end` and exits 0.
    pub fn pi() -> Self {
        Self::new(
            |event| event_type(event) == Some("agent_end"),
            |event| {
                if event_type(event) != Some("message_end")
                    || str_at(event, "/message/role") != Some("assistant")
                    || str_at(event, "/message/stopReason") != Some("error")
                {
                    return None;
                }
                Some(
                    str_at(event, "/message/errorMessage")
                        .unwrap_or("assistant message stopped with an error")
                        .to_string(),
                )
            },
        )
    }
}

impl Activity for ManagedActivity {
    fn observe_event(&mut self, event: &Value) -> bool {
        if is_sidechain(event) {
            return false;
        }
        let failure = (self.failure_of)(event);
        if (self.is_turn_end)(event) {
            self.ended = true;
            if failure.is_some() {
                self.failure = failure;
            }
            return false;
        }
        if is_control_plane(event) {
            return false;
        }
        // Main-line output. A turn is open — the one Fletch started, or one the
        // provider began on its own after the last terminal event; a turn the
        // provider opened is a new one, so the last turn's failure is not its.
        if self.ended {
            self.failure = None;
        }
        self.ended = false;
        if failure.is_some() {
            self.failure = failure;
        }
        true
    }

    fn turn_ended(&self) -> bool {
        // Structured providers define an explicit terminal event. Silence is
        // not completion: reasoning, tools, and provider-side work can all be
        // quiet for an arbitrary amount of time before more events arrive.
        self.ended
    }

    fn turn_failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    fn reset_for_new_turn(&mut self) {
        self.ended = false;
        self.failure = None;
    }
}

/// Native view: claude runs in a PTY rendering its full TUI. There's
/// no clean external turn-end event, so we use the silence between
/// PTY chunks. Claude's TUI animates its "working" state with
/// frequent redraws, so silence longer than the threshold is our best
/// available heuristic. The supervisor corrects it if output resumes.
#[derive(Default)]
pub struct ClaudeNativeActivity {
    last_byte_at: Option<Instant>,
}

impl ClaudeNativeActivity {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Activity for ClaudeNativeActivity {
    fn observe_bytes(&mut self, _bytes: &[u8]) {
        self.last_byte_at = Some(Instant::now());
    }

    fn turn_ended(&self) -> bool {
        self.last_byte_at
            .map(|t| t.elapsed() >= NATIVE_SILENCE_THRESHOLD)
            .unwrap_or(false)
    }

    fn reset_for_new_turn(&mut self) {
        self.last_byte_at = Some(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn managed_ends_on_result_event() {
        let mut a = ManagedActivity::claude();
        assert!(!a.turn_ended());
        a.observe_event(&serde_json::json!({"type": "assistant"}));
        assert!(!a.turn_ended());
        a.observe_event(&serde_json::json!({"type": "result", "subtype": "success"}));
        assert!(a.turn_ended());
    }

    #[test]
    fn managed_resets_after_new_turn() {
        let mut a = ManagedActivity::claude();
        a.observe_event(&serde_json::json!({"type": "result", "is_error": true, "result": "boom"}));
        assert!(a.turn_ended());
        assert_eq!(a.turn_failure(), Some("boom"));
        a.reset_for_new_turn();
        assert!(!a.turn_ended());
        assert_eq!(a.turn_failure(), None);
    }

    /// Each provider's in-band failure is read off its own event, with its
    /// message; a clean turn end reports none.
    #[test]
    fn each_provider_reports_its_in_band_failure() {
        use serde_json::json;

        let mut claude = ManagedActivity::claude();
        claude.observe_event(&json!({"type": "assistant"}));
        claude.observe_event(&json!({"type": "result", "subtype": "success", "is_error": false}));
        assert!(claude.turn_ended());
        assert_eq!(claude.turn_failure(), None);
        claude.reset_for_new_turn();
        claude.observe_event(&json!({"type": "result", "subtype": "error_max_turns",
                                     "is_error": true, "result": "Reached max turns (3)"}));
        assert_eq!(claude.turn_failure(), Some("Reached max turns (3)"));

        // codex: `error` then `turn.failed` with the same message, then exit 1.
        let mut codex = ManagedActivity::codex();
        codex.observe_event(&json!({"type": "thread.started"}));
        codex.observe_event(&json!({"type": "error", "message": "usage limit reached"}));
        assert!(!codex.turn_ended());
        codex.observe_event(&json!({"type": "turn.failed",
                                    "error": {"message": "usage limit reached"}}));
        assert!(codex.turn_ended());
        assert_eq!(codex.turn_failure(), Some("usage limit reached"));

        // opencode 1.18: an `error` event, no step_finish, then exit 1.
        let mut opencode = ManagedActivity::opencode();
        opencode.observe_event(&json!({"type": "error", "sessionID": "ses_1",
            "error": {"name": "UnknownError",
                      "data": {"message": "Unexpected server error. Check server logs for details."}}}));
        assert!(!opencode.turn_ended());
        assert_eq!(
            opencode.turn_failure(),
            Some("UnknownError: Unexpected server error. Check server logs for details.")
        );

        // pi: the assistant message stops with `error`, then `agent_end`, exit 0.
        let mut pi = ManagedActivity::pi();
        pi.observe_event(
            &json!({"type": "message_end", "message": {"role": "assistant",
            "content": [], "stopReason": "error", "errorMessage": "429 Rate limit exceeded"}}),
        );
        pi.observe_event(&json!({"type": "agent_end"}));
        assert!(pi.turn_ended());
        assert_eq!(pi.turn_failure(), Some("429 Rate limit exceeded"));
        // A tool result that errored is the model's business, not a failed turn.
        pi.reset_for_new_turn();
        pi.observe_event(
            &json!({"type": "message_end", "message": {"role": "toolResult",
            "isError": true, "content": [{"type": "text", "text": "exit 1"}]}}),
        );
        pi.observe_event(&json!({"type": "agent_end"}));
        assert_eq!(pi.turn_failure(), None);
    }

    /// A turn the provider opens on its own after a failed one starts clean.
    #[test]
    fn a_provider_started_turn_does_not_inherit_the_last_failure() {
        let mut a = ManagedActivity::claude();
        a.observe_event(&serde_json::json!({"type": "result", "is_error": true, "result": "boom"}));
        assert_eq!(a.turn_failure(), Some("boom"));
        a.observe_event(&serde_json::json!({"type": "assistant"}));
        assert!(!a.turn_ended());
        assert_eq!(a.turn_failure(), None);
    }

    #[test]
    fn managed_non_terminal_events_never_end_turn() {
        let mut a = ManagedActivity::claude();
        a.observe_event(&serde_json::json!({
            "type": "assistant",
            "message": {"content": [{"type": "tool_use", "id": "toolu_1", "name": "Bash"}]}
        }));
        assert!(!a.turn_ended());
        a.observe_event(&serde_json::json!({
            "type": "user",
            "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_1"}]}
        }));
        assert!(!a.turn_ended());
    }

    #[test]
    fn managed_result_ends_after_tool_activity() {
        let mut a = ManagedActivity::claude();
        a.observe_event(&serde_json::json!({
            "type": "assistant",
            "message": {"content": [{"type": "tool_use", "id": "toolu_1"}]}
        }));
        a.observe_event(&serde_json::json!({"type": "result", "subtype": "success"}));
        assert!(a.turn_ended());
    }

    #[test]
    fn managed_ignores_subagent_sidechain_events() {
        // The main agent spawns a subagent via a Task tool call.
        let mut a = ManagedActivity::claude();
        a.observe_event(&serde_json::json!({
            "type": "assistant",
            "message": {"content": [{"type": "tool_use", "id": "toolu_task", "name": "Task"}]}
        }));
        assert!(!a.turn_ended());

        // The subagent runs, emitting its own tool cycle and finally a `result`,
        // all tagged with the spawning tool's id. None of it must end the main
        // turn.
        a.observe_event(&serde_json::json!({
            "type": "assistant",
            "parent_tool_use_id": "toolu_task",
            "message": {"content": [{"type": "tool_use", "id": "toolu_sub", "name": "Bash"}]}
        }));
        a.observe_event(&serde_json::json!({
            "type": "result",
            "parent_tool_use_id": "toolu_task",
            "subtype": "success"
        }));
        // Subagent's result must NOT end the main turn.
        assert!(!a.turn_ended());

        // The main agent finally gets the Task tool_result, then ends its turn.
        a.observe_event(&serde_json::json!({
            "type": "user",
            "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_task"}]}
        }));
        a.observe_event(&serde_json::json!({"type": "result", "subtype": "success"}));
        assert!(a.turn_ended());
    }

    #[test]
    fn managed_main_line_output_after_the_terminal_event_reopens_the_turn() {
        // Claude ends the user's turn while a background sub-agent is still
        // running, then starts a turn of its own to answer the task's
        // notification. That turn is ordinary main-line output and must count.
        let mut a = ManagedActivity::claude();
        a.observe_event(&serde_json::json!({"type": "result", "subtype": "success"}));
        assert!(a.turn_ended());

        let opened = a.observe_event(&serde_json::json!({
            "type": "user",
            "message": {"role": "user", "content": "<task-notification>done</task-notification>"}
        }));
        assert!(opened);
        assert!(!a.turn_ended());

        assert!(a.observe_event(&serde_json::json!({"type": "assistant"})));
        assert!(!a.turn_ended());

        // Its own `result` closes it again.
        assert!(!a.observe_event(&serde_json::json!({"type": "result", "subtype": "success"})));
        assert!(a.turn_ended());
    }

    #[test]
    fn managed_control_plane_and_sidechain_events_never_open_a_turn() {
        let mut a = ManagedActivity::claude();
        a.observe_event(&serde_json::json!({"type": "result", "subtype": "success"}));

        // Background-task lifecycle on the main stream: not a turn.
        for subtype in [
            "task_started",
            "task_progress",
            "task_notification",
            "background_tasks_changed",
            "init",
        ] {
            assert!(!a.observe_event(&serde_json::json!({"type": "system", "subtype": subtype})));
            assert!(a.turn_ended(), "{subtype} must not reopen the turn");
        }
        // A sub-agent streaming after the main turn: not the main turn.
        assert!(!a.observe_event(&serde_json::json!({
            "type": "assistant", "parent_tool_use_id": "toolu_bg"
        })));
        assert!(a.turn_ended());
    }

    #[test]
    fn managed_reports_main_line_activity_while_the_turn_runs() {
        let mut a = ManagedActivity::claude();
        a.reset_for_new_turn();
        assert!(a.observe_event(&serde_json::json!({"type": "stream_event", "event": {}})));
        assert!(a.observe_event(&serde_json::json!({"type": "assistant"})));
        assert!(!a.observe_event(&serde_json::json!({"type": "system", "subtype": "init"})));
        assert!(!a.turn_ended());
    }

    #[test]
    fn codex_ends_on_turn_completed_event() {
        let mut a = ManagedActivity::codex();
        assert!(!a.turn_ended());
        a.observe_event(&serde_json::json!({"type": "turn.started"}));
        assert!(!a.turn_ended());
        a.observe_event(&serde_json::json!({"type": "item.completed"}));
        assert!(!a.turn_ended());
        a.observe_event(&serde_json::json!({"type": "turn.completed", "usage": {}}));
        assert!(a.turn_ended());
    }

    #[test]
    fn codex_ends_on_turn_failed_event() {
        let mut a = ManagedActivity::codex();
        a.observe_event(&serde_json::json!({"type": "turn.started"}));
        a.observe_event(&serde_json::json!({"type": "error", "message": "usage limit"}));
        assert!(!a.turn_ended());
        a.observe_event(&serde_json::json!({"type": "turn.failed", "error": {"message": "x"}}));
        assert!(a.turn_ended());
    }

    #[test]
    fn codex_resets_after_new_turn() {
        let mut a = ManagedActivity::codex();
        a.observe_event(&serde_json::json!({"type": "turn.completed"}));
        assert!(a.turn_ended());
        a.reset_for_new_turn();
        assert!(!a.turn_ended());
    }

    #[test]
    fn opencode_ends_only_on_step_finish_stop() {
        let mut a = ManagedActivity::opencode();
        assert!(!a.turn_ended());
        a.observe_event(&serde_json::json!({"type": "step_start"}));
        assert!(!a.turn_ended());
        a.observe_event(&serde_json::json!({"type": "tool_use"}));
        assert!(!a.turn_ended());
        // Intermediate step finishing for a tool call must NOT end the turn.
        a.observe_event(
            &serde_json::json!({"type": "step_finish", "part": {"reason": "tool-calls"}}),
        );
        assert!(!a.turn_ended());
        // The final step stops.
        a.observe_event(&serde_json::json!({"type": "step_finish", "part": {"reason": "stop"}}));
        assert!(a.turn_ended());
    }

    #[test]
    fn opencode_resets_after_new_turn() {
        let mut a = ManagedActivity::opencode();
        a.observe_event(&serde_json::json!({"type": "step_finish", "part": {"reason": "stop"}}));
        assert!(a.turn_ended());
        a.reset_for_new_turn();
        assert!(!a.turn_ended());
    }

    #[test]
    fn pi_ends_on_agent_end_not_turn_end() {
        let mut a = ManagedActivity::pi();
        assert!(!a.turn_ended());
        a.observe_event(&serde_json::json!({"type": "session", "id": "x"}));
        assert!(!a.turn_ended());
        // A mid-turn `turn_end` (after a tool step) must NOT end the turn.
        a.observe_event(&serde_json::json!({"type": "turn_end"}));
        assert!(!a.turn_ended());
        a.observe_event(&serde_json::json!({"type": "agent_end"}));
        assert!(a.turn_ended());
    }

    #[test]
    fn pi_resets_after_new_turn() {
        let mut a = ManagedActivity::pi();
        a.observe_event(&serde_json::json!({"type": "agent_end"}));
        assert!(a.turn_ended());
        a.reset_for_new_turn();
        assert!(!a.turn_ended());
    }

    #[test]
    fn native_ends_after_silence() {
        // Run with a much shorter threshold so the test stays fast.
        let mut a = ClaudeNativeActivity::new();
        a.observe_bytes(b"hello");
        // Just-observed — definitely still active.
        assert!(!a.turn_ended());
        // Past the native threshold (2.5s) would take too long to wait
        // in a unit test — instead, sanity-check the time math with a
        // freshly-zero detector.
        let stale = ClaudeNativeActivity {
            last_byte_at: Some(Instant::now() - Duration::from_secs(5)),
        };
        assert!(stale.turn_ended());
    }

    #[test]
    fn empty_native_does_not_claim_turn_ended() {
        // Fresh detector with no observed bytes shouldn't claim
        // turn-end — otherwise a just-spawned process would
        // immediately be flagged idle before any real activity.
        let a = ClaudeNativeActivity::new();
        assert!(!a.turn_ended());
        // (Sleep just to make sure elapsed-on-None is the path used.)
        sleep(Duration::from_millis(10));
        assert!(!a.turn_ended());
    }
}
