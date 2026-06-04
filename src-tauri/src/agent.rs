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

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::activity::{Activity, ManagedActivity};
use crate::error::{Error, Result};
use crate::exec_session::{ExecCallbacks, ExecSession, ExecSpawn};
use crate::managed_session::{ManagedExit, ManagedSession, ManagedSpawn};
use crate::pty_session::{PtyExit, PtySession, PtySpawn};
use crate::sandbox;

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

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
    _profile_file: tempfile::NamedTempFile,
}

pub struct ManagedAgent {
    session: ManagedSession,
    _profile_file: tempfile::NamedTempFile,
}

pub struct PerTurnAgent {
    session: ExecSession,
}

/// Parameters for spawning a per-turn runner. Unlike `SpawnSpec` there's
/// no sandbox profile (the agent sandboxes itself) and the session id is
/// optional — these agents assign one on the first turn.
pub struct PerTurnSpec {
    /// The agent's working directory — the primary repo's worktree.
    pub cwd: PathBuf,
    /// Session id to resume, if one has been captured already.
    pub session_id: Option<String>,
}

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
    bin: &'static str,
    /// Human-facing product name, used only in the not-found error.
    label: &'static str,
    /// Builds the CLI args for a turn: `(prompt, resume_session_id)`.
    build_args: fn(&str, Option<&str>) -> Vec<String>,
    /// Extracts the agent-assigned session id from a turn's events.
    session_id: fn(&Value) -> Option<String>,
    /// Constructs this agent's turn-end detector (custom-view `Activity`).
    pub activity: fn() -> Box<dyn Activity>,
    /// Rollout flags — see `AgentCapabilities`. These describe what's wired
    /// *today*, not a permanent limit; each is being brought to every agent
    /// in follow-up PRs, at which point its flag flips to `true`.
    native_view: bool,
    transcript_replay: bool,
}

/// What an agent can do *right now*. These are rollout flags, not fixed
/// traits: the roadmap is native (PTY/TUI) views and on-disk transcript
/// replay for every agent, with the SQLite event log as the canonical
/// history store you can always restore from. As each capability is wired
/// for an agent, its flag flips — callers gate on the capability, never on
/// the provider id, so nothing else changes when support lands.
pub struct AgentCapabilities {
    /// Can render in the native PTY view (its interactive TUI streamed into
    /// xterm), in addition to the structured custom view. Today: claude
    /// only; per-turn agents are custom-only until their TUI path is wired.
    pub native_view: bool,
    /// Has its native on-disk transcript wired for replay (parsed by the
    /// frontend adapter's `normalizeTranscript`). When false, re-attaching
    /// still restores history from the provider-agnostic SQLite event log —
    /// this flag only governs the richer native-format path.
    pub transcript_replay: bool,
}

/// Capabilities for a provider. Per-turn agents read theirs from the
/// descriptor table; claude (the lone persistent-runner agent) is the
/// fully-wired baseline. Unknown providers get nothing.
pub fn capabilities(provider: &str) -> AgentCapabilities {
    match per_turn_descriptor(provider) {
        Some(d) => AgentCapabilities {
            native_view: d.native_view,
            transcript_replay: d.transcript_replay,
        },
        None if provider == "claude" => AgentCapabilities {
            native_view: true,
            transcript_replay: true,
        },
        None => AgentCapabilities {
            native_view: false,
            transcript_replay: false,
        },
    }
}

const PER_TURN_AGENTS: &[PerTurnDescriptor] = &[
    PerTurnDescriptor {
        id: "codex",
        bin: "codex",
        label: "Codex",
        build_args: codex_build_args,
        session_id: codex_session_id,
        activity: || Box::new(ManagedActivity::codex()),
        native_view: false,
        // Codex persists a `rollout-*.jsonl`; `find_codex_rollout` + the
        // frontend codex adapter replay it.
        transcript_replay: true,
    },
    PerTurnDescriptor {
        id: "cursor",
        bin: "cursor-agent",
        label: "Cursor",
        build_args: cursor_build_args,
        session_id: cursor_session_id,
        // Cursor emits Claude-shaped stream-json incl. a `result` turn-end,
        // so it reuses the Claude managed detector.
        activity: || Box::new(ManagedActivity::claude()),
        native_view: false,
        // Cursor's on-disk chat format is undocumented; restore from the
        // SQLite log until a native transcript path is wired.
        transcript_replay: false,
    },
    PerTurnDescriptor {
        id: "opencode",
        bin: "opencode",
        label: "OpenCode",
        build_args: opencode_build_args,
        session_id: opencode_session_id,
        activity: || Box::new(ManagedActivity::opencode()),
        native_view: false,
        // OpenCode's `export` schema differs from its live stream; restore
        // from the SQLite log until that's mapped.
        transcript_replay: false,
    },
    PerTurnDescriptor {
        id: "pi",
        bin: "pi",
        label: "Pi",
        build_args: pi_build_args,
        session_id: pi_session_id,
        activity: || Box::new(ManagedActivity::pi()),
        native_view: false,
        // Pi persists a `session.jsonl` that's wireable later; restore from
        // the SQLite log until then.
        transcript_replay: false,
    },
];

