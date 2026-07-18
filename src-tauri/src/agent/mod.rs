//! Per-agent lifecycle.
//!
//! An agent is a git worktree + a coding-agent process running inside
//! it. There are three runner shapes:
//!
//! - **Pty** (claude native view): a sandboxed `claude` process in a PTY
//!   rendering its TUI; the app overlays its own input over the prompt.
//! - **Managed** (claude custom view): a sandboxed, persistent
//!   `claude --print` stream-json subprocess; the app renders structured
//!   chat. Both claude shapes attach to the same conversation via
//!   `--session-id <uuid>` on first spawn and `--resume <uuid>` after.
//! - **CodexManaged** (codex custom view): codex's `exec` runs one turn
//!   and exits, so there's no persistent process — each user message
//!   spawns a fresh `codex exec [resume <id>]` (see `codex_session`).
//!   Codex sandboxes itself rather than running under sandbox-exec.

mod args;
mod capabilities;
mod probe;
mod providers;
mod spawn;
mod transcript;

#[cfg(test)]
mod tests;

use crate::exec_session::ExecSession;
use crate::managed_session::ManagedSession;
use crate::pty_session::PtySession;
use crate::sandbox::Keepalive;

pub use capabilities::{
    capabilities, injection_mode, per_turn_descriptor, transcript_reader, PerTurnDescriptor,
};
pub use probe::{
    cached_provider_version, check_cli, probe_all_providers, validate_bin, BinValidation,
    ProviderProbe, ToolStatus,
};
pub(crate) use probe::parse_semver;
pub use spawn::{PerTurnSpec, SpawnSpec};
pub use transcript::{read_jsonl_tail, ReadDiagnostics};

pub enum Agent {
    Pty(PtyAgent),
    Managed(ManagedAgent),
    /// A per-turn runner (codex, cursor): holds no live process between
    /// turns; each user message spawns a fresh process. The agent
    /// sandboxes itself, so there's no sandbox-exec profile.
    PerTurn(PerTurnAgent),
}

pub struct PtyAgent {
    pty: PtySession,
    #[allow(dead_code)]
    keepalive: Keepalive,
}

pub struct ManagedAgent {
    session: ManagedSession,
    #[allow(dead_code)]
    keepalive: Keepalive,
}

pub struct PerTurnAgent {
    session: ExecSession,
}

/// The per-turn inputs a `*_build_args` builder turns into CLI argv. Bundled
/// so builders and their call sites read as named fields instead of a row of
/// positional `Option<&str>`s. `Default` (all-`None`, empty prompt) keeps test
/// call sites terse: `TurnArgs { prompt: "hi", ..Default::default() }`.
#[derive(Clone, Copy, Default)]
pub struct TurnArgs<'a> {
    /// The user's message for this turn (positional in most agents' argv).
    pub prompt: &'a str,
    /// Resume target: `None` starts a fresh session, `Some(id)` resumes it.
    pub session_id: Option<&'a str>,
    /// Reasoning-effort level, for agents that expose one.
    pub thinking: Option<&'a str>,
    /// Session-level model override; `None` keeps the provider CLI default.
    pub model: Option<&'a str>,
    /// A custom agent's standing brief, injected into the turn.
    pub extra: Option<&'a str>,
    /// Provider-specific MCP override args, prebuilt once per session by the
    /// descriptor's `mcp_args` (codex `-c mcp_servers.*`). Empty otherwise.
    pub mcp_args: &'a [String],
}

/// Builds the native (PTY) view launch args from
/// `(session [None = fresh], model, custom_instructions, mcp_args)`.
pub(crate) type PtyArgsBuilder =
    fn(Option<&str>, Option<&str>, Option<&str>, &[String]) -> Vec<String>;

/// Builds a provider's MCP-delivery args from the session's server snapshot
/// (e.g. codex `-c mcp_servers.*` overrides).
pub(crate) type McpArgsBuilder = fn(&[crate::agent_profile::McpServerSnapshot]) -> Vec<String>;
