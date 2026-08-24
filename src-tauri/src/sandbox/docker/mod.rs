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

mod cleanup;
mod cli;
mod engine;
mod image;
mod probe;
mod progress;
pub mod setup_token;

pub use cleanup::remove_agent_containers;
pub use engine::{
    init_version_refresh_guard, set_launch_settings, DockerEngine, LaunchSettings, CPUS_SETTING,
    IMAGE_SETTING, MEMORY_SETTING, VERSION_GUARD_SETTING,
};
pub use probe::{availability, DockerAvailability};
pub use progress::set_build_sink;

/// The container auth chain, now runtime-neutral. Re-exported so the
/// `sandbox::docker::auth::…` paths the app and its Tauri commands already use
/// keep resolving.
pub use crate::sandbox::container::auth;

/// The set of providers a container can run, under the name the rest of the app
/// knows it by. Docker was the first container runtime, so the type is spelled
/// [`ContainerProvider`](crate::sandbox::container::ContainerProvider) at its new
/// home and re-exported here.
pub use crate::sandbox::container::ContainerProvider as DockerProvider;

/// How long the startup sweep waits between probes while the daemon is still
/// coming up. Comfortably above [`probe`]'s 5s cache TTL, so every poll is one
/// genuine daemon round-trip rather than a re-read of the same cached answer,
/// and short enough that the sweeps start within half a minute of Docker
/// Desktop finishing its boot.
const SWEEP_RETRY_INTERVAL: Duration = Duration::from_secs(30);

/// How long the startup sweep keeps waiting for a down daemon before giving
/// up. Ten minutes covers a cold login — Docker Desktop's own 20-60s plus a
/// slow disk, a VPN prompt, or a user who starts it by hand a few minutes in —
/// while staying strictly bounded: a machine where Docker simply never runs
/// must not keep a thread ticking for the life of the app. The budget gates
/// when we stop *scheduling* waits, so the final probe can land up to one
/// [`SWEEP_RETRY_INTERVAL`] past it. Giving up is cheap: the next app start
/// sweeps again, so the cost of a miss is one run's worth of unreclaimed
/// images, not a permanent leak.
const SWEEP_RETRY_BUDGET: Duration = Duration::from_secs(10 * 60);

/// What the startup sweep thread does after one probe — see [`sweep_step`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SweepStep {
    /// Docker answered: run both sweeps, once, and finish.
    Sweep,
    /// The daemon is down but may still be booting: wait and probe again.
    Retry,
    /// Nothing to wait for, or the budget is spent: the thread exits having
    /// swept nothing.
    Stop,
}

/// The retry schedule as a pure decision: given the latest probe result and
/// how long the sweep thread has been waiting, keep waiting, sweep, or stop.
/// Split out of [`sweep_orphans_at_startup`] so the schedule is unit-testable
/// without a daemon and without sleeping.
///
/// [`DockerAvailability::NotInstalled`] stops immediately rather than
/// retrying: Docker being *installed* mid-run is rare, the next app start
/// covers it, and each retry would re-pay `cli::docker_bin`'s
/// binary-resolution stat walk for a state that essentially never flips.
/// [`DockerAvailability::DaemonDown`], by contrast, is the state we expect to
/// flip — that's the whole point of the retry.
fn sweep_step(availability: &DockerAvailability, elapsed: Duration) -> SweepStep {
    match availability {
        DockerAvailability::Available { .. } => SweepStep::Sweep,
        DockerAvailability::NotInstalled => SweepStep::Stop,
        DockerAvailability::DaemonDown if elapsed < SWEEP_RETRY_BUDGET => SweepStep::Retry,
        DockerAvailability::DaemonDown => SweepStep::Stop,
    }
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
