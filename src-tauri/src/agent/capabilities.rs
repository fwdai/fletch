//! Agent capabilities, injection-mode dispatch, and the per-turn descriptor
//! table that drives every per-turn provider.

use std::path::Path;

use serde_json::Value;

use crate::activity::{Activity, ManagedActivity};
use crate::message_queue::InjectionMode;

use super::providers::antigravity::{
    antigravity_build_args, antigravity_locate, antigravity_pty_args, antigravity_read,
    antigravity_session_id_from_cwd,
};
use super::providers::claude::CLAUDE_TRANSCRIPT;
use super::providers::codex::{
    codex_build_args, codex_locate, codex_pty_args, codex_read, codex_session_id,
};
use super::providers::cursor::{
    cursor_build_args, cursor_locate, cursor_pty_args, cursor_read, cursor_session_id,
};
use super::providers::opencode::{
    opencode_build_args, opencode_locate, opencode_pty_args, opencode_read, opencode_session_id,
};
use super::providers::pi::{pi_build_args, pi_locate, pi_pty_args, pi_read, pi_session_id};
use super::transcript::{JsonlTail, TranscriptReader};
use super::{McpArgsBuilder, PtyArgsBuilder, TurnArgs};

/// Everything that varies between per-turn agents. The runner lifecycle —
/// one fresh process per turn via `ExecSession` — is identical for all of
/// them; only the binary, CLI args, session-id extraction, and turn-end
/// detector differ. Capturing those as a table entry means a new
/// per-turn agent is one `PER_TURN_AGENTS` row, with no new `spawn_*`
/// method, `resolve_*` helper, or `match provider` arm anywhere.
pub struct PerTurnDescriptor {
    /// Provider id (matches the frontend adapter / `AgentRecord.provider`).
    pub id: &'static str,
    /// Executable name resolved via `resolve_agent_bin`.
    pub(crate) bin: &'static str,
    /// Human-facing product name, used only in the not-found error.
    pub(crate) label: &'static str,
    /// Builds the CLI args for one turn from the turn's [`TurnArgs`].
    pub(crate) build_args: fn(&TurnArgs) -> Vec<String>,
    /// Builds the args to launch this agent's interactive TUI in the native
    /// (PTY) view: `(session [None = fresh], model, custom_instructions,
    /// mcp_args)`.
    pub(crate) pty_args: PtyArgsBuilder,
    /// Builds this provider's MCP-delivery args from the session's snapshot
    /// (e.g. codex `-c mcp_servers.*` overrides), applied to both the per-turn
    /// and native-PTY launches. `None` = provider has no MCP config surface we
    /// can drive; the snapshot is ignored (the editor UI says so up front).
    pub(crate) mcp_args: Option<McpArgsBuilder>,
    /// Extracts the agent-assigned session id from a turn's events. The
    /// event-based agents are thin wrappers over `gated_session_id` (same shape,
    /// different gates). No-op for `plaintext` agents (they emit no events — see
    /// `session_id_from_cwd`).
    pub(crate) session_id: fn(&Value) -> Option<String>,
    /// Constructs this agent's turn-end detector (custom-view `Activity`).
    pub activity: fn() -> Box<dyn Activity>,
    /// Whether this agent can render in the native PTY view (see
    /// `AgentCapabilities::native_view`).
    native_view: bool,
    /// True if the turn process emits **plaintext** on stdout rather than a
    /// newline-delimited JSON event stream. The runner then drains stdout
    /// without parsing (no events; history comes from `transcript`), and the
    /// session id is captured via `session_id_from_cwd` instead of events.
    pub(crate) plaintext: bool,
    /// For agents whose session id isn't in their event stream (e.g. agy), read
    /// it from the filesystem at turn-end given the checkout cwd. `None` =
    /// session id comes from events via `session_id`.
    pub session_id_from_cwd: Option<fn(&Path) -> Option<String>>,
    /// Reader for this agent's on-disk transcript, used by `sync_session` to
    /// ingest verbatim records into `session_records`. `None` = no readable
    /// transcript.
    pub transcript: Option<TranscriptReader>,
}

impl PerTurnDescriptor {
    /// Human-facing product name, for error copy (e.g. the docker-sandbox
    /// refusal in `supervisor::lifecycle`).
    pub fn label(&self) -> &'static str {
        self.label
    }
}

/// What an agent can do *right now*. A rollout flag, not a fixed trait: native
/// (PTY/TUI) view support is being brought to every agent, and callers gate on
/// the capability, never on the provider id, so nothing else changes when
/// support lands.
pub struct AgentCapabilities {
    /// Can render in the native PTY view (its interactive TUI streamed into
    /// xterm), in addition to the structured custom view. Wired for claude
    /// and every per-turn agent (codex/cursor/opencode/pi); a per-turn agent
    /// can only switch *into* native once it has a session id to resume.
    pub native_view: bool,
}

/// Capabilities for a provider. Per-turn agents read theirs from the
/// descriptor table; claude (the lone persistent-runner agent) is the
/// fully-wired baseline. Unknown providers get nothing.
pub fn capabilities(provider: &str) -> AgentCapabilities {
    match per_turn_descriptor(provider) {
        Some(d) => AgentCapabilities {
            native_view: d.native_view,
        },
        None if provider == "claude" => AgentCapabilities { native_view: true },
        None => AgentCapabilities { native_view: false },
    }
}

