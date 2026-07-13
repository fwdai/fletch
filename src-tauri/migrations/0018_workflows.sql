-- 0012_workflows.sql — workflow definitions + run execution state.
--
-- A workflow is a named, ordered chain of steps. Steps are stored as a JSON
-- array because the builder edits and saves a workflow as a whole and nothing
-- queries individual steps yet; normalize into a `workflow_step` table later if
-- per-step queries are needed. `run_count` and `created_at` are preserved across
-- edits by the upsert in workflows.rs (it updates only the mutable columns).
--
-- A `workflow_run` is one execution of a workflow; its step list is snapshotted
-- at launch (steps_snapshot) so editing the definition never mutates an
-- in-flight run. A `workflow_run_step` is one *execution* of a step — loops
-- produce multiple rows per step_id, keyed by iteration. Both are upserted whole
-- by the engine, so resume always reads a consistent snapshot. Timestamps are
-- epoch milliseconds, matching the rest of the schema.

CREATE TABLE IF NOT EXISTS workflow (
  id          TEXT PRIMARY KEY,
  name        TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  hue         INTEGER NOT NULL DEFAULT 265,
  steps       TEXT NOT NULL DEFAULT '[]',
  run_count   INTEGER NOT NULL DEFAULT 0,
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS workflow_run (
  id              TEXT PRIMARY KEY,
  workflow_id     TEXT NOT NULL,
  name            TEXT NOT NULL DEFAULT '',
  steps_snapshot  TEXT NOT NULL DEFAULT '[]',
  task            TEXT NOT NULL DEFAULT '',
  project_id      TEXT NOT NULL DEFAULT '',
  repo_path       TEXT NOT NULL DEFAULT '',
  run_dir         TEXT NOT NULL DEFAULT '',
  branch          TEXT NOT NULL DEFAULT '',
  base_sha        TEXT NOT NULL DEFAULT '',
  status          TEXT NOT NULL DEFAULT 'pending',
  current_step_id TEXT,
  current_iter    INTEGER NOT NULL DEFAULT 0,
  created_at      INTEGER NOT NULL,
  updated_at      INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_workflow_run_status ON workflow_run(status);

CREATE TABLE IF NOT EXISTS workflow_run_step (
  id            TEXT PRIMARY KEY,
  run_id        TEXT NOT NULL,
  step_id       TEXT NOT NULL,
  iteration     INTEGER NOT NULL DEFAULT 0,
  agent_id      TEXT,
  status        TEXT NOT NULL DEFAULT 'pending',
  advance_mode  TEXT NOT NULL DEFAULT 'signal',
  head_start    TEXT,
  head_end      TEXT,
  summary       TEXT,
  started_at    INTEGER,
  ended_at      INTEGER
);

CREATE INDEX IF NOT EXISTS idx_workflow_run_step_run ON workflow_run_step(run_id);
