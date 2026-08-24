//! Locate the podman binary and run it with hard timeouts, over the
//! runtime-neutral machinery in
//! [`container::proc`](crate::sandbox::container::proc).
//!
//! Every podman invocation goes through here: a Finder-launched app's PATH
//! misses homebrew, and a suspended `podman machine` leaves a socket that
//! accepts and then stalls forever, so no call may be unbounded.

use std::process::{Command, Output, Stdio};
use std::time::Duration;

use crate::error::{Error, Result};
use crate::sandbox::container::proc::{forward_lines, run_with_timeout, wait_with_deadline};

/// Absolute path of the podman CLI, or `None` when it isn't installed.
/// Resolved fresh per call — caching a `None` would pin the probe to
/// `NotInstalled` for the whole run even after the user installs Podman.
pub(super) fn podman_bin() -> Option<std::path::PathBuf> {
    let home = dirs::home_dir()?;
    crate::bin_resolve::resolve_bin("podman", &home).map(std::path::PathBuf::from)
}

/// Run `podman <args>` on whichever connection is the default. Anything
/// belonging to one container's lifetime must use [`run_podman_on`] with that
/// container's pinned connection instead — the default can change mid-run.
pub(super) fn run_podman(args: &[&str], timeout: Duration) -> Result<Output> {
    run_podman_on(None, args, timeout)
}

/// Run `podman [--connection <name>] <args>` capturing stdout/stderr, failing
/// after `timeout`. Returns the raw `Output`: callers inspect the exit status
/// themselves, since several podman commands use non-zero exits as answers
/// (e.g. `image inspect` on a missing image), not as errors.
pub(super) fn run_podman_on(
    connection: Option<&str>,
    args: &[&str],
    timeout: Duration,
) -> Result<Output> {
    let bin = podman_bin()
        .ok_or_else(|| Error::Other("podman binary not found — is Podman installed?".into()))?;
    let mut cmd = Command::new(bin);
    // `--connection` is a global flag: it has to precede the subcommand.
    if let Some(connection) = connection {
        cmd.args(["--connection", connection]);
    }
    cmd.args(args);
    let what = format!("podman {}", args.first().copied().unwrap_or_default());
    run_with_timeout(cmd, timeout, &what)
}

/// Run `podman [--connection <name>] <args>` streaming every stdout/stderr line
/// to `on_line` as it appears, so a minutes-long build reaches the log while it
/// runs. Fails on non-zero exit with the last output lines in the message, or on
/// `timeout` expiry. `connection` matters here too: images live per-machine, so
/// a build has to land on the machine the run will use.
pub(super) fn run_podman_streaming(
    connection: Option<&str>,
    args: &[&str],
    timeout: Duration,
    on_line: &(dyn Fn(&str) + Send + Sync),
) -> Result<()> {
    let bin = podman_bin()
        .ok_or_else(|| Error::Other("podman binary not found — is Podman installed?".into()))?;
    let what = format!("podman {}", args.first().copied().unwrap_or_default());
    let mut cmd = Command::new(bin);
    if let Some(connection) = connection {
        cmd.args(["--connection", connection]);
    }
    let mut child = cmd
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().expect("stdout piped above");
    let stderr = child.stderr.take().expect("stderr piped above");

    // Bounded: podman's error lands near the end of the stream, so a tail
    // carries it without buffering an entire multi-minute build log.
    let tail = std::sync::Mutex::new(std::collections::VecDeque::<String>::new());

    // Both pipes must be drained continuously, or the child blocks on a full
    // pipe while we sit in the wait loop.
    let status = std::thread::scope(|scope| {
        scope.spawn(|| forward_lines(stdout, on_line, &tail));
        scope.spawn(|| forward_lines(stderr, on_line, &tail));
        wait_with_deadline(&mut child, timeout, &what)
    })?;

    if !status.success() {
        let tail = tail.lock().unwrap();
        return Err(Error::Other(format!(
            "{what} failed (exit {}):\n{}",
            status.code().unwrap_or(-1),
            tail.iter().cloned().collect::<Vec<_>>().join("\n"),
        )));
    }
    Ok(())
}
