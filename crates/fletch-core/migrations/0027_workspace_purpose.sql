-- What a workspace is FOR, when it isn't an ordinary feature-development agent.
--
-- Every workspace so far has been the same thing: an agent the user spawned to
-- change code, listed in the sidebar, publishing its own branch. The Roadmap
-- tab introduces a second kind — a project-manager chat. It is still a real
-- workspace (its own clone, session and transcript, so it can read the code it
-- reasons about), but it is a *manual chat*, not a run: it never edits or
-- publishes code, and it must not appear in the sidebar, which is reserved for
-- feature work and workflow runs.
--
-- Rather than a boolean per kind, this is an open tag. NULL — the only value an
-- existing row can have, and the default for every ordinary spawn — means
-- "a normal agent"; a non-NULL value names the surface that owns the workspace
-- and is the single thing the sidebar filter, the RPC capability grant, and the
-- owning surface's list query all key off (see `workspace::PURPOSE_ROADMAP_PM`).
--
-- Deliberately parallel to `owner_run_id` (0019), which hides run-owned step
-- agents the same way: `query_all_agents` now requires both to be NULL.
ALTER TABLE workspaces ADD COLUMN purpose TEXT;

-- The listing this drives is "every PM chat in this project, newest first".
CREATE INDEX IF NOT EXISTS idx_workspaces_purpose
    ON workspaces(project_id, purpose)
    WHERE purpose IS NOT NULL;
