//! Turn-end transcript ingestion into `session_records`, plus PR-state
//! fetch/emit for an agent's primary repo.

use std::time::Duration;
use tauri::AppHandle;

use crate::agent::per_turn_descriptor;
use crate::github::{PrState, PrStatus};
use crate::workspace::{repo_checkout_path, AgentRecord, AgentView, TrackedRepo, WorkspaceManager};

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
            // Only bound PRs are emitted app-wide: an unbound merged/closed PR
            // discovered on a recycled branch name is focused-panel display
            // (`get_pr_state`), not this agent's state.
            let state = resolve_pr_state(&workspace, &agent_id)
                .await
                .and_then(|(pr, bound)| bound.then_some(pr));
            emit_pr_state(&app, &agent_id, state);
        });
    }
}

/// The last persisted state of a repo's bound PR, rebuilt from its database
/// columns — what the UI shows when GitHub or the checkout is unavailable.
/// `None` when no PR is bound or no fetch has ever succeeded (state column
/// still NULL). `mergeable` isn't persisted and reads `false`; it only means
/// anything while an open PR is being polled live.
pub(crate) fn pr_snapshot(repo: &TrackedRepo) -> Option<PrState> {
    let number = repo.pr_number?;
    let state = PrStatus::parse(repo.pr_state.as_deref()?)?;
    Some(PrState {
        number: number as u32,
        url: repo.pr_url.clone().unwrap_or_default(),
        state,
        title: repo.pr_title.clone().unwrap_or_default(),
        mergeable: false,
        opened_at: None,
        merged_at: None,
    })
}

/// Persist a freshly-fetched PR state to its repo's snapshot columns, logging
/// (never propagating) a write failure. Every path that learns a PR's state —
/// the single and batched polls and `create_pr` — funnels through here so the
/// persistence contract lives in one place.
pub(crate) fn persist_pr_snapshot(
    workspace: &WorkspaceManager,
    agent_id: &str,
    subdir: &str,
    pr: &PrState,
) {
    if let Err(e) = workspace.set_repo_pr_snapshot(agent_id, subdir, pr) {
        tracing::warn!(error = %e, agent_id, pr = pr.number, "failed to persist PR snapshot");
    }
}

/// Resolve the current PR state for an agent's primary repo, persisting what
/// it learns. Returns the state plus whether that PR is *bound* to the agent
/// by number. The single implementation behind the focused panel
/// (`get_pr_state`), the app-wide poll (`refresh_all_pr_states`), and the
/// per-trigger emit (`fetch_and_emit_pr_state`).
///
/// - **Bound PR** (`pr_number` recorded): a persisted `merged` state returns
///   straight from the database — merges don't un-happen, so no network or
///   git access is spent re-confirming them. Otherwise fetch by number
///   (resolving owner/repo from the checkout, or from the source repo when
///   the checkout is broken), persist the result, and on failure degrade to
///   the last persisted snapshot — a failed fetch must never erase state
///   GitHub already confirmed.
/// - **No bound PR**: discover one by branch name. An OPEN PR is adopted
///   (persisted, becoming bound); a merged/closed one is returned unbound —
///   displayable, but never claimed as this agent's, so a recycled branch
///   name can't inherit a prior agent's PR.
pub(crate) async fn resolve_pr_state(
    workspace: &WorkspaceManager,
    agent_id: &str,
) -> Option<(PrState, bool)> {
    let record = workspace.agent(agent_id).ok()?;
    let repo = record.repos.first()?;
    // No branch yet → nothing pushed, so no PR to find or bind.
    repo.branch.as_ref()?;
    let checkout = repo_checkout_path(agent_id, &repo.subdir).ok()?;

    if let Some(number) = repo.pr_number {
        // Merged is the one terminal state — it can't be undone, so it's
        // served from the database with no network spent re-confirming it.
        // Closed is deliberately NOT short-circuited: a closed PR can be
        // reopened, so it stays on the live-fetch path (and keeps costing a
        // poll per cycle) to catch that transition.
        if repo.pr_state.as_deref() == Some(PrStatus::Merged.as_str()) {
            return pr_snapshot(repo).map(|pr| (pr, true));
        }
        match crate::github::pr_view_number(&checkout, Some(&repo.repo_path), number as u32).await
        {
            Ok(Some(pr)) => {
                persist_pr_snapshot(workspace, agent_id, &repo.subdir, &pr);
                Some((pr, true))
            }
            // Unreachable or not found — fall back to the last confirmed state.
            _ => pr_snapshot(repo).map(|pr| (pr, true)),
        }
    } else {
        match crate::github::pr_view(&checkout).await.unwrap_or(None) {
            Some(pr) if matches!(pr.state, PrStatus::Open) => {
                persist_pr_snapshot(workspace, agent_id, &repo.subdir, &pr);
                Some((pr, true))
            }
            Some(pr) => Some((pr, false)),
            None => None,
        }
    }
}

