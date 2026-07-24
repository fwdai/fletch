-- Reasoning effort is a session-level setting, read from the session record on
-- every turn (like the model) rather than carried per message. The per-message
-- `thinking` tag on queued follow-ups is therefore dead — drop the column.
-- Every per-turn CLI treats effort as a config/session value re-passed each
-- invocation, so nothing was ever gained by varying it per message.
ALTER TABLE pending_messages DROP COLUMN thinking;
