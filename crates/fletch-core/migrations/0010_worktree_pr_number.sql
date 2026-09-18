-- The GitHub PR number a worktree's branch was opened as, captured when the
-- PR is created (via the open_pr RPC or the panel's Create PR action) or
-- adopted from an OPEN out-of-band PR discovered by branch. Once stored, PR
-- state is fetched by this number rather than by the current branch name.
--
-- This is what unbinds PR identity from the (recyclable) workspace/branch
-- name: a fresh agent starts with NULL here, so a reused workspace name can
-- never inherit a previous agent's now-merged PR. NULL = no known PR yet.
ALTER TABLE worktrees ADD COLUMN pr_number INTEGER;
