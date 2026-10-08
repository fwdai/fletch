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

use super::accounts;
use super::args::{prepare_managed_args, prepare_pty_args};
use super::capabilities::{mcp_delivery, per_turn_descriptor};
use super::codex_home;
use super::host_login;
use super::host_login::claude::AccessToken;
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
    /// The authoritative source repos this agent's checkouts were forked from
    /// (`AgentRecord.repos[].repo_path`). Threaded to the sandbox engine so the
    /// Docker borrowed-object-store mounts derive from these user-owned paths,
    /// never from the agent-writable checkout alternates (see `AgentLaunchCtx`).
    pub source_repos: Vec<PathBuf>,
    /// Session id to resume, if one has been captured already.
    pub session_id: Option<String>,
    /// A custom agent's standing instructions, snapshotted on the session and
    /// injected into every turn (appended after Fletch's global system prompt).
    /// Includes the materialized skill index when the session has skills.
    /// `None` for a plain built-in spawn.
    pub instructions: Option<String>,
    /// The session's MCP-server snapshot, delivered by the provider's
    /// `McpDeliveryBuilder` (see `agent::mcp_delivery`). Empty for plain spawns
    /// and ignored by providers without MCP support.
    pub mcp_servers: Vec<crate::agent_profile::McpServerSnapshot>,
    /// The agent's RPC mailbox dir, exposed to the child as `FLETCH_RPC_DIR`.
    pub rpc_dir: PathBuf,
    /// Sandbox engine stamped on the agent's record at creation and reused on
    /// every subsequent spawn, so a settings change never re-engines an
    /// existing agent (see `supervisor::lifecycle`).
    pub engine: EngineKind,
    /// Provider account stamped on the agent's record at creation (see
    /// `SpawnSpec::account`). `None` = the CLI's own default account.
    pub account: Option<String>,
    /// The run blackboard dir to grant this per-turn step agent write access to
    /// (§8). `None` for a normal spawn.
    pub blackboard: Option<PathBuf>,
}

/// How a claude launch attaches to its conversation (see `args::session_args`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStart {
    /// A new, empty conversation under the agent's own session id.
    Fresh,
    /// Continue the agent's own session — one it ran, or one Fletch wrote for
    /// it (`Supervisor::materialize`).
    Resume,
}

impl SessionStart {
    /// A categorical label for logs: no ids, so it may egress (`sentry_scrub`).
    pub fn label(&self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Resume => "resume",
        }
    }
}

pub struct SpawnSpec<'a> {
    pub agent_id: &'a str,
    /// Claude's working directory — the primary repo's checkout.
    pub cwd: PathBuf,
    /// Sandbox writable root — the agent's parent dir, which may
    /// contain multiple per-repo checkouts as siblings of `cwd`. Writes
    /// are allowed anywhere under this path.
    pub sandbox_root: PathBuf,
    /// The authoritative source repos this agent's checkouts were forked from
    /// (`AgentRecord.repos[].repo_path`). Threaded to the sandbox engine so the
    /// Docker borrowed-object-store mounts derive from these user-owned paths,
    /// never from the agent-writable checkout alternates (see `AgentLaunchCtx`).
    pub source_repos: &'a [PathBuf],
    pub session_id: &'a str,
    /// How claude attaches to `session_id`: fresh until its transcript holds a
    /// message, resumed from then on (view switch, respawn, restore). Per-turn
    /// agents' native view always resumes and ignores it.
    pub start: SessionStart,
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
    /// Whether the codegraph MCP server actually landed in this session (see
    /// `codegraph::McpInjection`). Gates the Fletch-defined `codegraph`
    /// subagent the same way it gates the instruction block: never define a
    /// subagent whose only tool the agent wasn't given.
    pub codegraph_available: bool,
    /// The session's MCP-server snapshot. Delivered by the provider's
    /// `McpDeliveryBuilder` at spawn (see `agent::mcp_delivery`); ignored for
    /// providers that have no MCP surface.
    pub mcp_servers: &'a [crate::agent_profile::McpServerSnapshot],
    /// The agent's RPC mailbox dir, exposed to the child as `FLETCH_RPC_DIR`.
    pub rpc_dir: PathBuf,
    pub cols: u16,
    pub rows: u16,
    /// Sandbox engine stamped on the agent's record at creation and reused on
    /// every subsequent spawn (fresh, view-switch, resume), so a settings
    /// change never re-engines an existing agent (see `supervisor::lifecycle`).
    pub engine: EngineKind,
    /// Provider account stamped on the agent's record at creation and reused on
    /// every spawn, like `engine` (`agent::accounts`). `None` = the CLI's own
    /// default account.
    pub account: Option<&'a str>,
    /// The access token a claude launch signs in with, resolved (and
    /// refreshed) by the host for `account` — see `host_login::claude::launch_token`.
    /// `None` for per-turn providers.
    pub oauth_token: Option<&'a AccessToken>,
    /// The run blackboard dir to grant this agent write access to, when it is a
    /// workflow step agent (§8). `None` for a normal spawn. The sandbox engine
    /// turns it into the seatbelt subpath / Docker mount + `WF_BLACKBOARD`.
    pub blackboard: Option<&'a Path>,
}

