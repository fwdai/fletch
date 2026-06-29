-- Per-message reasoning effort for outgoing user turns. The transcript records
-- the agent's response but never the *requested* effort, so without this a
-- retry of a turn rebuilt from records (or after a restart) can't replay the
-- effort that turn used. NULL = sent at the agent's session default.
ALTER TABLE session_user_turns ADD COLUMN thinking TEXT;
