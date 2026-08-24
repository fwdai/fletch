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

use std::process::{Command, Output};
use std::time::Duration;

use crate::error::{Error, Result};
use crate::sandbox::container::proc::run_with_timeout;

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
