//! Run-panel process management: setup/dev phase planning, spawning, and
//! phase-exit chaining.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::pty_session::PtyExit;
use crate::run_detect::{DetectedConfig, Tree};
use crate::run_session::{self, shell_args, user_shell, RunPhase, RunSession, RunStateSnapshot};
use crate::sandbox::{KillGuard, KillHandle};

use super::events::{emit_run_output, emit_run_port, emit_run_state};
use super::Supervisor;

/// How far past the configured port to scan for a free one before giving up.
const PORT_SCAN_CAP: u16 = 30;

impl Supervisor {
    /// Start the Run-panel process for an agent.
    ///
    /// If the agent has never completed setup before, runs the setup
    /// command first; on exit 0 marks setup complete and chains into
    /// the run command. On setup failure → does NOT proceed to run.
    /// If setup is already complete, starts the run command directly.
    ///
    /// No-op if a run is already in progress for this agent.
    pub fn run_start(self: Arc<Self>, ctx: Arc<EngineCtx>, agent_id: &str) -> Result<()> {
        // Held until its command is spawned: no move starts under the start,
        // and one under way refuses it (`sandbox_move`).
        let _held = self.hold_checkouts(agent_id)?;
        let record = self.workspace.agent(agent_id)?;
        if record.archive.is_some() {
            return Err(Error::Other("agent is archived".into()));
        }
        let primary = record
            .repos
            .first()
            .ok_or_else(|| Error::Other("agent has no repos".into()))?;
        let cwd = primary.checkout_path(agent_id)?;
        // Every engine Fletch ships runs the Run panel's process — on the host
        // in the checkout, or in the guest that holds it (`sandbox::guest_run`)
        // — and the cap is where one that cannot would say so
        // (docs/sandbox-foundation.md rule 7).
        if !crate::sandbox::stamped_engine(&record).caps().run_panel {
            return Err(Error::Other(
                "the Run panel isn't available under this workspace's sandbox".into(),
            ));
        }

        let session = self.run_session(agent_id);
        if session.is_active() {
            return Ok(()); // already running, idempotent
        }

        // The checkout, where it lives: a box keeps it in its guest, and a
        // stopped box boots here, under a note, since the boot is the longest
        // part of such a start and the panel has nothing else to show yet.
        if crate::sandbox::checkout_asleep(&cwd) {
            write_note(&ctx, agent_id, &session, "Starting the Linux VM\u{2026}");
        }
        let tree = match Tree::open(&cwd) {
            Ok(tree) => tree,
            Err(e) => {
                let msg = format!("Failed to start: could not read the checkout: {e}");
                session.mark_stopped(Some(msg.clone()));
                emit_run_state(ctx.sink.as_ref(), agent_id, RunPhase::Stopped, Some(msg));
                return Err(Error::Other(e.to_string()));
            }
        };
        let configs = crate::run_detect::detect_all_in(&tree);
        session.remember_detected(&configs);
        let (setup_cmd, run_cmd, intended_port) =
            self.read_run_config(agent_id, &record.project_id, &configs);
        let setup_done = self.workspace.is_setup_completed(agent_id)?;

        // Nothing to run (unrecognized ecosystem with no install/dev) —
        // leave the button Idle rather than spawning an empty command.
        let Some(plan) = plan_run_phases(setup_done, &setup_cmd, &run_cmd) else {
            return Ok(());
        };

        // Secure a free port for the dev phase *before* flipping the session
        // active, so a "no free port" failure surfaces cleanly without leaving
        // the button stuck. Setup runs unchanged; its chained run is prepared
        // later, in handle_run_phase_exit, closer to its own spawn — the
        // intended port rides along so that pass reuses this detection. A
        // command in a box's guest settles its port as its phase spawns
        // (`spawn_guest_run_phase`): the probe runs in the guest.
        let remote = crate::sandbox::checkout_is_remote(&cwd);
        let prepared = if plan.first_phase == RunPhase::Running && !remote {
            match prepare_run(&plan.first_cmd, intended_port) {
                Ok(p) => p,
                Err(e) => {
                    let msg = format!("Failed to start command: {e}");
                    emit_run_state(ctx.sink.as_ref(), agent_id, RunPhase::Stopped, Some(msg));
                    return Err(e);
                }
            }
        } else {
            PreparedRun::passthrough(plan.first_cmd.clone())
        };

        let gen = session.begin_phase(plan.first_phase);
        // Emit the resolved port *before* the state flips to running, so the
        // store already holds the correct port when the UI reacts to
        // `run:state` — otherwise the link/label briefly shows the old port.
        session.set_ports(prepared.port, prepared.port);
        if let Some(port) = prepared.port {
            emit_run_port(ctx.sink.as_ref(), agent_id, port, port);
        }
        emit_run_state(ctx.sink.as_ref(), agent_id, plan.first_phase, None);
        write_header(&ctx, agent_id, &session, &prepared.cmd);
        if let Some(note) = &prepared.note {
            write_note(&ctx, agent_id, &session, note);
        }

        // begin_phase already flipped the session active. If the spawn fails we
        // must reset to Stopped — otherwise is_active() stays true and every
        // later ▶ click no-ops on the idempotency guard. Mirrors the chained
        // run-phase failure path in handle_run_phase_exit.
        if let Err(e) = spawn_run_phase(
            self.clone(),
            ctx.clone(),
            agent_id.to_string(),
            session.clone(),
            gen,
            cwd,
            intended_port,
            plan.first_phase,
            prepared.cmd,
            prepared.extra_env,
            plan.chained_run_cmd,
        ) {
            let msg = format!("Failed to start command: {e}");
            session.mark_stopped(Some(msg.clone()));
            emit_run_state(ctx.sink.as_ref(), agent_id, RunPhase::Stopped, Some(msg));
            return Err(e);
        }
        Ok(())
    }

