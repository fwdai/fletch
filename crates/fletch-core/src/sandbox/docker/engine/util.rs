//! Docker's liveness lookups and its wording for the reserved exit codes.
//!
//! Container naming and the exit-code message templates are runtime-neutral and
//! live in [`container::util`](crate::sandbox::container::util); the liveness
//! lookups stay here because they shell out to docker.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::sandbox::container::util::{linux_host_user, ExitCopy};
use crate::sandbox::docker::cli;

/// Liveness lookups (`docker inspect`).
const INSPECT_TIMEOUT: Duration = Duration::from_secs(5);

/// `docker info` for the rootless check below. The daemon answers in
/// milliseconds; this is the same 2s cap the availability probe uses.
const INFO_TIMEOUT: Duration = Duration::from_secs(2);

/// `uid:gid` to launch containers as, or `None` to launch them as the image's
/// root — see
/// [`container::util::linux_host_user`](crate::sandbox::container::util::linux_host_user)
/// for why Linux needs one and macOS does not.
///
/// The one Linux case that must *not* be mapped is a **rootless** daemon: it
/// already maps the container's root to the user who started it, so files land
/// user-owned as they do on macOS, and a `--user 1000:1000` inside that
/// namespace would land on a subordinate uid the user cannot touch at all —
/// the very failure the mapping exists to prevent. Probed once per app run
/// (`SecurityOptions` carries `name=rootless`) and cached: it cannot change
/// without a daemon restart.
pub(super) fn launch_user() -> Option<&'static str> {
    static USER: OnceLock<Option<String>> = OnceLock::new();
    USER.get_or_init(|| {
        let user = linux_host_user()?;
        if daemon_is_rootless() {
            tracing::info!("rootless Docker daemon: containers keep the image's user");
            return None;
        }
        tracing::info!(user = %user, "Linux host: containers run as the service user");
        Some(user)
    })
    .as_deref()
}

/// Whether the daemon runs rootless. A daemon that cannot be reached reads as
/// *not* rootless: the standard install is rootful, the launch that follows
/// will fail on its own if the daemon really is down, and mapping is the safe
/// default for the case this cannot tell apart.
fn daemon_is_rootless() -> bool {
    match cli::run_docker(&["info", "-f", "{{.SecurityOptions}}"], INFO_TIMEOUT) {
        Ok(out) if out.status.success() => {
            String::from_utf8_lossy(&out.stdout).contains("name=rootless")
        }
        _ => false,
    }
}

/// Whether the daemon says the container is currently running. Errors
/// (container gone, daemon down, timeout) read as not running.
pub(super) fn container_running(name: &str) -> bool {
    match cli::run_docker(
        &["inspect", "-f", "{{.State.Running}}", name],
        INSPECT_TIMEOUT,
    ) {
        Ok(out) => out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true",
        Err(e) => {
            tracing::debug!(container = %name, error = %e, "docker inspect failed; treating as dead");
            false
        }
    }
}

/// Poll until the container stops running or `budget` elapses.
pub(super) fn container_gone_within(name: &str, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    loop {
        if !container_running(name) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Docker's wording for the shared reserved-exit-code messages. The daemon is
/// what reports a start failure, Docker Desktop is what the user restarts, and
/// `docker_image` is the override a 126/127 may be pointing at.
const EXIT_COPY: ExitCopy = ExitCopy {
    runtime: crate::sandbox::docker::RUNTIME_NAME,
    error_source: "the daemon",
    remedy: "Is Docker Desktop still running?",
    image_setting: Some(super::IMAGE_SETTING),
};

/// User-readable meanings for the docker CLI's reserved exit codes — see
/// [`container::util::describe_exit_code`](crate::sandbox::container::util::describe_exit_code).
pub(super) fn describe_exit_code(code: i32) -> Option<String> {
    crate::sandbox::container::util::describe_exit_code(code, &EXIT_COPY)
}
