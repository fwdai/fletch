-- Where a session starts as a native branch of another provider session — a
-- rewind's `Exact` handoff (claude: `--resume <from> --fork-session
-- --session-id <own> --resume-session-at <message>`). `branch_from_session` is
-- the provider session id to copy; `branch_at_message` is the last message the
-- branch keeps (NULL keeps the whole session). Both NULL for an ordinary
-- session.
--
-- Never cleared: a launch branches only while the session's own transcript has
-- no message yet, and resumes it like any other once it does.
ALTER TABLE sessions ADD COLUMN branch_from_session TEXT;
ALTER TABLE sessions ADD COLUMN branch_at_message TEXT;
