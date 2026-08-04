-- Drop the dormant roadmap_items columns: `size`, `epic`, `parent_id`.
--
-- `size` and `epic` were cut from every surface in the model-pruning pass (the
-- roadmap plan's A0): nothing consumes them — there are no sprints to size for
-- and no epics to group under; agent labor is rationed by budgets and
-- concurrency caps, not story points. `parent_id` was a v1 bet on sub-items
-- that no slice ever took; it has been documented-unused since 0026. The
-- columns were left in place until now because dropping them costs a table
-- rebuild (below), which wasn't worth taking while the table was still gaining
-- columns every other slice.
--
-- A rebuild rather than three ALTER ... DROP COLUMNs because `parent_id`
-- carries a self-referencing FK, and SQLite refuses to drop a column named in
-- a foreign-key constraint. The migration runner (rusqlite_migration) applies
-- this with foreign-key enforcement off, so the DROP/RENAME below can't
-- cascade into roadmap_item_events/roadmap_proposals rows; their `REFERENCES
-- roadmap_items` clauses resolve against the renamed table again the moment
-- the rename lands, and both operations sit in the same migration transaction.
--
-- Column order matches what 0026 + 0032 (rank) + 0033 (holds) left behind,
-- minus the three dropped — the row decoder reads by name, but keeping the
-- shape recognizable costs nothing.
CREATE TABLE roadmap_items_new (
    id              TEXT PRIMARY KEY,            -- uuid
    project_id      TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    code            TEXT NOT NULL,               -- "FLT-142" style, unique per project
    title           TEXT NOT NULL,
    why             TEXT NOT NULL DEFAULT '',
    horizon         TEXT NOT NULL,               -- now | next | later
    status          TEXT NOT NULL,               -- proposed | open | queued | active | in_review | done
    rank            REAL NOT NULL DEFAULT 0,     -- priority order (0032)
    area            TEXT,
    source          TEXT NOT NULL DEFAULT 'user',-- user | pm | linear | github
    accept_json     TEXT,                        -- JSON array of acceptance criteria strings
    deps_json       TEXT,                        -- JSON array of item codes this must land after
    agent_id        TEXT,                        -- workspace working it
    workflow_def_id TEXT,                        -- assigned workflow override
    run_id          TEXT,                        -- wf_run executing it
    pr_url          TEXT,
    pr_number       INTEGER,
    hold_reason     TEXT,                        -- holds (0033)
    held_by         TEXT,
    held_at         INTEGER,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    UNIQUE(project_id, code)
);

INSERT INTO roadmap_items_new
    (id, project_id, code, title, why, horizon, status, rank, area, source,
     accept_json, deps_json, agent_id, workflow_def_id, run_id, pr_url,
     pr_number, hold_reason, held_by, held_at, created_at, updated_at)
SELECT
     id, project_id, code, title, why, horizon, status, rank, area, source,
     accept_json, deps_json, agent_id, workflow_def_id, run_id, pr_url,
     pr_number, hold_reason, held_by, held_at, created_at, updated_at
FROM roadmap_items;

DROP TABLE roadmap_items;
ALTER TABLE roadmap_items_new RENAME TO roadmap_items;

-- The one index 0026 declared; DROP TABLE took it with the old table.
CREATE INDEX idx_roadmap_items_project ON roadmap_items(project_id);
