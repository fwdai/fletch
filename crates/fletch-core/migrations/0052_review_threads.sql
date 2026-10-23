-- The local reviewer's findings, one row per thread, keyed by the coder's
-- checkout (`agent_id` + `subdir`, NULL for the primary repo). They are the
-- local counterpart of a GitHub review thread: the Git panel, the readiness
-- ladder, autopilot and the coder's `pr_threads` / `reply_thread` /
-- `resolve_thread` ops read both through one merged set (`review::store`), so
-- a Codex finding and a Greptile comment are handled by the same code.
--
-- `replies` is a JSON array of `{author, body, at}` where author is "agent"
-- (the coder) or "user"; a thread whose last reply is the coder's and is still
-- open reads as `we_replied_last` (a deliberate push-back awaiting a human).
-- No foreign key on `agent_id`: an archived agent keeps its history;
-- `WorkspaceManager::remove_agent` deletes the rows with the workspace.
--
-- Deliberately NOT on the generic CRUD allow-list (`database::validate`).
CREATE TABLE review_threads (
    id          TEXT PRIMARY KEY,           -- "local:<uuid>"
    agent_id    TEXT NOT NULL,
    subdir      TEXT,
    round       INTEGER NOT NULL,           -- the review round that raised it
    severity    TEXT,                       -- high | medium | low
    path        TEXT,
    line        INTEGER,
    title       TEXT NOT NULL,
    body        TEXT NOT NULL DEFAULT '',
    author      TEXT NOT NULL,              -- the reviewer agent's name
    created_at  INTEGER NOT NULL,           -- epoch ms
    resolved_at INTEGER,
    resolved_by TEXT,                       -- agent | user
    replies     TEXT NOT NULL DEFAULT '[]'
);

CREATE INDEX idx_review_threads_checkout ON review_threads(agent_id, subdir, resolved_at);
