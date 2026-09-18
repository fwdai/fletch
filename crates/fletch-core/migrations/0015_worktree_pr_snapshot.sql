-- Last-known PR snapshot for a checkout's bound PR, alongside the existing
-- pr_number / pr_opened_at / pr_merged_at. Stamped on every successful PR
-- fetch, so the UI can render PR identity and terminal state (merged/closed)
-- from the database alone — a broken checkout, a pruned worktree, network
-- loss, or a fresh app start must never blank a badge GitHub already
-- confirmed. NULL = no fetch has succeeded since the PR was bound.
ALTER TABLE worktrees ADD COLUMN pr_url TEXT;
ALTER TABLE worktrees ADD COLUMN pr_title TEXT;
-- 'open' | 'merged' | 'closed' (serialized github::PrStatus)
ALTER TABLE worktrees ADD COLUMN pr_state TEXT;
