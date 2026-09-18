-- Quorum persistence schema (single consolidated baseline).

CREATE TABLE accounts (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL DEFAULT '',
    email      TEXT,
    avatar_url TEXT,
    first_name TEXT,
    last_name  TEXT,
    created_at INTEGER NOT NULL
);

CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE projects (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE project_settings (
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    key        TEXT NOT NULL,
    value      TEXT NOT NULL,
    PRIMARY KEY (project_id, key)
);
CREATE INDEX idx_project_settings_project_id ON project_settings(project_id);

CREATE TABLE repos (
    id         TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    path       TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_repos_project_id ON repos(project_id);

-- The sidebar "agent": a feature work-area in a project.
CREATE TABLE workspaces (
    id                 TEXT PRIMARY KEY,
    project_id         TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name               TEXT NOT NULL,
    task               TEXT NOT NULL DEFAULT '',
    created_at         INTEGER NOT NULL,
    setup_completed_at INTEGER,
    stopped_at         INTEGER,
    archived_at        INTEGER
);
CREATE INDEX idx_workspaces_project_id ON workspaces(project_id);

-- Per-repo checkout under a workspace; multi-repo => multiple rows.
CREATE TABLE worktrees (
    id                TEXT PRIMARY KEY,
    workspace_id      TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    repo_id           TEXT NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
    subdir            TEXT NOT NULL,
    branch            TEXT,
    parent_branch     TEXT,
    branch_tip_sha    TEXT,
    parent_branch_sha TEXT,
    diff_additions    INTEGER NOT NULL DEFAULT 0,
    diff_deletions    INTEGER NOT NULL DEFAULT 0,
    created_at        INTEGER NOT NULL,
    UNIQUE(workspace_id, repo_id)
);
CREATE INDEX idx_worktrees_workspace_id ON worktrees(workspace_id);
CREATE INDEX idx_worktrees_repo_id ON worktrees(repo_id);

-- A provider run within a workspace (exactly one today; many with workflows).
CREATE TABLE sessions (
    id                  TEXT PRIMARY KEY,
    workspace_id        TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    provider            TEXT NOT NULL DEFAULT 'claude',
    view                TEXT NOT NULL DEFAULT 'custom' CHECK (view IN ('custom','native')),
    provider_session_id TEXT,
    last_error          TEXT,
    created_at          INTEGER NOT NULL
);
CREATE INDEX idx_sessions_workspace_id ON sessions(workspace_id);

-- The canonical raw-event work log: one session's complete, ordered event
-- stream (user input + agent output), replayed through reduce() on restore.
CREATE TABLE session_events (
    id         INTEGER PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    seq        INTEGER NOT NULL,
    event_json TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_session_events_session_seq ON session_events(session_id, seq);
