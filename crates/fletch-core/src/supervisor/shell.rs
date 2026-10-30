//! Interactive per-agent shell PTYs: open, close, write, resize.

use std::sync::Arc;

use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::pty_session::{PtySession, PtySpawn};
use crate::sandbox::{boxes, KillHandle};

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
        //
        // The box is held like an agent's session, from before it is reached,
        // so its idle stop never ends the user's shell (and whatever it runs)
        // under them. Any return without a shell drops the hold.
        let hold = boxes::hold_session(&checkout);
        let guest = boxes::guest_program(&checkout, "bash", &["-i".to_string()])?;

        // Two opens race while one waits on a boot (the panel's mount effect
        // runs twice in dev): the first past this lock keeps its shell. The
        // spawn under it is quick, and the exit callback that takes the lock
        // runs on its own thread.
        let mut shells = self.shells.lock();
        if shells.contains_key(agent_id) {
            return Ok(());
        }
        let (shell_path, args, kill_plan) = match guest {
            Some((program, args)) => (
                program,
                args,
                hold.map_or(KillHandle::ProcessGroup, boxes::Hold::into_kill),
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

        let spawned = PtySession::spawn(
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
                    sup.end_shell(&agent_id_exit);
                }
            },
        );
        match spawned {
            Ok(session) => {
                shells.insert(agent_id.to_string(), session);
                Ok(())
            }
            Err(e) => {
                // No session to run the kill: release the box hold here, off
                // the lock like every other shell teardown (`end_shell`).
                drop(shells);
                let _ = kill_plan.kill();
                Err(e)
            }
        }
    }

    pub fn close_agent_shell(&self, agent_id: &str) -> Result<()> {
        self.end_shell(agent_id);
        Ok(())
    }

    /// End `agent_id`'s shell, if it has one — the one way a shell leaves the
    /// map. The session is dropped, and so killed (a process-group reap, plus
    /// an ssh round trip for a box session), only once the lock is released:
    /// every other agent's shell input waits on that lock.
    pub(super) fn end_shell(&self, agent_id: &str) {
        let shell = self.shells.lock().remove(agent_id);
        drop(shell);
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
