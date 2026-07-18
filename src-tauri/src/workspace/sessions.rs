//! `impl WorkspaceManager` — canonical session-record persistence.

use super::*;

impl WorkspaceManager {
    // ── Session event log ─────────────────────────────────────────────────

    /// Append a canonical record to the workspace's current session. Idempotent
    /// on `(session_id, native_id)`: a duplicate native_id is ignored and the
    /// original row's body is retained. Returns `true` if a new row was
    /// inserted, `false` if it was a duplicate or the workspace has no session.
    /// Append many transcript records in a single transaction. Same idempotency
    /// as `append_session_record` (ignored on a `(session_id, native_id)`
    /// conflict), but one commit for the whole batch instead of one per record —
    /// so turn-end ingest is O(batch) commits, not O(conversation). `seq` stays
    /// contiguous: an ignored duplicate doesn't burn a number. Returns how many
    /// rows were actually inserted.
    pub fn append_session_records(
        &self,
        workspace_id: &str,
        provider: &str,
        source: &str,
        agent_version: Option<&str>,
        records: &[(&str, &serde_json::Value)],
    ) -> Result<usize> {
        if records.is_empty() {
            return Ok(0);
        }
        let conn = self.db.lock();

        let sid: Option<String> = conn
            .query_row(
                "SELECT id FROM sessions WHERE workspace_id = ?1 ORDER BY created_at DESC LIMIT 1",
                [workspace_id],
                |r| r.get(0),
            )
            .ok();
        let Some(sid) = sid else {
            return Ok(0);
        };

        let now = now_millis();
        let tx = conn.unchecked_transaction()?;
        let mut seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM session_records WHERE session_id = ?1",
            [&sid],
            |r| r.get(0),
        )?;
        let mut inserted = 0usize;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO session_records
                    (session_id, seq, provider, source, native_id, agent_version, body, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for (native_id, body) in records {
                let body_json = serde_json::to_string(body)
                    .map_err(|e| Error::Other(format!("serialize record body: {e}")))?;
                let next = seq + 1;
                let n = stmt.execute(rusqlite::params![
                    sid,
                    next,
                    provider,
                    source,
                    native_id,
                    agent_version,
                    body_json,
                    now
                ])?;
                if n > 0 {
                    seq = next; // consumed only on a real insert; dups keep seq dense
                    inserted += 1;
                }
            }
        }
        tx.commit()?;

        Ok(inserted)
    }

    /// Byte offset into the current session's transcript up to which records have
    /// been ingested — the resume point for an incremental tail read. 0 if there
    /// is no session yet or nothing has been ingested.
    pub fn session_ingest_offset(&self, workspace_id: &str) -> Result<u64> {
        let conn = self.db.lock();
        let offset: Option<i64> = conn
            .query_row(
                "SELECT ingest_offset FROM sessions WHERE workspace_id = ?1 ORDER BY created_at DESC LIMIT 1",
                [workspace_id],
                |r| r.get(0),
            )
            .ok();
        Ok(offset.unwrap_or(0).max(0) as u64)
    }

    /// Persist the tail offset for the current session after an incremental read.
    pub fn set_session_ingest_offset(&self, workspace_id: &str, offset: u64) -> Result<()> {
        let conn = self.db.lock();
        conn.execute(
            "UPDATE sessions SET ingest_offset = ?2
             WHERE id = (SELECT id FROM sessions WHERE workspace_id = ?1 ORDER BY created_at DESC LIMIT 1)",
            rusqlite::params![workspace_id, offset as i64],
        )?;
        Ok(())
    }

    /// Count of records already ingested for the current session (= MAX(seq)) —
    /// the starting index for positional `ln:{i}` native ids on the next read.
    pub fn session_record_count(&self, workspace_id: &str) -> Result<usize> {
        let conn = self.db.lock();
        let sid: Option<String> = conn
            .query_row(
                "SELECT id FROM sessions WHERE workspace_id = ?1 ORDER BY created_at DESC LIMIT 1",
                [workspace_id],
                |r| r.get(0),
            )
            .ok();
        let Some(sid) = sid else {
            return Ok(0);
        };
        let count: i64 = conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM session_records WHERE session_id = ?1",
            [&sid],
            |r| r.get(0),
        )?;
        Ok(count.max(0) as usize)
    }

    /// Ingest timestamp (ms epoch) of the most recent `session_records` row for
    /// the workspace's current session, or `None` if nothing has been ingested
    /// yet. The workflow stall watchdog compares this against `stall_timeout` to
    /// tell a working agent from a silent one (see `workflow::attempt`).
    pub fn last_activity(&self, workspace_id: &str) -> Option<i64> {
        let conn = self.db.lock();
        let sid: String = conn
            .query_row(
                "SELECT id FROM sessions WHERE workspace_id = ?1 ORDER BY created_at DESC LIMIT 1",
                [workspace_id],
                |r| r.get(0),
            )
            .ok()?;
        // MAX over an empty set is SQL NULL, so decode into an Option and let a
        // session with no records yet report `None` rather than 0.
        conn.query_row(
            "SELECT MAX(created_at) FROM session_records WHERE session_id = ?1",
            [&sid],
            |r| r.get::<_, Option<i64>>(0),
        )
        .ok()
        .flatten()
    }

    /// All canonical records for the workspace's current session, in seq order.
    pub fn read_session_records(&self, workspace_id: &str) -> Result<Vec<SessionRecord>> {
        let conn = self.db.lock();

        let sid: Option<String> = conn
            .query_row(
                "SELECT id FROM sessions WHERE workspace_id = ?1 ORDER BY created_at DESC LIMIT 1",
                [workspace_id],
                |r| r.get(0),
            )
            .ok();

        let Some(sid) = sid else {
            return Ok(vec![]);
        };

        let mut stmt = conn.prepare(
            "SELECT seq, provider, source, native_id, agent_version, body
             FROM session_records WHERE session_id = ?1 ORDER BY seq ASC",
        )?;

        let rows: Vec<(i64, String, String, String, Option<String>, String)> = stmt
            .query_map([&sid], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })?
            .collect::<std::result::Result<_, rusqlite::Error>>()?;

        rows.into_iter()
            .map(
                |(seq, provider, source, native_id, agent_version, body_text)| {
                    let body = serde_json::from_str(&body_text)
                        .map_err(|e| Error::Other(format!("deserialize record body: {e}")))?;
                    Ok(SessionRecord {
                        seq,
                        provider,
                        source,
                        native_id,
                        agent_version,
                        body,
                    })
                },
            )
            .collect()
    }
}
