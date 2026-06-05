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
    /// `sandbox-exec` profile for claude's PTY run. `None` for per-turn
    /// agents in the native view: they launch their own binary directly and
    /// self-sandbox, so there's no profile to keep alive.
    _profile_file: Option<tempfile::NamedTempFile>,
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
    /// Builds the CLI args for a turn: `(prompt, resume_session_id, thinking_effort)`.
    build_args: fn(&str, Option<&str>, Option<&str>) -> Vec<String>,
    /// Builds the args to launch this agent's interactive TUI in the native
    /// (PTY) view: `None` = fresh session, `Some(id)` = resume.
    pty_args: fn(Option<&str>) -> Vec<String>,
    /// Extracts the agent-assigned session id from a turn's events.
    session_id: fn(&Value) -> Option<String>,
    /// Constructs this agent's turn-end detector (custom-view `Activity`).
    pub activity: fn() -> Box<dyn Activity>,
    /// Rollout flags — see `AgentCapabilities`. These describe what's wired
    /// *today*, not a permanent limit; each is being brought to every agent
    /// in follow-up PRs, at which point its flag flips to `true`.
    native_view: bool,
    transcript_replay: bool,
    /// True if this event is a finalized/durable form worth persisting to
    /// session_events; false for ephemeral streaming/lifecycle events.
    pub is_durable: fn(&Value) -> bool,
    /// Reader for this agent's on-disk transcript, used by `sync_session` to
    /// ingest verbatim records into `session_records`. `None` = no readable
    /// transcript yet (or a live-compiled agent).
    pub transcript: Option<TranscriptReader>,
}

/// One verbatim durable record from an agent's transcript: the raw body in the
/// agent's own shape plus a stable per-record dedup key (`native_id`).
#[derive(Debug, Clone)]
pub struct RawRecord {
    pub native_id: String,
    pub body: Value,
}

/// How to find and parse a provider's on-disk transcript into ordered records.
pub struct TranscriptReader {
    /// Ordered transcript artifact paths for a session (empty if none / not
    /// yet flushed). Multiple paths concatenate in order (resume can split).
    pub locate: fn(session_id: &str, cwd: &Path) -> Vec<PathBuf>,
    /// Parse located artifacts into ordered verbatim records.
    pub read: fn(paths: &[PathBuf]) -> Vec<RawRecord>,
}

/// What an agent can do *right now*. These are rollout flags, not fixed
/// traits: the roadmap is native (PTY/TUI) views and on-disk transcript
/// replay for every agent, with the SQLite event log as the canonical
/// history store you can always restore from. As each capability is wired
/// for an agent, its flag flips — callers gate on the capability, never on
/// the provider id, so nothing else changes when support lands.
pub struct AgentCapabilities {
    /// Can render in the native PTY view (its interactive TUI streamed into
    /// xterm), in addition to the structured custom view. Wired for claude
    /// and every per-turn agent (codex/cursor/opencode/pi); a per-turn agent
    /// can only switch *into* native once it has a session id to resume.
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
        pty_args: codex_pty_args,
        session_id: codex_session_id,
        activity: || Box::new(ManagedActivity::codex()),
        native_view: true,
        // Codex persists a `rollout-*.jsonl`; `find_codex_rollout` + the
        // frontend codex adapter replay it.
        transcript_replay: true,
        is_durable: codex_is_durable,
        transcript: Some(TranscriptReader {
            locate: codex_locate,
            read: codex_read,
        }),
    },
    PerTurnDescriptor {
        id: "cursor",
        bin: "cursor-agent",
        label: "Cursor",
        build_args: cursor_build_args,
        pty_args: cursor_pty_args,
        session_id: cursor_session_id,
        // Cursor emits Claude-shaped stream-json incl. a `result` turn-end,
        // so it reuses the Claude managed detector.
        activity: || Box::new(ManagedActivity::claude()),
        native_view: true,
        // Cursor's on-disk chat format is undocumented; restore from the
        // SQLite log until a native transcript path is wired.
        transcript_replay: false,
        // Cursor emits Claude-shaped events for most types, but uses a
        // dedicated tool_call event (started/completed) rather than
        // Claude's assistant.content tool_use + user.content tool_result.
        is_durable: cursor_is_durable,
        transcript: Some(TranscriptReader {
            locate: cursor_locate,
            read: cursor_read,
        }),
    },
    PerTurnDescriptor {
        id: "opencode",
        bin: "opencode",
        label: "OpenCode",
        build_args: opencode_build_args,
        pty_args: opencode_pty_args,
        session_id: opencode_session_id,
        activity: || Box::new(ManagedActivity::opencode()),
        native_view: true,
        // OpenCode's `export` schema differs from its live stream; restore
        // from the SQLite log until that's mapped.
        transcript_replay: false,
        is_durable: opencode_is_durable,
        transcript: None,
    },
    PerTurnDescriptor {
        id: "pi",
        bin: "pi",
        label: "Pi",
        build_args: pi_build_args,
        pty_args: pi_pty_args,
        session_id: pi_session_id,
        activity: || Box::new(ManagedActivity::pi()),
        native_view: true,
        // Pi persists a per-session JSONL whose lines match its live event
        // shape; `pi_*` read it into session_records (the reference reader).
        transcript_replay: false,
        is_durable: pi_is_durable,
        transcript: Some(TranscriptReader {
            locate: pi_locate,
            read: pi_read,
        }),
    },
];

