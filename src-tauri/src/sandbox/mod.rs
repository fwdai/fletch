pub mod docker;
mod engine;
pub mod provision;
mod seatbelt;

use std::sync::{Arc, OnceLock};

pub use engine::{AgentLaunchCtx, Keepalive, KillHandle, LaunchPlan, SandboxEngine};
pub use seatbelt::{
    build_run_profile, cleanup_nested_rpc_roots, cleanup_nested_worktrees_roots, nested_rpc_root,
    nested_worktrees_root, profile_tempfile, SANDBOX_EXEC,
};

/// The engine used for agent launches. Hardcoded to seatbelt until slice C1
/// makes it setting-driven; resolved once so every launch in a process run
/// shares the same instance. Teardown never consults this — sessions carry a
/// `KillHandle` bound to the engine that launched them.
pub fn current_engine() -> Arc<dyn SandboxEngine> {
    static ENGINE: OnceLock<Arc<dyn SandboxEngine>> = OnceLock::new();
    ENGINE
        .get_or_init(|| Arc::new(seatbelt::SandboxExecEngine))
        .clone()
}