    /// Stop the Run-panel process for an agent. Idempotent.
    pub fn run_stop(&self, ctx: Arc<EngineCtx>, agent_id: &str) -> Result<()> {
        let session = {
            let runs = self.runs.lock();
            runs.get(agent_id).cloned()
        };
        let Some(session) = session else {
            return Ok(());
        };
        let prior = session.stop();
        if matches!(prior, RunPhase::Setup | RunPhase::Running) {
            emit_run_state(ctx.sink.as_ref(), agent_id, RunPhase::Stopped, None);
        }
        Ok(())
    }

    /// Snapshot of the current state and accumulated log for the
    /// panel to rehydrate on mount.
    pub fn run_state(&self, agent_id: &str) -> RunStateSnapshot {
        let session = {
            let runs = self.runs.lock();
            runs.get(agent_id).cloned()
        };
        match session {
            Some(s) => s.snapshot(),
            None => RunStateSnapshot {
                phase: RunPhase::Idle,
                last_error: None,
                port: None,
                host_port: None,
                log: Vec::new(),
                log_seq: 0,
            },
        }
    }

    /// The agent's run session, made on first use.
    fn run_session(&self, agent_id: &str) -> Arc<RunSession> {
        self.runs
            .lock()
            .entry(agent_id.to_string())
            .or_insert_with(|| Arc::new(RunSession::new()))
            .clone()
    }

    /// Read the setup + run commands and the intended dev-server port for an
    /// agent, from the checkout's detection `configs`. The detector provides
    /// the baseline (same values the panel shows); the project's `run.install`
    /// / `run.dev` / `run.port` settings layer on top, and the agent's
    /// `run.agent.<id>.*` overrides (edited in the Run panel sheet) take final
    /// precedence. One detector feeds both the panel and the runner, so there
    /// is no hardcoded default to keep in sync. The port is `None` when none
    /// can be inferred (e.g. a plain script) — port safety is then a no-op.
    fn read_run_config(
        &self,
        agent_id: &str,
        project_id: &str,
        configs: &[DetectedConfig],
    ) -> (String, String, Option<u16>) {
        let detected = |id: &str| -> Option<String> {
            configs
                .first()
                .and_then(|c| c.rows.iter().find(|r| r.id == id))
                .map(|r| r.value.clone())
        };
        let install_default = detected("install").unwrap_or_default();
        let dev_default = detected("dev").unwrap_or_default();
        let port_default = detected("port");

        let (install, dev, port_raw) = if project_id.is_empty() {
            (install_default, dev_default, port_default)
        } else {
            let setting = |key: &str| -> Option<String> {
                self.workspace
                    .project_setting(project_id, &format!("run.agent.{agent_id}.{key}"))
                    .or_else(|| {
                        self.workspace
                            .project_setting(project_id, &format!("run.{key}"))
                    })
            };
            (
                setting("install").unwrap_or(install_default),
                setting("dev").unwrap_or(dev_default),
                setting("port").or(port_default),
            )
        };
        (
            install,
            dev,
            port_raw.and_then(|s| s.trim().parse::<u16>().ok()),
        )
    }

    /// Detect the run configuration for an agent's primary repo,
    /// ranked by confidence. The panel renders the first (highest
    /// confidence) entry; the rest are returned for future
    /// multi-ecosystem selection.
    ///
    /// A look at the checkout, not an action in it: it never boots a stopped
    /// box (docs/sandbox-foundation.md rule 2). Such a checkout answers with
    /// its last detection — a start's, or an earlier look's while the box
    /// ran — and nothing before the first; the panel looks again as a start
    /// wakes the box.
    pub fn detect_run_config(&self, agent_id: &str) -> Result<Vec<DetectedConfig>> {
        let record = self.workspace.agent(agent_id)?;
        let primary = record
            .repos
            .first()
            .ok_or_else(|| Error::Other("agent has no repos".into()))?;
        let checkout = primary.checkout_path(agent_id)?;
        let session = self.run_session(agent_id);
        match crate::exec::passive_sync(|| Tree::open(&checkout)) {
            Ok(tree) => {
                let configs = crate::run_detect::detect_all_in(&tree);
                session.remember_detected(&configs);
                Ok(configs)
            }
            Err(e) => {
                tracing::debug!(agent_id, error = %e, "run config: checkout not readable now");
                Ok(session.last_detected().unwrap_or_default())
            }
        }
    }