/// Look up the descriptor for a per-turn provider id. `None` means the
/// provider isn't a per-turn agent (e.g. claude, which has its own
/// Pty/Managed runners) or isn't a known agent at all.
pub fn per_turn_descriptor(id: &str) -> Option<&'static PerTurnDescriptor> {
    PER_TURN_AGENTS.iter().find(|d| d.id == id)
}

pub struct SpawnSpec<'a> {
    pub agent_id: &'a str,
    /// Claude's working directory — the primary repo's worktree.
    pub cwd: PathBuf,
    /// Sandbox writable root — the agent's parent dir, which may
    /// contain multiple per-repo worktrees as siblings of `cwd`. Writes
    /// are allowed anywhere under this path.
    pub sandbox_root: PathBuf,
    pub session_id: &'a str,
    /// True if this is the agent's first spawn (no prior conversation
    /// on disk for this session). False if we're respawning to switch
    /// views — claude should `--resume` instead of starting fresh.
    pub fresh: bool,
    pub cols: u16,
    pub rows: u16,
}

impl Agent {
    pub fn spawn_pty<F, G>(spec: SpawnSpec<'_>, on_output: F, on_exit: G) -> Result<Self>
    where
        F: Fn(Vec<u8>) + Send + 'static,
        G: Fn(PtyExit) + Send + 'static,
    {
        let (profile_file, args) = prepare_pty_args(&spec)?;

        tracing::info!(
            agent_id = %spec.agent_id,
            session = %spec.session_id,
            fresh = spec.fresh,
            cwd = %spec.cwd.display(),
            sandbox_root = %spec.sandbox_root.display(),
            profile = %profile_file.path().display(),
            argv = ?args,
            "spawning sandboxed pty agent"
        );

        let pty = PtySession::spawn(
            PtySpawn {
                program: Path::new(SANDBOX_EXEC),
                args: &args,
                cwd: &spec.cwd,
                cols: spec.cols,
                rows: spec.rows,
            },
            on_output,
            on_exit,
        )?;

        Ok(Self::Pty(PtyAgent {
            pty,
            _profile_file: profile_file,
        }))
    }

    pub fn spawn_managed<F, G>(spec: SpawnSpec<'_>, on_event: F, on_exit: G) -> Result<Self>
    where
        F: Fn(Value) + Send + 'static,
        G: Fn(ManagedExit) + Send + 'static,
    {
        let (profile_file, args) = prepare_managed_args(&spec)?;

        tracing::info!(
            agent_id = %spec.agent_id,
            session = %spec.session_id,
            fresh = spec.fresh,
            cwd = %spec.cwd.display(),
            sandbox_root = %spec.sandbox_root.display(),
            profile = %profile_file.path().display(),
            argv = ?args,
            "spawning sandboxed managed agent"
        );

        let session = ManagedSession::spawn(
            ManagedSpawn {
                program: Path::new(SANDBOX_EXEC),
                args: &args,
                cwd: &spec.cwd,
            },
            on_event,
            on_exit,
        )?;

        Ok(Self::Managed(ManagedAgent {
            session,
            _profile_file: profile_file,
        }))
    }

    /// Build a per-turn runner (codex, cursor, opencode, pi) from its
    /// `PerTurnDescriptor`. The binary, CLI args, and session-id extraction
    /// come from the descriptor; the lifecycle is the shared `spawn_exec`.
    /// Per-turn agents hold no live process between turns — each user
    /// message spawns a fresh process — and sandbox themselves, so there's
    /// no sandbox-exec profile.
    pub fn spawn_per_turn<F, G, H>(
        desc: &PerTurnDescriptor,
        spec: PerTurnSpec,
        on_event: F,
        on_session_id: G,
        on_turn_exit: H,
    ) -> Result<Self>
    where
        F: Fn(Value) + Send + Sync + 'static,
        G: Fn(String) + Send + Sync + 'static,
        H: Fn(bool) + Send + Sync + 'static,
    {
        let home = dirs::home_dir()
            .ok_or_else(|| Error::Other("HOME directory not available".into()))?;
        let program = PathBuf::from(resolve_agent_bin(desc.bin, desc.label, &home)?);
        Self::spawn_exec(
            program,
            spec,
            desc.build_args,
            desc.session_id,
            ExecCallbacks {
                on_event,
                on_session_id,
                on_exit: on_turn_exit,
            },
        )
    }

    /// Shared per-turn exec lifecycle. Spawns no process yet — the first
    /// turn is launched when the first user message arrives. `on_exit(success)`
    /// fires when a turn's process exits (and that turn is still current)
    /// — the per-turn analogue of a turn-end signal, so an interrupted or
    /// failed turn that never emits an in-band turn-end still leaves the
    /// agent promptly.
    fn spawn_exec<A, I, F, G, H>(
        program: PathBuf,
        spec: PerTurnSpec,
        build_args: A,
        extract_session_id: I,
        cb: ExecCallbacks<F, G, H>,
    ) -> Result<Self>
    where
        A: Fn(&str, Option<&str>) -> Vec<String> + Send + Sync + 'static,
        I: Fn(&Value) -> Option<String> + Send + Sync + 'static,
        F: Fn(Value) + Send + Sync + 'static,
        G: Fn(String) + Send + Sync + 'static,
        H: Fn(bool) + Send + Sync + 'static,
    {
        tracing::info!(
            program = %program.display(),
            cwd = %spec.cwd.display(),
            resume = spec.session_id.is_some(),
            "preparing per-turn runner"
        );
        let session = ExecSession::new(
            ExecSpawn {
                program,
                cwd: spec.cwd,
                session_id: spec.session_id,
            },
            build_args,
            extract_session_id,
            cb,
        );
        Ok(Self::PerTurn(PerTurnAgent { session }))
    }

    pub fn write_pty(&self, bytes: &[u8]) -> Result<()> {
        match self {
            Self::Pty(a) => a.pty.write(bytes),
            Self::Managed(_) | Self::PerTurn(_) => Err(Error::Other(
                "write_pty called on a managed agent".into(),
            )),
        }
    }

    pub fn send_user_message(&self, text: &str, attachments: &[String]) -> Result<()> {
        match self {
            Self::Managed(a) => a.session.send_user_message(text, attachments),
            Self::PerTurn(a) => a.session.send_user_message(text, attachments),
            Self::Pty(_) => Err(Error::Other(
                "send_user_message called on pty agent".into(),
            )),
        }
    }

    /// Interrupt the agent's current turn without terminating the process.
    /// For PTY agents this writes Ctrl+C; for managed agents this sends SIGINT.
    pub fn interrupt(&self) {
        match self {
            Self::Pty(a) => {
                let _ = a.pty.interrupt();
            }
            Self::Managed(a) => {
                a.session.interrupt();
            }
            Self::PerTurn(a) => {
                a.session.interrupt();
            }
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        match self {
            Self::Pty(a) => a.pty.resize(cols, rows),
            Self::Managed(_) | Self::PerTurn(_) => Ok(()),
        }
    }

    pub fn shutdown(self) -> Result<()> {
        drop(self);
        Ok(())
    }
}

fn prepare_sandbox(
    worktree: &Path,
    home: &Path,
) -> Result<tempfile::NamedTempFile> {
    let profile_text = sandbox::build_profile(worktree, home)?;
    let mut profile_file = tempfile::Builder::new()
        .prefix("quorum-sandbox-")
        .suffix(".sb")
        .tempfile()
        .map_err(|e| Error::Other(format!("create sandbox profile tmp: {e}")))?;
    profile_file
        .write_all(profile_text.as_bytes())
        .map_err(|e| Error::Other(format!("write sandbox profile: {e}")))?;
    profile_file
        .flush()
        .map_err(|e| Error::Other(format!("flush sandbox profile: {e}")))?;
    Ok(profile_file)
}

fn prepare_pty_args(
    spec: &SpawnSpec<'_>,
) -> Result<(tempfile::NamedTempFile, Vec<String>)> {
    let home = dirs::home_dir()
        .ok_or_else(|| Error::Other("HOME directory not available".into()))?;
    let claude = resolve_claude(&home)?;
    let profile_file = prepare_sandbox(&spec.sandbox_root, &home)?;

    let profile_path = profile_file
        .path()
        .to_str()
        .ok_or_else(|| Error::Other("profile path not utf-8".into()))?
        .to_string();

    let mut args: Vec<String> = vec![
        "-f".into(),
        profile_path,
        claude,
        "--dangerously-skip-permissions".into(),
        "--permission-mode".into(),
        "bypassPermissions".into(),
    ];

    if spec.fresh {
        args.push("--session-id".into());
        args.push(spec.session_id.to_string());
    } else {
        args.push("--resume".into());
        args.push(spec.session_id.to_string());
    }

    Ok((profile_file, args))
}

fn prepare_managed_args(
    spec: &SpawnSpec<'_>,
) -> Result<(tempfile::NamedTempFile, Vec<String>)> {
    let home = dirs::home_dir()
        .ok_or_else(|| Error::Other("HOME directory not available".into()))?;
    let claude = resolve_claude(&home)?;
    let profile_file = prepare_sandbox(&spec.sandbox_root, &home)?;

    let profile_path = profile_file
        .path()
        .to_str()
        .ok_or_else(|| Error::Other("profile path not utf-8".into()))?
        .to_string();

    // Stream-json input + output give us a structured back-and-forth
    // over stdio. --verbose is required when using stream-json output
    // so events keep flowing. --include-partial-messages emits
    // incremental assistant text deltas for a responsive UI.
    let mut args: Vec<String> = vec![
        "-f".into(),
        profile_path,
        claude,
        "--print".into(),
        "--input-format".into(),
        "stream-json".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--include-partial-messages".into(),
        "--dangerously-skip-permissions".into(),
        "--permission-mode".into(),
        "bypassPermissions".into(),
    ];

    if spec.fresh {
        args.push("--session-id".into());
        args.push(spec.session_id.to_string());
    } else {
        args.push("--resume".into());
        args.push(spec.session_id.to_string());
    }

    Ok((profile_file, args))
}

fn resolve_claude(home: &Path) -> Result<String> {
    resolve_agent_bin("claude", "Claude Code", home)
}

// ── per-turn provider configs ────────────────────────────────────────────

/// Codex: `codex exec [resume <id>] --json …`. Approvals off + codex's own
/// workspace-write sandbox on, via `-c` (works on both `exec` and
/// `exec resume`, unlike the `-s`/`-a` flags). Quorum does not wrap codex
/// in sandbox-exec; codex sandboxes itself.
fn codex_build_args(prompt: &str, session_id: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec!["exec".into()];
    if let Some(id) = session_id {
        args.push("resume".into());
        args.push(id.to_string());
    }
    args.push("--json".into());
    args.push("--skip-git-repo-check".into());
    args.push("-c".into());
    args.push("approval_policy=\"never\"".into());
    args.push("-c".into());
    args.push("sandbox_mode=\"workspace-write\"".into());
    args.push(prompt.to_string());
    args
}

/// Codex assigns its thread id on the first turn via `thread.started`.
fn codex_session_id(event: &Value) -> Option<String> {
    if event.get("type").and_then(|t| t.as_str()) != Some("thread.started") {
        return None;
    }
    event
        .get("thread_id")
        .and_then(|t| t.as_str())
        .map(str::to_string)
}

/// Cursor: `cursor-agent -p --output-format stream-json --force [--resume <id>] <prompt>`.
/// `--force` runs commands without approval prompts; `--trust` trusts the
/// workspace in headless mode. Cursor's own sandbox applies; cwd comes from
/// the child process working directory.
fn cursor_build_args(prompt: &str, session_id: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-p".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--force".into(),
        "--trust".into(),
    ];
    if let Some(id) = session_id {
        args.push("--resume".into());
        args.push(id.to_string());
    }
    // Prompt is positional and must come after options.
    args.push(prompt.to_string());
    args
}

/// Cursor assigns its session id on the first turn, reported on the
/// `system`/`init` event (and echoed on every later event).
fn cursor_session_id(event: &Value) -> Option<String> {
    if event.get("type").and_then(|t| t.as_str()) != Some("system") {
        return None;
    }
    if event.get("subtype").and_then(|s| s.as_str()) != Some("init") {
        return None;
    }
    event
        .get("session_id")
        .and_then(|s| s.as_str())
        .map(str::to_string)
}

/// OpenCode: `opencode run --format json --dangerously-skip-permissions [--session <id>] <prompt>`.
/// `--dangerously-skip-permissions` auto-approves tools (incl. shell + file
/// writes) so turns run unattended; verified end-to-end against opencode
/// 1.15.12. OpenCode runs in the child's cwd (no `--dir` needed) and assigns
/// its own session id on the first turn. The prompt is positional and must
/// come after the flags.
fn opencode_build_args(prompt: &str, session_id: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "run".into(),
        "--format".into(),
        "json".into(),
        "--dangerously-skip-permissions".into(),
    ];
    if let Some(id) = session_id {
        args.push("--session".into());
        args.push(id.to_string());
    }
    args.push(prompt.to_string());
    args
}

/// OpenCode stamps the session id (`ses_…`) on the top-level `sessionID`
/// field of every event, so the first event of the first turn carries it.
/// `maybe_capture_session_id` captures it once and ignores the later echoes.
fn opencode_session_id(event: &Value) -> Option<String> {
    event
        .get("sessionID")
        .and_then(|s| s.as_str())
        .map(str::to_string)
}

/// Pi: `pi -p --mode json [--session <id>] <prompt>`. `-p` runs one turn
/// non-interactively and exits; in that mode Pi auto-runs its tools (bash,
/// write, …) with no approval prompt. Pi assigns its own session id on the
/// first turn (captured from the `session` event), and `--session <id>`
/// resumes it. We deliberately use `--session` (not the newer `--session-id`):
/// it's the resume flag common to the versions we target — 0.74.x lacks
/// `--session-id` entirely. Verified end-to-end against pi 0.74.2. Pi runs in
/// the child's cwd; the prompt is positional and must come after the flags.
fn pi_build_args(prompt: &str, session_id: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec!["-p".into(), "--mode".into(), "json".into()];
    if let Some(id) = session_id {
        args.push("--session".into());
        args.push(id.to_string());
    }
    args.push(prompt.to_string());
    args
}

/// Pi reports its session id on the first `{"type":"session","id":"…"}` event.
fn pi_session_id(event: &Value) -> Option<String> {
    if event.get("type").and_then(|t| t.as_str()) != Some("session") {
        return None;
    }
    event.get("id").and_then(|s| s.as_str()).map(str::to_string)
}

/// Locate an agent CLI by name: PATH first, then the user's login shell
/// (catches nvm / fnm / volta / homebrew setups the GUI process's bare
/// PATH misses), then the usual install dirs. `label` is the
/// human-facing product name used only in the not-found error.
fn resolve_agent_bin(name: &str, label: &str, home: &Path) -> Result<String> {
    if let Some(path) = command_in_path(name) {
        return Ok(path);
    }
    if let Some(path) = command_from_login_shell(name) {
        return Ok(path);
    }
    for candidate in common_bin_paths(name, home) {
        if candidate.is_file() {
            return Ok(candidate.to_string_lossy().into_owned());
        }
    }
    Err(Error::Other(format!(
        "Could not find the `{name}` executable. Install {label} or make it available on PATH."
    )))
}

fn common_bin_paths(name: &str, home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(format!(".local/bin/{name}")),
        home.join(format!(".npm-global/bin/{name}")),
        home.join(format!(".bun/bin/{name}")),
        PathBuf::from(format!("/opt/homebrew/bin/{name}")),
        PathBuf::from(format!("/usr/local/bin/{name}")),
    ]
}

