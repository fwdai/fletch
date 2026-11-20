-- A workspace's id used to be its place name, and names are handed out again.
-- Rows that name a workspace by id with no foreign key stayed behind when a
-- workspace row was deleted (with its project, or to free its name) and then
-- read as the next workspace given that name: a review thread about another
-- repository in a new workspace's Review row.
--
-- Ids are unique from here on (`names::unique_id`), so nothing new is
-- inherited. This removes what already was: rows no workspace's id matches,
-- and rows older than the workspace that has the id now.
DELETE FROM review_threads
 WHERE NOT EXISTS (SELECT 1 FROM workspaces w
                    WHERE w.id = review_threads.agent_id
                      AND w.created_at <= review_threads.created_at);

DELETE FROM review_state
 WHERE NOT EXISTS (SELECT 1 FROM workspaces w
                    WHERE w.id = review_state.agent_id
                      AND w.created_at <= review_state.updated_at);

DELETE FROM autopilot_log
 WHERE NOT EXISTS (SELECT 1 FROM workspaces w
                    WHERE w.id = autopilot_log.agent_id
                      AND w.created_at <= autopilot_log.at);

-- Run overrides carry no time, so only those of an id no workspace of the
-- project has can be told apart. The key is `run.agent.<id>.<row>`.
DELETE FROM project_settings
 WHERE substr(key, 1, 10) = 'run.agent.'
   AND NOT EXISTS (SELECT 1 FROM workspaces w
                    WHERE w.project_id = project_settings.project_id
                      AND substr(project_settings.key, 1, length(w.id) + 11)
                          = 'run.agent.' || w.id || '.');

-- An item handed to an agent (`roadmap::assignments`). `updated_at` is at or
-- after the hand-off, so one last touched before the workspace existed was
-- handed to an earlier holder of the id.
UPDATE roadmap_items SET agent_id = NULL
 WHERE agent_id IS NOT NULL
   AND NOT EXISTS (SELECT 1 FROM workspaces w
                    WHERE w.id = roadmap_items.agent_id
                      AND w.created_at <= roadmap_items.updated_at);

-- Autopilot's paused agents, a JSON array of ids with no time: only the ids
-- no workspace has can be told apart.
UPDATE settings
   SET value = (SELECT json_group_array(j.value) FROM json_each(settings.value) j
                 WHERE j.value IN (SELECT id FROM workspaces))
 WHERE key = 'autopilotPausedAgents' AND json_valid(value) AND json_type(value) = 'array';
