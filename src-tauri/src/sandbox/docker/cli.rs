//! Locate the docker binary and run it with hard timeouts.
//!
//! Two rules every docker invocation in the app must follow, enforced by
//! funneling them through this module:
//!
//! 1. **Resolve the binary like a GUI app.** Docker Desktop symlinks the CLI
//!    into `/usr/local/bin`, which a Finder-launched Tauri app's PATH may not
//!    include — `bin_resolve::resolve_bin` handles that (its common-dirs
//!    fallback already covers `/usr/local/bin`).
//! 2. **Bound every call.** A stopped Docker Desktop leaves a socket that
//!    accepts connections and then hangs; an unbounded `docker` call would
//!    wedge whatever thread issued it (UI polling, startup sweep). Callers
//!    pass an explicit timeout and get a clear "timed out" error instead. The
//!    bounding machinery itself is runtime-neutral and lives in
//!    [`container::proc`](crate::sandbox::container::proc); what's here is the
//!    thin Docker-specific layer over it.

use std::process::{Command, Output, Stdio};
use std::time::Duration;

use crate::error::{Error, Result};
use crate::sandbox::container::proc::{forward_lines, run_with_timeout, wait_with_deadline};

/// Absolute path of the docker CLI, or `None` when it isn't installed.
/// Resolved fresh on every call (the underlying login-shell env is cached, so
/// this is just a stat walk): caching a `None` here would pin the probe to
/// `NotInstalled` for the whole app run even after the user installs Docker,
/// and the probe's own 5s cache already bounds the frequency.
pub(super) fn docker_bin() -> Option<std::path::PathBuf> {
    let home = dirs::home_dir()?;
    crate::bin_resolve::resolve_bin("docker", &home).map(std::path::PathBuf::from)
}

/// Run `docker <args>` capturing stdout/stderr, failing after `timeout`.
/// Returns the raw `Output` — callers inspect the exit status themselves,
/// since several docker commands use non-zero exits as answers (e.g.
/// `image inspect` on a missing image), not as errors.
pub(super) fn run_docker(args: &[&str], timeout: Duration) -> Result<Output> {
    let bin = docker_bin()
        .ok_or_else(|| Error::Other("docker binary not found — is Docker installed?".into()))?;
    let mut cmd = Command::new(bin);
    cmd.args(args);
    let what = format!("docker {}", args.first().copied().unwrap_or_default());
    run_with_timeout(cmd, timeout, &what)
}

/// Run `docker <args>` streaming every output line (stdout and stderr) to
/// `on_line` as it appears — the shape `docker build` needs so image-build
/// progress can reach the UI. Fails on non-zero exit with the last output
/// lines in the message, or on `timeout` expiry.
pub(super) fn run_docker_streaming(
    args: &[&str],
    timeout: Duration,
    on_line: &(dyn Fn(&str) + Send + Sync),
) -> Result<()> {
    let bin = docker_bin()
        .ok_or_else(|| Error::Other("docker binary not found — is Docker installed?".into()))?;
    let what = format!("docker {}", args.first().copied().unwrap_or_default());
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().expect("stdout piped above");
    let stderr = child.stderr.take().expect("stderr piped above");

    // Keep a bounded tail of everything seen, so a failure message carries
    // the actual docker error (which lands near the end of the stream)
    // without buffering an entire multi-minute build log.
    let tail = std::sync::Mutex::new(std::collections::VecDeque::<String>::new());

    // Scoped threads: the readers borrow `on_line` and `tail`, and both
    // pipes are drained continuously so the child can never block on a full
    // pipe while we sit in the wait loop below.
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