/// How a provider accepts a follow-up message sent mid-turn. Claude (the lone
/// persistent-runner agent) keeps an open stream-json stdin, so it can take a
/// message live; per-turn agents are one-shot processes with no live stdin, so
/// they queue until the next turn boundary. Unknown providers default to the
/// safe boundary path.
pub fn injection_mode(provider: &str) -> InjectionMode {
    match per_turn_descriptor(provider) {
        Some(_) => InjectionMode::AtTurnBoundary,
        None if provider == "claude" => InjectionMode::Live,
        None => InjectionMode::AtTurnBoundary,
    }
}

pub(crate) const PER_TURN_AGENTS: &[PerTurnDescriptor] = &[
    PerTurnDescriptor {
        id: "codex",
        bin: "codex",
        label: "Codex",
        build_args: codex_build_args,
        pty_args: codex_pty_args,
        mcp_args: Some(crate::agent_profile::codex_mcp_args),
        session_id: codex_session_id,
        activity: || Box::new(ManagedActivity::codex()),
        native_view: true,
        plaintext: false,
        session_id_from_cwd: None,
        transcript: Some(TranscriptReader {
            locate: codex_locate,
            read: codex_read,
            tail: None, // multiple rollout files
        }),
    },
    PerTurnDescriptor {
        id: "cursor",
        bin: "cursor-agent",
        label: "Cursor",
        build_args: cursor_build_args,
        pty_args: cursor_pty_args,
        mcp_args: None,
        session_id: cursor_session_id,
        // Cursor emits Claude-shaped stream-json incl. a `result` turn-end,
        // so it reuses the Claude managed detector.
        activity: || Box::new(ManagedActivity::claude()),
        native_view: true,
        plaintext: false,
        session_id_from_cwd: None,
        transcript: Some(TranscriptReader {
            locate: cursor_locate,
            read: cursor_read,
            tail: Some(JsonlTail { id_field: None }), // single jsonl, positional ids
        }),
    },
    PerTurnDescriptor {
        id: "opencode",
        bin: "opencode",
        label: "OpenCode",
        build_args: opencode_build_args,
        pty_args: opencode_pty_args,
        mcp_args: None,
        session_id: opencode_session_id,
        activity: || Box::new(ManagedActivity::opencode()),
        native_view: true,
        plaintext: false,
        session_id_from_cwd: None,
        transcript: Some(TranscriptReader {
            locate: opencode_locate,
            read: opencode_read,
            tail: None, // blob-store directory, not a single file
        }),
    },
    PerTurnDescriptor {
        id: "pi",
        bin: "pi",
        label: "Pi",
        build_args: pi_build_args,
        pty_args: pi_pty_args,
        mcp_args: None,
        session_id: pi_session_id,
        activity: || Box::new(ManagedActivity::pi()),
        native_view: true,
        plaintext: false,
        session_id_from_cwd: None,
        // Pi is the reference reader — its per-session JSONL feeds session_records.
        transcript: Some(TranscriptReader {
            locate: pi_locate,
            read: pi_read,
            tail: Some(JsonlTail {
                id_field: Some("id"),
            }), // single jsonl when one file
        }),
    },
    PerTurnDescriptor {
        id: "antigravity",
        bin: "agy",
        label: "Antigravity",
        build_args: antigravity_build_args,
        pty_args: antigravity_pty_args,
        mcp_args: None,
        // agy emits no JSON events; its session id is read from the filesystem.
        session_id: |_| None,
        // No event stream to detect turn-end from — the turn's process exit ends
        // the turn (on_turn_exit). The detector is never fed, so any is fine.
        activity: || Box::new(ManagedActivity::claude()),
        // Native PTY view runs agy's interactive TUI, resuming the conversation
        // the custom view established (see antigravity_pty_args).
        native_view: true,
        plaintext: true,
        session_id_from_cwd: Some(antigravity_session_id_from_cwd),
        transcript: Some(TranscriptReader {
            locate: antigravity_locate,
            read: antigravity_read,
            tail: None, // per-turn agent; full read on exit is bounded
        }),
    },
];

/// Look up the descriptor for a per-turn provider id. `None` means the
/// provider isn't a per-turn agent (e.g. claude, which has its own
/// Pty/Managed runners) or isn't a known agent at all.
pub fn per_turn_descriptor(id: &str) -> Option<&'static PerTurnDescriptor> {
    PER_TURN_AGENTS.iter().find(|d| d.id == id)
}

/// The transcript reader for a provider, or `None` if it has no on-disk
/// transcript wired. Per-turn agents read theirs from the descriptor table;
/// claude (persistent runner) is special-cased here. Callers gate on this, not
/// on the provider id.
pub fn transcript_reader(provider: &str) -> Option<&'static TranscriptReader> {
    match per_turn_descriptor(provider) {
        Some(d) => d.transcript.as_ref(),
        None if provider == "claude" => Some(&CLAUDE_TRANSCRIPT),
        None => None,
    }
}

/// (binary, human label) for a provider, or `None` if unknown. Same dispatch
/// as `transcript_reader`: per-turn descriptors + the claude special case.
pub(crate) fn provider_bin_label(provider: &str) -> Option<(&'static str, &'static str)> {
    match per_turn_descriptor(provider) {
        Some(d) => Some((d.bin, d.label)),
        None if provider == "claude" => Some(("claude", "Claude Code")),
        None => None,
    }
}
