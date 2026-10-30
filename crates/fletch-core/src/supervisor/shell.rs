//! Interactive per-agent shell PTYs: open, close, write, resize.

use std::sync::Arc;

use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::pty_session::{PtySession, PtySpawn};
use crate::sandbox::KillHandle;

use super::events::emit_shell_output;
use super::Supervisor;

impl Supervisor {
    /// Blocks while a stopped box boots: call it off the UI thread.
    pub fn open_agent_shell(self: Arc<Self>, ctx: Arc<EngineCtx>, agent_id: &str) -> Result<()> {
        if self.shells.lock().contains_key(agent_id) {
            return Ok(());
        }

        let record = self.workspace.agent(agent_id)?;
        let repo = record
            .repos
            .first()
            .ok_or_else(|| Error::Other("agent has no repos".into()))?;
        let checkout = repo.checkout_path(agent_id)?;

        // The shell opens where the checkout is: a box keeps it in its guest,
        // so the terminal is a shell there, over the box's channel (booting a
        // stopped box); a host checkout gets the user's own shell. The host
        // PTY's cwd is the checkout either way (a box has a placeholder
        // directory there). The guest shell is interactive but not a login
        // shell: Debian's /etc/profile resets PATH, dropping the tools disk
        // (claude, codegraph) the guest env puts on it.
        let guest = crate::sandbox::boxes::guest_program(&checkout, "bash", &["-i".to_string()])?;

        // Two opens race while one waits on a boot (the panel's mount effect
        // runs twice in dev): the first past this lock keeps its shell. The
        // spawn under it is quick, and the exit callback that takes the lock
        // runs on its own thread.
        let mut shells = self.shells.lock();
        if shells.contains_key(agent_id) {
            return Ok(());
        }
        let (shell_path, args, kill_plan) = match guest {
            // Held like an agent's session, so the box's idle stop never ends
            // the user's shell (and whatever it runs) under them.
            Some((program, args)) => (
                program,
                args,
                crate::sandbox::boxes::attach_session(&checkout),
            ),
            None => {
                let shell_str = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
                (
                    std::path::PathBuf::from(&shell_str),
                    Vec::new(),
                    KillHandle::ProcessGroup,
                )
            }
        };

        let sup_weak = Arc::downgrade(&self);
        let agent_id_out = agent_id.to_string();
        let agent_id_exit = agent_id.to_string();

        let session = PtySession::spawn(
            PtySpawn {
                program: &shell_path,
                args: &args,
                cwd: &checkout,
                env: &[],
                cols: 120,
                rows: 32,
                env_remove: &[],
                kill_plan: kill_plan.clone(),
            },
            move |bytes| {
                emit_shell_output(ctx.sink.as_ref(), &agent_id_out, bytes);
            },
            move |exit| {
                tracing::info!(
                    success = exit.success,
                    message = %exit.message,
                    agent_id = %agent_id_exit,
                    "shell exited"
                );
                if let Some(sup) = sup_weak.upgrade() {
                    // Dropped (and so killed: a box session detaches over
                    // ssh) after the lock is released.
                    let gone = sup.shells.lock().remove(&agent_id_exit);
                    drop(gone);
                }
            },
        )
        // No session to run the kill: release the box hold here.
        .inspect_err(|_| {
            let _ = kill_plan.kill();
        })?;

        shells.insert(agent_id.to_string(), session);
        Ok(())
    }

    pub fn close_agent_shell(&self, agent_id: &str) -> Result<()> {
        // Drop impl kills the PTY, after the lock is released.
        let gone = self.shells.lock().remove(agent_id);
        drop(gone);
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
