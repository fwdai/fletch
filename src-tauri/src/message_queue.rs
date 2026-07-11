//! Mid-turn follow-up messages.
//!
//! When an agent is working, the user can still send follow-up messages. How
//! they're delivered depends on the provider:
//!
//! ```text
//!  user sends message
//!         │
//!         ▼
//!   ┌──────────────┐  busy = backend Activity/status (source of truth)
//!   │ agent busy?  │── no & queue empty ─▶ DeliverNow  (direct, new turn)
//!   └──────┬───────┘── no & queue full  ─▶ FlushNow    (coalesce leftovers + this)
//!         yes
//!          ▼
//!   ┌──────────────────┐
//!   │ injection mode?  │
//!   └──┬────────────┬──┘
//!   Live         AtTurnBoundary
//!     │               └──────────────▶ Enqueue (deliver at next turn boundary)
//!   tool-gate paused?
//!     │
//!   yes ─▶ Enqueue (can't write into a paused turn)
//!   no  ─▶ WriteLive (write into the running turn's stdin)
//! ```
//!
//! This module is **pure**: it owns the per-agent queue and the coalescing /
//! decision logic, but performs no I/O. The supervisor executes the chosen
//! [`Delivery`] (stdin write, DB persist, process spawn). Queued messages live
//! only in memory — if the app exits mid-turn the turn itself is aborted, so
//! dropping its queued follow-ups is consistent.

use std::collections::{HashMap, HashSet, VecDeque};

/// How a provider accepts a follow-up sent while a turn is in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectionMode {
    /// Write into the running turn's open stdin immediately (claude managed).
    /// The CLI folds the message into the in-flight turn at its next inference
    /// boundary, so each live message stays its own transcript record.
    Live,
    /// Can't inject mid-turn (one-shot-per-turn processes with no live stdin):
    /// queue and deliver at the next turn boundary, coalesced into one prompt.
    /// All per-turn agents (codex/cursor/opencode/pi/antigravity).
    AtTurnBoundary,
}

/// One follow-up message the user sent. `text` + `attachments` are what get
/// coalesced; `thinking` rides along to the delivery path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingMsg {
    pub turn_id: String,
    pub text: String,
    pub attachments: Vec<String>,
    pub thinking: Option<String>,
}

/// What to do with a freshly-sent message. Pure decision — see
/// [`decide_delivery`]; the supervisor performs the matching I/O.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// Idle, nothing queued: deliver this message directly as a new turn
    /// (the pre-existing send path).
    DeliverNow,
    /// Idle but messages are queued (e.g. left over after a stop): enqueue this
    /// one, then flush the whole queue coalesced, now.
    FlushNow,
    /// Busy, provider supports live injection and isn't paused on a tool gate:
    /// write into the running turn.
    WriteLive,
    /// Busy (or Live but paused on a tool gate): hold for the next boundary.
    Enqueue,
}

/// Decide how a freshly-sent message should be handled. Pure over the four
/// inputs so the full matrix is unit-testable without spawning a process.
///
/// `busy` is the backend's runtime truth (the agent is Spawning/Running),
/// `tool_gated` is true when the managed session is paused on a held
/// permission prompt, and `queue_nonempty` reflects leftovers (typically from
/// a prior stop that kept the queue).
pub fn decide_delivery(
    busy: bool,
    mode: InjectionMode,
    tool_gated: bool,
    queue_nonempty: bool,
) -> Delivery {
    match (busy, mode, tool_gated) {
        (false, _, _) if queue_nonempty => Delivery::FlushNow,
        (false, _, _) => Delivery::DeliverNow,
        (true, InjectionMode::Live, false) => Delivery::WriteLive,
        (true, _, _) => Delivery::Enqueue,
    }
}

/// In-memory per-agent FIFO of follow-up messages awaiting delivery.
#[derive(Default)]
pub struct MessageQueue {
    by_agent: HashMap<String, VecDeque<PendingMsg>>,
}

