-- The branches a workspace created on origin, per checkout, and the commit it
-- last published to each: the record an agent's force-push and a push to its
-- own stacked base are authorized against (`rpc::caps::refuses_publish`).
--
-- `worktrees.branch` can't be that record: it names only the branch the
-- checkout is on now, and each new branch overwrites it, so the lower branch
-- of a stack is forgotten the moment the next one is cut. `worktree_prs` can't
-- either: branch discovery adopts PRs opened elsewhere, a teammate's included,
-- so a PR in the set says nothing about who made its branch.
--
-- A row is written only when the host's own push created the branch, having
-- asked origin first and found no such branch there; the host's later pushes
-- move `tip`. A name alone would outlive the branch: deleted after a merge and
-- recreated by someone else, it would still read as this workspace's. So a
-- branch is treated as the one this workspace made only while origin's tip
-- still descends from `tip`; a recreated branch does not.
CREATE TABLE worktree_branches (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    -- The checkout within the workspace, matching `worktrees.subdir` (not a
    -- foreign key, for the reason `worktree_prs` gives).
    subdir       TEXT NOT NULL,
    branch       TEXT NOT NULL,
    -- The commit this workspace last published to the branch. NULL for a row
    -- seeded below, until this workspace pushes the branch again.
    tip          TEXT,
    created_at   INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, subdir, branch)
);

-- Seed from each checkout's recorded branch, the force-push anchor until now,
-- so a branch that could be force-pushed before this migration still can.
-- What it last published is unknown, so `tip` stays NULL.
INSERT OR IGNORE INTO worktree_branches (workspace_id, subdir, branch, created_at)
SELECT workspace_id, subdir, branch, CAST(strftime('%s', 'now') AS INTEGER) * 1000
  FROM worktrees
 WHERE branch IS NOT NULL AND TRIM(branch) <> '';
