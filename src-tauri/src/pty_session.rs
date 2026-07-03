//! Local PTY around a child process.
//!
//! Used to wrap the `claude` invocation so the frontend xterm gets a
//! full interactive terminal (readline, ANSI colors, resize, ^C, etc.).

use parking_lot::Mutex;
use portable_pty::{ChildKiller, CommandBuilder, ExitStatus, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

pub struct PtySession {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    /// The child's process-group id. portable-pty makes the child a `setsid`
    /// session leader, so its pid *is* the pgid, and every descendant that
    /// stays in the group shares it. We keep it to signal the whole group on
    /// kill (see `kill`) rather than just the leader.
    #[cfg(unix)]
    pgid: Option<nix::unistd::Pid>,
}

pub struct PtySpawn<'a> {
    /// Path to the binary to exec.
    pub program: &'a std::path::Path,
    /// argv after the program.
    pub args: &'a [String],
    /// Working directory inside the PTY.
    pub cwd: &'a std::path::Path,
    /// Extra environment variables to set on the child, applied after the
    /// inherited environment (so they win on collision).
    pub env: &'a [(String, String)],
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone)]
pub struct PtyExit {
    pub success: bool,
    pub message: String,
}

impl PtySession {
    pub fn spawn<F, G>(spec: PtySpawn<'_>, on_output: F, on_exit: G) -> Result<Self>
    where
        F: Fn(Vec<u8>) + Send + 'static,
        G: Fn(PtyExit) + Send + 'static,
    {
        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: spec.rows,
                cols: spec.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| Error::Other(format!("openpty: {e}")))?;

        let mut cmd = CommandBuilder::new(spec.program);
        for a in spec.args {
            cmd.arg(a);
        }
        cmd.cwd(spec.cwd);

        // Inherit the user's environment so the child sees PATH, HOME,
        // ANTHROPIC_API_KEY, locale settings, etc. portable-pty doesn't
        // do this by default.
        for (k, v) in std::env::vars() {
            cmd.env(k, v);
        }
        if let Some(env) = crate::bin_resolve::login_shell_env() {
            for (k, v) in env {
                cmd.env(k, v);
            }
        }
        // Explicit terminal type — claude code uses ink which checks TERM
        // for capability lookups. Without this, input handling falls back
        // to a line-buffered mode that doesn't match what the user expects.
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        // Caller-supplied overrides (e.g. FLETCH_RPC_DIR) last, so they win.
        for (k, v) in spec.env {
            cmd.env(k, v);
        }

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| Error::Other(format!("pty spawn: {e}")))?;
        let killer = child.clone_killer();
        #[cfg(unix)]
        let pgid = child
            .process_id()
            .map(|pid| nix::unistd::Pid::from_raw(pid as i32));
        drop(pair.slave); // host side only needs master from here on

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| Error::Other(format!("pty clone reader: {e}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| Error::Other(format!("pty take writer: {e}")))?;

        let master = Arc::new(Mutex::new(pair.master));
        let writer = Arc::new(Mutex::new(writer));

        // Reader thread keeps a tight blocking-read loop so the PTY drains
        // promptly, handing each chunk to the coalescer over a channel. Batching
        // the emits happens downstream, not here.
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        thread::spawn({
            let mut reader = reader;
            move || {
                let mut buf = vec![0u8; 4096];
                let mut total: usize = 0;
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => {
                            tracing::info!(total_bytes = total, "pty reader: EOF");
                            break;
                        }
                        Ok(n) => {
                            total += n;
                            if tx.send(buf[..n].to_vec()).is_err() {
                                break; // coalescer gone; nothing left to feed
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(e) => {
                            tracing::warn!(error = %e, total = total, "pty reader: error, exiting");
                            break;
                        }
                    }
                }
                // tx drops here → coalescer sees Disconnected and flushes the tail.
            }
        });
        thread::spawn(move || coalesce_output(rx, on_output));

        thread::spawn(move || match child.wait() {
            Ok(status) => {
                let exit = exit_from_status(status);
                tracing::info!(
                    success = exit.success,
                    message = %exit.message,
                    "pty child exited"
                );
                on_exit(exit);
            }
            Err(e) => {
                tracing::warn!(error = %e, "pty child wait failed");
                on_exit(PtyExit {
                    success: false,
                    message: format!("wait failed: {e}"),
                });
            }
        });

        Ok(Self {
            master,
            writer,
            killer: Mutex::new(killer),
            #[cfg(unix)]
            pgid,
        })
    }

    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        self.writer
            .lock()
            .write_all(bytes)
            .map_err(|e| Error::Other(format!("pty write: {e}")))
    }

