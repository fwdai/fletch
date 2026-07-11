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
//! - [`cleanup`] — container labels, the dead-instance orphan sweep, and the
//!   stale agent-image GC.
//! - [`engine`] — `DockerEngine`, the `SandboxEngine` implementation
//!   (one `docker run --rm --init` container per agent process).

pub mod auth;
mod cleanup;
mod cli;
mod engine;
mod image;
mod probe;
mod progress;
pub mod setup_token;

pub use engine::{
    init_version_refresh_guard, set_launch_settings, DockerEngine, LaunchSettings, CPUS_SETTING,
    IMAGE_SETTING, MEMORY_SETTING, VERSION_GUARD_SETTING,
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
/// gets its image + config-mount + auth wired here — claude, codex, opencode, pi,
/// and cursor so far. antigravity remains gated: its CLI (`agy`) has no
/// non-interactive credential path — auth is browser OAuth with its tokens in the
/// host keychain and no API-key env fallback (maintainer-confirmed), so a fresh
/// container cannot authenticate. See `ensure_engine_supports_provider`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DockerProvider {
    Claude,
    Codex,
    Opencode,
    Pi,
    Cursor,
}

impl DockerProvider {
    /// Every docker-supported provider. The image GC derives "the current
    /// expected images" from this list, so a variant missing here would make
    /// the GC treat that provider's live image as stale — when adding a
    /// variant, extend this list (the exhaustive `match` in `image::image_spec`
    /// will already force you into that file).
    pub const ALL: [Self; 5] = [
        Self::Claude,
        Self::Codex,
        Self::Opencode,
        Self::Pi,
        Self::Cursor,
    ];

    /// Map a provider id (as stamped on `AgentRecord.provider` / used by the
    /// frontend) to its Docker support, or `None` when the provider has no
    /// container support yet — the launch gate turns `None` into the
    /// user-facing "isn't available in Docker sandboxes yet" refusal.
    pub fn from_id(provider: &str) -> Option<Self> {
        match provider {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "opencode" => Some(Self::Opencode),
            "pi" => Some(Self::Pi),
            "cursor" => Some(Self::Cursor),
            _ => None,
        }
    }

    /// The provider id string — [`from_id`](Self::from_id)'s inverse
    /// (round-trip enforced by a test in [`image`]). Used where a variant must
    /// key string-indexed state shared with the rest of the app, e.g. the host
    /// version probe (`agent::cached_provider_version`) and the persisted
    /// version-refresh loop guard (see `engine`).
    pub fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::Pi => "pi",
            Self::Cursor => "cursor",
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
            Self::Opencode => "opencode",
            Self::Pi => "pi",
            Self::Cursor => "cursor-agent",
        }
    }
}

/// Best-effort reclamation of containers left behind by dead Fletch
/// instances — and of superseded agent images — for app startup (`lib.rs`,
/// next to the nested-root sweeps). Runs on its own thread and probes the
/// daemon first, so startup never waits on Docker — not even for the 2s probe
/// timeout — and a machine without Docker skips both sweeps entirely. The
/// image sweep runs second: a removed orphan container can unpin the stale
/// image it was running. Both sweeps are non-fatal by construction.
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
