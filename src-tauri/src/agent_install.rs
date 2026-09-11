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
//! Cancel and timeout both have to reach more than the process we spawned. On
//! Unix `curl … | bash` runs the download and the shell that executes it as
//! *children* of our `bash -c`, so killing only the leader would leave the
//! install to finish behind a UI that already said "cancelled". The installer
//! is therefore spawned as its own process-group leader, and both paths signal
//! the whole group and *wait* for it to be gone before emitting their terminal
//! event (see `run_installer` / `kill_group_and_reap`).
//!
//! `cancel` resolves on that same beat: it returns only once the run released
//! its in-flight slot, which happens after the group is confirmed dead. So the
//! instant the UI is told a run is cancelled, nothing of it is still
//! installing and a fresh install for that agent is already accepted — no
//! window where Retry either races the old installer or bounces off "already
//! in progress".

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::{watch, Notify};

/// Ceiling on a single installer run. The scripts download ~50–150 MB and
/// finish in well under a minute on a normal connection; this only exists so
/// a hung curl doesn't hold the per-agent in-flight guard forever.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Ceiling on how long a `cancel` waits for the run to finish tearing down.
/// The signal escalation is under a second, so reaching this means something
/// is badly stuck; the caller is freed rather than left hanging on the IPC.
const CANCEL_TIMEOUT: Duration = Duration::from_secs(15);

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

/// A live run, as seen from the outside: the way to ask it to stop, and the
/// way to learn that it has. `done` flips exactly when the run gives its
/// in-flight slot back, so "the cancel is complete" and "a new install is
/// accepted" are the same event and can never drift apart.
struct InFlight {
    cancel: Notify,
    done: watch::Sender<bool>,
}

/// Agents with an installer currently running. Double duty: a second click on
/// the same tile (or the same agent from two windows) errors instead of racing
/// two installers over the same install dir, and `cancel` reaches a live run
/// through the same entry.
fn in_flight() -> &'static Mutex<HashMap<String, Arc<InFlight>>> {
    static MAP: OnceLock<Mutex<HashMap<String, Arc<InFlight>>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Frees the agent's in-flight slot on drop, so the error, timeout and cancel
/// paths can't leak a stuck "already installing" state.
///
/// The single place the slot is released, and therefore the single place
/// `done` is signalled: a waiting `cancel` is woken by the very drop that lets
/// the next install in.
struct InFlightGuard {
    id: String,
    entry: Arc<InFlight>,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        in_flight().lock().unwrap().remove(&self.id);
        // `send_replace`, not `send`: a run nobody cancelled has no receivers,
        // and that is not an error.
        self.entry.done.send_replace(true);
    }
}

/// Stop the installer running for `id` and wait for it to be gone: the run
/// kills its whole process group — the `bash -c` leader *and* the
/// `curl … | bash` pipeline it spawned — emits `{id, phase: "cancelled"}`, and
/// only then releases its in-flight slot, which is what resolves this call. A
/// caller that awaits it can therefore offer Install/Retry immediately, with
/// no chance of overlapping the installer it just stopped.
///
/// Returns whether an install was actually in flight — cancelling an idle
/// agent is a no-op, not an error.
pub async fn cancel(id: &str) -> bool {
    let entry = {
        let running = in_flight().lock().unwrap();
        match running.get(id) {
            Some(entry) => entry.clone(),
            None => return false,
        }
    };
    // `notify_one`, not `notify_waiters`: it stores a permit when nothing is
    // waiting yet, so a cancel landing between registration and the first poll
    // of the run's select still takes effect.
    entry.cancel.notify_one();

    let mut done = entry.done.subscribe();
    // We hold the entry, so the sender outlives this wait and `wait_for` can
    // only fail by timing out.
    let torn_down = async { done.wait_for(|d| *d).await.is_ok() };
    match tokio::time::timeout(CANCEL_TIMEOUT, torn_down).await {
        Ok(true) => tracing::info!(agent = %id, "agent install cancelled"),
        Ok(false) | Err(_) => {
            tracing::warn!(agent = %id, "agent install cancel gave up waiting for teardown")
        }
    }
    true
}