impl MessageQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enqueue(&mut self, agent_id: &str, msg: PendingMsg) {
        self.by_agent
            .entry(agent_id.to_string())
            .or_default()
            .push_back(msg);
    }

    /// Put a message back at the *front* of the queue — used when a flush failed
    /// to deliver (raced with teardown/respawn), so it stays ahead of anything
    /// queued since and the original send order is preserved.
    pub fn requeue_front(&mut self, agent_id: &str, msg: PendingMsg) {
        self.by_agent
            .entry(agent_id.to_string())
            .or_default()
            .push_front(msg);
    }

    pub fn is_empty(&self, agent_id: &str) -> bool {
        // `map_or(true, …)` rather than `is_none_or` to stay on the crate's
        // declared rust-version (1.77; `is_none_or` landed in 1.82).
        self.by_agent.get(agent_id).map_or(true, VecDeque::is_empty)
    }

    pub fn len(&self, agent_id: &str) -> usize {
        self.by_agent.get(agent_id).map_or(0, VecDeque::len)
    }

    /// The `turn_id`s currently queued for an agent, in FIFO order. Used by the
    /// durable mirror: after a flush delivers, the supervisor deletes every
    /// persisted row *except* these, so a follow-up that arrived during the
    /// delivery window (still queued here) keeps its row (see
    /// `WorkspaceManager::delete_pending_messages_except`).
    pub fn turn_ids(&self, agent_id: &str) -> Vec<String> {
        self.by_agent.get(agent_id).map_or_else(Vec::new, |q| {
            q.iter().map(|m| m.turn_id.clone()).collect()
        })
    }

    /// Remove and return all queued messages for an agent, coalesced into a
    /// single [`PendingMsg`]. `None` if nothing was queued.
    pub fn drain_coalesced(&mut self, agent_id: &str) -> Option<PendingMsg> {
        let q = self.by_agent.get_mut(agent_id)?;
        if q.is_empty() {
            return None;
        }
        let msgs: Vec<PendingMsg> = q.drain(..).collect();
        Some(coalesce(msgs))
    }

    /// Drop every queued message for an agent (teardown / archive).
    pub fn clear(&mut self, agent_id: &str) {
        self.by_agent.remove(agent_id);
    }
}

