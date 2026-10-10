//! When the extractor runs. The watermark comes from the workspace's user-turn
//! observations in `context.observations`: the latest one's `created_at`
//! debounces, whatever came of that run, and the latest *extracted* one's
//! source reference — the last turn id handed to a run that landed — says
//! which turns are new, so a run that timed out or answered garbage leaves
//! its turns for the next. No table of its own; the observation log is
//! already the record of what was looked at.

use rusqlite::OptionalExtension;

use crate::context::{ContextStore, Result};

/// The least time between two runs for one workspace. A turn-end that comes
/// sooner waits for a later one (or the archive) to carry its turns. Long, on
/// purpose: the archive run, which sees the completed conversation, is the
/// primary one; turn-ends only keep a long-lived workspace covered.
pub const DEBOUNCE_MS: i64 = 4 * 60 * 60 * 1000;

/// What the runs so far for a workspace covered, and when the last one was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watermark {
    /// When the last run started, whether or not it succeeded.
    pub last_run_at: i64,
    /// The last turn given to the last run that succeeded; `None` when no run
    /// has yet, or that run named no turn.
    pub turn_id: Option<String>,
}

/// Whether a run may start now, before anything is read: an archive always
/// may; a turn-end waits out the debounce since the last run. Whether there
/// is anything new to run over is the caller's check, once it has the turns.
pub fn due(last_run_at: Option<i64>, now: i64, is_archive: bool) -> bool {
    // `map_or`, not `is_none_or`: the crate's rust-version is 1.77.
    is_archive || last_run_at.map_or(true, |at| now - at >= DEBOUNCE_MS)
}

/// The workspace's watermark, or `None` when no run has been recorded for it.
pub fn watermark(
    store: &ContextStore,
    project_id: &str,
    workspace_id: &str,
) -> Result<Option<Watermark>> {
    let conn = store.db().lock();
    // The kind the pipeline stamps its observations with, as the JSON tag.
    let kind = serde_json::to_value(super::OBSERVATION_SOURCE)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    let latest = |extracted_only: bool| {
        conn.query_row(
            &format!(
                "SELECT created_at, json_extract(source, '$.reference')
                 FROM context.observations
                 WHERE project_id = ?1
                   AND json_extract(source, '$.kind') = ?3
                   AND json_extract(provenance, '$.workspace_id') = ?2
                   {}
                 ORDER BY created_at DESC, id DESC
                 LIMIT 1",
                if extracted_only {
                    "AND extracted_at IS NOT NULL"
                } else {
                    ""
                }
            ),
            [project_id, workspace_id, kind.as_str()],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)),
        )
        .optional()
    };
    let Some((last_run_at, _)) = latest(false)? else {
        return Ok(None);
    };
    let turn_id = latest(true)?.and_then(|(_, turn_id)| turn_id);
    Ok(Some(Watermark {
        last_run_at,
        turn_id,
    }))
}
