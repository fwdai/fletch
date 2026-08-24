//! Launch knobs and the version-refresh loop guard, both mirrored in-process
//! (the spawn path and background threads have no DB handle). Seeded at startup
//! and kept in sync by the settings set-commands — see [`set_launch_settings`]
//! and [`init_version_refresh_guard`].
//!
//! Deliberately a sibling of `docker::engine::settings` rather than a shared
//! module: the keys are per runtime (`podman_image` next to `docker_image`) so a
//! user running both engines can point each at its own image and limits, and
//! the version guard must stay per runtime because the two keep separate image
//! stores — an image present in one says nothing about the other.

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

/// Launch knobs read from the `settings` table, mirrored in-process (the spawn
/// path has no DB handle — same pattern as `sandbox::set_selected_engine_kind`).
/// Seeded at startup in `lib.rs setup` and kept in sync by the settings
/// set-commands.
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
/// `provider id → "host_version@image_tag"`, recording the last host/image
/// pairing a version-mismatch rebuild *succeeded* for. Not a user-facing
/// setting — private bookkeeping that must survive restarts, for the reason
/// [`container::version_guard`](crate::sandbox::container::version_guard)
/// documents.
pub const VERSION_GUARD_SETTING: &str = "podman_version_refresh_guard";

/// Podman's loop-guard state, mirrored in-process like [`LAUNCH_SETTINGS`].
static VERSION_GUARD: VersionGuard = VersionGuard::new();

/// Install the loop-guard state: `attempted` as loaded from
/// [`VERSION_GUARD_SETTING`], `persist` writing it back. The app wires this to a
/// `database::set_setting` closure at startup.
pub fn init_version_refresh_guard(
    attempted: std::collections::HashMap<String, String>,
    persist: impl Fn(&std::collections::HashMap<String, String>) + Send + Sync + 'static,
) {
    VERSION_GUARD.init(attempted, persist);
}

/// Whether a version-mismatch rebuild already succeeded for exactly this `pair`
/// (`"host_version@image_tag"`).
pub(super) fn version_refresh_attempted(provider_id: &str, pair: &str) -> bool {
    VERSION_GUARD.attempted(provider_id, pair)
}

/// Record (and persist, when wired) that a version-mismatch rebuild succeeded
/// for `pair`. Called from the background rebuild thread on success only.
pub(super) fn record_version_refresh(provider_id: &str, pair: String) {
    VERSION_GUARD.record(provider_id, pair);
}

/// The current `podman_image` override, trimmed, `None` when unset/blank — read
/// by the image GC ([`super::cleanup::sweep_stale_images`]) to defensively
/// exclude the user's image from removal. Structurally it should never be a
/// candidate (Fletch never builds it, so it carries no `fletch.agent` label and
/// lives outside Fletch's repos), but a lifecycle we don't own gets a second
/// fence.
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

    /// The keys mirror docker's shape one-for-one under a `podman_` prefix, so
    /// the settings table stays readable and neither runtime can clobber the
    /// other's knobs.
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

    /// The override reader treats blank as unset — the same
    /// blank-falls-back-to-default semantics the launch path applies.
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

    /// Podman's guard is its own static: exact-pair matching and per-provider
    /// isolation hold here independently of docker's (the shared machinery is
    /// covered in `docker::engine::tests`), and recording works before `init` so
    /// a headless run still guards its own process.
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
