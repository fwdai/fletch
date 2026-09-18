-- Archive is now two writes, and these columns record the second.
--
-- `archived_at` used to be stamped only after the checkouts were snapshotted
-- and torn down, so for the seconds that took, every workspace read still
-- showed the agent as live — and a refetch landing in that window put a
-- just-archived row back in the sidebar. The user's click is the decision;
-- `archived_at` now records the click, and the cleanup runs after it.
--
-- `archive_completed_at` is stamped when that cleanup finishes, so
-- (completed - archived) is how long an archive really took, and a row with
-- `archived_at` set but `archive_completed_at` NULL is an archive that never
-- finished (crash mid-cleanup). `archive_error` holds the joined cleanup
-- failures when any step went wrong; NULL means clean. Neither is shown in the
-- UI — they are the audit trail for how often archive fails and how slow it is.
ALTER TABLE workspaces ADD COLUMN archive_completed_at INTEGER;
ALTER TABLE workspaces ADD COLUMN archive_error TEXT;
