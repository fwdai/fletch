//! Turn-end transcript ingestion into `session_records`, plus PR-state
//! fetch/emit for an agent's primary repo.

use std::time::Duration;
use tauri::AppHandle;

use crate::agent::per_turn_descriptor;
use crate::workspace::{repo_checkout_path, AgentRecord, AgentView, WorkspaceManager};

use super::events::{emit_pr_state, emit_session_records_appended};
use super::Supervisor;

impl Supervisor {
    /// Synchronously ingest the agent's transcript into session_records (used
    /// for lazy backfill when a session is opened with no records yet). `None`
    /// if the provider has no transcript reader.
    pub fn sync_session(&self, agent_id: &str) -> Option<usize> {
        sync_session_records(&self.workspace, agent_id)
    }

    /// Fire-and-forget transcript ingest at turn-end. Called from
    /// `transition_active` whenever any agent reaches Idle. Emits
    /// `session:records-appended` when new records land; WARNs once if a
    /// reader-backed agent ingests nothing.
    ///
    /// The polling shape depends on whether the agent's process persists across
    /// turns (see `SyncPoll`):
    /// - **Per-turn agents** (custom view) have *exited* by turn-end, so the
    ///   file is complete and quiescent — we just ride out any flush lag and stop
    ///   at the first non-empty read.
    /// - **Claude / native-view agents** keep the transcript file open, so the
    ///   final line can still be flushing. We poll until the file settles (two
    ///   consecutive reads add nothing) before trusting the turn is fully on disk.
    pub fn trigger_session_sync(&self, app: AppHandle, agent_id: String) {
        let workspace = self.workspace.clone();
        let persistent = workspace
            .agent(&agent_id)
            .map(|r| is_persistent_runner(&r))
            .unwrap_or(true);
        tauri::async_runtime::spawn(async move {
            // Immediate attempt, then fine-grained backoff (ms) to ride out flush
            // lag / detect settle. Reads are incremental (O(new)), so polling is
            // cheap even on long transcripts.
            let backoffs = [0u64, 150, 150, 150, 200, 300, 400, 600];
            let mut poll = SyncPoll::new(persistent);
            for wait in backoffs {
                if wait > 0 {
                    tokio::time::sleep(Duration::from_millis(wait)).await;
                }
                let result = sync_session_records(&workspace, &agent_id);
                if matches!(poll.observe(result), PollControl::Stop) {
                    break;
                }
            }
            if poll.should_emit() {
                emit_session_records_appended(&app, &agent_id);
            } else if poll.reader_ingested_nothing() {
                tracing::warn!(
                    agent_id,
                    "session sync ingested 0 records after retries (transcript not found or unchanged)"
                );
            }
        });
    }

    /// Fetch the current PR state for an agent's primary repo and emit
    /// a `pr:state_changed` event. Runs as a background task — never blocks the caller.
    pub fn fetch_and_emit_pr_state(&self, app: AppHandle, agent_id: String) {
        let workspace = self.workspace.clone();
        tauri::async_runtime::spawn(async move {
            let record = match workspace.agent(&agent_id) {
                Ok(r) => r,
                Err(_) => return,
            };
            let repo = match record.repos.first() {
                Some(r) => r,
                None => return,
            };
            // Only fetch if there's a branch (agent may still be on detached HEAD)
            if repo.branch.is_none() {
                return;
            }
            let subdir = repo.subdir.clone();
            let checkout = match crate::workspace::repo_checkout_path(&agent_id, &subdir) {
                Ok(p) => p,
                Err(_) => return,
            };
            let state = if let Some(number) = repo.pr_number {
                // Known PR: fetch by number, never by branch. This is what keeps
                // PR identity bound to the agent rather than the recyclable
                // branch name.
                crate::github::pr_view_number(&checkout, number as u32)
                    .await
                    .unwrap_or(None)
            } else {
                // No PR recorded yet. Discover one created out-of-band (agent ran
                // `gh pr create`, or it was opened on github.com), but only adopt
                // it if it's OPEN — a stale merged/closed PR sitting on a recycled
                // branch must not be claimed as this agent's. Once adopted we
                // persist the number so all later lookups go by number.
                match crate::github::pr_view(&checkout).await.unwrap_or(None) {
                    Some(pr) if matches!(pr.state, crate::github::PrStatus::Open) => {
                        if let Err(e) =
                            workspace.set_repo_pr_number(&agent_id, &subdir, pr.number as i64)
                        {
                            tracing::warn!(
                                error = %e,
                                agent_id = %agent_id,
                                pr = pr.number,
                                "pr discovery: failed to persist PR number"
                            );
                        }
                        Some(pr)
                    }
                    _ => None,
                }
            };
            // Accrue PR history: stamp GitHub's own open/merge times whenever
            // a fetch reports them (feeds per-day PR stats).
            if let Some(pr) = &state {
                if let Err(e) =
                    workspace.set_repo_pr_times(&agent_id, &subdir, pr.opened_at, pr.merged_at)
                {
                    tracing::warn!(error = %e, agent_id, "failed to stamp PR times");
                }
            }
            emit_pr_state(&app, &agent_id, state);
        });
    }
}