/// The environment Fletch injects into every agent child: the absolute path to
/// its file-mailbox RPC dir (the agent posts requests there for the app to
/// execute, see `rpc.rs`), plus the flag that arms or disarms the attribution
/// `commit-msg` hook from "Remove agent attribution" (see `attribution`).
/// Layered on top of the inherited environment.
fn rpc_env(rpc_dir: &Path) -> Vec<(String, String)> {
    let mut env = vec![(
        "FLETCH_RPC_DIR".to_string(),
        rpc_dir.to_string_lossy().into_owned(),
    )];
    env.extend(crate::attribution::agent_env());
    env
}

/// What a launch needs from the managed account it runs under: a codex
/// account's dir, naming the login the agent's [`CodexHome`] copies from (no
/// CLI runs in it; a claude account signs in by token instead), and the default
/// account's credential vars to strip from the child so the CLI can only
/// authenticate as that account (the container engines filter the same vars
/// in their auth chain; host-side launches inherit the login shell, so they
/// must drop them here — an ambient `ANTHROPIC_API_KEY` would outrank the
/// account's token). The sessions apply `unset` to the inherited layers only,
/// before the launch plan's own env: the account's token, which the plan
/// carries under one of those names, is kept. All empty for the default
/// account.
#[derive(Default)]
struct AccountLaunch {
    dir: Option<PathBuf>,
    unset: Vec<String>,
}

/// The managed account a launch of `provider` runs under. A stamped account
/// whose dir was removed fails the launch rather than recreating it signed
/// out, or silently running the agent under another login. Nothing for the
/// default account. No CLI runs in an account dir: a codex account's dir is
/// repaired,
/// so shared config linked since the last launch is there for the host's own
/// codex runs, and handed on as the login the agent's [`CodexHome`] copies
/// from; a claude account signs in by token.
fn account_launch(provider: &str, account: Option<&str>) -> Result<AccountLaunch> {
    let Some(id) = account.filter(|id| !accounts::is_default(id)) else {
        return Ok(AccountLaunch::default());
    };
    accounts::existing_account_dir(provider, Some(id))?;
    let unset = accounts::ambient_credential_vars(provider)
        .iter()
        .map(|v| v.to_string())
        .collect();
    let dir = if provider == "codex" {
        Some(accounts::ensure_account_dir(provider, id)?)
    } else {
        None
    };
    Ok(AccountLaunch { dir, unset })
}

/// The `CODEX_HOME` a codex launch runs in: the per-agent overlay under its
/// writable root, holding the agent's own sessions, the shared config linked
/// in, and a launch credential with no refresh token, which the host writes
/// fresh from the account's login (`source`) before the launch and before
/// every turn (see `codex_home` and `host_login::codex`). The sandbox sees neither the account's
/// directory nor its refresh token.
struct CodexHome {
    overlay: PathBuf,
    source: PathBuf,
}

impl CodexHome {
    /// Assemble agent `agent_id`'s overlay for a codex launch (`None` for any
    /// other provider) and write its first credential.
    fn prepare(
        provider: &str,
        agent_id: &str,
        account_dir: Option<&Path>,
        home: &Path,
        session_id: Option<&str>,
    ) -> Result<Option<Self>> {
        if provider != "codex" {
            return Ok(None);
        }
        Self::prepare_at(
            codex_home::overlay_for_agent(agent_id)?,
            account_dir,
            home,
            session_id,
        )
        .map(Some)
    }

