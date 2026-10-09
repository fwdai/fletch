//! `impl WorkspaceManager` — canonical session-record persistence, the one
//! definition of a workspace's current session, and starting a new one in
//! place.

use rusqlite::OptionalExtension;

use super::*;

/// A session's own transcript, written by Fletch before its first launch so
/// the CLI resumes the conversation the session continues
/// (`Supervisor::materialize`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeTranscript {
    /// The provider session the transcript was written as.
    pub provider_session_id: String,
    /// How many records it opens with: the copied history, which the session
    /// already shows through its lineage, so ingestion drops them
    /// (`sessions.transcript_prefix`).
    pub prefix: usize,
}

/// The workspace's current session: its one session that hasn't been
/// superseded (`idx_sessions_current` allows at most one). Every read or write
/// that means "this workspace's conversation" resolves its session here; SQL
/// that has to say it inline (a join, a single-statement update) uses the same
/// predicate, `superseded_at IS NULL`. `None` when the workspace has no session.
pub(super) fn current_session_id(conn: &Connection, workspace_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT id FROM sessions WHERE workspace_id = ?1 AND superseded_at IS NULL",
        [workspace_id],
        |r| r.get(0),
    )
    .ok()
}

/// The sessions of `workspace_ids`, read inside the transaction that is about
/// to delete them, so `delete_transcripts_for_sessions` can run once it has
/// committed.
pub(super) fn session_ids_for_workspaces(
    conn: &Connection,
    workspace_ids: &[String],
) -> Result<Vec<String>> {
    if workspace_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(&format!(
        "SELECT id FROM sessions WHERE workspace_id IN ({})",
        placeholders(workspace_ids.len())
    ))?;
    let ids = stmt
        .query_map(rusqlite::params_from_iter(workspace_ids), |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(ids)
}

/// Transcript rows a workspace deletion owes, applied only after the
/// transaction that removed or handed over their sessions has committed.
/// `session_records` lives in the attached `transcripts` file, where no
/// foreign key reaches and a WAL commit is atomic per file, not across the
/// two: deleting inside that transaction could leave a session durable with
/// its transcript gone. In this order a crash leaves rows behind instead —
/// orphans the startup sweep in `database::connection` removes, or rows past
/// a handed-over session's cut, which no read ever reaches.
#[derive(Default)]
pub(super) struct TranscriptCleanup {
    /// Sessions deleted outright: every row goes.
    pub(super) sessions: Vec<String>,
    /// Sessions handed over to an heir (`lineage::detach_children`): rows at
    /// or past the cut go.
    pub(super) trims: Vec<(String, i64)>,
}

impl TranscriptCleanup {
    /// Best-effort: the authoritative transaction has already committed, so a
    /// failure here (busy, full, I/O) must not be reported as a failed
    /// deletion. What it leaves behind is exactly what a crash would, and is
    /// handled the same way.
    pub(super) fn apply(self, conn: &Connection) {
        if let Err(e) = self.run(conn) {
            tracing::warn!(error = %e, "transcript cleanup failed; rows left for the startup sweep");
        }
    }

    fn run(&self, conn: &Connection) -> Result<()> {
        // Chunked to stay under SQLite's bound-parameter limit.
        for chunk in self.sessions.chunks(500) {
            conn.execute(
                &format!(
                    "DELETE FROM transcripts.session_records WHERE session_id IN ({})",
                    placeholders(chunk.len())
                ),
                rusqlite::params_from_iter(chunk),
            )?;
        }
        for (session, cut) in &self.trims {
            conn.execute(
                "DELETE FROM transcripts.session_records WHERE session_id = ?1 AND seq >= ?2",
                rusqlite::params![session, cut],
            )?;
        }
        Ok(())
    }
}

/// `?1,?2,…,?n`, for an `IN` list bound from a slice.
pub(super) fn placeholders(n: usize) -> String {
    (1..=n)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// One session's own records with `seq < below`, in seq order, each tagged
/// `inherited` as given — the single row decoder behind both the own-session
/// read and the stitched history read (`lineage`).
pub(super) fn query_records(
    conn: &Connection,
    session_id: &str,
    below: i64,
    inherited: bool,
) -> Result<Vec<SessionRecord>> {
    query_newest_records(conn, session_id, below, inherited, None)
}

/// [`query_records`] cut to the newest `limit` of them (all for `None`), still
/// in seq order. Read newest first so a transcript page costs its own size,
/// not the session's.
pub(super) fn query_newest_records(
    conn: &Connection,
    session_id: &str,
    below: i64,
    inherited: bool,
    limit: Option<usize>,
) -> Result<Vec<SessionRecord>> {
    // SQLite reads a negative LIMIT as none.
    let limit = limit.map_or(-1, |n| i64::try_from(n).unwrap_or(i64::MAX));
    let mut stmt = conn.prepare(
        "SELECT seq, provider, source, native_id, agent_version, body
         FROM transcripts.session_records WHERE session_id = ?1 AND seq < ?2
         ORDER BY seq DESC LIMIT ?3",
    )?;
    let mut rows: Vec<(i64, String, String, String, Option<String>, String)> = stmt
        .query_map(rusqlite::params![session_id, below, limit], |r| {
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
    rows.reverse();

    rows.into_iter()
        .map(
            |(seq, provider, source, native_id, agent_version, body_text)| {
                let body = serde_json::from_str(&body_text)
                    .map_err(|e| Error::Other(format!("deserialize record body: {e}")))?;
                Ok(SessionRecord {
                    session_id: session_id.to_string(),
                    seq,
                    provider,
                    source,
                    native_id,
                    agent_version,
                    body,
                    inherited,
                })
            },
        )
        .collect()
}

impl WorkspaceManager {
    // ── Session event log ─────────────────────────────────────────────────

    /// Append many transcript records to the workspace's current session in a
    /// single transaction. Idempotent on `(session_id, native_id)`: a duplicate
    /// native_id is ignored and the original row's body is retained. One commit
    /// for the whole batch instead of one per record, so turn-end ingest is
    /// O(batch) commits, not O(conversation). An ignored duplicate doesn't burn
    /// a `seq`. Returns how many rows were actually inserted (0 when the
    /// workspace has no session).
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
        let Some(sid) = current_session_id(&conn, workspace_id) else {
            return Ok(0);
        };

        let now = now_millis();
        let tx = conn.unchecked_transaction()?;
        let mut seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM transcripts.session_records WHERE session_id = ?1",
            [&sid],
            |r| r.get(0),
        )?;
        let mut inserted = 0usize;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO transcripts.session_records
                    (session_id, seq, provider, source, native_id, agent_version, body, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for (native_id, body) in records {
                let next = seq + 1;
                let body_json = serde_json::to_string(body)
                    .map_err(|e| Error::Other(format!("serialize record body: {e}")))?;
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
                "SELECT ingest_offset FROM sessions WHERE workspace_id = ?1 AND superseded_at IS NULL",
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
            "UPDATE sessions SET ingest_offset = ?2 WHERE workspace_id = ?1 AND superseded_at IS NULL",
            rusqlite::params![workspace_id, offset as i64],
        )?;
        Ok(())
    }

    /// Start a new current session in `workspace_id` that continues `lineage`
    /// — rewind's conversation half — and return its id. One transaction:
    ///
    /// - The current session is superseded. Its open turn is closed and its
    ///   queued messages are dropped: nothing will deliver them now.
    /// - The new session takes the old one's settings (provider, view, effort,
    ///   model, brief, custom agent, skills, MCP servers) but none of its
    ///   state: no handoff context, no error, nothing ingested, and a provider
    ///   session of its own: `native`'s, the transcript written for it, else
    ///   claude's minted here, as `insert_agent` does, or a per-turn
    ///   provider's captured from its first turn.
    ///
    /// The view carries over unless the new session can't open in the native
    /// view yet: a per-turn provider without a session has none to resume
    /// until its first turn (`Supervisor::switch_view` refuses it too).
    pub fn start_session(
        &self,
        workspace_id: &str,
        lineage: &SessionLineage,
        native: Option<&NativeTranscript>,
    ) -> Result<String> {
        let conn = self.db.lock();
        let tx = conn.unchecked_transaction()?;
        let old = current_session_id(&tx, workspace_id)
            .ok_or_else(|| Error::AgentNotFound(workspace_id.to_string()))?;
        let (provider, view): (String, String) = tx.query_row(
            "SELECT provider, view FROM sessions WHERE id = ?1",
            [&old],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let provider_session_id = match native {
            Some(native) => Some(native.provider_session_id.clone()),
            None => (!is_per_turn_provider(&provider)).then(|| uuid::Uuid::new_v4().to_string()),
        };
        let view = match provider_session_id {
            Some(_) => view,
            None => view_to_str(&AgentView::Custom).to_string(),
        };

        let now = now_millis();
        // Superseded first: `idx_sessions_current` allows one current session.
        tx.execute(
            "UPDATE sessions SET superseded_at = ?2 WHERE id = ?1",
            rusqlite::params![old, now],
        )?;
        tx.execute(
            "UPDATE session_user_turns SET ended_at = ?2
             WHERE session_id = ?1 AND started_at IS NOT NULL AND ended_at IS NULL",
            rusqlite::params![old, now],
        )?;
        tx.execute("DELETE FROM pending_messages WHERE session_id = ?1", [&old])?;
        let id = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO sessions (id, workspace_id, provider, view, provider_session_id,
                                   effort, model, instructions, custom_agent_id, skills, mcp_servers,
                                   parent_session_id, parent_cut_seq, transcript_prefix, created_at)
             SELECT ?2, workspace_id, provider, ?3, ?4,
                    effort, model, instructions, custom_agent_id, skills, mcp_servers,
                    ?5, ?6, ?7, ?8
               FROM sessions WHERE id = ?1",
            rusqlite::params![
                old,
                id,
                view,
                provider_session_id,
                lineage.parent_session_id,
                lineage.cut_seq,
                native.map_or(0, |n| n.prefix as i64),
                now,
            ],
        )?;
        tx.commit()?;
        Ok(id)
    }

    /// Give the current session the transcript Fletch wrote for it before its
    /// first launch (`Supervisor::materialize`): the provider session it was
    /// written as, and how many leading records ingestion drops.
    pub fn set_native_transcript(
        &self,
        workspace_id: &str,
        native: &NativeTranscript,
    ) -> Result<()> {
        let conn = self.db.lock();
        let sid = current_session_id(&conn, workspace_id)
            .ok_or_else(|| Error::AgentNotFound(workspace_id.to_string()))?;
        conn.execute(
            "UPDATE sessions SET provider_session_id = ?2, transcript_prefix = ?3 WHERE id = ?1",
            rusqlite::params![sid, native.provider_session_id, native.prefix as i64],
        )?;
        Ok(())
    }

    /// How many leading records of the current session's own transcript are
    /// the history it continues, written there for its CLI to resume (see
    /// [`NativeTranscript`]). 0 for a session that started its own.
    pub fn session_transcript_prefix(&self, workspace_id: &str) -> Result<usize> {
        let conn = self.db.lock();
        let Some(sid) = current_session_id(&conn, workspace_id) else {
            return Ok(0);
        };
        let prefix: i64 = conn.query_row(
            "SELECT transcript_prefix FROM sessions WHERE id = ?1",
            [&sid],
            |r| r.get(0),
        )?;
        Ok(prefix.max(0) as usize)
    }

    /// What session `session_id`'s agent was told of the conversation it
    /// continued (`set_handoff_context`). A session continuing it natively is
    /// told the same, so it knows what that agent knew.
    pub fn session_handoff_context(&self, session_id: &str) -> Result<Option<String>> {
        let conn = self.db.lock();
        conn.query_row(
            "SELECT handoff_context FROM sessions WHERE id = ?1",
            [session_id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| Error::Other(format!("unknown session {session_id}")))
    }

    /// The sessions `workspace_id` has superseded — the conversations it
    /// rewound away from — oldest first, each with its records. A rewind
    /// leaves the abandoned branch's records where they are, and what it
    /// spent is still the workspace's, so its usage folds these in. Each
    /// record is its own session's (`inherited: false`).
    ///
    /// A superseded session never changes again (nothing ingests into it), so
    /// a caller folds each one once: every session is listed, but those in
    /// `known` come without their records.
    ///
    /// Sessions `detach_children` handed over from a deleted workspace are
    /// left out: what they spent was that workspace's. They are told apart by
    /// age. A workspace is only ever handed sessions its history inherits
    /// from, which all existed before it, while every session it starts is as
    /// old as it is or younger (`insert_agent` stamps the first one with the
    /// workspace's own `created_at`).
    pub fn read_superseded_records(
        &self,
        workspace_id: &str,
        known: &[String],
    ) -> Result<Vec<SupersededSession>> {
        let conn = self.db.lock();
        let mut stmt = conn.prepare(
            "SELECT s.id FROM sessions s JOIN workspaces w ON w.id = s.workspace_id
              WHERE s.workspace_id = ?1 AND s.superseded_at IS NOT NULL
                AND s.created_at >= w.created_at
              ORDER BY s.created_at, s.rowid",
        )?;
        let sessions: Vec<String> = stmt
            .query_map([workspace_id], |r| r.get(0))?
            .collect::<std::result::Result<_, rusqlite::Error>>()?;
        sessions
            .into_iter()
            .map(|session_id| {
                let records = if known.contains(&session_id) {
                    Vec::new()
                } else {
                    query_records(&conn, &session_id, i64::MAX, false)?
                };
                Ok(SupersededSession {
                    session_id,
                    records,
                })
            })
            .collect()
    }

    /// Store what the current session's agent is told about the conversation
    /// it continues (`crate::handoff`), composed into its instructions from
    /// the next launch on.
    pub fn set_handoff_context(&self, workspace_id: &str, context: &str) -> Result<()> {
        let conn = self.db.lock();
        let sid = current_session_id(&conn, workspace_id)
            .ok_or_else(|| Error::AgentNotFound(workspace_id.to_string()))?;
        conn.execute(
            "UPDATE sessions SET handoff_context = ?2 WHERE id = ?1",
            rusqlite::params![sid, context],
        )?;
        Ok(())
    }

    /// Count of records already ingested for the current session (= MAX(seq))
    /// — past its transcript's prefix, the starting index for positional
    /// `ln:{i}` native ids on the next read. Own records only: history
    /// inherited through lineage is never re-read from this session's
    /// transcript.
    pub fn session_record_count(&self, workspace_id: &str) -> Result<usize> {
        let conn = self.db.lock();
        let Some(sid) = current_session_id(&conn, workspace_id) else {
            return Ok(0);
        };
        let count: i64 = conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM transcripts.session_records WHERE session_id = ?1",
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
        let sid = current_session_id(&conn, workspace_id)?;
        // MAX over an empty set is SQL NULL, so decode into an Option and let a
        // session with no records yet report `None` rather than 0.
        conn.query_row(
            "SELECT MAX(created_at) FROM transcripts.session_records WHERE session_id = ?1",
            [&sid],
            |r| r.get::<_, Option<i64>>(0),
        )
        .ok()
        .flatten()
    }

    /// Bodies of the current session's records whose JSON text contains
    /// `needle`, in seq order. A cheap prefilter for a lookup keyed on a field
    /// deep inside the body (e.g. the sub-agent id a Claude tool result names):
    /// SQL scans the stored text and only the hits are deserialized, instead of
    /// every record in the conversation. Plain substring match, no wildcards.
    pub fn session_record_bodies_containing(
        &self,
        workspace_id: &str,
        needle: &str,
    ) -> Result<Vec<serde_json::Value>> {
        let conn = self.db.lock();
        let Some(sid) = current_session_id(&conn, workspace_id) else {
            return Ok(vec![]);
        };
        let mut stmt = conn.prepare(
            "SELECT body FROM transcripts.session_records
             WHERE session_id = ?1 AND instr(body, ?2) > 0
             ORDER BY seq ASC",
        )?;
        let bodies: Vec<String> = stmt
            .query_map(rusqlite::params![sid, needle], |r| r.get(0))?
            .collect::<std::result::Result<_, rusqlite::Error>>()?;
        bodies
            .iter()
            .map(|text| {
                serde_json::from_str(text)
                    .map_err(|e| Error::Other(format!("deserialize record body: {e}")))
            })
            .collect()
    }

    /// The current session's own records, in seq order — what this session's
    /// agent actually produced, for internal readers (the workflow budget
    /// ledger). Display reads use the stitched `read_history_records`.
    pub fn read_session_records(&self, workspace_id: &str) -> Result<Vec<SessionRecord>> {
        let conn = self.db.lock();
        let Some(sid) = current_session_id(&conn, workspace_id) else {
            return Ok(vec![]);
        };
        query_records(&conn, &sid, i64::MAX, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message_queue::PendingMsg;
    use crate::workspace::tests::{
        agent, concat, exchange, history, history_turns, inherited, owned, seed_repo, session_of,
        test_db,
    };
    use crate::workspace::Anchor;
    use serde_json::json;

    /// Workspace `a`, which said alpha, then bravo.
    fn talked() -> WorkspaceManager {
        let db = test_db();
        seed_repo(&db, "/r");
        let wm = WorkspaceManager::new(db);
        agent(&wm, "a", "/r", None);
        exchange(&wm, "a", "a1", "alpha");
        exchange(&wm, "a", "a2", "bravo");
        wm
    }

    /// `a` rewound to just before bravo, in a new session.
    fn rewound(wm: &WorkspaceManager) -> String {
        let lineage = wm.resolve_anchor("a", Anchor::Before("a2")).unwrap();
        wm.start_session("a", &lineage, None).unwrap()
    }

    /// A transcript written for a new session, opening with `prefix` records.
    fn native(prefix: usize) -> NativeTranscript {
        NativeTranscript {
            provider_session_id: "written-session".into(),
            prefix,
        }
    }

    fn queued(turn_id: &str) -> PendingMsg {
        PendingMsg {
            turn_id: turn_id.into(),
            text: "later".into(),
            attachments: vec![],
        }
    }

    /// A turn of `a` in flight, with a follow-up queued behind it.
    fn busy(wm: &WorkspaceManager) {
        wm.insert_user_turn("a", "a3", "charlie", &[]).unwrap();
        wm.mark_user_turn_started("a3", 1).unwrap();
        wm.enqueue_pending_message("a", &queued("q1")).unwrap();
    }

    fn ended_at(wm: &WorkspaceManager, turn_id: &str) -> Option<i64> {
        wm.db
            .lock()
            .query_row(
                "SELECT ended_at FROM session_user_turns WHERE turn_id = ?1",
                [turn_id],
                |r| r.get(0),
            )
            .unwrap()
    }

    fn superseded_at(wm: &WorkspaceManager, session: &str) -> Option<i64> {
        wm.db
            .lock()
            .query_row(
                "SELECT superseded_at FROM sessions WHERE id = ?1",
                [session],
                |r| r.get(0),
            )
            .unwrap()
    }

    /// The session columns a new session takes over, and the ones it doesn't.
    fn settings(
        wm: &WorkspaceManager,
        session: &str,
    ) -> (Vec<Option<String>>, Vec<Option<String>>) {
        wm.db
            .lock()
            .query_row(
                "SELECT provider, view, effort, model, instructions, custom_agent_id, skills,
                        mcp_servers, handoff_context, last_error
                   FROM sessions WHERE id = ?1",
                [session],
                |r| {
                    let taken = (0..8).map(|i| r.get(i)).collect::<rusqlite::Result<_>>()?;
                    let left = (8..10).map(|i| r.get(i)).collect::<rusqlite::Result<_>>()?;
                    Ok((taken, left))
                },
            )
            .unwrap()
    }

    #[test]
    fn a_new_session_takes_the_old_ones_place_in_every_read_and_write() {
        let wm = talked();
        busy(&wm);
        let old = session_of(&wm, "a");
        wm.db
            .lock()
            .execute(
                "UPDATE sessions SET view = 'native', effort = 'high', model = 'opus',
                        instructions = 'be brief', custom_agent_id = 'ca', skills = '[1]',
                        mcp_servers = '[2]', handoff_context = 'digest', last_error = 'boom'
                  WHERE id = ?1",
                [&old],
            )
            .unwrap();
        let before = wm.agent("a").unwrap();
        let lineage = wm.resolve_anchor("a", Anchor::Before("a2")).unwrap();

        let new = wm.start_session("a", &lineage, None).unwrap();

        // One current session, the new one; the old one stays as an ancestor.
        assert_eq!(session_of(&wm, "a"), new);
        assert!(superseded_at(&wm, &old).is_some());
        let current: i64 = wm
            .db
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE workspace_id = 'a' AND superseded_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(current, 1);
        // Settings carry over; context, error and the provider session don't.
        let (taken, left) = settings(&wm, &new);
        assert_eq!(taken, settings(&wm, &old).0);
        assert_eq!(left, vec![None, None]);
        let after = wm.agent("a").unwrap();
        assert_eq!(after.lineage, Some(lineage));
        assert!(after.session_id.is_some());
        assert_ne!(after.session_id, before.session_id);
        assert_eq!(wm.session_transcript_prefix("a").unwrap(), 0);
        // The old session's turn is closed and its queue is gone.
        assert!(ended_at(&wm, "a3").is_some());
        assert!(wm.read_all_pending_messages().unwrap().is_empty());
        // What it shows: the history before bravo, nothing of its own yet.
        assert_eq!(history(&wm, "a"), inherited(&["a1-u", "a1-a"]));
        assert_eq!(history_turns(&wm, "a"), inherited(&["a1"]));
        assert_eq!(wm.session_record_count("a").unwrap(), 0);
        assert_eq!(wm.last_activity("a"), None);

        // Every write goes to the new session.
        exchange(&wm, "a", "n1", "delta");
        wm.update_agent_effort("a", Some("low")).unwrap();
        wm.enqueue_pending_message("a", &queued("q2")).unwrap();
        assert_eq!(
            history(&wm, "a"),
            concat(&[inherited(&["a1-u", "a1-a"]), owned(&["n1-u", "n1-a"])])
        );
        assert_eq!(
            history_turns(&wm, "a"),
            concat(&[inherited(&["a1"]), owned(&["n1"])])
        );
        assert_eq!(wm.agent("a").unwrap().effort.as_deref(), Some("low"));
        assert_eq!(settings(&wm, &old).0[2].as_deref(), Some("high"));
        let pending = wm.read_all_pending_messages().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].1.turn_id, "q2");
        assert!(wm
            .mark_user_turn_ended("a", TurnOutcome::Completed)
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_session_that_cant_start_leaves_the_old_one_as_it_was() {
        let wm = talked();
        busy(&wm);
        let old = session_of(&wm, "a");
        let nowhere = SessionLineage {
            parent_session_id: "no-such-session".into(),
            cut_seq: 1,
        };

        assert!(wm.start_session("a", &nowhere, None).is_err());

        assert_eq!(session_of(&wm, "a"), old);
        assert_eq!(superseded_at(&wm, &old), None);
        assert_eq!(ended_at(&wm, "a3"), None);
        assert_eq!(wm.read_all_pending_messages().unwrap().len(), 1);
        assert!(wm.start_session("nonesuch", &nowhere, None).is_err());
    }

    fn codex_agent(wm: &WorkspaceManager, id: &str) {
        let mut codex = new_agent_record(
            id.into(),
            id.into(),
            "codex".into(),
            crate::workspace::tests::mk_repo("/r"),
            "task".into(),
            AgentView::Native,
        );
        wm.add_agent(&mut codex).unwrap();
    }

    /// The view carries over only where the new session can open in it.
    #[test]
    fn a_session_that_cant_open_natively_yet_starts_in_the_chat_view() {
        let wm = talked();
        let restart = |ws: &str, native: Option<&NativeTranscript>| {
            let lineage = wm.resolve_anchor(ws, Anchor::End).unwrap();
            wm.start_session(ws, &lineage, native).unwrap();
        };
        wm.update_agent_view("a", AgentView::Native).unwrap();
        restart("a", None);
        assert_eq!(wm.agent("a").unwrap().view, AgentView::Native);

        // A per-turn provider's session is captured from its first turn, and
        // only then can its native view resume it.
        codex_agent(&wm, "c");
        wm.set_agent_session_id("c", "thread-1").unwrap();
        restart("c", None);
        let c = wm.agent("c").unwrap();
        assert_eq!(c.view, AgentView::Custom);
        assert_eq!(c.session_id, None);
        // And the superseded session's id stays its own.
        assert!(!wm.set_agent_session_id("c", "thread-1").unwrap());
        assert!(wm.set_agent_session_id("c", "thread-2").unwrap());
    }

    /// A session whose transcript was written for it starts as that provider
    /// session, in whatever view it was in: it has a conversation to resume.
    #[test]
    fn a_session_with_a_written_transcript_starts_as_it() {
        let wm = talked();
        codex_agent(&wm, "c");
        let lineage = wm.resolve_anchor("a", Anchor::End).unwrap();

        wm.start_session("c", &lineage, Some(&native(4))).unwrap();

        let c = wm.agent("c").unwrap();
        assert_eq!(c.session_id.as_deref(), Some("written-session"));
        assert_eq!(c.view, AgentView::Native);
        assert_eq!(wm.session_transcript_prefix("c").unwrap(), 4);
        assert_eq!(wm.session_record_count("c").unwrap(), 0);
    }

    /// A session created first (a fork's) is given its transcript once it is
    /// written; the session it continues says what its agent was told.
    #[test]
    fn a_transcript_written_after_the_session_is_recorded_on_it() {
        let wm = talked();
        let lineage = wm.resolve_anchor("a", Anchor::End).unwrap();
        wm.set_handoff_context("a", "what a was told").unwrap();
        codex_agent(&wm, "c");

        wm.set_native_transcript("c", &native(3)).unwrap();

        assert_eq!(
            wm.agent("c").unwrap().session_id.as_deref(),
            Some("written-session")
        );
        assert_eq!(wm.session_transcript_prefix("c").unwrap(), 3);
        assert_eq!(wm.session_transcript_prefix("a").unwrap(), 0);
        assert_eq!(
            wm.session_handoff_context(&lineage.parent_session_id)
                .unwrap()
                .as_deref(),
            Some("what a was told")
        );
        assert!(wm.session_handoff_context("nonesuch").is_err());
    }

    /// Nothing about a record's id makes ingestion skip it: a session stores
    /// what its transcript holds past its prefix, whatever its history shows.
    #[test]
    fn a_new_session_stores_what_its_transcript_holds() {
        let wm = talked();
        rewound(&wm);
        let alpha = json!({"type": "user", "text": "alpha"});
        wm.append_session_records("a", "claude", "transcript", None, &[("a1-u", &alpha)])
            .unwrap();
        assert_eq!(
            history(&wm, "a"),
            concat(&[inherited(&["a1-u", "a1-a"]), owned(&["a1-u"])])
        );
    }

    /// `ws`'s superseded sessions, given the `known` ones, as `(session id,
    /// native ids)`.
    fn superseded(wm: &WorkspaceManager, ws: &str, known: &[String]) -> Vec<(String, Vec<String>)> {
        wm.read_superseded_records(ws, known)
            .unwrap()
            .into_iter()
            .map(|s| {
                assert!(s.records.iter().all(|r| !r.inherited));
                let ids = s.records.into_iter().map(|r| r.native_id).collect();
                (s.session_id, ids)
            })
            .collect()
    }

    fn native_ids(sessions: Vec<(String, Vec<String>)>) -> Vec<Vec<String>> {
        sessions.into_iter().map(|(_, ids)| ids).collect()
    }

    /// `a` rewound twice: before bravo, then before delta.
    fn rewound_twice() -> (WorkspaceManager, [String; 2]) {
        let wm = talked();
        let first = session_of(&wm, "a");
        rewound(&wm);
        let second = session_of(&wm, "a");
        exchange(&wm, "a", "n1", "delta");
        let lineage = wm.resolve_anchor("a", Anchor::Before("n1")).unwrap();
        wm.start_session("a", &lineage, None).unwrap();
        exchange(&wm, "a", "m1", "echo");
        (wm, [first, second])
    }

    #[test]
    fn superseded_sessions_keep_their_records_for_the_workspace() {
        assert!(superseded(&talked(), "a", &[]).is_empty());
        let (wm, ids) = rewound_twice();

        let sessions = superseded(&wm, "a", &[]);

        assert_eq!(
            sessions
                .iter()
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>(),
            ids
        );
        assert_eq!(
            native_ids(sessions),
            vec![vec!["a1-u", "a1-a", "a2-u", "a2-a"], vec!["n1-u", "n1-a"]]
        );
    }

    /// A caller folds a superseded session once: those it names come back
    /// listed, so it still learns the whole set, but without their records.
    #[test]
    fn known_superseded_sessions_come_back_without_their_records() {
        let (wm, [first, second]) = rewound_twice();

        assert_eq!(
            superseded(&wm, "a", std::slice::from_ref(&first)),
            vec![
                (first.clone(), vec![]),
                (second.clone(), vec!["n1-u".into(), "n1-a".into()])
            ]
        );
        let both = [first.clone(), second.clone(), "elsewhere".into()];
        assert_eq!(
            superseded(&wm, "a", &both),
            vec![(first, vec![]), (second, vec![])]
        );
    }

    /// A deleted workspace's session handed to its fork stays out of the
    /// fork's own: what it spent was the deleted workspace's.
    #[test]
    fn a_session_handed_over_on_delete_is_not_the_heirs() {
        let db = test_db();
        seed_repo(&db, "/r");
        let wm = WorkspaceManager::new(db);
        // Created some seconds ago, as a fork always is after its parent.
        let add = |id: &str, secs_ago: i64, lineage: Option<SessionLineage>| {
            let mut rec = new_agent_record(
                id.into(),
                id.into(),
                "claude".into(),
                crate::workspace::tests::mk_repo("/r"),
                "task".into(),
                AgentView::Custom,
            );
            rec.created_at = (Utc::now() - chrono::Duration::seconds(secs_ago)).to_rfc3339();
            rec.lineage = lineage;
            wm.add_agent(&mut rec).unwrap();
        };
        add("p", 20, None);
        exchange(&wm, "p", "p1", "alpha");
        add("f", 10, Some(wm.resolve_anchor("p", Anchor::End).unwrap()));
        exchange(&wm, "f", "f1", "bravo");

        wm.remove_agent("p").unwrap();
        assert!(superseded(&wm, "f", &[]).is_empty());

        let lineage = wm.resolve_anchor("f", Anchor::Before("f1")).unwrap();
        wm.start_session("f", &lineage, None).unwrap();
        assert_eq!(
            native_ids(superseded(&wm, "f", &[])),
            vec![vec!["f1-u", "f1-a"]]
        );
    }
}
