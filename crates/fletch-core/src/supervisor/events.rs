//! Every event the supervisor emits, in one place: the payload types and a
//! typed emit fn per event, each taking the [`EventSink`] the event goes out
//! on (the desktop's is the `TauriSink`). Emit failures (serialization —
//! effectively never) are logged, not surfaced; no event is delivery-guaranteed
//! and the frontend resyncs on focus rather than trusting delivery.

use serde_json::Value;

use crate::github::PrState;
use crate::host::EventSink;
// Shared with the other PTY-carrying events (see `provider_login` commands),
// so the base64 wire format is defined once next to the sessions producing it.
use crate::pty_session::serialize_bytes_b64;
use crate::run_session::RunPhase;
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
    /// Overlaying the source workspace's uncommitted work (fork "carry code").
    Carrying,
    /// Checking out the project's other repos.
    AttachingRepos,
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
/// the spawn's terminal status — every caller sits ahead of the work it names.
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
    state: Option<PrState>,
}

pub(super) fn emit_pr_state(sink: &dyn EventSink, agent_id: &str, state: Option<PrState>) {
    emit(
        sink,
        "pr:state_changed",
        PrStateChangedPayload {
            agent_id: agent_id.to_string(),
            state,
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
}

pub(super) fn emit_run_state(
    sink: &dyn EventSink,
    agent_id: &str,
    phase: RunPhase,
    last_error: Option<String>,
) {
    emit(
        sink,
        "run:state",
        RunStatePayload {
            agent_id: agent_id.to_string(),
            phase,
            last_error,
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct RunPortPayload {
    agent_id: String,
    port: u16,
}

/// The port the dev server is actually being launched on — emitted just before
/// the Run panel's dev phase spawns. May differ from the configured port when
/// port-safety bumped it to the next free one; the frontend uses this to render
/// the correct `localhost:<port>` link and sidebar indicator.
pub(super) fn emit_run_port(sink: &dyn EventSink, agent_id: &str, port: u16) {
    emit(
        sink,
        "run:port",
        RunPortPayload {
            agent_id: agent_id.to_string(),
            port,
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
/// its Mission Control card renders a tests chip from this report. Fire-and-
/// forget from `trigger_turn_end_verification`; the frontend stores the latest
/// per agent.
pub(super) fn emit_verification(
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
}
