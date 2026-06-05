-- Quorum-origin capture of outgoing user messages, kept OUT of session_records
-- so that store stays a pure 1:1 mirror of the agent's on-disk transcript.
--
-- One row per outgoing user message, written eagerly at send time (so a message
-- survives even if the agent call fails). `native_id` is filled at turn-end once
-- the matching transcript user-message lands in session_records; it points at
-- that canonical row via the stable (session_id, native_id) key, so the
-- association survives a full re-ingest from disk. NULL native_id = pending or
-- failed (rendered standalone for retry). `attachments` is a JSON array of
-- file paths attached to the message.
CREATE TABLE session_user_turns (
    turn_id     TEXT PRIMARY KEY,           -- frontend-generated uuid; idempotent across send retries
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    seq         INTEGER NOT NULL,           -- per-session monotonic send order
    text        TEXT NOT NULL,              -- original prompt (no injected "Attached file:" lines)
    attachments TEXT NOT NULL,              -- JSON array of attachment paths
    native_id   TEXT,                       -- matched session_records.native_id; NULL = pending/failed
    created_at  INTEGER NOT NULL,
    UNIQUE(session_id, seq)
);
CREATE INDEX idx_session_user_turns_session ON session_user_turns(session_id);
