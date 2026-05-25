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
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => on_output(buf[..n].to_vec()),
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
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
