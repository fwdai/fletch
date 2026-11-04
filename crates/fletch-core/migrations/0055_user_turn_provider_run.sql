-- Time the provider spent in turns it opened on its own after this one closed:
-- claude answering a background task's notification, or waking on a schedule.
-- Those turns have no row (nothing Fletch delivered opened them), so without
-- this a turn's duration stops at its first `result` however much work follows.
-- Kept apart from `ended_at`, which still says when the turn itself closed: the
-- gaps between those runs are idle time, and a wake-up can come hours later.
ALTER TABLE session_user_turns ADD COLUMN provider_run_ms INTEGER NOT NULL DEFAULT 0;
