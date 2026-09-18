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
/// the very failure the mapping exists to prevent.
///
/// The probe (`SecurityOptions` carries `name=rootless`) runs at most once per
/// app run, because the answer cannot change without a daemon restart — but
/// only an *answer* is cached. A `docker info` that times out or errors is not
/// one, and caching it as "rootful" would pin `--user` for the whole process
/// lifetime on a daemon that is in fact rootless (see [`decide_launch_user`]).
pub(super) fn launch_user() -> Option<String> {
    static USER: OnceLock<Option<String>> = OnceLock::new();
    if let Some(cached) = USER.get() {
        return cached.clone();
    }
    // Nothing to probe where there is nothing to map: this is a `cfg` test, so
    // macOS pays neither the probe nor the cache.
    let host_user = linux_host_user()?;
    let decided = decide_launch_user(host_user, daemon_is_rootless());
    if decided.cacheable {
        let _ = USER.set(decided.user.clone());
    }
    decided.user
}

/// One launch's mapping decision: the `--user` value (or `None` for the image's
/// own user), and whether it is an answer worth keeping.
struct LaunchUser {
    user: Option<String>,
    cacheable: bool,
}

/// What a rootless probe means for the mapping. Pure, so the cache policy is
/// testable without a daemon.
///
/// `Ok` is a definite answer about a daemon that cannot change what it is
/// without restarting, so it is cached for the process. `Err` is not an answer
/// at all — a `docker info` timeout, a daemon mid-restart, a CLI that is not
/// there yet — so fall back to the standard install (rootful, so map) for
/// *this* launch only and let the next one probe again. Mapping is the right
/// guess to make: on a rootful daemon it is what keeps the agent's files
/// user-owned, and a launch against a daemon that is really down fails on its
/// own either way.
fn decide_launch_user(host_user: String, rootless: Result<bool, String>) -> LaunchUser {
    match rootless {
        Ok(true) => {
            tracing::info!("rootless Docker daemon: containers keep the image's user");
            LaunchUser {
                user: None,
                cacheable: true,
            }
        }
        Ok(false) => {
            tracing::info!(user = %host_user, "Linux host: containers run as the service user");
            LaunchUser {
                user: Some(host_user),
                cacheable: true,
            }
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                user = %host_user,
                "docker info did not say whether the daemon is rootless; \
                 mapping this launch as if it were rootful and re-probing on the next one",
            );
            LaunchUser {
                user: Some(host_user),
                cacheable: false,
            }
        }
    }
}

/// Whether the daemon runs rootless, or why the question went unanswered.
fn daemon_is_rootless() -> Result<bool, String> {
    match cli::run_docker(&["info", "-f", "{{.SecurityOptions}}"], INFO_TIMEOUT) {
        Ok(out) if out.status.success() => {
            Ok(String::from_utf8_lossy(&out.stdout).contains("name=rootless"))
        }
        Ok(out) => Err(format!(
            "docker info exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Err(e) => Err(e.to_string()),
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

#[cfg(test)]
mod tests {
    use super::*;

    const USER: &str = "1000:1000";

    /// A daemon that answered cannot change what it is without a restart, so
    /// both answers are the process's to keep.
    #[test]
    fn an_answered_probe_is_cached() {
        let rootful = decide_launch_user(USER.to_string(), Ok(false));
        assert_eq!(rootful.user.as_deref(), Some(USER));
        assert!(rootful.cacheable);

        let rootless = decide_launch_user(USER.to_string(), Ok(true));
        assert_eq!(rootless.user, None);
        assert!(rootless.cacheable);
    }

    /// A failed probe is not an answer. Map for this launch — the standard
    /// install is rootful — but never cache it: a `docker info` that timed out
    /// while the daemon was starting would otherwise pin `--user` for the
    /// process lifetime on a rootless daemon, where the mapped uid is one the
    /// user cannot touch.
    #[test]
    fn a_failed_probe_maps_for_this_launch_only() {
        let decided = decide_launch_user(USER.to_string(), Err("timed out after 2s".into()));
        assert_eq!(decided.user.as_deref(), Some(USER));
        assert!(
            !decided.cacheable,
            "a probe failure must not pin the mapping for the process lifetime"
        );
    }
}
