-- The canonical, per-agent verbatim session store, in its own database file
-- (`transcripts.db`, attached to the main connection as `transcripts`). Each
-- row is one durable record: a transcript line (Claude/Codex/Pi/Cursor), a
-- reassembled blob (OpenCode), or a turn-end compiled entry (live-compiled
-- agents). Bodies are stored verbatim in the agent's own shape and normalized
-- on read by the per-provider adapter.
--
-- `session_id` names a row of `main.sessions`, but SQLite enforces no foreign
-- key into an attached database, so ownership is kept by explicit deletes
-- (`workspace::sessions::delete_session_records_for_workspaces`) plus the
-- startup orphan sweep in `database::connection`.
CREATE TABLE session_records (
    id            INTEGER PRIMARY KEY,
    session_id    TEXT NOT NULL,
    seq           INTEGER NOT NULL,          -- per-session monotonic insert order
    provider      TEXT NOT NULL,             -- denormalized from sessions for hot read
    source        TEXT NOT NULL,             -- 'transcript' | 'live_compiled'
    native_id     TEXT NOT NULL,             -- per-agent dedup key; positional 'ln:{n}' where no native id
    agent_version TEXT,                      -- probed agent CLI version at ingest (nullable)
    body          TEXT NOT NULL,             -- verbatim record JSON
    created_at    INTEGER NOT NULL,          -- ms epoch (ingest time)
    UNIQUE(session_id, seq),
    UNIQUE(session_id, native_id)
);
