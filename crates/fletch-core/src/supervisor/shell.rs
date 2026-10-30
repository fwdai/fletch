//! Interactive per-agent shell PTYs: open, close, write, resize.

use std::collections::hash_map::Entry;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::pty_session::{PtySession, PtySpawn};
use crate::sandbox::{boxes, KillGuard, KillHandle};

use super::events::emit_shell_output;
use super::Supervisor;

/// One agent's entry in [`Supervisor::shells`]. An open reserves the slot
/// (`Opening`) before its long part — booting the box the shell runs in — and
/// publishes its session only into a slot that is still its own, so a close or
/// an archive in that window, which removes the slot in either state, cancels
/// the open rather than being overtaken by it. Each open has its own
/// generation, which is also how a session's exit ends that session and never
/// a later one of the same agent.
pub enum ShellSlot {
    Opening { gen: u64 },
    Open { gen: u64, session: PtySession },
}

impl ShellSlot {
    fn gen(&self) -> u64 {
        match self {
            Self::Opening { gen } | Self::Open { gen, .. } => *gen,
        }
    }
}

/// [`ShellSlot`] generations, unique for the process.
static NEXT_GEN: AtomicU64 = AtomicU64::new(1);

impl Supervisor {
    /// Open `agent_id`'s shell, unless one is open or opening already. Blocks
    /// while a stopped box boots: call it off the UI thread.
    pub fn open_agent_shell(self: Arc<Self>, ctx: Arc<EngineCtx>, agent_id: &str) -> Result<()> {
        let Some(gen) = self.reserve_shell(agent_id) else {
            // Open, or an open is under way (the panel's mount effect runs
            // twice in dev, and a tab switch remounts it): that one delivers.
            return Ok(());
        };
        let started = self.start_shell(&ctx, agent_id, gen);
        self.publish_shell(agent_id, gen, started).inspect_err(|e| {
            // Into the shell's own stream, which every mount of the panel
            // replays: the caller this error returns to may be a terminal
            // already unmounted, a later mount having found this open under
            // way.
            let line = format!("\r\n\x1b[31mCould not open a shell: {e}\x1b[0m\r\n");
            emit_shell_output(ctx.sink.as_ref(), agent_id, line.into_bytes());
        })
    }

    /// Reserve `agent_id`'s slot for a new open and return its generation;
    /// `None` when the slot is taken, open or opening.
    fn reserve_shell(&self, agent_id: &str) -> Option<u64> {
        match self.shells.lock().entry(agent_id.to_string()) {
            Entry::Occupied(_) => None,
            Entry::Vacant(slot) => {
                let gen = NEXT_GEN.fetch_add(1, Ordering::Relaxed);
                slot.insert(ShellSlot::Opening { gen });
                Some(gen)
            }
        }
    }

