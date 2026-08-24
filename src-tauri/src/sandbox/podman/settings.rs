//! Launch knobs and the version-refresh loop guard, both mirrored in-process
//! (the spawn path and background threads have no DB handle).
//!
//! Per runtime rather than shared with `docker::engine::settings`: the two keep
//! separate image stores, so an image present in one says nothing about the
//! other. The guard is not per *connection* even though podman's stores are
//! per machine — the map holds one pair per provider, so two machines would
//! evict each other and ping-pong rebuilds. The TTL backstops the second one.

use parking_lot::RwLock;

use crate::sandbox::container::version_guard::VersionGuard;

/// Settings key overriding the container image (see [`super::image::resolve_image`]).
pub const IMAGE_SETTING: &str = "podman_image";
/// Settings key for the container memory limit (`podman run --memory`).
pub const MEMORY_SETTING: &str = "podman_memory";
/// Settings key for the container CPU limit (`podman run --cpus`).
pub const CPUS_SETTING: &str = "podman_cpus";

// The launch defaults are container policy, not podman's: they live in
// [`crate::sandbox::container::run_args`] so both runtimes cap the same way.

/// Launch knobs read from the `settings` table. Seeded at startup in `lib.rs
/// setup` and kept in sync by the settings set-commands.
#[derive(Clone, Default)]
pub struct LaunchSettings {
    /// `podman_image` — a non-empty value is used verbatim, skipping the
    /// embedded image build entirely.
    pub image_override: Option<String>,
    /// `podman_memory` — `--memory` value; `None`/blank means the container
    /// layer's `DEFAULT_MEMORY`.
    pub memory: Option<String>,
    /// `podman_cpus` — `--cpus` value; `None`/blank means the container layer's
    /// `DEFAULT_CPUS`.
    pub cpus: Option<String>,
}

pub(super) static LAUNCH_SETTINGS: RwLock<LaunchSettings> = RwLock::new(LaunchSettings {
    image_override: None,
    memory: None,
    cpus: None,
});

pub fn set_launch_settings(settings: LaunchSettings) {
    *LAUNCH_SETTINGS.write() = settings;
}

/// Settings key persisting the version-refresh loop guard: a JSON object of
/// `provider id → "host_version@image_tag"`, the last pairing a rebuild
/// succeeded for. Private bookkeeping that must survive restarts, not a
/// user-facing setting — see
/// [`container::version_guard`](crate::sandbox::container::version_guard).
pub const VERSION_GUARD_SETTING: &str = "podman_version_refresh_guard";

/// Podman's loop-guard state, mirrored in-process like [`LAUNCH_SETTINGS`].
static VERSION_GUARD: VersionGuard = VersionGuard::new();

/// Install the loop-guard state: `attempted` as loaded from
/// [`VERSION_GUARD_SETTING`], `persist` writing it back.
pub fn init_version_refresh_guard(
    attempted: std::collections::HashMap<String, String>,
    persist: impl Fn(&std::collections::HashMap<String, String>) + Send + Sync + 'static,
) {
    VERSION_GUARD.init(attempted, persist);
}

/// Whether a rebuild already succeeded for exactly this `pair`
/// (`"host_version@image_tag"`).
pub(super) fn version_refresh_attempted(provider_id: &str, pair: &str) -> bool {
    VERSION_GUARD.attempted(provider_id, pair)
}

/// Record (and persist, when wired) a rebuild that succeeded for `pair`.
pub(super) fn record_version_refresh(provider_id: &str, pair: String) {
    VERSION_GUARD.record(provider_id, pair);
}

/// The current `podman_image` override, trimmed, `None` when unset/blank. The
/// image GC ([`super::cleanup::sweep_stale_images`]) excludes it: it should
/// never be a candidate anyway (unlabelled, outside Fletch's repos), but a
/// lifecycle we don't own gets a second fence.
pub(super) fn image_override() -> Option<String> {
    LAUNCH_SETTINGS
        .read()
        .image_override
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_mirror_dockers_shape_under_a_podman_prefix() {
        use crate::sandbox::docker;
        for (podman, docker) in [
            (IMAGE_SETTING, docker::IMAGE_SETTING),
            (MEMORY_SETTING, docker::MEMORY_SETTING),
            (CPUS_SETTING, docker::CPUS_SETTING),
            (VERSION_GUARD_SETTING, docker::VERSION_GUARD_SETTING),
        ] {
            assert_ne!(podman, docker, "keys must not collide");
            assert_eq!(
                podman.strip_prefix("podman_"),
                docker.strip_prefix("docker_"),
                "{podman} should mirror {docker}",
            );
        }
    }

    #[test]
    fn blank_override_reads_as_unset() {
        set_launch_settings(LaunchSettings {
            image_override: Some("   ".into()),
            memory: None,
            cpus: None,
        });
        assert_eq!(image_override(), None);
        set_launch_settings(LaunchSettings {
            image_override: Some("  ghcr.io/me/custom:1  ".into()),
            memory: None,
            cpus: None,
        });
        assert_eq!(image_override().as_deref(), Some("ghcr.io/me/custom:1"));
        set_launch_settings(LaunchSettings::default());
        assert_eq!(image_override(), None);
    }

    /// Podman's guard is its own static, and works before `init` so a headless
    /// run still guards its own process.
    #[test]
    fn version_guard_records_before_init() {
        let pair = "v1@fletch-agent:podmanonly";
        assert!(!version_refresh_attempted("claude", pair));
        record_version_refresh("claude", pair.into());
        assert!(version_refresh_attempted("claude", pair));
        // Exact pair only, and one provider's verdict never answers another's.
        assert!(!version_refresh_attempted(
            "claude",
            "v2@fletch-agent:podmanonly"
        ));
        assert!(!version_refresh_attempted("codex", pair));
    }
}
