-- Prior-conversation digest injected into a forked agent's brief, stored
-- separately from the user/custom-agent `instructions` so the two are never
-- parsed apart heuristically. NULL for non-fork sessions. A fork rebuilds this
-- fresh from the parent's records and never inherits the parent's value, so
-- there is no stacking and the user brief is never mutated.
ALTER TABLE sessions ADD COLUMN forked_context TEXT;
