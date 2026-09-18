//! Docker's liveness lookups and its wording for the reserved exit codes.
//!
//! Container naming and the exit-code message templates are runtime-neutral and
//! live in [`container::util`](crate::sandbox::container::util); the liveness
//! lookups stay here because they shell out to docker.

use std::time::{Duration, Instant};

use crate::sandbox::container::util::ExitCopy;
use crate::sandbox::docker::cli;

/// Liveness lookups (`docker inspect`).
const INSPECT_TIMEOUT: Duration = Duration::from_secs(5);

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
