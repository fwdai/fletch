use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineKind {
    SandboxExec,
    Docker,
    Podman,
}

impl EngineKind {
    /// Every engine Fletch ships. The authority for "iterate the engines" —
    /// notably [`super::guarantees`], which must state each engine's coverage of
    /// each guarantee, so an engine added here without a coverage declaration
    /// fails to compile.
    pub const ALL: &'static [Self] = &[Self::SandboxExec, Self::Docker, Self::Podman];

    /// The `sandbox_engine` settings-value spelling for this kind. Shared with
    /// the frontend's `SandboxEngine` type, so both sides agree on the wire
    /// strings.
    pub fn as_setting(self) -> &'static str {
        match self {
            Self::SandboxExec => "sandbox-exec",
            Self::Docker => "docker",
            Self::Podman => "podman",
        }
    }

    /// Whether this engine runs the agent inside a container. Call sites outside
    /// the sandbox module ask this instead of matching `Docker` directly, so a
    /// second container engine updates one place. Matched exhaustively: a new
    /// variant forces the decision here rather than silently landing on a
    /// default.
    pub fn is_container(self) -> bool {
        match self {
            Self::Docker | Self::Podman => true,
            Self::SandboxExec => false,
        }
    }

    /// Parse a `sandbox_engine` settings value. `None` for unknown values so
    /// callers pick their own fallback (spawn paths default to seatbelt).
    pub fn from_setting(value: &str) -> Option<Self> {
        match value {
            "sandbox-exec" => Some(Self::SandboxExec),
            "docker" => Some(Self::Docker),
            "podman" => Some(Self::Podman),
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
    /// The authoritative source repos each checkout under `writable_root` was
    /// forked from — `AgentRecord.repos[].repo_path`, the user's own repos,
    /// which the agent cannot write. The Docker engine derives its borrowed git
    /// object-store mounts from THESE user-owned paths, never from the checkout's
    /// own `.git/objects/info/alternates`: that file lives inside the container's
    /// read-write checkout, so a container agent could overwrite it to point a
    /// read-only bind mount at an arbitrary host path (`~/.ssh`, `~/.aws`, …) on
    /// the next reused-checkout relaunch (resume / switch_view). Deriving the
    /// mount set from the trusted source repos keeps that agent-writable file out
    /// of the trust boundary. Seatbelt ignores it: it shares the host filesystem,
    /// so borrowed objects need no separate mount/grant here.
    pub source_repos: &'a [PathBuf],
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
        for &kind in EngineKind::ALL {
            assert_eq!(EngineKind::from_setting(kind.as_setting()), Some(kind));
        }
    }

    #[test]
    fn only_the_container_runtimes_are_container_engines() {
        assert!(EngineKind::Docker.is_container());
        assert!(EngineKind::Podman.is_container());
        assert!(!EngineKind::SandboxExec.is_container());
    }

    #[test]
    fn engine_kind_rejects_unknown_setting_values() {
        assert_eq!(EngineKind::from_setting("containerd"), None);
        assert_eq!(EngineKind::from_setting(""), None);
    }
}
