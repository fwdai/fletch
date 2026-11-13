//! Every event the supervisor emits, in one place: the payload types and a
//! typed emit fn per event, each taking the [`EventSink`] the event goes out
//! on (the desktop's is the `TauriSink`). Emit failures (serialization —
//! effectively never) are logged, not surfaced; no event is delivery-guaranteed
//! and the frontend resyncs on focus rather than trusting delivery.

use serde_json::Value;

use crate::github::{PrChecks, PrComments, PrState};
use crate::host::EventSink;
// Shared with the other PTY-carrying events (see `provider_login` commands),
// so the base64 wire format is defined once next to the sessions producing it.
use crate::pty_session::serialize_bytes_b64;
use crate::run_session::{RunPhase, RunSession, RunState};
use crate::sandbox::ForwardedPort;
use crate::workspace::{AgentStatus, AgentView, TrackedRepo};

/// Emit one event, logging (not propagating) failure — the shared tail of
/// every typed emitter below.
fn emit<T: serde::Serialize>(sink: &dyn EventSink, event: &str, payload: T) {
    crate::host::emit(sink, event, &payload);
}

#[derive(Clone, serde::Serialize)]
struct AgentOutputPayload {
    agent_id: String,
    #[serde(serialize_with = "serialize_bytes_b64")]
    bytes: Vec<u8>,
}

