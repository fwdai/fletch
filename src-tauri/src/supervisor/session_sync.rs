//! Turn-end transcript ingestion into `session_records`, plus PR-state
//! fetch/emit for an agent's primary repo.

use std::time::Duration;
use tauri::AppHandle;

use crate::agent::per_turn_descriptor;
use crate::github::{PrState, PrStatus};
use crate::workspace::{repo_checkout_path, AgentRecord, AgentView, TrackedRepo, WorkspaceManager};

use super::events::{
    emit_pr_state, emit_session_records_appended, emit_session_sync_health, emit_verification,
};
use super::Supervisor;

use parking_lot::Mutex;
use std::collections::HashMap;

/// Wall-clock ceiling for a turn-end verification, matching the ad-hoc
/// `run_verification` command (spec §9.4 uses the same 15-minute bound).
const TURN_END_VERIFY_TIMEOUT_SECS: u64 = 900;

/// Project setting key (stored by the frontend Project Settings toggle) that
/// opts a project into running verification at every ad-hoc turn end.
const VERIFY_ON_TURN_END_KEY: &str = "verify.on_turn_end";

impl Supervisor {
    /// Synchronously ingest the agent's transcript into session_records (used
    /// for lazy backfill when a session is opened with no records yet). `None`
    /// if the provider has no transcript reader.
    pub fn sync_session(&self, agent_id: &str) -> Option<usize> {
        sync_session_records(&self.workspace, agent_id).map(|o| o.inserted)
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
        let health_map = self.sync_health.clone();
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
            }
            // Classify only now, after the retry loop is exhausted — earlier the
            // transcript may just be mid-flush, which must never read as drift.
            if let Some(diag) = poll.turn_diagnostics() {
                let provider = workspace
                    .agent(&agent_id)
                    .map(|r| r.provider)
                    .unwrap_or_default();
                report_sync_health(
                    &app,
                    &health_map,
                    &agent_id,
                    &provider,
                    classify(diag),
                    diag,
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
            let state = resolve_pr_state(&workspace, &agent_id, None)
                .await
                .and_then(|(pr, bound)| bound.then_some(pr));
            emit_pr_state(&app, &agent_id, state);
        });
    }

    /// Fire-and-forget turn-end verification for an ad-hoc agent, gated on the
    /// project's opt-in `verify.on_turn_end` flag. Runs the same engine as the
    /// `run_verification` command on the agent's primary checkout and emits
    /// `verify:report`, so the agent's Mission Control card arrives with test
    /// evidence (workflow items already carry gate evidence). Never blocks the
    /// turn; any failure is silent.
    ///
    /// Guardrails (each a cheap early return): only ad-hoc agents (a workflow
    /// step agent has gates — `owner_run_id` set → skip), only when the flag is
    /// on, and never two at once for the same agent (they'd race on the
    /// checkout). Called only on a normal Idle transition (not stop/archive).
    pub fn trigger_turn_end_verification(&self, app: AppHandle, agent_id: String) {
        let Ok(record) = self.workspace.agent(&agent_id) else {
            return;
        };
        // Workflow step agents have their own gate evidence — skip them.
        if record.owner_run_id.is_some() {
            return;
        }
        // A user-stopped turn also converges on Idle, but its checkout is a
        // half-done interruption — not a turn to certify. The flag is still set
        // here (drain_message_queue consumes it after us), so peek without
        // clearing.
        if self.interrupted.lock().contains(&agent_id) {
            return;
        }
        let project_id = record.project_id;
        if project_id.is_empty() {
            return;
        }
        // Opt-in per project, OFF by default.
        let enabled = self
            .workspace
            .project_setting(&project_id, VERIFY_ON_TURN_END_KEY)
            .is_some_and(|v| matches!(v.trim(), "1" | "true"));
        if !enabled {
            return;
        }
        // Primary repo's checkout (the repo the agent was spawned against).
        let Some(primary) = record.repos.first() else {
            return;
        };
        let Ok(checkout) = repo_checkout_path(&agent_id, &primary.subdir) else {
            return;
        };
        // Debounce: skip if a verification for this agent is already running.
        if !self.verify_inflight.lock().insert(agent_id.clone()) {
            return;
        }
        // Project command overrides layer over detection, same as the ad-hoc
        // `run_verification` command and the workflow tests gate.
        let setting = |key: &str| self.workspace.project_setting(&project_id, key);
        let verifier = match crate::verify::Verifier::new(
            setting("run.test"),
            setting("run.install"),
            setting("run.lint"),
            TURN_END_VERIFY_TIMEOUT_SECS,
        ) {
            Ok(v) => v,
            Err(_) => {
                self.verify_inflight.lock().remove(&agent_id);
                return;
            }
        };
        let inflight = self.verify_inflight.clone();
        tauri::async_runtime::spawn(async move {
            let report = verifier.verify(&checkout).await;
            inflight.lock().remove(&agent_id);
            emit_verification(&app, &agent_id, report);
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
    subdir: Option<&str>,
) -> Option<(PrState, bool)> {
    let record = workspace.agent(agent_id).ok()?;
    let repo = match subdir {
        // A specific checkout (the multi-repo panel's per-repo sections).
        Some(s) => record.repos.iter().find(|r| r.subdir == s)?,
        // Default: the primary — the app-wide badge/poll shape.
        None => record.repos.first()?,
    };
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
        match crate::github::pr_view_number(&checkout, Some(&repo.repo_path), number as u32).await {
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

/// Store key for one repo's PR state in the app-wide maps, mirroring the
/// frontend's `gitKey` (`src/store/git.ts`): the primary repo keeps the plain
/// agent id — the key every existing consumer (sidebar badge, title bar,
/// `pr:state_changed` reducer) reads — and secondaries get
/// `"{agent_id}::{subdir}"`, the same suffixed form the Git panel's per-repo
/// fetches use.
pub(crate) fn pr_map_key(agent_id: &str, subdir: &str, primary: bool) -> String {
    if primary {
        agent_id.to_string()
    } else {
        format!("{agent_id}::{subdir}")
    }
}

/// Resolve PR state for *every* bound repo of every agent in one batched
/// round-trip — the app-wide poll behind `refresh_all_pr_states`. Same
/// per-repo policy as [`resolve_pr_state`], but the live lookups are collapsed
/// into a single aliased GraphQL query instead of a per-agent fan-out:
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
/// badge. Repos that resolve to nothing are omitted from the map (never
/// written as absent state), matching the command's contract. Keys follow
/// [`pr_map_key`]: plain agent id for the primary, `"{agent_id}::{subdir}"`
/// for secondaries — so single-repo agents produce exactly one plain-keyed
/// entry, byte-identical to before.
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

    // A network-bound repo: what to fetch, plus the snapshot to fall back to.
    struct Pending {
        agent_id: String,
        subdir: String,
        key: String,
        snapshot: Option<PrState>,
        pr_ref: PrRef,
    }
    let mut pending: Vec<Pending> = Vec::new();

    for agent in ws.agents {
        if agent.archive.is_some() {
            continue;
        }
        for (i, repo) in agent.repos.iter().enumerate() {
            let key = pr_map_key(&agent.id, &repo.subdir, i == 0);
            // No branch → nothing pushed; no number → discovery isn't this poll's job.
            if repo.branch.is_none() {
                continue;
            }
            let Some(number) = repo.pr_number else {
                continue;
            };
            let snapshot = pr_snapshot(repo);

            let terminal = repo.pr_state.as_deref() == Some(PrStatus::Merged.as_str());
            let closed = repo.pr_state.as_deref() == Some(PrStatus::Closed.as_str());
            // Merged never re-fetches; closed only on the slow re-verify tick.
            let fetch = !paused && !terminal && (!closed || reverify_closed);
            if !fetch {
                if let Some(snap) = snapshot {
                    out.insert(key, snap);
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
                    key,
                    snapshot,
                    pr_ref: PrRef {
                        owner,
                        repo: repo_name,
                        number: number as u32,
                    },
                }),
                None => {
                    if let Some(snap) = snapshot {
                        out.insert(key, snap);
                    }
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
                        out.insert(p.key, pr);
                    }
                    // Not found this round / partial error — keep last-known.
                    None => {
                        if let Some(snap) = p.snapshot {
                            out.insert(p.key, snap);
                        }
                    }
                }
            }
        }
        // Whole-batch failure — degrade every bound repo to its snapshot.
        Err(_) => {
            for p in pending {
                if let Some(snap) = p.snapshot {
                    out.insert(p.key, snap);
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

/// One `sync_session_records` pass: how many new records it inserted plus the
/// counters the locate/read collected. The count drives the poll's stop/settle
/// logic; the diagnostics drive the post-exhaustion health classification.
struct SyncOutcome {
    inserted: usize,
    diagnostics: crate::agent::ReadDiagnostics,
}

/// The turn-end ingest health for a reader-backed agent, derived purely from
/// the last pass's [`ReadDiagnostics`](crate::agent::ReadDiagnostics).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum SyncHealth {
    /// Records parsed this turn — ingestion is working.
    Healthy,
    /// The CLI's transcript root dir is gone though it just ran a turn — a
    /// strong drift signal.
    NoRoot,
    /// Root present, but no transcript files matched. Ambiguous (slow flush /
    /// empty session), so this is logged, never surfaced to the UI.
    NoFiles,
    /// Files matched and carried non-blank lines, yet none parsed — the vendor
    /// reshaped its format. The smoking gun, essentially zero false positives.
    FormatDrift,
    /// Files matched but couldn't be opened or read (permissions, vanished
    /// mid-read) and nothing parsed — ingestion failed just as surely as drift.
    ReadError,
    /// Records parsed, but the turn also hit a read failure — the tail of the
    /// transcript may be missing. Degraded: the parsed records are kept, but
    /// the read failure surfaces like any other ingest failure.
    PartialRead,
}

impl SyncHealth {
    /// The wire status for `session:sync-health`. `None` for `NoFiles`, which
    /// is never emitted.
    fn wire(self) -> Option<&'static str> {
        match self {
            SyncHealth::Healthy => Some("healthy"),
            SyncHealth::NoRoot => Some("no_root"),
            SyncHealth::FormatDrift => Some("format_drift"),
            SyncHealth::ReadError => Some("read_error"),
            SyncHealth::PartialRead => Some("partial_read"),
            SyncHealth::NoFiles => None,
        }
    }
}

/// Classify a completed poll's diagnostics into a health signal. Pure so it's
/// unit-testable over hand-built counters.
///
/// The final fall-through to `Healthy` is deliberate: an incremental tail read
/// of an already-ingested transcript reads no new bytes on the settle poll
/// (`files_matched > 0`, `records_parsed == 0`, `lines_seen == 0`,
/// `io_errors == 0`), which must NOT read as drift. Real drift always leaves
/// non-blank lines that failed to parse (`lines_seen > 0`), and a matched file
/// that couldn't be read at all leaves `io_errors > 0`.
fn classify(diag: &crate::agent::ReadDiagnostics) -> SyncHealth {
    if diag.records_parsed > 0 {
        if diag.io_errors > 0 {
            SyncHealth::PartialRead
        } else {
            SyncHealth::Healthy
        }
    } else if !diag.root_exists {
        SyncHealth::NoRoot
    } else if diag.files_matched == 0 {
        SyncHealth::NoFiles
    } else if diag.lines_seen > 0 {
        SyncHealth::FormatDrift
    } else if diag.io_errors > 0 {
        SyncHealth::ReadError
    } else {
        SyncHealth::Healthy
    }
}

/// Decision logic for the turn-end transcript sync, split out from
/// `trigger_session_sync` so it's unit-testable without timers or the
/// filesystem. Fed each `sync_session_records` result (`None` = no reader,
/// `Some(outcome)` = the pass's record count + diagnostics).
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
    /// Diagnostics accumulated across every reader pass this turn, classified
    /// only after the retry loop exhausts — never mid-poll, so flush lag can't
    /// false-positive. Accumulated (not last-pass) so a clean settle read can't
    /// erase an earlier pass's read errors and misreport the turn as Healthy.
    turn_diag: Option<crate::agent::ReadDiagnostics>,
}

impl SyncPoll {
    fn new(persistent: bool) -> Self {
        Self {
            persistent,
            had_reader: false,
            inserted: 0,
            stable_polls: 0,
            turn_diag: None,
        }
    }

    fn observe(&mut self, result: Option<SyncOutcome>) -> PollControl {
        let Some(SyncOutcome {
            inserted: n,
            diagnostics,
        }) = result
        else {
            return PollControl::Stop; // no reader — nothing to wait for
        };
        self.had_reader = true;
        match &mut self.turn_diag {
            Some(acc) => acc.absorb(&diagnostics),
            None => self.turn_diag = Some(diagnostics),
        }
        if n == 0 {
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
        } else {
            self.inserted += n;
            self.stable_polls = 0; // new content → not settled
            if self.persistent {
                PollControl::Continue
            } else {
                PollControl::Stop // exited → the batch is complete
            }
        }
    }

    fn should_emit(&self) -> bool {
        self.inserted > 0
    }

    /// The health of the whole turn — `None` when there was no reader (nothing
    /// to classify). Test-only convenience; production classifies via
    /// [`turn_diagnostics`](Self::turn_diagnostics) so it can log the counters.
    #[cfg(test)]
    fn health(&self) -> Option<SyncHealth> {
        self.turn_diag.as_ref().map(classify)
    }

    /// Diagnostics accumulated over the turn's passes — `None` when there was
    /// no reader.
    fn turn_diagnostics(&self) -> Option<&crate::agent::ReadDiagnostics> {
        self.turn_diag.as_ref()
    }
}

/// Log every degraded pass (with full counters) and emit `session:sync-health`
/// on status *change* only — a persistent drift must not fire an event per
/// turn. `Healthy` is emitted solely to clear a prior degraded state; `NoFiles`
/// is logged but never emitted (ambiguous — slow flush or empty session).
fn report_sync_health(
    app: &AppHandle,
    health_map: &Mutex<HashMap<String, SyncHealth>>,
    agent_id: &str,
    provider: &str,
    health: SyncHealth,
    diag: &crate::agent::ReadDiagnostics,
) {
    match health {
        SyncHealth::Healthy => {}
        SyncHealth::PartialRead => {
            tracing::warn!(
                agent_id,
                provider,
                records_parsed = diag.records_parsed,
                io_errors = diag.io_errors,
                "session sync hit read errors after ingesting records — transcript tail may be missing",
            );
        }
        _ => {
            tracing::warn!(
                agent_id,
                provider,
                ?health,
                root_exists = diag.root_exists,
                files_matched = diag.files_matched,
                lines_seen = diag.lines_seen,
                records_parsed = diag.records_parsed,
                io_errors = diag.io_errors,
                "session sync ingested 0 records after retries",
            );
        }
    }

    let mut map = health_map.lock();
    match health {
        // Ambiguous — logged above, but never emitted and never mutates state
        // (so it can't clear a real degraded status either).
        SyncHealth::NoFiles => {}
        SyncHealth::Healthy => {
            if map.remove(agent_id).is_some() {
                // Clearing a previously-degraded status.
                emit_session_sync_health(
                    app,
                    agent_id,
                    provider,
                    "healthy",
                    crate::agent::cached_provider_version(provider),
                );
            }
        }
        degraded => {
            if map.get(agent_id) != Some(&degraded) {
                map.insert(agent_id.to_string(), degraded);
                if let Some(status) = degraded.wire() {
                    emit_session_sync_health(
                        app,
                        agent_id,
                        provider,
                        status,
                        crate::agent::cached_provider_version(provider),
                    );
                }
            }
        }
    }
}

/// Ingest the agent's on-disk transcript into `session_records`, idempotent per
/// `native_id`. `None` = no transcript reader for this provider (skip, don't
/// retry); `Some(outcome)` = reader ran, carrying the count of new records
/// inserted (`0` = nothing yet: file not flushed, or its location/format
/// changed) plus the `ReadDiagnostics` the locate/read pass collected so the
/// caller can tell those two apart.
fn sync_session_records(workspace: &WorkspaceManager, agent_id: &str) -> Option<SyncOutcome> {
    let record = workspace.agent(agent_id).ok()?;
    let reader = crate::agent::transcript_reader(&record.provider)?;

    // A reader exists but we couldn't even attempt a real read this pass (no
    // repo / broken checkout / session id not captured yet). This is an
    // internal / not-yet-ready state, never vendor drift, so report it as
    // ambiguous (`root_exists`, no files → NoFiles, which is log-only) rather
    // than letting the default all-false diagnostics read as NoRoot. Still
    // `inserted == 0`, so the poll keeps retrying exactly as before.
    let pending = || SyncOutcome {
        inserted: 0,
        diagnostics: crate::agent::ReadDiagnostics {
            root_exists: true,
            ..Default::default()
        },
    };

    let Some(repo) = record.repos.first() else {
        return Some(pending());
    };
    let Ok(cwd) = repo_checkout_path(agent_id, &repo.subdir) else {
        return Some(pending());
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
                None => return Some(pending()),
            }
        }
    };

    let mut diagnostics = crate::agent::ReadDiagnostics::default();
    let paths = (reader.locate)(&session_id, &cwd, &mut diagnostics);

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
                &mut diagnostics,
            );
            (recs, Some(next))
        }
        _ => ((reader.read)(&paths, &mut diagnostics), None),
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

    Some(SyncOutcome {
        inserted,
        diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::ReadDiagnostics;
    use std::path::Path;

    // Serialize the two env-var drift tests: they mutate process-global
    // CODEX_HOME / CLAUDE_CONFIG_DIR, and cargo runs tests in parallel.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A pass that ingested `n` records off a healthy transcript.
    fn healthy(n: usize) -> Option<SyncOutcome> {
        Some(SyncOutcome {
            inserted: n,
            diagnostics: ReadDiagnostics {
                root_exists: true,
                files_matched: 1,
                lines_seen: n.max(1),
                records_parsed: n.max(1),
                io_errors: 0,
            },
        })
    }

    /// A pass that read a matched file but parsed nothing (format drift).
    fn drifted() -> Option<SyncOutcome> {
        Some(SyncOutcome {
            inserted: 0,
            diagnostics: ReadDiagnostics {
                root_exists: true,
                files_matched: 1,
                lines_seen: 3,
                records_parsed: 0,
                io_errors: 0,
            },
        })
    }

    // ── SyncPoll: per-turn stops on first batch; persistent waits for settle ──

    #[test]
    fn sync_poll_per_turn_stops_at_first_non_empty_read() {
        // A per-turn agent has exited, so the file is complete: the first batch
        // is the whole turn. Empty reads before it just ride out flush lag.
        let mut poll = SyncPoll::new(false);
        assert_eq!(poll.observe(healthy(0)), PollControl::Continue); // not flushed yet
        assert_eq!(poll.observe(healthy(6)), PollControl::Stop); // complete — done
        assert!(poll.should_emit());
    }

    #[test]
    fn sync_poll_persistent_settles_after_two_quiet_polls() {
        // Claude keeps the file open; only stop once it's been quiet for two
        // consecutive reads after we started ingesting.
        let mut poll = SyncPoll::new(true);
        assert_eq!(poll.observe(healthy(5)), PollControl::Continue);
        assert_eq!(poll.observe(healthy(0)), PollControl::Continue); // quiet 1
        assert_eq!(poll.observe(healthy(0)), PollControl::Stop); // quiet 2 → settled
        assert!(poll.should_emit());
    }

    #[test]
    fn sync_poll_persistent_captures_a_late_answer_across_a_gap() {
        // The live-evidence case: tool-result + bookkeeping flush first, then the
        // final answer a phase later (an empty read in between). A new batch
        // resets the settle counter, so the answer is still ingested this turn.
        let mut poll = SyncPoll::new(true);
        assert_eq!(poll.observe(healthy(7)), PollControl::Continue);
        assert_eq!(poll.observe(healthy(0)), PollControl::Continue); // gap, quiet 1
        assert_eq!(poll.observe(healthy(2)), PollControl::Continue); // answer lands → reset
        assert_eq!(poll.observe(healthy(0)), PollControl::Continue); // quiet 1
        assert_eq!(poll.observe(healthy(0)), PollControl::Stop); // quiet 2 → settled
        assert!(poll.should_emit());
    }

    #[test]
    fn sync_poll_no_reader_stops_immediately() {
        let mut poll = SyncPoll::new(true);
        assert_eq!(poll.observe(None), PollControl::Stop);
        assert!(!poll.should_emit());
        assert_eq!(poll.health(), None);
    }

    // ── classifier: the four states, over hand-built diagnostics ──

    #[test]
    fn classify_healthy_when_records_parsed() {
        let d = ReadDiagnostics {
            root_exists: true,
            files_matched: 1,
            lines_seen: 4,
            records_parsed: 4,
            io_errors: 0,
        };
        assert_eq!(classify(&d), SyncHealth::Healthy);
    }

    #[test]
    fn classify_no_root_when_root_missing() {
        let d = ReadDiagnostics {
            root_exists: false,
            ..Default::default()
        };
        assert_eq!(classify(&d), SyncHealth::NoRoot);
    }

    #[test]
    fn classify_no_files_when_root_present_but_no_matches() {
        let d = ReadDiagnostics {
            root_exists: true,
            files_matched: 0,
            ..Default::default()
        };
        assert_eq!(classify(&d), SyncHealth::NoFiles);
    }

    #[test]
    fn classify_format_drift_when_lines_seen_but_none_parse() {
        let d = ReadDiagnostics {
            root_exists: true,
            files_matched: 2,
            lines_seen: 5,
            records_parsed: 0,
            io_errors: 0,
        };
        assert_eq!(classify(&d), SyncHealth::FormatDrift);
    }

    #[test]
    fn classify_partial_read_when_records_and_read_failure() {
        // A read that fails partway after ingesting records is PartialRead:
        // records flowed (so no new banner — the next sync re-attempts the
        // tail via the persisted offset / idempotent dedup), but the pass
        // still failed, so it must not clear a prior degraded status either.
        let d = ReadDiagnostics {
            root_exists: true,
            files_matched: 1,
            lines_seen: 3,
            records_parsed: 3,
            io_errors: 1,
        };
        assert_eq!(classify(&d), SyncHealth::PartialRead);
    }

    #[test]
    fn classify_read_error_when_matched_files_unreadable() {
        // locate matched a transcript but every read failed (permissions,
        // vanished mid-read): no lines were ever seen, so without the
        // io_errors branch this would fall through to Healthy.
        let d = ReadDiagnostics {
            root_exists: true,
            files_matched: 1,
            lines_seen: 0,
            records_parsed: 0,
            io_errors: 1,
        };
        assert_eq!(classify(&d), SyncHealth::ReadError);
    }

    #[test]
    fn classify_healthy_on_tail_settle_reading_no_new_bytes() {
        // The false-positive guard: an already-ingested transcript tail-read at
        // EOF matches a file but reads no new lines — must not read as drift.
        let d = ReadDiagnostics {
            root_exists: true,
            files_matched: 1,
            lines_seen: 0,
            records_parsed: 0,
            io_errors: 0,
        };
        assert_eq!(classify(&d), SyncHealth::Healthy);
    }

    // ── SyncPoll health: classification waits for retry exhaustion ──

    #[test]
    fn sync_poll_flush_lag_zero_then_records_is_healthy() {
        // A zero→nonzero flush-lag sequence: early empty reads must not classify
        // (that would false-positive a slow flush as drift). The turn's
        // accumulated diagnostics carried records → Healthy.
        let mut poll = SyncPoll::new(false);
        assert_eq!(poll.observe(healthy(0)), PollControl::Continue);
        assert_eq!(poll.observe(healthy(4)), PollControl::Stop);
        assert_eq!(poll.health(), Some(SyncHealth::Healthy));
    }

    #[test]
    fn sync_poll_persistent_drift_classifies_after_exhaustion() {
        // Claude never ingests anything: every pass matches the file but parses
        // nothing. After the loop exhausts, the turn diagnostics → FormatDrift.
        let mut poll = SyncPoll::new(true);
        for _ in 0..8 {
            assert_eq!(poll.observe(drifted()), PollControl::Continue);
        }
        assert!(!poll.should_emit());
        assert_eq!(poll.health(), Some(SyncHealth::FormatDrift));
    }

    #[test]
    fn sync_poll_settle_reads_do_not_erase_an_earlier_read_error() {
        // Persistent agent: the first pass ingests records but also hits a read
        // error, then the file goes quiet and clean settle reads follow. The
        // turn must classify PartialRead — a last-pass-wins diagnostic would let
        // the clean settle read report Healthy and clear a real degraded state.
        let partial = || {
            Some(SyncOutcome {
                inserted: 5,
                diagnostics: ReadDiagnostics {
                    root_exists: true,
                    files_matched: 1,
                    lines_seen: 5,
                    records_parsed: 5,
                    io_errors: 1,
                },
            })
        };
        // A genuinely empty settle read: tail at EOF, nothing seen, no errors.
        let quiet = || {
            Some(SyncOutcome {
                inserted: 0,
                diagnostics: ReadDiagnostics {
                    root_exists: true,
                    files_matched: 1,
                    lines_seen: 0,
                    records_parsed: 0,
                    io_errors: 0,
                },
            })
        };
        let mut poll = SyncPoll::new(true);
        assert_eq!(poll.observe(partial()), PollControl::Continue);
        assert_eq!(poll.observe(quiet()), PollControl::Continue); // quiet 1
        assert_eq!(poll.observe(quiet()), PollControl::Stop); // quiet 2 → settled
        assert!(poll.should_emit());
        assert_eq!(poll.health(), Some(SyncHealth::PartialRead));
    }

    // ── real-reader drift, via env-injectable roots ──
    // These exercise the actual vendor readers through the public
    // `transcript_reader` dispatch, so the diagnostics + classification are
    // validated end-to-end (not just over hand-built counters).

    /// Run a vendor reader against a session id / cwd and classify the result.
    fn read_and_classify(
        provider: &str,
        session_id: &str,
        cwd: &Path,
    ) -> (SyncHealth, ReadDiagnostics) {
        let reader = crate::agent::transcript_reader(provider).expect("reader");
        let mut diag = ReadDiagnostics::default();
        let paths = (reader.locate)(session_id, cwd, &mut diag);
        let _ = (reader.read)(&paths, &mut diag);
        (classify(&diag), diag)
    }

    #[test]
    fn codex_reader_drift_states_via_codex_home() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("CODEX_HOME", home.path());
        let cwd = home.path(); // codex ignores cwd
        let day = home.path().join("sessions/2024/01/02");

        // (a) missing root → NoRoot (no `sessions` dir yet).
        let (h, d) = read_and_classify("codex", "sid-a", cwd);
        assert_eq!(h, SyncHealth::NoRoot, "diag={d:?}");
        assert!(!d.root_exists);

        // (b) root present, no matching rollout → NoFiles.
        std::fs::create_dir_all(&day).unwrap();
        let (h, d) = read_and_classify("codex", "sid-b", cwd);
        assert_eq!(h, SyncHealth::NoFiles, "diag={d:?}");
        assert!(d.root_exists && d.files_matched == 0);

        // (c) matching rollout of pure garbage → FormatDrift, zero records.
        let garbage = day.join("rollout-20240102T0000-sid-c.jsonl");
        std::fs::write(&garbage, b"not json\nalso not json\n").unwrap();
        let (h, d) = read_and_classify("codex", "sid-c", cwd);
        assert_eq!(h, SyncHealth::FormatDrift, "diag={d:?}");
        assert_eq!(d.files_matched, 1);
        assert_eq!(d.records_parsed, 0);
        assert!(d.lines_seen >= 2);

        // (d) mixed file: good lines still ingested, skips counted.
        let mixed = day.join("rollout-20240102T0001-sid-d.jsonl");
        std::fs::write(&mixed, b"{\"a\":1}\ngarbage\n{\"b\":2}\n").unwrap();
        let reader = crate::agent::transcript_reader("codex").unwrap();
        let mut diag = ReadDiagnostics::default();
        let paths = (reader.locate)("sid-d", cwd, &mut diag);
        let recs = (reader.read)(&paths, &mut diag);
        assert_eq!(recs.len(), 2, "the two valid lines are still returned");
        assert_eq!(diag.records_parsed, 2);
        assert_eq!(
            diag.lines_seen, 3,
            "the garbage line was seen but not parsed"
        );
        assert_eq!(classify(&diag), SyncHealth::Healthy);

        std::env::remove_var("CODEX_HOME");
    }

    #[test]
    fn claude_reader_drift_states_via_config_dir() {
        // NoRoot is not asserted here: Claude falls back to `~/.claude/projects`,
        // which exists on many dev machines, so a missing CLAUDE_CONFIG_DIR alone
        // can't guarantee `!root_exists`. NoRoot is covered by the codex reader
        // test and the pure classifier test.
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap(); // no docker per-agent dir under it
        std::env::set_var("CLAUDE_CONFIG_DIR", cfg.path());
        let projects = cfg.path().join("projects");
        let slug = projects.join("-tmp-slug");
        std::fs::create_dir_all(&slug).unwrap();

        // (b) root present, session file absent → NoFiles (random uuid can't
        // collide with a real file in the ~/.claude fallback).
        let sid_b = "11111111-1111-4111-8111-111111111111";
        let (h, d) = read_and_classify("claude", sid_b, cwd.path());
        assert_eq!(h, SyncHealth::NoFiles, "diag={d:?}");
        assert!(d.root_exists && d.files_matched == 0);

        // (c) session file of garbage → FormatDrift.
        let sid_c = "22222222-2222-4222-8222-222222222222";
        std::fs::write(slug.join(format!("{sid_c}.jsonl")), b"{oops\nnope\n").unwrap();
        let (h, d) = read_and_classify("claude", sid_c, cwd.path());
        assert_eq!(h, SyncHealth::FormatDrift, "diag={d:?}");
        assert_eq!(d.files_matched, 1);
        assert_eq!(d.records_parsed, 0);

        // (d) mixed file: valid records still ingested.
        let sid_d = "33333333-3333-4333-8333-333333333333";
        std::fs::write(
            slug.join(format!("{sid_d}.jsonl")),
            b"{\"uuid\":\"x\",\"type\":\"user\"}\ntorn{\n{\"uuid\":\"y\"}\n",
        )
        .unwrap();
        let reader = crate::agent::transcript_reader("claude").unwrap();
        let mut diag = ReadDiagnostics::default();
        let paths = (reader.locate)(sid_d, cwd.path(), &mut diag);
        let recs = (reader.read)(&paths, &mut diag);
        assert_eq!(recs.len(), 2);
        assert_eq!(diag.records_parsed, 2);
        assert_eq!(diag.lines_seen, 3);
        assert_eq!(classify(&diag), SyncHealth::Healthy);

        std::env::remove_var("CLAUDE_CONFIG_DIR");
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
            label: None,
        }
    }

    /// The app-wide poll keys mirror the frontend's `gitKey`: the primary repo
    /// keeps the plain agent id (so every existing consumer is untouched) and
    /// secondaries get the `::`-suffixed form the Git panel already reads.
    #[test]
    fn pr_map_key_mirrors_frontend_git_key() {
        assert_eq!(pr_map_key("ag-1", "frontend", true), "ag-1");
        assert_eq!(pr_map_key("ag-1", "backend", false), "ag-1::backend");
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
