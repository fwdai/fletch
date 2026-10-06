-- The head branch each logged PR was opened from. A checkout now holds a set of
-- PRs (sub-agents each open their own), and `worktrees.branch` only names the
-- branch the checkout is on right now — so without this, a PR other than the
-- latest has no record of which branch it lives on. Nullable: rows logged
-- before this column, and payloads that don't carry it, simply don't know yet;
-- writers COALESCE so a later payload without it never erases a known one.
ALTER TABLE worktree_prs ADD COLUMN branch TEXT;
