//! Per-agent lifecycle.
//!
//! An agent is now just: a git worktree + a sandboxed `claude` process
//! running inside it, with its I/O wired to a PTY the frontend
//! displays as a terminal.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::pty_session::{PtySession, PtySpawn};
use crate::sandbox;

/// Where the sandbox-exec binary lives on every supported macOS.
const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// Default to claude on the user's PATH. Resolved via /usr/bin/env so
/// shims (Volta, fnm, etc.) work without us having to know about them.
const CLAUDE_LAUNCHER: &str = "/usr/bin/env";

pub struct Agent {
    #[allow(dead_code)]
    pub id: String,
    #[allow(dead_code)]
    pub worktree: PathBuf,
    pty: PtySession,
    /// SBPL profile file kept on disk for the lifetime of the agent.
    /// Dropped automatically when the Agent is dropped.
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
    /// Spawn claude inside a sandbox-exec profile rooted at the agent's
    /// worktree. Output is streamed to `on_output`.
    pub fn spawn<F>(spec: SpawnSpec<'_>, on_output: F) -> Result<Self>
    where
        F: Fn(Vec<u8>) + Send + 'static,
    {
        // 1. Resolve $HOME for the sandbox profile. claude reads from
        //    ~/.claude.json for auth and writes session state under
        //    ~/.claude, so we narrowly re-allow writes to those.
        let home = dirs::home_dir()
            .ok_or_else(|| Error::Other("HOME directory not available".into()))?;

        // 2. Build the profile and write it to a tempfile (sandbox-exec
        //    `-f` reads from a path; keeps the profile out of argv).
        let profile_text = sandbox::build_profile(&spec.worktree, &home)?;
        let mut profile_file = tempfile::Builder::new()
            .prefix("algiers-sandbox-")
            .suffix(".sb")
            .tempfile()
            .map_err(|e| Error::Other(format!("create sandbox profile tmp: {e}")))?;
        profile_file
            .write_all(profile_text.as_bytes())
            .map_err(|e| Error::Other(format!("write sandbox profile: {e}")))?;
        profile_file
            .flush()
            .map_err(|e| Error::Other(format!("flush sandbox profile: {e}")))?;

        // 3. argv for sandbox-exec:
        //      sandbox-exec -f <profile>
        //        /usr/bin/env claude --dangerously-skip-permissions "<task>"
        let args: Vec<String> = vec![
            "-f".into(),
            profile_file
                .path()
                .to_str()
                .ok_or_else(|| Error::Other("profile path not utf-8".into()))?
                .into(),
            CLAUDE_LAUNCHER.into(),
            "claude".into(),
            "--dangerously-skip-permissions".into(),
            spec.task.to_string(),
        ];

        // 4. Spawn under a PTY so the frontend xterm gets a real
        //    interactive terminal.
        tracing::info!(
            agent_id = %spec.agent_id,
            worktree = %spec.worktree.display(),
            profile = %profile_file.path().display(),
            argv = ?args,
            "spawning agent under sandbox-exec"
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
        // PtySession::Drop kills the child + closes the PTY; the
        // profile tempfile drops with us.
        drop(self.pty);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_profile_includes_correct_subpath() {
        let td = tempfile::tempdir().unwrap();
        let wt = td.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        let profile = sandbox::build_profile(&wt, td.path()).unwrap();
        let canon = std::fs::canonicalize(&wt).unwrap();
        assert!(profile.contains(canon.to_str().unwrap()));
    }
}
