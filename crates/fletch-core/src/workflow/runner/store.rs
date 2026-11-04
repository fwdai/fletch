//! The kernel's reads and writes of its run row and `wf_step_exec` rows. The
//! status-and-journal pair every outcome goes through lives in `journal`.

use std::sync::Arc;

use rusqlite::Connection;
use serde_json::json;

use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::workflow::journal::{emit_run_row, journal_event};
use crate::workflow::now_ms;
use crate::workflow::spec::Gate;
use crate::workflow::types::event_type;

/// The `wf_run` columns the kernel reads.
pub(crate) struct RunEssentials {
    pub(crate) spec_json: String,
    pub(crate) task: String,
    pub(crate) repo_path: String,
    pub(crate) run_dir: String,
    pub(crate) branch: String,
    pub(crate) base_sha: String,
    /// Caller-selected launch base branch, or empty when none was given — the
    /// default PR base at finalization unless the spec pins `finalize.pr_base`.
    pub(crate) base_branch: String,
    pub(crate) status: String,
    /// The issue this run was started from (`"123"` for GitHub, `"ENG-123"` for
    /// Linear), or `None`. Drives the finalized PR's closing trailer.
    pub(crate) issue_ref: Option<String>,
}

pub(crate) fn load_run(conn: &Connection, run_id: &str) -> Result<RunEssentials> {
    conn.query_row(
        "SELECT spec_json, task, repo_path, run_dir, branch, base_sha, base_branch, status,
                issue_ref
         FROM wf_run WHERE id = ?1",
        [run_id],
        |r| {
            Ok(RunEssentials {
                spec_json: r.get(0)?,
                task: r.get(1)?,
                repo_path: r.get(2)?,
                run_dir: r.get(3)?,
                branch: r.get(4)?,
                base_sha: r.get(5)?,
                base_branch: r.get(6)?,
                status: r.get(7)?,
                issue_ref: r.get(8)?,
            })
        },
    )
    .map_err(|e| Error::Other(format!("run {run_id} not found: {e}")))
}

/// Record the PR a finalize opened on the run row and emit `wf:run` (0029).
///
/// Called only when a PR actually exists — a `push`-only finalize or a failed
/// `pr create` leaves both columns NULL, which every reader treats as "no PR to
/// poll". Written before the run flips `done` so nothing can observe a finished
/// roadmap-dispatched run without its PR: the drainer settles such a run into
/// `in_review` off exactly these columns, and the merge sweep polls from there.
///
/// `updated_at` is deliberately *not* bumped: this records something that was
/// already true the moment GitHub answered, and the run's own status write
/// (immediately after) carries the timestamp the sidebar orders on.
pub(crate) fn set_pr(
    conn: &Connection,
    engine: Option<&Arc<EngineCtx>>,
    run_id: &str,
    number: Option<i64>,
    url: &str,
) {
    let _ = conn.execute(
        "UPDATE wf_run SET pr_number = ?1, pr_url = ?2 WHERE id = ?3",
        rusqlite::params![number, url, run_id],
    );
    emit_run_row(conn, engine, run_id);
}

/// The `wf_step_exec.gate_mode` spelling of a gate, also journaled on
/// `gate_evaluated`.
pub(crate) fn gate_mode(gate: &Gate) -> &'static str {
    match gate {
        Gate::Verdict => "verdict",
        Gate::Commit => "commit",
    }
}

pub(crate) fn create_step_exec(
    conn: &Connection,
    id: &str,
    run_id: &str,
    step_id: &str,
    attempt: i64,
    iteration: i64,
    gate_mode: &str,
) {
    let _ = conn.execute(
        "INSERT INTO wf_step_exec (id, run_id, step_id, attempt, iteration, status, gate_mode)
         VALUES (?1, ?2, ?3, ?4, ?5, 'spawning', ?6)",
        rusqlite::params![id, run_id, step_id, attempt, iteration, gate_mode],
    );
}

/// Link the live agent to its exec row and mark the row `running`. This is what
/// lets the monitor mount the step's chat (it resolves the agent through
/// `wf_step_exec.agent_id`) while the turn is still in flight.
pub(crate) fn stamp_spawned(conn: &Connection, exec_id: &str, agent_id: &str) {
    let _ = conn.execute(
        "UPDATE wf_step_exec SET agent_id = ?1, status = 'running', started_at = ?2
         WHERE id = ?3",
        rusqlite::params![agent_id, now_ms(), exec_id],
    );
}

pub(crate) fn finish_step_exec(conn: &Connection, id: &str, status: &str, head_end: Option<&str>) {
    let _ = conn.execute(
        "UPDATE wf_step_exec SET status = ?1, head_end = ?2, ended_at = ?3 WHERE id = ?4",
        rusqlite::params![status, head_end, now_ms(), id],
    );
}

/// Mark a step exec `abandoned` (ended now) and journal the reason as an
/// `ATTEMPT_ABANDONED` event (§8.3). The caller holds `conn`.
pub(crate) fn abandon_exec(
    conn: &Connection,
    engine: Option<&Arc<EngineCtx>>,
    run_id: &str,
    exec_id: &str,
    cause: &str,
) {
    let _ = conn.execute(
        "UPDATE wf_step_exec SET status = 'abandoned', ended_at = ?1 WHERE id = ?2",
        rusqlite::params![now_ms(), exec_id],
    );
    journal_event(
        conn,
        engine,
        run_id,
        event_type::ATTEMPT_ABANDONED,
        Some(exec_id),
        &json!({ "cause": cause }),
    );
}

/// Live (spawned, non-terminal) step agents for a run — stopped on cancel.
pub(crate) fn live_step_agents(conn: &Connection, run_id: &str) -> Vec<String> {
    conn.prepare(
        "SELECT agent_id FROM wf_step_exec
         WHERE run_id = ?1 AND agent_id IS NOT NULL
           AND status IN ('spawning','running','gating')",
    )
    .and_then(|mut s| {
        s.query_map([run_id], |r| r.get::<_, Option<String>>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()
    })
    .map(|v| v.into_iter().flatten().collect())
    .unwrap_or_default()
}