/// Look up the descriptor for a per-turn provider id. `None` means the
/// provider isn't a per-turn agent (e.g. claude, which has its own
/// Pty/Managed runners) or isn't a known agent at all.
pub fn per_turn_descriptor(id: &str) -> Option<&'static PerTurnDescriptor> {
    PER_TURN_AGENTS.iter().find(|d| d.id == id)
}

// ── Transcript readers ──────────────────────────────────────────────────────

/// Build ordered `RawRecord`s from a parsed JSONL stream. `native_id` is the
/// value's `id_field` (a string) when present, else a positional `ln:{i}` key
/// over the global stream offset — stable across append-only multi-file reads.
pub fn records_with_id(values: Vec<Value>, id_field: Option<&str>) -> Vec<RawRecord> {
    values
        .into_iter()
        .enumerate()
        .map(|(i, body)| {
            let native_id = id_field
                .and_then(|f| body.get(f))
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| format!("ln:{i}"));
            RawRecord { native_id, body }
        })
        .collect()
}

/// JSONL files directly in `dir` whose filename ends with `suffix`, sorted
/// lexically (filenames are timestamp-prefixed, so lexical == chronological).
fn jsonl_files_ending(dir: &Path, suffix: &str) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(suffix))
        })
        .collect();
    paths.sort();
    paths
}

/// Pi's session-dir slug: cwd with `/` → `-`, wrapped in `--…--`.
/// `/Users/alex/Code/amux` → `--Users-alex-Code-amux--`. Dots are preserved.
fn pi_session_slug(cwd: &Path) -> String {
    format!("-{}--", cwd.to_string_lossy().replace('/', "-"))
}

fn pi_locate(session_id: &str, cwd: &Path) -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let dir = home.join(".pi/agent/sessions").join(pi_session_slug(cwd));
    // Files are `<ts>_<session_id>.jsonl`.
    jsonl_files_ending(&dir, &format!("_{session_id}.jsonl"))
}

fn pi_read(paths: &[PathBuf]) -> Vec<RawRecord> {
    let values: Vec<Value> = paths
        .iter()
        .flat_map(|p| crate::supervisor::read_jsonl_values(p).unwrap_or_default())
        .collect();
    // Pi's JSONL lines carry a stable `id`.
    records_with_id(values, Some("id"))
}

// ── Claude ──
// Claude is the lone persistent-runner agent (not in PER_TURN_AGENTS), launched
// `--session-id <uuid>` / `--resume <uuid>`, so it writes
// `~/.claude/projects/<slug>/<uuid>.jsonl`. find_session_jsonl already locates
// it (the existing transcript_replay path). Content lines carry a top-level
// `uuid`; metadata lines (mode/permission-mode/…) don't → positional fallback.

fn claude_locate(session_id: &str, _cwd: &Path) -> Vec<PathBuf> {
    crate::supervisor::find_session_jsonl(session_id)
        .into_iter()
        .collect()
}

