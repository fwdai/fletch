-- PR lifecycle timestamps + daily token-usage snapshots, both feeding the
-- Project Pulse stats block (and any future per-project analytics).
--
-- pr_opened_at / pr_merged_at are ms-epoch values taken from GitHub's own
-- createdAt/mergedAt on the PR, stamped whenever a PR-state fetch sees them
-- (see `record_pr_times`). GitHub is the source of truth, so a PR that merged
-- while the app was closed still gets its real merge time on the next fetch.
-- NULL = not yet observed.
ALTER TABLE worktrees ADD COLUMN pr_opened_at INTEGER;
ALTER TABLE worktrees ADD COLUMN pr_merged_at INTEGER;

-- One row per (workspace, local day): a snapshot of the session's CUMULATIVE
-- token totals as of the last fold that day (see `recordUsageSnapshot` in the
-- frontend, which upserts whenever usage is re-folded from session_records).
-- Cumulative snapshots — not per-day deltas — because some providers (codex)
-- only report running totals; a day's spend is the difference between
-- consecutive snapshots. `project_id` is denormalized for cheap per-project
-- range queries.
CREATE TABLE usage_daily (
    workspace_id       TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    project_id         TEXT NOT NULL,
    day                TEXT NOT NULL,             -- local date, YYYY-MM-DD
    input_tokens       INTEGER NOT NULL DEFAULT 0,
    output_tokens      INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens  INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    cost_usd           REAL NOT NULL DEFAULT 0,   -- 0 when the provider reports no cost
    updated_at         INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, day)
);
CREATE INDEX idx_usage_daily_project ON usage_daily(project_id, day);
