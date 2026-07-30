-- Every PR a checkout has ever held, not just the current binding.
--
-- `worktrees.pr_number` + its snapshot columns track the ONE PR a checkout is
-- bound to right now, and re-binding replaces them (see `set_repo_pr_number`):
-- a workspace that keeps working after its PR merges loses the merged PR's
-- identity the moment a follow-up binds. That made "merged" look terminal in
-- the data even though it isn't, and it under-counted the Project Pulse "PRs
-- opened" chart, which read one row per checkout.
--
-- This is the append-only log beside it: one row per (checkout, PR number),
-- upserted from `set_repo_pr_snapshot` — the single choke point every path that
-- learns a PR's state already funnels through. The `worktrees` columns stay the
-- current-binding fast path, so no read path changes; this table answers "what
-- else has this checkout proposed".
CREATE TABLE worktree_prs (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    -- The checkout within the workspace, matching `worktrees.subdir`. Not a
    -- foreign key: `worktrees` is unique on (workspace_id, repo_id), not on
    -- subdir, so there is no key to point at. Deleting a workspace cascades;
    -- rows for a removed checkout are simply never read (every query scopes by
    -- workspace_id + subdir).
    subdir       TEXT NOT NULL,
    number       INTEGER NOT NULL,
    url          TEXT NOT NULL,
    title        TEXT NOT NULL,
    -- 'open' | 'merged' | 'closed' (serialized github::PrStatus)
    state        TEXT NOT NULL,
    -- ms-epoch, from GitHub's own createdAt/mergedAt. NULL = not yet observed.
    opened_at    INTEGER,
    merged_at    INTEGER,
    PRIMARY KEY (workspace_id, subdir, number)
);

-- Backs the per-project "PRs opened per day" range query in pulseData.ts.
CREATE INDEX idx_worktree_prs_opened ON worktree_prs(opened_at);

-- Seed from the current bindings so history starts complete rather than at
-- today: without this the Pulse chart would blank its past days until each PR
-- happened to be re-fetched. Only rows with a persisted state are carried —
-- `pr_state IS NULL` means no fetch ever succeeded, so there is no snapshot to
-- preserve (the same bar `pr_snapshot` sets before it will render one).
INSERT INTO worktree_prs (workspace_id, subdir, number, url, title, state, opened_at, merged_at)
SELECT workspace_id, subdir, pr_number,
       COALESCE(pr_url, ''), COALESCE(pr_title, ''), pr_state,
       pr_opened_at, pr_merged_at
  FROM worktrees
 WHERE pr_number IS NOT NULL AND pr_state IS NOT NULL;
