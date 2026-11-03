use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineKind {
    SandboxExec,
    /// A libkrun micro-VM per agent, reached only over ssh on vsock and an
    /// egress proxy. Named here so settings, guarantees and the seams can refer
    /// to it; [`super::engine_for`] refuses it on a machine that cannot boot
    /// one.
    Box,
}

impl EngineKind {
    /// Every engine Fletch ships. The authority for "iterate the engines" —
    /// notably [`super::guarantees`], which must state each engine's coverage of
    /// each guarantee, so an engine added here without a coverage declaration
    /// fails to compile.
    pub const ALL: &'static [Self] = &[Self::SandboxExec, Self::Box];

    /// The `sandbox_engine` settings-value spelling for this kind. Shared with
    /// the frontend's `SandboxEngine` type, so both sides agree on the wire
    /// strings.
    pub fn as_setting(self) -> &'static str {
        match self {
            Self::SandboxExec => "sandbox-exec",
            Self::Box => "box",
        }
    }

    /// What this engine can and cannot do: the one table every "does this
    /// engine …" question outside `sandbox/` reads (docs/sandbox-foundation.md
    /// rule 1). Matched exhaustively, so a new variant states every fact.
    pub const fn caps(self) -> EngineCaps {
        match self {
            Self::SandboxExec => EngineCaps {
                checkout_on_host: true,
                shares_host_fs: true,
                token_only_launch: false,
                share_roots: None,
                run_panel: true,
                workflow_steps: true,
            },
            Self::Box => EngineCaps {
                checkout_on_host: false,
                shares_host_fs: false,
                token_only_launch: true,
                share_roots: Some(&["/Users", "/Volumes"]),
                run_panel: false,
                workflow_steps: false,
            },
        }
    }

    /// Parse a `sandbox_engine` settings value. `None` for unknown values so
    /// callers pick their own fallback (spawn paths default to seatbelt).
    pub fn from_setting(value: &str) -> Option<Self> {
        match value {
            "sandbox-exec" => Some(Self::SandboxExec),
            "box" => Some(Self::Box),
            _ => None,
        }
    }
}

/// One engine's facts ([`EngineKind::caps`]). A `false` is a gap the engine
/// declares, read by the feature's entry point rather than discovered by
/// probing a path (docs/sandbox-foundation.md rule 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineCaps {
    /// The agent's checkout is a real tree on this host. A box keeps it in
    /// its guest and the host path is an empty placeholder.
    pub checkout_on_host: bool,
    /// The agent sees the host's own filesystem at the host's paths, so what
    /// the OS reports for a path (symlinks resolved) is what the agent sees,
    /// and its provider CLI writes the host's own session dirs (seatbelt).
    pub shares_host_fs: bool,
    /// A claude launch can be signed in only by a token the host hands over:
    /// the engine has no view of the host's credential files, so the stored
    /// setup-token is a valid fallback.
    pub token_only_launch: bool,
    /// The roots a granted folder must lie under, `None` for any: a box guest
    /// mounts shares only under the mountpoints its image ships (both empty
    /// mountpoints; see `packaging/box-image/build.sh`).
    pub share_roots: Option<&'static [&'static str]>,
    /// The Run panel can run the agent's dev server. It runs on the host, in
    /// the checkout; a box's checkout is in its guest, which needs port
    /// forwarding over the channel first.
    pub run_panel: bool,
    /// The agent can be a workflow step. A step forks from the run repo, a
    /// host path a guest cannot see.
    pub workflow_steps: bool,
}

