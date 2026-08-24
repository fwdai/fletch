//! The version-refresh loop guard: one per container runtime, sharing this
//! implementation.
//!
//! It caps the version-parity rebuild trigger at one attempt per
//! `host_version@image_tag` pairing *ever* — a host CLI pinned away from the
//! registry's latest keeps mismatching after a successful rebuild, so an
//! in-memory-only guard would mean a `--no-cache` rebuild every app run.
//!
//! Each runtime owns its own `static` and settings key: an image present in one
//! store says nothing about the other's.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

/// Writes the guard map back to its settings row. An `Arc` so
/// [`VersionGuard::record`] can clone it out and call it with the lock released.
type Persist = Arc<dyn Fn(&HashMap<String, String>) + Send + Sync>;

/// provider id → `"host_version@image_tag"` last successfully rebuilt for, plus
/// the write-back. One pair per provider: a change to either side warrants one
/// fresh attempt.
struct State {
    attempted: HashMap<String, String>,
    persist: Option<Persist>,
}

/// A runtime's loop guard, mirrored in-process because the image code that
/// consults it runs on threads with no DB handle. Before
/// [`init`](Self::init) it is empty and unpersisted, but still guards this run.
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
            persist: Some(Arc::new(persist)),
        });
    }

    /// Whether a rebuild already succeeded for exactly this `pair`. If so the
    /// trigger is inert — the mismatch survived a rebuild, so it isn't
    /// rebuildable away.
    pub(crate) fn attempted(&self, provider_id: &str, pair: &str) -> bool {
        self.0
            .read()
            .as_ref()
            .is_some_and(|g| g.attempted.get(provider_id).map(String::as_str) == Some(pair))
    }

    /// Record (and persist, when wired) that a rebuild succeeded for `pair`.
    /// Success only — a failure must retry on a later run.
    pub(crate) fn record(&self, provider_id: &str, pair: String) {
        // The persister writes the DB; calling it under the lock would order
        // guard-before-db and invite a deadlock. Snapshot, unlock, then persist.
        let (snapshot, persist) = {
            let mut guard = self.0.write();
            let state = guard.get_or_insert_with(|| State {
                attempted: HashMap::new(),
                persist: None,
            });
            state.attempted.insert(provider_id.to_string(), pair);
            (state.attempted.clone(), state.persist.clone())
        };
        if let Some(persist) = persist {
            persist(&snapshot);
        }
    }
}
