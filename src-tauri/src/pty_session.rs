//! Local PTY around a child process.
//!
//! Used to wrap the sandboxed `claude` invocation so the frontend
//! xterm gets a full interactive terminal (readline, ANSI colors,
//! resize, ^C, etc.). Replaces the previous SSH-based PtySession.

use parking_lot::Mutex;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::Arc;
use std::thread;

use crate::error::{Error, Result};

pub struct PtySession {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
}

pub struct PtySpawn<'a> {
    /// Path to the binary to exec.
    pub program: &'a std::path::Path,
    /// argv after the program.
    pub args: &'a [String],
    /// Extra env vars to set (on top of inherited).
    pub envs: &'a [(&'a str, String)],
    /// Working directory inside the PTY.
    pub cwd: &'a std::path::Path,
    pub cols: u16,
    pub rows: u16,
}

impl PtySession {
    pub fn spawn<F>(spec: PtySpawn<'_>, on_output: F) -> Result<Self>
    where
        F: Fn(Vec<u8>) + Send + 'static,
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
        // Explicit terminal type — claude code uses ink which checks TERM
        // for capability lookups. Without this, input handling falls back
        // to a line-buffered mode that doesn't match what the user expects.
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        // Caller-supplied overrides take precedence.
        for (k, v) in spec.envs {
            cmd.env(*k, v);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| Error::Other(format!("pty spawn: {e}")))?;
        let killer = child.clone_killer();
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
                            tracing::trace!(
                                bytes = n,
                                total = total,
                                preview = %String::from_utf8_lossy(&buf[..n.min(120)]).replace('\n', "\\n"),
                                "pty reader: chunk"
                            );
                            on_output(buf[..n].to_vec());
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(e) => {
                            tracing::warn!(error = %e, total = total, "pty reader: error, exiting");
                            break;
                        }
                    }
                }
            }
        });

        Ok(Self {
            master,
            writer,
            killer: Mutex::new(killer),
        })
    }

    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        self.writer
            .lock()
            .write_all(bytes)
            .map_err(|e| Error::Other(format!("pty write: {e}")))
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

    pub fn kill(&self) -> Result<()> {
        self.killer
            .lock()
            .kill()
            .map_err(|e| Error::Other(format!("pty kill: {e}")))
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}
