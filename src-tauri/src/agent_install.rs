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
//!
//! Cancel and timeout both tear a run down by dropping its future. On Unix
//! that has to reach more than the process we spawned: `curl … | bash` runs
//! the download and the shell that executes it as *children* of our `bash -c`,
//! so killing only the leader would leave the install to finish behind a UI
//! that already said "cancelled". The installer is therefore spawned as its
//! own process-group leader and the whole group is signalled on teardown (see
//! `spawn_installer_group` / `GroupKill`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::Notify;

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

/// Agents with an installer currently running, each mapped to its cancel
/// signal. Double duty: a second click on the same tile (or the same agent
/// from two windows) errors instead of racing two installers over the same
/// install dir, and `cancel` reaches a live run through the same entry.
fn in_flight() -> &'static Mutex<HashMap<String, Arc<Notify>>> {
    static MAP: OnceLock<Mutex<HashMap<String, Arc<Notify>>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Removes the agent from the in-flight map on drop, so the error, timeout and
/// cancel paths can't leak a stuck "already installing" state.
struct InFlightGuard(String);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        in_flight().lock().unwrap().remove(&self.0);
    }
}

/// Stop the installer running for `id`: the run's own future is dropped, which
/// kills the installer's whole process group — the `bash -c` leader *and* the
/// `curl … | bash` pipeline it spawned — and emits `{id, phase: "cancelled"}`.
/// Returns whether an install was actually in flight — cancelling an idle agent
/// is a no-op, not an error.
pub fn cancel(id: &str) -> bool {
    let Some(signal) = in_flight().lock().unwrap().get(id).cloned() else {
        return false;
    };
    // `notify_one`, not `notify_waiters`: it stores a permit when nothing is
    // waiting yet, so a cancel landing between registration and the first poll
    // of the select below still takes effect.
    signal.notify_one();
    true
}

