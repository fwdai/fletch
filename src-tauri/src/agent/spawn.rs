//! Spawn specs and the `Agent` runner lifecycle (spawn / write / interrupt /
//! resize / shutdown).

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::{Error, Result};
use crate::exec_session::{ExecCallbacks, ExecExit, ExecSession, ExecSpawn};
use crate::managed_session::{ManagedExit, ManagedSession, ManagedSpawn, ToolUseBehavior};
use crate::pty_session::{PtyExit, PtySession, PtySpawn};
use crate::sandbox;
use crate::sandbox::{AgentLaunchCtx, EngineKind, LaunchPlan, SandboxEngine};

use super::args::{prepare_managed_args, prepare_pty_args};
use super::capabilities::per_turn_descriptor;
use super::probe::resolve_agent_bin;
use super::{Agent, ManagedAgent, PerTurnAgent, PerTurnDescriptor, PtyAgent, TurnArgs};

/// Parameters for spawning a per-turn runner. Unlike `SpawnSpec` there's
/// no sandbox profile (the agent sandboxes itself) and the session id is
/// optional — these agents assign one on the first turn.
pub struct PerTurnSpec {
    /// The agent's id, forwarded to the sandbox engine's launch context.
    pub agent_id: String,
    /// The agent's working directory — the primary repo's checkout.
    pub cwd: PathBuf,
    /// Sandbox writable root — the agent's parent dir (same role as
    /// `SpawnSpec::sandbox_root`). Per-turn agents now run under sandbox-exec
    /// too, so they need it to build the profile.
    pub sandbox_root: PathBuf,
    /// Session id to resume, if one has been captured already.
    pub session_id: Option<String>,
    /// Session-level model override. `None` keeps the provider CLI default.
    pub model: Option<String>,
    /// Session-level reasoning effort, re-emitted on every turn like `model`
    /// (each turn is a fresh process). `None` keeps the CLI default.
    pub effort: Option<String>,
    /// A custom agent's standing instructions, snapshotted on the session and
    /// injected into every turn (appended after Fletch's global system prompt).
    /// Includes the materialized skill index when the session has skills.
    /// `None` for a plain built-in spawn.
    pub instructions: Option<String>,
    /// The session's MCP-server snapshot, delivered by providers with a
    /// descriptor-level `mcp_args` builder (codex). Empty for plain spawns and
    /// ignored by providers without MCP support.
    pub mcp_servers: Vec<crate::agent_profile::McpServerSnapshot>,
    /// The agent's RPC mailbox dir, exposed to the child as `FLETCH_RPC_DIR`.
    pub rpc_dir: PathBuf,
    /// Sandbox engine stamped on the agent's record at creation and reused on
    /// every subsequent spawn, so a settings change never re-engines an
    /// existing agent (see `supervisor::lifecycle`).
    pub engine: EngineKind,
    /// The run blackboard dir to grant this per-turn step agent write access to
    /// (§8). `None` for a normal spawn.
    pub blackboard: Option<PathBuf>,
}

pub struct SpawnSpec<'a> {
    pub agent_id: &'a str,
    /// Claude's working directory — the primary repo's checkout.
    pub cwd: PathBuf,
    /// Sandbox writable root — the agent's parent dir, which may
    /// contain multiple per-repo checkouts as siblings of `cwd`. Writes
    /// are allowed anywhere under this path.
    pub sandbox_root: PathBuf,
    pub session_id: &'a str,
    /// True if this is the agent's first spawn (no prior conversation
    /// on disk for this session). False if we're respawning to switch
    /// views — claude should `--resume` instead of starting fresh.
    pub fresh: bool,
    /// Claude's session-level effort (`--effort <level>`), chosen at session
    /// creation and persisted on the `AgentRecord`. Applied on every spawn
    /// (fresh, view-switch, resume) so it sticks for the session. `None` =
    /// no selection; claude uses its own default. Ignored by per-turn agents,
    /// which take effort per-turn via their `thinking` build-args instead.
    pub effort: Option<&'a str>,
    /// Session-level model override. `None` keeps the provider CLI default.
    pub model: Option<&'a str>,
    /// A custom agent's standing instructions, injected after Fletch's global
    /// system prompt on every spawn/resume. Includes the materialized skill
    /// index when the session has skills. `None` for a plain built-in spawn.
    pub instructions: Option<&'a str>,
    /// The session's MCP-server snapshot, consumed by per-turn providers with
    /// a descriptor `mcp_args` builder when this spec launches their native
    /// TUI. Claude ignores it (it takes `mcp_config` instead).
    pub mcp_servers: &'a [crate::agent_profile::McpServerSnapshot],
    /// Claude's generated MCP config file, passed as
    /// `--mcp-config <path> --strict-mcp-config`. `None` = no servers attached.
    pub mcp_config: Option<&'a Path>,
    /// The agent's RPC mailbox dir, exposed to the child as `FLETCH_RPC_DIR`.
    pub rpc_dir: PathBuf,
    pub cols: u16,
    pub rows: u16,
    /// Sandbox engine stamped on the agent's record at creation and reused on
    /// every subsequent spawn (fresh, view-switch, resume), so a settings
    /// change never re-engines an existing agent (see `supervisor::lifecycle`).
    pub engine: EngineKind,
    /// The run blackboard dir to grant this agent write access to, when it is a
    /// workflow step agent (§8). `None` for a normal spawn. The sandbox engine
    /// turns it into the seatbelt subpath / Docker mount + `WF_BLACKBOARD`.
    pub blackboard: Option<&'a Path>,
}

