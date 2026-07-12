use std::path::Path;
use std::sync::Arc;

use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineKind {
    SandboxExec,
    Docker,
}

impl EngineKind {
    /// The `sandbox_engine` settings-value spelling for this kind. Shared with
    /// the frontend's `SandboxEngine` type, so both sides agree on the wire
    /// strings.
    pub fn as_setting(self) -> &'static str {
        match self {
            Self::SandboxExec => "sandbox-exec",
            Self::Docker => "docker",
        }
    }

    /// Parse a `sandbox_engine` settings value. `None` for unknown values so
    /// callers pick their own fallback (spawn paths default to seatbelt).
    pub fn from_setting(value: &str) -> Option<Self> {
        match value {
            "sandbox-exec" => Some(Self::SandboxExec),
            "docker" => Some(Self::Docker),
            _ => None,
        }
    }
}

pub struct AgentLaunchCtx<'a> {
    pub agent_id: &'a str,
    /// The provider launching (`AgentRecord.provider`). Seatbelt ignores it —
    /// its profile is provider-agnostic — while the Docker engine branches on it
    /// for the per-provider image, config-dir mount, and auth (see
    /// [`sandbox::docker::DockerProvider`]).
    pub provider: &'a str,
    pub writable_root: &'a Path,
    pub rpc_dir: &'a Path,
    pub cwd: &'a Path,
    pub home: &'a Path,
    pub interactive: bool,
    /// A workflow step agent's blackboard directory
    /// (`~/.fletch/runs/<run-id>/blackboard/`), granted read-write into the
    /// sandbox on top of the writable root: seatbelt adds it as a writable
    /// subpath, Docker bind-mounts it at its identical host path. Both engines
    /// export it as `WF_BLACKBOARD`. `None` for ordinary (non-workflow) agents,
    /// which is every agent until the scheduler (S4) populates it at spawn.
    pub blackboard: Option<&'a Path>,
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
#[derive(Clone)]
pub enum KillPlan {
    Container { name: String },
}

/// Teardown handle bound at launch time. Sessions call [`KillHandle::kill`]
/// before their own process-group escalation and never inspect the variant —
/// adding an engine requires no session changes. The engine that produced the
/// plan is captured here, not looked up at kill time, so a session is always
/// torn down by the engine that launched it regardless of the current setting.
#[derive(Clone)]
pub enum KillHandle {
    /// The session's own child-handle / process-group termination is the whole
    /// story; the sandbox adds no teardown of its own (seatbelt).
    ProcessGroup,
    /// Engine-managed teardown (e.g. `docker kill` on a container that the
    /// local CLI child merely attaches to).
    Engine {
        engine: Arc<dyn SandboxEngine>,
        plan: KillPlan,
    },
}

impl KillHandle {
    /// Engine-side teardown. Callers still run their local child kill after
    /// this — for containers the local child is just the attached CLI.
    ///
    /// Note the `Result` is only the *engine* teardown's outcome. Sessions run
    /// their local child/process-group kill unconditionally regardless of it,
    /// but each combines the two differently: `pty_session` surfaces a local
    /// kill failure too, while `managed`/`exec` treat the local child as
    /// best-effort and return this engine result alone. So a caller can't read a
    /// uniform meaning from `kill()`'s `Result` across the spawn shapes.
    pub fn kill(&self) -> Result<()> {
        match self {
            Self::ProcessGroup => Ok(()),
            Self::Engine { engine, plan } => engine.kill(plan),
        }
    }

    /// A user-readable replacement for the launcher process's raw exit code,
    /// when the engine knows what it means (docker CLI 125/126/127 —
    /// daemon/image failures the user can act on). `None` = no special
    /// meaning; the session reports the plain exit status. Sessions call this
    /// when building their exit message and, as everywhere else on this type,
    /// never inspect the variant.
    pub fn describe_exit(&self, code: i32) -> Option<String> {
        match self {
            Self::ProcessGroup => None,
            Self::Engine { engine, plan } => engine.describe_exit(plan, code),
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
    fn kind(&self) -> EngineKind;

    fn launch_agent(&self, ctx: &AgentLaunchCtx, agent_bin: &str) -> Result<LaunchPlan>;

    /// Engine-side teardown for a plan this engine produced. Reached only via
    /// [`KillHandle::Engine`], which pairs the plan with its engine, so an
    /// implementation never sees another engine's plan. Default: the local
    /// child kill is sufficient.
    fn kill(&self, _plan: &KillPlan) -> Result<()> {
        Ok(())
    }

    /// A user-readable meaning for the launcher's exit `code`, if this engine
    /// reserves codes of its own (the docker CLI reserves 125/126/127 for
    /// daemon and image failures). Default: no reserved codes, sessions
    /// report the raw exit status.
    fn describe_exit(&self, _plan: &KillPlan, _code: i32) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::EngineKind;

    #[test]
    fn engine_kind_setting_round_trips() {
        for kind in [EngineKind::SandboxExec, EngineKind::Docker] {
            assert_eq!(EngineKind::from_setting(kind.as_setting()), Some(kind));
        }
    }

    #[test]
    fn engine_kind_rejects_unknown_setting_values() {
        assert_eq!(EngineKind::from_setting("podman"), None);
        assert_eq!(EngineKind::from_setting(""), None);
    }
}
