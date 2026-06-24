-- Custom agents: user-defined presets that wrap a base provider with a name,
-- color, model, reasoning effort, and a standing instruction brief. They show
-- up in the composer alongside the built-in providers and inject their
-- instructions into the agent they spawn.
CREATE TABLE custom_agents (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    description  TEXT NOT NULL DEFAULT '',
    color        INTEGER NOT NULL,          -- hue (0-360) for the monogram tile
    base         TEXT NOT NULL,             -- base provider id (claude/codex/…)
    model        TEXT,                      -- NULL = provider CLI default
    effort       TEXT,                      -- reasoning budget; NULL = none
    instructions TEXT NOT NULL DEFAULT '',
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
);

-- A session spawned from a custom agent snapshots that agent's instructions
-- here, so they re-inject identically on every respawn/resume even if the
-- custom agent is later edited or deleted. NULL = a plain built-in spawn.
ALTER TABLE sessions ADD COLUMN instructions TEXT;

-- Reference to the custom agent this session was spawned from, used to show
-- the agent's name/color in the sidebar. NULL = a plain built-in spawn.
ALTER TABLE sessions ADD COLUMN custom_agent_id TEXT;
