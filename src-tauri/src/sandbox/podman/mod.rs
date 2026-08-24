//! The Podman sandbox engine and its primitives: availability probing, the
//! agent image, orphaned-container cleanup, and the launch path.
//!
//! Everything here must work when Podman is absent or its machine is down:
//! probing reports that state instead of erroring, the startup sweep is
//! probe-gated so an install-less machine never pays for a podman invocation,
//! and `sandbox::engine_for` only routes launches here when the probe says the
//! machine answers. "Down" and "absent" are distinguished throughout: a down
//! machine is a state that flips (see [`sweep_orphans_at_startup`], which waits
//! one out), an absent binary is treated as settled for the run.
//!
//! Layout mirrors [`docker`](crate::sandbox::docker) — the Podman *runtime* on
//! top of the runtime-neutral policy in [`crate::sandbox::container`], which it
//! reuses unchanged (identical-path binds, the same labels, the same image
//! content, the same per-provider auth):
//! - [`cli`] — podman binary resolution + bounded-invocation wrappers. Every
//!   podman call in this module goes through it, so no invocation can hang the
//!   app on a wedged machine connection.
//! - [`probe`] — cached availability for UI polling.
//! - [`image`] — building and inspecting the agent images.
//! - [`machine`] — the shared-directory preflight, Podman's one behavioural
//!   difference from Docker at launch time.
//! - [`cleanup`] — the dead-instance orphan sweep and per-agent removal.
//! - [`engine`] — `PodmanEngine`, the `SandboxEngine` implementation (one
//!   `podman run --rm --init` container per agent process).

use std::time::Instant;

use crate::sandbox::container::sweep::{SweepStep, SWEEP_RETRY_INTERVAL};

mod cleanup;
mod cli;
mod engine;
mod image;
mod machine;
mod probe;

pub use cleanup::remove_agent_containers;
pub use engine::PodmanEngine;
pub use probe::{availability, PodmanAvailability};

/// Map a Podman probe result onto the shared retry schedule
/// ([`container::sweep`](crate::sandbox::container::sweep)). A missing binary is
/// settled for the run; a down machine is the state we expect to flip — a user
/// who runs `podman machine start` a minute after login is the case this exists
/// for.
fn sweep_step(availability: &PodmanAvailability, elapsed: std::time::Duration) -> SweepStep {
    let usable = matches!(availability, PodmanAvailability::Available { .. });
    let installed = !matches!(availability, PodmanAvailability::NotInstalled);
    crate::sandbox::container::sweep::sweep_step(usable, installed, elapsed)
}

/// Best-effort reclamation of containers left behind by dead Fletch instances,
/// for app startup (`lib.rs`, next to the docker sweep). Runs on its own thread,
/// so startup never waits on Podman — not even for the 2s probe timeout — and a
/// machine without Podman skips the sweep entirely. Non-fatal by construction.
///
/// No image sweep: this runtime ships no image GC yet, so there is nothing to
/// run as a second pass.
pub fn sweep_orphans_at_startup() {
    std::thread::spawn(|| {
        let waiting_since = Instant::now();
        loop {
            match sweep_step(&probe::availability(), waiting_since.elapsed()) {
                SweepStep::Sweep => break,
                SweepStep::Retry => std::thread::sleep(SWEEP_RETRY_INTERVAL),
                SweepStep::Stop => {
                    tracing::debug!(
                        target: "fletch::podman",
                        waited_secs = waiting_since.elapsed().as_secs(),
                        "podman unavailable; skipping the startup sweep this run",
                    );
                    return;
                }
            }
        }
        match cleanup::sweep_orphans() {
            Ok(0) => {}
            Ok(n) => tracing::info!(removed = n, "swept orphaned fletch podman containers"),
            Err(e) => tracing::warn!(error = %e, "podman orphan sweep failed"),
        }
    });
}

/// Gate for the `#[ignore]`d integration tests: they touch a real Podman
/// machine, so they run only when explicitly opted in via
/// `FLETCH_PODMAN_TESTS=1 cargo test -- --ignored`.
#[cfg(test)]
pub(crate) fn podman_tests_enabled() -> bool {
    std::env::var("FLETCH_PODMAN_TESTS").as_deref() == Ok("1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::container::sweep::SWEEP_RETRY_BUDGET;
    use std::time::Duration;

    /// The Podman probe's mapping onto the shared schedule: an answering
    /// machine sweeps, a missing binary never retries, and a down machine
    /// retries until — and only until — the budget is spent.
    #[test]
    fn sweep_retry_schedule() {
        let available = PodmanAvailability::Available {
            server_version: "5.3.1".into(),
        };
        assert_eq!(sweep_step(&available, Duration::ZERO), SweepStep::Sweep);
        assert_eq!(
            sweep_step(&available, SWEEP_RETRY_BUDGET * 2),
            SweepStep::Sweep,
            "a late machine is still worth sweeping for on the probe that catches it",
        );
        assert_eq!(
            sweep_step(&PodmanAvailability::NotInstalled, Duration::ZERO),
            SweepStep::Stop,
            "podman being installed mid-run isn't worth polling for",
        );
        assert_eq!(
            sweep_step(&PodmanAvailability::MachineDown, Duration::ZERO),
            SweepStep::Retry,
            "the first probe usually loses the `podman machine start` race",
        );
        // The budget is a deadline, not a suggestion: the loop is bounded.
        assert_eq!(
            sweep_step(&PodmanAvailability::MachineDown, SWEEP_RETRY_BUDGET),
            SweepStep::Stop,
        );
    }
}
