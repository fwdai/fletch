-- Every branch a checkout has created on origin, not just the latest.
--
-- `worktrees.branch` names the branch the checkout is on right now and is
-- overwritten each time the host materializes another one. The push guard
-- (`rpc::caps`) used that single name as the set of branches the agent may
-- force-push, and as the only branch exempt from "the base is off limits" —
-- so a workspace that stacked a second PR on its first branch could no longer
-- push follow-ups to the first (#1054 / #1055). A branch stays the
-- workspace's own for as long as the workspace lives; this is the record.
--
-- Written when the host's own push creates the branch on origin
-- (`git.branch_created`), never from a name the agent supplies.
CREATE TABLE worktree_branches (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    -- The checkout within the workspace, matching `worktrees.subdir` (not a
    -- foreign key for the same reason `worktree_prs.subdir` isn't).
    subdir       TEXT NOT NULL,
    branch       TEXT NOT NULL,
    PRIMARY KEY (workspace_id, subdir, branch)
);

-- Seed from the one branch each checkout has recorded, so a workspace that is
-- mid-flight keeps the force-push it had on that branch.
INSERT OR IGNORE INTO worktree_branches (workspace_id, subdir, branch)
SELECT workspace_id, subdir, branch
  FROM worktrees
 WHERE branch IS NOT NULL AND branch <> '';
