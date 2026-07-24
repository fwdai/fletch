//! Every Tauri event the supervisor emits, in one place: the payload types
//! and a typed emit fn per event. Emit failures (serialization — effectively
//! never) are logged, not surfaced; no event is delivery-guaranteed and the
//! frontend resyncs on focus rather than trusting delivery.

use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::github::PrState;
use crate::run_session::RunPhase;
use crate::workspace::{AgentStatus, AgentView, TrackedRepo};

/// Emit one event, logging (not propagating) failure — the shared tail of
/// every typed emitter below.
fn emit<T: serde::Serialize + Clone>(app: &AppHandle, event: &str, payload: T) {
    if let Err(e) = app.emit(event, payload) {
        tracing::warn!(error = %e, event, "emit failed");
    }
}

/// Serialize raw PTY bytes as a base64 string rather than serde's default
/// JSON number array (`[27,91,...]`), which inflates the payload ~3.5×. The
/// frontend base64-decodes back to the identical byte stream.
fn serialize_bytes_b64<S: serde::Serializer>(
    bytes: &[u8],
    s: S,
) -> std::result::Result<S::Ok, S::Error> {
    use base64::Engine;
    s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
}

#[derive(Clone, serde::Serialize)]
struct AgentOutputPayload {
    agent_id: String,
    #[serde(serialize_with = "serialize_bytes_b64")]
    bytes: Vec<u8>,
}

/// Raw PTY bytes from an agent's process (native view).
pub(super) fn emit_agent_output(app: &AppHandle, agent_id: &str, bytes: Vec<u8>) {
    emit(
        app,
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
}

/// One parsed JSON event from a managed/per-turn agent's stream.
pub(super) fn emit_agent_event(app: &AppHandle, agent_id: &str, event: Value) {
    emit(
        app,
        "agent:event",
        AgentEventPayload {
            agent_id: agent_id.to_string(),
            event,
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct SessionRecordsAppendedPayload {
    agent_id: String,
}

/// New transcript records were ingested into `session_records`.
pub(super) fn emit_session_records_appended(app: &AppHandle, agent_id: &str) {
    emit(
        app,
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
    app: &AppHandle,
    agent_id: &str,
    provider: &str,
    status: &'static str,
    version: Option<String>,
) {
    emit(
        app,
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

pub(super) fn emit_turn_started(app: &AppHandle, agent_id: &str, started_at: i64) {
    emit(
        app,
        "turn:started",
        TurnStartedPayload {
            agent_id: agent_id.to_string(),
            started_at,
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
    app: &AppHandle,
    agent_id: &str,
    status: AgentStatus,
    last_error: Option<String>,
) {
    emit(
        app,
        "agent:status",
        AgentStatusPayload {
            agent_id: agent_id.to_string(),
            status,
            last_error,
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct AgentViewPayload {
    agent_id: String,
    view: AgentView,
}

pub(super) fn emit_view(app: &AppHandle, agent_id: &str, view: AgentView) {
    emit(
        app,
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
pub(super) fn emit_effort(app: &AppHandle, agent_id: &str, effort: Option<&str>) {
    emit(
        app,
        "agent:effort",
        AgentEffortPayload {
            agent_id: agent_id.to_string(),
            effort: effort.map(str::to_string),
        },
    );
}

#[derive(Clone, serde::Serialize)]
struct AgentTaskPayload {
    agent_id: String,
    task: String,
}

/// The agent's task (first user message) was captured.
pub(super) fn emit_task(app: &AppHandle, agent_id: &str, task: String) {
    emit(
        app,
        "agent:task",
        AgentTaskPayload {
            agent_id: agent_id.to_string(),
            task,
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
pub(super) fn emit_branch(app: &AppHandle, agent_id: &str, subdir: &str, branch: &str) {
    emit(
        app,
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

pub(super) fn emit_repo_added(app: &AppHandle, agent_id: &str, repo: TrackedRepo) {
    emit(
        app,
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

pub(super) fn emit_git_action(app: &AppHandle, agent_id: &str, op: String) {
    emit(
        app,
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
pub(super) fn emit_shell_output(app: &AppHandle, agent_id: &str, bytes: Vec<u8>) {
    emit(
        app,
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

pub(super) fn emit_pr_state(app: &AppHandle, agent_id: &str, state: Option<PrState>) {
    emit(
        app,
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
pub(super) fn emit_run_output(app: &AppHandle, agent_id: &str, bytes: Vec<u8>, seq: u64) {
    emit(
        app,
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
    app: &AppHandle,
    agent_id: &str,
    phase: RunPhase,
    last_error: Option<String>,
) {
    emit(
        app,
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
pub(super) fn emit_run_port(app: &AppHandle, agent_id: &str, port: u16) {
    emit(
        app,
        "run:port",
        RunPortPayload {
            agent_id: agent_id.to_string(),
            port,
        },
    );
}

/// Structural workspace change (archive/restore) — the frontend reloads the
/// whole workspace on this signal rather than patching from finer events.
pub(super) fn emit_workspace_changed(app: &AppHandle) {
    emit(app, "workspace:changed", ());
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
    app: &AppHandle,
    agent_id: &str,
    report: crate::verify::VerificationReport,
) {
    emit(
        app,
        "verify:report",
        VerificationReportPayload {
            agent_id: agent_id.to_string(),
            report,
        },
    );
}