    /// [`prepare`](Self::prepare) for the overlay at `overlay`. A thread the
    /// agent ran before it had an overlay is copied in so `resume` still
    /// finds it.
    fn prepare_at(
        overlay: PathBuf,
        account_dir: Option<&Path>,
        home: &Path,
        session_id: Option<&str>,
    ) -> Result<Self> {
        codex_home::prepare_overlay(&overlay, home)?;
        if let Some(id) = session_id {
            super::providers::codex::adopt_legacy_rollouts(id, &overlay)?;
        }
        let this = Self {
            overlay,
            source: host_login::codex::source_home(account_dir, home),
        };
        this.write_credential()?;
        Ok(this)
    }

    fn write_credential(&self) -> Result<()> {
        codex_home::write_launch_credential(
            &self.source,
            &self.overlay,
            &host_login::codex::http_refresh,
            chrono::Utc::now().timestamp(),
        )
    }

    fn env(&self) -> (String, String) {
        (
            "CODEX_HOME".to_string(),
            self.overlay.to_string_lossy().into_owned(),
        )
    }
}

/// Run `provider`'s MCP-delivery builder over the session's snapshot, writing
/// whatever config file the provider reads and returning the argv/env that
/// points at it. An empty delivery when the provider has no MCP surface, which
/// is exactly the "snapshot not consumed" behavior those providers had before.
///
/// Called on every launch path (fresh, resume, view-switch) rather than
/// persisted, so a config file is always regenerated from the current snapshot
/// — the same reasoning as `write_claude_mcp_config` and the skill index.
fn resolve_mcp(
    provider: &str,
    servers: &[crate::agent_profile::McpServerSnapshot],
    sandbox_root: &Path,
) -> Result<crate::agent_profile::McpDelivery> {
    match mcp_delivery(provider) {
        Some(build) => build(&crate::agent_profile::McpTarget {
            servers,
            sandbox_root,
        }),
        None => Ok(crate::agent_profile::McpDelivery::default()),
    }
}

