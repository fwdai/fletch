//! Non-PTY runner for "custom view" agents.
//!
//! Launches `claude --print` with `--input-format stream-json` and
//! `--output-format stream-json` so the app owns the input box and
//! renders structured events instead of raw terminal bytes.
//!
//! ## Permission control protocol
//!
//! We run Claude in `--permission-mode default --permission-prompt-tool stdio`
//! (not `bypassPermissions`). In that mode the CLI does not execute a tool
//! until the client approves it: it writes a `control_request`
//! (`subtype: "can_use_tool"`) to stdout and blocks until we write a matching
//! `control_response` to stdin. We auto-approve every tool the instant it
//! arrives — preserving the prior fully-headless "run without nagging" feel —
//! **except** the user-input tools (`AskUserQuestion`, `ExitPlanMode`), which
//! we hold open and surface to the UI so the human actually answers. Holding
//! the response is the real pause: the agent's turn is suspended until
//! `answer_tool_use` delivers the user's selection as the tool result.
//!
//! `bypassPermissions` cannot do this — it short-circuits the permission flow
//! and auto-denies `AskUserQuestion` before the client ever sees it.

use parking_lot::Mutex;
use std::collections::HashSet;
use std::io::Write;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::child_io;
use crate::error::{Error, Result};

/// Tools whose `can_use_tool` request we hold open for a human answer instead
/// of auto-approving. Everything else runs unattended.
const HOLD_TOOLS: &[&str] = &["AskUserQuestion", "ExitPlanMode"];

type Stdin = Arc<Mutex<Option<ChildStdin>>>;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ToolUseBehavior {
    Allow,
    Deny,
}

pub struct ManagedSession {
    child: Arc<Mutex<Option<Child>>>,
    stdin: Stdin,
    /// `request_id`s of `can_use_tool` prompts we're holding open, awaiting a
    /// human answer. Drained on interrupt so the turn can unwind.
    pending: Arc<Mutex<HashSet<String>>>,
}