pub struct AgentLaunchCtx<'a> {
    pub agent_id: &'a str,
    /// The agent's project (`AgentRecord.project_id`), which keys the
    /// toolchain cache the launch prepares and is granted or mounted
    /// ([`super::toolchain_cache`]): agents of one project share a cache, and
    /// no other project's processes can write it.
    pub project_id: &'a str,
    /// The provider launching (`AgentRecord.provider`). Seatbelt ignores it —
    /// its profile is provider-agnostic — while the box engine branches on it
    /// for the provider's home and auth (`sandbox::provider`).
    pub provider: &'a str,
    pub writable_root: &'a Path,
    /// The authoritative source repos each checkout under `writable_root` was
    /// forked from — `AgentRecord.repos[].repo_path`, the user's own repos,
    /// which the agent cannot write. Seatbelt reads one back out of the macOS
    /// privacy deny, as the `--shared` clone borrows its objects.
    pub source_repos: &'a [PathBuf],
    pub rpc_dir: &'a Path,
    pub cwd: &'a Path,
    pub home: &'a Path,
    pub interactive: bool,
    /// A workflow step agent's blackboard directory
    /// (`~/.fletch/runs/<run-id>/blackboard/`), granted read-write into the
    /// sandbox on top of the writable root: seatbelt adds it as a writable
    /// subpath and exports it as `WF_BLACKBOARD`. `None` for ordinary (non-workflow) agents,
    /// which is every agent until the scheduler (S4) populates it at spawn.
    pub blackboard: Option<&'a Path>,
    /// The config dir of the managed provider account this launch runs under
    /// (`agent::accounts`). For codex it only says *which* login the launch
    /// runs under: codex itself runs in [`codex_home`](Self::codex_home) and
    /// never sees this directory. `None` = the CLI's own default account, and
    /// always `None` for claude, whose accounts reach the agent as
    /// `oauth_token` instead.
    pub account_dir: Option<&'a Path>,
    /// The claude access token the host resolved for this launch
    /// (`agent::host_login::claude`), for the account the agent is stamped with.
    /// Every engine hands it to claude as `CLAUDE_CODE_OAUTH_TOKEN`; the
    /// stored login it came from never enters the sandbox. `None` for other
    /// providers, or a default account with no host login.
    pub oauth_token: Option<&'a crate::agent::host_login::claude::AccessToken>,
    /// The per-agent `CODEX_HOME` the host assembled for a codex launch
    /// (`agent::codex_home`): shared config linked in, the agent's own
    /// sessions, and a launch credential with no refresh token. The one codex
    /// home a sandbox may write or mount. `None` for every other provider.
    pub codex_home: Option<&'a Path>,
    /// The [`adopted_workspace`](Self::adopted_workspace) is to be read, never
    /// written: the local reviewer reads the coder's checkout it adopts. Seatbelt
    /// grants it no write and denies writes under it after every grant.
    /// `false` for every other launch, including a
    /// workflow step, which adopts its run's tree to change it. Meaningless
    /// without an adopted workspace.
    pub adopted_read_only: bool,
    /// Folders on the user's machine the user granted this workspace
    /// (`AgentRecord::folder_grants`), each exposed at its own path: seatbelt
    /// re-allows it (read, or read-write with the invariant-3 deny over any
    /// `.git` inside), a box mounts
    /// it as a virtiofs share (read-only enforced by the host). Already
    /// checked (`workspace::folder_grants::check`); empty for most launches.
    pub folder_grants: &'a [crate::workspace::FolderGrant],
}

impl<'a> AgentLaunchCtx<'a> {
    /// The working tree this launch must be able to write *outside* the writable
    /// root — an adopted checkout, i.e. the workflow kernel's shared run
    /// workspace (`~/.fletch/runs/<run-id>/repo`), which every step of a run
    /// works in instead of cloning one under its own root.
    ///
    /// Derived from `cwd` rather than declared by the caller: an agent always
    /// runs in its own checkout, so a `cwd` outside `writable_root` IS an
    /// adopted tree — and a launch path that forgot to declare it would hand the
    /// agent a working directory it cannot write. Seatbelt grants it the same
    /// way it grants [`blackboard`](Self::blackboard): as a writable subpath
    /// (with its own invariant-3 deny).
    pub fn adopted_workspace(&self) -> Option<&'a Path> {
        (!self.cwd.starts_with(self.writable_root)).then_some(self.cwd)
    }
}

