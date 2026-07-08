//! Run-panel process management: setup/dev phase planning, spawning, and
//! phase-exit chaining.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::AppHandle;

use crate::error::{Error, Result};
use crate::run_session::{self, shell_args, user_shell, RunPhase, RunSession, RunStateSnapshot};
use crate::workspace::repo_checkout_path;

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
    pub fn run_start(self: Arc<Self>, app: AppHandle, agent_id: &str) -> Result<()> {
        let record = self.workspace.agent(agent_id)?;
        if record.archive.is_some() {
            return Err(Error::Other("agent is archived".into()));
        }
        let primary = record
            .repos
            .first()
            .ok_or_else(|| Error::Other("agent has no repos".into()))?;
        let cwd = repo_checkout_path(agent_id, &primary.subdir)?;

        let (setup_cmd, run_cmd) = self.read_run_commands(&record.project_id, &cwd);
        let setup_done = self.workspace.is_setup_completed(agent_id)?;

        let session = {
            let mut runs = self.runs.lock();
            runs.entry(agent_id.to_string())
                .or_insert_with(|| Arc::new(RunSession::new()))
                .clone()
        };

        if session.is_active() {
            return Ok(()); // already running, idempotent
        }

        // Nothing to run (unrecognized ecosystem with no install/dev) —
        // leave the button Idle rather than spawning an empty command.
        let Some(plan) = plan_run_phases(setup_done, &setup_cmd, &run_cmd) else {
            return Ok(());
        };

        // Secure a free port for the dev phase *before* flipping the session
        // active, so a "no free port" failure surfaces cleanly without leaving
        // the button stuck. Setup runs unchanged; its chained run is prepared
        // later, in handle_run_phase_exit, closer to its own spawn.
        let prepared = if plan.first_phase == RunPhase::Running {
            match self.prepare_run(&record.project_id, &cwd, &plan.first_cmd) {
                Ok(p) => p,
                Err(e) => {
                    let msg = format!("Failed to start command: {e}");
                    emit_run_state(&app, agent_id, RunPhase::Stopped, Some(msg));
                    return Err(e);
                }
            }
        } else {
            PreparedRun::passthrough(plan.first_cmd.clone())
        };

        let gen = session.begin_phase(plan.first_phase);
        emit_run_state(&app, agent_id, plan.first_phase, None);
        if let Some(port) = prepared.port {
            emit_run_port(&app, agent_id, port);
        }
        write_header(&app, agent_id, &session, &prepared.cmd);
        if let Some(note) = &prepared.note {
            write_note(&app, agent_id, &session, note);
        }

        // begin_phase already flipped the session active. If the spawn fails we
        // must reset to Stopped — otherwise is_active() stays true and every
        // later ▶ click no-ops on the idempotency guard. Mirrors the chained
        // run-phase failure path in handle_run_phase_exit.
        if let Err(e) = spawn_run_phase(
            self.clone(),
            app.clone(),
            agent_id.to_string(),
            session.clone(),
            gen,
            cwd,
            record.project_id.clone(),
            plan.first_phase,
            prepared.cmd,
            prepared.extra_env,
            plan.chained_run_cmd,
        ) {
            let msg = format!("Failed to start command: {e}");
            session.mark_stopped(Some(msg.clone()));
            emit_run_state(&app, agent_id, RunPhase::Stopped, Some(msg));
            return Err(e);
        }
        Ok(())
    }

    /// Stop the Run-panel process for an agent. Idempotent.
    pub fn run_stop(&self, app: AppHandle, agent_id: &str) -> Result<()> {
        let session = {
            let runs = self.runs.lock();
            runs.get(agent_id).cloned()
        };
        let Some(session) = session else {
            return Ok(());
        };
        let prior = session.stop();
        if matches!(prior, RunPhase::Setup | RunPhase::Running) {
            emit_run_state(&app, agent_id, RunPhase::Stopped, None);
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
                log: Vec::new(),
            },
        }
    }

    /// Read the setup + run commands for an agent. The detector provides
    /// the baseline (same values the panel shows), and any persisted
    /// `run.install` / `run.dev` overrides in project_settings take
    /// precedence. One detector feeds both the panel and the runner, so
    /// there is no hardcoded default to keep in sync.
    fn read_run_commands(&self, project_id: &str, checkout: &Path) -> (String, String) {
        let configs = crate::run_detect::detect_all(checkout);
        let detected = |id: &str| -> String {
            configs
                .first()
                .and_then(|c| c.rows.iter().find(|r| r.id == id))
                .map(|r| r.value.clone())
                .unwrap_or_default()
        };
        let install_default = detected("install");
        let dev_default = detected("dev");
        if project_id.is_empty() {
            return (install_default, dev_default);
        }
        (
            self.workspace
                .project_setting(project_id, "run.install")
                .unwrap_or(install_default),
            self.workspace
                .project_setting(project_id, "run.dev")
                .unwrap_or(dev_default),
        )
    }

    /// Resolve the intended dev-server port the same way the panel does:
    /// the detected `port` row, overridden by the `run.port` project setting
    /// when present. `None` when no port can be inferred (e.g. a plain script
    /// or an ecosystem with no port concept) — port safety is then a no-op.
    fn read_run_port(&self, project_id: &str, checkout: &Path) -> Option<u16> {
        let configs = crate::run_detect::detect_all(checkout);
        let detected = configs
            .first()
            .and_then(|c| c.rows.iter().find(|r| r.id == "port"))
            .map(|r| r.value.clone());
        let raw = if project_id.is_empty() {
            detected
        } else {
            self.workspace
                .project_setting(project_id, "run.port")
                .or(detected)
        };
        raw.and_then(|s| s.trim().parse::<u16>().ok())
    }

    /// Prepare a dev command for spawn with port safety: find a free port at or
    /// after the configured one (scanning up to [`PORT_SCAN_CAP`] past it) and
    /// force the dev server onto it — via a `PORT` env var and, when the command
    /// carries an explicit port token, by rewriting that token too. When the
    /// configured port is already free the command is unchanged (only `PORT` is
    /// pinned). Returns a passthrough when no port is inferred, and errors when
    /// every port in range is taken.
    fn prepare_run(&self, project_id: &str, cwd: &Path, run_cmd: &str) -> Result<PreparedRun> {
        let Some(intended) = self.read_run_port(project_id, cwd) else {
            return Ok(PreparedRun::passthrough(run_cmd.to_string()));
        };
        let chosen = crate::run_detect::port::find_free_port(intended, PORT_SCAN_CAP)
            .ok_or_else(|| {
                Error::Other(format!(
                    "No free port available in {}\u{2013}{}",
                    intended,
                    intended.saturating_add(PORT_SCAN_CAP)
                ))
            })?;

        let mut cmd = run_cmd.to_string();
        let mut note = None;
        if chosen != intended {
            if let Some(rewritten) = crate::run_detect::port::rewrite_explicit_port(run_cmd, chosen)
            {
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

    /// Detect the run configuration for an agent's primary repo,
    /// ranked by confidence. The panel renders the first (highest
    /// confidence) entry; the rest are returned for future
    /// multi-ecosystem selection.
    pub fn detect_run_config(
        &self,
        agent_id: &str,
    ) -> Result<Vec<crate::run_detect::DetectedConfig>> {
        let record = self.workspace.agent(agent_id)?;
        let primary = record
            .repos
            .first()
            .ok_or_else(|| Error::Other("agent has no repos".into()))?;
        let checkout = repo_checkout_path(agent_id, &primary.subdir)?;
        Ok(crate::run_detect::detect_all(&checkout))
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

/// Run configuration for a project resolved from a repo path: the detected
/// configs plus the `project_id` they belong to. See [`Supervisor::project_run_config`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectRunConfig {
    pub project_id: String,
    pub configs: Vec<crate::run_detect::DetectedConfig>,
}

/// Inject a "$ <cmd>" header line into the log so each phase has a
/// visible boundary, then emit it like any other PTY output.
fn write_header(app: &AppHandle, agent_id: &str, session: &Arc<RunSession>, cmd: &str) {
    // Dim ANSI for the prompt — the frontend strips ANSI for v1,
    // so the line still reads fine without color support.
    let line = format!("\x1b[2m$ {cmd}\x1b[0m\r\n");
    let bytes = line.into_bytes();
    session.append_log(&bytes);
    emit_run_output(app, agent_id, bytes);
}

/// Inject a port-safety note (e.g. "Port 3000 in use — using 3001") into the
/// log so the user understands why the dev server bound a different port.
fn write_note(app: &AppHandle, agent_id: &str, session: &Arc<RunSession>, note: &str) {
    let line = format!("\x1b[2m{note}\x1b[0m\r\n");
    let bytes = line.into_bytes();
    session.append_log(&bytes);
    emit_run_output(app, agent_id, bytes);
}

/// A dev command prepared for spawn: the (possibly port-rewritten) command,
/// extra env vars to inject (the pinned `PORT`), an optional user-facing note
/// when the port was bumped, and the actual port to advertise to the UI.
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
/// generation check.
#[allow(clippy::too_many_arguments)]
fn spawn_run_phase(
    sup: Arc<Supervisor>,
    app: AppHandle,
    agent_id: String,
    session: Arc<RunSession>,
    gen: u64,
    cwd: std::path::PathBuf,
    project_id: String,
    phase: RunPhase,
    cmd: String,
    extra_env: Vec<(String, String)>,
    chain_run_cmd: Option<String>,
) -> Result<()> {
    // Confine the run command to the checkout + toolchain caches. The command
    // string is repo-derived (package.json scripts, postinstall, dev-server
    // config), so a malicious agent could otherwise plant a script that runs
    // unsandboxed with full user privilege the moment the user clicks ▶. Reads
    // and network stay open (dev servers need them); only writes are fenced.
    let (program, args, mut env, profile_file) = sandboxed_run_command(&cwd, &cmd)?;
    // Pin the resolved (port-safe) PORT last so it wins over anything the
    // sandbox layer set.
    env.extend(extra_env);

    let session_out = session.clone();
    let app_out = app.clone();
    let id_out = agent_id.clone();

    let sup_exit = sup.clone();
    let app_exit = app.clone();
    let id_exit = agent_id.clone();
    let session_exit = session.clone();
    let cwd_exit = cwd.clone();

    let pty = run_session::spawn_command(
        &program,
        &args,
        &cwd,
        &env,
        move |bytes| {
            session_out.append_log(&bytes);
            emit_run_output(&app_out, &id_out, bytes);
        },
        move |exit| {
            handle_run_phase_exit(
                sup_exit.clone(),
                app_exit.clone(),
                id_exit.clone(),
                session_exit.clone(),
                gen,
                cwd_exit.clone(),
                project_id.clone(),
                phase,
                exit,
                chain_run_cmd.clone(),
            );
        },
    )?;

    session.attach_pty(pty, profile_file);
    Ok(())
}

/// Build the `sandbox-exec`-wrapped invocation for a Run-panel command:
/// `sandbox-exec -f <profile> <shell> -lic <cmd>`. Returns the program, argv,
/// and the profile tempfile. `sandbox-exec` reads the profile once, at the
/// child's `exec`, so the tempfile must survive until then; the caller parks it
/// on the `RunSession` (via `attach_pty`), which conservatively keeps it for the
/// process's lifetime.
fn sandboxed_run_command(
    cwd: &Path,
    cmd: &str,
) -> Result<(PathBuf, Vec<String>, Vec<(String, String)>, tempfile::NamedTempFile)> {
    let home =
        dirs::home_dir().ok_or_else(|| Error::Other("HOME directory not available".into()))?;
    // Grant the target's git *common dir* so `git worktree add` (and later
    // commits) can write worktree admin data / objects / refs. For a normal
    // repo this is `<cwd>/.git`, already inside `writable_root`; for a linked
    // worktree (the dogfooding case) it's the source repo's real `.git`,
    // outside `writable_root`, which would otherwise fail closed.
    let extra_writable: Vec<PathBuf> = run_target_git_common_dir(cwd).into_iter().collect();
    let profile_text = crate::sandbox::build_run_profile(cwd, &home, &extra_writable)?;
    let profile_file = crate::sandbox::profile_tempfile(&profile_text)?;
    let profile_path = profile_file
        .path()
        .to_str()
        .ok_or_else(|| Error::Other("profile path not utf-8".into()))?
        .to_string();
    let shell = user_shell();
    let shell_str = shell
        .to_str()
        .ok_or_else(|| Error::Other("shell path not utf-8".into()))?
        .to_string();
    // sandbox-exec -f <profile> <shell> -lic <cmd>
    let mut args = vec!["-f".to_string(), profile_path, shell_str];
    args.extend(shell_args(cmd));
    // A nested Fletch (dogfooding) can't reach the host's `~/.fletch/{rpc,
    // worktrees}` under this profile; steer both to sandbox-writable roots.
    // Harmless for any other Run target — nothing but Fletch reads these.
    let env = vec![
        (
            crate::rpc::RPC_ROOT_ENV.to_string(),
            crate::sandbox::nested_rpc_root(cwd)
                .to_string_lossy()
                .into_owned(),
        ),
        (
            crate::workspace::WORKSPACES_ROOT_ENV.to_string(),
            crate::sandbox::nested_checkouts_root(cwd)
                .to_string_lossy()
                .into_owned(),
        ),
    ];
    Ok((
        PathBuf::from(crate::sandbox::SANDBOX_EXEC),
        args,
        env,
        profile_file,
    ))
}

/// Resolve the git *common dir* of the Run target `cwd`, canonicalized, so the
/// Run sandbox can grant writes to it. Returns `None` when `cwd` isn't a git
/// repo (nothing to grant), git can't be resolved, or the dir can't be
/// canonicalized (a non-real path wouldn't match the kernel's symlink-resolved
/// check anyway). Runs synchronously — the profile is assembled off the async
/// runtime — via the app's resolved git binary (portable-git fallback),
/// matching every other git call.
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
    app: AppHandle,
    agent_id: String,
    session: Arc<RunSession>,
    gen: u64,
    cwd: std::path::PathBuf,
    project_id: String,
    phase: RunPhase,
    exit: crate::pty_session::PtyExit,
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
            // port could have been taken since the click.
            let prepared = match sup.prepare_run(&project_id, &cwd, &run_cmd) {
                Ok(p) => p,
                Err(e) => {
                    let msg = format!("Failed to start run command: {e}");
                    session.mark_stopped(Some(msg.clone()));
                    emit_run_state(&app, &agent_id, RunPhase::Stopped, Some(msg));
                    return;
                }
            };
            session.transition_phase(RunPhase::Running);
            emit_run_state(&app, &agent_id, RunPhase::Running, None);
            if let Some(port) = prepared.port {
                emit_run_port(&app, &agent_id, port);
            }
            write_header(&app, &agent_id, &session, &prepared.cmd);
            if let Some(note) = &prepared.note {
                write_note(&app, &agent_id, &session, note);
            }
            if let Err(e) = spawn_run_phase(
                sup,
                app.clone(),
                agent_id.clone(),
                session.clone(),
                gen,
                cwd,
                project_id.clone(),
                RunPhase::Running,
                prepared.cmd,
                prepared.extra_env,
                None,
            ) {
                let msg = format!("Failed to start run command: {e}");
                session.mark_stopped(Some(msg.clone()));
                emit_run_state(&app, &agent_id, RunPhase::Stopped, Some(msg));
            }
            return;
        }
        // No run command to chain into — treat as clean stop.
        session.mark_stopped(None);
        emit_run_state(&app, &agent_id, RunPhase::Stopped, None);
        return;
    }

    // Setup failed → do NOT proceed to run. Surface the error.
    if matches!(phase, RunPhase::Setup) && !exit.success {
        let msg = format!("Setup failed: {}", exit.message);
        session.mark_stopped(Some(msg.clone()));
        emit_run_state(&app, &agent_id, RunPhase::Stopped, Some(msg));
        return;
    }

    // Run-phase exit — natural end or crash. Either way → Stopped.
    let err = if exit.success {
        None
    } else {
        Some(format!("Run exited: {}", exit.message))
    };
    session.mark_stopped(err.clone());
    emit_run_state(&app, &agent_id, RunPhase::Stopped, err);
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
}
