//! Per-agent activity tracking — the abstraction that decides when a
//! turn has ended.
//!
//! Each running agent owns one `Activity` instance. The supervisor
//! feeds it whatever signal the agent's output channel produces (PTY
//! bytes for native view, stream-json events for custom view) and
//! periodically asks "has the turn ended?". The state machine is
//! provider-agnostic; only the impls know about claude-specific event
//! shapes. Adding gemini / codex / etc. means writing new impls
//! against this trait, not touching the supervisor.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde_json::Value;

/// Backstop threshold for the silence-based fallback. Generous on
/// purpose — explicit turn-end signals (e.g. claude's `result` event)
/// should fire well within this window. If they don't, the watchdog
/// catches it.
const MANAGED_SILENCE_BACKSTOP: Duration = Duration::from_secs(30);

/// Native-mode silence threshold. Claude's TUI emits continuous
/// redraw bytes (spinners, progress lines) while it's working, so a
/// pause this long indicates the prompt is sitting idle.
const NATIVE_SILENCE_THRESHOLD: Duration = Duration::from_millis(2500);

pub trait Activity: Send {
    /// Feed a PTY chunk to the detector. Default does nothing — only
    /// native-mode impls care about raw bytes.
    fn observe_bytes(&mut self, _bytes: &[u8]) {}

    /// Feed a structured event to the detector. Default does nothing —
    /// only managed-mode impls care.
    fn observe_event(&mut self, _event: &Value) {}

    /// Called every watchdog tick. Returns true if the current turn
    /// should be considered ended (claude has stopped responding).
    fn turn_ended(&self) -> bool;

    /// Called when a new user turn is submitted. Resets the detector
    /// so any prior explicit-end flag or stale silence doesn't
    /// immediately mark the new turn as ended.
    fn reset_for_new_turn(&mut self);
}

/// Custom view: agents that stream structured events and signal end-of-turn
/// with one specific event. The turn-end *signal* is the only thing that
/// varies between providers, so it's injected as a predicate; the rest of the
/// state machine (trust the explicit signal, fall back to silence if it never
/// fires) is shared. Construct via the provider helpers below.
pub struct ManagedActivity {
    last_event_at: Option<Instant>,
    explicit_turn_end: bool,
    /// Returns true for the event that marks the end of a turn.
    is_turn_end: fn(&Value) -> bool,
    /// Whether to track outstanding tool calls (Claude-shaped streams only).
    /// A long, quiet tool call (a build, a test run) emits no events while it
    /// runs, so the silence backstop would otherwise wrongly flag the turn as
    /// ended mid-tool. While a tool is outstanding we suppress that backstop.
    track_tools: bool,
    /// `tool_use` ids seen without a matching `tool_result` yet. Non-empty
    /// means a tool is mid-flight, so silence is expected, not turn-end.
    outstanding_tools: HashSet<String>,
}

impl ManagedActivity {
    fn new(is_turn_end: fn(&Value) -> bool) -> Self {
        Self {
            last_event_at: None,
            explicit_turn_end: false,
            is_turn_end,
            track_tools: false,
            outstanding_tools: HashSet::new(),
        }
    }

    /// Claude (`--print --output-format stream-json`) ends a turn with a
    /// `result` event. Cursor emits Claude-shaped stream-json, so it shares
    /// this detector.
    pub fn claude() -> Self {
        let mut a = Self::new(|event| event.get("type").and_then(|v| v.as_str()) == Some("result"));
        a.track_tools = true;
        a
    }

    /// Codex (`codex exec --json`) ends a turn with `turn.completed`. (The
    /// per-turn process exit is handled separately and does not feed this
    /// detector.)
    pub fn codex() -> Self {
        Self::new(|event| event.get("type").and_then(|v| v.as_str()) == Some("turn.completed"))
    }

    /// OpenCode (`opencode run --format json`) emits one or more `step_finish`
    /// events per turn (one per reasoning/tool step); only the final one
    /// carries `part.reason == "stop"`. Intermediate steps use `tool-calls`
    /// and must not be treated as turn-end.
    pub fn opencode() -> Self {
        Self::new(|event| {
            event.get("type").and_then(|v| v.as_str()) == Some("step_finish")
                && event
                    .get("part")
                    .and_then(|p| p.get("reason"))
                    .and_then(|r| r.as_str())
                    == Some("stop")
        })
    }

    /// Pi (`pi -p --mode json`) emits a `turn_end` per assistant step (so it
    /// fires mid-turn after a tool call); the whole turn ends with a single
    /// `agent_end`, which is the signal we key on.
    pub fn pi() -> Self {
        Self::new(|event| event.get("type").and_then(|v| v.as_str()) == Some("agent_end"))
    }
}

