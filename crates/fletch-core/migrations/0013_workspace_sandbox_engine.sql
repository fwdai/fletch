-- Sandbox engine the agent was stamped with at first spawn ("sandbox-exec" |
-- "docker"). Reused on every subsequent process spawn (respawn, view-switch,
-- restore), so a later settings change never re-engines an existing agent.
-- NULL for agents created before engine selection existed — they always ran
-- (and keep running) under sandbox-exec.
ALTER TABLE workspaces ADD COLUMN sandbox_engine TEXT;
