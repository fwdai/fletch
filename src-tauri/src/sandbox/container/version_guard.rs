//! The version-refresh loop guard: one per container runtime, sharing this
//! implementation.
//!
//! The guard caps the version-parity rebuild trigger at one attempt per
//! `host_version@image_tag` pairing *ever*, not one per app run. In the guarded
//! case — a host CLI pinned away from the registry's latest, so the mismatch
//! survives even a successful rebuild — an in-memory-only guard would decay into
//! one full `--no-cache` rebuild on every app run.
//!
//! Each runtime owns a `static VersionGuard` and its own settings key (docker's
//! `docker_version_refresh_guard`, podman's `podman_version_refresh_guard`), so
//! the two runtimes' image stores never talk each other out of a rebuild: an
//! image present in one store says nothing about the other's.

use std::collections::HashMap;

use parking_lot::RwLock;

/// Writes the guard map back to its settings row (installed by
/// [`VersionGuard::init`]).
type Persist = Box<dyn Fn(&HashMap<String, String>) + Send + Sync>;

/// provider id → `"host_version@image_tag"` last successfully rebuilt for, plus
/// the write-back. One pair per provider suffices — any change to either side
/// legitimately warrants one fresh attempt.
struct State {
    attempted: HashMap<String, String>,
    persist: Option<Persist>,
}

/// A runtime's loop guard, mirrored in-process because the image code that
/// consults it runs on spawn paths and background threads with no DB handle.
/// Seeded and wired to a persister at startup by [`init`](Self::init); until
/// then (tests, headless) it's empty and unpersisted, and recording still guards
/// the current process run.
pub(crate) struct VersionGuard(RwLock<Option<State>>);

impl VersionGuard {
    pub(crate) const fn new() -> Self {
        Self(RwLock::new(None))
    }

    /// Install the guard state: `attempted` as loaded from the runtime's
    /// settings row, `persist` writing it back.
    pub(crate) fn init(
        &self,
        attempted: HashMap<String, String>,
        persist: impl Fn(&HashMap<String, String>) + Send + Sync + 'static,
    ) {
        *self.0.write() = Some(State {
            attempted,
            persist: Some(Box::new(persist)),
        });
    }

    /// Whether a version-mismatch rebuild already succeeded for exactly this
    /// `pair` (`"host_version@image_tag"`). If so the trigger is inert: the
    /// mismatch survived a rebuild, so it isn't rebuildable-away (pinned host).
    pub(crate) fn attempted(&self, provider_id: &str, pair: &str) -> bool {
        self.0
            .read()
            .as_ref()
            .is_some_and(|g| g.attempted.get(provider_id).map(String::as_str) == Some(pair))
    }

    /// Record (and persist, when wired) that a version-mismatch rebuild
    /// succeeded for `pair`. Called from the background rebuild thread on
    /// success only — failures must retry on a later run, exactly like TTL
    /// rebuild failures.
    pub(crate) fn record(&self, provider_id: &str, pair: String) {
        let mut guard = self.0.write();
        let state = guard.get_or_insert_with(|| State {
            attempted: HashMap::new(),
            persist: None,
        });
        state.attempted.insert(provider_id.to_string(), pair);
        if let Some(persist) = &state.persist {
            persist(&state.attempted);
        }
    }
}
