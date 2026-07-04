mod engine;
mod seatbelt;

use std::sync::Arc;

pub use engine::{AgentLaunchCtx, EngineKind, Keepalive, KillPlan, LaunchPlan, SandboxEngine};
pub use seatbelt::{
    build_run_profile, cleanup_nested_rpc_roots, cleanup_nested_worktrees_roots, nested_rpc_root,
    nested_worktrees_root, profile_tempfile, SANDBOX_EXEC,
};

pub fn current_engine() -> Arc<dyn SandboxEngine> {
    Arc::new(seatbelt::SandboxExecEngine)
}
