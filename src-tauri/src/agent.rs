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

use crate::codex_session::{CodexSession, CodexSpawn};
use crate::error::{Error, Result};
use crate::managed_session::{ManagedExit, ManagedSession, ManagedSpawn};
use crate::pty_session::{PtyExit, PtySession, PtySpawn};
use crate::sandbox;

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

pub enum Agent {
    Pty(PtyAgent),
    Managed(ManagedAgent),
    /// Codex's per-turn `exec` runner. Holds no live process between
    /// turns; each user message spawns a fresh `codex exec`.
    CodexManaged(CodexManagedAgent),
}

pub struct PtyAgent {
    pty: PtySession,
    _profile_file: tempfile::NamedTempFile,
}

pub struct ManagedAgent {
    session: ManagedSession,
    _profile_file: tempfile::NamedTempFile,
}

pub struct CodexManagedAgent {
    session: CodexSession,
}

/// Parameters for spawning a Codex runner. Unlike `SpawnSpec` there's
/// no sandbox profile (codex sandboxes itself) and the session id is
/// optional — codex assigns one on the first turn.
pub struct CodexSpawnSpec {
    /// Codex's working directory — the primary repo's worktree.
    pub cwd: PathBuf,
    /// Codex thread id to resume, if one has been captured already.
    pub session_id: Option<String>,
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

    /// Build a Codex per-turn runner. Spawns no process yet — the first
    /// `codex exec` is launched when the first user message arrives.
    /// `on_turn_exit(success)` fires when a turn's process exits (and that
    /// turn is still the current one) — the per-turn analogue of the
    /// turn-end signal, so an interrupted or failed turn that never emits
    /// `turn.completed` still leaves the agent promptly.
    pub fn spawn_codex<F, G, H>(
        spec: CodexSpawnSpec,
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
        let codex = resolve_codex(&home)?;

        tracing::info!(
            codex = %codex,
            cwd = %spec.cwd.display(),
            resume = spec.session_id.is_some(),
            "preparing codex runner"
        );

        let session = CodexSession::new(
            CodexSpawn {
                codex: PathBuf::from(codex),
                cwd: spec.cwd,
                session_id: spec.session_id,
            },
            on_event,
            on_session_id,
            on_turn_exit,
        );

        Ok(Self::CodexManaged(CodexManagedAgent { session }))
    }

    pub fn write_pty(&self, bytes: &[u8]) -> Result<()> {
        match self {
            Self::Pty(a) => a.pty.write(bytes),
            Self::Managed(_) | Self::CodexManaged(_) => Err(Error::Other(
                "write_pty called on a managed agent".into(),
            )),
        }
    }

    pub fn send_user_message(&self, text: &str, attachments: &[String]) -> Result<()> {
        match self {
            Self::Managed(a) => a.session.send_user_message(text, attachments),
            Self::CodexManaged(a) => a.session.send_user_message(text, attachments),
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
            Self::CodexManaged(a) => {
                a.session.interrupt();
            }
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        match self {
            Self::Pty(a) => a.pty.resize(cols, rows),
            Self::Managed(_) | Self::CodexManaged(_) => Ok(()),
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

fn resolve_codex(home: &Path) -> Result<String> {
    resolve_agent_bin("codex", "Codex", home)
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

