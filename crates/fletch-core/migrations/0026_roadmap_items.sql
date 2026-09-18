-- The project roadmap: what this project is going to build, per project.
--
-- Until now the Roadmap tab rendered from a frontend mock module, so every
-- project showed the same seven invented items and nothing survived a reload.
-- This is the real table behind it. It is deliberately a *flat* list of items
-- keyed by a short human code ("FLT-142") that is unique within the project —
-- the same identifier the PM agent, the board, and (later) commit messages and
-- PR titles all quote, so the code has to be allocated once and never move.
--
-- Codes are allocated Rust-side inside the single connection mutex
-- (`roadmap::store::next_code`): prefix from `project_settings`
-- (`roadmap.code_prefix`, derived from the project name on first allocation),
-- number = MAX(existing suffix) + 1 starting at 100. Because every writer
-- serializes through that one mutex, the read-max/insert pair is atomic, and
-- `UNIQUE(project_id, code)` is the backstop if a future writer forgets.
--
-- `status` lifecycle:
--   proposed  — the PM suggested it and the user hasn't accepted yet; renders
--               as a ghost row on the board and counts for nothing.
--   open      — a real item on the board, nobody is on it.
--   queued    — handed to the run queue, waiting for a slot.
--   active    — an agent/workflow run is working it right now.
--   in_review — the work is done and a PR is open awaiting review.
--   done      — shipped. Done items leave the board entirely; the board header
--               shows their count as the "shipped" stat.
-- The full enum is written down now so later slices (queue drainer, workflow
-- binding, PR tracking) don't need a migration to move an item along it; this
-- slice only ever writes proposed/open/active/done.
--
-- `parent_id` carries no UI yet and is always NULL: sub-items are a known next
-- step, and retrofitting a self-referencing FK later would mean another table
-- rewrite. The forward-looking columns after `agent_id` are the same bet — the
-- roadmap is the hand-off point to the agent runtime, and those are the four
-- facts a hand-off produces.
--
-- Deliberately NOT added to the generic CRUD allow-list (`database::validate`):
-- like the `wf_*` tables, roadmap rows are written by typed commands only, so
-- code allocation, JSON marshalling and the row-level `roadmap:item` event can
-- never be bypassed by a frontend `db_insert`.
CREATE TABLE roadmap_items (
    id              TEXT PRIMARY KEY,            -- uuid
    project_id      TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    code            TEXT NOT NULL,               -- "FLT-142" style, unique per project
    parent_id       TEXT REFERENCES roadmap_items(id) ON DELETE CASCADE,  -- future subtasks; always NULL in v1, no UI
    title           TEXT NOT NULL,
    why             TEXT NOT NULL DEFAULT '',
    horizon         TEXT NOT NULL,               -- now | next | later
    status          TEXT NOT NULL,               -- proposed | open | queued | active | in_review | done
    size            TEXT,                        -- XS | S | M | L, nullable
    area            TEXT,
    source          TEXT NOT NULL DEFAULT 'user',-- user | pm | linear | github
    epic            TEXT,
    accept_json     TEXT,                        -- JSON array of acceptance criteria strings
    deps_json       TEXT,                        -- JSON array of item codes this must land after
    agent_id        TEXT,                        -- workspace working it
    workflow_def_id TEXT,                        -- assigned workflow override
    run_id          TEXT,                        -- wf_run executing it
    pr_url          TEXT,
    pr_number       INTEGER,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    UNIQUE(project_id, code)
);

-- Every read is "the board for this project"; the UNIQUE above already covers
-- code lookups, so this is the one index the surface needs.
CREATE INDEX idx_roadmap_items_project ON roadmap_items(project_id);