/// Run the pinned installer for `id`, streaming its output through `emit` as
/// `agent-install:state` payloads: `{id, phase: "running", line}` per output
/// line, then a final `{id, phase: "done"}`, `{id, phase: "failed", error}` or
/// `{id, phase: "cancelled"}`. Resolves when the installer exits; the caller
/// re-probes to confirm the binary actually appeared.
///
/// A cancel or a timeout ends the run early, and the terminal event is emitted
/// only once the installer's whole process group is confirmed gone — nothing
/// keeps installing behind it.
pub async fn install(
    id: String,
    emit: impl Fn(Value) + Send + Sync + 'static,
) -> Result<(), String> {
    let cmd = install_command(&id)
        .ok_or_else(|| format!("no scripted installer for `{id}` on this platform"))?;
    install_with_command(id, cmd, emit).await
}

/// `install` with the command handed in rather than looked up, so tests can
/// drive the real in-flight/cancel/teardown machinery against a harmless
/// stand-in instead of a vendor download.
async fn install_with_command(
    id: String,
    cmd: &str,
    emit: impl Fn(Value) + Send + Sync + 'static,
) -> Result<(), String> {
    let entry = Arc::new(InFlight {
        cancel: Notify::new(),
        done: watch::channel(false).0,
    });
    {
        let mut running = in_flight().lock().unwrap();
        if running.contains_key(&id) {
            return Err(format!("`{id}` installation is already in progress"));
        }
        running.insert(id.clone(), entry.clone());
    }
    let _guard = InFlightGuard {
        id: id.clone(),
        entry: entry.clone(),
    };
    let emit: Arc<dyn Fn(Value) + Send + Sync> = Arc::new(emit);

    emit(json!({ "id": id, "phase": "running", "line": format!("$ {cmd}") }));
    tracing::info!(agent = %id, %cmd, "running agent installer");

    // Whatever ends the run, `run_installer` has already finished tearing the
    // process group down by the time it answers — so the event below is the
    // truth, and dropping `_guard` right after it is safe to act on.
    match run_installer(&id, cmd, emit.clone(), &entry.cancel).await {
        Outcome::Finished(Ok(())) => {
            emit(json!({ "id": id, "phase": "done" }));
            tracing::info!(agent = %id, "agent installer finished");
            Ok(())
        }
        Outcome::Finished(Err(e)) => {
            tracing::warn!(agent = %id, error = %e, "agent installer failed");
            emit(json!({ "id": id, "phase": "failed", "error": e.clone() }));
            Err(e)
        }
        Outcome::Cancelled => {
            tracing::info!(agent = %id, "agent installer cancelled");
            emit(json!({ "id": id, "phase": "cancelled" }));
            Ok(())
        }
        Outcome::TimedOut => {
            let e = "installer timed out".to_string();
            tracing::warn!(agent = %id, "agent installer timed out");
            emit(json!({ "id": id, "phase": "failed", "error": e.clone() }));
            Err(e)
        }
    }
}

/// How a run ended. `Cancelled` and `TimedOut` are only produced once the
/// installer's process group has been torn down, so `install` can turn them
/// straight into a terminal event.
enum Outcome {
    Finished(Result<(), String>),
    Cancelled,
    TimedOut,
}