/// Engine-specific data describing how to tear down what was launched.
#[derive(Clone)]
pub enum KillPlan {
    /// One ssh session into a box. The session's own process-group kill ends
    /// the ssh client, and with it the guest PTY; the engine only detaches the
    /// launch, leaving the VM to the registry's idle timer. `agent_id` is the
    /// box's owner — the launching agent, or its host when the launch is a
    /// tenant's (`sandbox::boxes`).
    Box { agent_id: String, launch_id: String },
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
    /// Engine-managed teardown (a box detaches the launch).
    Engine {
        engine: Arc<dyn SandboxEngine>,
        plan: KillPlan,
    },
}

impl KillHandle {
    /// Engine-side teardown. Callers still run their local child kill after
    /// this.
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
}

/// A launch's [`KillHandle`] from its plan until a session owns it. Planning
/// can leave engine state behind that only the kill releases — a box attaches
/// the launch (holding its VM against the idle stop) and is then sent its env
/// file — so every failure between [`SandboxEngine::launch_agent`] and a
/// started session (env delivery, the session's own spawn) must run the kill.
/// Dropping the guard runs it; [`Self::start`] hands the handle to the session
/// and stands down only once the session started.
#[must_use]
pub struct KillGuard {
    kill: KillHandle,
    armed: bool,
}

impl KillGuard {
    pub fn new(kill: KillHandle) -> Self {
        Self { kill, armed: true }
    }

    /// Start the session `start` builds around the handle. On an error the
    /// session never owned the handle, so the kill runs here.
    pub fn start<T>(mut self, start: impl FnOnce(KillHandle) -> Result<T>) -> Result<T> {
        let session = start(self.kill.clone())?;
        self.armed = false;
        Ok(session)
    }
}

impl Drop for KillGuard {
    fn drop(&mut self) {
        if self.armed {
            if let Err(e) = self.kill.kill() {
                tracing::warn!(error = %e, "releasing a launch that never started failed");
            }
        }
    }
}

/// How the agent's assembled env reaches its process.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum EnvDelivery {
    /// Set on the launcher process the spawn starts, which the agent inherits
    /// — every engine whose launcher runs where the agent runs.
    #[default]
    Process,
    /// Shipped into the guest by [`SandboxEngine::deliver_env`] as a file the
    /// plan's launch command already names, because the launcher is an ssh
    /// client whose env never reaches the remote. `launch_id` is the engine's
    /// name for this launch, chosen when it built the plan.
    GuestFile { launch_id: String },
}

/// Work run on the host before each turn's process of a per-turn session is
/// spawned; an error fails the turn before anything launches.
pub type TurnPrep = Arc<dyn Fn() -> Result<()> + Send + Sync>;

/// Puts the agent's own arguments for one process of a launch where that
/// launch reads them, and returns the launcher's whole argv. Run on the host,
/// by the spawn, before that process starts; an error fails the spawn before
/// anything launches.
pub type StageArgs = Arc<dyn Fn(&[String]) -> Result<Vec<String>> + Send + Sync>;

/// How each process of a launch gets the agent's own arguments (its CLI's
/// flags, the system prompt among them). Every spawn shape goes through
/// [`LauncherArgs::for_process`], so none can hand a staged launch's arguments
/// to its launcher directly.
#[derive(Clone)]
pub enum LauncherArgs {
    /// On the launcher's argv, after these: an engine whose launcher runs
    /// where the agent runs (seatbelt: `sandbox-exec`'s profile and the agent
    /// binary).
    Prefix(Vec<String>),
    /// Staged by the engine, for each process, before it is spawned: a box
    /// writes them into its guest itself (`boxes::launch_files`), and what
    /// comes back is the launcher's whole argv. The launcher writes nothing,
    /// so the engine's teardown is ordered after every write it made.
    Staged(StageArgs),
}

