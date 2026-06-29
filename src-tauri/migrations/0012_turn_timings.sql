-- Per-turn run-timer durations, stored on the existing per-turn row so a turn's
-- timing lives next to the turn it measures and rides the same read/hydrate path
-- as `read_user_turns` (no extra table).
--
-- The timer measures the agent's *active* working time and excludes spans where
-- the turn is paused awaiting a human answer (a held `can_use_tool` prompt), per
-- the run-timer spec. `active_ms` accumulates closed active spans; `running_since`
-- marks the open span's start (NULL while paused or completed). On completion the
-- open span is folded in and `completed_at` is stamped.
--
--   live elapsed (ms) = active_ms + (running_since IS NULL ? 0 : now - running_since)
--
-- All NULL/0 for turns that predate this feature → they simply render no duration.
ALTER TABLE session_user_turns ADD COLUMN active_ms     INTEGER NOT NULL DEFAULT 0;
ALTER TABLE session_user_turns ADD COLUMN running_since INTEGER;  -- ms epoch of open active span; NULL = paused/done
ALTER TABLE session_user_turns ADD COLUMN completed_at  INTEGER;  -- ms epoch turn ended; NULL = still open
