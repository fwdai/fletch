//! The Docker sandbox engine and its primitives: availability probing, the
//! agent image, orphaned-container cleanup, and the launch path.
//!
//! Everything here must work when Docker is absent or the daemon is down:
//! probing reports that state instead of erroring, the startup sweep is
//! probe-gated so an install-less machine never pays for a docker
//! invocation, and `sandbox::engine_for` only routes launches here when the
//! probe says the daemon is up. "Down" and "absent" are distinguished
//! throughout: a down daemon is a state that flips (see
//! [`sweep_orphans_at_startup`], which waits one out), an absent binary is
//! treated as settled for the run.
//!
//! Layout — the Docker *runtime*, on top of the runtime-neutral policy in
//! [`crate::sandbox::container`]:
//! - [`cli`] — docker binary resolution + bounded-invocation wrappers. Every
//!   docker call in this module goes through it, so no invocation can hang
//!   the app on a wedged daemon.
//! - [`probe`] — cached daemon availability for UI polling.
//! - [`image`] — building, inspecting and reclaiming the agent images whose
//!   content lives in [`container::images`](crate::sandbox::container::images).
//! - [`cleanup`] — the dead-instance orphan sweep and the stale agent-image GC,
//!   keyed on [`container::labels`](crate::sandbox::container::labels).
//! - [`engine`] — `DockerEngine`, the `SandboxEngine` implementation
//!   (one `docker run --rm --init` container per agent process).

use std::time::{Duration, Instant};

use crate::sandbox::container::sweep::{SweepStep, SWEEP_RETRY_INTERVAL};

mod cleanup;
mod cli;
mod engine;
mod image;
mod probe;
pub mod setup_token;

pub use cleanup::remove_agent_containers;
pub use engine::{
    init_version_refresh_guard, set_launch_settings, DockerEngine, LaunchSettings, CPUS_SETTING,
    IMAGE_SETTING, MEMORY_SETTING, VERSION_GUARD_SETTING,
};
pub use probe::{availability, DockerAvailability};

/// The image-build progress sink, now runtime-neutral and shared with podman
/// (one sink, one toast — see
/// [`container::progress`](crate::sandbox::container::progress)). Re-exported so
/// the `sandbox::docker::set_build_sink` path `lib.rs` installs at startup keeps
/// resolving.
pub use crate::sandbox::container::progress::set_build_sink;

/// The container auth chain, now runtime-neutral. Re-exported so the
/// `sandbox::docker::auth::…` paths the app and its Tauri commands already use
/// keep resolving.
pub use crate::sandbox::container::auth;

/// The set of providers a container can run, under the name the rest of the app
/// knows it by. Docker was the first container runtime, so the type is spelled
/// [`ContainerProvider`](crate::sandbox::container::ContainerProvider) at its new
/// home and re-exported here.
pub use crate::sandbox::container::ContainerProvider as DockerProvider;

/// This runtime's display name in user-facing copy — the build toast's
/// [`BuildEvent::Started`](crate::sandbox::container::progress::BuildEvent) and
/// the reserved-exit-code messages.
pub(super) const RUNTIME_NAME: &str = "Docker";

/// Map a Docker probe result onto the shared retry schedule
/// ([`container::sweep`](crate::sandbox::container::sweep)). A missing binary
/// is settled for the run; a down daemon is the state we expect to flip.
fn sweep_step(availability: &DockerAvailability, elapsed: Duration) -> SweepStep {
    let usable = matches!(availability, DockerAvailability::Available { .. });
    let installed = !matches!(availability, DockerAvailability::NotInstalled);
    crate::sandbox::container::sweep::sweep_step(usable, installed, elapsed)
}