impl LauncherArgs {
    /// The launcher's argv for one process whose agent takes `agent`.
    pub fn for_process(&self, agent: &[String]) -> Result<Vec<String>> {
        match self {
            Self::Prefix(prefix) => Ok(prefix.iter().chain(agent).cloned().collect()),
            Self::Staged(stage) => stage(agent),
        }
    }
}

pub struct LaunchPlan {
    pub program: std::path::PathBuf,
    pub launcher_args: LauncherArgs,
    pub env: Vec<(String, String)>,
    /// Vars to strip from the child's inherited and login-shell env (merged
    /// into the session's `env_remove`, applied before `env`). Seatbelt drops
    /// `SSH_AUTH_SOCK` here.
    pub env_remove: Vec<String>,
    pub env_delivery: EnvDelivery,
    /// The engine's own step before every turn of a per-turn session, run
    /// after the host refreshed what the launch's credential comes from
    /// (`agent::spawn` chains the two): the box engine ships the refreshed
    /// credential file into the guest here. `None` for an engine whose
    /// launcher runs where the agent runs, and ignored by the long-lived
    /// session shapes, which get their credential once at launch.
    pub before_turn: Option<TurnPrep>,
    /// The engine's own step once a process of this launch has exited, run
    /// by the session's reaper before it reports the exit: after each turn's
    /// process of a per-turn session, after the one process of a long-lived
    /// session. The box engine writes back what the CLI changed in its
    /// credential files here — after the process, so nothing races it, and
    /// after every turn, so a host crash between turns loses nothing.
    pub after_exit: Option<TurnPrep>,
    pub kill: KillHandle,
    /// Whether the claude this plan runs — the binary the engine committed to
    /// in `launch_agent`: the host's or a box's tools
    /// disk's — parses `--thinking-display`, so the custom view can ask it for
    /// thinking summaries (`agent::args::prepare_managed_args`). A claude that
    /// doesn't parse the flag refuses to launch with it, so this is evidence
    /// about that very binary (`agent::probe`), never a guess from another;
    /// `false` for any other provider and wherever there is no evidence.
    pub claude_thinking_summaries: bool,
}

pub trait SandboxEngine: Send + Sync {
    fn kind(&self) -> EngineKind;

    fn launch_agent(&self, ctx: &AgentLaunchCtx, agent_bin: &str) -> Result<LaunchPlan>;

    /// Ship `env` into the guest for launch `launch_id` of `agent_id`, and
    /// return the guest path it was written to — the one the plan's
    /// launch command already names. Called by the spawn shapes in
    /// `agent::spawn` only for a plan whose delivery is
    /// [`EnvDelivery::GuestFile`], so an engine that delivers in-process never
    /// implements it; the box engine does.
    fn deliver_env(
        &self,
        _agent_id: &str,
        _launch_id: &str,
        _env: &[(String, String)],
    ) -> Result<PathBuf> {
        Err(Error::Other("this engine delivers env in-process".into()))
    }

