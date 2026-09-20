//! The running turn, kept in memory so a client that missed the stream can
//! catch up.
//!
//! `agent:event` is fire-and-forget and the transcript is ingested into
//! `session_records` only at turn end, so while a turn runs the only complete
//! copy of it lives in whichever webview happened to be connected. A phone
//! whose socket died in the background comes back to a log that stops at the
//! previous turn and nothing to fetch. This buffer is what `read_live_turn`
//! answers with: every event of the current turn, in order, for any provider —
//! it sits on the one callback all managed and per-turn runners share, so it
//! needs nothing from the provider's transcript format.
//!
//! Cleared when a new user turn starts (`mark_user_turn_started`), not when the
//! turn ends: between Idle and the turn-end ingest landing, the records still
//! lack the turn and the buffer is the only place it exists. Clients replay it
//! only while the agent is busy, so the stale copy is never rendered twice.

use std::collections::VecDeque;

use serde::Serialize;
use serde_json::Value;

/// How many events one turn keeps. A long agentic turn is a few hundred events;
/// the cap is there so a runaway loop cannot grow the host's memory unbounded.
/// Over the cap the *oldest* events go, and `dropped` tells the client so it
/// can say the head of the turn is missing rather than render a hole silently.
pub const LIVE_TURN_CAP: usize = 2000;

#[derive(Default)]
pub struct LiveTurn {
    events: VecDeque<Value>,
    dropped: usize,
}

/// The wire shape of `read_live_turn`.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct LiveTurnSnapshot {
    /// The turn's `agent:event` payloads (`event` field only), oldest first.
    pub events: Vec<Value>,
    /// How many events from the head of the turn were discarded to the cap.
    pub dropped: usize,
}

impl LiveTurn {
    pub fn push(&mut self, event: Value) {
        if self.events.len() >= LIVE_TURN_CAP {
            self.events.pop_front();
            self.dropped += 1;
        }
        self.events.push_back(event);
    }

    /// A held permission prompt was answered: replaying it would show the
    /// client a card for a question that is already settled, so it leaves the
    /// buffer. Matched on the control protocol's `request_id`.
    pub fn answered(&mut self, request_id: &str) {
        self.events.retain(|e| {
            !(e.get("type").and_then(Value::as_str) == Some("control_request")
                && e.get("request_id").and_then(Value::as_str) == Some(request_id))
        });
    }

    pub fn snapshot(&self) -> LiveTurnSnapshot {
        LiveTurnSnapshot {
            events: self.events.iter().cloned().collect(),
            dropped: self.dropped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keeps_events_in_order() {
        let mut turn = LiveTurn::default();
        turn.push(json!({ "type": "assistant", "n": 1 }));
        turn.push(json!({ "type": "user", "n": 2 }));
        let snap = turn.snapshot();
        assert_eq!(snap.dropped, 0);
        assert_eq!(snap.events.len(), 2);
        assert_eq!(snap.events[0]["n"], 1);
        assert_eq!(snap.events[1]["n"], 2);
    }

    #[test]
    fn over_the_cap_the_oldest_go_and_are_counted() {
        let mut turn = LiveTurn::default();
        for n in 0..(LIVE_TURN_CAP + 3) {
            turn.push(json!({ "n": n }));
        }
        let snap = turn.snapshot();
        assert_eq!(snap.dropped, 3);
        assert_eq!(snap.events.len(), LIVE_TURN_CAP);
        assert_eq!(snap.events[0]["n"], 3);
    }

    #[test]
    fn an_answered_prompt_leaves_the_buffer_and_nothing_else_does() {
        let mut turn = LiveTurn::default();
        turn.push(json!({ "type": "control_request", "request_id": "r1" }));
        turn.push(json!({ "type": "control_request", "request_id": "r2" }));
        turn.push(json!({ "type": "assistant", "request_id": "r1" }));
        turn.answered("r1");
        let snap = turn.snapshot();
        assert_eq!(snap.events.len(), 2);
        assert_eq!(snap.events[0]["request_id"], "r2");
        assert_eq!(snap.events[1]["type"], "assistant");
    }
}
