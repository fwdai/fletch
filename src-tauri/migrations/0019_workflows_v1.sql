-- 0019_workflows_v1.sql — workflows v1 schema (TECH_SPEC §4).
--
-- Replaces the v0 tables. 0018_workflows.sql has already been applied to dev
-- databases (rusqlite_migration tracks user_version, so a rewritten 0018 would
-- silently never re-run there). It therefore stays as shipped and v1 lands as a
-- fresh migration: drop the v0 tables, then create the five new ones.
--
-- v0 was never released, so its definitions/runs are dev-era scratch data and
-- are intentionally dropped, not migrated (§17 reuse map: "v0 run tables —
-- deleted / replaced"). The v0 command surface + builder/run UI are cut over to
-- the wf_* commands in S4; until then those old commands query dropped tables.
--
-- v1 is event-sourced: wf_event is an append-only journal per run, and every
-- wf_run / wf_step_exec status change is written in the same transaction as the
-- journal event that caused it. spec_json on wf_run is immutable after launch
-- (editing a definition never affects an in-flight run). Timestamps are epoch
-- milliseconds, matching the rest of the schema.

DROP TABLE IF EXISTS workflow_run_step;
DROP TABLE IF EXISTS workflow_run;
DROP TABLE IF EXISTS workflow;

-- A stored workflow: name + agents + block tree + budgets. `spec_json` is the
-- serialized Spec (§5). `run_count` / `created_at` survive edits (see the upsert
-- in workflow/mod.rs, added by the definition slice).
CREATE TABLE wf_definition (
  id          TEXT PRIMARY KEY,
  name        TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  hue         INTEGER,
  spec_json   TEXT NOT NULL,
  run_count   INTEGER NOT NULL DEFAULT 0,
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL
);

-- One execution of a definition against a task. `spec_json` is a launch-time
-- snapshot and the source of truth; `parent_run_id` is set only for composed
-- sub-runs. `cursor_json` / `spent_json` / `budgets_json` are the scheduler's
-- resumable state (§6.4, §11), written by later slices.
CREATE TABLE wf_run (
  id            TEXT PRIMARY KEY,
  definition_id TEXT,                   -- NULL for composed sub-runs
  parent_run_id TEXT REFERENCES wf_run(id),
  name          TEXT NOT NULL,
  spec_json     TEXT NOT NULL,          -- launch-time snapshot (source of truth)
  task          TEXT NOT NULL,
  project_id    TEXT NOT NULL,
  repo_path     TEXT NOT NULL,
  run_dir       TEXT NOT NULL,          -- ~/.fletch/runs/<run-id>/
  branch        TEXT NOT NULL,          -- wf/<slug>-<suffix>
  base_sha      TEXT NOT NULL,          -- commit-ish step 1 forks from
  status        TEXT NOT NULL,          -- pending|running|paused|done|failed|canceled
  paused_reason TEXT,                   -- approval|question|blocked_gate|budget_exceeded|conflict|stalled
  cursor_json   TEXT,                   -- scheduler cursor (§6.4)
  budgets_json  TEXT NOT NULL,          -- effective budgets (defaults ∪ spec)
  spent_json    TEXT NOT NULL,          -- ledger snapshot (§11)
  error         TEXT,                   -- terminal failure cause (human-readable)
  created_at    INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL
);
CREATE INDEX idx_wf_run_status ON wf_run(status);
CREATE INDEX idx_wf_run_parent ON wf_run(parent_run_id);

-- One execution of a step. Loops and retries produce multiple rows per step_id,
-- keyed by (attempt, iteration). agent_id is NULL until spawned and never reused.
CREATE TABLE wf_step_exec (
  id           TEXT PRIMARY KEY,
  run_id       TEXT NOT NULL REFERENCES wf_run(id),
  step_id      TEXT NOT NULL,           -- id of the step in the spec block tree
  attempt      INTEGER NOT NULL,        -- 1-based; retries increment
  iteration    INTEGER NOT NULL,        -- loop iteration, 0-based
  agent_id     TEXT,                    -- NULL until spawned; never reused
  status       TEXT NOT NULL,           -- §6.2
  gate_mode    TEXT NOT NULL,
  head_start   TEXT,
  head_end     TEXT,
  verdict_json TEXT,                    -- parsed verdict at gate time
  error        TEXT,
  started_at   INTEGER,
  ended_at     INTEGER
);
CREATE INDEX idx_wf_exec_run ON wf_step_exec(run_id);

-- Append-only journal. `seq` is per-run and monotonically increasing; no UPDATE
-- or DELETE while a run is live. Deleting a run cascades its events.
CREATE TABLE wf_event (
  run_id       TEXT NOT NULL REFERENCES wf_run(id),
  seq          INTEGER NOT NULL,        -- per-run, monotonically increasing
  ts           INTEGER NOT NULL,
  step_exec_id TEXT,
  type         TEXT NOT NULL,           -- §7.1
  payload_json TEXT NOT NULL,
  PRIMARY KEY (run_id, seq)
);

-- Host-brokered inter-agent messages (§10). Reports, asks/answers, notifies and
-- decisions all land here; NULL from/to endpoints mean the engine or the human.
CREATE TABLE wf_message (
  id                TEXT PRIMARY KEY,
  run_id            TEXT NOT NULL REFERENCES wf_run(id),
  from_step_exec_id TEXT,               -- NULL = engine or human
  to_step_exec_id   TEXT,               -- NULL = human (or orchestrator resolution)
  kind              TEXT NOT NULL,      -- report|ask|answer|notify|decision
  body_json         TEXT NOT NULL,
  status            TEXT NOT NULL,      -- queued|delivered|answered|expired
  created_at        INTEGER NOT NULL,
  delivered_at      INTEGER
);
CREATE INDEX idx_wf_msg_run ON wf_message(run_id);

-- Run-owned step agents are ordinary workspaces tagged with the owning run, so
-- they can be filtered out of the normal sidebar (§6.3) and discarded when the
-- run is deleted (§13). A plain tag column, NOT a foreign key with ON DELETE
-- CASCADE: run deletion goes through wf_delete_run, which discards these
-- workspaces via the app's workspace path so the on-disk worktrees/checkouts are
-- cleaned too — a DB-level cascade would drop the row but orphan those files and
-- leave supervisor state dangling. This mirrors the wf_* run_id FKs above, which
-- are RESTRICT for the same reason (children are deleted app-side, in order).
-- NULL for every normal (user-launched) agent.
ALTER TABLE workspaces ADD COLUMN owner_run_id TEXT;
