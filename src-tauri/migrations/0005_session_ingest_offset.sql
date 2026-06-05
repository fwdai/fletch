-- Byte offset into the agent's append-only JSONL transcript up to which we've
-- ingested records into session_records. Lets the single-file readers tail the
-- file (read only the new bytes) instead of re-parsing the whole conversation
-- every turn. 0 = nothing ingested yet (also the default for existing sessions,
-- so their first sync after upgrade tails from the start — a one-time full read).
ALTER TABLE sessions ADD COLUMN ingest_offset INTEGER NOT NULL DEFAULT 0;
