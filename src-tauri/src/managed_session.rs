//! Non-PTY runner for "custom view" agents.
//!
//! Launches `claude --print` with `--input-format stream-json` and
//! `--output-format stream-json` so the app owns the input box and
//! renders structured events instead of raw terminal bytes.

use parking_lot::Mutex;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::thread;

use serde_json::{json, Value};

use crate::error::{Error, Result};

pub struct ManagedSession {
    child: Arc<Mutex<Option<Child>>>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
}

pub struct ManagedSpawn<'a> {
    pub program: &'a Path,
    pub args: &'a [String],
    pub cwd: &'a Path,
    /// First user message to send after spawn. `None` when resuming an
    /// existing session — the user will type the next turn themselves.
    pub initial_task: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct ManagedExit {
    pub success: bool,
    pub message: String,
}

impl ManagedSession {
    pub fn spawn<F, G>(spec: ManagedSpawn<'_>, on_event: F, on_exit: G) -> Result<Self>
    where
        F: Fn(Value) + Send + 'static,
        G: Fn(ManagedExit) + Send + 'static,
    {
        let mut cmd = Command::new(spec.program);
        cmd.args(spec.args);
        cmd.current_dir(spec.cwd);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| Error::Other(format!("managed spawn: {e}")))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Other("managed: child stdout missing".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::Other("managed: child stderr missing".into()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Other("managed: child stdin missing".into()))?;

        let child_arc = Arc::new(Mutex::new(Some(child)));
        let stdin_arc = Arc::new(Mutex::new(Some(stdin)));

        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(l) if l.trim().is_empty() => continue,
                    Ok(l) => match serde_json::from_str::<Value>(&l) {
                        Ok(v) => on_event(v),
                        Err(e) => {
                            tracing::warn!(error = %e, raw = %l, "managed: bad json line");
                        }
                    },
                    Err(e) => {
                        tracing::warn!(error = %e, "managed: stdout read error");
                        break;
                    }
                }
            }
            tracing::info!("managed: stdout closed");
        });

        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(l) => tracing::warn!(stderr = %l, "managed: stderr"),
                    Err(_) => break,
                }
            }
        });

        {
            let child_for_wait = child_arc.clone();
            thread::spawn(move || {
                let mut taken = match child_for_wait.lock().take() {
                    Some(c) => c,
                    None => return,
                };
                let exit = match taken.wait() {
                    Ok(status) => ManagedExit {
                        success: status.success(),
                        message: format!("{status}"),
                    },
                    Err(e) => ManagedExit {
                        success: false,
                        message: format!("wait failed: {e}"),
                    },
                };
                tracing::info!(success = exit.success, message = %exit.message, "managed: exited");
                on_exit(exit);
            });
        }

        let session = Self {
            child: child_arc,
            stdin: stdin_arc,
        };

        if let Some(text) = spec.initial_task {
            session.send_user_message(text)?;
        }

        Ok(session)
    }

    pub fn send_user_message(&self, text: &str) -> Result<()> {
        let envelope = json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": text}]
            }
        });
        let mut line = envelope.to_string();
        line.push('\n');

        let mut guard = self.stdin.lock();
        let stdin = guard
            .as_mut()
            .ok_or_else(|| Error::Other("managed: stdin closed".into()))?;
        stdin
            .write_all(line.as_bytes())
            .map_err(|e| Error::Other(format!("managed write: {e}")))?;
        stdin
            .flush()
            .map_err(|e| Error::Other(format!("managed flush: {e}")))
    }

    pub fn kill(&self) -> Result<()> {
        // Close stdin to signal EOF (claude exits cleanly that way),
        // then kill if it's still alive.
        let _ = self.stdin.lock().take();
        if let Some(mut child) = self.child.lock().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(())
    }
}

impl Drop for ManagedSession {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}
