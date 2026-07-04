//! The Docker sandbox engine and its primitives: availability probing, the
//! agent image, orphaned-container cleanup, and the launch path.
//!
//! Everything here must work when Docker is absent or the daemon is down:
//! probing reports that state instead of erroring, the startup sweep is
//! probe-gated so an install-less machine never pays for a docker
//! invocation, and `sandbox::engine_for` only routes launches here when the
//! probe says the daemon is up.
//!
//! Layout:
//! - [`cli`] — docker binary resolution + bounded-invocation helpers. Every
//!   docker call in this module goes through it, so no invocation can hang
//!   the app on a wedged daemon.
//! - [`probe`] — cached daemon availability for UI polling.
//! - [`image`] — the embedded agent Dockerfile and content-addressed builds.
//! - [`cleanup`] — container labels and the dead-instance orphan sweep.
//! - [`engine`] — `DockerEngine`, the `SandboxEngine` implementation
//!   (one `docker run --rm --init` container per agent process).

mod cleanup;
mod cli;
mod engine;
mod image;
mod probe;

// Label plumbing for slice C2's surfacing needs; unused until then.
#[allow(unused_imports)]
pub use cleanup::{agent_id_label, host_pid_label, sweep_orphans, AGENT_ID_LABEL, HOST_PID_LABEL};
pub use engine::{
    set_launch_settings, DockerEngine, LaunchSettings, CPUS_SETTING, IMAGE_SETTING, MEMORY_SETTING,
};
// Build progress plumbing for slice C2's UI events; unused until then.
#[allow(unused_imports)]
pub use image::{ensure_image, image_tag, resolve_image, Progress};
pub use probe::{availability, DockerAvailability};

/// Best-effort reclamation of containers left behind by dead Fletch
/// instances, for app startup (`lib.rs`, next to the nested-root sweeps).
/// Runs on its own thread and probes the daemon first, so startup never
/// waits on Docker — not even for the 2s probe timeout — and a machine
/// without Docker skips the sweep entirely.
pub fn sweep_orphans_at_startup() {
    std::thread::spawn(|| {
        if !matches!(probe::availability(), DockerAvailability::Available { .. }) {
            return;
        }
        match cleanup::sweep_orphans() {
            Ok(0) => {}
            Ok(n) => tracing::info!(removed = n, "swept orphaned fletch containers"),
            Err(e) => tracing::warn!(error = %e, "docker orphan sweep failed"),
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
