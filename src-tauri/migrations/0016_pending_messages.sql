-- Durable mirror of the in-memory follow-up queue (see `message_queue.rs`).
--
-- Follow-ups the user sends while a turn is in flight are held in an in-memory
-- per-agent VecDeque and, before this table, were lost if the app exited
-- mid-turn. Each row is one queued message for the workspace's current session,
-- written on enqueue and deleted once the coalesced batch is delivered as a turn
-- (or the agent is archived/discarded). On startup the in-memory queue is
-- rehydrated from here, so a follow-up survives a crash/restart and flushes on
-- the user's next interaction.
--
-- Keyed to `session_id` (like `session_user_turns`), so the FK cascade drops the
-- rows when the session — and, transitively, the workspace — is deleted.
-- `attachments` is a JSON array of file paths; `thinking` is the optional
-- thinking-effort tag that rides along to delivery.
CREATE TABLE pending_messages (
    id          INTEGER PRIMARY KEY,
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    seq         INTEGER NOT NULL,          -- per-session monotonic enqueue order
    turn_id     TEXT NOT NULL,             -- frontend-generated uuid for this message
    text        TEXT NOT NULL,
    attachments TEXT NOT NULL,             -- JSON array of attachment paths
    thinking    TEXT,                      -- optional thinking-effort tag
    created_at  INTEGER NOT NULL,
    UNIQUE(session_id, seq)
);
CREATE INDEX idx_pending_messages_session ON pending_messages(session_id);