/// Best-effort reclamation of containers left behind by dead Fletch
/// instances — and of superseded agent images — for app startup (`lib.rs`,
/// next to the nested-root sweeps). Runs on its own thread, so startup never
/// waits on Docker — not even for the 2s probe timeout — and a machine without
/// Docker skips both sweeps entirely. The image sweep runs second: a removed
/// orphan container can unpin the stale image it was running. Both sweeps are
/// non-fatal by construction.
///
/// The thread probes on the [`sweep_step`] schedule rather than once, because
/// a single probe loses a race it usually can't win: Docker Desktop takes
/// 20-60s after login to answer, the probe times out at 2s, and a Fletch
/// launched from the dock or as a login item therefore reports `DaemonDown`
/// and skips both sweeps. These two sweeps are the *only* thing that ever
/// reclaims superseded agent images, so a user who loses that race every
/// launch accumulates dangling images forever — the failure mode behind a
/// report of 250GB of Docker disk usage. Retrying costs an idle thread and one
/// `docker version` round-trip per [`SWEEP_RETRY_INTERVAL`], bounded by
/// [`SWEEP_RETRY_BUDGET`]. The sweeps still run exactly once, on the first
/// probe that reports the daemon up.
pub fn sweep_orphans_at_startup() {
    std::thread::spawn(|| {
        let waiting_since = Instant::now();
        loop {
            match sweep_step(&probe::availability(), waiting_since.elapsed()) {
                SweepStep::Sweep => break,
                SweepStep::Retry => std::thread::sleep(SWEEP_RETRY_INTERVAL),
                SweepStep::Stop => {
                    tracing::debug!(
                        target: "fletch::docker",
                        waited_secs = waiting_since.elapsed().as_secs(),
                        "docker unavailable; skipping startup sweeps this run",
                    );
                    return;
                }
            }
        }
        match cleanup::sweep_orphans() {
            Ok(0) => {}
            Ok(n) => tracing::info!(removed = n, "swept orphaned fletch containers"),
            Err(e) => tracing::warn!(error = %e, "docker orphan sweep failed"),
        }
        match cleanup::sweep_stale_images() {
            Ok(0) => {}
            Ok(n) => tracing::info!(removed = n, "swept stale fletch agent images"),
            Err(e) => tracing::warn!(error = %e, "docker image sweep failed"),
        }
    });
}

/// Gate for the `#[ignore]`d integration tests: they touch a real Docker
/// daemon, so they run only when explicitly opted in via
/// `FLETCH_DOCKER_TESTS=1 cargo test -- --ignored`.
#[cfg(test)]
pub(crate) fn docker_tests_enabled() -> bool {
    std::env::var("FLETCH_DOCKER_TESTS").as_deref() == Ok("1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::container::sweep::SWEEP_RETRY_BUDGET;

    /// The startup sweep's retry schedule: an available daemon sweeps at any
    /// point in the window, a missing binary never retries, and a down daemon
    /// retries until — and only until — the budget is spent.
    #[test]
    fn sweep_retry_schedule() {
        let available = DockerAvailability::Available {
            server_version: "28.1.1".into(),
        };
        let zero = Duration::ZERO;

        assert_eq!(sweep_step(&available, zero), SweepStep::Sweep);
        assert_eq!(
            sweep_step(&available, SWEEP_RETRY_BUDGET * 2),
            SweepStep::Sweep,
            "a late daemon is still worth sweeping for on the probe that catches it",
        );

        assert_eq!(
            sweep_step(&DockerAvailability::NotInstalled, zero),
            SweepStep::Stop,
            "docker being installed mid-run isn't worth polling for",
        );

        assert_eq!(
            sweep_step(&DockerAvailability::DaemonDown, zero),
            SweepStep::Retry,
            "the first probe usually loses the Docker Desktop startup race",
        );
        assert_eq!(
            sweep_step(
                &DockerAvailability::DaemonDown,
                SWEEP_RETRY_BUDGET - SWEEP_RETRY_INTERVAL
            ),
            SweepStep::Retry,
        );
        // The budget is a deadline, not a suggestion: the loop is bounded.
        assert_eq!(
            sweep_step(&DockerAvailability::DaemonDown, SWEEP_RETRY_BUDGET),
            SweepStep::Stop,
        );
        assert_eq!(
            sweep_step(&DockerAvailability::DaemonDown, SWEEP_RETRY_BUDGET * 2),
            SweepStep::Stop,
        );
    }

    /// Polling faster than [`probe`]'s cache TTL would spend wakeups re-reading
    /// the same cached answer, so the interval must stay above it — and the
    /// budget must leave room for more than one retry to be worth having.
    #[test]
    fn sweep_retry_constants_are_sane() {
        assert!(SWEEP_RETRY_INTERVAL > Duration::from_secs(5));
        assert!(SWEEP_RETRY_BUDGET >= SWEEP_RETRY_INTERVAL * 2);
    }
}