/// Does this agent keep its transcript file open across turns? Per-turn agents
/// in the custom view *exit* at each turn-end, so the file is complete and
/// quiescent the moment we sync. Everything else — claude, and any agent in the
/// native (PTY/TUI) view — holds the file open, so the final line may still be
/// flushing and we must poll until it settles.
fn is_persistent_runner(record: &AgentRecord) -> bool {
    let per_turn = per_turn_descriptor(&record.provider).is_some();
    !(per_turn && record.view == AgentView::Custom)
}

/// Whether the turn-end transcript poll should keep going.
#[derive(Debug, PartialEq, Eq)]
enum PollControl {
    Continue,
    Stop,
}

/// Decision logic for the turn-end transcript sync, split out from
/// `trigger_session_sync` so it's unit-testable without timers or the
/// filesystem. Fed each `sync_session_records` result (`None` = no reader,
/// `Some(n)` = n new records this pass).
///
/// The stop condition depends on whether the runner persists:
/// - **Non-persistent (per-turn, exited):** the file is complete, so stop at the
///   first non-empty read — earlier empty reads just ride out flush lag.
/// - **Persistent (claude / native):** the final line may still be flushing,
///   possibly after a gap, so keep polling until the file *settles* — two
///   consecutive reads that add nothing once we've started ingesting. A later
///   batch resets the counter, so a multi-phase flush (tool-result, then the
///   answer) is still captured this turn.
struct SyncPoll {
    persistent: bool,
    had_reader: bool,
    inserted: usize,
    stable_polls: u32,
}

impl SyncPoll {
    fn new(persistent: bool) -> Self {
        Self {
            persistent,
            had_reader: false,
            inserted: 0,
            stable_polls: 0,
        }
    }

    fn observe(&mut self, result: Option<usize>) -> PollControl {
        match result {
            None => PollControl::Stop, // no reader — nothing to wait for
            Some(0) => {
                self.had_reader = true;
                if self.inserted == 0 {
                    return PollControl::Continue; // not flushed yet — keep waiting
                }
                if !self.persistent {
                    return PollControl::Stop; // exited → first batch was the whole turn
                }
                self.stable_polls += 1;
                if self.stable_polls >= 2 {
                    PollControl::Stop // file quiet for two polls → settled
                } else {
                    PollControl::Continue
                }
            }
            Some(n) => {
                self.had_reader = true;
                self.inserted += n;
                self.stable_polls = 0; // new content → not settled
                if self.persistent {
                    PollControl::Continue
                } else {
                    PollControl::Stop // exited → the batch is complete
                }
            }
        }
    }

    fn should_emit(&self) -> bool {
        self.inserted > 0
    }

    fn reader_ingested_nothing(&self) -> bool {
        self.had_reader && self.inserted == 0
    }
}

