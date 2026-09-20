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
//! Every event carries a per-agent sequence number, on the wire and in the
//! snapshot, so a client can tell an event the snapshot already holds from one
//! that arrived while the snapshot was in flight: the two are otherwise
//! indistinguishable frames, and a client has to fold exactly one copy.
//!
//! Emptied when a new user turn is about to be delivered (`begin_turn`), not
//! when the turn ends: between Idle and the turn-end ingest landing, the
//! records still lack the turn and the buffer is the only place it exists.
//! Clients replay it only while the agent is busy, so the stale copy is never
//! rendered twice.

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
    /// The sequence number the next event gets. Never reset for the agent's
    /// life under this host process — a new turn continues the count — so a
    /// client's watermark from one turn stays valid into the next.
    next_seq: u64,
}

/// The wire shape of `read_live_turn`.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct LiveTurnSnapshot {
    /// The turn's `agent:event` payloads (`event` field only), oldest first.
    pub events: Vec<Value>,
    /// How many events from the head of the turn were discarded to the cap.
    pub dropped: usize,
    /// The `seq` the next `agent:event` for this agent will carry. Every event
    /// in `events` has a lower one; a live frame with this or higher arrived
    /// after the snapshot and is not in it.
    pub next_seq: u64,
}

impl LiveTurn {
    /// Keep `event` as the turn's next, returning the sequence number it goes
    /// out on the wire with.
    pub fn push(&mut self, event: Value) -> u64 {
        if self.events.len() >= LIVE_TURN_CAP {
            self.events.pop_front();
            self.dropped += 1;
        }
        self.events.push_back(event);
        let seq = self.next_seq;
        self.next_seq += 1;
        seq
    }

    /// A new turn is about to start: forget the previous one, keep counting.
    pub fn begin_turn(&mut self) {
        self.events.clear();
        self.dropped = 0;
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
            next_seq: self.next_seq,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keeps_events_in_order_and_numbers_them() {
        let mut turn = LiveTurn::default();
        assert_eq!(turn.push(json!({ "type": "assistant", "n": 1 })), 0);
        assert_eq!(turn.push(json!({ "type": "user", "n": 2 })), 1);
        let snap = turn.snapshot();
        assert_eq!(snap.dropped, 0);
        assert_eq!(snap.next_seq, 2);
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
    fn a_new_turn_empties_the_buffer_but_the_count_goes_on() {
        let mut turn = LiveTurn::default();
        turn.push(json!({ "n": 1 }));
        turn.push(json!({ "n": 2 }));
        turn.begin_turn();
        assert_eq!(turn.push(json!({ "n": 3 })), 2);
        let snap = turn.snapshot();
        assert_eq!(snap.events.len(), 1);
        assert_eq!(snap.events[0]["n"], 3);
        assert_eq!(snap.next_seq, 3);
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