fn claude_read(paths: &[PathBuf]) -> Vec<RawRecord> {
    let values: Vec<Value> = paths
        .iter()
        .flat_map(|p| crate::supervisor::read_jsonl_values(p).unwrap_or_default())
        .collect();
    records_with_id(values, Some("uuid"))
}

static CLAUDE_TRANSCRIPT: TranscriptReader = TranscriptReader {
    locate: claude_locate,
    read: claude_read,
};

// ── Codex ──
// Codex writes `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<ts>-<id>.jsonl`.
// Lines are `{timestamp,type,payload}` dual-channel with no stable per-line id,
// so records key positionally. The codex frontend adapter already normalizes.
fn codex_locate(session_id: &str, _cwd: &Path) -> Vec<PathBuf> {
    crate::supervisor::find_codex_rollouts(session_id)
}

fn codex_read(paths: &[PathBuf]) -> Vec<RawRecord> {
    let values: Vec<Value> = paths
        .iter()
        .flat_map(|p| crate::supervisor::read_jsonl_values(p).unwrap_or_default())
        .collect();
    records_with_id(values, None)
}

// ── Cursor ──
// cursor-agent writes `~/.cursor/projects/<slug>/agent-transcripts/<id>/<id>.jsonl`.
// The session-id dir is unique, so glob by it (like claude) rather than
// reverse-engineering the undocumented slug. Lines have no per-line id →
// positional keys.
fn cursor_locate(session_id: &str, _cwd: &Path) -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let rel = format!("agent-transcripts/{session_id}/{session_id}.jsonl");
    let projects = home.join(".cursor").join("projects");
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&projects) {
        for entry in entries.flatten() {
            let path = entry.path().join(&rel);
            if path.exists() {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

fn cursor_read(paths: &[PathBuf]) -> Vec<RawRecord> {
    let values: Vec<Value> = paths
        .iter()
        .flat_map(|p| crate::supervisor::read_jsonl_values(p).unwrap_or_default())
        .collect();
    records_with_id(values, None)
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
            _profile_file: Some(profile_file),
        }))
    }

    /// Launch a per-turn agent's interactive TUI in a PTY — the native view
    /// for codex/cursor/opencode/pi. Unlike claude's `spawn_pty`, the agent
    /// binary runs directly (no `sandbox-exec`): these agents self-sandbox.
    /// The session is always resumed (`spec.fresh == false`); the supervisor
    /// only routes a per-turn agent here once it has an established session
    /// id, so the TUI continues the same conversation the Custom view built.
    pub fn spawn_pty_native<F, G>(
        spec: SpawnSpec<'_>,
        provider: &str,
        on_output: F,
        on_exit: G,
    ) -> Result<Self>
    where
        F: Fn(Vec<u8>) + Send + 'static,
        G: Fn(PtyExit) + Send + 'static,
    {
        let desc = per_turn_descriptor(provider)
            .ok_or_else(|| Error::Other(format!("no per-turn descriptor for `{provider}`")))?;
        let home =
            dirs::home_dir().ok_or_else(|| Error::Other("HOME directory not available".into()))?;
        let bin = resolve_agent_bin(desc.bin, desc.label, &home)?;
        let session = if spec.fresh {
            None
        } else {
            Some(spec.session_id)
        };
        let args = (desc.pty_args)(session);

        tracing::info!(
            agent_id = %spec.agent_id,
            provider = %provider,
            session = %spec.session_id,
            fresh = spec.fresh,
            cwd = %spec.cwd.display(),
            bin = %bin,
            argv = ?args,
            "spawning native pty per-turn agent"
        );

        let pty = PtySession::spawn(
            PtySpawn {
                program: Path::new(&bin),
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
            _profile_file: None,
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
        A: Fn(&str, Option<&str>, Option<&str>) -> Vec<String> + Send + Sync + 'static,
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

    pub fn send_user_message(&self, text: &str, attachments: &[String], thinking: Option<&str>) -> Result<()> {
        match self {
            Self::Managed(a) => a.session.send_user_message(text, attachments),
            Self::PerTurn(a) => a.session.send_user_message(text, attachments, thinking),
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

// ── durable-event classification ─────────────────────────────────────────

/// Extract the `type` string from a raw provider event. Used by all
/// per-provider durability predicates.
fn event_type(ev: &Value) -> Option<&str> {
    ev.get("type").and_then(|t| t.as_str())
}

fn claude_is_durable(ev: &Value) -> bool {
    matches!(event_type(ev), Some("assistant" | "user" | "result"))
}

fn codex_is_durable(ev: &Value) -> bool {
    matches!(
        event_type(ev),
        Some("item.completed" | "turn.completed" | "turn.failed" | "error")
    )
}

fn opencode_is_durable(ev: &Value) -> bool {
    match event_type(ev) {
        // `reasoning` is a whole, finalized part (like `text`) — persist it so
        // thinking survives canonical-history replay, not just the live stream.
        Some("text" | "reasoning" | "step_finish" | "error") => true,
        // A tool_use is durable only once settled; pending/running are ephemeral.
        Some("tool_use") => {
            let status = ev
                .get("part")
                .and_then(|p| p.get("state"))
                .and_then(|s| s.get("status"))
                .and_then(|s| s.as_str());
            matches!(status, Some("completed" | "error"))
        }
        _ => false,
    }
}

fn cursor_is_durable(ev: &Value) -> bool {
    match event_type(ev) {
        // Cursor reuses Claude's stream-json for these.
        Some("assistant" | "user" | "result") => true,
        // Tool calls are a dedicated event; only the settled "completed" form
        // is durable ("started" is the in-progress/streaming form). Errors are
        // encoded inside the completed payload.
        Some("tool_call") => ev.get("subtype").and_then(|s| s.as_str()) == Some("completed"),
        // Thinking is its own event (NOT a Claude content block); the `delta`s
        // carry the text, terminated by `completed`. Persist the deltas so the
        // accumulated reasoning survives canonical-history replay.
        Some("thinking") => ev.get("subtype").and_then(|s| s.as_str()) == Some("delta"),
        _ => false,
    }
}

fn pi_is_durable(ev: &Value) -> bool {
    matches!(
        event_type(ev),
        Some("message_end" | "tool_execution_end" | "agent_end")
    )
}

/// True if this raw provider event is a durable, finalized form worth
/// persisting to the session_events log (replayed through the reducer on
/// restore). Ephemeral streaming deltas and no-op lifecycle events return
/// false. Unknown providers default to storing everything (lossless).
pub fn is_durable_event(provider: &str, ev: &Value) -> bool {
    match per_turn_descriptor(provider) {
        Some(d) => (d.is_durable)(ev),
        None if provider == "claude" => claude_is_durable(ev),
        None => true,
    }
}

// ── per-turn provider configs ─────────────────────────────────────────────

/// Codex: `codex exec [resume <id>] --json …`. Approvals off + codex's own
/// workspace-write sandbox on, via `-c` (works on both `exec` and
/// `exec resume`, unlike the `-s`/`-a` flags). Quorum does not wrap codex
/// in sandbox-exec; codex sandboxes itself.
fn codex_build_args(prompt: &str, session_id: Option<&str>, thinking: Option<&str>) -> Vec<String> {
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
    if let Some(effort) = thinking {
        args.push("-c".into());
        args.push(format!("reasoning_effort=\"{effort}\""));
    }
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
fn cursor_build_args(prompt: &str, session_id: Option<&str>, _thinking: Option<&str>) -> Vec<String> {
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
fn opencode_build_args(prompt: &str, session_id: Option<&str>, thinking: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "run".into(),
        "--format".into(),
        "json".into(),
        "--dangerously-skip-permissions".into(),
        // Surface the model's reasoning as `reasoning` events (captured by the
        // opencode reducer and persisted via opencode_is_durable).
        "--thinking".into(),
    ];
    if let Some(variant) = thinking {
        args.push("--variant".into());
        args.push(variant.to_string());
    }
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
fn pi_build_args(prompt: &str, session_id: Option<&str>, thinking: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec!["-p".into(), "--mode".into(), "json".into()];
    if let Some(level) = thinking {
        args.push("--thinking".into());
        args.push(level.to_string());
    }
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

// ── native (PTY/TUI) arg builders ───────────────────────────────────────────
//
// These launch each agent's *interactive* TUI inside a PTY (the native view),
// as opposed to the one-shot JSON `*_build_args` used by the structured Custom
// view. `session_id == None` starts a fresh interactive session; `Some(id)`
// resumes the prior one. The PTY runs in the agent's cwd (set by `PtySession`),
// so none of these need a working-dir flag. Verified against codex-cli 0.135,
// cursor-agent 2026.06, opencode 1.15, pi 0.74+.

/// Codex: bare `codex` launches the interactive TUI;
/// `--dangerously-bypass-approvals-and-sandbox` runs it unattended (Quorum
/// already isolates the worktree). `resume <id>` continues a prior session.
fn codex_pty_args(session_id: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec!["--dangerously-bypass-approvals-and-sandbox".into()];
    if let Some(id) = session_id {
        args.push("resume".into());
        args.push(id.to_string());
    }
    args
}

/// Cursor: bare `cursor-agent` launches the TUI; `--force` auto-allows
/// commands. `--resume <id>` continues a prior chat.
fn cursor_pty_args(session_id: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec!["--force".into()];
    if let Some(id) = session_id {
        args.push("--resume".into());
        args.push(id.to_string());
    }
    args
}

/// OpenCode: bare `opencode` launches the interactive TUI; `--session <id>`
/// continues a prior session. Note: no auto-approve flag — that's
/// `--dangerously-skip-permissions`, which belongs to the `run` (headless)
/// subcommand and makes the *default* (TUI) command print help and exit. The
/// TUI prompts for tool permissions interactively, which the native view
/// handles like any other keystroke.
fn opencode_pty_args(session_id: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if let Some(id) = session_id {
        args.push("--session".into());
        args.push(id.to_string());
    }
    args
}

/// Pi: bare `pi` launches the interactive TUI (tools auto-run there).
/// `--session <id>` resumes — same flag the Custom-view runner uses, since the
/// versions we target (0.74.x) lack `--session-id`.
fn pi_pty_args(session_id: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if let Some(id) = session_id {
        args.push("--session".into());
        args.push(id.to_string());
    }
    args
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

// ── Version probing ───────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct ProviderProbe {
    pub id: String,
    pub version: Option<String>,
    pub path: Option<String>,
}

/// Probe every known provider in parallel and return their resolved path +
/// version string. Missing/uninstalled providers return `None` for both fields;
/// the frontend falls back to the hardcoded defaults in that case.
pub async fn probe_all_providers() -> Vec<ProviderProbe> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));

    // (id, bin_name, human_label)
    let mut targets: Vec<(&str, &str, &str)> = vec![("claude", "claude", "Claude Code")];
    for d in PER_TURN_AGENTS {
        targets.push((d.id, d.bin, d.label));
    }

    let mut handles = Vec::new();
    for (id, bin, label) in targets {
        let home = home.clone();
        let id = id.to_string();
        let bin = bin.to_string();
        let label = label.to_string();
        handles.push(tokio::task::spawn_blocking(move || {
            let path = resolve_agent_bin(&bin, &label, &home).ok();
            let version = path.as_deref().and_then(probe_version);
            ProviderProbe { id, version, path }
        }));
    }

    let mut results = Vec::new();
    for handle in handles {
        if let Ok(probe) = handle.await {
            results.push(probe);
        }
    }
    results
}

/// Run `<bin> --version` and extract the first semver-like token from stdout
/// (or stderr as fallback). Returns `None` if the binary errors or emits no
/// recognisable version.
fn probe_version(bin: &str) -> Option<String> {
    let out = Command::new(bin).arg("--version").output().ok()?;
    let text = if !out.stdout.is_empty() {
        String::from_utf8_lossy(&out.stdout).into_owned()
    } else {
        String::from_utf8_lossy(&out.stderr).into_owned()
    };
    parse_semver(&text)
}

/// Extract the first `N.N[.N[.N]]` token from arbitrary version output.
/// Strips a leading `v` from each word before testing so `v1.0.42` and
/// `1.0.42` both match. Returns the token with a `v` prefix.
fn parse_semver(s: &str) -> Option<String> {
    for word in s.split_whitespace() {
        let word = word.trim_start_matches('v');
        // Accept anything that is purely digit-and-dot with at least one dot.
        if word.contains('.') && word.chars().all(|c| c.is_ascii_digit() || c == '.') && !word.starts_with('.') && !word.ends_with('.') {
            return Some(format!("v{word}"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── transcript readers ────────────────────────────────────────────────

    #[test]
    fn transcript_reader_dispatch() {
        // Claude (persistent runner, not a per-turn agent) + Pi/Codex/Cursor.
        assert!(transcript_reader("claude").is_some());
        assert!(transcript_reader("pi").is_some());
        assert!(transcript_reader("codex").is_some());
        assert!(transcript_reader("cursor").is_some());
        // Not yet wired / unknown.
        assert!(transcript_reader("opencode").is_none());
        assert!(transcript_reader("nope").is_none());
    }

    #[test]
    fn pi_slug_wraps_cwd_with_dashes() {
        assert_eq!(
            pi_session_slug(Path::new("/Users/alex/Code/amux")),
            "--Users-alex-Code-amux--"
        );
        // Dots are preserved (unlike Cursor) — only slashes are replaced.
        assert_eq!(
            pi_session_slug(Path::new("/Users/alex/.quorum/worktrees/balkhash/agent")),
            "--Users-alex-.quorum-worktrees-balkhash-agent--"
        );
    }

    #[test]
    fn records_with_id_uses_id_field_when_present() {
        let values = vec![json!({"id": "abc", "v": 1}), json!({"id": "def", "v": 2})];
        let recs = records_with_id(values, Some("id"));
        assert_eq!(recs[0].native_id, "abc");
        assert_eq!(recs[1].native_id, "def");
        assert_eq!(recs[0].body, json!({"id": "abc", "v": 1}));
    }

    #[test]
    fn records_with_id_positional_fallback_is_global() {
        // First line has an id, second doesn't; the positional index is the
        // global stream offset, not reset per missing line.
        let values = vec![json!({"id": "abc"}), json!({"no_id": true})];
        let recs = records_with_id(values, Some("id"));
        assert_eq!(recs[0].native_id, "abc");
        assert_eq!(recs[1].native_id, "ln:1");
    }

    #[test]
    fn records_with_id_none_field_is_all_positional() {
        let values = vec![json!({"a": 1}), json!({"a": 2})];
        let recs = records_with_id(values, None);
        assert_eq!(recs[0].native_id, "ln:0");
        assert_eq!(recs[1].native_id, "ln:1");
    }

    #[test]
    fn jsonl_files_ending_filters_and_sorts() {
        let td = tempfile::tempdir().unwrap();
        let dir = td.path();
        std::fs::write(dir.join("2026-06-04T19-10-20Z_sess-1.jsonl"), "{}").unwrap();
        std::fs::write(dir.join("2026-06-04T08-00-00Z_sess-1.jsonl"), "{}").unwrap();
        std::fs::write(dir.join("2026-06-04T09-00-00Z_other.jsonl"), "{}").unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();

        let found = jsonl_files_ending(dir, "_sess-1.jsonl");
        let names: Vec<String> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        // Only the two matching files, sorted lexically (== chronological here).
        assert_eq!(
            names,
            vec![
                "2026-06-04T08-00-00Z_sess-1.jsonl".to_string(),
                "2026-06-04T19-10-20Z_sess-1.jsonl".to_string(),
            ]
        );
    }

    #[test]
    fn missing_dir_yields_no_files() {
        let td = tempfile::tempdir().unwrap();
        assert!(jsonl_files_ending(&td.path().join("nope"), "_x.jsonl").is_empty());
    }

    // ── is_durable_event ──────────────────────────────────────────────────

    #[test]
    fn claude_durability() {
        assert!(is_durable_event("claude", &json!({"type": "assistant"})));
        assert!(is_durable_event("claude", &json!({"type": "user"})));
        assert!(is_durable_event("claude", &json!({"type": "result"})));
        assert!(!is_durable_event("claude", &json!({"type": "stream_event"})));
        assert!(!is_durable_event("claude", &json!({"type": "system"})));
    }

    #[test]
    fn codex_durability() {
        assert!(is_durable_event("codex", &json!({"type": "item.completed"})));
        assert!(is_durable_event("codex", &json!({"type": "turn.completed"})));
        assert!(is_durable_event("codex", &json!({"type": "turn.failed"})));
        assert!(is_durable_event("codex", &json!({"type": "error"})));
        assert!(!is_durable_event("codex", &json!({"type": "item.started"})));
        assert!(!is_durable_event("codex", &json!({"type": "turn.started"})));
    }

    #[test]
    fn opencode_durability() {
        assert!(is_durable_event("opencode", &json!({"type": "text"})));
        assert!(is_durable_event("opencode", &json!({"type": "reasoning"})));
        assert!(is_durable_event("opencode", &json!({"type": "step_finish"})));
        assert!(is_durable_event("opencode", &json!({"type": "error"})));
        assert!(!is_durable_event("opencode", &json!({"type": "step_start"})));
        // tool_use settled → durable
        assert!(is_durable_event(
            "opencode",
            &json!({"type": "tool_use", "part": {"state": {"status": "completed"}}})
        ));
        assert!(is_durable_event(
            "opencode",
            &json!({"type": "tool_use", "part": {"state": {"status": "error"}}})
        ));
        // tool_use in-flight → ephemeral
        assert!(!is_durable_event(
            "opencode",
            &json!({"type": "tool_use", "part": {"state": {"status": "running"}}})
        ));
        assert!(!is_durable_event(
            "opencode",
            &json!({"type": "tool_use", "part": {"state": {"status": "pending"}}})
        ));
    }

    #[test]
    fn pi_durability() {
        assert!(is_durable_event("pi", &json!({"type": "message_end"})));
        assert!(is_durable_event("pi", &json!({"type": "tool_execution_end"})));
        assert!(is_durable_event("pi", &json!({"type": "agent_end"})));
        assert!(!is_durable_event("pi", &json!({"type": "message_update"})));
    }

    #[test]
    fn cursor_durability() {
        // Claude-shaped events are durable.
        assert!(is_durable_event("cursor", &json!({"type": "assistant"})));
        assert!(is_durable_event("cursor", &json!({"type": "user"})));
        assert!(is_durable_event("cursor", &json!({"type": "result"})));
        // tool_call/completed is the settled, durable form.
        assert!(is_durable_event(
            "cursor",
            &json!({"type": "tool_call", "subtype": "completed"})
        ));
        // tool_call/started is the in-progress/streaming form — ephemeral.
        assert!(!is_durable_event(
            "cursor",
            &json!({"type": "tool_call", "subtype": "started"})
        ));
        // thinking/delta carries the reasoning text → durable; completed is an
        // empty terminator → ephemeral.
        assert!(is_durable_event(
            "cursor",
            &json!({"type": "thinking", "subtype": "delta"})
        ));
        assert!(!is_durable_event(
            "cursor",
            &json!({"type": "thinking", "subtype": "completed"})
        ));
        // Lifecycle events are ephemeral.
        assert!(!is_durable_event("cursor", &json!({"type": "stream_event"})));
        assert!(!is_durable_event("cursor", &json!({"type": "system"})));
    }

    #[test]
    fn unknown_provider_stores_everything() {
        // Lossless fallback: unknown provider → always durable.
        assert!(is_durable_event("zzz", &json!({"type": "anything"})));
        assert!(is_durable_event("zzz", &json!({})));
    }

    // ── build_args ────────────────────────────────────────────────────────

    #[test]
    fn opencode_args_request_thinking() {
        // Without --thinking, opencode emits no `reasoning` events at all.
        let args = opencode_build_args("hi", None, None);
        assert!(args.contains(&"--thinking".to_string()));
        assert!(args.contains(&"--format".to_string()));
        // Prompt is positional and last.
        assert_eq!(args.last().unwrap(), "hi");
    }

    // ── pty (native TUI) args ──────────────────────────────────────────────

    #[test]
    fn codex_pty_args_launch_tui_fresh_and_resume() {
        // Fresh: bypass approvals/sandbox so the TUI runs unattended; no
        // `exec`/`resume` subcommand means the interactive CLI.
        let fresh = codex_pty_args(None);
        assert!(fresh.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
        assert!(!fresh.iter().any(|a| a == "resume"));
        // Resume: `resume <id>` continues the prior interactive session.
        let resume = codex_pty_args(Some("abc123"));
        assert!(resume.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
        let pos = resume
            .iter()
            .position(|a| a == "resume")
            .expect("resume subcommand");
        assert_eq!(resume.get(pos + 1).map(String::as_str), Some("abc123"));
    }

    #[test]
    fn cursor_pty_args_force_and_resume() {
        let fresh = cursor_pty_args(None);
        assert!(fresh.contains(&"--force".to_string()));
        assert!(!fresh.iter().any(|a| a == "--resume"));
        let resume = cursor_pty_args(Some("chat-1"));
        assert!(resume.contains(&"--force".to_string()));
        let pos = resume
            .iter()
            .position(|a| a == "--resume")
            .expect("--resume flag");
        assert_eq!(resume.get(pos + 1).map(String::as_str), Some("chat-1"));
    }

    #[test]
    fn opencode_pty_args_launch_tui_and_session() {
        // Fresh: bare `opencode` launches the TUI. It must NOT carry
        // `--dangerously-skip-permissions` — that's a `run`-only flag, and
        // the default (TUI) command prints help and exits when given it.
        let fresh = opencode_pty_args(None);
        assert!(!fresh.iter().any(|a| a == "--dangerously-skip-permissions"));
        assert!(fresh.is_empty());
        // Resume: `--session <id>` continues the prior session.
        let resume = opencode_pty_args(Some("ses_9"));
        assert!(!resume.iter().any(|a| a == "--dangerously-skip-permissions"));
        let pos = resume
            .iter()
            .position(|a| a == "--session")
            .expect("--session flag");
        assert_eq!(resume.get(pos + 1).map(String::as_str), Some("ses_9"));
    }

    #[test]
    fn pi_pty_args_bare_tui_and_session() {
        // Fresh: bare `pi` launches the interactive TUI; tools auto-run there.
        let fresh = pi_pty_args(None);
        assert!(fresh.is_empty());
        // Resume uses `--session <id>` (target pi 0.74.x lacks `--session-id`).
        let resume = pi_pty_args(Some("u-7"));
        let pos = resume
            .iter()
            .position(|a| a == "--session")
            .expect("--session flag");
        assert_eq!(resume.get(pos + 1).map(String::as_str), Some("u-7"));
    }

    #[test]
    fn every_per_turn_agent_has_a_pty_arg_builder() {
        // Native view is wired for every per-turn agent, so each descriptor
        // must carry a TUI arg-builder. Fresh launch never references resume.
        for d in PER_TURN_AGENTS {
            let fresh = (d.pty_args)(None);
            assert!(
                !fresh
                    .iter()
                    .any(|a| a == "resume" || a == "--resume" || a == "--session"),
                "fresh {} args must not resume: {fresh:?}",
                d.id
            );
        }
    }

    #[test]
    fn opencode_args_variant_when_thinking_set() {
        let args = opencode_build_args("hi", None, Some("max"));
        assert!(args.contains(&"--variant".to_string()));
        assert!(args.contains(&"max".to_string()));
    }

    #[test]
    fn codex_args_reasoning_effort_when_thinking_set() {
        let args = codex_build_args("hi", None, Some("high"));
        assert!(args.contains(&"reasoning_effort=\"high\"".to_string()));
    }

    #[test]
    fn pi_args_thinking_when_set() {
        let args = pi_build_args("hi", None, Some("xhigh"));
        assert!(args.contains(&"--thinking".to_string()));
        assert!(args.contains(&"xhigh".to_string()));
    }

    #[test]
    fn cursor_args_ignores_thinking() {
        let with_none = cursor_build_args("hi", None, None);
        let with_some = cursor_build_args("hi", None, Some("high"));
        assert_eq!(with_none, with_some);
    }

    // ── descriptor table ──────────────────────────────────────────────────

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
            ("codex", true, true),
            ("cursor", true, false),
            ("opencode", true, false),
            ("pi", true, false),
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

