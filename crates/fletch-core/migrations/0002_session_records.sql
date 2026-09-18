-- The canonical, per-agent verbatim session store. Each row is one durable
-- record: a transcript line (Claude/Codex/Pi/Cursor), a reassembled blob
-- (OpenCode), or a turn-end compiled entry (live-compiled agents). Bodies are
-- stored verbatim in the agent's own shape and normalized on read by the
-- per-provider adapter. Supersedes session_events (kept read-only for legacy
-- sessions until a later cutover).
CREATE TABLE session_records (
    id            INTEGER PRIMARY KEY,
    session_id    TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    seq           INTEGER NOT NULL,          -- per-session monotonic insert order
    provider      TEXT NOT NULL,             -- denormalized from sessions for hot read
    source        TEXT NOT NULL,             -- 'transcript' | 'live_compiled'
    native_id     TEXT NOT NULL,             -- per-agent dedup key; positional 'ln:{n}' where no native id
    agent_version TEXT,                       -- probed agent CLI version at ingest (nullable)
    body          TEXT NOT NULL,             -- verbatim record JSON
    created_at    INTEGER NOT NULL,          -- ms epoch (ingest time)
    UNIQUE(session_id, seq),
    UNIQUE(session_id, native_id)
);
CREATE INDEX idx_session_records_session_seq ON session_records(session_id, seq);
