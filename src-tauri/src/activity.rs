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
}

impl ManagedActivity {
    fn new(is_turn_end: fn(&Value) -> bool) -> Self {
        Self {
            last_event_at: None,
            explicit_turn_end: false,
            is_turn_end,
        }
    }

    /// Claude (`--print --output-format stream-json`) ends a turn with a
    /// `result` event. Cursor emits Claude-shaped stream-json, so it shares
    /// this detector.
    pub fn claude() -> Self {
        Self::new(|event| event.get("type").and_then(|v| v.as_str()) == Some("result"))
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
        self.last_event_at = Some(Instant::now());
        if (self.is_turn_end)(event) {
            self.explicit_turn_end = true;
        }
    }

    fn turn_ended(&self) -> bool {
        if self.explicit_turn_end {
            return true;
        }
        self.last_event_at
            .map(|t| t.elapsed() >= MANAGED_SILENCE_BACKSTOP)
            .unwrap_or(false)
    }

    fn reset_for_new_turn(&mut self) {
        self.last_event_at = Some(Instant::now());
        self.explicit_turn_end = false;
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
        a.observe_event(&serde_json::json!({"type": "step_finish", "part": {"reason": "tool-calls"}}));
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