    /// Spawn the shell for the open of generation `gen`, off the map's lock.
    fn start_shell(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
        agent_id: &str,
        gen: u64,
    ) -> Result<PtySession> {
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
        let (shell_path, args, kill_plan) =
            match boxes::guest_program(&checkout, "bash", &["-i".to_string()])? {
                Some((program, args)) => (
                    program,
                    args,
                    hold.map_or(KillHandle::ProcessGroup, boxes::Hold::into_kill),
                ),
                None => {
                    let shell_str =
                        std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
                    (
                        std::path::PathBuf::from(&shell_str),
                        Vec::new(),
                        KillHandle::ProcessGroup,
                    )
                }
            };

        let sup_weak = Arc::downgrade(self);
        let ctx = ctx.clone();
        let agent_id_out = agent_id.to_string();
        let agent_id_exit = agent_id.to_string();
        KillGuard::new(kill_plan).start(|kill_plan| {
            PtySession::spawn(
                PtySpawn {
                    program: &shell_path,
                    args: &args,
                    cwd: &checkout,
                    env: &[],
                    cols: 120,
                    rows: 32,
                    env_remove: &[],
                    kill_plan,
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
                        sup.end_shell_of(&agent_id_exit, gen);
                    }
                },
            )
        })
    }

    /// Publish what the open of generation `gen` started into its slot, if the
    /// slot is still that open's. A close, an archive or the session's own exit
    /// took it since: what was started is dropped — killed, its box hold
    /// released — and the open ends quietly, `Ok`.
    fn publish_shell(&self, agent_id: &str, gen: u64, started: Result<PtySession>) -> Result<()> {
        let mut shells = self.shells.lock();
        let ours = matches!(shells.get(agent_id), Some(ShellSlot::Opening { gen: g }) if *g == gen);
        if !ours {
            drop(shells);
            drop(started);
            return Ok(());
        }
        match started {
            Ok(session) => {
                shells.insert(agent_id.to_string(), ShellSlot::Open { gen, session });
                Ok(())
            }
            Err(e) => {
                shells.remove(agent_id);
                Err(e)
            }
        }
    }

    pub fn close_agent_shell(&self, agent_id: &str) -> Result<()> {
        self.end_shell(agent_id);
        Ok(())
    }

    /// End `agent_id`'s shell, open or opening: close, archive and discard. An
    /// open under way finds its slot gone and drops what it started
    /// ([`Self::publish_shell`]). The session is dropped — killed: a
    /// process-group reap, plus an ssh round trip for a box session — only
    /// once the lock is released, since every agent's shell input waits on it.
    pub(super) fn end_shell(&self, agent_id: &str) {
        let slot = self.shells.lock().remove(agent_id);
        drop(slot);
    }

    /// A session's exit: end its own slot, never a later open's.
    fn end_shell_of(&self, agent_id: &str, gen: u64) {
        let slot = {
            let mut shells = self.shells.lock();
            match shells.get(agent_id) {
                Some(slot) if slot.gen() == gen => shells.remove(agent_id),
                _ => None,
            }
        };
        drop(slot);
    }

    pub fn write_to_shell(&self, agent_id: &str, data: &[u8]) -> Result<()> {
        self.with_open_shell(agent_id, |session| session.write(data))
    }

    pub fn resize_shell(&self, agent_id: &str, cols: u16, rows: u16) -> Result<()> {
        self.with_open_shell(agent_id, |session| session.resize(cols, rows))
    }

    fn with_open_shell(
        &self,
        agent_id: &str,
        f: impl FnOnce(&PtySession) -> Result<()>,
    ) -> Result<()> {
        match self.shells.lock().get(agent_id) {
            Some(ShellSlot::Open { session, .. }) => f(session),
            Some(ShellSlot::Opening { .. }) => {
                Err(Error::Other("the shell is still opening".into()))
            }
            None => Err(Error::Other("no shell for agent".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::Duration;

    /// A long-lived shell whose exit is reported on the returned channel.
    fn long_lived() -> (PtySession, mpsc::Receiver<()>) {
        let (exit_tx, exit_rx) = mpsc::channel();
        let session = PtySession::spawn(
            PtySpawn {
                program: Path::new("/bin/sh"),
                args: &["-c".to_string(), "while :; do sleep 0.1; done".to_string()],
                cwd: Path::new("/"),
                env: &[],
                cols: 80,
                rows: 24,
                env_remove: &[],
                kill_plan: KillHandle::ProcessGroup,
            },
            |_| {},
            move |_| {
                let _ = exit_tx.send(());
            },
        )
        .unwrap();
        (session, exit_rx)
    }

    /// A close (or an archive) while an open is under way cancels it: the
    /// session the open went on to start is killed rather than published, and
    /// the next open starts afresh.
    #[test]
    fn a_close_cancels_an_open_under_way() {
        let sup = super::super::tests::test_supervisor();
        let gen = sup.reserve_shell("agent").unwrap();
        assert!(sup.reserve_shell("agent").is_none(), "one open at a time");

        sup.close_agent_shell("agent").unwrap();
        let (session, exited) = long_lived();
        sup.publish_shell("agent", gen, Ok(session)).unwrap();

        exited
            .recv_timeout(Duration::from_secs(5))
            .expect("the cancelled open's session is killed");
        assert!(sup.shells.lock().is_empty());
        assert!(sup.reserve_shell("agent").is_some());
    }

    /// A failed open frees its slot; an open whose slot was taken from it
    /// reports nothing, its caller having closed it.
    #[test]
    fn a_failed_open_frees_its_slot_and_a_cancelled_one_is_quiet() {
        let sup = super::super::tests::test_supervisor();
        let gen = sup.reserve_shell("agent").unwrap();
        let failed = sup.publish_shell("agent", gen, Err(Error::Other("no boot".into())));
        assert!(failed.is_err());
        assert!(sup.shells.lock().is_empty());

        let gen = sup.reserve_shell("agent").unwrap();
        sup.end_shell("agent");
        let cancelled = sup.publish_shell("agent", gen, Err(Error::Other("no boot".into())));
        assert!(cancelled.is_ok());
    }

    /// A session's exit ends its own generation only: an older session
    /// exiting late leaves the shell opened after it.
    #[test]
    fn an_old_sessions_exit_leaves_a_newer_shell() {
        let sup = super::super::tests::test_supervisor();
        let old = sup.reserve_shell("agent").unwrap();
        sup.end_shell("agent");
        let new = sup.reserve_shell("agent").unwrap();
        let (session, _exited) = long_lived();
        sup.publish_shell("agent", new, Ok(session)).unwrap();

        sup.end_shell_of("agent", old);
        assert!(matches!(
            sup.shells.lock().get("agent"),
            Some(ShellSlot::Open { gen, .. }) if *gen == new
        ));
        sup.end_shell_of("agent", new);
        assert!(sup.shells.lock().is_empty());
    }
}
