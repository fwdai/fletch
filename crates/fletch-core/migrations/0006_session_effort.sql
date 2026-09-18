-- Claude's session-level reasoning effort (`--effort <level>`), chosen in the
-- composer at session creation and re-applied on every process spawn. NULL for
-- existing sessions and for agents that don't use a spawn-level effort flag.
ALTER TABLE sessions ADD COLUMN effort TEXT;
