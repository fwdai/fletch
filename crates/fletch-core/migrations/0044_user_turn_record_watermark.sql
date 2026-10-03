-- Where a turn's prompt record can start: the session's last record seq when
-- the turn's row was written, which is before its message reaches the agent.
-- Only a record past it can be the turn's prompt; one at or below it can only
-- quote it (`associate_pending_user_turns`). NULL for a row written before this
-- column existed, which is matched as it always was, with no lower bound.
ALTER TABLE session_user_turns ADD COLUMN record_watermark INTEGER;
