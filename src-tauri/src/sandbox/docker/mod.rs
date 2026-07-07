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

pub mod auth;
pub mod setup_token;
mod cleanup;
mod cli;
mod engine;
mod image;
mod probe;
mod progress;

pub use engine::{
    set_launch_settings, DockerEngine, LaunchSettings, CPUS_SETTING, IMAGE_SETTING, MEMORY_SETTING,
};
pub use probe::{availability, DockerAvailability};
pub use progress::set_build_sink;

/// A provider Fletch can run inside a Docker sandbox. This is the single
/// capability gate the rest of the app consults instead of string-matching
/// `provider == "claude"`: [`supervisor::lifecycle::ensure_engine_supports_provider`]
/// refuses anything [`from_id`](DockerProvider::from_id) doesn't recognize, and
/// the launch path ([`engine`]) branches on the variant for the provider-specific
/// image ([`image`]), config-dir mount, and auth. Everything else about a
/// container (workspace / RPC / object-store mounts, naming, teardown) is
/// provider-agnostic.
///
/// Seatbelt runs six providers; Docker is being brought up one at a time as each
/// gets its image + config-mount + auth wired here — claude and codex so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DockerProvider {
    Claude,
    Codex,
}

impl DockerProvider {
    /// Map a provider id (as stamped on `AgentRecord.provider` / used by the
    /// frontend) to its Docker support, or `None` when the provider has no
    /// container support yet — the launch gate turns `None` into the
    /// user-facing "isn't available in Docker sandboxes yet" refusal.
    pub fn from_id(provider: &str) -> Option<Self> {
        match provider {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }

    /// The command name on the image's PATH — what this provider's npm package
    /// installs as its executable. Handed to `launch_agent` as the in-image
    /// `agent_bin` (a host-resolved absolute path would be meaningless inside
    /// the container). Matches the provider's `bin` field for both supported
    /// providers today, but named explicitly so it stays an image fact, not a
    /// coincidence.
    pub fn image_bin(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

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
