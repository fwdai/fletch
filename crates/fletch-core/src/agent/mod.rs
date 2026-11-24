//! Per-agent lifecycle.
//!
//! An agent is a git worktree + a coding-agent process running inside
//! it. There are two runner shapes, both rendered by the app as structured
//! chat:
//!
//! - **Managed** (claude): a sandboxed, persistent `claude --print`
//!   stream-json subprocess. It attaches to its conversation via
//!   `--session-id <uuid>` until its transcript holds a message and
//!   `--resume <uuid>` after (`SessionStart`).
//! - **PerTurn** (codex, cursor, opencode, pi, antigravity): the CLI runs one
//!   turn and exits, so there's no persistent process — each user message
//!   spawns a fresh one (e.g. `codex exec [resume <id>]`, see
//!   `exec_session`).

mod account_identity;
pub mod accounts;
mod args;
mod auth_probe;
mod capabilities;
pub(crate) mod codex_home;
pub(crate) mod credential_file;
pub(crate) mod host_login;
mod host_state;
pub mod limits;
mod login;
mod probe;
mod providers;
mod spawn;
mod transcript;

#[cfg(test)]
mod tests;

use crate::exec_session::ExecSession;
use crate::managed_session::ManagedSession;

pub(crate) use auth_probe::env_key_present;
pub use auth_probe::{probe_all_provider_auth, AuthStatus, ProviderAuthProbe};
pub use capabilities::provider_bin_label;
pub use capabilities::{
    capabilities, injection_mode, mcp_delivery, one_shot, per_turn_descriptor, transcript_reader,
    PerTurnDescriptor,
};
pub use host_state::{host_providers, HostProvider};
pub use login::{login_command, logout_command};
pub use probe::{
    cached_provider_version, check_cli, probe_all_providers, resolve_all_providers, validate_bin,
    BinValidation, ProviderProbe, ToolStatus,
};
pub(crate) use probe::{
    claude_supports_thinking_display, file_identity, probe_shows_thinking_display,
    remember_thinking_display, run_probe_bounded, thinking_display_known,
    THINKING_DISPLAY_PROBE_ARGS,
};
pub use probe::{parse_semver, resolve_agent_bin};
pub(crate) use providers::claude::claude_session_has_messages;
pub use spawn::{PerTurnSpec, SessionStart, SpawnSpec};
pub use transcript::{
    read_jsonl_tail, PromptTexts, ReadDiagnostics, SubagentLayout, TranscriptReader,
};
pub(crate) use transcript::{write_new_jsonl, SUBAGENT_TAG};

pub enum Agent {
    Managed(ManagedAgent),
    /// A per-turn runner (codex, cursor): holds no live process between
    /// turns; each user message spawns a fresh process. The agent
    /// sandboxes itself, so there's no sandbox-exec profile.
    PerTurn(PerTurnAgent),
}

#[cfg(test)]
impl Agent {
    /// An agent over a bare process (`program args` in `cwd`), for a test that
    /// needs a live one to hand messages to. `login_expires_at_ms` is the host
    /// token it would have been launched on, when the test is about one.
    pub(crate) fn over_process(
        program: &std::path::Path,
        args: &[String],
        cwd: &std::path::Path,
        login_expires_at_ms: Option<i64>,
        on_exit: impl Fn(crate::managed_session::ManagedExit) + Send + 'static,
    ) -> Self {
        let session = ManagedSession::spawn(
            crate::managed_session::ManagedSpawn {
                program,
                args,
                cwd,
                env: &[],
                env_remove: &[],
                kill_plan: crate::sandbox::KillHandle::ProcessGroup,
                after_exit: None,
            },
            |_| {},
            on_exit,
        )
        .expect("the stand-in process starts");
        Self::Managed(ManagedAgent {
            session,
            login_expires_at_ms,
        })
    }

    /// [`Agent::over_process`] for a process that takes whatever it is handed
    /// and ignores it.
    pub(crate) fn idle_process(cwd: &std::path::Path, login_expires_at_ms: Option<i64>) -> Self {
        Self::over_process(
            std::path::Path::new("/bin/sh"),
            &["-c".to_string(), "cat >/dev/null".to_string()],
            cwd,
            login_expires_at_ms,
            |_| {},
        )
    }
}

pub struct ManagedAgent {
    session: ManagedSession,
    /// When the claude access token this process was launched with lapses
    /// (`host_login::claude`). The process can't take a new one, so the supervisor
    /// relaunches it before a turn that would outrun it.
    login_expires_at_ms: Option<i64>,
}

pub struct PerTurnAgent {
    // Boxed so `Agent` stays the size of its other variants.
    session: Box<ExecSession>,
}

/// The per-turn inputs a `*_build_args` builder turns into an invocation.
/// Bundled so builders and their call sites read as named fields instead of a
/// row of positional `Option<&str>`s. `Default` (all-`None`, empty prompt)
/// keeps test call sites terse: `TurnArgs { prompt: "hi", ..Default::default() }`.
#[derive(Clone, Copy, Default)]
pub struct TurnArgs<'a> {
    /// The user's message for this turn.
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

/// How a provider's CLI runs once as a plain text completion: headless, with
/// tools off or read-only, the prompt on stdin. The handoff summarizer's
/// runner (`crate::handoff`); a provider that can't run that way has none.
#[derive(Clone, Copy)]
pub struct OneShot {
    /// The argv for one run, given the model (`None` keeps the CLI default).
    /// Never the prompt: that goes to stdin, so its size is never argv's.
    pub args: fn(Option<&str>) -> Vec<String>,
    /// Where the answer lands: stdout when `None`, else the file the CLI
    /// writes when passed this flag and a path.
    pub reply_flag: Option<&'static str>,
}

/// Places the session's MCP servers where a provider's CLI will find them.
///
/// The one seam every provider's MCP support goes through: the builder writes
/// whatever config file its CLI reads (into the profile dir or the checkout,
/// per `McpTarget`) and returns the argv and/or environment pointing at it.
/// Fallible because most implementations touch the filesystem. Adding a
/// provider means writing one of these and listing it on the descriptor — no
/// new branch at any call site.
pub(crate) type McpDeliveryBuilder = fn(
    &crate::agent_profile::McpTarget<'_>,
) -> crate::error::Result<crate::agent_profile::McpDelivery>;
