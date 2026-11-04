-- The clone-per-step workflow engine is deleted; the sequential kernel is the
-- one engine (docs/workflow-kernel-handoff.md). Two kinds of run row only that
-- engine produced would otherwise outlive it.
--
-- `paused` runs. Nothing can resume, retry, approve or answer them any more,
-- and `paused` is not a status this build reads. Each is failed the way the
-- engine fails a run — a `run_failed` journal event carrying the cause, then
-- the row's status and `error` — so its timeline ends in a stated failure
-- rather than a pause nobody can lift. `paused_reason` goes with the pause.
--
-- Composed sub-runs (`parent_run_id`). The orchestrate stage that started them
-- and joined on them is gone. A live one is failed rather than left for the
-- kernel to run on its own, for nobody; then every sub-run is detached into a
-- standalone run, so each deletes on its own and its parent can be deleted at
-- all (the FK has no cascade).
--
-- The `paused_reason`, `parent_run_id`, `cursor_json`, `budgets_json` and
-- `spent_json` columns and the `wf_message` table stay — the schema is
-- append-only — but nothing reads them.

INSERT INTO wf_event (run_id, seq, ts, step_exec_id, type, payload_json)
SELECT r.id,
       (SELECT COALESCE(MAX(e.seq), 0) + 1 FROM wf_event e WHERE e.run_id = r.id),
       CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER),
       NULL,
       'run_failed',
       json_object('error', 'paused by a workflow engine that no longer exists, so nothing can resume it; the run repository still holds its finished steps'' commits')
  FROM wf_run r
 WHERE r.status = 'paused';

UPDATE wf_run
   SET status = 'failed',
       paused_reason = NULL,
       error = 'paused by a workflow engine that no longer exists, so nothing can resume it; the run repository still holds its finished steps'' commits',
       updated_at = CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
 WHERE status = 'paused';

INSERT INTO wf_event (run_id, seq, ts, step_exec_id, type, payload_json)
SELECT r.id,
       (SELECT COALESCE(MAX(e.seq), 0) + 1 FROM wf_event e WHERE e.run_id = r.id),
       CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER),
       NULL,
       'run_failed',
       json_object('error', 'a sub-run of an orchestrate stage, which no longer exists to join it')
  FROM wf_run r
 WHERE r.parent_run_id IS NOT NULL AND r.status IN ('pending', 'running');

UPDATE wf_run
   SET status = 'failed',
       error = 'a sub-run of an orchestrate stage, which no longer exists to join it',
       updated_at = CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
 WHERE parent_run_id IS NOT NULL AND status IN ('pending', 'running');

UPDATE wf_run SET parent_run_id = NULL WHERE parent_run_id IS NOT NULL;