/// The provider-identity half of a per-turn launch: what to run, under whose
/// name, and the extra environment its MCP delivery asked for. Bundled so
/// `spawn_exec` keeps a readable signature as delivery grows inputs.
struct ExecLaunch<'a> {
    /// Provider id, forwarded to the sandbox engine's launch context.
    provider: &'a str,
    /// Resolved agent binary (host path under seatbelt, image bin under docker).
    program: PathBuf,
    /// Extra environment from the provider's MCP delivery, layered on after the
    /// engine's launch env and `FLETCH_RPC_DIR`.
    mcp_env: Vec<(String, String)>,
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
        let mcp = resolve_mcp("claude", spec.mcp_servers, &spec.sandbox_root)?;
        let agent_args = prepare_pty_args(&spec, &mcp.args);
        let account = account_launch("claude", spec.account)?;

        let ctx = AgentLaunchCtx {
            agent_id: spec.agent_id,
            provider: "claude",
            writable_root: &spec.sandbox_root,
            source_repos: spec.source_repos,
            rpc_dir: &spec.rpc_dir,
            cwd: &spec.cwd,
            home: &home,
            interactive: true,
            blackboard: spec.blackboard,
            account_dir: account.dir.as_deref(),
            oauth_token: spec.oauth_token,
            codex_home: None,
        };
        let LaunchPlan {
            program,
            prefix_args,
            env: launch_env,
            kill,
        } = engine.launch_agent(&ctx, &claude)?;
        let mut args = prefix_args;
        args.extend(agent_args);
        let mut env = launch_env;
        env.extend(rpc_env(&spec.rpc_dir));
        env.extend(mcp.env);

        tracing::info!(
            agent_id = %spec.agent_id,
            session = %spec.session_id,
            start = spec.start.label(),
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
                env_remove: &account.unset,
                kill_plan: kill,
            },
            on_output,
            on_exit,
        )?;

        Ok(Self::Pty(PtyAgent {
            pty,
            login_expires_at_ms: spec.oauth_token.map(AccessToken::expires_at_ms),
        }))
    }

    /// Launch a per-turn agent's interactive TUI in a PTY — the native view
    /// for codex/cursor/opencode/pi. Unlike claude's `spawn_pty`, the agent
    /// binary runs directly (no `sandbox-exec`): these agents self-sandbox.
    /// The session is always resumed (`spec.start` is ignored); the supervisor
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
        // Provider MCP delivery, rebuilt from the session's snapshot so the TUI
        // resumes with the same tool set the Custom-view turns had.
        let mcp = resolve_mcp(provider, spec.mcp_servers, &spec.sandbox_root)?;
        let agent_args = (desc.pty_args)(
            Some(spec.session_id),
            spec.model,
            spec.instructions,
            &mcp.args,
        );
        let account = account_launch(provider, spec.account)?;
        // The TUI is one long process and gets its credential once, here: a
        // TUI left open past the access token's expiry has to be reopened.
        let codex = CodexHome::prepare(
            provider,
            spec.agent_id,
            account.dir.as_deref(),
            &home,
            Some(spec.session_id),
        )?;

        // Unified sandbox: run the agent's TUI under the sandbox engine (the
        // agent's own sandbox is disabled in its arg builder), so per-turn
        // agents are confined exactly like claude.
        let ctx = AgentLaunchCtx {
            agent_id: spec.agent_id,
            provider,
            writable_root: &spec.sandbox_root,
            source_repos: spec.source_repos,
            rpc_dir: &spec.rpc_dir,
            cwd: &spec.cwd,
            home: &home,
            interactive: true,
            blackboard: spec.blackboard,
            account_dir: account.dir.as_deref(),
            oauth_token: None,
            codex_home: codex.as_ref().map(|c| c.overlay.as_path()),
        };
        let LaunchPlan {
            program,
            prefix_args,
            env: launch_env,
            kill,
        } = engine.launch_agent(&ctx, &bin)?;
        let mut args = prefix_args;
        args.extend(agent_args);
        let mut env = launch_env;
        env.extend(rpc_env(&spec.rpc_dir));
        env.extend(mcp.env);
        env.extend(codex.as_ref().map(CodexHome::env));

        tracing::info!(
            agent_id = %spec.agent_id,
            provider = %provider,
            session = %spec.session_id,
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
                env_remove: &account.unset,
                kill_plan: kill,
            },
            on_output,
            on_exit,
        )?;

        Ok(Self::Pty(PtyAgent {
            pty,
            login_expires_at_ms: None,
        }))
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
        let mcp = resolve_mcp("claude", spec.mcp_servers, &spec.sandbox_root)?;
        let agent_args = prepare_managed_args(&spec, &mcp.args);
        let account = account_launch("claude", spec.account)?;

        let ctx = AgentLaunchCtx {
            agent_id: spec.agent_id,
            provider: "claude",
            writable_root: &spec.sandbox_root,
            source_repos: spec.source_repos,
            rpc_dir: &spec.rpc_dir,
            cwd: &spec.cwd,
            home: &home,
            interactive: false,
            blackboard: spec.blackboard,
            account_dir: account.dir.as_deref(),
            oauth_token: spec.oauth_token,
            codex_home: None,
        };
        let LaunchPlan {
            program,
            prefix_args,
            env: launch_env,
            kill,
        } = engine.launch_agent(&ctx, &claude)?;
        let mut args = prefix_args;
        args.extend(agent_args);
        let mut env = launch_env;
        env.extend(rpc_env(&spec.rpc_dir));
        env.extend(mcp.env);

        tracing::info!(
            agent_id = %spec.agent_id,
            session = %spec.session_id,
            start = spec.start.label(),
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
                env_remove: &account.unset,
                kill_plan: kill,
            },
            on_event,
            on_exit,
        )?;

        Ok(Self::Managed(ManagedAgent {
            session,
            login_expires_at_ms: spec.oauth_token.map(AccessToken::expires_at_ms),
        }))
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
        let mcp = resolve_mcp(desc.id, &spec.mcp_servers, &spec.sandbox_root)?;
        let mcp_args = mcp.args;
        Self::spawn_exec(
            ExecLaunch {
                provider: desc.id,
                program,
                mcp_env: mcp.env,
            },
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
        launch: ExecLaunch<'_>,
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
        let ExecLaunch {
            provider,
            program,
            mcp_env,
        } = launch;
        let home =
            dirs::home_dir().ok_or_else(|| Error::Other("HOME directory not available".into()))?;
        let agent_bin = program
            .to_str()
            .ok_or_else(|| Error::Other("agent bin path not utf-8".into()))?;
        // Resolved once for the session: every turn's process runs under the
        // same account, from the env captured here.
        let account = account_launch(provider, spec.account.as_deref())?;
        let codex = CodexHome::prepare(
            provider,
            &spec.agent_id,
            account.dir.as_deref(),
            &home,
            spec.session_id.as_deref(),
        )?;

        let ctx = AgentLaunchCtx {
            agent_id: &spec.agent_id,
            provider,
            writable_root: &spec.sandbox_root,
            source_repos: &spec.source_repos,
            rpc_dir: &spec.rpc_dir,
            cwd: &spec.cwd,
            home: &home,
            interactive: false,
            blackboard: spec.blackboard.as_deref(),
            account_dir: account.dir.as_deref(),
            oauth_token: None,
            codex_home: codex.as_ref().map(|c| c.overlay.as_path()),
        };
        let LaunchPlan {
            program: launch_program,
            prefix_args,
            env: launch_env,
            kill,
        } = sandbox::engine_for(spec.engine)?.launch_agent(&ctx, agent_bin)?;
        let mut env = launch_env;
        env.extend(rpc_env(&spec.rpc_dir));
        env.extend(mcp_env);
        env.extend(codex.as_ref().map(CodexHome::env));
        // Every turn is a fresh process, so every turn gets a fresh credential.
        let before_turn = codex.map(|codex| {
            let codex = std::sync::Arc::new(codex);
            std::sync::Arc::new(move || codex.write_credential()) as crate::exec_session::TurnPrep
        });

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
                cwd: spec.cwd,
                session_id: spec.session_id,
                stdout_is_json,
                env,
                env_remove: account.unset,
                kill_plan: kill,
                before_turn,
            },
            build_args,
            extract_session_id,
            cb,
        );
        Ok(Self::PerTurn(PerTurnAgent {
            session: Box::new(session),
        }))
    }

    pub fn write_pty(&self, bytes: &[u8]) -> Result<()> {
        match self {
            Self::Pty(a) => a.pty.write(bytes),
            Self::Managed(_) | Self::PerTurn(_) => {
                Err(Error::Other("write_pty called on a managed agent".into()))
            }
        }
    }

    /// Deliver a user turn. `model`/`effort` are the session's current config,
    /// resolved from the record at dispatch (see `deliver_user_message`) and
    /// used only by per-turn runners, which bake them into that turn's argv.
    /// claude (Managed) ignores them — its config is fixed on the persistent
    /// process at spawn and changed via a session-preserving respawn instead.
    /// The native (Pty) view has no structured input channel, so a message is
    /// typed into the TUI exactly as a user would — clearing whatever was in
    /// the editor first (see [`pty_input_keystrokes`]); model/effort are
    /// likewise fixed on its running process.
    pub fn send_user_message(
        &self,
        text: &str,
        attachments: &[String],
        model: Option<&str>,
        effort: Option<&str>,
    ) -> Result<()> {
        match self {
            Self::Managed(a) => a.session.send_user_message(text, attachments),
            Self::PerTurn(a) => a
                .session
                .send_user_message(text, attachments, model, effort),
            Self::Pty(a) => a.pty.write(&pty_input_keystrokes(text, attachments)),
        }
    }

    /// When the claude access token this process runs on lapses; `None` for a
    /// process launched without a host token (per-turn providers, or a default
    /// account with no host login).
    pub fn login_expires_at_ms(&self) -> Option<i64> {
        match self {
            Self::Pty(a) => a.login_expires_at_ms,
            Self::Managed(a) => a.login_expires_at_ms,
            Self::PerTurn(_) => None,
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

/// Put the TUI's line editor into a known-empty state: Ctrl-E (end of line)
/// then Ctrl-U (kill line). Correct under both readline semantics — where
/// Ctrl-U clears the whole line the Ctrl-E is a harmless no-op, and where it
/// only kills back to the cursor, Ctrl-E has already moved the cursor to the
/// end. `NativeInputTracker` models 0x15 as a line clear for the same reason.
const CLEAR_LINE: &[u8] = &[0x05, 0x15];

/// Render an app-originated message as the exact keystrokes a user would type
/// into an agent's TUI: clear the editor, type one line, submit with CR.
///
/// Three ways this can otherwise submit something other than what we recorded
/// as the turn — all of them make the agent run a conversation that diverges
/// from `session_user_turns`:
///
/// 1. **The editor is not empty.** A half-typed prompt may be sitting at the
///    prompt when a git-action trigger or a queued follow-up is delivered.
///    Typing on top of it submits `<their draft><our message>`, so the editor
///    is cleared first. (That discards the draft — unavoidable, and the right
///    trade for "send this message now"; the app also drops its mirror of it,
///    see `Supervisor::reset_native_input`.)
/// 2. **Embedded newlines submit early.** A TUI reads LF/CR as "send now", so
///    an unflattened multi-line prompt arrives as several truncated turns
///    instead of one.
/// 3. **Other control bytes steer the TUI rather than entering it.** Ctrl-C
///    interrupts, ESC opens modes, Ctrl-U clears. These are dropped — in the
///    message *and* in attachment paths, which are ordinary bytes on Unix and
///    may legally contain a newline or a control character.
pub(super) fn pty_input_keystrokes(text: &str, attachments: &[String]) -> Vec<u8> {
    let mut line = String::new();
    push_typed(&mut line, text);
    for path in attachments {
        // Only separate from preceding content — a message that is nothing but
        // an attachment must not submit a leading space.
        if !line.is_empty() {
            line.push(' ');
        }
        push_typed(&mut line, path);
    }

    let mut out = Vec::with_capacity(CLEAR_LINE.len() + line.len() + 1);
    out.extend_from_slice(CLEAR_LINE);
    out.extend_from_slice(line.as_bytes());
    out.push(b'\r'); // the one and only submit
    out
}

/// Append `s` as literal typing. Newlines become a single space so words stay
/// separated (a run of them collapses rather than padding the prompt), and every
/// other control character is dropped.
fn push_typed(out: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '\r' | '\n' => {
                if !out.ends_with(' ') && !out.is_empty() {
                    out.push(' ');
                }
            }
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
}

/// The agent binary handed to `launch_agent`, decided by the *resolved*
/// engine's kind: under a container engine it's the provider's in-image command
/// name (`claude` / `codex` — the image carries its own install; a resolved host
/// path would be meaningless, or missing, inside the container), under seatbelt
/// the host-resolved absolute path as always. Keyed off the resolved engine rather
/// than the stamped setting; a docker-stamped agent whose daemon is down never
/// reaches here — `sandbox::engine_for` fails the spawn instead of degrading to
/// seatbelt — so the resolved engine always matches the launch boundary.
///
/// Docker reaches here only for a provider `DockerProvider::from_id` accepts
/// (the `supervisor::lifecycle` gate ran first), so the `None` case is defensive.
fn agent_bin_for(
    provider: &str,
    bin: &str,
    label: &str,
    engine: &dyn SandboxEngine,
    home: &Path,
) -> Result<String> {
    if engine.kind().is_container() {
        sandbox::docker::DockerProvider::from_id(provider)
            .map(|p| p.image_bin().to_string())
            .ok_or_else(|| {
                Error::Other(format!(
                    "{label} isn't available in container sandboxes yet"
                ))
            })
    } else {
        resolve_agent_bin(provider, bin, label, home)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

    /// A claude account launches in the shared default config dir: nothing
    /// relocates the CLI or hands the engine the account dir, and the default
    /// account's ambient credentials are stripped so they can't outrank the
    /// account's token.
    #[test]
    fn a_claude_account_launch_relocates_nothing_and_strips_ambient_credentials() {
        accounts::with_test_root(|root| {
            std::fs::create_dir_all(root.join("claude").join("work")).unwrap();
            let launch = account_launch("claude", Some("work")).unwrap();
            assert!(launch.dir.is_none());
            for var in [
                "ANTHROPIC_API_KEY",
                "CLAUDE_CODE_OAUTH_TOKEN",
                "ANTHROPIC_AUTH_TOKEN",
            ] {
                assert!(launch.unset.iter().any(|v| v == var), "{var}");
            }
        });
    }

    #[test]
    fn a_removed_claude_account_still_fails_the_launch() {
        accounts::with_test_root(|_| {
            let err = account_launch("claude", Some("gone")).err().unwrap();
            assert!(err.to_string().contains("has been removed"), "{err}");
        });
    }

    #[test]
    fn the_default_account_launch_changes_nothing() {
        let launch = account_launch("claude", None).unwrap();
        assert!(launch.dir.is_none() && launch.unset.is_empty());
    }

    fn far_future_login() -> serde_json::Value {
        let enc = |v: serde_json::Value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string().as_bytes())
        };
        let exp = chrono::Utc::now().timestamp() + 9 * 24 * 3600;
        let access = format!(
            "{}.{}.sig",
            enc(serde_json::json!({"alg": "RS256"})),
            enc(serde_json::json!({"iat": exp - 10 * 24 * 3600, "exp": exp}))
        );
        serde_json::json!({
            "auth_mode": "chatgpt",
            "tokens": {"access_token": access, "id_token": "id", "refresh_token": "rt.host", "account_id": "a"},
            "last_refresh": "2026-10-01T00:00:00Z"
        })
    }

    #[test]
    fn a_codex_launch_runs_in_its_overlay_with_a_credential_and_no_refresh_token() {
        let td = tempfile::tempdir().unwrap();
        let (root, home, account) = (
            td.path().join("w"),
            td.path().join("home"),
            td.path().join("acct"),
        );
        for dir in [&root, &home, &account] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(account.join("auth.json"), far_future_login().to_string()).unwrap();

        let overlay = codex_home::overlay_in(&root);
        let codex = CodexHome::prepare_at(overlay.clone(), Some(&account), &home, None).unwrap();

        assert_eq!(
            codex.env(),
            (
                "CODEX_HOME".to_string(),
                overlay.to_string_lossy().into_owned()
            )
        );
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(overlay.join("auth.json")).unwrap()).unwrap();
        assert_eq!(written["tokens"]["refresh_token"], "");
        assert!(overlay.join("sessions").is_dir());
    }

    /// The default account's login is copied from a `CODEX_HOME` only the
    /// login shell exports, the home the user's own codex runs from.
    #[test]
    fn a_default_codex_launch_copies_the_login_from_the_login_shells_codex_home() {
        let td = tempfile::tempdir().unwrap();
        let (root, home, shell_home) = (
            td.path().join("w"),
            td.path().join("home"),
            td.path().join("shell-codex"),
        );
        for dir in [&root, &home.join(".codex"), &shell_home] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let mut shell_login = far_future_login();
        shell_login["tokens"]["account_id"] = "shell".into();
        std::fs::write(shell_home.join("auth.json"), shell_login.to_string()).unwrap();
        std::fs::write(
            home.join(".codex/auth.json"),
            far_future_login().to_string(),
        )
        .unwrap();

        let overlay = codex_home::overlay_in(&root);
        crate::bin_resolve::with_login_shell_env(
            &[("CODEX_HOME", shell_home.to_str().unwrap())],
            || CodexHome::prepare_at(overlay.clone(), None, &home, None),
        )
        .unwrap();

        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(overlay.join("auth.json")).unwrap()).unwrap();
        assert_eq!(written["tokens"]["account_id"], "shell");
        assert_eq!(written["tokens"]["refresh_token"], "");
    }

    #[test]
    fn only_codex_launches_get_an_overlay() {
        assert!(
            CodexHome::prepare("claude", "yosemite", None, Path::new("/"), None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_managed_codex_account_launch_names_its_dir_and_sets_no_env() {
        accounts::with_test_root(|root| {
            std::fs::create_dir_all(root.join("codex").join("work")).unwrap();
            let launch = account_launch("codex", Some("work")).unwrap();
            assert_eq!(launch.dir, Some(root.join("codex").join("work")));
            assert!(launch.unset.contains(&"OPENAI_API_KEY".to_string()));
        });
    }

    /// Live, end to end on this Mac: a real `codex exec` turn under the
    /// seatbelt profile, in an overlay assembled from the login in
    /// `$FLETCH_LIVE_CODEX_SOURCE` (a directory holding a *copy* of an
    /// `auth.json` with more than a day left on its access token, so nothing
    /// refreshes). Spends one tiny turn of that account's quota;
    /// `FLETCH_LIVE_CODEX_MODEL` overrides the model the shared config picks.
    /// Run with:
    ///   FLETCH_LIVE_CODEX_SOURCE=<dir> cargo test --lib live_codex_turn -- --ignored --nocapture
    #[test]
    #[ignore]
    #[cfg(target_os = "macos")]
    fn live_codex_turn_runs_from_its_overlay_under_seatbelt() {
        let Some(source) = std::env::var_os("FLETCH_LIVE_CODEX_SOURCE").map(PathBuf::from) else {
            eprintln!("FLETCH_LIVE_CODEX_SOURCE unset; skipping");
            return;
        };
        let home = dirs::home_dir().unwrap();
        let before = std::fs::read(source.join("auth.json")).unwrap();
        let td = tempfile::tempdir().unwrap();
        let (root, rpc) = (td.path().join("agent"), td.path().join("rpc"));
        let cwd = root.join("repo");
        for dir in [&cwd, &rpc] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let codex =
            CodexHome::prepare_at(codex_home::overlay_in(&root), Some(&source), &home, None)
                .unwrap();
        let ctx = AgentLaunchCtx {
            agent_id: "live",
            provider: "codex",
            writable_root: &root,
            source_repos: &[],
            rpc_dir: &rpc,
            cwd: &cwd,
            home: &home,
            interactive: false,
            blackboard: None,
            account_dir: Some(&source),
            oauth_token: None,
            codex_home: Some(&codex.overlay),
        };
        let bin = resolve_agent_bin("codex", "codex", "Codex", &home).unwrap();
        let plan = sandbox::engine_for(EngineKind::SandboxExec)
            .unwrap()
            .launch_agent(&ctx, &bin)
            .unwrap();
        println!(
            "CA source: plan={:?} app env={:?}",
            plan.env
                .iter()
                .find(|(k, _)| k == "CODEX_CA_CERTIFICATE")
                .map(|(_, v)| v),
            ["SSL_CERT_FILE", "CODEX_CA_CERTIFICATE"]
                .into_iter()
                .filter(|v| std::env::var_os(v).is_some())
                .collect::<Vec<_>>()
        );
        let mut cmd = std::process::Command::new(&plan.program);
        cmd.args(&plan.prefix_args)
            .args(["exec", "--json", "--skip-git-repo-check"])
            .args(
                std::env::var("FLETCH_LIVE_CODEX_MODEL")
                    .map(|m| vec!["-m".to_string(), m])
                    .unwrap_or_default(),
            )
            .arg("Reply with exactly the word: pong")
            .current_dir(&cwd)
            .stdin(std::process::Stdio::null());
        crate::bin_resolve::apply_login_shell_env(&mut cmd);
        cmd.envs(plan.env.iter().map(|(k, v)| (k, v)))
            .env(codex.env().0, codex.env().1)
            .env_remove("OPENAI_API_KEY");
        let out = cmd.output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        eprintln!("{stdout}");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(stdout.contains("\"text\":\"pong\""), "{stdout}");
        assert_eq!(std::fs::read(source.join("auth.json")).unwrap(), before);
        let mut diag = crate::agent::ReadDiagnostics::default();
        let thread = stdout
            .lines()
            .find_map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).ok()?;
                v.get("thread_id")?.as_str().map(str::to_string)
            })
            .unwrap();
        let rollouts = crate::transcripts::find_codex_rollouts_in(
            &codex.overlay.join("sessions"),
            &thread,
            &mut diag,
        );
        assert_eq!(rollouts.len(), 1);
        assert!(rollouts[0].starts_with(&codex.overlay), "{rollouts:?}");
    }
}