    /// Write Ctrl+C to the PTY to interrupt the currently running command
    /// without exiting the shell/process.
    pub fn interrupt(&self) -> Result<()> {
        self.write(&[0x03])
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.master
            .lock()
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| Error::Other(format!("pty resize: {e}")))
    }

    /// Terminate the child and everything running in its process group.
    ///
    /// portable-pty's own kill sends a lone `SIGHUP` to the leader's pid, so
    /// anything that ignores HUP or daemonizes into its own group (a dev
    /// server, a docker/compose invocation, a backgrounded script) is
    /// orphaned and keeps holding its port. We instead signal the whole
    /// group and escalate HUP → TERM → KILL, giving well-behaved processes a
    /// window to shut down before we force it.
    ///
    /// Blocks until the group is gone (or KILL is delivered): quit reaps
    /// synchronously, so the app can't exit and leak an orphan. Callers that
    /// hold a lock should drop it first (see `RunSession::stop`).
    pub fn kill(&self) -> Result<()> {
        #[cfg(unix)]
        if let Some(pgid) = self.pgid {
            return kill_process_group(pgid);
        }
        self.killer
            .lock()
            .kill()
            .map_err(|e| Error::Other(format!("pty kill: {e}")))
    }
}

/// Reap a process group with escalating signals, polling after each so we
/// stop the instant the group empties. A well-behaved child dies on the first
/// HUP and the poll returns in a tick; each grace window is a worst case.
/// SIGKILL is polled too, so the quit path doesn't return before the kernel
/// has torn the group down and released its ports.
///
/// Returns `Ok` only with positive evidence the group is gone (a probe or a
/// send seeing `ESRCH`). `EPERM` (can't signal — e.g. a reused pgid now owned
/// by another user) or a group that outlives `SIGKILL` yields `Err`, so we
/// never report a reap we couldn't confirm.
#[cfg(unix)]
fn kill_process_group(pgid: nix::unistd::Pid) -> Result<()> {
    use nix::errno::Errno;
    use nix::sys::signal::Signal::{SIGHUP, SIGKILL, SIGTERM};

    for (sig, grace) in [
        (SIGHUP, Duration::from_millis(200)),
        (SIGTERM, Duration::from_millis(300)),
        (SIGKILL, Duration::from_millis(200)),
    ] {
        match nix::sys::signal::killpg(pgid, sig) {
            Ok(()) => {}
            Err(Errno::ESRCH) => return Ok(()), // already empty — nothing to reap
            Err(e) => {
                return Err(Error::Other(format!(
                    "killpg({}, {sig:?}): {e}",
                    pgid.as_raw()
                )))
            }
        }
        if group_gone_within(pgid, grace) {
            return Ok(());
        }
    }
    Err(Error::Other(format!(
        "process group {} survived SIGKILL",
        pgid.as_raw()
    )))
}