pub struct ManagedSpawn<'a> {
    pub program: &'a Path,
    pub args: &'a [String],
    pub cwd: &'a Path,
    /// Extra environment variables (e.g. `FLETCH_RPC_DIR`). `Command` inherits
    /// the parent environment by default; these are layered on top.
    pub env: &'a [(String, String)],
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
        crate::bin_resolve::apply_login_shell_env(&mut cmd);
        // Portable-git PATH/env so the child's own `git` calls work on a
        // machine with no usable system git. No-op on system git.
        for (k, v) in crate::git_dist::child_env() {
            cmd.env(k, v);
        }
        for (k, v) in spec.env {
            cmd.env(k, v);
        }
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
        let stdin_arc: Stdin = Arc::new(Mutex::new(Some(stdin)));
        let pending: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

        // Establish the control channel so the CLI routes permission prompts to
        // us (rather than auto-denying). The reply (request_id "quorum-init")
        // comes back as a control_response, which the reader drops.
        let _ = write_line(
            &stdin_arc,
            json!({
                "type": "control_request",
                "request_id": "quorum-init",
                "request": { "subtype": "initialize", "hooks": {} }
            }),
        );

        let stdin_for_reader = stdin_arc.clone();
        let pending_for_reader = pending.clone();
        child_io::spawn_json_reader(stdout, "managed", tracing::Level::INFO, move |v| {
            match v.get("type").and_then(Value::as_str) {
                // Permission gate from the CLI: respond on stdin.
                Some("control_request") => {
                    handle_control_request(&stdin_for_reader, &pending_for_reader, &on_event, v)
                }
                // Reply to a request we sent (e.g. initialize) — control plane,
                // never a transcript event.
                Some("control_response") => {}
                _ => on_event(v),
            }
        });

        child_io::spawn_stderr_reader(stderr, "managed", |_| {});

        child_io::spawn_reaper(child_arc.clone(), "managed", move |status| {
            let exit = match status {
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

        Ok(Self {
            child: child_arc,
            stdin: stdin_arc,
            pending,
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
        write_line(
            &self.stdin,
            json!({
                "type": "user",
                "message": { "role": "user", "content": content }
            }),
        )
    }

    /// Answer a held `can_use_tool` prompt (the question tools). `updated_input`
    /// is the tool's input with the user's `answers` merged in; the CLI feeds it
    /// back to the model as the tool result and resumes the turn.
    pub fn answer_tool_use(
        &self,
        request_id: &str,
        updated_input: Value,
        behavior: ToolUseBehavior,
        message: Option<String>,
    ) -> Result<()> {
        if !self.pending.lock().remove(request_id) {
            tracing::debug!(
                request_id,
                "managed: ignoring answer for non-pending tool request"
            );
            return Ok(());
        }
        let response = match behavior {
            ToolUseBehavior::Allow => allow_response(request_id, updated_input),
            ToolUseBehavior::Deny => {
                deny_response(request_id, message.as_deref().unwrap_or("Denied by user"))
            }
        };
        write_line(&self.stdin, response)
    }

    /// True while the turn is paused on a held permission prompt (a question
    /// tool). We can't write a new user message into a paused turn, so the
    /// supervisor queues mid-turn follow-ups instead of injecting them live.
    pub fn is_tool_gated(&self) -> bool {
        !self.pending.lock().is_empty()
    }

    /// Send SIGINT to the child process to interrupt the current turn
    /// without killing it. First releases any held permission prompts (denied),
    /// so a turn paused on a question can unwind rather than hang.
    pub fn interrupt(&self) {
        let held: Vec<String> = self.pending.lock().drain().collect();
        for rid in held {
            let _ = write_line(&self.stdin, deny_response(&rid, "Interrupted by user"));
        }
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

/// Decide what to do with one `can_use_tool` request and act on it: auto-approve
/// ordinary tools immediately, or hold the question tools open for the user
/// (recording the `request_id` and surfacing the request to the UI). Non-tool
/// control requests (hook/mcp callbacks, which we never register) are
/// acknowledged so the agent never stalls waiting on us.
fn handle_control_request<F: Fn(Value)>(
    stdin: &Stdin,
    pending: &Arc<Mutex<HashSet<String>>>,
    on_event: &F,
    v: Value,
) {
    let request_id = v
        .get("request_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let req = v.get("request");
    let subtype = req
        .and_then(|r| r.get("subtype"))
        .and_then(Value::as_str)
        .unwrap_or_default();

    if subtype == "can_use_tool" {
        let tool = req
            .and_then(|r| r.get("tool_name"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if HOLD_TOOLS.contains(&tool) {
            // The real pause: don't respond. Record it and hand the request to
            // the UI, which answers via `answer_tool_use`.
            pending.lock().insert(request_id);
            on_event(v);
        } else {
            let input = req
                .and_then(|r| r.get("input"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            let _ = write_line(stdin, allow_response(&request_id, input));
        }
    } else {
        // hook_callback / mcp_message / unknown — we register neither hooks nor
        // SDK MCP servers, so this is unexpected; ack so the turn keeps moving.
        tracing::warn!(subtype, "managed: unexpected control_request, acking");
        let _ = write_line(
            stdin,
            json!({
                "type": "control_response",
                "response": { "subtype": "success", "request_id": request_id, "response": {} }
            }),
        );
    }
}

fn allow_response(request_id: &str, updated_input: Value) -> Value {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": { "behavior": "allow", "updatedInput": updated_input }
        }
    })
}

fn deny_response(request_id: &str, message: &str) -> Value {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": { "behavior": "deny", "message": message }
        }
    })
}

/// Serialize one stream-json envelope as a newline-delimited line to the
/// agent's stdin.
fn write_line(stdin: &Stdin, value: Value) -> Result<()> {
    let mut line = value.to_string();
    line.push('\n');

    let mut guard = stdin.lock();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_only_question_tools() {
        assert!(HOLD_TOOLS.contains(&"AskUserQuestion"));
        assert!(HOLD_TOOLS.contains(&"ExitPlanMode"));
        assert!(!HOLD_TOOLS.contains(&"Bash"));
        assert!(!HOLD_TOOLS.contains(&"Edit"));
    }

    #[test]
    fn stale_tool_answer_does_not_write() {
        let session = ManagedSession {
            child: Arc::new(Mutex::new(None)),
            stdin: Arc::new(Mutex::new(None)),
            pending: Arc::new(Mutex::new(HashSet::new())),
        };

        assert!(session
            .answer_tool_use(
                "already-drained",
                json!({}),
                ToolUseBehavior::Allow,
                None,
            )
            .is_ok());
    }

    #[test]
    fn tool_use_behavior_deserializes_from_ipc_strings() {
        assert_eq!(
            serde_json::from_value::<ToolUseBehavior>(json!("allow")).unwrap(),
            ToolUseBehavior::Allow
        );
        assert_eq!(
            serde_json::from_value::<ToolUseBehavior>(json!("deny")).unwrap(),
            ToolUseBehavior::Deny
        );
    }

    #[test]
    fn allow_response_shape() {
        let v = allow_response("req-1", json!({"questions": [], "answers": {"q": "a"}}));
        assert_eq!(v["type"], "control_response");
        assert_eq!(v["response"]["subtype"], "success");
        assert_eq!(v["response"]["request_id"], "req-1");
        assert_eq!(v["response"]["response"]["behavior"], "allow");
        assert_eq!(v["response"]["response"]["updatedInput"]["answers"]["q"], "a");
    }

    #[test]
    fn deny_response_shape() {
        let v = deny_response("req-2", "nope");
        assert_eq!(v["response"]["response"]["behavior"], "deny");
        assert_eq!(v["response"]["response"]["message"], "nope");
    }
}
