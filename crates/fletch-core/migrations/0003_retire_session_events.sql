-- session_records (0002) is now the single canonical session store; the live
-- event log is retired. Existing rows are discarded — history re-ingests from
-- each agent's on-disk transcript on next open (the sync_session backfill).
-- DROP TABLE also removes idx_session_events_session_seq.
DROP TABLE IF EXISTS session_events;