    /// Detect the run configuration for a project, keyed by repo path
    /// rather than an agent. The Project Settings surface is reachable
    /// from the sidebar's repo groups — including pinned repos with no
    /// live agent — so it can't resolve a worktree the way the Run panel
    /// does. Detection runs against the repo checkout root, and the
    /// resolved `project_id` is bundled in so the frontend can read and
    /// write `project_settings` without a second round-trip.
    pub fn project_run_config(&self, repo_path: &str) -> Result<ProjectRunConfig> {
        let project_id = self.workspace.project_id_for_repo(repo_path)?;
        let configs = crate::run_detect::detect_all(Path::new(repo_path));
        Ok(ProjectRunConfig {
            project_id,
            configs,
        })
    }
}

/// The Run panel's process as the agent's `run_*` mailbox ops drive it
/// (`rpc::run`): the same start, stop and snapshot the ▶ and ■ buttons use,
/// bound to one supervisor and event sink so the dispatcher needs neither.
pub(super) struct RunControlHandle {
    sup: Arc<Supervisor>,
    ctx: Arc<EngineCtx>,
}

impl RunControlHandle {
    pub(super) fn new(sup: Arc<Supervisor>, ctx: Arc<EngineCtx>) -> Self {
        Self { sup, ctx }
    }
}

impl crate::rpc::run::RunControl for RunControlHandle {
    fn start(&self, agent_id: &str) -> Result<()> {
        Supervisor::run_start(self.sup.clone(), self.ctx.clone(), agent_id)
    }

    fn stop(&self, agent_id: &str) -> Result<()> {
        self.sup.run_stop(self.ctx.clone(), agent_id)
    }

    fn state(&self, agent_id: &str) -> RunStateSnapshot {
        self.sup.run_state(agent_id)
    }
}

/// Run configuration for a project resolved from a repo path: the detected
/// configs plus the `project_id` they belong to. See [`Supervisor::project_run_config`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectRunConfig {
    pub project_id: String,
    pub configs: Vec<crate::run_detect::DetectedConfig>,
}

/// Inject a "$ <cmd>" header line into the log so each phase has a
/// visible boundary, then emit it like any other PTY output.
fn write_header(ctx: &Arc<EngineCtx>, agent_id: &str, session: &Arc<RunSession>, cmd: &str) {
    // Dim ANSI for the prompt — the panel renders ANSI, so this shows as a
    // muted prompt line separating each phase.
    let line = format!("\x1b[2m$ {cmd}\x1b[0m\r\n");
    let bytes = line.into_bytes();
    let seq = session.append_log(&bytes);
    emit_run_output(ctx.sink.as_ref(), agent_id, bytes, seq);
}

/// Inject a port-safety note (e.g. "Port 3000 in use — using 3001") into the
/// log so the user understands why the dev server bound a different port.
fn write_note(ctx: &Arc<EngineCtx>, agent_id: &str, session: &Arc<RunSession>, note: &str) {
    let line = format!("\x1b[2m{note}\x1b[0m\r\n");
    let bytes = line.into_bytes();
    let seq = session.append_log(&bytes);
    emit_run_output(ctx.sink.as_ref(), agent_id, bytes, seq);
}

/// A dev command prepared for spawn: the (possibly port-rewritten) command,
/// extra env vars to inject (the pinned `PORT`), an optional user-facing note
/// when the port was bumped, and the actual port to advertise to the UI.
#[derive(Debug)]
struct PreparedRun {
    cmd: String,
    extra_env: Vec<(String, String)>,
    note: Option<String>,
    port: Option<u16>,
}

impl PreparedRun {
    /// A no-op preparation: run the command as-is, no port injection. Used for
    /// the setup phase and for commands with no inferable port.
    fn passthrough(cmd: String) -> Self {
        Self {
            cmd,
            extra_env: Vec::new(),
            note: None,
            port: None,
        }
    }
}

