-- Quorum persistence schema

-- Pinned sidebar repos
CREATE TABLE workspace_repos (
    id TEXT PRIMARY KEY,
    repo_path TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL
);

-- Agent records (replaces agents array in workspaces.json)
CREATE TABLE agents (
    id TEXT PRIMARY KEY,
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

CREATE INDEX idx_agents_status ON agents(status);
CREATE INDEX idx_agents_created_at ON agents(created_at);
CREATE INDEX idx_agents_archived_at ON agents(archived_at);

-- Repos tracked per agent (one agent can have multiple worktrees)
CREATE TABLE agent_repos (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    repo_path TEXT NOT NULL,
    subdir TEXT NOT NULL,
    branch TEXT,
    parent_branch TEXT,
    is_primary INTEGER NOT NULL DEFAULT 0 CHECK (is_primary IN (0, 1)),
    -- Populated at archive time, NULL for live agents
    branch_tip_sha TEXT,
    parent_branch_sha TEXT,
    diff_additions INTEGER NOT NULL DEFAULT 0,
    diff_deletions INTEGER NOT NULL DEFAULT 0,
    UNIQUE(agent_id, subdir)
);

CREATE INDEX idx_agent_repos_agent_id ON agent_repos(agent_id);

-- Conversation messages
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
