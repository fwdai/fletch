//! Interactive per-agent shell PTYs: open, close, write, resize.

use std::sync::Arc;
use tauri::{AppHandle, Emitter};

use crate::error::{Error, Result};
use crate::pty_session::{PtySession, PtySpawn};
use crate::workspace::repo_worktree_path;

use super::events::ShellOutputPayload;
use super::Supervisor;

impl Supervisor {
    pub fn open_agent_shell(self: Arc<Self>, app: AppHandle, agent_id: &str) -> Result<()> {
        {
            let shells = self.shells.lock();
            if shells.contains_key(agent_id) {
                return Ok(());
            }
        }

        let record = self.workspace.agent(agent_id)?;
        let repo = record
            .repos
            .first()
            .ok_or_else(|| Error::Other("agent has no repos".into()))?;
        let worktree = repo_worktree_path(agent_id, &repo.subdir)?;

        let shell_str = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let shell_path = std::path::PathBuf::from(&shell_str);

        let sup_weak = Arc::downgrade(&self);
        let agent_id_out = agent_id.to_string();
        let agent_id_exit = agent_id.to_string();

        let session = PtySession::spawn(
            PtySpawn {
                program: &shell_path,
                args: &[],
                cwd: &worktree,
                env: &[],
                cols: 120,
                rows: 32,
            },
            move |bytes| {
                if let Err(e) = app.emit(
                    "shell:output",
                    ShellOutputPayload {
                        agent_id: agent_id_out.clone(),
                        bytes,
                    },
                ) {
                    tracing::warn!(error = %e, agent_id = %agent_id_out, "emit shell:output failed");
                }
            },
            move |exit| {
                tracing::info!(
                    success = exit.success,
                    message = %exit.message,
                    agent_id = %agent_id_exit,
                    "shell exited"
                );
                if let Some(sup) = sup_weak.upgrade() {
                    sup.shells.lock().remove(&agent_id_exit);
                }
            },
        )?;

        self.shells.lock().insert(agent_id.to_string(), session);
        Ok(())
    }

    pub fn close_agent_shell(&self, agent_id: &str) -> Result<()> {
        self.shells.lock().remove(agent_id); // Drop impl kills the PTY
        Ok(())
    }

    pub fn write_to_shell(&self, agent_id: &str, data: &[u8]) -> Result<()> {
        self.shells
            .lock()
            .get(agent_id)
            .ok_or_else(|| Error::Other("no shell for agent".into()))?
            .write(data)
    }

    pub fn resize_shell(&self, agent_id: &str, cols: u16, rows: u16) -> Result<()> {
        self.shells
            .lock()
            .get(agent_id)
            .ok_or_else(|| Error::Other("no shell for agent".into()))?
            .resize(cols, rows)
    }
}
