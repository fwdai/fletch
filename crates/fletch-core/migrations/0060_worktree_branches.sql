-- The branches a workspace created on origin, per checkout: the record an
-- agent's force-push and a push to its own stacked base are authorized against
-- (`rpc::caps::refuses_publish`).
--
-- `worktrees.branch` can't be that record: it names only the branch the
-- checkout is on now, and each new branch overwrites it, so the lower branch
-- of a stack is forgotten the moment the next one is cut. `worktree_prs` can't
-- either: branch discovery adopts PRs opened elsewhere, a teammate's included,
-- so a PR in the set says nothing about who made its branch.
--
-- A row is written only when the host's own push created the branch, having
-- asked origin first and found no such branch there (`EVENT_BRANCH_OWNED`).
-- Append-only: a branch stays owned for the workspace's life.
CREATE TABLE worktree_branches (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    -- The checkout within the workspace, matching `worktrees.subdir` (not a
    -- foreign key, for the reason `worktree_prs` gives).
    subdir       TEXT NOT NULL,
    branch       TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, subdir, branch)
);

-- Seed from each checkout's recorded branch, the force-push anchor until now,
-- so a branch that could be force-pushed before this migration still can.
INSERT OR IGNORE INTO worktree_branches (workspace_id, subdir, branch, created_at)
SELECT workspace_id, subdir, branch, CAST(strftime('%s', 'now') AS INTEGER) * 1000
  FROM worktrees
 WHERE branch IS NOT NULL AND TRIM(branch) <> '';
