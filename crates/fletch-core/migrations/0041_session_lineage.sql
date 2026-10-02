-- Session lineage and the current session (docs/fork-and-rewind.md).
--
-- A session's history is its parent's history below `parent_cut_seq` (an
-- exclusive `session_records.seq` in the parent's own seq space, applied
-- recursively up the chain), followed by its own records. History is stored
-- once and referenced, never copied. Both columns are NULL for a root session
-- and set together for a child.
--
-- The parent reference deliberately has no ON DELETE action: every path that
-- deletes sessions must first detach the children it would orphan
-- (`workspace::lineage::detach_children`), and one that forgets fails on this
-- foreign key instead of silently cutting a child's history short.
ALTER TABLE sessions ADD COLUMN parent_session_id TEXT REFERENCES sessions(id);
ALTER TABLE sessions ADD COLUMN parent_cut_seq INTEGER
    CHECK ((parent_cut_seq IS NULL) = (parent_session_id IS NULL));
CREATE INDEX idx_sessions_parent ON sessions(parent_session_id);

-- A workspace has exactly one current session: the one not superseded. Older
-- sessions stay only as ancestors of a newer one.
ALTER TABLE sessions ADD COLUMN superseded_at INTEGER;

-- Every workspace has had one session so far, but the index below must not be
-- able to fail an upgrade: should a workspace hold several, the newest stays
-- current, exactly as the `ORDER BY created_at DESC LIMIT 1` reads resolved it.
UPDATE sessions SET superseded_at = created_at
 WHERE EXISTS (SELECT 1 FROM sessions newer
                WHERE newer.workspace_id = sessions.workspace_id
                  AND (newer.created_at > sessions.created_at
                       OR (newer.created_at = sessions.created_at AND newer.id > sessions.id)));

CREATE UNIQUE INDEX idx_sessions_current ON sessions(workspace_id) WHERE superseded_at IS NULL;