/// Spawn the installer and forward each output line to `emit`. stderr is
/// read alongside stdout — installer scripts write progress to both — and
/// the last non-empty line is kept for the error message when the exit
/// status is non-zero (curl and the vendor scripts put the reason there).
///
/// Owns the whole lifetime of the child, which is why cancel and timeout are
/// raced *here* rather than around the call: both need the pgid and the child
/// handle to tear the run down and then wait for that teardown to finish.
async fn run_installer(
    id: &str,
    cmd: &str,
    emit: Arc<dyn Fn(Value) + Send + Sync>,
    cancel: &Notify,
) -> Outcome {
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

    let (mut child, mut group) = match spawn_installer_group(command) {
        Ok(pair) => pair,
        Err(e) => return Outcome::Finished(Err(format!("spawn installer: {e}"))),
    };

    let last_line = Arc::new(Mutex::new(String::new()));
    let streams: [Option<Box<dyn AsyncRead + Send + Unpin>>; 2] = [
        child.stdout.take().map(|s| Box::new(s) as _),
        child.stderr.take().map(|s| Box::new(s) as _),
    ];
    // JoinSet, not detached spawns: the teardown below shuts it down, so no
    // reader can keep emitting "running" lines after the terminal event while
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
    // The only place the three ways a run can end meet. Borrows of `child` and
    // `readers` end with the select, leaving both usable for the teardown.
    enum Ended {
        Exited(std::io::Result<std::process::ExitStatus>),
        Cancelled,
        TimedOut,
    }
    let ended = tokio::select! {
        status = async {
            while readers.join_next().await.is_some() {}
            child.wait().await
        } => Ended::Exited(status),
        _ = cancel.notified() => Ended::Cancelled,
        _ = tokio::time::sleep(INSTALL_TIMEOUT) => Ended::TimedOut,
    };

    let status = match ended {
        Ended::Exited(status) => status,
        // Readers first — they must stop emitting before the terminal event —
        // then the group, awaited, so we answer only with the install dead.
        ended => {
            readers.shutdown().await;
            kill_group_and_reap(&mut group, &mut child).await;
            return match ended {
                Ended::Cancelled => Outcome::Cancelled,
                _ => Outcome::TimedOut,
            };
        }
    };

    let status = match status {
        Ok(status) => status,
        Err(e) => return Outcome::Finished(Err(format!("wait installer: {e}"))),
    };
    // The installer is reaped, so the kernel may hand its pid — which is the
    // pgid — to someone else at any moment: stand the guard down. A `wait`
    // that errors instead leaves it armed, since then nothing was reaped.
    group.disarm();
    if status.success() {
        return Outcome::Finished(Ok(()));
    }
    let last = last_line.lock().unwrap().clone();
    Outcome::Finished(Err(if last.is_empty() {
        format!("installer exited with {status}")
    } else {
        format!("installer exited with {status}: {last}")
    }))
}

/// Signal the installer's whole process group and return only once it is gone.
///
/// The kill and the reap run together on purpose: our leader is itself a
/// member of the group and an unreaped zombie still answers `killpg`, so the
/// escalation's poll could never see the group empty if we waited for it
/// first. The escalation sleeps between signals, so it goes to a blocking
/// thread rather than stalling a runtime worker — including the one tokio
/// needs to reap the leader we just killed.
#[cfg(unix)]
async fn kill_group_and_reap(group: &mut GroupKill, child: &mut tokio::process::Child) {
    let pgid = group.pgid;
    let (killed, reaped) = tokio::join!(
        tokio::task::spawn_blocking(move || crate::pty_session::kill_process_group(pgid)),
        child.wait(),
    );
    match killed {
        Ok(Ok(())) => {}
        // Signals were delivered; only the confirming probe failed — typically
        // an EPERM on a member we can't signal.
        Ok(Err(e)) => {
            tracing::warn!(pgid = pgid.as_raw(), error = %e, "installer group teardown unconfirmed")
        }
        Err(e) => tracing::warn!(pgid = pgid.as_raw(), error = %e, "installer group kill panicked"),
    }
    // Same rule as the clean exit: disarm only once something was actually
    // reaped, so a recycled pgid can never be signalled by a stale guard.
    if reaped.is_ok() {
        group.disarm();
    }
}