impl Activity for ManagedActivity {
    fn observe_event(&mut self, event: &Value) {
        // Subagent (sidechain) events carry the spawning Task/Agent tool's id in
        // a top-level `parent_tool_use_id`. They belong to a nested turn, not the
        // main one: a subagent's `result` must not end the main turn, and its
        // tool_use/tool_result pairs must not touch the main turn's outstanding
        // set. Ignore them entirely here — the main agent's own Task tool call
        // stays in `outstanding_tools` until its real `tool_result`, which keeps
        // `turn_ended()` false while the subagent runs. (The frontend mirrors
        // this via `parent_tool_use_id` routing in reduce.ts.)
        if event
            .get("parent_tool_use_id")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty())
        {
            return;
        }
        self.last_event_at = Some(Instant::now());
        if (self.is_turn_end)(event) {
            self.explicit_turn_end = true;
        }
        if self.track_tools {
            self.track_tool_lifecycle(event);
        }
    }

    fn turn_ended(&self) -> bool {
        if self.explicit_turn_end {
            return true;
        }
        // A tool call is in flight (e.g. a long build/test). It emits no events
        // while it runs, so the silence below is expected — not a finished turn.
        if !self.outstanding_tools.is_empty() {
            return false;
        }
        self.last_event_at
            .map(|t| t.elapsed() >= MANAGED_SILENCE_BACKSTOP)
            .unwrap_or(false)
    }

    fn reset_for_new_turn(&mut self) {
        self.last_event_at = Some(Instant::now());
        self.explicit_turn_end = false;
        self.outstanding_tools.clear();
    }
}

impl ManagedActivity {
    /// Track Claude-shaped tool calls: an `assistant` message's `tool_use`
    /// blocks open a tool; the matching `user` message's `tool_result` blocks
    /// close it. Used only to know whether silence is "waiting on a tool" vs
    /// "turn done".
    fn track_tool_lifecycle(&mut self, event: &Value) {
        let Some(kind) = event.get("type").and_then(|v| v.as_str()) else {
            return;
        };
        let blocks = event
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array());
        let Some(blocks) = blocks else { return };
        for block in blocks {
            match (kind, block.get("type").and_then(|v| v.as_str())) {
                ("assistant", Some("tool_use")) => {
                    if let Some(id) = block.get("id").and_then(|v| v.as_str()) {
                        self.outstanding_tools.insert(id.to_string());
                    }
                }
                ("user", Some("tool_result")) => {
                    if let Some(id) = block.get("tool_use_id").and_then(|v| v.as_str()) {
                        self.outstanding_tools.remove(id);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Native view: claude runs in a PTY rendering its full TUI. There's
/// no clean external turn-end event, so we use the silence between
/// PTY chunks. Claude's TUI animates its "working" state with
/// frequent redraws, so silence longer than the threshold is a
/// dependable signal that the prompt is back to idle.
pub struct ClaudeNativeActivity {
    last_byte_at: Option<Instant>,
}

impl ClaudeNativeActivity {
    pub fn new() -> Self {
        Self { last_byte_at: None }
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
        a.observe_event(&serde_json::json!({"type": "result"}));
        assert!(a.turn_ended());
        a.reset_for_new_turn();
        assert!(!a.turn_ended());
    }

    #[test]
    fn managed_suppresses_silence_while_tool_outstanding() {
        let mut a = ManagedActivity::claude();
        // Assistant opens a tool call; no result yet.
        a.observe_event(&serde_json::json!({
            "type": "assistant",
            "message": {"content": [{"type": "tool_use", "id": "toolu_1", "name": "Bash"}]}
        }));
        // Simulate a long, quiet tool run well past the silence backstop.
        a.last_event_at = Some(Instant::now() - MANAGED_SILENCE_BACKSTOP - Duration::from_secs(5));
        // Tool still outstanding → silence must NOT be read as turn-end.
        assert!(!a.turn_ended());
        // Tool result arrives → tool closed; silence now genuinely means idle.
        a.observe_event(&serde_json::json!({
            "type": "user",
            "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_1"}]}
        }));
        a.last_event_at = Some(Instant::now() - MANAGED_SILENCE_BACKSTOP - Duration::from_secs(5));
        assert!(a.turn_ended());
    }

    #[test]
    fn managed_result_ends_even_with_outstanding_tool() {
        // An explicit turn-end always wins over outstanding-tool suppression.
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
        // turn or leak into the main outstanding-tool set.
        a.observe_event(&serde_json::json!({
            "type": "assistant",
            "parent_tool_use_id": "toolu_task",
            "message": {"content": [{"type": "tool_use", "id": "toolu_sub", "name": "Bash"}]}
        }));
        assert_eq!(a.outstanding_tools.len(), 1); // only the main Task call
        a.observe_event(&serde_json::json!({
            "type": "result",
            "parent_tool_use_id": "toolu_task",
            "subtype": "success"
        }));
        // Subagent's result must NOT end the main turn — the Task call is still
        // outstanding.
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
    fn managed_reset_clears_outstanding_tools() {
        let mut a = ManagedActivity::claude();
        a.observe_event(&serde_json::json!({
            "type": "assistant",
            "message": {"content": [{"type": "tool_use", "id": "toolu_1"}]}
        }));
        assert!(!a.outstanding_tools.is_empty());
        a.reset_for_new_turn();
        assert!(a.outstanding_tools.is_empty());
    }

    #[test]
    fn codex_does_not_track_tools() {
        // Only the Claude-shaped detector tracks tool lifecycle; others ignore
        // the (foreign-shaped) blocks entirely.
        let mut a = ManagedActivity::codex();
        a.observe_event(&serde_json::json!({
            "type": "assistant",
            "message": {"content": [{"type": "tool_use", "id": "x"}]}
        }));
        assert!(a.outstanding_tools.is_empty());
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
