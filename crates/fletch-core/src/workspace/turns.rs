//! `impl WorkspaceManager` — Fletch-origin user turns and their timing.

use super::sessions::current_session_id;
use super::*;

/// One session's user turns in seq order, each tagged `inherited` as given —
/// the row decoder behind the stitched history read (`lineage`). `below`
/// keeps only the turns whose matched prompt record lies below that seq; an
/// unmatched turn has no position, so it is left out. `None` keeps every turn,
/// pending ones included.
pub(super) fn query_turns(
    conn: &Connection,
    session_id: &str,
    below: Option<i64>,
    inherited: bool,
) -> Result<Vec<UserTurn>> {
    let mut stmt = conn.prepare(
        "SELECT t.turn_id, t.seq, t.text, t.attachments, t.native_id, t.started_at, t.ended_at,
                t.outcome
         FROM session_user_turns t
         LEFT JOIN transcripts.session_records r ON r.session_id = t.session_id AND r.native_id = t.native_id
         WHERE t.session_id = ?1 AND (?2 IS NULL OR r.seq < ?2)
         ORDER BY t.seq ASC",
    )?;
    // (turn_id, seq, text, attachments, native_id, started_at, ended_at, outcome)
    type UserTurnRow = (
        String,
        i64,
        String,
        String,
        Option<String>,
        Option<i64>,
        Option<i64>,
        Option<String>,
    );
    let rows: Vec<UserTurnRow> = stmt
        .query_map(rusqlite::params![session_id, below], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
            ))
        })?
        .collect::<std::result::Result<_, rusqlite::Error>>()?;
    rows.into_iter()
        .map(
            |(turn_id, seq, text, attachments_text, native_id, started_at, ended_at, outcome)| {
                let attachments = serde_json::from_str(&attachments_text)
                    .map_err(|e| Error::Other(format!("deserialize attachments: {e}")))?;
                Ok(UserTurn {
                    turn_id,
                    seq,
                    text,
                    attachments,
                    native_id,
                    started_at,
                    ended_at,
                    outcome,
                    inherited,
                })
            },
        )
        .collect()
}

impl WorkspaceManager {
    // ── Outgoing user turns (session_user_turns) ──────────────────────────

