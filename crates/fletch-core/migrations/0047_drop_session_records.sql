-- The rows now live in transcripts.db (migrations_transcripts/0001); they were
-- copied there by `relocate_session_records`, which `init` runs before this.
DROP TABLE IF EXISTS session_records;