/// Resolve PR state for *every* bound agent in one batched round-trip — the
/// app-wide poll behind `refresh_all_pr_states`. Same per-agent policy as
/// [`resolve_pr_state`], but the live lookups are collapsed into a single
/// aliased GraphQL query instead of a per-agent fan-out:
///
/// - **Merged** PRs are served from the persisted snapshot (terminal — never
///   re-fetched).
/// - **Closed** PRs are served from the snapshot too, *except* on the slow
///   re-verify tick (`reverify_closed`), so a reopen is still eventually caught
///   without paying a poll every cycle.
/// - Everything else is fetched live by number and its snapshot refreshed.
///
/// A paused backoff, an unresolvable slug, a not-found alias, or a whole-batch
/// failure all degrade to the last persisted snapshot rather than wiping the
/// badge. Agents that resolve to nothing are omitted from the map (never
/// written as absent state), matching the command's contract.
pub(crate) async fn resolve_all_pr_states(
    workspace: &WorkspaceManager,
    reverify_closed: bool,
) -> std::collections::HashMap<String, PrState> {
    use crate::github::{client, PrRef};
    use std::collections::HashMap;

    let mut out: HashMap<String, PrState> = HashMap::new();
    let Some(ws) = workspace.current() else {
        return out;
    };
    // Paused → touch no network; every bound PR renders from its snapshot.
    let paused = client::is_backing_off();

    // A network-bound agent: what to fetch, plus the snapshot to fall back to.
    struct Pending {
        agent_id: String,
        subdir: String,
        snapshot: Option<PrState>,
        pr_ref: PrRef,
    }
    let mut pending: Vec<Pending> = Vec::new();

    for agent in ws.agents {
        if agent.archive.is_some() {
            continue;
        }
        let Some(repo) = agent.repos.first() else { continue };
        // No branch → nothing pushed; no number → discovery isn't this poll's job.
        if repo.branch.is_none() {
            continue;
        }
        let Some(number) = repo.pr_number else { continue };
        let snapshot = pr_snapshot(repo);

        let terminal = repo.pr_state.as_deref() == Some(PrStatus::Merged.as_str());
        let closed = repo.pr_state.as_deref() == Some(PrStatus::Closed.as_str());
        // Merged never re-fetches; closed only on the slow re-verify tick.
        let fetch = !paused && !terminal && (!closed || reverify_closed);
        if !fetch {
            if let Some(snap) = snapshot {
                out.insert(agent.id.clone(), snap);
            }
            continue;
        }

        // Resolve the slug now (local git); the network cost is deferred to the
        // one batched query below. A broken checkout / non-GitHub origin can't
        // be fetched — hold the snapshot instead.
        let slug = match repo_checkout_path(&agent.id, &repo.subdir) {
            Ok(checkout) => crate::github::resolve_slug(&checkout, Some(&repo.repo_path)).await,
            Err(_) => None,
        };
        match slug {
            Some((owner, repo_name)) => pending.push(Pending {
                agent_id: agent.id.clone(),
                subdir: repo.subdir.clone(),
                snapshot,
                pr_ref: PrRef { owner, repo: repo_name, number: number as u32 },
            }),
            None => {
                if let Some(snap) = snapshot {
                    out.insert(agent.id.clone(), snap);
                }
            }
        }
    }

    if pending.is_empty() {
        return out;
    }

    let refs: Vec<PrRef> = pending.iter().map(|p| p.pr_ref.clone()).collect();
    match crate::github::pr_states_batch(&refs).await {
        Ok(results) => {
            for (p, res) in pending.into_iter().zip(results) {
                match res {
                    Some(pr) => {
                        persist_pr_snapshot(workspace, &p.agent_id, &p.subdir, &pr);
                        out.insert(p.agent_id, pr);
                    }
                    // Not found this round / partial error — keep last-known.
                    None => {
                        if let Some(snap) = p.snapshot {
                            out.insert(p.agent_id, snap);
                        }
                    }
                }
            }
        }
        // Whole-batch failure — degrade every bound agent to its snapshot.
        Err(_) => {
            for p in pending {
                if let Some(snap) = p.snapshot {
                    out.insert(p.agent_id, snap);
                }
            }
        }
    }
    out
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

    fn snapshot_repo() -> TrackedRepo {
        TrackedRepo {
            repo_path: std::path::PathBuf::from("/r"),
            subdir: "repo".into(),
            branch: Some("feat/x".into()),
            parent_branch: Some("main".into()),
            base_sha: None,
            pr_number: Some(42),
            pr_url: Some("https://github.com/o/r/pull/42".into()),
            pr_title: Some("feat: x".into()),
            pr_state: Some("merged".into()),
        }
    }

    /// The persisted columns rebuild into a renderable PrState.
    #[test]
    fn pr_snapshot_rebuilds_from_columns() {
        let pr = pr_snapshot(&snapshot_repo()).expect("snapshot");
        assert_eq!(pr.number, 42);
        assert_eq!(pr.url, "https://github.com/o/r/pull/42");
        assert_eq!(pr.title, "feat: x");
        assert!(matches!(pr.state, PrStatus::Merged));
        assert!(!pr.mergeable, "mergeable isn't persisted — must read false");
    }

    /// No bound number, no persisted state, or an unknown state string all
    /// mean "no snapshot" — never a fabricated badge.
    #[test]
    fn pr_snapshot_requires_number_and_valid_state() {
        let mut no_number = snapshot_repo();
        no_number.pr_number = None;
        assert!(pr_snapshot(&no_number).is_none());

        let mut no_state = snapshot_repo();
        no_state.pr_state = None;
        assert!(pr_snapshot(&no_state).is_none());

        let mut bad_state = snapshot_repo();
        bad_state.pr_state = Some("weird".into());
        assert!(pr_snapshot(&bad_state).is_none());
    }
}
