-- A session that continues a conversation natively starts its own transcript
-- with that conversation: Fletch writes the history it continues as the
-- provider's session file before the first launch, and the CLI resumes it.
-- `transcript_prefix` is how many leading records of the session's own
-- transcript are that copy, counted by reading the written file back.
-- Ingestion drops them, since the session already shows them through its
-- lineage. 0 for every other session.
--
-- That replaces the native branch point (0042): no session starts as a CLI
-- fork of another any more.
ALTER TABLE sessions DROP COLUMN branch_from_session;
ALTER TABLE sessions DROP COLUMN branch_at_message;
ALTER TABLE sessions ADD COLUMN transcript_prefix INTEGER NOT NULL DEFAULT 0;