/// Raw PTY bytes from an agent's process (native view).
pub(super) fn emit_agent_output(sink: &dyn EventSink, agent_id: &str, bytes: Vec<u8>) {
    emit(
        sink,
        "agent:output",
        AgentOutputPayload {
            agent_id: agent_id.to_string(),
            bytes,
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct AgentEventPayload {
    agent_id: String,
    event: Value,
    /// The agent's per-event count under this host process (see `live_turn`):
    /// what lets a client that replayed the turn skip the frames the replay
    /// already held.
    seq: u64,
}

/// One parsed JSON event from a managed/per-turn agent's stream.
pub(super) fn emit_agent_event(sink: &dyn EventSink, agent_id: &str, event: Value, seq: u64) {
    emit(
        sink,
        "agent:event",
        AgentEventPayload {
            agent_id: agent_id.to_string(),
            event,
            seq,
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct SessionRecordsAppendedPayload {
    agent_id: String,
}

/// New transcript records were ingested into `session_records`.
pub(super) fn emit_session_records_appended(sink: &dyn EventSink, agent_id: &str) {
    emit(
        sink,
        "session:records-appended",
        SessionRecordsAppendedPayload {
            agent_id: agent_id.to_string(),
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct SessionSyncHealthPayload {
    agent_id: String,
    provider: String,
    /// `"healthy"` (clears a prior degraded state), `"no_root"`, or
    /// `"format_drift"`. `NoFiles` is never emitted (log-only, ambiguous).
    status: &'static str,
    /// The current CLI version string (memoized `<bin> --version`) for the
    /// log/message only — not a historical DB lookup. `None` if unprobed.
    version: Option<String>,
}

/// The transcript-ingest health for an agent changed (drift detected, or a
/// prior drift cleared). Emitted on status *change* only — see
/// `session_sync::trigger_session_sync`.
pub(super) fn emit_session_sync_health(
    sink: &dyn EventSink,
    agent_id: &str,
    provider: &str,
    status: &'static str,
    version: Option<String>,
) {
    emit(
        sink,
        "session:sync-health",
        SessionSyncHealthPayload {
            agent_id: agent_id.to_string(),
            provider: provider.to_string(),
            status,
            version,
        },
    );
}

/// Emitted when a turn flips to Running, carrying the backend's own start
/// timestamp (the same value persisted as the turn's `started_at`). The live
/// timer anchors to this rather than the event's client-receipt time, so it
/// shares the footer's clock and the two never disagree by the delivery latency.
#[derive(Clone, serde::Serialize)]
struct TurnStartedPayload {
    agent_id: String,
    started_at: i64,
}

pub(super) fn emit_turn_started(sink: &dyn EventSink, agent_id: &str, started_at: i64) {
    emit(
        sink,
        "turn:started",
        TurnStartedPayload {
            agent_id: agent_id.to_string(),
            started_at,
        },
    );
}

/// A user message was accepted for an agent — from any client (desktop
/// webview, a paired phone, a git-action trigger). Every connected client
/// mirrors it into its chat log, so the conversation reads the same on every
/// device rather than each one seeing only the prompts it typed itself. The
/// sender dedupes against its own optimistic bubble by `turn_id`. `follow_up`
/// says whether the agent was mid-turn when the message arrived — the same
/// fact the sending client keyed its own render on — so a mirroring client
/// draws a follow-up bubble vs. a turn-opening one without consulting its own
/// (possibly lagging) busy flag.
#[derive(Clone, serde::Serialize)]
struct TurnSentPayload {
    agent_id: String,
    turn_id: String,
    text: String,
    attachments: Vec<String>,
    follow_up: bool,
}

pub(super) fn emit_turn_sent(
    sink: &dyn EventSink,
    agent_id: &str,
    turn_id: &str,
    text: &str,
    attachments: &[String],
    follow_up: bool,
) {
    emit(
        sink,
        "turn:sent",
        TurnSentPayload {
            agent_id: agent_id.to_string(),
            turn_id: turn_id.to_string(),
            text: text.to_string(),
            attachments: attachments.to_vec(),
            follow_up,
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct AgentStatusPayload {
    agent_id: String,
    status: AgentStatus,
    last_error: Option<String>,
}

/// Runtime status transition (Spawning/Running/Idle/Error).
pub(super) fn emit_status(
    sink: &dyn EventSink,
    agent_id: &str,
    status: AgentStatus,
    last_error: Option<String>,
) {
    emit(
        sink,
        "agent:status",
        AgentStatusPayload {
            agent_id: agent_id.to_string(),
            status,
            last_error,
        },
    );
}

/// Which step of a fresh spawn is running right now. Progress only — the
/// authoritative state stays `agent:status`, and a client that misses these
/// still sees `spawning` followed by `idle`/`error`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SpawnStage {
    /// Creating the agent's workspace directory.
    Preparing,
    /// Cloning the source repo and detaching at the base commit.
    Cloning,
    /// Warming the codegraph index for the new checkout.
    Indexing,
    /// Checking out the project's other repos.
    AttachingRepos,
    /// Starting the checkouts from a fork's code (`Supervisor::start_from`).
    Carrying,
    /// Condensing the conversation the session continues into its handoff
    /// context (`crate::handoff`).
    Summarizing,
    /// Launching the agent process.
    Starting,
}

#[derive(Clone, serde::Serialize)]
struct SpawnProgressPayload {
    agent_id: String,
    stage: SpawnStage,
    detail: Option<String>,
}

/// One stage boundary of a fresh spawn, so a client can say what the 15-odd
/// seconds behind "spawning" are actually being spent on. Never emitted after
/// the spawn's terminal status: every caller sits ahead of the work it names,
/// and re-checks `Supervisor::spawn_still_live` for its own attempt before
/// emitting, so a spawn whose outcome was already claimed (timeout) stops
/// instead of narrating — and stays stopped if the agent is resumed under a
/// later attempt.
pub(super) fn emit_spawn_progress(
    sink: &dyn EventSink,
    agent_id: &str,
    stage: SpawnStage,
    detail: Option<String>,
) {
    emit(
        sink,
        "agent:spawn-progress",
        SpawnProgressPayload {
            agent_id: agent_id.to_string(),
            stage,
            detail,
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct AgentViewPayload {
    agent_id: String,
    view: AgentView,
}

pub(super) fn emit_view(sink: &dyn EventSink, agent_id: &str, view: AgentView) {
    emit(
        sink,
        "agent:view",
        AgentViewPayload {
            agent_id: agent_id.to_string(),
            view,
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct AgentEffortPayload {
    agent_id: String,
    effort: Option<String>,
}

/// The session's reasoning-effort level changed mid-conversation
/// (user-initiated). Mirrors `agent:view` so the composer reflects the new
/// value without a full resync.
pub(super) fn emit_effort(sink: &dyn EventSink, agent_id: &str, effort: Option<&str>) {
    emit(
        sink,
        "agent:effort",
        AgentEffortPayload {
            agent_id: agent_id.to_string(),
            effort: effort.map(str::to_string),
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct AgentFolderGrantsPayload {
    agent_id: String,
    #[serde(flatten)]
    update: crate::workspace::folder_grants::FolderGrantsUpdate,
}

/// The folders granted to a workspace's agent changed (`Supervisor::
/// grant_folder` / `revoke_folder`): the whole list, so every client's
/// composer shows the same grants without a resync, and whether the change
/// still waits for the running turn to end (so each can offer "apply now").
pub(super) fn emit_folder_grants(
    sink: &dyn EventSink,
    agent_id: &str,
    update: &crate::workspace::folder_grants::FolderGrantsUpdate,
) {
    emit(
        sink,
        "agent:folder-grants",
        AgentFolderGrantsPayload {
            agent_id: agent_id.to_string(),
            update: update.clone(),
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct AgentModelPayload {
    agent_id: String,
    model: Option<String>,
}

/// The session's model changed mid-conversation (user-initiated). Mirrors
/// `agent:effort` so the composer reflects the new value without a full resync.
pub(super) fn emit_model(sink: &dyn EventSink, agent_id: &str, model: Option<&str>) {
    emit(
        sink,
        "agent:model",
        AgentModelPayload {
            agent_id: agent_id.to_string(),
            model: model.map(str::to_string),
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct AgentTaskPayload {
    agent_id: String,
    task: String,
}

/// The agent's task (first user message) was captured.
pub(super) fn emit_task(sink: &dyn EventSink, agent_id: &str, task: String) {
    emit(
        sink,
        "agent:task",
        AgentTaskPayload {
            agent_id: agent_id.to_string(),
            task,
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct AgentTitlePayload {
    agent_id: String,
    title: String,
}

/// The agent titled its work via the `set_title` mailbox op. Mirrors
/// `agent:task`; the sidebar subtitle prefers this over the task's first line.
pub(super) fn emit_title(sink: &dyn EventSink, agent_id: &str, title: String) {
    emit(
        sink,
        "agent:title",
        AgentTitlePayload {
            agent_id: agent_id.to_string(),
            title,
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct TourChangedPayload {
    agent_id: String,
}

/// The agent wrote its walkthrough via the `tour_set` mailbox op. Carries no
/// steps: the Code panel reloads them with `get_tour`.
pub(super) fn emit_tour_changed(sink: &dyn EventSink, agent_id: &str) {
    emit(
        sink,
        "tour:changed",
        TourChangedPayload {
            agent_id: agent_id.to_string(),
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct AgentNotifyPayload {
    agent_id: String,
    title: String,
    body: String,
}

/// The agent asked for the user's attention via the `notify` mailbox op. The
/// desktop shows it as a native notification under its own mute; nothing is
/// stored, and the turn's reply still carries the substance. Not forwarded to
/// remote clients: the phone has no handler for it and the push path
/// synthesizes no alert, so forwarding it would only be discarded there.
pub(super) fn emit_notify(sink: &dyn EventSink, agent_id: &str, title: String, body: String) {
    emit(
        sink,
        "agent:notify",
        AgentNotifyPayload {
            agent_id: agent_id.to_string(),
            title,
            body,
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct AgentBranchPayload {
    agent_id: String,
    subdir: String,
    branch: String,
}

/// A repo's branch was materialized (first push / PR open).
pub(super) fn emit_branch(sink: &dyn EventSink, agent_id: &str, subdir: &str, branch: &str) {
    emit(
        sink,
        "agent:branch",
        AgentBranchPayload {
            agent_id: agent_id.to_string(),
            subdir: subdir.to_string(),
            branch: branch.to_string(),
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct AgentRepoAddedPayload {
    agent_id: String,
    repo: TrackedRepo,
}

pub(super) fn emit_repo_added(sink: &dyn EventSink, agent_id: &str, repo: TrackedRepo) {
    emit(
        sink,
        "agent:repo_added",
        AgentRepoAddedPayload {
            agent_id: agent_id.to_string(),
            repo,
        },
    );
}

/// A successful, mutating git RPC op (`op`) the agent ran this turn — the
/// causal signal the delegation panel uses to confirm the agent did the work.
#[derive(Clone, serde::Serialize)]
struct AgentGitActionPayload {
    agent_id: String,
    op: String,
}

pub(super) fn emit_git_action(sink: &dyn EventSink, agent_id: &str, op: String) {
    emit(
        sink,
        "agent:git-action",
        AgentGitActionPayload {
            agent_id: agent_id.to_string(),
            op,
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct ShellOutputPayload {
    agent_id: String,
    #[serde(serialize_with = "serialize_bytes_b64")]
    bytes: Vec<u8>,
}

/// Raw bytes from the agent's interactive shell PTY.
pub(super) fn emit_shell_output(sink: &dyn EventSink, agent_id: &str, bytes: Vec<u8>) {
    emit(
        sink,
        "shell:output",
        ShellOutputPayload {
            agent_id: agent_id.to_string(),
            bytes,
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct PrStateChangedPayload {
    agent_id: String,
    /// `None` for the agent's primary repo, the subdir for a secondary — as on
    /// `pr:checks_changed`. Added after the fact, so a client that predates it
    /// reads every event as the primary's.
    subdir: Option<String>,
    state: Option<PrState>,
}

/// The focused (bound) PR of one checkout, as found now — a state, not a
/// transition. Only ever the focused PR: a client that predates PR sets reads
/// every one as its checkout's PR, so the rest of the set goes out as
/// [`emit_pr_set_entry`] instead.
pub(super) fn emit_pr_state(
    sink: &dyn EventSink,
    agent_id: &str,
    subdir: Option<&str>,
    state: Option<PrState>,
) {
    emit(
        sink,
        "pr:state_changed",
        PrStateChangedPayload {
            agent_id: agent_id.to_string(),
            subdir: subdir.map(str::to_string),
            state,
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct PrSetEntryChangedPayload {
    agent_id: String,
    /// As on `pr:state_changed`: `None` for the primary repo.
    subdir: Option<String>,
    entry: super::session_sync::PrSetEntry,
}

/// A PR of a checkout's set that is *not* its focused PR changed state or
/// checks (`supervisor::pr_watch`). Its own event, never one of the legacy
/// three: a client that predates PR sets ignores an event name it does not
/// know, where it would have read a `pr:state_changed` as its focused PR's.
/// `checks: None` is "nothing to say this round", as in `get_all_pr_status`.
pub(super) fn emit_pr_set_entry(
    sink: &dyn EventSink,
    agent_id: &str,
    subdir: Option<&str>,
    state: PrState,
    checks: Option<PrChecks>,
) {
    emit(
        sink,
        "pr:set_entry_changed",
        PrSetEntryChangedPayload {
            agent_id: agent_id.to_string(),
            subdir: subdir.map(str::to_string),
            entry: super::session_sync::PrSetEntry {
                state,
                checks,
                threads: None,
            },
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct PrChecksChangedPayload {
    agent_id: String,
    /// `None` for the agent's primary repo, the subdir for a secondary — the
    /// same addressing the per-repo PR reads take.
    subdir: Option<String>,
    /// Which PR these checks belong to, so a listener that remembers the last
    /// rollup per checkout can tell a new PR on the same branch from the old.
    number: u32,
    checks: PrChecks,
}

/// The host-side PR watcher saw the focused open PR's CI rollup, or its set of
/// failing checks, change (`supervisor::pr_watch`).
pub(super) fn emit_pr_checks(
    sink: &dyn EventSink,
    agent_id: &str,
    subdir: Option<&str>,
    number: u32,
    checks: PrChecks,
) {
    emit(
        sink,
        "pr:checks_changed",
        PrChecksChangedPayload {
            agent_id: agent_id.to_string(),
            subdir: subdir.map(str::to_string),
            number,
            checks,
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct PrThreadsChangedPayload {
    agent_id: String,
    subdir: Option<String>,
    comments: PrComments,
    /// The unresolved thread ids the watcher had not seen before this read.
    new_thread_ids: Vec<String>,
}

/// The host-side PR watcher saw the unresolved review threads of the focused
/// open PR change (the only PR whose threads it reads). Carries the whole unresolved set so a client replaces its copy, plus
/// the ids that are new so it can announce exactly those.
pub(crate) fn emit_pr_threads(
    sink: &dyn EventSink,
    agent_id: &str,
    subdir: Option<&str>,
    comments: PrComments,
    new_thread_ids: Vec<String>,
) {
    emit(
        sink,
        "pr:threads_changed",
        PrThreadsChangedPayload {
            agent_id: agent_id.to_string(),
            subdir: subdir.map(str::to_string),
            comments,
            new_thread_ids,
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct RunOutputPayload {
    agent_id: String,
    bytes: Vec<u8>,
    /// Absolute end offset of this chunk (total bytes appended to the run log
    /// including it). The panel dedupes against `RunStateSnapshot::log_seq`:
    /// a chunk with `seq <= log_seq` is already in the snapshot.
    seq: u64,
}

/// Raw bytes from the Run panel's PTY (setup or dev-server phase). `seq` is the
/// running byte offset returned by `RunSession::append_log`.
pub(super) fn emit_run_output(sink: &dyn EventSink, agent_id: &str, bytes: Vec<u8>, seq: u64) {
    emit(
        sink,
        "run:output",
        RunOutputPayload {
            agent_id: agent_id.to_string(),
            bytes,
            seq,
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct RunStatePayload {
    agent_id: String,
    phase: RunPhase,
    last_error: Option<String>,
    /// As on `RunStateSnapshot`. `None` until the port is settled: a box's
    /// run is `running` before its server listens and the box forwards its
    /// port, and the `run:state` that follows the forward carries it.
    port: Option<u16>,
    guest_port: Option<u16>,
    /// As on `RunStateSnapshot`: what the box forwards, a run or no run.
    ports: Vec<ForwardedPort>,
    /// As on `RunStateSnapshot`: which state this is, so a client drops an
    /// event or a snapshot that describes an older state than one it has.
    revision: u64,
}

/// The Run panel's state, read from `session` rather than passed in, so the
/// event says what `run_state` would. Callers apply the change first.
pub(super) fn emit_run_state(sink: &dyn EventSink, agent_id: &str, session: &RunSession) {
    let RunState {
        phase,
        last_error,
        port,
        guest_port,
        ports,
        revision,
    } = session.state();
    emit(
        sink,
        "run:state",
        RunStatePayload {
            agent_id: agent_id.to_string(),
            phase,
            last_error,
            port,
            guest_port,
            ports,
            revision,
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct RunPortPayload {
    agent_id: String,
    port: u16,
    guest_port: Option<u16>,
}

/// The port on this host that reaches the dev server, emitted as the Run
/// panel's dev phase spawns, or for a box's as the box forwards the port it
/// binds in the guest: `port`, what the frontend's `localhost:<port>`
/// link and sidebar indicator show, as the field has meant since the event
/// was added (a client from before boxes reads nothing else). It may differ
/// from the configured port when port-safety bumped it to the next free one,
/// or when the forward from a box's guest had to take another. `guest_port`
/// is additive: the port a server in a box's guest binds there, which the
/// forward to `port` reaches; `None` for a server on the host, where the two
/// are one.
pub(super) fn emit_run_port(
    sink: &dyn EventSink,
    agent_id: &str,
    port: u16,
    guest_port: Option<u16>,
) {
    emit(
        sink,
        "run:port",
        RunPortPayload {
            agent_id: agent_id.to_string(),
            port,
            guest_port,
        },
    );
}

/// Structural workspace change (archive/restore, a project pinned or cloned) —
/// the frontend reloads the whole workspace on this signal rather than patching
/// from finer events.
pub fn emit_workspace_changed(sink: &dyn EventSink) {
    emit(sink, "workspace:changed", ());
}

#[derive(Clone, serde::Serialize)]
struct VerificationReportPayload {
    agent_id: String,
    report: crate::verify::VerificationReport,
}

/// A turn-end verification (opt-in per project) finished for an ad-hoc agent —
/// the phone renders a tests verdict from this report. Fire-and-forget from
/// `trigger_turn_end_verification`; the phone stores the latest per agent.
pub(crate) fn emit_verification(
    sink: &dyn EventSink,
    agent_id: &str,
    report: crate::verify::VerificationReport,
) {
    emit(
        sink,
        "verify:report",
        VerificationReportPayload {
            agent_id: agent_id.to_string(),
            report,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::sink::RecordingSink;
    use serde_json::json;

    /// These payloads are the frontend's contract, so they are asserted against
    /// literals rather than against the payload structs that produced them.
    #[test]
    fn a_status_event_carries_its_name_and_payload() {
        let sink = RecordingSink::new();
        emit_status(&sink, "fuji", AgentStatus::Idle, Some("boom".to_string()));
        assert_eq!(
            sink.events(),
            vec![(
                "agent:status".to_string(),
                json!({ "agent_id": "fuji", "status": "idle", "last_error": "boom" })
            )]
        );
    }

    /// Same contract, and the stage strings in particular are what the clients
    /// switch their label on.
    #[test]
    fn a_spawn_progress_event_carries_its_stage() {
        let sink = RecordingSink::new();
        emit_spawn_progress(&sink, "fuji", SpawnStage::Cloning, None);
        emit_spawn_progress(
            &sink,
            "fuji",
            SpawnStage::AttachingRepos,
            Some("docs".to_string()),
        );
        assert_eq!(
            sink.events(),
            vec![
                (
                    "agent:spawn-progress".to_string(),
                    json!({ "agent_id": "fuji", "stage": "cloning", "detail": null })
                ),
                (
                    "agent:spawn-progress".to_string(),
                    json!({ "agent_id": "fuji", "stage": "attaching_repos", "detail": "docs" })
                ),
            ]
        );
    }

    /// PTY bytes ride as base64 (`serialize_bytes_b64`), not as a number array.
    /// Serializing the payload to a `Value` before the sink sees it is what
    /// keeps that custom serializer in the path.
    #[test]
    fn pty_output_stays_base64() {
        let sink = RecordingSink::new();
        emit_agent_output(&sink, "fuji", b"hi".to_vec());
        assert_eq!(
            sink.events(),
            vec![(
                "agent:output".to_string(),
                json!({ "agent_id": "fuji", "bytes": "aGk=" })
            )]
        );
    }

    /// `run:state` carries the session's ports as they stand: the host's run
    /// has its port before it goes `running`, a box's does not until the box
    /// forwards the port its server binds in the guest, and a stop clears
    /// the run's while the box's forwards stay.
    #[test]
    fn a_run_state_event_carries_the_sessions_ports() {
        let sink = RecordingSink::new();
        let session = RunSession::new();
        let gen = session
            .begin_phase(session.generation(), RunPhase::Running)
            .unwrap();
        session.set_ports(gen, Some(3001), None);
        emit_run_state(&sink, "fuji", &session);
        session.stop();
        let gen = session
            .begin_phase(session.generation(), RunPhase::Running)
            .unwrap();
        session.set_ports(gen, None, Some(3000));
        emit_run_state(&sink, "fuji", &session);
        session.set_forwarded(&[ForwardedPort {
            guest_port: 3000,
            port: 3002,
        }]);
        emit_run_state(&sink, "fuji", &session);
        session.mark_stopped(gen, Some("Run exited: 1".to_string()));
        emit_run_state(&sink, "fuji", &session);
        let forwarded = json!([{ "guest_port": 3000, "port": 3002 }]);
        let state = |phase: &str, last_error: Value, port: Value, guest_port: Value, ports| {
            (
                "run:state".to_string(),
                json!({
                    "agent_id": "fuji",
                    "phase": phase,
                    "last_error": last_error,
                    "port": port,
                    "guest_port": guest_port,
                    "ports": ports,
                }),
            )
        };
        // The revision rides every event and moves on with each change, so a
        // client orders them whatever order they arrive in.
        let mut revisions = Vec::new();
        let events: Vec<(String, Value)> = sink
            .events()
            .into_iter()
            .map(|(name, mut payload)| {
                let revision = payload
                    .as_object_mut()
                    .and_then(|o| o.remove("revision"))
                    .and_then(|r| r.as_u64())
                    .expect("every run:state carries a revision");
                revisions.push(revision);
                (name, payload)
            })
            .collect();
        assert_eq!(
            events,
            vec![
                state("running", Value::Null, json!(3001), Value::Null, json!([])),
                state("running", Value::Null, Value::Null, json!(3000), json!([])),
                state(
                    "running",
                    Value::Null,
                    json!(3002),
                    json!(3000),
                    forwarded.clone()
                ),
                state(
                    "stopped",
                    json!("Run exited: 1"),
                    Value::Null,
                    Value::Null,
                    forwarded
                ),
            ]
        );
        assert!(
            revisions.windows(2).all(|w| w[0] < w[1]),
            "revisions must strictly increase: {revisions:?}"
        );
    }
}
