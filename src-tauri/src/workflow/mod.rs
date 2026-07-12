//! Workflows v1 backend (TECH_SPEC §3.1).
//!
//! This slice lands the persistence substrate: the domain types (`types`), the
//! append-only journal (`journal`), and the read-only command surface below.
//! The scheduler, gates, budgets, comms and git transport arrive in later
//! slices; until one of them populates the tables, the read commands simply
//! return empty results.
//!
//! `dead_code` is allowed module-wide: this slice deliberately publishes the
//! write API (`journal::append`, the `wf:event`/`wf:run` emitters, the §7.1
//! event-type names) that the scheduler slice consumes but nothing calls yet.
//! The allow is removed once those callers land.
#![allow(dead_code)]

pub mod journal;
pub mod types;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension};

use crate::workflow::types::{Event, Message, Run, RunDetail, StepExec};

type Db = Arc<Mutex<Connection>>;

/// Epoch milliseconds, matching the core schema's timestamp convention.
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ───────────────────────────── read commands (§7.2, §13) ────────────────────

/// Every run, newest-updated first; optionally scoped to one project. Drives the
/// sidebar's run rows (§14.2).
#[tauri::command]
pub async fn wf_list_runs(
    project_id: Option<String>,
    db: tauri::State<'_, Db>,
) -> Result<Vec<Run>, String> {
    let conn = db.lock();
    let map_err = |e: rusqlite::Error| e.to_string();
    match project_id {
        Some(pid) => {
            let mut stmt = conn
                .prepare("SELECT * FROM wf_run WHERE project_id = ?1 ORDER BY updated_at DESC")
                .map_err(map_err)?;
            let rows = stmt.query_map([pid], Run::from_row).map_err(map_err)?;
            rows.collect::<rusqlite::Result<_>>().map_err(map_err)
        }
        None => {
            let mut stmt = conn
                .prepare("SELECT * FROM wf_run ORDER BY updated_at DESC")
                .map_err(map_err)?;
            let rows = stmt.query_map([], Run::from_row).map_err(map_err)?;
            rows.collect::<rusqlite::Result<_>>().map_err(map_err)
        }
    }
}

/// A run plus its attempts and messages (§7.2). `None` if the run doesn't exist.
#[tauri::command]
pub async fn wf_get_run(
    run_id: String,
    db: tauri::State<'_, Db>,
) -> Result<Option<RunDetail>, String> {
    let conn = db.lock();
    let map_err = |e: rusqlite::Error| e.to_string();

    let run = conn
        .query_row(
            "SELECT * FROM wf_run WHERE id = ?1",
            [&run_id],
            Run::from_row,
        )
        .optional()
        .map_err(map_err)?;
    let Some(run) = run else {
        return Ok(None);
    };

    let attempts = {
        let mut stmt = conn
            .prepare("SELECT * FROM wf_step_exec WHERE run_id = ?1 ORDER BY rowid")
            .map_err(map_err)?;
        let rows = stmt
            .query_map([&run_id], StepExec::from_row)
            .map_err(map_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_err)?
    };

    let messages = {
        let mut stmt = conn
            .prepare("SELECT * FROM wf_message WHERE run_id = ?1 ORDER BY created_at, rowid")
            .map_err(map_err)?;
        let rows = stmt
            .query_map([&run_id], Message::from_row)
            .map_err(map_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_err)?
    };

    Ok(Some(RunDetail {
        run,
        attempts,
        messages,
    }))
}

/// A page of a run's journal (§7.2): events strictly after `after_seq`, oldest
/// first, capped at `limit`.
#[tauri::command]
pub async fn wf_events(
    run_id: String,
    after_seq: i64,
    limit: i64,
    db: tauri::State<'_, Db>,
) -> Result<Vec<Event>, String> {
    let conn = db.lock();
    journal::read_events(&conn, &run_id, after_seq, limit).map_err(|e| e.to_string())
}