    /// Insert an outgoing user message for the workspace's current session.
    /// Idempotent on `turn_id` (send auto-retries reuse the same id). Returns
    /// `true` if a new row was inserted, `false` on duplicate / no session.
    ///
    /// Callers insert before the message reaches the agent, so the row's
    /// `record_watermark` (the session's last record seq, read in the same
    /// statement) is below the record its prompt produces
    /// (`associate_pending_user_turns`).
    pub fn insert_user_turn(
        &self,
        workspace_id: &str,
        turn_id: &str,
        text: &str,
        attachments: &[String],
    ) -> Result<bool> {
        let conn = self.db.lock();
        let Some(sid) = current_session_id(&conn, workspace_id) else {
            return Ok(false);
        };
        let attachments_json = serde_json::to_string(attachments)
            .map_err(|e| Error::Other(format!("serialize attachments: {e}")))?;

        let tx = conn.unchecked_transaction()?;
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM session_user_turns WHERE session_id = ?1",
            [&sid],
            |r| r.get(0),
        )?;
        let n = tx.execute(
            "INSERT OR IGNORE INTO session_user_turns
                (turn_id, session_id, seq, text, attachments, native_id, record_watermark,
                 created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL,
                     (SELECT COALESCE(MAX(seq), 0) FROM transcripts.session_records WHERE session_id = ?2),
                     ?6)",
            rusqlite::params![turn_id, sid, seq, text, attachments_json, now_millis()],
        )?;
        tx.commit()?;
        Ok(n > 0)
    }

    /// Withdraw turns that never reached the agent as themselves: the row
    /// [`Self::insert_user_turn`] just created for a send that then failed, or
    /// the rows of queued follow-ups that went out folded into another turn's
    /// coalesced prompt. Only a row that never ran, never matched and has no
    /// outcome is removed; any of those makes it a turn of its own.
    pub fn delete_pending_user_turns(&self, turn_ids: &[String]) -> Result<()> {
        let conn = self.db.lock();
        let mut stmt = conn.prepare(
            "DELETE FROM session_user_turns
             WHERE turn_id = ?1 AND native_id IS NULL AND started_at IS NULL
               AND outcome IS NULL",
        )?;
        for turn_id in turn_ids {
            stmt.execute([turn_id])?;
        }
        Ok(())
    }

    /// Stamp a turn's run start when it flips to Running, with the caller's
    /// timestamp so the same value reaches the live timer (via the `turn:started`
    /// event) and the persisted duration. Guarded on `started_at IS NULL` so a
    /// delivery retry (same `turn_id`) never resets the clock. No-op when the row
    /// doesn't exist (native PTY turns carry no timing row).
    pub fn mark_user_turn_started(&self, turn_id: &str, started_at: i64) -> Result<()> {
        let conn = self.db.lock();
        conn.execute(
            "UPDATE session_user_turns SET started_at = ?1
             WHERE turn_id = ?2 AND started_at IS NULL",
            rusqlite::params![started_at, turn_id],
        )?;
        Ok(())
    }

    /// Withdraw a turn's run start: the hand-off to the process failed after
    /// the stamp above, so the turn never ran. Clearing `started_at` keeps the
    /// row out of `mark_user_turn_ended`'s open-turn set — an unrelated Idle
    /// would otherwise close it — and lets the retry's own stamp land, since
    /// that stamp only writes a null. A turn already closed is left alone.
    pub fn unmark_user_turn_started(&self, turn_id: &str) -> Result<()> {
        let conn = self.db.lock();
        conn.execute(
            "UPDATE session_user_turns SET started_at = NULL
             WHERE turn_id = ?1 AND ended_at IS NULL",
            [turn_id],
        )?;
        Ok(())
    }

    /// Close the in-flight turn at turn end by stamping `ended_at` and
    /// `outcome` on the open turn (started, not yet ended) of the workspace's
    /// current session, and return its stats for telemetry. `None` when none is open — e.g. the
    /// resting Idle emitted at spawn, or a native turn with no timing row. At
    /// most one turn is ever open per session (each end closes the open turn
    /// before the next one starts), but the `WHERE` would safely close all open
    /// turns if one were ever stranded; duration then anchors on the earliest.
    pub fn mark_user_turn_ended(
        &self,
        workspace_id: &str,
        outcome: TurnOutcome,
    ) -> Result<Option<ClosedTurn>> {
        let conn = self.db.lock();
        let Some(sid) = current_session_id(&conn, workspace_id) else {
            return Ok(None);
        };
        let started_at: Option<i64> = conn.query_row(
            "SELECT MIN(started_at) FROM session_user_turns
             WHERE session_id = ?1 AND started_at IS NOT NULL AND ended_at IS NULL",
            [&sid],
            |r| r.get(0),
        )?;
        let Some(started_at) = started_at else {
            return Ok(None);
        };
        let now = now_millis();
        conn.execute(
            "UPDATE session_user_turns SET ended_at = ?1, outcome = ?3
             WHERE session_id = ?2 AND started_at IS NOT NULL AND ended_at IS NULL",
            rusqlite::params![now, sid, outcome.as_str()],
        )?;
        // Records land before the terminal event that trips turn-end detection,
        // so the window is complete by the time we get here.
        let record_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM transcripts.session_records
             WHERE session_id = ?1 AND created_at BETWEEN ?2 AND ?3",
            rusqlite::params![sid, started_at, now],
            |r| r.get(0),
        )?;
        Ok(Some(ClosedTurn {
            duration_ms: now - started_at,
            record_count,
        }))
    }

    /// Mark turns whose messages were dropped before they ever ran — queued
    /// follow-ups an archive or discard throws away — as `failed`, so their
    /// rows read as given up on rather than waiting forever. Only a row that
    /// never started and has no outcome yet is touched; a message that was
    /// queued without a row (the busy path writes none) has nothing to mark.
    pub fn mark_user_turns_abandoned(&self, turn_ids: &[String]) -> Result<()> {
        let conn = self.db.lock();
        let mut stmt = conn.prepare(
            "UPDATE session_user_turns SET outcome = ?2
             WHERE turn_id = ?1 AND started_at IS NULL AND outcome IS NULL",
        )?;
        for turn_id in turn_ids {
            stmt.execute(rusqlite::params![turn_id, TurnOutcome::Failed.as_str()])?;
        }
        Ok(())
    }

    /// Match pending (`native_id IS NULL`) user turns to their canonical
    /// `session_records` user-message rows and fill in `native_id`. Run at
    /// turn-end after transcript ingest. Matching: for each pending turn (seq
    /// order) find the lowest-seq transcript record not already claimed, past
    /// the turn's `record_watermark`, whose body contains the turn's
    /// distinctive marker — the first attachment path (injected by the runner
    /// as `Attached file: <path>`) when present, else the prompt text. Returns
    /// the number newly associated.
    ///
    /// A record at or below the watermark was stored before the turn's row,
    /// and so before its message went out (`insert_user_turn`): it can't be the
    /// prompt. It can quote it, though, and a short prompt ("yes") is quoted
    /// often; matched to that, every cut at the turn (`resolve_anchor`) would
    /// land in the wrong place. A seq is fixed for good: a re-ingested record
    /// keeps its first row (`append_session_records`). A row from before the
    /// watermark existed has none, and matches any record, as it always did.
    pub fn associate_pending_user_turns(&self, workspace_id: &str) -> Result<usize> {
        let conn = self.db.lock();
        let Some(sid) = current_session_id(&conn, workspace_id) else {
            return Ok(0);
        };

        // Pending turns, oldest first.
        let pending: Vec<(String, String, String, Option<i64>)> = {
            let mut stmt = conn.prepare(
                "SELECT turn_id, text, attachments, record_watermark FROM session_user_turns
                 WHERE session_id = ?1 AND native_id IS NULL ORDER BY seq ASC",
            )?;
            let v = stmt
                .query_map([&sid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<std::result::Result<_, rusqlite::Error>>()?;
            v
        };
        if pending.is_empty() {
            return Ok(0);
        }

        // Transcript records, oldest first.
        let records: Vec<(i64, String, String)> = {
            let mut stmt = conn.prepare(
                "SELECT seq, native_id, body FROM transcripts.session_records
                 WHERE session_id = ?1 AND source = 'transcript' ORDER BY seq ASC",
            )?;
            let v = stmt
                .query_map([&sid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<std::result::Result<_, rusqlite::Error>>()?;
            v
        };

        // native_ids already claimed by any user turn for this session.
        let mut claimed: std::collections::HashSet<String> = {
            let mut stmt = conn.prepare(
                "SELECT native_id FROM session_user_turns
                 WHERE session_id = ?1 AND native_id IS NOT NULL",
            )?;
            let v = stmt
                .query_map([&sid], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<_, rusqlite::Error>>()?;
            v
        };

        let tx = conn.unchecked_transaction()?;
        let mut associated = 0usize;
        for (turn_id, text, attachments_text, watermark) in pending {
            let attachments: Vec<String> =
                serde_json::from_str(&attachments_text).unwrap_or_default();
            // Distinctive needle: an attachment path beats the prompt text
            // (paths are unique; text can be empty or duplicated).
            let needle = attachments.first().cloned().unwrap_or(text);
            if needle.is_empty() {
                continue;
            }
            // The body is stored as serde_json::to_string(value), so characters
            // like newlines appear JSON-escaped (\n) in the stored string. Escape
            // the needle the same way so the substring match works for multi-line
            // messages. serde_json::to_string wraps in quotes; strip them.
            let needle_escaped = serde_json::to_string(&needle)
                .map(|s| s[1..s.len() - 1].to_string())
                .unwrap_or(needle.clone());
            let hit = records.iter().find(|(seq, nid, body)| {
                // `map_or`, not `is_none_or`: the crate's rust-version is 1.77.
                watermark.map_or(true, |w| *seq > w)
                    && !claimed.contains(nid)
                    && body.contains(&needle_escaped)
            });
            if let Some((_, nid, _)) = hit {
                tx.execute(
                    "UPDATE session_user_turns SET native_id = ?1 WHERE turn_id = ?2",
                    rusqlite::params![nid, turn_id],
                )?;
                claimed.insert(nid.clone());
                associated += 1;
            }
        }
        tx.commit()?;
        Ok(associated)
    }
}
