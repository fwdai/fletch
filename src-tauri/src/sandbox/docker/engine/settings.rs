//! Launch knobs and the version-refresh loop guard, both mirrored in-process
//! (the spawn path and background threads have no DB handle). Seeded at startup
//! and kept in sync by the settings set-commands — see [`set_launch_settings`]
//! and [`init_version_refresh_guard`].

use parking_lot::RwLock;

/// Settings key overriding the container image (see [`super::image::resolve_image`]).
pub const IMAGE_SETTING: &str = "docker_image";
/// Settings key for the container memory limit (`docker run --memory`).
pub const MEMORY_SETTING: &str = "docker_memory";
/// Settings key for the container CPU limit (`docker run --cpus`).
pub const CPUS_SETTING: &str = "docker_cpus";

pub(super) const DEFAULT_MEMORY: &str = "4g";
pub(super) const DEFAULT_CPUS: &str = "2";

/// Launch knobs read from the `settings` table, mirrored in-process (the spawn
/// path has no DB handle — same pattern as `sandbox::set_selected_engine_kind`).
/// Seeded at startup in `lib.rs setup` and kept in sync by the settings
/// set-commands.
#[derive(Clone, Default)]
pub struct LaunchSettings {
    /// `docker_image` — a non-empty value is used verbatim, skipping the
    /// embedded image build entirely.
    pub image_override: Option<String>,
    /// `docker_memory` — `--memory` value; `None`/blank means [`DEFAULT_MEMORY`].
    pub memory: Option<String>,
    /// `docker_cpus` — `--cpus` value; `None`/blank means [`DEFAULT_CPUS`].
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
/// setting — private bookkeeping that must survive restarts: in the guarded
/// case (host CLI pinned away from the registry's latest, so the mismatch
/// persists even after a successful rebuild) an in-memory guard would decay
/// into one full `--no-cache` rebuild on every app run. One pair per provider
/// suffices — any change to either side legitimately warrants one fresh
/// attempt.
pub const VERSION_GUARD_SETTING: &str = "docker_version_refresh_guard";

/// Writes the guard map back to its settings row (installed by
/// [`init_version_refresh_guard`]).
type VersionGuardPersist = Box<dyn Fn(&std::collections::HashMap<String, String>) + Send + Sync>;

/// The version-refresh loop guard, mirrored in-process like
/// [`LAUNCH_SETTINGS`] (the image code that consults it runs on spawn paths
/// and background threads with no DB handle). Seeded and wired to a persister
/// at startup by [`init_version_refresh_guard`]; until then (tests, headless)
/// it's empty and unpersisted, and recording still guards the current
/// process run.
struct VersionGuard {
    /// provider id → `"host_version@image_tag"` last successfully rebuilt for.
    attempted: std::collections::HashMap<String, String>,
    /// Writes the whole map back to the settings row.
    persist: Option<VersionGuardPersist>,
}

static VERSION_GUARD: RwLock<Option<VersionGuard>> = RwLock::new(None);

/// Install the loop-guard state: `attempted` as loaded from
/// [`VERSION_GUARD_SETTING`], `persist` writing it back. The app wires this
/// to a `database::set_setting` closure at startup — the same mirror idiom as
/// [`set_launch_settings`] and `progress::set_build_sink`.
pub fn init_version_refresh_guard(
    attempted: std::collections::HashMap<String, String>,
    persist: impl Fn(&std::collections::HashMap<String, String>) + Send + Sync + 'static,
) {
    *VERSION_GUARD.write() = Some(VersionGuard {
        attempted,
        persist: Some(Box::new(persist)),
    });
}

/// Whether a version-mismatch rebuild already succeeded for exactly this
/// `pair` (`"host_version@image_tag"`). If so the trigger is inert: the
/// mismatch survived a rebuild, so it isn't rebuildable-away (pinned host).
pub(crate) fn version_refresh_attempted(provider_id: &str, pair: &str) -> bool {
    VERSION_GUARD
        .read()
        .as_ref()
        .is_some_and(|g| g.attempted.get(provider_id).map(String::as_str) == Some(pair))
}

/// Record (and persist, when wired) that a version-mismatch rebuild succeeded
/// for `pair`. Called from the background rebuild thread on success only —
/// failures must retry on a later run, exactly like TTL rebuild failures.
pub(crate) fn record_version_refresh(provider_id: &str, pair: String) {
    let mut guard = VERSION_GUARD.write();
    let state = guard.get_or_insert_with(|| VersionGuard {
        attempted: std::collections::HashMap::new(),
        persist: None,
    });
    state.attempted.insert(provider_id.to_string(), pair);
    if let Some(persist) = &state.persist {
        persist(&state.attempted);
    }
}

/// The current `docker_image` override, trimmed, `None` when unset/blank —
/// read by the image GC (`cleanup::sweep_stale_images`) to defensively exclude
/// the user's image from removal. Structurally it should never be a candidate
/// (Fletch never builds it, so it carries no `fletch.agent` label and lives
/// outside Fletch's repos), but a lifecycle we don't own gets a second fence.
pub(crate) fn image_override() -> Option<String> {
    LAUNCH_SETTINGS
        .read()
        .image_override
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}
