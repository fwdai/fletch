-- What a session's agent is told about the conversation it continues: a
-- summary of it written at fork time, or the transcript's tail when no summary
-- could be made (`handoff`). Composed into the session's instructions on every
-- launch; NULL for a session that continues nothing. It was the fork's raw
-- transcript digest; rows written then keep that text, which still reads as
-- carried-over context.
ALTER TABLE sessions RENAME COLUMN forked_context TO handoff_context;