    /// Engine-side teardown for a plan this engine produced. Reached only via
    /// [`KillHandle::Engine`], which pairs the plan with its engine, so an
    /// implementation never sees another engine's plan. Default: the local
    /// child kill is sufficient.
    fn kill(&self, _plan: &KillPlan) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentLaunchCtx, EngineKind};
    use std::path::{Path, PathBuf};

    fn ctx<'a>(writable_root: &'a Path, cwd: &'a Path) -> AgentLaunchCtx<'a> {
        AgentLaunchCtx {
            agent_id: "yosemite",
            project_id: "p1",
            provider: "claude",
            writable_root,
            source_repos: &[],
            rpc_dir: Path::new("/rpc/yosemite"),
            cwd,
            home: Path::new("/home/u"),
            interactive: true,
            blackboard: None,
            account_dir: None,
            oauth_token: None,
            codex_home: None,
            adopted_read_only: false,
            folder_grants: &[],
        }
    }

    /// An ordinary agent's cwd is one of its own checkouts under the writable
    /// root, so it needs no grant of its own; a kernel step's cwd is the run's
    /// shared tree, which does.
    #[test]
    fn adopted_workspace_is_the_cwd_only_when_it_escapes_the_writable_root() {
        let root = PathBuf::from("/w/yosemite");
        assert_eq!(ctx(&root, &root.join("repo")).adopted_workspace(), None);
        assert_eq!(ctx(&root, &root).adopted_workspace(), None);

        let run_tree = PathBuf::from("/runs/run-7/repo");
        assert_eq!(
            ctx(&root, &run_tree).adopted_workspace(),
            Some(run_tree.as_path())
        );
    }

    #[test]
    fn engine_kind_setting_round_trips() {
        for &kind in EngineKind::ALL {
            assert_eq!(EngineKind::from_setting(kind.as_setting()), Some(kind));
        }
    }

    /// The fact table as docs/sandbox-foundation.md states it.
    #[test]
    fn caps_match_the_charter_table() {
        use EngineKind::{Box, SandboxExec};
        let with = |fact: fn(EngineKind) -> bool| -> Vec<EngineKind> {
            EngineKind::ALL
                .iter()
                .copied()
                .filter(|&kind| fact(kind))
                .collect()
        };
        assert_eq!(
            with(|k| !k.caps().checkout_on_host),
            vec![Box],
            "only a box keeps the checkout off the host"
        );
        assert_eq!(with(|k| k.caps().shares_host_fs), vec![SandboxExec]);
        assert_eq!(with(|k| k.caps().token_only_launch), vec![Box]);
        assert_eq!(with(|k| k.caps().share_roots.is_some()), vec![Box]);
        assert_eq!(Box.caps().share_roots, Some(&["/Users", "/Volumes"][..]));
        assert_eq!(with(|k| !k.caps().run_panel), vec![Box]);
        assert_eq!(with(|k| !k.caps().workflow_steps), vec![Box]);
    }

    #[test]
    fn engine_kind_rejects_unknown_setting_values() {
        assert_eq!(EngineKind::from_setting("containerd"), None);
        assert_eq!(EngineKind::from_setting("docker"), None);
        assert_eq!(EngineKind::from_setting(""), None);
    }

    /// A plan's kill runs exactly when no session took it: the guard dropped
    /// on the way (a failed env delivery) or a session whose start failed. A
    /// started session owns it, and the guard leaves it alone.
    #[test]
    fn a_kill_guard_kills_unless_a_session_started() {
        use super::{KillGuard, KillHandle, KillPlan, LaunchPlan, SandboxEngine};
        use crate::error::{Error, Result};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        #[derive(Default)]
        struct Counting(AtomicUsize);
        impl SandboxEngine for Counting {
            fn kind(&self) -> EngineKind {
                EngineKind::Box
            }
            fn launch_agent(&self, _: &AgentLaunchCtx, _: &str) -> Result<LaunchPlan> {
                unreachable!("the guard never plans")
            }
            fn kill(&self, _: &KillPlan) -> Result<()> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }
        let engine = Arc::new(Counting::default());
        let guard = || {
            KillGuard::new(KillHandle::Engine {
                engine: engine.clone(),
                plan: KillPlan::Box {
                    agent_id: "a".into(),
                    launch_id: "l".into(),
                },
            })
        };
        let kills = || engine.0.load(Ordering::SeqCst);

        drop(guard());
        assert_eq!(kills(), 1);

        let failed: Result<()> = guard().start(|_| Err(Error::Other("spawn failed".into())));
        assert!(failed.is_err());
        assert_eq!(kills(), 2);

        let session = guard().start(Ok).unwrap();
        assert_eq!(kills(), 2);
        session.kill().unwrap();
        assert_eq!(kills(), 3);
    }
}
