//! The Podman sandbox engine: availability probing, the agent image,
//! orphaned-container cleanup, and the launch path. Layout mirrors
//! [`docker`](crate::sandbox::docker), over the runtime-neutral policy in
//! [`crate::sandbox::container`].
//!
//! Everything here must work when Podman is absent or its machine is down. The
//! two are distinct: a down machine is a state that flips and is waited out, an
//! absent binary is settled for the run.

use std::time::Instant;

use crate::sandbox::container::sweep::{SweepStep, SWEEP_RETRY_INTERVAL};

mod cleanup;
mod cli;
mod engine;
mod image;
mod machine;
mod probe;
mod settings;

pub use cleanup::remove_agent_containers;
pub use engine::PodmanEngine;
pub use probe::{availability, PodmanAvailability};
pub use settings::{
    init_version_refresh_guard, set_launch_settings, LaunchSettings, CPUS_SETTING, IMAGE_SETTING,
    MEMORY_SETTING, VERSION_GUARD_SETTING,
};

/// This runtime's display name in user-facing copy.
pub(super) const RUNTIME_NAME: &str = "Podman";

/// Map a Podman probe result onto the shared retry schedule
/// ([`container::sweep`](crate::sandbox::container::sweep)): a missing binary is
/// settled for the run, a down machine is expected to flip.
fn sweep_step(availability: &PodmanAvailability, elapsed: std::time::Duration) -> SweepStep {
    let usable = matches!(availability, PodmanAvailability::Available { .. });
    let installed = !matches!(availability, PodmanAvailability::NotInstalled);
    crate::sandbox::container::sweep::sweep_step(usable, installed, elapsed)
}

/// Best-effort startup reclamation of containers left by dead Fletch instances,
/// then of superseded agent images. Own thread, so startup never waits on
/// Podman. Image sweep runs second: removing an orphan can unpin the stale image
/// it was running. These sweeps and the post-refresh one are the only thing that
/// reclaims agent images, hence the [`sweep_step`] retries rather than accepting
/// the first probe answer.
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
        match cleanup::sweep_stale_images() {
            Ok(0) => {}
            Ok(n) => tracing::info!(removed = n, "swept stale fletch agent podman images"),
            Err(e) => tracing::warn!(error = %e, "podman image sweep failed"),
        }
    });
}

/// Why a launch would be refused right now even though [`availability`] answers
/// `Available`, or `None` when nothing stands in the way — `podman info`
/// succeeds over a remote default connection that every launch then refuses.
/// The message is [`machine::resolve_launch_target`]'s own refusal, verbatim, so
/// selection and launch errors can't drift.
pub fn launch_blocker() -> Option<String> {
    machine::resolve_launch_target()
        .err()
        .map(|e| e.to_string())
}

/// Gate for the `#[ignore]`d integration tests, which touch a real Podman
/// machine: `FLETCH_PODMAN_TESTS=1 cargo test -- --ignored`.
#[cfg(test)]
pub(crate) fn podman_tests_enabled() -> bool {
    std::env::var("FLETCH_PODMAN_TESTS").as_deref() == Ok("1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::container::sweep::SWEEP_RETRY_BUDGET;
    use std::time::Duration;

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
        assert_eq!(
            sweep_step(&PodmanAvailability::MachineDown, SWEEP_RETRY_BUDGET),
            SweepStep::Stop,
        );
    }
}
