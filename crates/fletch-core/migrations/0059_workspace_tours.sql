-- The agent-authored walkthrough of a workspace's changes (`workspace::tours`),
-- written through the `tour_set` mailbox op and shown beside the Code panel's
-- diff. One per workspace, newest wins: a later turn's tour replaces the
-- earlier one. `turn_id` is the user turn it was written in, when the host
-- knew it; `steps` is the validated step list as JSON.
--
-- Written only by `WorkspaceManager::set_tour`. Deliberately NOT on the generic
-- CRUD allow-list (`database::validate`).
CREATE TABLE workspace_tours (
    workspace_id TEXT PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
    turn_id      TEXT,
    steps        TEXT NOT NULL,              -- JSON
    created_at   INTEGER NOT NULL            -- epoch ms
);
