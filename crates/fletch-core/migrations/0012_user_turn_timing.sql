-- Wall-clock timing for a turn, so the UI can show a live timer while a turn
-- runs and "Ran 34s" once it finishes.
--
-- `created_at` is *send* time and pays the first-turn cold-start spawn latency,
-- so it is not a faithful start anchor. `started_at` is stamped when the turn
-- actually flips to Running; `ended_at` when it reaches a terminal state
-- (clean Idle, error, or stop). `ended_at IS NULL` is the "in flight" signal
-- the UI ticks against. Both NULL for turns created before this column existed
-- and for turns reconstructed purely from disk; readers render no duration for
-- those rather than a bogus 0s.
ALTER TABLE session_user_turns ADD COLUMN started_at INTEGER;
ALTER TABLE session_user_turns ADD COLUMN ended_at   INTEGER;
