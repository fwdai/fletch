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
use std::time::Duration;

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
                let exit = loop {
                    let status = {
                        let mut guard = child_for_wait.lock();
                        let Some(child) = guard.as_mut() else {
                            return;
                        };
                        match child.try_wait() {
                            Ok(Some(status)) => {
                                let _ = guard.take();
                                Some(Ok(status))
                            }
                            Ok(None) => None,
                            Err(e) => {
                                let _ = guard.take();
                                Some(Err(e))
                            }
                        }
                    };

                    match status {
                        Some(Ok(status)) => {
                            break ManagedExit {
                                success: status.success(),
                                message: format!("{status}"),
                            };
                        }
                        Some(Err(e)) => {
                            break ManagedExit {
                                success: false,
                                message: format!("wait failed: {e}"),
                            };
                        }
                        None => thread::sleep(Duration::from_millis(50)),
                    }
                };
                tracing::info!(success = exit.success, message = %exit.message, "managed: exited");
                on_exit(exit);
            });
        }

        Ok(Self {
            child: child_arc,
            stdin: stdin_arc,
        })
    }

    pub fn send_user_message(&self, text: &str, attachments: &[String]) -> Result<()> {
        // The typed message stays its own block; each attachment is sent as
        // a separate reference block so paths never pollute the user's prose.
        // The agent reads each path via its own file tools.
        let mut content: Vec<serde_json::Value> = Vec::new();
        if !text.is_empty() {
            content.push(json!({"type": "text", "text": text}));
        }
        for path in attachments {
            content.push(json!({"type": "text", "text": format!("Attached file: {path}")}));
        }
        let envelope = json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": content
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

    /// Send SIGINT to the child process to interrupt the current turn
    /// without killing it. Claude in stream-json mode handles SIGINT by
    /// aborting the current turn and emitting a result event, then
    /// returning to idle. If the process does not survive SIGINT the
    /// exit handler will transition the agent to Idle automatically.
    pub fn interrupt(&self) {
        #[cfg(unix)]
        {
            use nix::sys::signal::{kill, Signal};
            use nix::unistd::Pid;
            if let Some(child) = self.child.lock().as_ref() {
                let id = child.id();
                let _ = kill(Pid::from_raw(id as i32), Signal::SIGINT);
            }
        }
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
