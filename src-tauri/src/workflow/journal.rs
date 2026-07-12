//! The workflow journal (TECH_SPEC §7): an append-only `wf_event` log per run.
//!
//! `append` allocates the next per-run `seq` and inserts within the caller's DB
//! lock — every workflow writer serializes through the single connection mutex
//! (`Arc<Mutex<Connection>>`), so the `MAX(seq)+1` read and the INSERT are one
//! atomic step and `seq` stays monotonic per run even under concurrent
//! appenders. Status-row changes are written in the same transaction as the
//! event that caused them (that wiring lands with the scheduler); this module
//! owns the event write and the frontend notification (§7.2).

use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::workflow::now_ms;
use crate::workflow::types::{Event, Run};

/// Append an event to `run_id`'s journal and return the persisted row.
///
/// Call while holding the connection lock (and, when mutating status rows, the
/// enclosing transaction) so the event and its caused state change commit
/// together.
pub fn append(
    conn: &Connection,
    run_id: &str,
    event_type: &str,
    step_exec_id: Option<&str>,
    payload: &Value,
) -> rusqlite::Result<Event> {
    let seq: i64 = conn.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM wf_event WHERE run_id = ?1",
        [run_id],
        |r| r.get(0),
    )?;
    let ts = now_ms();
    let payload_str = serde_json::to_string(payload)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    conn.execute(
        "INSERT INTO wf_event (run_id, seq, ts, step_exec_id, type, payload_json) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![run_id, seq, ts, step_exec_id, event_type, payload_str],
    )?;
    Ok(Event {
        run_id: run_id.to_string(),
        seq,
        ts,
        step_exec_id: step_exec_id.map(str::to_string),
        event_type: event_type.to_string(),
        payload: payload.clone(),
    })
}

/// A page of journal events strictly after `after_seq`, oldest first. `limit`
/// bounds the page; the caller pages by passing the last returned `seq`.
pub fn read_events(
    conn: &Connection,
    run_id: &str,
    after_seq: i64,
    limit: i64,
) -> rusqlite::Result<Vec<Event>> {
    let mut stmt = conn.prepare(
        "SELECT run_id, seq, ts, step_exec_id, type, payload_json \
         FROM wf_event WHERE run_id = ?1 AND seq > ?2 ORDER BY seq ASC LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![run_id, after_seq, limit], Event::from_row)?;
    rows.collect()
}

/// The `wf:event` envelope (§7.2). Only the addressing fields ride the event;
/// the payload is fetched on demand via `wf_events`, so nothing large or
/// transcript-derived is duplicated onto the tauri channel.
#[derive(Serialize, Clone)]
struct EventEnvelope<'a> {
    run_id: &'a str,
    seq: i64,
    #[serde(rename = "type")]
    event_type: &'a str,
    ts: i64,
    step_exec_id: Option<&'a str>,
}

/// Notify the frontend that an event was appended (§7.2). Best-effort: a failed
/// emit (no renderer listening) never affects the persisted journal.
pub fn emit_event(app: &AppHandle, ev: &Event) {
    let _ = app.emit(
        "wf:event",
        EventEnvelope {
            run_id: &ev.run_id,
            seq: ev.seq,
            event_type: &ev.event_type,
            ts: ev.ts,
            step_exec_id: ev.step_exec_id.as_deref(),
        },
    );
}

/// Notify the frontend that a run row changed (§7.2): emits the full row so the
/// sidebar and monitor update without a round-trip.
pub fn emit_run(app: &AppHandle, run: &Run) {
    let _ = app.emit("wf:run", run);
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use serde_json::json;
    use std::sync::Arc;

    /// A bare connection with just the `wf_event` table (mirrors the columns in
    /// 0019); enough to exercise seq allocation and paging in isolation.
    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE wf_event (
               run_id       TEXT NOT NULL,
               seq          INTEGER NOT NULL,
               ts           INTEGER NOT NULL,
               step_exec_id TEXT,
               type         TEXT NOT NULL,
               payload_json TEXT NOT NULL,
               PRIMARY KEY (run_id, seq)
             );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn seq_is_per_run_monotonic_from_one() {
        let conn = test_conn();
        for expected in 1..=3 {
            let ev = append(&conn, "run-a", event_type_ref(), None, &json!({"n": expected})).unwrap();
            assert_eq!(ev.seq, expected);
        }
        // A second run has its own independent sequence.
        let ev = append(&conn, "run-b", event_type_ref(), None, &json!({})).unwrap();
        assert_eq!(ev.seq, 1);
        let ev = append(&conn, "run-a", event_type_ref(), None, &json!({})).unwrap();
        assert_eq!(ev.seq, 4);
    }

    #[test]
    fn read_events_pages_after_seq() {
        let conn = test_conn();
        for i in 0..5 {
            append(&conn, "run", "t", Some("exec-1"), &json!({ "i": i })).unwrap();
        }
        // First page.
        let page = read_events(&conn, "run", 0, 2).unwrap();
        assert_eq!(page.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![1, 2]);
        // Next page continues strictly after the last seq seen.
        let last = page.last().unwrap().seq;
        let page = read_events(&conn, "run", last, 2).unwrap();
        assert_eq!(page.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![3, 4]);
        // Tail page + round-trip of the addressing fields and payload.
        let page = read_events(&conn, "run", 4, 100).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].seq, 5);
        assert_eq!(page[0].step_exec_id.as_deref(), Some("exec-1"));
        assert_eq!(page[0].payload, json!({ "i": 4 }));
        assert_eq!(page[0].event_type, "t");
    }

    #[test]
    fn concurrent_appenders_produce_a_gapless_sequence() {
        // Mirrors the app's discipline: every writer goes through the shared
        // connection mutex, so seq allocation stays atomic across threads.
        let db = Arc::new(Mutex::new(test_conn()));
        const THREADS: i64 = 8;
        const PER_THREAD: i64 = 25;
        let handles: Vec<_> = (0..THREADS)
            .map(|t| {
                let db = Arc::clone(&db);
                std::thread::spawn(move || {
                    for i in 0..PER_THREAD {
                        let conn = db.lock();
                        append(&conn, "run", "t", None, &json!({ "t": t, "i": i })).unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        let conn = db.lock();
        let all = read_events(&conn, "run", 0, THREADS * PER_THREAD + 1).unwrap();
        let seqs: Vec<i64> = all.iter().map(|e| e.seq).collect();
        let expected: Vec<i64> = (1..=THREADS * PER_THREAD).collect();
        assert_eq!(seqs, expected, "seqs must be gapless and unique");
    }

    fn event_type_ref() -> &'static str {
        crate::workflow::types::event_type::RUN_LAUNCHED
    }
}
