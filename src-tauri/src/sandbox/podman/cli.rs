//! Locate the podman binary and run it with hard timeouts.
//!
//! The same two rules as [`docker::cli`](crate::sandbox::docker::cli), enforced
//! by funneling every podman invocation through this module:
//!
//! 1. **Resolve the binary like a GUI app.** Podman installs the CLI into
//!    `/opt/homebrew/bin` or `/usr/local/bin`, neither of which a
//!    Finder-launched Tauri app's PATH reliably includes —
//!    `bin_resolve::resolve_bin` handles that (its common-dirs fallback already
//!    covers both).
//! 2. **Bound every call.** Podman has no daemon on macOS, but it does talk to a
//!    `podman machine` VM over a socket, and a VM that is suspended or
//!    mid-shutdown leaves a socket that accepts and then stalls — the same hazard
//!    a stopped Docker Desktop poses. Callers pass an explicit timeout and get a
//!    clear "timed out" error instead. The bounding machinery is runtime-neutral
//!    and lives in [`container::proc`](crate::sandbox::container::proc); what's
//!    here is the thin Podman-specific layer over it.

use std::process::{Command, Output, Stdio};
use std::time::Duration;

use crate::error::{Error, Result};
use crate::sandbox::container::proc::{forward_lines, run_with_timeout, wait_with_deadline};

/// Absolute path of the podman CLI, or `None` when it isn't installed.
/// Resolved fresh on every call (the underlying login-shell env is cached, so
/// this is just a stat walk): caching a `None` here would pin the probe to
/// `NotInstalled` for the whole app run even after the user installs Podman,
/// and the probe's own 5s cache already bounds the frequency.
pub(super) fn podman_bin() -> Option<std::path::PathBuf> {
    let home = dirs::home_dir()?;
    crate::bin_resolve::resolve_bin("podman", &home).map(std::path::PathBuf::from)
}

/// Run `podman <args>` capturing stdout/stderr, failing after `timeout`.
/// Returns the raw `Output` — callers inspect the exit status themselves, since
/// several podman commands use non-zero exits as answers (e.g. `image inspect`
/// on a missing image), not as errors.
pub(super) fn run_podman(args: &[&str], timeout: Duration) -> Result<Output> {
    let bin = podman_bin()
        .ok_or_else(|| Error::Other("podman binary not found — is Podman installed?".into()))?;
    let mut cmd = Command::new(bin);
    cmd.args(args);
    let what = format!("podman {}", args.first().copied().unwrap_or_default());
    run_with_timeout(cmd, timeout, &what)
}

/// Run `podman <args>` streaming every output line (stdout and stderr) to
/// `on_line` as it appears — the shape `podman build` needs so a minutes-long
/// image build reaches the log while it runs. Fails on non-zero exit with the
/// last output lines in the message, or on `timeout` expiry.
pub(super) fn run_podman_streaming(
    args: &[&str],
    timeout: Duration,
    on_line: &(dyn Fn(&str) + Send + Sync),
) -> Result<()> {
    let bin = podman_bin()
        .ok_or_else(|| Error::Other("podman binary not found — is Podman installed?".into()))?;
    let what = format!("podman {}", args.first().copied().unwrap_or_default());
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().expect("stdout piped above");
    let stderr = child.stderr.take().expect("stderr piped above");

    // Keep a bounded tail of everything seen, so a failure message carries the
    // actual podman error (which lands near the end of the stream) without
    // buffering an entire multi-minute build log.
    let tail = std::sync::Mutex::new(std::collections::VecDeque::<String>::new());

    // Scoped threads: the readers borrow `on_line` and `tail`, and both pipes
    // are drained continuously so the child can never block on a full pipe
    // while we sit in the wait loop below.
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
