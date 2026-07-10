//! One-click installation of agent CLIs via their official native installer
//! scripts (no Node/npm required — each ships a standalone binary).
//!
//! The commands are pinned here rather than passed from the renderer, so the
//! UI can only ever trigger a known vendor installer. Each command mirrors
//! the copy-paste string shown as the manual fallback in the UI
//! (`src/data/providerDetail.ts`) — keep the two in sync.
//!
//! Installers drop binaries into the usual per-user dirs (`~/.local/bin`,
//! `~/.codex/bin`, …), all of which `bin_resolve` already scans, so a
//! post-install re-probe picks the new agent up with no extra wiring.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};

/// Ceiling on a single installer run. The scripts download ~50–150 MB and
/// finish in well under a minute on a normal connection; this only exists so
/// a hung curl doesn't hold the per-agent in-flight guard forever.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// The pinned official installer for an agent on this platform, if one
/// exists. Unix commands run through `bash -c`; Windows through PowerShell.
/// Agents without a scripted installer (antigravity, pi — and cursor/opencode
/// on Windows) return `None` and the UI falls back to their docs link.
pub fn install_command(id: &str) -> Option<&'static str> {
    if cfg!(windows) {
        match id {
            "claude" => Some("irm https://claude.ai/install.ps1 | iex"),
            "codex" => Some("irm https://chatgpt.com/codex/install.ps1 | iex"),
            _ => None,
        }
    } else {
        match id {
            "claude" => Some("curl -fsSL https://claude.ai/install.sh | bash"),
            "codex" => Some("curl -fsSL https://chatgpt.com/codex/install.sh | sh"),
            "cursor" => Some("curl -fsSL https://cursor.com/install | bash"),
            "opencode" => Some("curl -fsSL https://opencode.ai/install | bash"),
            _ => None,
        }
    }
}

/// Agents with an installer currently running — a second click on the same
/// tile (or the same agent from two windows) errors instead of racing two
/// installers over the same install dir.
fn in_flight() -> &'static Mutex<HashSet<String>> {
    static SET: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Removes the agent from the in-flight set on drop, so the error and timeout
/// paths can't leak a stuck "already installing" state.
struct InFlightGuard(String);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        in_flight().lock().unwrap().remove(&self.0);
    }
}

/// Run the pinned installer for `id`, streaming its output through `emit` as
/// `agent-install:state` payloads: `{id, phase: "running", line}` per output
/// line, then a final `{id, phase: "done"}` or `{id, phase: "failed", error}`.
/// Resolves when the installer exits; the caller re-probes to confirm the
/// binary actually appeared.
pub async fn install(
    id: String,
    emit: impl Fn(Value) + Send + Sync + 'static,
) -> Result<(), String> {
    let cmd = install_command(&id)
        .ok_or_else(|| format!("no scripted installer for `{id}` on this platform"))?;
    if !in_flight().lock().unwrap().insert(id.clone()) {
        return Err(format!("`{id}` installation is already in progress"));
    }
    let _guard = InFlightGuard(id.clone());
    let emit: Arc<dyn Fn(Value) + Send + Sync> = Arc::new(emit);

    emit(json!({ "id": id, "phase": "running", "line": format!("$ {cmd}") }));
    tracing::info!(agent = %id, %cmd, "running agent installer");

    let result = tokio::time::timeout(INSTALL_TIMEOUT, run_streamed(&id, cmd, emit.clone())).await;
    match result {
        Ok(Ok(())) => {
            emit(json!({ "id": id, "phase": "done" }));
            tracing::info!(agent = %id, "agent installer finished");
            Ok(())
        }
        Ok(Err(e)) => {
            tracing::warn!(agent = %id, error = %e, "agent installer failed");
            emit(json!({ "id": id, "phase": "failed", "error": e.clone() }));
            Err(e)
        }
        Err(_) => {
            let e = "installer timed out".to_string();
            tracing::warn!(agent = %id, "agent installer timed out");
            emit(json!({ "id": id, "phase": "failed", "error": e.clone() }));
            Err(e)
        }
    }
}

/// Spawn the installer and forward each output line to `emit`. stderr is
/// read alongside stdout — installer scripts write progress to both — and
/// the last non-empty line is kept for the error message when the exit
/// status is non-zero (curl and the vendor scripts put the reason there).
async fn run_streamed(
    id: &str,
    cmd: &str,
    emit: Arc<dyn Fn(Value) + Send + Sync>,
) -> Result<(), String> {
    use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

    let mut command = if cfg!(windows) {
        let mut c = tokio::process::Command::new("powershell");
        c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", cmd]);
        c
    } else {
        let mut c = tokio::process::Command::new("bash");
        c.args(["-c", cmd]);
        c
    };
    // GUI processes inherit launchd's sparse env; the installers expect a
    // normal user environment (PATH, HOME, proxy vars) to pick install dirs
    // and update shell profiles.
    if let Some(env) = crate::bin_resolve::login_shell_env() {
        command.envs(env);
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let mut child = command
        .spawn()
        .map_err(|e| format!("spawn installer: {e}"))?;

    let last_line = Arc::new(Mutex::new(String::new()));
    let streams: [Option<Box<dyn AsyncRead + Send + Unpin>>; 2] = [
        child.stdout.take().map(|s| Box::new(s) as _),
        child.stderr.take().map(|s| Box::new(s) as _),
    ];
    // JoinSet, not detached spawns: when the caller's timeout cancels this
    // future, dropping the set aborts the readers with it — otherwise they'd
    // keep emitting "running" lines after the terminal "failed" event while
    // the killed child's pipes drain.
    let mut readers = tokio::task::JoinSet::new();
    for stream in streams.into_iter().flatten() {
        let id = id.to_string();
        let emit = emit.clone();
        let last_line = last_line.clone();
        readers.spawn(async move {
            let mut lines = BufReader::new(stream).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                // Installer output is untrusted length-wise (progress bars
                // can be one huge line); cap what crosses the event bridge.
                let line: String = line.chars().take(400).collect();
                *last_line.lock().unwrap() = line.clone();
                emit(json!({ "id": id, "phase": "running", "line": line }));
            }
        });
    }
    while readers.join_next().await.is_some() {}

    let status = child
        .wait()
        .await
        .map_err(|e| format!("wait installer: {e}"))?;
    if status.success() {
        return Ok(());
    }
    let last = last_line.lock().unwrap().clone();
    Err(if last.is_empty() {
        format!("installer exited with {status}")
    } else {
        format!("installer exited with {status}: {last}")
    })
}