/// Prepare a dev command for spawn with port safety: given the already-resolved
/// `intended` port (see [`Supervisor::read_run_config`]), find a free port at or
/// after it (scanning up to [`PORT_SCAN_CAP`] past it) and force the dev server
/// onto it — via a `PORT` env var and, when the command carries an explicit port
/// token, by rewriting that token too. When the intended port is already free
/// the command is unchanged (only `PORT` is pinned). Returns a passthrough when
/// no port was inferred, and errors when every port in range is taken.
///
/// The free-port probe runs here, right before spawn, rather than at click time:
/// setup/install can take minutes, so a port checked earlier may be gone by the
/// time the dev server actually binds. This is the host's probe, for a command
/// that runs on the host; [`prepare_run_with`] takes the probe of wherever the
/// command runs.
fn prepare_run(run_cmd: &str, intended: Option<u16>) -> Result<PreparedRun> {
    prepare_run_with(run_cmd, intended, |start, cap| {
        Ok(crate::run_detect::port::find_free_port(start, cap))
    })
}

/// [`prepare_run`] with `free_port` as the probe: the first free port at or
/// after `start`, scanning up to `cap` past it, where the command will run —
/// `None` when every port in range is taken.
fn prepare_run_with(
    run_cmd: &str,
    intended: Option<u16>,
    free_port: impl FnOnce(u16, u16) -> Result<Option<u16>>,
) -> Result<PreparedRun> {
    let Some(intended) = intended else {
        return Ok(PreparedRun::passthrough(run_cmd.to_string()));
    };
    let chosen = free_port(intended, PORT_SCAN_CAP)?.ok_or_else(|| {
        Error::Other(format!(
            "No free port available in {}\u{2013}{}",
            intended,
            intended.saturating_add(PORT_SCAN_CAP)
        ))
    })?;

    let mut cmd = run_cmd.to_string();
    let mut note = None;
    if chosen != intended {
        if let Some(rewritten) = crate::run_detect::port::rewrite_explicit_port(run_cmd, chosen) {
            cmd = rewritten;
        }
        note = Some(format!("Port {intended} in use — using {chosen}"));
    }
    Ok(PreparedRun {
        cmd,
        extra_env: vec![("PORT".to_string(), chosen.to_string())],
        note,
        port: Some(chosen),
    })
}

/// The phases to spawn for a single `run_start`, derived from the
/// resolved commands and whether setup has already completed.
#[derive(Debug)]
struct RunPlan {
    first_phase: RunPhase,
    first_cmd: String,
    /// Run command to chain after a successful setup phase. `None` when
    /// the first phase is already the run, or when there is no run
    /// command to chain (so we never spawn an empty command).
    chained_run_cmd: Option<String>,
}

/// Decide what to spawn. Returns `None` when there is nothing to run —
/// neither a setup nor a run command — so the caller can leave the
/// button Idle instead of spawning an empty command that would exit 0
/// and flash the panel to Stopped with no explanation.
fn plan_run_phases(setup_done: bool, setup_cmd: &str, run_cmd: &str) -> Option<RunPlan> {
    let needs_setup = !setup_done && !setup_cmd.trim().is_empty();
    let has_run_cmd = !run_cmd.trim().is_empty();
    if needs_setup {
        Some(RunPlan {
            first_phase: RunPhase::Setup,
            first_cmd: setup_cmd.to_string(),
            chained_run_cmd: has_run_cmd.then(|| run_cmd.to_string()),
        })
    } else if has_run_cmd {
        Some(RunPlan {
            first_phase: RunPhase::Running,
            first_cmd: run_cmd.to_string(),
            chained_run_cmd: None,
        })
    } else {
        None
    }
}

