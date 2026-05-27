//! Per-agent lifecycle.
//!
//! An agent is a git worktree + a sandboxed `claude` process running
//! inside it. The process runs either in a PTY (native view — xterm
//! shows claude's TUI; the app overlays its own input over claude's
//! prompt) or as a stream-json subprocess (custom view — the app
//! renders structured chat messages itself).
//!
//! Both shapes attach to the *same* conversation via claude's
//! `--session-id <uuid>` on first spawn and `--resume <uuid>` on
//! subsequent spawns (view switches). Only one process is alive at a
//! time per agent.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::managed_session::{ManagedExit, ManagedSession, ManagedSpawn};
use crate::pty_session::{PtyExit, PtySession, PtySpawn};
use crate::sandbox;

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

pub enum Agent {
    Pty(PtyAgent),
    Managed(ManagedAgent),
}

pub struct PtyAgent {
    pty: PtySession,
    _profile_file: tempfile::NamedTempFile,
}

pub struct ManagedAgent {
    session: ManagedSession,
    _profile_file: tempfile::NamedTempFile,
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

    pub fn write_pty(&self, bytes: &[u8]) -> Result<()> {
        match self {
            Self::Pty(a) => a.pty.write(bytes),
            Self::Managed(_) => Err(Error::Other(
                "write_pty called on managed agent".into(),
            )),
        }
    }

    pub fn send_user_message(&self, text: &str) -> Result<()> {
        match self {
            Self::Managed(a) => a.session.send_user_message(text),
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
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        match self {
            Self::Pty(a) => a.pty.resize(cols, rows),
            Self::Managed(_) => Ok(()),
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
    if let Some(path) = command_in_path("claude") {
        return Ok(path);
    }

    if let Some(path) = command_from_login_shell("claude") {
        return Ok(path);
    }

    for candidate in common_claude_paths(home) {
        if candidate.is_file() {
            return Ok(candidate.to_string_lossy().into_owned());
        }
    }

    Err(Error::Other(
        "Could not find the `claude` executable. Install Claude Code or make it available on PATH."
            .into(),
    ))
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

fn common_claude_paths(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".local/bin/claude"),
        home.join(".npm-global/bin/claude"),
        home.join(".bun/bin/claude"),
        PathBuf::from("/opt/homebrew/bin/claude"),
        PathBuf::from("/usr/local/bin/claude"),
    ]
}