/// See `spawn_installer_group`: on Windows the installer runs the downloaded
/// script inside the process we spawned, so killing that one process — and
/// waiting for it — is the whole teardown.
#[cfg(not(unix))]
async fn kill_group_and_reap(group: &mut GroupKill, child: &mut tokio::process::Child) {
    let _ = child.kill().await;
    group.disarm();
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

/// Kills the installer's process group when dropped, because `kill_on_drop`
/// alone would only reach the `bash -c` leader and leave its `curl … | bash`
/// pipeline installing.
///
/// Armed at spawn and disarmed once the child has been waited for, so a pgid
/// the kernel later recycles can never be signalled by a stale guard.
///
/// NOT the normal teardown path any more: cancel and timeout both go through
/// `kill_group_and_reap`, which disarms this guard, so that they can *wait*
/// for the group to be gone before telling the UI the run is over. What is
/// left here is the last resort for drops nobody planned — a panic between
/// spawn and teardown, or the runtime going away underneath the run — where
/// there is no one to await and a detached thread is the only option.
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
        // the steps while it polls. A drop can't await, and it may well be
        // running on a runtime worker (or during runtime shutdown), so the
        // blocking escalation goes to a thread of its own rather than stalling
        // whatever is tearing us down.
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

    /// A shell that backgrounds a `sleep` (same process group — non-interactive
    /// bash has no job control) and blocks on `wait`, mirroring how
    /// `curl … | bash` leaves work running under the leader. `kill_on_drop`
    /// alone would leave the `sleep` behind, exactly the way a cancelled
    /// install used to keep installing, so its death proves the whole group
    /// was signalled.
    fn installer_with_descendant(pidfile: &std::path::Path) -> String {
        format!("sleep 30 & echo $! > '{}'; wait", pidfile.display())
    }

    /// Block until the stand-in has written its descendant's pid, so a cancel
    /// can't land before there is anything to kill.
    async fn await_descendant(pidfile: &std::path::Path) -> nix::unistd::Pid {
        let start = std::time::Instant::now();
        loop {
            if let Ok(pid) = std::fs::read_to_string(pidfile)
                .unwrap_or_default()
                .trim()
                .parse::<i32>()
            {
                return nix::unistd::Pid::from_raw(pid);
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "backgrounded child never started"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Collects the `agent-install:state` payloads a run emits.
    #[derive(Clone, Default)]
    struct Events(Arc<Mutex<Vec<Value>>>);

    impl Events {
        fn sink(&self) -> impl Fn(Value) + Send + Sync + 'static {
            let seen = self.0.clone();
            move |payload| seen.lock().unwrap().push(payload)
        }

        fn phases(&self) -> Vec<String> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .filter_map(|e| e["phase"].as_str().map(str::to_owned))
                .collect()
        }
    }

    /// The guarantee `cancel` makes to the UI: once it answers, the install is
    /// *over* — its terminal event is out, nothing it started is still alive,
    /// and the agent's slot is free for the Retry the user can now see. The
    /// three assertions are the three ways that used to be false while the
    /// teardown ran on a detached thread.
    #[tokio::test]
    async fn cancel_resolves_only_once_the_group_is_dead_and_the_slot_is_free() {
        use nix::errno::Errno;

        // `in_flight` is process-global; a unique id keeps tests independent.
        let id = "test-cancel-awaits-teardown".to_string();
        let td = tempfile::tempdir().unwrap();
        let pidfile = td.path().join("descendant.pid");
        let script = installer_with_descendant(&pidfile);

        let events = Events::default();
        let run = tokio::spawn({
            let (id, sink) = (id.clone(), events.sink());
            async move { install_with_command(id, &script, sink).await }
        });
        let descendant = await_descendant(&pidfile).await;

        assert!(cancel(&id).await, "an install was in flight");

        assert!(
            events.phases().contains(&"cancelled".to_string()),
            "the terminal event is out before the cancel resolves, got {:?}",
            events.phases()
        );
        assert_eq!(
            nix::sys::signal::kill(descendant, None),
            Err(Errno::ESRCH),
            "descendant {descendant} outlived the cancel that acknowledged it"
        );

        // The slot is free: a fresh install for the same agent is accepted and
        // runs to completion rather than bouncing off "already in progress".
        let retry = Events::default();
        install_with_command(id, "true", retry.sink())
            .await
            .expect("a retry right after the cancel is accepted");
        assert_eq!(retry.phases(), vec!["running", "done"]);

        assert_eq!(run.await.unwrap(), Ok(()));
    }

    /// The last-resort path: an unplanned drop still takes the descendants
    /// with it. Nothing awaits here, so the check is a bounded poll rather
    /// than an immediate assertion — that laxness is exactly why cancel and
    /// timeout no longer go this way.
    #[tokio::test]
    async fn dropping_the_guard_kills_descendants_not_just_the_leader() {
        use nix::errno::Errno;
        use std::time::Instant;

        let td = tempfile::tempdir().unwrap();
        let pidfile = td.path().join("descendant.pid");
        let script = installer_with_descendant(&pidfile);

        let mut command = tokio::process::Command::new("bash");
        command
            .args(["-c", &script])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let (child, group) = spawn_installer_group(command).unwrap();

        let descendant = await_descendant(&pidfile).await;

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
