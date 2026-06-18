-- The immutable fork-point commit a worktree was created from. Captured at
-- spawn time (after a best-effort fetch of the parent branch) so diffs are
-- measured against the exact commit the agent started from, not a branch name
-- that may resolve to stale local state. NULL for agents created before this
-- column existed; readers fall back to the parent branch name.
ALTER TABLE worktrees ADD COLUMN base_sha TEXT;