/// Run the pinned installer for `id`, streaming its output through `emit` as
/// `agent-install:state` payloads: `{id, phase: "running", line}` per output
/// line, then a final `{id, phase: "done"}`, `{id, phase: "failed", error}` or
/// `{id, phase: "cancelled"}`. Resolves when the installer exits; the caller
/// re-probes to confirm the binary actually appeared.
///
/// A cancel or a timeout resolves early and takes the installer's whole
/// process group with it, so nothing keeps installing behind the terminal
/// event.
pub async fn install(
    id: String,
    emit: impl Fn(Value) + Send + Sync + 'static,
) -> Result<(), String> {
    let cmd = install_command(&id)
        .ok_or_else(|| format!("no scripted installer for `{id}` on this platform"))?;
    let cancelled = Arc::new(Notify::new());
    {
        let mut running = in_flight().lock().unwrap();
        if running.contains_key(&id) {
            return Err(format!("`{id}` installation is already in progress"));
        }
        running.insert(id.clone(), cancelled.clone());
    }
    let _guard = InFlightGuard(id.clone());
    let emit: Arc<dyn Fn(Value) + Send + Sync> = Arc::new(emit);

    emit(json!({ "id": id, "phase": "running", "line": format!("$ {cmd}") }));
    tracing::info!(agent = %id, %cmd, "running agent installer");

    // A cancel drops the `run_streamed` future, and with it the reader tasks,
    // the process-group guard that reaps the installer's whole pipeline and the
    // child itself — the same teardown the timeout below gets for free.
    let run = async {
        tokio::select! {
            result = run_streamed(&id, cmd, emit.clone()) => Some(result),
            _ = cancelled.notified() => None,
        }
    };
    match tokio::time::timeout(INSTALL_TIMEOUT, run).await {
        Ok(Some(Ok(()))) => {
            emit(json!({ "id": id, "phase": "done" }));
            tracing::info!(agent = %id, "agent installer finished");
            Ok(())
        }
        Ok(Some(Err(e))) => {
            tracing::warn!(agent = %id, error = %e, "agent installer failed");
            emit(json!({ "id": id, "phase": "failed", "error": e.clone() }));
            Err(e)
        }
        Ok(None) => {
            tracing::info!(agent = %id, "agent installer cancelled");
            emit(json!({ "id": id, "phase": "cancelled" }));
            Ok(())
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

    // Bindings drop in reverse, so a cancel/timeout unwinds this as readers
    // (declared below — they stop emitting), then the guard (signals the whole
    // pipeline while the leader is still ours, i.e. unreaped), then the child.
    let (mut child, mut group) =
        spawn_installer_group(command).map_err(|e| format!("spawn installer: {e}"))?;

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
    // The installer is reaped, so the kernel may hand its pid — which is the
    // pgid — to someone else at any moment: stand the guard down. A `wait`
    // that errors instead leaves it armed, since then nothing was reaped.
    group.disarm();
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

/// Spawn `command` together with the guard that reaps everything it starts.
///
/// Unix: `process_group(0)` makes the child `setpgid(0, 0)` itself, so its pid
/// *is* the pgid and every descendant — the `curl`, the `bash` on the far end
/// of the pipe — inherits it. One `killpg` then reaches the whole install.
/// `kill_on_drop` stays on as belt-and-braces for the leader.
///
/// Windows: the pinned installers are `irm … | iex`, which runs the downloaded
/// script *inside* the PowerShell process, so killing that one process is
/// killing the install and `kill_on_drop` already does it. Job objects would
/// only be needed for an installer that forks, and none of the pinned ones do.
///
/// Split out of `run_streamed` so a test can drive the exact spawn/teardown
/// pair production uses against a harmless command.
#[cfg(unix)]
fn spawn_installer_group(
    mut command: tokio::process::Command,
) -> std::io::Result<(tokio::process::Child, GroupKill)> {
    use std::os::unix::process::CommandExt;

    command.as_std_mut().process_group(0);
    let child = command.spawn()?;
    let pid = child
        .id()
        .ok_or_else(|| std::io::Error::other("installer had no pid right after spawn"))?;
    let guard = GroupKill {
        pgid: nix::unistd::Pid::from_raw(pid as i32),
        armed: true,
    };
    Ok((child, guard))
}

#[cfg(not(unix))]
fn spawn_installer_group(
    mut command: tokio::process::Command,
) -> std::io::Result<(tokio::process::Child, GroupKill)> {
    Ok((command.spawn()?, GroupKill))
}

/// Kills the installer's process group when dropped — which is what makes
/// cancel and timeout honest, since both tear the run down by dropping its
/// future and `kill_on_drop` alone would only reach the `bash -c` leader.
///
/// Armed at spawn and disarmed once the child has been waited for, so a pgid
/// the kernel later recycles can never be signalled by a stale guard.
#[cfg(unix)]
struct GroupKill {
    pgid: nix::unistd::Pid,
    armed: bool,
}

#[cfg(unix)]
impl GroupKill {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

#[cfg(unix)]
impl Drop for GroupKill {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let pgid = self.pgid;
        // `kill_process_group` escalates HUP → TERM → KILL and sleeps between
        // the steps while it polls, so it runs on its own thread: this drop
        // happens on a runtime worker (the cancelled future's), and blocking it
        // would stall the runtime — and stop tokio from reaping the leader we
        // just killed, which is what the poll is waiting to see.
        std::thread::spawn(move || {
            if let Err(e) = crate::pty_session::kill_process_group(pgid) {
                // Signals were delivered; only the confirming probe failed —
                // typically because our own leader is still an unreaped zombie,
                // which keeps the group "alive" for a moment longer.
                tracing::debug!(pgid = pgid.as_raw(), error = %e, "installer group teardown unconfirmed");
            }
        });
    }
}

/// See `spawn_installer_group`: on Windows the installer has no descendants to
/// reap, so the guard is an inert placeholder that keeps `run_streamed` common.
#[cfg(not(unix))]
struct GroupKill;

#[cfg(not(unix))]
impl GroupKill {
    fn disarm(&mut self) {}
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// The installer stand-in: a shell that backgrounds a `sleep` (same process
    /// group — non-interactive bash has no job control) and blocks on `wait`,
    /// mirroring how `curl … | bash` leaves work running under the leader.
    /// `kill_on_drop` alone would leave the `sleep` behind, exactly the way a
    /// cancelled install used to keep installing, so its death proves the whole
    /// group was signalled.
    #[tokio::test]
    async fn dropping_the_guard_kills_descendants_not_just_the_leader() {
        use nix::errno::Errno;
        use std::time::Instant;

        let td = tempfile::tempdir().unwrap();
        let pidfile = td.path().join("descendant.pid");
        let script = format!("sleep 30 & echo $! > '{}'; wait", pidfile.display());

        let mut command = tokio::process::Command::new("bash");
        command
            .args(["-c", &script])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let (child, group) = spawn_installer_group(command).unwrap();

        let start = Instant::now();
        let descendant = loop {
            if let Ok(pid) = std::fs::read_to_string(&pidfile)
                .unwrap_or_default()
                .trim()
                .parse::<i32>()
            {
                break nix::unistd::Pid::from_raw(pid);
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "backgrounded child never started"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        };

        // The teardown a cancel or a timeout performs, in the same order.
        drop(group);
        drop(child);

        let start = Instant::now();
        while !matches!(nix::sys::signal::kill(descendant, None), Err(Errno::ESRCH)) {
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "descendant {descendant} outlived the cancelled installer"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