/// Spawn one phase's PTY (setup or run). Wires up output streaming
/// and the exit handler that chains setup→run or transitions to
/// Stopped on natural exit. Out-of-band stops are handled via the
/// generation check. `intended_port` is the configured dev-server port,
/// for the run phase a setup phase chains into, and for a run phase in a
/// box's guest, whose port is settled here.
#[allow(clippy::too_many_arguments)]
fn spawn_run_phase(
    sup: Arc<Supervisor>,
    ctx: Arc<EngineCtx>,
    agent_id: String,
    session: Arc<RunSession>,
    gen: u64,
    cwd: std::path::PathBuf,
    intended_port: Option<u16>,
    phase: RunPhase,
    cmd: String,
    extra_env: Vec<(String, String)>,
    chain_run_cmd: Option<String>,
) -> Result<()> {
    // A checkout the engine keeps off the host — a box's, in its guest — has
    // its command run there, by the engine. The Run profile below is the
    // host's own.
    if crate::sandbox::checkout_is_remote(&cwd) {
        return spawn_guest_run_phase(
            sup,
            ctx,
            agent_id,
            session,
            gen,
            cwd,
            intended_port,
            phase,
            cmd,
            chain_run_cmd,
        );
    }
    // Confine the run command to the checkout + toolchain caches. The command
    // string is repo-derived (package.json scripts, postinstall, dev-server
    // config), so a malicious agent could otherwise plant a script that runs
    // unsandboxed with full user privilege the moment the user clicks ▶. Reads
    // and network stay open (dev servers need them); only writes are fenced.
    // The agent's project keys the toolchain cache the command may write, so
    // without the record there is no cache to grant and the run fails.
    let record = sup.workspace.agent(&agent_id)?;
    let home =
        dirs::home_dir().ok_or_else(|| Error::Other("HOME directory not available".into()))?;
    let SandboxedRunCommand {
        program,
        args,
        mut env,
        toolchain_cache,
    } = sandboxed_run_command(&cwd, &cmd, &home, &record.project_id)?;
    // A nested Fletch (dogfooding) can't reach the host's `~/.fletch` under
    // this profile; steer its mailboxes, checkouts and toolchain caches to roots
    // it may write, the caches inside this project's root (see
    // `sandbox::toolchain_cache`). Harmless for any other Run target: nothing
    // but Fletch reads these.
    env.extend([
        (
            crate::rpc::RPC_ROOT_ENV.to_string(),
            crate::sandbox::nested_rpc_root(&cwd)
                .to_string_lossy()
                .into_owned(),
        ),
        (
            crate::workspace::WORKSPACES_ROOT_ENV.to_string(),
            crate::sandbox::nested_checkouts_root(&cwd)
                .to_string_lossy()
                .into_owned(),
        ),
        (
            crate::sandbox::TOOLCHAIN_CACHE_ROOT_ENV.to_string(),
            toolchain_cache
                .join("nested")
                .to_string_lossy()
                .into_owned(),
        ),
    ]);
    // Layer the project's opt-in Run environment: the `.env` variables the user
    // chose to share into the sandbox (mirrored live from the source repo's
    // `.env` or overridden per project, with `{{agent_id}}`/`{{worktree}}`
    // interpolated). Nothing is shared unless explicitly enabled in Project
    // Settings. Best-effort — a record with no repo just injects nothing.
    if let Some(primary) = record.repos.first() {
        let project_env =
            sup.workspace
                .run_env(&record.project_id, &primary.repo_path, &agent_id, &cwd);
        env.extend(project_env);
    }
    // Pin the resolved (port-safe) PORT last so it wins over anything the
    // sandbox layer — or the project env above — set.
    env.extend(extra_env);

    let (on_output, on_exit) = phase_hooks(
        sup,
        ctx,
        agent_id,
        session.clone(),
        gen,
        cwd.clone(),
        intended_port,
        phase,
        chain_run_cmd,
    );
    let pty = run_session::spawn_command(
        &program,
        &args,
        &cwd,
        &env,
        KillHandle::ProcessGroup,
        on_output,
        on_exit,
    )?;

    session.attach_pty(pty);
    Ok(())
}

/// [`spawn_run_phase`] for a checkout the engine keeps off the host: the
/// command runs in the guest that holds it (`sandbox::guest_run`), held
/// against the box's idle stop for as long as it runs, and a run phase has
/// its dev server's port forwarded to a free port on this host, which the
/// panel's link opens. The port is settled here rather than before the phase
/// began, as the host's is: finding a free port in the guest, and forwarding
/// it, both need the box, which a stopped one must first boot — under a
/// phase the panel already shows, so the wait is visible. `run:port` thus
/// follows `run:state`, carrying the host port once the forward is up.
///
/// The env the command gets is the guest's own (`guest_env`, the project's
/// toolchain cache in the guest) plus the project's shared Run env and the
/// pinned `PORT`; the host's nested-Fletch roots mean nothing there.
#[allow(clippy::too_many_arguments)]
fn spawn_guest_run_phase(
    sup: Arc<Supervisor>,
    ctx: Arc<EngineCtx>,
    agent_id: String,
    session: Arc<RunSession>,
    gen: u64,
    cwd: PathBuf,
    intended_port: Option<u16>,
    phase: RunPhase,
    cmd: String,
    chain_run_cmd: Option<String>,
) -> Result<()> {
    let record = sup.workspace.agent(&agent_id)?;
    let Some(mut guest) = crate::sandbox::guest_run(&cwd)? else {
        return Err(Error::Other(format!(
            "{} is not in a sandbox this host runs commands in",
            cwd.display()
        )));
    };
    let prepared = if phase == RunPhase::Running {
        prepare_run_with(&cmd, intended_port, |start, cap| guest.free_port(start, cap))?
    } else {
        PreparedRun::passthrough(cmd.clone())
    };
    let mut env: Vec<(String, String)> = Vec::new();
    if let Some(primary) = record.repos.first() {
        env.extend(
            sup.workspace
                .run_env(&record.project_id, &primary.repo_path, &agent_id, &cwd),
        );
    }
    env.extend(prepared.extra_env);

    let host_port = match prepared.port {
        Some(port) => Some(guest.forward(port, PORT_SCAN_CAP)?),
        None => None,
    };
    session.set_ports(prepared.port, host_port);
    if let (Some(port), Some(host_port)) = (prepared.port, host_port) {
        emit_run_port(ctx.sink.as_ref(), &agent_id, port, host_port);
    }
    if let Some(note) = &prepared.note {
        write_note(&ctx, &agent_id, &session, note);
    }
    if let (Some(port), Some(host_port)) = (prepared.port, host_port) {
        if host_port != port {
            write_note(
                &ctx,
                &agent_id,
                &session,
                &format!("Port {port} in use on this Mac — reachable at localhost:{host_port}"),
            );
        }
    }

    let (program, args, kill) = guest.launch(&cwd, &prepared.cmd, &env)?;
    let (on_output, on_exit) = phase_hooks(
        sup,
        ctx,
        agent_id,
        session.clone(),
        gen,
        cwd.clone(),
        intended_port,
        phase,
        chain_run_cmd,
    );
    // The guard runs the kill — the box released, the forward dropped — if
    // the spawn fails; a spawned session owns it from then on.
    let pty = KillGuard::new(kill).start(|kill| {
        run_session::spawn_command(&program, &args, &cwd, &[], kill, on_output, on_exit)
    })?;
    session.attach_pty(pty);
    Ok(())
}

