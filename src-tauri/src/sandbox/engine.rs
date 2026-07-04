use std::path::Path;
use std::sync::Arc;

use crate::error::Result;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineKind {
    SandboxExec,
    Docker,
}

#[allow(dead_code)]
pub struct AgentLaunchCtx<'a> {
    pub agent_id: &'a str,
    pub writable_root: &'a Path,
    pub rpc_dir: &'a Path,
    pub cwd: &'a Path,
    pub home: &'a Path,
    pub interactive: bool,
}

/// A resource that must outlive the launched process. Parked on the session
/// struct purely for its Drop side-effect (e.g. unlinking a profile tempfile),
/// hence the dead_code allowance — it is held, never read.
#[allow(dead_code)]
pub enum Keepalive {
    None,
    /// Seatbelt SBPL profile — `sandbox-exec -f <path>` reads it at the
    /// child's exec, and per-turn sessions respawn from the same
    /// `prefix_args`, so the file must live as long as the session.
    Profile(tempfile::NamedTempFile),
}

/// Engine-specific data describing how to tear down what was launched.
#[allow(dead_code)]
pub enum KillPlan {
    ProcessGroup,
    Container { name: String },
}

/// Teardown handle bound at launch time. Sessions call [`KillHandle::kill`]
/// before their own process-group escalation and never inspect the variant —
/// adding an engine requires no session changes. The engine that produced the
/// plan is captured here, NOT looked up at kill time: once engine selection is
/// setting-driven (slice C1), a session must be torn down by the engine that
/// launched it, regardless of what the setting says now.
pub enum KillHandle {
    /// The session's own child-handle / process-group termination is the whole
    /// story; the sandbox adds no teardown of its own (seatbelt).
    ProcessGroup,
    /// Engine-managed teardown (e.g. `docker kill` on a container that the
    /// local CLI child merely attaches to).
    #[allow(dead_code)]
    Engine {
        engine: Arc<dyn SandboxEngine>,
        plan: KillPlan,
    },
}

impl KillHandle {
    /// Engine-side teardown. Callers still run their local child kill after
    /// this — for containers the local child is just the attached CLI.
    pub fn kill(&self) -> Result<()> {
        match self {
            Self::ProcessGroup => Ok(()),
            Self::Engine { engine, plan } => engine.kill(plan),
        }
    }

    /// Whether the sandboxed process is still running, as far as the engine
    /// can tell. `ProcessGroup` children can't outlive the session's own child
    /// handle, so the session's view is authoritative and this returns true.
    /// Docker containers can die independently of the host process (daemon
    /// stop, OOM kill), which is what this exists to surface — see slice B2.
    #[allow(dead_code)]
    pub fn is_alive(&self) -> bool {
        match self {
            Self::ProcessGroup => true,
            Self::Engine { engine, plan } => engine.is_alive(plan),
        }
    }
}

pub struct LaunchPlan {
    pub program: std::path::PathBuf,
    pub prefix_args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub keepalive: Keepalive,
    pub kill: KillHandle,
}

pub trait SandboxEngine: Send + Sync {
    #[allow(dead_code)]
    fn kind(&self) -> EngineKind;

    fn launch_agent(&self, ctx: &AgentLaunchCtx, agent_bin: &str) -> Result<LaunchPlan>;

    /// Engine-side teardown for a plan this engine produced. Reached only via
    /// [`KillHandle::Engine`], which pairs the plan with its engine, so an
    /// implementation never sees another engine's plan. Default: the local
    /// child kill is sufficient.
    fn kill(&self, _plan: &KillPlan) -> Result<()> {
        Ok(())
    }

    /// Whether the sandboxed process behind `plan` is still running. Override
    /// where the sandbox can outlive or die independently of the local child
    /// (Docker); the default defers to the session's own child handle.
    fn is_alive(&self, _plan: &KillPlan) -> bool {
        true
    }
}
