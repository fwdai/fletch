//! What the engine needs from its host.
//!
//! Engine code used to carry a `tauri::AppHandle` for two unrelated reasons: to
//! emit events, and as a service locator (`try_state`) for the supervisor, the
//! workflow service and the roadmap DB. [`EngineCtx`] is that pair made
//! explicit — a sink plus the handful of shared services — so the same engine
//! runs under a Tauri desktop today and a headless host later.

use std::sync::{Arc, OnceLock};

use crate::host::Sink;
use crate::roadmap::Db;
use crate::supervisor::Supervisor;
use crate::workflow::scheduler::WorkflowService;

/// The engine's view of its host. One instance per process, held as
/// `Arc<EngineCtx>` by everything the engine spawns.
pub struct EngineCtx {
    /// Where events go (the desktop webview, a remote subscriber, nowhere).
    pub sink: Sink,
    pub db: Db,
    /// Set once during boot, after the supervisor exists. Circular by nature —
    /// the supervisor's own methods take the ctx — so it cannot be a
    /// constructor argument; a `OnceLock` keeps [`Self::supervisor`] honest
    /// about the gap (the same gap `try_state` had before anything was managed)
    /// without a lock on the read path.
    supervisor: OnceLock<Arc<Supervisor>>,
    /// Set once during boot, for the same reason.
    workflows: OnceLock<Arc<WorkflowService>>,
    /// Is the user looking at this host right now? Drives the push-alert
    /// suppression that used to ask the desktop window directly; a host with no
    /// window answers `false`.
    pub focus: Box<dyn Fn() -> bool + Send + Sync>,
}

impl EngineCtx {
    pub fn new(sink: Sink, db: Db, focus: Box<dyn Fn() -> bool + Send + Sync>) -> Self {
        Self {
            sink,
            db,
            supervisor: OnceLock::new(),
            workflows: OnceLock::new(),
            focus,
        }
    }

    /// Publish the supervisor. Boot calls this exactly once; a second call is a
    /// bug in the boot sequence, so it warns and keeps the first.
    pub fn set_supervisor(&self, supervisor: Arc<Supervisor>) {
        if self.supervisor.set(supervisor).is_err() {
            tracing::warn!("engine ctx: supervisor already set");
        }
    }

    /// Publish the workflow service. Boot calls this exactly once.
    pub fn set_workflows(&self, workflows: Arc<WorkflowService>) {
        if self.workflows.set(workflows).is_err() {
            tracing::warn!("engine ctx: workflow service already set");
        }
    }

    /// The supervisor, or `None` before boot published it — exactly what
    /// `try_state::<Arc<Supervisor>>()` returned on the same paths.
    pub fn supervisor(&self) -> Option<Arc<Supervisor>> {
        self.supervisor.get().cloned()
    }

    /// The workflow service, or `None` before boot published it.
    pub fn workflows(&self) -> Option<Arc<WorkflowService>> {
        self.workflows.get().cloned()
    }
}

/// An `EngineCtx` over a fresh temp DB, for tests that need one to reach an
/// emitter. Returns the sink too so the test can read back what was emitted,
/// and the `TempDir` because dropping it would delete the database.
#[cfg(test)]
pub(crate) fn test_ctx() -> (
    Arc<EngineCtx>,
    Arc<super::sink::RecordingSink>,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::database::init(dir.path()).unwrap();
    let sink = Arc::new(super::sink::RecordingSink::new());
    let ctx = Arc::new(EngineCtx::new(sink.clone(), db, Box::new(|| false)));
    (ctx, sink, dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The point of the ctx: an engine emitter reached through it lands in
    /// whatever sink the host supplied, with no Tauri anywhere.
    #[test]
    fn an_engine_emitter_reaches_the_ctx_sink() {
        let (ctx, sink, _dir) = test_ctx();

        crate::workflow::journal::emit_run_deleted(ctx.sink.as_ref(), "run-1");

        assert_eq!(
            sink.events(),
            vec![("wf:run-deleted".to_string(), serde_json::json!("run-1"))]
        );
    }

    #[test]
    fn services_are_absent_until_boot_sets_them() {
        let (ctx, _sink, _dir) = test_ctx();
        assert!(ctx.supervisor().is_none());
        assert!(ctx.workflows().is_none());

        let sup = Arc::new(Supervisor::new(Arc::new(
            crate::workspace::WorkspaceManager::new(ctx.db.clone()),
        )));
        ctx.set_supervisor(sup.clone());
        assert!(ctx.supervisor().is_some());

        // A second set keeps the first rather than swapping it out from under
        // whoever already read it.
        ctx.set_supervisor(Arc::new(Supervisor::new(Arc::new(
            crate::workspace::WorkspaceManager::new(ctx.db.clone()),
        ))));
        assert!(Arc::ptr_eq(&ctx.supervisor().unwrap(), &sup));
    }

    #[test]
    fn focus_is_whatever_the_host_says() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::database::init(dir.path()).unwrap();
        let ctx = EngineCtx::new(Arc::new(crate::host::sink::NullSink), db, Box::new(|| true));
        assert!((ctx.focus)());
    }
}