/// The output and exit hooks of one phase's PTY, whichever engine runs it:
/// output into the session's log and out to the panel; the exit into
/// [`handle_run_phase_exit`] under the phase's generation.
#[allow(clippy::too_many_arguments)]
fn phase_hooks(
    sup: Arc<Supervisor>,
    ctx: Arc<EngineCtx>,
    agent_id: String,
    session: Arc<RunSession>,
    gen: u64,
    cwd: PathBuf,
    intended_port: Option<u16>,
    phase: RunPhase,
    chain_run_cmd: Option<String>,
) -> (
    impl Fn(Vec<u8>) + Send + 'static,
    impl Fn(PtyExit) + Send + 'static,
) {
    let session_out = session.clone();
    let ctx_out = ctx.clone();
    let id_out = agent_id.clone();
    let on_output = move |bytes: Vec<u8>| {
        let seq = session_out.append_log(&bytes);
        emit_run_output(ctx_out.sink.as_ref(), &id_out, bytes, seq);
    };
    let on_exit = move |exit: PtyExit| {
        handle_run_phase_exit(
            sup.clone(),
            ctx.clone(),
            agent_id.clone(),
            session.clone(),
            gen,
            cwd.clone(),
            intended_port,
            phase,
            exit,
            chain_run_cmd.clone(),
        );
    };
    (on_output, on_exit)
}

/// A command wrapped in the Run profile, as [`sandboxed_run_command`] builds
/// it.
pub(crate) struct SandboxedRunCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// The toolchain cache redirect; the caller layers its own env on top.
    pub env: Vec<(String, String)>,
    /// The project's toolchain cache root the profile grants.
    pub toolchain_cache: PathBuf,
}

/// Build the `sandbox-exec`-wrapped invocation for a project command run
/// under the Run profile: `sandbox-exec -p <profile> <shell> -lic <cmd>`. The
/// Run panel and the verifier both run through here, so a project's checks
/// get exactly the profile and the toolchain cache its Run commands do, shared
/// with that project's agents. The profile travels in argv rather than a
/// tempfile — see [`crate::sandbox::profile_args`] for why a file would be
/// racy — so there is nothing for the caller to keep alive.
pub(crate) fn sandboxed_run_command(
    cwd: &Path,
    cmd: &str,
    home: &Path,
    project_id: &str,
) -> Result<SandboxedRunCommand> {
    let cache = crate::sandbox::prepare_toolchain_cache(
        home,
        project_id,
        cwd,
        crate::sandbox::BoxPlatform::host(),
    )?;
    // Grant the target's git *common dir* so `git worktree add` (and later
    // commits) can write worktree admin data / objects / refs. For a normal
    // repo this is `<cwd>/.git`, already inside `writable_root`; for a linked
    // worktree (the dogfooding case) it's the source repo's real `.git`,
    // outside `writable_root`, which would otherwise fail closed.
    let extra_writable: Vec<PathBuf> = run_target_git_common_dir(cwd).into_iter().collect();
    let profile_text = crate::sandbox::build_run_profile(cwd, home, &cache.root, &extra_writable)?;
    let shell = user_shell();
    let shell_str = shell
        .to_str()
        .ok_or_else(|| Error::Other("shell path not utf-8".into()))?
        .to_string();
    let mut args = crate::sandbox::profile_args(&profile_text).to_vec();
    args.push(shell_str);
    args.extend(shell_args(cmd));
    Ok(SandboxedRunCommand {
        program: PathBuf::from(crate::sandbox::SANDBOX_EXEC),
        args,
        env: cache.env,
        toolchain_cache: cache.root,
    })
}