/// Merge queued messages into one delivery:
/// - text: non-empty bodies newline-joined in send order,
/// - attachments: unioned, order-preserving, deduped,
/// - metadata (`thinking`) + `turn_id`: taken from the LAST message — most
///   recent intent (CQ1-A), and a real client-known id for the coalesced row.
///
/// A single queued message passes through untouched (no separators, no churn).
fn coalesce(msgs: Vec<PendingMsg>) -> PendingMsg {
    debug_assert!(!msgs.is_empty(), "coalesce called with no messages");
    if msgs.len() == 1 {
        return msgs.into_iter().next().expect("len checked");
    }
    let mut texts: Vec<&str> = Vec::new();
    let mut attachments: Vec<String> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for m in &msgs {
        if !m.text.is_empty() {
            texts.push(&m.text);
        }
        for a in &m.attachments {
            if seen.insert(a.as_str()) {
                attachments.push(a.clone());
            }
        }
    }
    let last = msgs.last().expect("len > 1");
    PendingMsg {
        turn_id: last.turn_id.clone(),
        text: texts.join("\n\n"),
        attachments,
        thinking: last.thinking.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(turn_id: &str, text: &str, attachments: &[&str], thinking: Option<&str>) -> PendingMsg {
        PendingMsg {
            turn_id: turn_id.into(),
            text: text.into(),
            attachments: attachments.iter().map(|s| s.to_string()).collect(),
            thinking: thinking.map(str::to_string),
        }
    }

    // ── decide_delivery matrix ────────────────────────────────────────────

    #[test]
    fn idle_empty_queue_delivers_now() {
        for mode in [InjectionMode::Live, InjectionMode::AtTurnBoundary] {
            assert_eq!(
                decide_delivery(false, mode, false, false),
                Delivery::DeliverNow
            );
        }
    }

    #[test]
    fn idle_with_leftovers_flushes_now() {
        // Leftovers (e.g. after a stop) flush together with the new message.
        for mode in [InjectionMode::Live, InjectionMode::AtTurnBoundary] {
            assert_eq!(
                decide_delivery(false, mode, false, true),
                Delivery::FlushNow
            );
        }
    }

    #[test]
    fn busy_live_not_gated_writes_live() {
        assert_eq!(
            decide_delivery(true, InjectionMode::Live, false, false),
            Delivery::WriteLive
        );
    }

    #[test]
    fn busy_live_tool_gated_enqueues() {
        // Can't write into a turn paused on a permission prompt.
        assert_eq!(
            decide_delivery(true, InjectionMode::Live, true, false),
            Delivery::Enqueue
        );
    }

    #[test]
    fn busy_per_turn_always_enqueues() {
        for gated in [false, true] {
            for q in [false, true] {
                assert_eq!(
                    decide_delivery(true, InjectionMode::AtTurnBoundary, gated, q),
                    Delivery::Enqueue
                );
            }
        }
    }

    // ── queue FIFO + lifecycle ────────────────────────────────────────────

    #[test]
    fn enqueue_len_and_emptiness() {
        let mut q = MessageQueue::new();
        assert!(q.is_empty("a"));
        assert_eq!(q.len("a"), 0);
        q.enqueue("a", msg("t1", "hi", &[], None));
        q.enqueue("a", msg("t2", "there", &[], None));
        assert!(!q.is_empty("a"));
        assert_eq!(q.len("a"), 2);
        // isolation between agents
        assert!(q.is_empty("b"));
    }

    #[test]
    fn turn_ids_lists_queued_in_fifo_order() {
        let mut q = MessageQueue::new();
        assert!(q.turn_ids("a").is_empty());
        q.enqueue("a", msg("t1", "hi", &[], None));
        q.enqueue("a", msg("t2", "there", &[], None));
        assert_eq!(q.turn_ids("a"), vec!["t1".to_string(), "t2".to_string()]);
        // Draining empties it; the persisted-row cleanup then keeps nothing.
        q.drain_coalesced("a");
        assert!(q.turn_ids("a").is_empty());
    }

    #[test]
    fn drain_empty_is_none() {
        let mut q = MessageQueue::new();
        assert_eq!(q.drain_coalesced("a"), None);
    }

    #[test]
    fn drain_clears_the_queue() {
        let mut q = MessageQueue::new();
        q.enqueue("a", msg("t1", "hi", &[], None));
        assert!(q.drain_coalesced("a").is_some());
        assert!(q.is_empty("a"));
        assert_eq!(q.drain_coalesced("a"), None);
    }

    #[test]
    fn requeue_front_keeps_failed_flush_ahead_of_newer_messages() {
        // A failed flush re-queues the coalesced batch; a message queued since
        // must not jump ahead of it.
        let mut q = MessageQueue::new();
        q.enqueue(
            "a",
            msg("t-new", "queued after the failed flush", &[], None),
        );
        q.requeue_front("a", msg("t-coalesced", "first\n\nsecond", &[], None));
        let out = q.drain_coalesced("a").unwrap();
        assert_eq!(out.text, "first\n\nsecond\n\nqueued after the failed flush");
    }

    #[test]
    fn clear_drops_messages() {
        let mut q = MessageQueue::new();
        q.enqueue("a", msg("t1", "hi", &[], None));
        q.clear("a");
        assert!(q.is_empty("a"));
    }

    // ── coalescing ────────────────────────────────────────────────────────

    #[test]
    fn single_message_passes_through() {
        let mut q = MessageQueue::new();
        let m = msg("t1", "only", &["/a.txt"], Some("high"));
        q.enqueue("a", m.clone());
        assert_eq!(q.drain_coalesced("a"), Some(m));
    }

    #[test]
    fn coalesce_joins_text_in_order() {
        let mut q = MessageQueue::new();
        q.enqueue("a", msg("t1", "first", &[], None));
        q.enqueue("a", msg("t2", "second", &[], None));
        q.enqueue("a", msg("t3", "third", &[], None));
        let out = q.drain_coalesced("a").unwrap();
        assert_eq!(out.text, "first\n\nsecond\n\nthird");
    }

    #[test]
    fn coalesce_skips_empty_text_bodies() {
        // An attachment-only message contributes no text line.
        let mut q = MessageQueue::new();
        q.enqueue("a", msg("t1", "hello", &[], None));
        q.enqueue("a", msg("t2", "", &["/img.png"], None));
        let out = q.drain_coalesced("a").unwrap();
        assert_eq!(out.text, "hello");
        assert_eq!(out.attachments, vec!["/img.png".to_string()]);
    }

    #[test]
    fn coalesce_unions_attachments_dedup_preserving_order() {
        let mut q = MessageQueue::new();
        q.enqueue("a", msg("t1", "x", &["/a.txt", "/b.txt"], None));
        q.enqueue("a", msg("t2", "y", &["/b.txt", "/c.txt"], None));
        let out = q.drain_coalesced("a").unwrap();
        assert_eq!(
            out.attachments,
            vec![
                "/a.txt".to_string(),
                "/b.txt".to_string(),
                "/c.txt".to_string()
            ]
        );
    }

    #[test]
    fn coalesce_takes_last_message_metadata_and_turn_id() {
        // CQ1-A: last-message-wins for thinking; turn_id is the last id.
        let mut q = MessageQueue::new();
        q.enqueue("a", msg("t1", "x", &[], Some("low")));
        q.enqueue("a", msg("t2", "y", &[], Some("high")));
        let out = q.drain_coalesced("a").unwrap();
        assert_eq!(out.thinking, Some("high".to_string()));
        assert_eq!(out.turn_id, "t2");
    }
}
