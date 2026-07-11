-- Skills: shared library of named instruction documents. A custom agent
-- references skills by id; at spawn the selected skills are snapshotted onto
-- the session (by value) and materialized as files the agent reads on demand,
-- so editing/deleting a skill never changes a running or resumed session.
CREATE TABLE skills (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',   -- one-liner shown in the skill index
    body        TEXT NOT NULL DEFAULT '',   -- markdown, materialized at spawn
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

-- MCP servers: shared registry of tool servers a custom agent can attach.
-- `command`/`env` describe a stdio server; `url`/`headers` an http one.
-- `env` and `headers` are stored as the user typed them (KEY=VALUE /
-- "Name: value" lines) and parsed at spawn when the snapshot is built.
CREATE TABLE mcp_servers (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    transport  TEXT NOT NULL DEFAULT 'stdio',  -- 'stdio' | 'http'
    command    TEXT NOT NULL DEFAULT '',       -- full command line, whitespace-split at spawn
    env        TEXT NOT NULL DEFAULT '',       -- KEY=VALUE per line
    url        TEXT NOT NULL DEFAULT '',
    headers    TEXT NOT NULL DEFAULT '',       -- "Name: value" per line
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- A custom agent's assigned skills/servers, as JSON id arrays. Ids are
-- resolved against the library tables at spawn; a dangling id (deleted skill)
-- resolves to nothing.
ALTER TABLE custom_agents ADD COLUMN skill_ids TEXT NOT NULL DEFAULT '[]';
ALTER TABLE custom_agents ADD COLUMN mcp_server_ids TEXT NOT NULL DEFAULT '[]';

-- Per-session snapshots (JSON arrays of resolved skills/servers), mirroring how
-- `sessions.instructions` snapshots the custom agent's brief: the session keeps
-- re-injecting exactly what it was spawned with. NULL = none attached.
ALTER TABLE sessions ADD COLUMN skills TEXT;
ALTER TABLE sessions ADD COLUMN mcp_servers TEXT;
