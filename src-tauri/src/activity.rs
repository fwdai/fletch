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

/// Custom view: claude runs in `--print --output-format stream-json`.
/// The `result` event is the official end-of-turn signal; we trust it
/// and use silence only as a backstop in case it ever fails to fire.
pub struct ClaudeManagedActivity {
    last_event_at: Option<Instant>,
    explicit_turn_end: bool,
}

impl ClaudeManagedActivity {
    pub fn new() -> Self {
        Self {
            last_event_at: None,
            explicit_turn_end: false,
        }
    }
}

impl Activity for ClaudeManagedActivity {
    fn observe_event(&mut self, event: &Value) {
        self.last_event_at = Some(Instant::now());
        if event.get("type").and_then(|v| v.as_str()) == Some("result") {
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

/// Custom view: codex runs as a per-turn `codex exec --json` process.
/// `turn.completed` is its official end-of-turn signal; we trust it and
/// fall back to silence only if it never arrives. (The per-turn process
/// exiting is handled separately by `CodexSession`; it does not feed
/// this detector.)
pub struct CodexManagedActivity {
    last_event_at: Option<Instant>,
    explicit_turn_end: bool,
}

impl CodexManagedActivity {
    pub fn new() -> Self {
        Self {
            last_event_at: None,
            explicit_turn_end: false,
        }
    }
}

impl Activity for CodexManagedActivity {
    fn observe_event(&mut self, event: &Value) {
        self.last_event_at = Some(Instant::now());
        if event.get("type").and_then(|v| v.as_str()) == Some("turn.completed") {
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
        let mut a = ClaudeManagedActivity::new();
        assert!(!a.turn_ended());
        a.observe_event(&serde_json::json!({"type": "assistant"}));
        assert!(!a.turn_ended());
        a.observe_event(&serde_json::json!({"type": "result", "subtype": "success"}));
        assert!(a.turn_ended());
    }

    #[test]
    fn managed_resets_after_new_turn() {
        let mut a = ClaudeManagedActivity::new();
        a.observe_event(&serde_json::json!({"type": "result"}));
        assert!(a.turn_ended());
        a.reset_for_new_turn();
        assert!(!a.turn_ended());
    }

    #[test]
    fn codex_ends_on_turn_completed_event() {
        let mut a = CodexManagedActivity::new();
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
        let mut a = CodexManagedActivity::new();
        a.observe_event(&serde_json::json!({"type": "turn.completed"}));
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
