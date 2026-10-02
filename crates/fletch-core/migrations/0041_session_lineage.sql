-- Session lineage and the current session (docs/fork-and-rewind.md).
--
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
