-- The head branch each logged PR was opened from. A checkout now holds a set of
-- PRs (sub-agents each open their own), and `worktrees.branch` only names the
-- branch the checkout is on right now — so without this, a PR other than the
-- latest has no record of which branch it lives on. Nullable: rows logged
-- before this column, and payloads that don't carry it, simply don't know yet;
-- writers COALESCE so a later payload without it never erases a known one.
ALTER TABLE worktree_prs ADD COLUMN branch TEXT;

-- Every binding has a row in its checkout's set. The focused PR is read from
-- that row (the sweep, `set_focused_pr`), so a checkout bound before binds were
-- logged — or before its first fetch succeeded, which 0025 skipped — gets one
-- now; an unknown state reads 'open' until its next fetch corrects it.
INSERT OR IGNORE INTO worktree_prs (workspace_id, subdir, number, url, title, state, opened_at, merged_at)
SELECT workspace_id, subdir, pr_number,
       COALESCE(pr_url, ''), COALESCE(pr_title, ''), COALESCE(pr_state, 'open'),
       pr_opened_at, pr_merged_at
  FROM worktrees
 WHERE pr_number IS NOT NULL;
