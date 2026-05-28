-- Tracks whether the Run panel's setup command has succeeded for
-- this agent at least once. NULL means setup still needs to run on
-- the next Start click; a millisecond timestamp means it's done and
-- subsequent starts skip straight to the run command.
ALTER TABLE agents ADD COLUMN setup_completed_at INTEGER;
