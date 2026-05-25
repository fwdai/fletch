//! Per-agent lifecycle.
//!
//! An agent is a git worktree + a sandboxed `claude` process running
//! inside it, with its I/O wired to a PTY the frontend displays as a
//! terminal.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};
use crate::pty_session::{PtyExit, PtySession, PtySpawn};
use crate::sandbox;

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

pub struct Agent {
    #[allow(dead_code)]
    pub id: String,
    #[allow(dead_code)]
    pub worktree: PathBuf,
    pty: PtySession,
    _profile_file: tempfile::NamedTempFile,
}

pub struct SpawnSpec<'a> {
    pub agent_id: &'a str,
    pub worktree: PathBuf,
    pub task: &'a str,
    pub cols: u16,
    pub rows: u16,
}

impl Agent {
    /// Spawn Claude in a sandbox rooted at the agent worktree. Output is
    /// streamed to `on_output`.
    pub fn spawn<F, G>(spec: SpawnSpec<'_>, on_output: F, on_exit: G) -> Result<Self>
    where
        F: Fn(Vec<u8>) + Send + 'static,
        G: Fn(PtyExit) + Send + 'static,
    {
        let home = dirs::home_dir()
            .ok_or_else(|| Error::Other("HOME directory not available".into()))?;
        let claude = resolve_claude(&home)?;

        let profile_text = sandbox::build_profile(&spec.worktree, &home)?;
        let mut profile_file = tempfile::Builder::new()
            .prefix("amux-sandbox-")
            .suffix(".sb")
            .tempfile()
            .map_err(|e| Error::Other(format!("create sandbox profile tmp: {e}")))?;
        profile_file
            .write_all(profile_text.as_bytes())
            .map_err(|e| Error::Other(format!("write sandbox profile: {e}")))?;
        profile_file
            .flush()
            .map_err(|e| Error::Other(format!("flush sandbox profile: {e}")))?;

        let args: Vec<String> = vec![
            "-f".into(),
            profile_file
                .path()
                .to_str()
                .ok_or_else(|| Error::Other("profile path not utf-8".into()))?
                .into(),
            claude,
            "--dangerously-skip-permissions".into(),
            "--permission-mode".into(),
            "bypassPermissions".into(),
            spec.task.to_string(),
        ];

        tracing::info!(
            agent_id = %spec.agent_id,
            worktree = %spec.worktree.display(),
            profile = %profile_file.path().display(),
            argv = ?args,
            "spawning sandboxed agent"
        );
        let pty = PtySession::spawn(
            PtySpawn {
                program: Path::new(SANDBOX_EXEC),
                args: &args,
                envs: &[],
                cwd: &spec.worktree,
                cols: spec.cols,
                rows: spec.rows,
            },
            on_output,
            on_exit,
        )?;

        Ok(Self {
            id: spec.agent_id.to_string(),
            worktree: spec.worktree,
            pty,
            _profile_file: profile_file,
        })
    }

    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        self.pty.write(bytes)
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.pty.resize(cols, rows)
    }

    pub fn shutdown(self) -> Result<()> {
        // PtySession::Drop kills the child + closes the PTY.
        drop(self.pty);
        Ok(())
    }
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
