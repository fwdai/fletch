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

use parking_lot::{Mutex, RwLock};

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
pub(crate) struct VersionGuard {
    state: RwLock<Option<State>>,
    /// Orders snapshot+persist pairs, so a slower [`record`](Self::record) can't
    /// write an older snapshot over a newer row. Never held with `state` locked
    /// for writing, and the DB call never runs under `state` at all.
    persist_order: Mutex<()>,
}

impl VersionGuard {
    pub(crate) const fn new() -> Self {
        Self {
            state: RwLock::new(None),
            persist_order: Mutex::new(()),
        }
    }

    /// Install the guard state: `attempted` as loaded from the runtime's
    /// settings row, `persist` writing it back.
    pub(crate) fn init(
        &self,
        attempted: HashMap<String, String>,
        persist: impl Fn(&HashMap<String, String>) + Send + Sync + 'static,
    ) {
        *self.state.write() = Some(State {
            attempted,
            persist: Some(Arc::new(persist)),
        });
    }

    /// Whether a rebuild already succeeded for exactly this `pair`. If so the
    /// trigger is inert — the mismatch survived a rebuild, so it isn't
    /// rebuildable away.
    pub(crate) fn attempted(&self, provider_id: &str, pair: &str) -> bool {
        self.state
            .read()
            .as_ref()
            .is_some_and(|g| g.attempted.get(provider_id).map(String::as_str) == Some(pair))
    }

    /// Record (and persist, when wired) that a rebuild succeeded for `pair`.
    /// Success only — a failure must retry on a later run.
    pub(crate) fn record(&self, provider_id: &str, pair: String) {
        {
            let mut guard = self.state.write();
            let state = guard.get_or_insert_with(|| State {
                attempted: HashMap::new(),
                persist: None,
            });
            state.attempted.insert(provider_id.to_string(), pair);
        }
        // Snapshot under `persist_order` (not the write lock above): persists
        // are serialized AND each snapshots after every previously persisted
        // insert, so a concurrent record can't write an older map over a newer
        // row. The DB call itself stays outside `state` — persisting under it
        // would order guard-before-db and invite a deadlock.
        let _order = self.persist_order.lock();
        let (snapshot, persist) = {
            let guard = self.state.read();
            let Some(state) = guard.as_ref() else { return };
            (state.attempted.clone(), state.persist.clone())
        };
        if let Some(persist) = persist {
            persist(&snapshot);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two concurrent records must never leave the persisted row missing one of
    /// them: the last persist to run snapshots after every previously persisted
    /// insert, so it always carries both entries.
    #[test]
    fn concurrent_records_never_persist_a_regressed_row() {
        let guard = VersionGuard::new();
        let persisted: Arc<Mutex<Vec<HashMap<String, String>>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = persisted.clone();
        guard.init(HashMap::new(), move |map| sink.lock().push(map.clone()));

        std::thread::scope(|s| {
            s.spawn(|| guard.record("claude", "v1@aaaaaaaaaaaa".into()));
            s.spawn(|| guard.record("codex", "v2@bbbbbbbbbbbb".into()));
        });

        let persisted = persisted.lock();
        let last = persisted.last().expect("both records persist");
        assert!(
            last.contains_key("claude") && last.contains_key("codex"),
            "last persisted row regressed: {last:?}"
        );
    }
}
