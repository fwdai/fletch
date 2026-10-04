-- What the host's autopilot did, per checkout: one row per dispatch, settle,
-- retry and give-up. The audit trail for a loop whose whole premise is acting
-- while nobody is watching, so it outlives a restart and is the same on every
-- client (`autopilot_log`, `autopilot:event`). Its cycles stay in memory — a
-- turn that did not survive a restart is not one to judge.
--
-- Written only by `autopilot::store::append_log`, which keeps the newest 50
-- rows per checkout. `subdir` is NULL for the agent's primary repo. No foreign
-- key on `agent_id`: the driver prunes rows once their agent is gone, and an
-- archived agent keeps its history.
--
-- Deliberately NOT on the generic CRUD allow-list (`database::validate`).
CREATE TABLE autopilot_log (
    id       TEXT PRIMARY KEY,           -- uuid
    agent_id TEXT NOT NULL,
    subdir   TEXT,
    at       INTEGER NOT NULL,           -- epoch ms of the decision
    outcome  TEXT NOT NULL,              -- dispatch | settle | retry | give-up
    rung     TEXT NOT NULL,              -- delegation kind
    attempt  INTEGER NOT NULL,
    reason   TEXT                        -- give-up only
);

CREATE INDEX idx_autopilot_log_checkout ON autopilot_log(agent_id, subdir, at);