/// Ingest the agent's on-disk transcript into `session_records`, idempotent per
/// `native_id`. `None` = no transcript reader for this provider (skip, don't
/// retry); `Some(n)` = reader ran, `n` new records inserted (`0` = nothing yet:
/// file not flushed, or its location/format changed).
fn sync_session_records(workspace: &WorkspaceManager, agent_id: &str) -> Option<usize> {
    let record = workspace.agent(agent_id).ok()?;
    let reader = crate::agent::transcript_reader(&record.provider)?;

    // A reader exists; from here any shortfall is "nothing yet" → Some(0).
    let Some(repo) = record.repos.first() else {
        return Some(0);
    };
    let Ok(cwd) = repo_checkout_path(agent_id, &repo.subdir) else {
        return Some(0);
    };

    // Resolve the session id. Event-stream agents have it on the record already;
    // plaintext agents (agy) read it from the filesystem at turn-end — persist
    // it here so the next turn can resume.
    let session_id = match record.session_id.clone() {
        Some(id) => id,
        None => {
            let captured = per_turn_descriptor(&record.provider)
                .and_then(|d| d.session_id_from_cwd)
                .and_then(|f| f(&cwd));
            match captured {
                Some(id) => {
                    let _ = workspace.set_agent_session_id(agent_id, &id);
                    id
                }
                None => return Some(0),
            }
        }
    };

    let paths = (reader.locate)(&session_id, &cwd);

    // Version-frozen snapshot tag (memoized probe — at most one --version per
    // provider per process).
    let version = crate::agent::cached_provider_version(&record.provider);

    // Read only what's new. Single-file JSONL readers tail from the stored byte
    // offset (O(new), not O(conversation) — the key win for long claude/image
    // sessions); multi-file / blob-dir readers fall back to a full read whose
    // already-stored rows are idempotently skipped. Either way the batch lands
    // in one transaction.
    // Per-turn agents in Custom view have exited by turn-end, so their final
    // line is complete even without a trailing newline (cursor/pi write it that
    // way) — consume it. Persistent writers (claude) keep the file open, so a
    // trailing line may be mid-write; hold it until it's newline-terminated.
    let consume_trailing = !is_persistent_runner(&record);
    let (records, new_offset) = match (reader.tail, paths.as_slice()) {
        (Some(tail), [path]) => {
            let offset = workspace.session_ingest_offset(agent_id).unwrap_or(0);
            let start_index = workspace.session_record_count(agent_id).unwrap_or(0);
            let (recs, next) = crate::agent::read_jsonl_tail(
                path,
                offset,
                start_index,
                tail.id_field,
                consume_trailing,
            );
            (recs, Some(next))
        }
        _ => ((reader.read)(&paths), None),
    };

    let batch: Vec<(&str, &serde_json::Value)> = records
        .iter()
        .map(|r| (r.native_id.as_str(), &r.body))
        .collect();
    let inserted = match workspace.append_session_records(
        agent_id,
        &record.provider,
        "transcript",
        version.as_deref(),
        &batch,
    ) {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(error = %e, agent_id, "append_session_records failed");
            0
        }
    };

    // Advance the tail cursor past the complete lines we just consumed (only for
    // the single-file readers; `None` leaves it untouched).
    if let Some(next) = new_offset {
        if let Err(e) = workspace.set_session_ingest_offset(agent_id, next) {
            tracing::warn!(error = %e, agent_id, "persist ingest offset failed");
        }
    }

    // Link any pending outgoing user turns to the canonical transcript
    // user-message rows just ingested (fills in their `native_id`).
    if let Err(e) = workspace.associate_pending_user_turns(agent_id) {
        tracing::warn!(error = %e, agent_id, "associate user turns failed");
    }

    Some(inserted)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── SyncPoll: per-turn stops on first batch; persistent waits for settle ──

    #[test]
    fn sync_poll_per_turn_stops_at_first_non_empty_read() {
        // A per-turn agent has exited, so the file is complete: the first batch
        // is the whole turn. Empty reads before it just ride out flush lag.
        let mut poll = SyncPoll::new(false);
        assert_eq!(poll.observe(Some(0)), PollControl::Continue); // not flushed yet
        assert_eq!(poll.observe(Some(6)), PollControl::Stop); // complete — done
        assert!(poll.should_emit());
    }

    #[test]
    fn sync_poll_persistent_settles_after_two_quiet_polls() {
        // Claude keeps the file open; only stop once it's been quiet for two
        // consecutive reads after we started ingesting.
        let mut poll = SyncPoll::new(true);
        assert_eq!(poll.observe(Some(5)), PollControl::Continue);
        assert_eq!(poll.observe(Some(0)), PollControl::Continue); // quiet 1
        assert_eq!(poll.observe(Some(0)), PollControl::Stop); // quiet 2 → settled
        assert!(poll.should_emit());
    }

    #[test]
    fn sync_poll_persistent_captures_a_late_answer_across_a_gap() {
        // The live-evidence case: tool-result + bookkeeping flush first, then the
        // final answer a phase later (an empty read in between). A new batch
        // resets the settle counter, so the answer is still ingested this turn.
        let mut poll = SyncPoll::new(true);
        assert_eq!(poll.observe(Some(7)), PollControl::Continue);
        assert_eq!(poll.observe(Some(0)), PollControl::Continue); // gap, quiet 1
        assert_eq!(poll.observe(Some(2)), PollControl::Continue); // answer lands → reset
        assert_eq!(poll.observe(Some(0)), PollControl::Continue); // quiet 1
        assert_eq!(poll.observe(Some(0)), PollControl::Stop); // quiet 2 → settled
        assert!(poll.should_emit());
    }

    #[test]
    fn sync_poll_no_reader_stops_immediately() {
        let mut poll = SyncPoll::new(true);
        assert_eq!(poll.observe(None), PollControl::Stop);
        assert!(!poll.should_emit());
        assert!(!poll.reader_ingested_nothing());
    }

    #[test]
    fn sync_poll_reader_but_nothing_ingested_warns() {
        let mut poll = SyncPoll::new(true);
        for _ in 0..5 {
            assert_eq!(poll.observe(Some(0)), PollControl::Continue);
        }
        assert!(!poll.should_emit());
        assert!(poll.reader_ingested_nothing());
    }
}
