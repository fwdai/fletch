//! `impl WorkspaceManager` — the pending outgoing-message queue.

use super::*;

impl WorkspaceManager {
    // ── Queued follow-up messages (pending_messages) ─────────────────────
    // Durable mirror of the in-memory `MessageQueue` (see `message_queue`), so
    // follow-ups enqueued behind an in-flight turn survive an app restart.
    // Rows are written on enqueue and dropped once the coalesced batch is
    // delivered (or the agent is torn down). Unlike `session_user_turns` these
    // are *un-delivered* messages: once delivered they become a normal turn and
    // their pending row is deleted.

    /// Persist one queued follow-up for the workspace's current session, at the
    /// next enqueue seq. Best-effort no-op (`Ok`) when the workspace has no
    /// session yet — a follow-up is only ever queued behind a live turn, which
    /// implies a session, so that case is defensive.
    pub fn enqueue_pending_message(
        &self,
        workspace_id: &str,
        msg: &crate::message_queue::PendingMsg,
    ) -> Result<()> {
        let conn = self.db.lock();
        let Some(sid) = current_session_id(&conn, workspace_id) else {
            return Ok(());
        };
        let attachments_json = serde_json::to_string(&msg.attachments)
            .map_err(|e| Error::Other(format!("serialize attachments: {e}")))?;
        let tx = conn.unchecked_transaction()?;
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM pending_messages WHERE session_id = ?1",
            [&sid],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO pending_messages
                (session_id, seq, turn_id, text, attachments, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                sid,
                seq,
                msg.turn_id,
                msg.text,
                attachments_json,
                now_millis()
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Drop the persisted follow-ups for the workspace's current session that
    /// were just delivered, keeping only the `keep` ids. Called after a flush:
    /// `keep` is whatever is still queued in memory (a follow-up that arrived
    /// during the delivery window), so its row survives while the delivered
    /// batch — including any coalesced-away rows from a prior failed flush — is
    /// cleared. `keep` empty ⇒ clear the whole session's queue.
    pub fn delete_pending_messages_except(
        &self,
        workspace_id: &str,
        keep: &[String],
    ) -> Result<()> {
        let conn = self.db.lock();
        let Some(sid) = current_session_id(&conn, workspace_id) else {
            return Ok(());
        };
        if keep.is_empty() {
            conn.execute("DELETE FROM pending_messages WHERE session_id = ?1", [&sid])?;
            return Ok(());
        }
        let placeholders = std::iter::repeat("?")
            .take(keep.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "DELETE FROM pending_messages WHERE session_id = ? AND turn_id NOT IN ({placeholders})"
        );
        let mut binds: Vec<&str> = Vec::with_capacity(keep.len() + 1);
        binds.push(sid.as_str());
        binds.extend(keep.iter().map(String::as_str));
        conn.execute(&sql, rusqlite::params_from_iter(binds))?;
        Ok(())
    }

    /// Drop every persisted follow-up for the workspace's current session
    /// (archive / discard teardown). Discard removes the workspace row and the
    /// FK cascade handles it too; archive keeps the row, so this clears it.
    pub fn clear_pending_messages(&self, workspace_id: &str) -> Result<()> {
        let conn = self.db.lock();
        let Some(sid) = current_session_id(&conn, workspace_id) else {
            return Ok(());
        };
        conn.execute("DELETE FROM pending_messages WHERE session_id = ?1", [&sid])?;
        Ok(())
    }

    /// Every persisted follow-up across all non-archived workspaces, for
    /// rehydrating the in-memory queue at startup. Returns `(workspace_id,
    /// PendingMsg)` pairs in per-workspace enqueue (seq) order. Archived
    /// workspaces are excluded so a leftover row can never resurrect a queue for
    /// an agent the user has put away.
    pub fn read_all_pending_messages(
        &self,
    ) -> Result<Vec<(String, crate::message_queue::PendingMsg)>> {
        let conn = self.db.lock();
        let mut stmt = conn.prepare(
            "SELECT s.workspace_id, p.turn_id, p.text, p.attachments
             FROM pending_messages p
             JOIN sessions s ON s.id = p.session_id
             JOIN workspaces w ON w.id = s.workspace_id
             WHERE w.archived_at IS NULL
             ORDER BY s.workspace_id, p.seq ASC",
        )?;
        let rows: Vec<(String, String, String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<std::result::Result<_, rusqlite::Error>>()?;
        rows.into_iter()
            .map(|(workspace_id, turn_id, text, attachments_text)| {
                let attachments = serde_json::from_str(&attachments_text)
                    .map_err(|e| Error::Other(format!("deserialize attachments: {e}")))?;
                Ok((
                    workspace_id,
                    crate::message_queue::PendingMsg {
                        turn_id,
                        text,
                        attachments,
                    },
                ))
            })
            .collect()
    }
}
