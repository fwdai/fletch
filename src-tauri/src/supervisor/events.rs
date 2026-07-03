//! Tauri event payload types and the small emit helpers shared by the
//! supervisor's modules.

use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::run_session::RunPhase;
use crate::workspace::{AgentStatus, AgentView, TrackedRepo};

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
pub struct AgentOutputPayload {
    pub agent_id: String,
    #[serde(serialize_with = "serialize_bytes_b64")]
    pub bytes: Vec<u8>,
}

#[derive(Clone, serde::Serialize)]
pub struct AgentEventPayload {
    pub agent_id: String,
    pub event: Value,
}

#[derive(Clone, serde::Serialize)]
pub struct SessionRecordsAppendedPayload {
    pub agent_id: String,
}

/// Emitted when a turn flips to Running, carrying the backend's own start
/// timestamp (the same value persisted as the turn's `started_at`). The live
/// timer anchors to this rather than the event's client-receipt time, so it
/// shares the footer's clock and the two never disagree by the delivery latency.
#[derive(Clone, serde::Serialize)]
pub struct TurnStartedPayload {
    pub agent_id: String,
    pub started_at: i64,
}

#[derive(Clone, serde::Serialize)]
pub struct AgentStatusPayload {
    pub agent_id: String,
    pub status: AgentStatus,
    pub last_error: Option<String>,
}

#[derive(Clone, serde::Serialize)]
pub struct AgentViewPayload {
    pub agent_id: String,
    pub view: AgentView,
}

#[derive(Clone, serde::Serialize)]
pub struct AgentTaskPayload {
    pub agent_id: String,
    pub task: String,
}

#[derive(Clone, serde::Serialize)]
pub struct AgentBranchPayload {
    pub agent_id: String,
    pub subdir: String,
    pub branch: String,
}

#[derive(Clone, serde::Serialize)]
pub struct AgentRepoAddedPayload {
    pub agent_id: String,
    pub repo: TrackedRepo,
}

/// A successful, mutating git RPC op (`op`) the agent ran this turn — the
/// causal signal the delegation panel uses to confirm the agent did the work.
#[derive(Clone, serde::Serialize)]
pub struct AgentGitActionPayload {
    pub agent_id: String,
    pub op: String,
}

#[derive(Clone, serde::Serialize)]
pub struct ShellOutputPayload {
    pub agent_id: String,
    #[serde(serialize_with = "serialize_bytes_b64")]
    pub bytes: Vec<u8>,
}

#[derive(Clone, serde::Serialize)]
pub struct PrStateChangedPayload {
    pub agent_id: String,
    pub state: Option<crate::gh::PrState>,
}

#[derive(Clone, serde::Serialize)]
pub struct RunOutputPayload {
    pub agent_id: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, serde::Serialize)]
pub struct RunStatePayload {
    pub agent_id: String,
    pub phase: RunPhase,
    pub last_error: Option<String>,
}

pub(super) fn emit_status(
    app: &AppHandle,
    agent_id: &str,
    status: AgentStatus,
    last_error: Option<String>,
) {
    let _ = app.emit(
        "agent:status",
        AgentStatusPayload {
            agent_id: agent_id.to_string(),
            status,
            last_error,
        },
    );
}

pub(super) fn emit_run_state(
    app: &AppHandle,
    agent_id: &str,
    phase: RunPhase,
    last_error: Option<String>,
) {
    let _ = app.emit(
        "run:state",
        RunStatePayload {
            agent_id: agent_id.to_string(),
            phase,
            last_error,
        },
    );
}

pub(super) fn emit_run_output(app: &AppHandle, agent_id: &str, bytes: Vec<u8>) {
    let _ = app.emit(
        "run:output",
        RunOutputPayload {
            agent_id: agent_id.to_string(),
            bytes,
        },
    );
}
