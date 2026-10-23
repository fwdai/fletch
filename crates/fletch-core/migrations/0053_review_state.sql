-- The local review loop's durable half, one row per coder checkout (`subdir`
-- is '' for the primary repo): which reviewer workspace it has, the signature
-- of the code last reviewed, the round count, and the phase it was last in —
-- as one JSON document (`review::engine::Tracked`), so a restart shows "clean"
-- or "findings" rather than idle. A review in flight at restart is loaded as
-- failed ("interrupted by restart"): its turn is gone with the process.
--
-- Written only by `review::persist`. Deliberately NOT on the generic CRUD
-- allow-list (`database::validate`).
CREATE TABLE review_state (
    agent_id   TEXT NOT NULL,
    subdir     TEXT NOT NULL DEFAULT '',
    state      TEXT NOT NULL,               -- JSON
    updated_at INTEGER NOT NULL,            -- epoch ms
    PRIMARY KEY (agent_id, subdir)
);
