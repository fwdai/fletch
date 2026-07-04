pub mod docker;
mod engine;
pub mod provision;
mod seatbelt;

use std::sync::{Arc, OnceLock};

use parking_lot::RwLock;

pub use docker::{availability as docker_availability, DockerAvailability};
pub use engine::{AgentLaunchCtx, EngineKind, Keepalive, KillHandle, LaunchPlan, SandboxEngine};
pub use seatbelt::{
    build_run_profile, cleanup_nested_rpc_roots, cleanup_nested_worktrees_roots, nested_rpc_root,
    nested_worktrees_root, profile_tempfile, SANDBOX_EXEC,
};

/// The `settings` key holding the user's engine choice; values are
/// [`EngineKind::as_setting`] spellings, default `sandbox-exec`.
pub const ENGINE_SETTING: &str = "sandbox_engine";

/// The user's engine selection, mirrored from the `sandbox_engine` setting so
/// spawn paths (deep in agent code, no DB handle) can read it. Seeded at
/// startup (`lib.rs setup`) and updated by the `set_sandbox_engine` command.
/// Teardown never consults this — sessions carry a `KillHandle` bound to the
/// engine that launched them.
static SELECTED_ENGINE: RwLock<EngineKind> = RwLock::new(EngineKind::SandboxExec);

pub fn set_selected_engine_kind(kind: EngineKind) {
    *SELECTED_ENGINE.write() = kind;
}

/// The engine a *new* agent would be stamped with right now. Existing agents
/// keep the kind stamped on their record at creation (see
/// `supervisor::lifecycle::spawn_agent`), so a settings change never
/// re-engines them.
pub fn selected_engine_kind() -> EngineKind {
    *SELECTED_ENGINE.read()
}

/// Resolve the engine for an agent stamped with `kind`, availability-checked
/// at spawn time: docker falls back to seatbelt with a warning when the
/// daemon is unreachable, so the agent still runs — sandboxed, just not
/// containerized. Callers pick the agent binary by the *returned* engine's
/// `kind()`, not by `kind`, so the fallback resolves a real host binary.
pub fn engine_for(kind: EngineKind) -> Arc<dyn SandboxEngine> {
    match kind {
        EngineKind::SandboxExec => seatbelt_engine(),
        EngineKind::Docker => match docker::availability() {
            DockerAvailability::Available { .. } => docker::DockerEngine::shared(),
            status => {
                tracing::warn!(
                    ?status,
                    "docker engine selected but unavailable; falling back to sandbox-exec"
                );
                seatbelt_engine()
            }
        },
    }
}

/// The engine matching the current setting — what a launch would use absent a
/// per-agent stamp. Launch paths resolve through `engine_for` with the kind
/// stamped on the agent's record instead; this stays for callers with no
/// record in hand (none today — B2's non-agent surfaces use it).
#[allow(dead_code)]
pub fn current_engine() -> Arc<dyn SandboxEngine> {
    engine_for(selected_engine_kind())
}

/// The seatbelt engine, shared process-wide: it is stateless, and per-launch
/// state (profile tempfile) lives on the `LaunchPlan`.
fn seatbelt_engine() -> Arc<dyn SandboxEngine> {
    static ENGINE: OnceLock<Arc<dyn SandboxEngine>> = OnceLock::new();
    ENGINE
        .get_or_init(|| Arc::new(seatbelt::SandboxExecEngine))
        .clone()
}
