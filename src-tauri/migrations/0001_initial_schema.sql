-- Quorum persistence schema

CREATE TABLE accounts (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL DEFAULT '',
    email TEXT,
    avatar_url TEXT,
    created_at INTEGER NOT NULL
);

CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE repos (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    path TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL
);

CREATE INDEX idx_repos_project_id ON repos(project_id);

CREATE TABLE agents (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    provider TEXT NOT NULL DEFAULT 'claude',
    task TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'spawning'
        CHECK (status IN ('spawning', 'running', 'idle', 'stopped', 'error')),
    view TEXT NOT NULL DEFAULT 'custom'
        CHECK (view IN ('custom', 'native')),
    session_id TEXT,
    created_at INTEGER NOT NULL,
    last_error TEXT,
    archived_at INTEGER
);

CREATE INDEX idx_agents_project_id ON agents(project_id);
CREATE INDEX idx_agents_status ON agents(status);
CREATE INDEX idx_agents_created_at ON agents(created_at);

CREATE TABLE worktrees (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    repo_id TEXT NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
    subdir TEXT NOT NULL,
    branch TEXT,
    parent_branch TEXT,
    -- Populated at archive time, NULL for live agents
    branch_tip_sha TEXT,
    parent_branch_sha TEXT,
    diff_additions INTEGER NOT NULL DEFAULT 0,
    diff_deletions INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    UNIQUE(agent_id, repo_id)
);

CREATE INDEX idx_worktrees_agent_id ON worktrees(agent_id);
CREATE INDEX idx_worktrees_repo_id ON worktrees(repo_id);

CREATE TABLE messages (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    kind TEXT NOT NULL
        CHECK (kind IN ('user_message', 'agent_message', 'tool_call', 'tool_result', 'notice')),
    content TEXT NOT NULL DEFAULT '',
    metadata_json TEXT,
    sequence INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX idx_messages_agent_id ON messages(agent_id, sequence);

-- Full-text search on message content
CREATE VIRTUAL TABLE messages_fts USING fts5(
    content,
    content='messages',
    content_rowid='rowid'
);

CREATE TRIGGER messages_fts_insert AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts(rowid, content) VALUES (new.rowid, new.content);
END;

CREATE TRIGGER messages_fts_delete AFTER DELETE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, content)
        VALUES ('delete', old.rowid, old.content);
END;

CREATE TRIGGER messages_fts_update AFTER UPDATE OF content ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, content)
        VALUES ('delete', old.rowid, old.content);
    INSERT INTO messages_fts(rowid, content) VALUES (new.rowid, new.content);
END;