/// Resolve the git *common dir* of the Run target `cwd`, canonicalized, so the
/// Run sandbox can grant writes to it. Returns `None` when `cwd` isn't a git
/// repo (nothing to grant), git can't be resolved, or the dir can't be
/// canonicalized (a non-real path wouldn't match the kernel's symlink-resolved
/// check anyway). Runs synchronously — the profile is assembled off the async
/// runtime — via the app's resolved git binary (portable-git fallback),
/// matching every other git call.
///
fn run_target_git_common_dir(cwd: &Path) -> Option<PathBuf> {
    let out = crate::git_dist::std_command(cwd)
        // Resolve strictly from `cwd`. `std_command` inherits the outer app's
        // environment, and an ambient GIT_DIR / GIT_WORK_TREE / GIT_COMMON_DIR
        // (set by whatever launched Fletch) would override cwd-based discovery —
        // pointing the probe at an unrelated repo and making us grant the wrong
        // common dir while the real one stays denied. Clear them.
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .args(["rev-parse", "--git-common-dir"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }
    // `--git-common-dir` is relative to `cwd` for a normal repo (`.git`),
    // absolute for a linked worktree — make it absolute first.
    let path = Path::new(&raw);
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    // The sbpl subpath must be the *real* path: the sandbox kernel resolves
    // symlinks before checking, and macOS $TMPDIR / checkout roots are commonly
    // symlinked (`/var` -> `/private/var`). If canonicalization fails, a raw
    // entry likely wouldn't match what the kernel checks, so grant nothing
    // rather than a bogus subpath that gives false confidence.
    std::fs::canonicalize(&abs).ok()
}

#[allow(clippy::too_many_arguments)]
fn handle_run_phase_exit(
    sup: Arc<Supervisor>,
    ctx: Arc<EngineCtx>,
    agent_id: String,
    session: Arc<RunSession>,
    gen: u64,
    cwd: std::path::PathBuf,
    intended_port: Option<u16>,
    phase: RunPhase,
    exit: PtyExit,
    chain_run_cmd: Option<String>,
) {
    // If the user clicked Stop (or started a fresh run), our
    // generation is stale — just drop this event.
    if !session.is_current_generation(gen) {
        tracing::debug!(
            agent_id = %agent_id,
            phase = ?phase,
            "ignoring stale run-phase exit"
        );
        return;
    }

    if matches!(phase, RunPhase::Setup) && exit.success {
        // Setup finished cleanly — persist the flag and chain into
        // the run command (if we have one).
        if let Err(e) = sup.workspace.mark_setup_completed(&agent_id) {
            tracing::warn!(error = %e, agent_id = %agent_id, "mark_setup_completed failed");
        }
        if let Some(run_cmd) = chain_run_cmd {
            // Secure a free port now — install may have run for minutes, so the
            // port could have been taken since the click. The intended port was
            // resolved at run_start and threaded here, so no re-detection. A
            // command in a box's guest settles its port as it spawns, as at
            // run_start.
            let prepared = if crate::sandbox::checkout_is_remote(&cwd) {
                PreparedRun::passthrough(run_cmd)
            } else {
                match prepare_run(&run_cmd, intended_port) {
                    Ok(p) => p,
                    Err(e) => {
                        let msg = format!("Failed to start run command: {e}");
                        session.mark_stopped(Some(msg.clone()));
                        emit_run_state(ctx.sink.as_ref(), &agent_id, RunPhase::Stopped, Some(msg));
                        return;
                    }
                }
            };
            session.transition_phase(RunPhase::Running);
            // Emit the resolved port before the running state so the store holds
            // it before the UI reacts (see run_start for the same ordering).
            session.set_ports(prepared.port, prepared.port);
            if let Some(port) = prepared.port {
                emit_run_port(ctx.sink.as_ref(), &agent_id, port, port);
            }
            emit_run_state(ctx.sink.as_ref(), &agent_id, RunPhase::Running, None);
            write_header(&ctx, &agent_id, &session, &prepared.cmd);
            if let Some(note) = &prepared.note {
                write_note(&ctx, &agent_id, &session, note);
            }
            if let Err(e) = spawn_run_phase(
                sup,
                ctx.clone(),
                agent_id.clone(),
                session.clone(),
                gen,
                cwd,
                intended_port,
                RunPhase::Running,
                prepared.cmd,
                prepared.extra_env,
                None,
            ) {
                let msg = format!("Failed to start run command: {e}");
                session.mark_stopped(Some(msg.clone()));
                emit_run_state(ctx.sink.as_ref(), &agent_id, RunPhase::Stopped, Some(msg));
            }
            return;
        }
        // No run command to chain into — treat as clean stop.
        session.mark_stopped(None);
        emit_run_state(ctx.sink.as_ref(), &agent_id, RunPhase::Stopped, None);
        return;
    }

    // Setup failed → do NOT proceed to run. Surface the error.
    if matches!(phase, RunPhase::Setup) && !exit.success {
        let msg = format!("Setup failed: {}", exit.message);
        session.mark_stopped(Some(msg.clone()));
        emit_run_state(ctx.sink.as_ref(), &agent_id, RunPhase::Stopped, Some(msg));
        return;
    }

    // Run-phase exit — natural end or crash. Either way → Stopped.
    let err = if exit.success {
        None
    } else {
        Some(format!("Run exited: {}", exit.message))
    };
    session.mark_stopped(err.clone());
    emit_run_state(ctx.sink.as_ref(), &agent_id, RunPhase::Stopped, err);
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── plan_run_phases ───────────────────────────────────────────────────

    #[test]
    fn plan_runs_dev_directly_when_setup_done() {
        let plan = plan_run_phases(true, "pnpm install", "pnpm dev").unwrap();
        assert_eq!(plan.first_phase, RunPhase::Running);
        assert_eq!(plan.first_cmd, "pnpm dev");
        assert_eq!(plan.chained_run_cmd, None);
    }

    #[test]
    fn plan_runs_setup_then_chains_dev() {
        let plan = plan_run_phases(false, "pnpm install", "pnpm dev").unwrap();
        assert_eq!(plan.first_phase, RunPhase::Setup);
        assert_eq!(plan.first_cmd, "pnpm install");
        assert_eq!(plan.chained_run_cmd.as_deref(), Some("pnpm dev"));
    }

    #[test]
    fn plan_does_not_chain_into_empty_run_cmd() {
        // Setup needed but no dev command (e.g. a plain Python project with
        // an install but no recognized run). Setup runs alone — no empty
        // command chained after it.
        let plan = plan_run_phases(false, "pip install -r requirements.txt", "").unwrap();
        assert_eq!(plan.first_phase, RunPhase::Setup);
        assert_eq!(plan.chained_run_cmd, None);
    }

    #[test]
    fn plan_is_none_when_nothing_to_run() {
        // Wholly unrecognized ecosystem: no setup, no run. Nothing should
        // be spawned — the button stays Idle instead of flashing Stopped.
        assert!(plan_run_phases(true, "", "").is_none());
        assert!(plan_run_phases(false, "", "").is_none());
        assert!(plan_run_phases(false, "   ", "  ").is_none());
    }

    #[test]
    fn plan_skips_completed_setup_even_if_run_empty() {
        // Setup already done and no run command → nothing to do.
        assert!(plan_run_phases(true, "pnpm install", "").is_none());
    }

    #[test]
    fn plan_runs_only_run_cmd_when_no_setup_needed() {
        let plan = plan_run_phases(true, "", "cargo run").unwrap();
        assert_eq!(plan.first_phase, RunPhase::Running);
        assert_eq!(plan.first_cmd, "cargo run");
        assert_eq!(plan.chained_run_cmd, None);
    }

    // ── prepare_run_with ──────────────────────────────────────────────────

    /// The probe decides the port wherever the command runs: the intended
    /// port free leaves the command alone and pins `PORT`; a bump rewrites
    /// an explicit token and says so; no port inferred probes nothing; and
    /// a range with nothing free is the error.
    #[test]
    fn prepare_run_takes_the_probes_port() {
        let free = prepare_run_with("vite --port 3000", Some(3000), |start, cap| {
            assert_eq!((start, cap), (3000, PORT_SCAN_CAP));
            Ok(Some(3000))
        })
        .unwrap();
        assert_eq!(free.cmd, "vite --port 3000");
        assert_eq!(free.port, Some(3000));
        assert_eq!(free.extra_env, vec![("PORT".to_string(), "3000".to_string())]);
        assert_eq!(free.note, None);

        let bumped = prepare_run_with("vite --port 3000", Some(3000), |_, _| Ok(Some(3002))).unwrap();
        assert_eq!(bumped.cmd, "vite --port 3002");
        assert_eq!(bumped.port, Some(3002));
        assert_eq!(bumped.note.as_deref(), Some("Port 3000 in use — using 3002"));

        let none = prepare_run_with("cargo run", None, |_, _| panic!("no port to probe")).unwrap();
        assert_eq!(none.port, None);
        assert!(none.extra_env.is_empty());

        let taken = prepare_run_with("next dev", Some(3000), |_, _| Ok(None)).unwrap_err();
        assert!(taken.to_string().contains("No free port"), "{taken}");
        let failed = prepare_run_with("next dev", Some(3000), |_, _| {
            Err(Error::Other("box gone".into()))
        })
        .unwrap_err();
        assert_eq!(failed.to_string(), "box gone");
    }
}
