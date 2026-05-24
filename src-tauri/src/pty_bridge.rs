//! SSH-over-PTY bridge.
//!
//! Spawns `ssh -tt user@host -- <remote-cmd>` inside a local PTY so the agent
//! gets a fully interactive terminal. Bytes flowing out of the PTY are handed
//! to a callback (the supervisor uses it to forward to Tauri events). Bytes
//! flowing in come from the frontend via [`PtySession::write`].

use parking_lot::Mutex;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::thread;

use crate::error::{Error, Result};

pub struct PtySession {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
}

pub struct SshTarget<'a> {
    pub user: &'a str,
    pub host: &'a str,
    pub key_path: &'a Path,
    pub port: Option<u16>,
}

pub struct SshSpawn<'a> {
    pub target: SshTarget<'a>,
    pub remote_cmd: &'a str,
    pub cols: u16,
    pub rows: u16,
}

impl PtySession {
    /// Spawn an SSH-over-PTY session. `on_output` is called from a background
    /// thread whenever bytes arrive from the remote PTY.
    pub fn spawn_ssh<F>(spec: SshSpawn<'_>, on_output: F) -> Result<Self>
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
            .map_err(|e| Error::Ssh(format!("openpty failed: {e}")))?;

        let mut cmd = CommandBuilder::new("ssh");
        // CommandBuilder::arg/args return (), so they can't be chained.
        cmd.arg("-tt"); // force PTY allocation on the remote
        cmd.args(["-o", "StrictHostKeyChecking=no"]);
        cmd.args(["-o", "UserKnownHostsFile=/dev/null"]);
        cmd.args(["-o", "LogLevel=ERROR"]);
        cmd.arg("-i");
        cmd.arg(spec.target.key_path);
        if let Some(p) = spec.target.port {
            cmd.arg("-p");
            cmd.arg(p.to_string());
        }
        cmd.arg(format!("{}@{}", spec.target.user, spec.target.host));
        cmd.arg("--");
        cmd.arg(spec.remote_cmd);

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| Error::Ssh(format!("ssh spawn failed: {e}")))?;
        let killer = child.clone_killer();
        drop(pair.slave); // we only need the master from here

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| Error::Ssh(format!("clone reader failed: {e}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| Error::Ssh(format!("take writer failed: {e}")))?;

        let master = Arc::new(Mutex::new(pair.master));
        let writer = Arc::new(Mutex::new(writer));

        // Reader thread: pump PTY output to the callback. Exits when the PTY
        // closes (the child has exited or been killed). Sole owner of the
        // callback — no Sync bound needed because we don't share it.
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
            .map_err(|e| Error::Ssh(format!("pty write: {e}")))
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
            .map_err(|e| Error::Ssh(format!("resize: {e}")))
    }

    pub fn kill(&self) -> Result<()> {
        self.killer
            .lock()
            .kill()
            .map_err(|e| Error::Ssh(format!("kill: {e}")))
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}
