# TODOS

## Mid-turn follow-up messages (composer unlock)

### Verify Claude mid-turn injection during pure token generation
**What:** Re-run the mid-turn injection spike, but inject while the model is streaming
text (no tool running), not during a tool-wait.
**Why:** The original spike (2026-06-27) only covered injection while a Bash tool was
running — the message buffered and folded into the same turn at the next inference
boundary (single `result`). Behavior during raw token generation is unverified and
could differ (buffered vs. ignored vs. spawning a new turn).
**Context:** Live mode (Claude) relies on writing a `{"type":"user"}` envelope to the
open stream-json stdin mid-turn. The tool-wait window is the dominant real-world case,
so this is low-likelihood-of-surprise, but worth confirming before relying on Live mode
broadly. Start from the same harness: spawn `claude --print --input-format stream-json
--output-format stream-json`, send a prompt that produces a long text response (no
tools), inject a second user message ~1s in, observe ordering and the number of
`result` events.
**Depends on:** nothing.

### Optimize associate_pending_user_turns substring scan
**What:** Anchor the matcher's record search to a seq window instead of scanning the
whole session.
**Why:** `associate_pending_user_turns` (workspace.rs:973) loads all transcript records
for the session and runs `body.contains(needle)` for every pending turn × every record
— O(records × pending × body_len) per turn-end. Mid-turn injection nudges `pending` up
per window, and long sessions make the full scan costly.
**Context:** Pre-existing and currently negligible (`pending` is single digits), so this
is explicitly deferred — not a correctness issue, and fixing it now would widen the diff
of the riskiest function. Start by restricting the records query to those with `seq`
greater than the highest already-matched user turn's record, so each turn-end only scans
the newly-appended tail.
**Depends on:** nothing; orthogonal to the mid-turn feature.