fn command_in_path(name: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
        .map(|path| path.to_string_lossy().into_owned())
}

fn command_from_login_shell(name: &str) -> Option<String> {
    let script = format!("command -v {name}");
    let out = Command::new("/bin/zsh")
        .args(["-lc", &script])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_per_turn_agent_resolves_to_its_descriptor() {
        for d in PER_TURN_AGENTS {
            assert_eq!(per_turn_descriptor(d.id).map(|x| x.id), Some(d.id));
        }
        assert!(per_turn_descriptor("claude").is_none());
        assert!(per_turn_descriptor("nope").is_none());
    }

    /// Pins the current capability rollout. The roadmap is native views and
    /// transcript replay for every agent; when a follow-up wires one, it
    /// flips the descriptor flag and updates the expectation here on purpose.
    #[test]
    fn capability_rollout_matches_what_is_wired_today() {
        let cases = [
            // provider     native_view  transcript_replay
            ("claude", true, true),
            ("codex", false, true),
            ("cursor", false, false),
            ("opencode", false, false),
            ("pi", false, false),
            ("unknown", false, false),
        ];
        for (provider, native_view, transcript_replay) in cases {
            let caps = capabilities(provider);
            assert_eq!(caps.native_view, native_view, "native_view for {provider}");
            assert_eq!(
                caps.transcript_replay, transcript_replay,
                "transcript_replay for {provider}"
            );
        }
    }
}