/// The environment Fletch injects into every agent child: the absolute path to
/// its file-mailbox RPC dir. The agent posts requests there for the app to
/// execute (see `rpc.rs`). Layered on top of the inherited environment.
fn rpc_env(rpc_dir: &Path) -> Vec<(String, String)> {
    vec![(
        "FLETCH_RPC_DIR".to_string(),
        rpc_dir.to_string_lossy().into_owned(),
    )]
}

impl Agent {
    pub fn spawn_pty<F, G>(spec: SpawnSpec<'_>, on_output: F, on_exit: G) -> Result<Self>
    where
        F: Fn(Vec<u8>) + Send + 'static,
        G: Fn(PtyExit) + Send + 'static,
    {
        let home =
            dirs::home_dir().ok_or_else(|| Error::Other("HOME directory not available".into()))?;
        let engine = sandbox::engine_for(spec.engine)?;
        let claude = agent_bin_for("claude", "claude", "Claude Code", engine.as_ref(), &home)?;
        let agent_args = prepare_pty_args(&spec);

        let ctx = AgentLaunchCtx {
            agent_id: spec.agent_id,
            provider: "claude",
            writable_root: &spec.sandbox_root,
            rpc_dir: &spec.rpc_dir,
            cwd: &spec.cwd,
            home: &home,
            interactive: true,
            blackboard: spec.blackboard,
        };
        let LaunchPlan {
            program,
            prefix_args,
            env: launch_env,
            keepalive,
            kill,
        } = engine.launch_agent(&ctx, &claude)?;
        let mut args = prefix_args;
        args.extend(agent_args);
        let mut env = launch_env;
        env.extend(rpc_env(&spec.rpc_dir));

        tracing::info!(
            agent_id = %spec.agent_id,
            session = %spec.session_id,
            fresh = spec.fresh,
            cwd = %spec.cwd.display(),
            sandbox_root = %spec.sandbox_root.display(),
            argv = ?args,
            "spawning sandboxed pty agent"
        );

        let pty = PtySession::spawn(
            PtySpawn {
                program: &program,
                args: &args,
                cwd: &spec.cwd,
                env: &env,
                cols: spec.cols,
                rows: spec.rows,
                kill_plan: kill,
            },
            on_output,
            on_exit,
        )?;

        Ok(Self::Pty(PtyAgent { pty, keepalive }))
    }

    /// Launch a per-turn agent's interactive TUI in a PTY — the native view
    /// for codex/cursor/opencode/pi. Unlike claude's `spawn_pty`, the agent
    /// binary runs directly (no `sandbox-exec`): these agents self-sandbox.
    /// The session is always resumed (`spec.fresh == false`); the supervisor
    /// only routes a per-turn agent here once it has an established session
    /// id, so the TUI continues the same conversation the Custom view built.
    pub fn spawn_pty_native<F, G>(
        spec: SpawnSpec<'_>,
        provider: &str,
        on_output: F,
        on_exit: G,
    ) -> Result<Self>
    where
        F: Fn(Vec<u8>) + Send + 'static,
        G: Fn(PtyExit) + Send + 'static,
    {
        let desc = per_turn_descriptor(provider)
            .ok_or_else(|| Error::Other(format!("no per-turn descriptor for `{provider}`")))?;
        let home =
            dirs::home_dir().ok_or_else(|| Error::Other("HOME directory not available".into()))?;
        let engine = sandbox::engine_for(spec.engine)?;
        // Under docker this is the provider's in-image bin (`codex`); under
        // seatbelt the host-resolved path — same decision claude makes.
        let bin = agent_bin_for(desc.id, desc.bin, desc.label, engine.as_ref(), &home)?;
        let session = if spec.fresh {
            None
        } else {
            Some(spec.session_id)
        };
        // Provider MCP overrides (codex `-c mcp_servers.*`), rebuilt from the
        // session's snapshot so the TUI resumes with the same tool set the
        // Custom-view turns had.
        let mcp_args = desc
            .mcp_args
            .map(|build| build(spec.mcp_servers))
            .unwrap_or_default();
        let agent_args = (desc.pty_args)(session, spec.model, spec.instructions, &mcp_args);

        // Unified sandbox: run the agent's TUI under the sandbox engine (the
        // agent's own sandbox is disabled in its arg builder), so per-turn
        // agents are confined exactly like claude.
        let ctx = AgentLaunchCtx {
            agent_id: spec.agent_id,
            provider,
            writable_root: &spec.sandbox_root,
            rpc_dir: &spec.rpc_dir,
            cwd: &spec.cwd,
            home: &home,
            interactive: true,
            blackboard: spec.blackboard,
        };
        let LaunchPlan {
            program,
            prefix_args,
            env: launch_env,
            keepalive,
            kill,
        } = engine.launch_agent(&ctx, &bin)?;
        let mut args = prefix_args;
        args.extend(agent_args);
        let mut env = launch_env;
        env.extend(rpc_env(&spec.rpc_dir));

        tracing::info!(
            agent_id = %spec.agent_id,
            provider = %provider,
            session = %spec.session_id,
            fresh = spec.fresh,
            cwd = %spec.cwd.display(),
            sandbox_root = %spec.sandbox_root.display(),
            bin = %bin,
            argv = ?args,
            "spawning sandboxed native pty per-turn agent"
        );

        let pty = PtySession::spawn(
            PtySpawn {
                program: &program,
                args: &args,
                cwd: &spec.cwd,
                env: &env,
                cols: spec.cols,
                rows: spec.rows,
                kill_plan: kill,
            },
            on_output,
            on_exit,
        )?;

        Ok(Self::Pty(PtyAgent { pty, keepalive }))
    }

    pub fn spawn_managed<F, G>(spec: SpawnSpec<'_>, on_event: F, on_exit: G) -> Result<Self>
    where
        F: Fn(Value) + Send + 'static,
        G: Fn(ManagedExit) + Send + 'static,
    {
        let home =
            dirs::home_dir().ok_or_else(|| Error::Other("HOME directory not available".into()))?;
        let engine = sandbox::engine_for(spec.engine)?;
        let claude = agent_bin_for("claude", "claude", "Claude Code", engine.as_ref(), &home)?;
        let agent_args = prepare_managed_args(&spec);

        let ctx = AgentLaunchCtx {
            agent_id: spec.agent_id,
            provider: "claude",
            writable_root: &spec.sandbox_root,
            rpc_dir: &spec.rpc_dir,
            cwd: &spec.cwd,
            home: &home,
            interactive: false,
            blackboard: spec.blackboard,
        };
        let LaunchPlan {
            program,
            prefix_args,
            env: launch_env,
            keepalive,
            kill,
        } = engine.launch_agent(&ctx, &claude)?;
        let mut args = prefix_args;
        args.extend(agent_args);
        let mut env = launch_env;
        env.extend(rpc_env(&spec.rpc_dir));

        tracing::info!(
            agent_id = %spec.agent_id,
            session = %spec.session_id,
            fresh = spec.fresh,
            cwd = %spec.cwd.display(),
            sandbox_root = %spec.sandbox_root.display(),
            argv = ?args,
            "spawning sandboxed managed agent"
        );

        let session = ManagedSession::spawn(
            ManagedSpawn {
                program: &program,
                args: &args,
                cwd: &spec.cwd,
                env: &env,
                kill_plan: kill,
            },
            on_event,
            on_exit,
        )?;

        Ok(Self::Managed(ManagedAgent { session, keepalive }))
    }

    /// Build a per-turn runner (codex, cursor, opencode, pi) from its
    /// `PerTurnDescriptor`. The binary, CLI args, and session-id extraction
    /// come from the descriptor; the lifecycle is the shared `spawn_exec`.
    /// Per-turn agents hold no live process between turns — each user
    /// message spawns a fresh process — and sandbox themselves, so there's
    /// no sandbox-exec profile.
    pub fn spawn_per_turn<F, G, H>(
        desc: &PerTurnDescriptor,
        spec: PerTurnSpec,
        on_event: F,
        on_session_id: G,
        on_turn_exit: H,
    ) -> Result<Self>
    where
        F: Fn(Value) + Send + Sync + 'static,
        G: Fn(String) + Send + Sync + 'static,
        H: Fn(ExecExit) + Send + Sync + 'static,
    {
        let home =
            dirs::home_dir().ok_or_else(|| Error::Other("HOME directory not available".into()))?;
        // Under docker this resolves to the provider's in-image bin (`codex`);
        // under seatbelt the host path — same decision claude makes. Resolved
        // here (not in `spawn_exec`) since the agent bin is provider-specific.
        let engine = sandbox::engine_for(spec.engine)?;
        let program = PathBuf::from(agent_bin_for(
            desc.id,
            desc.bin,
            desc.label,
            engine.as_ref(),
            &home,
        )?);
        // A custom agent's standing brief and MCP overrides are constant for
        // the session, so bind them into the per-turn args builder once here
        // rather than threading them through every turn. `ExecSession` keeps
        // calling a 4-arg builder.
        let build_args = desc.build_args;
        let extra = spec.instructions.clone();
        let mcp_args = desc
            .mcp_args
            .map(|build| build(&spec.mcp_servers))
            .unwrap_or_default();
        Self::spawn_exec(
            desc.id,
            program,
            spec,
            move |prompt, session_id, thinking, model| {
                build_args(&TurnArgs {
                    prompt,
                    session_id,
                    thinking,
                    model,
                    extra: extra.as_deref(),
                    mcp_args: &mcp_args,
                })
            },
            desc.session_id,
            !desc.plaintext,
            ExecCallbacks {
                on_event,
                on_session_id,
                on_exit: on_turn_exit,
            },
        )
    }

    /// Shared per-turn exec lifecycle. Spawns no process yet — the first
    /// turn is launched when the first user message arrives. `on_exit`
    /// fires when a turn's process exits (and that turn is still current)
    /// — the per-turn analogue of a turn-end signal, so an interrupted or
    /// failed turn that never emits an in-band turn-end still leaves the
    /// agent promptly.
    fn spawn_exec<A, I, F, G, H>(
        provider: &str,
        program: PathBuf,
        spec: PerTurnSpec,
        build_args: A,
        extract_session_id: I,
        stdout_is_json: bool,
        cb: ExecCallbacks<F, G, H>,
    ) -> Result<Self>
    where
        A: Fn(&str, Option<&str>, Option<&str>, Option<&str>) -> Vec<String>
            + Send
            + Sync
            + 'static,
        I: Fn(&Value) -> Option<String> + Send + Sync + 'static,
        F: Fn(Value) + Send + Sync + 'static,
        G: Fn(String) + Send + Sync + 'static,
        H: Fn(ExecExit) + Send + Sync + 'static,
    {
        let home =
            dirs::home_dir().ok_or_else(|| Error::Other("HOME directory not available".into()))?;
        let agent_bin = program
            .to_str()
            .ok_or_else(|| Error::Other("agent bin path not utf-8".into()))?;

        let ctx = AgentLaunchCtx {
            agent_id: &spec.agent_id,
            provider,
            writable_root: &spec.sandbox_root,
            rpc_dir: &spec.rpc_dir,
            cwd: &spec.cwd,
            home: &home,
            interactive: false,
            blackboard: spec.blackboard.as_deref(),
        };
        let LaunchPlan {
            program: launch_program,
            prefix_args,
            env: launch_env,
            keepalive,
            kill,
        } = sandbox::engine_for(spec.engine)?.launch_agent(&ctx, agent_bin)?;
        let mut env = launch_env;
        env.extend(rpc_env(&spec.rpc_dir));

        tracing::info!(
            agent_bin = %program.display(),
            cwd = %spec.cwd.display(),
            sandbox_root = %spec.sandbox_root.display(),
            resume = spec.session_id.is_some(),
            "preparing sandboxed per-turn runner"
        );
        let session = ExecSession::new(
            ExecSpawn {
                program: launch_program,
                prefix_args,
                keepalive,
                cwd: spec.cwd,
                session_id: spec.session_id,
                model: spec.model,
                effort: spec.effort,
                stdout_is_json,
                env,
                kill_plan: kill,
            },
            build_args,
            extract_session_id,
            cb,
        );
        Ok(Self::PerTurn(PerTurnAgent { session }))
    }

    pub fn write_pty(&self, bytes: &[u8]) -> Result<()> {
        match self {
            Self::Pty(a) => a.pty.write(bytes),
            Self::Managed(_) | Self::PerTurn(_) => {
                Err(Error::Other("write_pty called on a managed agent".into()))
            }
        }
    }

    pub fn send_user_message(&self, text: &str, attachments: &[String]) -> Result<()> {
        match self {
            Self::Managed(a) => a.session.send_user_message(text, attachments),
            Self::PerTurn(a) => a.session.send_user_message(text, attachments),
            Self::Pty(_) => Err(Error::Other("send_user_message called on pty agent".into())),
        }
    }

    /// True while a turn is paused on a held permission prompt. Only the managed
    /// (Claude stream-json) transport can pause this way; per-turn and PTY
    /// agents run fully auto-approved and never gate.
    pub fn is_tool_gated(&self) -> bool {
        match self {
            Self::Managed(a) => a.session.is_tool_gated(),
            Self::PerTurn(_) | Self::Pty(_) => false,
        }
    }

    /// Answer a held user-input prompt (`AskUserQuestion` / `ExitPlanMode`) by
    /// delivering the user's selection as a control response. Only the managed
    /// (Claude stream-json) transport pauses on tools this way; per-turn and
    /// PTY agents run fully auto-approved and never surface such a prompt.
    pub fn answer_tool_use(
        &self,
        request_id: &str,
        updated_input: serde_json::Value,
        behavior: ToolUseBehavior,
        message: Option<String>,
    ) -> Result<()> {
        match self {
            Self::Managed(a) => {
                a.session
                    .answer_tool_use(request_id, updated_input, behavior, message)
            }
            Self::PerTurn(_) | Self::Pty(_) => Err(Error::Other(
                "answer_tool_use is only supported for managed agents".into(),
            )),
        }
    }

    /// Interrupt the agent's current turn without terminating the process.
    /// For PTY agents this writes Ctrl+C; for managed agents this sends SIGINT.
    pub fn interrupt(&self) {
        match self {
            Self::Pty(a) => {
                let _ = a.pty.interrupt();
            }
            Self::Managed(a) => {
                a.session.interrupt();
            }
            Self::PerTurn(a) => {
                a.session.interrupt();
            }
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        match self {
            Self::Pty(a) => a.pty.resize(cols, rows),
            Self::Managed(_) | Self::PerTurn(_) => Ok(()),
        }
    }

    /// Tear the agent's child down explicitly via the session's `&self` kill
    /// path, rather than deferring to `Drop`. Takes `&self` so it can run on an
    /// `Arc<Agent>` clone at a map-removal site: the kill is exactly what
    /// unblocks a stuck write (EPIPE), so it must not wait behind an in-flight
    /// blocked write holding another `Arc` clone. Idempotent (each session's
    /// `kill` is), and `Drop` still runs it as a last-`Arc`-drop safety net.
    pub fn shutdown(&self) -> Result<()> {
        match self {
            Self::Pty(a) => a.pty.kill(),
            Self::Managed(a) => a.session.kill(),
            Self::PerTurn(a) => a.session.kill(),
        }
    }
}

/// The agent binary handed to `launch_agent`, decided by the *resolved*
/// engine's kind: under docker it's the provider's in-image command name
/// (`claude` / `codex` — the image carries its own install; a resolved host path
/// would be meaningless, or missing, inside the container), under seatbelt the
/// host-resolved absolute path as always. Keyed off the resolved engine rather
/// than the stamped setting; a docker-stamped agent whose daemon is down never
/// reaches here — `sandbox::engine_for` fails the spawn instead of degrading to
/// seatbelt — so the resolved engine always matches the launch boundary.
///
/// Docker reaches here only for a provider `DockerProvider::from_id` accepts
/// (the `supervisor::lifecycle` gate ran first), so the `None` arm is defensive.
fn agent_bin_for(
    provider: &str,
    bin: &str,
    label: &str,
    engine: &dyn SandboxEngine,
    home: &Path,
) -> Result<String> {
    match engine.kind() {
        EngineKind::Docker => sandbox::docker::DockerProvider::from_id(provider)
            .map(|p| p.image_bin().to_string())
            .ok_or_else(|| {
                Error::Other(format!("{label} isn't available in Docker sandboxes yet"))
            }),
        EngineKind::SandboxExec => resolve_agent_bin(provider, bin, label, home),
    }
}
