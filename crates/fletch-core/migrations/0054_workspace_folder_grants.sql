-- Folders on the user's machine granted to one workspace's agent, live at
-- their own paths (`workspace::folder_grants`): the user @-mentions or drops a
-- folder in the composer and every sandbox engine exposes it to that agent on
-- its next launch — a seatbelt grant, a container bind mount, a box virtiofs
-- share. `path` is canonical (symlinks resolved); `writable` is 0 for a
-- read-only grant, the default.
--
-- Deliberately NOT on the generic CRUD allow-list (`database::validate`): a
-- grant widens a sandbox, so it is written only through the validating
-- commands (`Supervisor::grant_folder`), never by the webview directly.
CREATE TABLE workspace_folder_grants (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    path         TEXT NOT NULL,
    writable     INTEGER NOT NULL DEFAULT 0,
    created_at   INTEGER NOT NULL,           -- epoch ms
    PRIMARY KEY (workspace_id, path)
);