/// Poll (signal-0 probe) until the group has no members or `budget` elapses.
/// Only `ESRCH` proves the group is gone; `EPERM` means it may still exist but
/// we can't signal it, so we must not report it reaped.
#[cfg(unix)]
fn group_gone_within(pgid: nix::unistd::Pid, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    loop {
        match nix::sys::signal::killpg(pgid, None) {
            Err(nix::errno::Errno::ESRCH) => return true, // nothing left in the group
            Err(_) => return false,                       // EPERM/other: can't confirm gone
            Ok(_) => {}
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Batch bursty PTY reads into fewer, larger `on_output` calls. A single 4 KB
/// read per emit means hundreds of IPC events/sec under heavy output; here we
/// accumulate until the batch reaches `MAX_BATCH` or `FLUSH_INTERVAL` elapses
/// since the batch's first byte, bounding added latency to one frame while
/// collapsing the event count. Byte order and content are preserved exactly.
fn coalesce_output<F: Fn(Vec<u8>)>(rx: mpsc::Receiver<Vec<u8>>, on_output: F) {
    const FLUSH_INTERVAL: Duration = Duration::from_millis(16);
    const MAX_BATCH: usize = 64 * 1024;

    let mut batch: Vec<u8> = Vec::new();
    let mut deadline: Option<Instant> = None;
    loop {
        let timeout = deadline
            .map(|d| d.saturating_duration_since(Instant::now()))
            .unwrap_or(FLUSH_INTERVAL);
        match rx.recv_timeout(timeout) {
            Ok(chunk) => {
                if batch.is_empty() {
                    deadline = Some(Instant::now() + FLUSH_INTERVAL);
                }
                batch.extend_from_slice(&chunk);
                // Flush on size, or once the frame elapses — checked here too so a
                // continuously-ready channel (recv_timeout never times out) can't
                // stretch the batch past one frame while waiting to hit MAX_BATCH.
                let expired = deadline.is_some_and(|d| Instant::now() >= d);
                if batch.len() >= MAX_BATCH || expired {
                    on_output(std::mem::take(&mut batch));
                    deadline = None;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if !batch.is_empty() {
                    on_output(std::mem::take(&mut batch));
                    deadline = None;
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                if !batch.is_empty() {
                    on_output(batch);
                }
                break;
            }
        }
    }
}

fn exit_from_status(status: ExitStatus) -> PtyExit {
    PtyExit {
        success: status.success(),
        message: status.to_string(),
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn streams_output_and_reports_exit() {
        let td = tempfile::tempdir().unwrap();
        let (out_tx, out_rx) = mpsc::channel();
        let (exit_tx, exit_rx) = mpsc::channel();

        let _pty = PtySession::spawn(
            PtySpawn {
                program: std::path::Path::new("/bin/sh"),
                args: &[
                    "-lc".to_string(),
                    "printf hello-from-pty".to_string(),
                ],
                cwd: td.path(),
                env: &[],
                cols: 80,
                rows: 24,
            },
            move |bytes| {
                let _ = out_tx.send(bytes);
            },
            move |exit| {
                let _ = exit_tx.send(exit);
            },
        )
        .unwrap();

        let first = out_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(String::from_utf8_lossy(&first).contains("hello-from-pty"));

        let exit = exit_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(exit.success, "unexpected PTY exit: {exit:?}");
    }

    #[cfg(unix)]
    #[test]
    fn kill_reaps_backgrounded_group_child() {
        use std::time::Instant;

        fn alive(pid: i32) -> bool {
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
        }

        let td = tempfile::tempdir().unwrap();
        let pidfile = td.path().join("child.pid");
        // The shell backgrounds a `sleep` (same group, no job control) then
        // blocks on `wait` — the stand-in for a dev server. portable-pty's
        // lone SIGHUP to the shell's pid never reaches the child; only a
        // group-wide signal does, so this proves killpg reaps it.
        let script = format!("sleep 1000 & echo $! > '{}'; wait", pidfile.display());
        let pty = PtySession::spawn(
            PtySpawn {
                program: std::path::Path::new("/bin/sh"),
                args: &["-c".to_string(), script],
                cwd: td.path(),
                env: &[],
                cols: 80,
                rows: 24,
            },
            |_| {},
            |_| {},
        )
        .unwrap();

        let start = Instant::now();
        let child = loop {
            if let Ok(pid) = std::fs::read_to_string(&pidfile)
                .unwrap_or_default()
                .trim()
                .parse::<i32>()
            {
                break pid;
            }
            assert!(start.elapsed() < Duration::from_secs(5), "child never started");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(alive(child), "backgrounded child should be running");

        pty.kill().unwrap();

        let start = Instant::now();
        while alive(child) && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !alive(child),
            "backgrounded child survived kill (pid {child}) — orphaned"
        );
    }
}
