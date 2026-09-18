//! Bringing the engine up.
//!
//! Everything the Tauri `setup` closure used to do that is *not* about a window
//! lives here: the database, the in-process setting mirrors the spawn path reads
//! without a DB handle, the supervisor, the workflow scheduler, the roadmap
//! loops, the startup sweeps and (on a unix host) the termination handler. The
//! desktop calls [`boot`] from `setup` and then does its own UI work with the
//! handles [`Engine`] hands back; a headless host calls the same function and
//! has nothing to add.
//!
//! No `tauri::` anywhere below, by design — that is the whole point of the
//! module. What genuinely needs the desktop is passed in as a closure
//! ([`BootConfig::focus`], [`BootConfig::on_supervisor`],
//! [`BootConfig::recover_db`], [`BootConfig::signals`]) or done by the caller
//! after [`boot`] returns.
//!
//! The order of the steps is the order `setup` ran them in, down to the
//! interleaving that looks arbitrary: several of these mirrors are read by
//! threads that start inside a later step, so "roughly the same order" is not
//! the same thing.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::broadcast;

use crate::host::sink::{BroadcastSink, FanoutSink, Sink, EVENT_BUFFER};
use crate::host::{EngineCtx, Event};
use crate::roadmap::Db;
use crate::supervisor::Supervisor;
use crate::workflow::scheduler::WorkflowService;
use crate::workspace::WorkspaceManager;
use crate::{
    bin_resolve, codegraph, database, git_dist, github, linear, publish_prefs, rpc, sandbox,
    secrets, workspace,
};

/// How the host recovers a failed `database::init`.
///
/// The desktop prompts (move the DB aside / reveal logs / quit) and loops until
/// it has a live handle or the user quits, which is why this returns a [`Db`]
/// rather than a `Result`. A host with nobody to ask passes `None` and gets
/// [`BootError::Database`].
pub type DbRecovery = Box<dyn Fn(crate::error::Error) -> Db>;

/// Host work that has to happen inside the boot sequence rather than after it;
/// see [`BootConfig::on_supervisor`].
pub type SupervisorHook = Box<dyn FnOnce(&Arc<Supervisor>)>;

/// Builds the host's local speech engine once the engine context exists; see
/// [`BootConfig::dictation`]. A closure because the dispatcher it returns reads
/// the ctx's DB handle, and the ctx is made inside [`boot`].
pub type DictationHook = Box<dyn FnOnce(Arc<EngineCtx>) -> Arc<dyn crate::remote::Dispatch>>;

/// What a caught SIGINT/SIGTERM does once the children are dead; see
/// [`BootConfig::signals`].
#[cfg(unix)]
pub type ExitHook = Box<dyn FnOnce() + Send>;

/// What the engine needs from its host to start.
pub struct BootConfig {
    /// Fletch's data directory. Must already exist.
    pub data_dir: PathBuf,
    /// Where the engine's events go — the desktop webview, or nothing at all.
    /// [`boot`] wraps it in a fanout with its own broadcast (see
    /// [`Engine::events`]), so the host's sink keeps receiving everything it
    /// did before.
    pub sink: Sink,
    /// Is the user looking at this host right now? See [`EngineCtx::focus`].
    pub focus: Box<dyn Fn() -> bool + Send + Sync>,
    /// The runtime the engine's background tasks belong to (`host::runtime`).
    pub runtime: tokio::runtime::Handle,
    pub remote: RemoteBoot,
    /// The host's local speech engine, for the five `dictation_*` ops. The
    /// desktop passes its whisper-backed dispatcher; a host without one passes
    /// `None`, and those ops answer "unavailable" (see
    /// [`crate::remote::SupervisorDispatch::with_dictation`]).
    pub dictation: Option<DictationHook>,
    /// Recovery for a failed `database::init`; `None` boots no further.
    pub recover_db: Option<DbRecovery>,
    /// Run once the supervisor exists and before anything resumes work. The
    /// desktop arms its activity monitor here rather than after [`boot`]: the
    /// monitor subscribes to the supervisor's status broadcast, and
    /// `resume_active_runs` (a few steps below) can bring an agent up, so
    /// arming afterwards would leave a window whose transitions nobody saw.
    pub on_supervisor: Option<SupervisorHook>,
    /// Install the SIGINT/SIGTERM handler, running this closure once the
    /// supervisor's children are dead so the host can exit its own way (the
    /// desktop flags the quit as confirmed and asks Tauri to exit). `None`
    /// installs no handler — what a test wants, since the handler is
    /// process-wide.
    #[cfg(unix)]
    pub signals: Option<ExitHook>,
}

/// Whether this host serves paired devices.
// `Off` is what a test and the "local engine off" desktop setting pass; the
// desktop always wants `Desktop`, and `fletch-host serve` always wants
// `Headless`.
#[allow(dead_code)]
pub enum RemoteBoot {
    /// No listener, no taps, no device store.
    Off,
    /// Today's desktop behaviour: install the taps unconditionally and start
    /// the listener if the stored setting says so.
    Desktop,
    /// A host with no window: the listener always starts, whatever
    /// `remote.enabled` says, because serving devices is the only reason the
    /// process exists. Each field overrides the stored setting *for this run
    /// only* — nothing here is written back, exactly as the desktop's `boot`
    /// writes none of the three (the settings pane's `remote_set_*` commands
    /// are what persist a user's choice).
    Headless {
        /// `None` uses the stored `remote.port`, or [`crate::remote::DEFAULT_PORT`].
        port: Option<u16>,
        relay: HeadlessRelay,
        /// The name a paired device shows for this host. `None` asks the
        /// machine, as the desktop does.
        name: Option<String>,
    },
}

/// What a headless host does about the relay.
pub enum HeadlessRelay {
    /// Whatever `remote.relay_url` says, which is what the desktop does.
    Stored,
    /// Dial this one instead.
    Url(String),
    /// No outbound link this run, whatever is stored (`--no-relay`).
    Off,
}

/// The engine, booted. Everything a host has to hold on to: the desktop hands
/// most of these to `app.manage` for its commands to read.
pub struct Engine {
    pub ctx: Arc<EngineCtx>,
    pub db: Db,
    /// The supervisor's own workspace manager. The desktop reaches it through
    /// the supervisor, so nothing here reads it yet; the host CLI's project
    /// commands will.
    #[allow(dead_code)]
    pub workspace: Arc<WorkspaceManager>,
    pub supervisor: Arc<Supervisor>,
    pub workflows: Arc<WorkflowService>,
    /// `None` under [`RemoteBoot::Off`].
    pub remote: Option<Arc<crate::remote::RemoteState>>,
    /// Every engine event, for the host's own subscribers. The remote taps are
    /// already subscribed (see [`RemoteBoot::Desktop`]), which is why the
    /// desktop ignores this; a headless host can watch it without a webview.
    #[allow(dead_code)]
    pub events: broadcast::Sender<Event>,
}

/// Why the engine could not start. The database is fatal for every host; the
/// other two are only reachable under [`RemoteBoot::Headless`], where they are
/// fatal because a host that cannot isolate an agent, or cannot be reached by
/// any client, has nothing left to do. Every other step is best-effort and logs.
#[derive(Debug)]
pub enum BootError {
    Database(crate::error::Error),
    /// No sandbox engine is available and none is configured. Off macOS only:
    /// there is no `sandbox-exec` to fall back to.
    Sandbox(String),
    /// The listener could not bind (the port is taken, most likely).
    Remote(crate::error::Error),
}

impl std::fmt::Display for BootError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(e) => write!(f, "database init failed: {e}"),
            Self::Sandbox(e) => write!(f, "{e}"),
            Self::Remote(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for BootError {}

/// Bring the engine up in this process.
pub fn boot(cfg: BootConfig) -> Result<Engine, BootError> {
    let BootConfig {
        data_dir,
        sink: host_sink,
        focus,
        runtime,
        remote,
        dictation,
        recover_db,
        on_supervisor,
        #[cfg(unix)]
        signals,
    } = cfg;

    let headless = matches!(remote, RemoteBoot::Headless { .. });
    // Before the first secret is read (the seeds a few steps below): a headless
    // process has no login keychain to unlock, so it keeps its secrets in the
    // settings table even where the desktop would not. See `secrets`.
    if headless {
        secrets::use_settings_store_only();
    }

    let db = match database::init(&data_dir) {
        Ok(db) => db,
        Err(e) => match recover_db {
            Some(recover) => recover(e),
            None => return Err(BootError::Database(e)),
        },
    };

    // The runtime the engine's background tasks belong to. Published before
    // anything below can spawn, so the engine never has to reach for `tauri::`
    // to start a task (see `host::runtime`).
    crate::host::runtime::init(runtime);

    // Where the engine's events go: the host's own sink (the desktop webview,
    // as always), plus a broadcast the host's subscribers read — today the
    // remote event taps, which used to be `listen_any` taps on the Tauri bus.
    // The fanout is kept concrete because the push-alert tap is added to it
    // further down, once the remote state it needs exists.
    let (events, _) = broadcast::channel(EVENT_BUFFER);
    let sink = Arc::new(FanoutSink::new(vec![
        host_sink,
        Arc::new(BroadcastSink(events.clone())),
    ]));

    // What the engine gets instead of an `AppHandle`: where its events go, the
    // DB handle it shares with the commands, and the "is the user looking at
    // this?" question only a host can answer. Built here so everything below is
    // constructed with it; the supervisor and the workflow service are published
    // onto it as they appear (they are what the engine used to fetch back out of
    // Tauri's managed state).
    let ctx = Arc::new(EngineCtx::new(sink.clone(), db.clone(), focus));

    // Seed the in-memory agent binary override registry so binary resolution
    // (deep in spawn/probe paths, with no DB handle) can honor user-set custom
    // paths without touching the DB each time.
    bin_resolve::set_agent_overrides(database::load_agent_bin_overrides(&db.lock()));

    // Seed the in-memory sandbox engine selection (mirror of the
    // `sandbox_engine` setting) so spawn-time engine resolution — deep in agent
    // code with no DB handle — honors the user's choice. Missing/unknown values
    // keep the sandbox-exec default, except on a headless host off macOS, where
    // there is no such thing (see `headless_container_engine`).
    match database::get_setting(&db.lock(), sandbox::ENGINE_SETTING)
        .as_deref()
        .and_then(sandbox::EngineKind::from_setting)
    {
        Some(kind) => sandbox::set_selected_engine_kind(kind),
        None if headless && !cfg!(target_os = "macos") => {
            sandbox::set_selected_engine_kind(headless_container_engine()?)
        }
        None => {}
    }

    {
        // Seed the publish-approval mirror the same way: the git dispatcher
        // reads it on the spawn path, where there is no DB handle.
        rpc::approval::set_enabled(rpc::approval::parse_enabled(
            database::get_setting(&db.lock(), rpc::approval::SETTING).as_deref(),
        ));
        rpc::approval::set_wait_secs(rpc::approval::parse_wait_secs(
            database::get_setting(&db.lock(), rpc::approval::WAIT_SETTING).as_deref(),
        ));
        // And the publish preferences the same dispatcher (and the PR path)
        // read: branch prefix and draft PRs.
        publish_prefs::set_branch_prefix(
            &database::get_setting(&db.lock(), publish_prefs::BRANCH_PREFIX_SETTING)
                .unwrap_or_default(),
        );
        publish_prefs::set_draft_prs(publish_prefs::parse_draft_prs(
            database::get_setting(&db.lock(), publish_prefs::DRAFT_PRS_SETTING).as_deref(),
        ));
        // The alert opt-out, read off threads with no DB handle. (Dictation's
        // is the desktop's to seed — it belongs to a capture session, not to
        // the engine.)
        crate::remote::push::set_turn_complete(crate::remote::push::parse_turn_complete(
            database::get_setting(&db.lock(), crate::remote::push::TURN_COMPLETE_SETTING)
                .as_deref(),
        ));
    }

    // Seed the in-memory code-indexing consent (mirror of the
    // `code_indexing_enabled` setting, default on) so the spawn path can read it
    // without a DB handle. If enabled, kick a silent background install of the
    // codegraph bundle — non-fatal: a failure just means MCP injection doesn't
    // happen until a later successful install (retried next launch or when the
    // user re-toggles).
    {
        let enabled = codegraph::parse_enabled(
            database::get_setting(&db.lock(), codegraph::SETTING).as_deref(),
        );
        codegraph::set_enabled(enabled);
        if enabled {
            crate::host::spawn(async {
                if let Err(e) = codegraph::ensure_installed().await {
                    tracing::warn!(error = %e, "codegraph startup install failed; continuing");
                }
            });
        }
    }

    // Seed the per-runtime launch knobs (image override + resource limits) the
    // same way — mirrored in-process for the spawn path, which has no DB handle.
    // Kept in sync mid-run by `set_docker_launch_settings` /
    // `set_podman_launch_settings`.
    {
        let conn = db.lock();
        sandbox::docker::set_launch_settings(sandbox::docker::LaunchSettings {
            image_override: database::get_setting(&conn, sandbox::docker::IMAGE_SETTING),
            memory: database::get_setting(&conn, sandbox::docker::MEMORY_SETTING),
            cpus: database::get_setting(&conn, sandbox::docker::CPUS_SETTING),
        });
        sandbox::podman::set_launch_settings(sandbox::podman::LaunchSettings {
            image_override: database::get_setting(&conn, sandbox::podman::IMAGE_SETTING),
            memory: database::get_setting(&conn, sandbox::podman::MEMORY_SETTING),
            cpus: database::get_setting(&conn, sandbox::podman::CPUS_SETTING),
        });
    }

    // Seed each runtime's version-refresh loop guard (mirrors of the
    // `docker_version_refresh_guard` / `podman_version_refresh_guard` settings —
    // private bookkeeping, not user-facing) and wire their write-backs, so a
    // host CLI pinned away from the registry's latest triggers at most one
    // version-parity rebuild ever, not one per app run. Same mirror idiom as the
    // launch knobs above; the guards are consulted and recorded on
    // spawn/background threads that have no DB handle. Per runtime because the
    // image stores are separate: a rebuild that settled docker's mismatch proves
    // nothing about podman's store.
    {
        let seed = |key: &'static str| -> std::collections::HashMap<String, String> {
            database::get_setting(&db.lock(), key)
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        };
        let persister = |key: &'static str| {
            let guard_db = db.clone();
            move |attempted: &std::collections::HashMap<String, String>| {
                if let Ok(json) = serde_json::to_string(attempted) {
                    let _ = database::set_setting(&guard_db.lock(), key, &json);
                }
            }
        };
        sandbox::docker::init_version_refresh_guard(
            seed(sandbox::docker::VERSION_GUARD_SETTING),
            persister(sandbox::docker::VERSION_GUARD_SETTING),
        );
        sandbox::podman::init_version_refresh_guard(
            seed(sandbox::podman::VERSION_GUARD_SETTING),
            persister(sandbox::podman::VERSION_GUARD_SETTING),
        );
    }

    // Seed the in-process container auth token (mirror of the stored
    // `claude_container_token` secret, same pattern as the GitHub token below)
    // so the docker auth chain — resolved at spawn time with no DB handle — sees
    // a token pasted in a previous run.
    seed_secret_mirror(
        &db,
        sandbox::docker::auth::TOKEN_SETTING,
        sandbox::docker::auth::seed_stored_token,
    );

    // Unified git resolution: point the portable-install root at app data, wire
    // the fallback commit identity to the signed-in profile, and kick off
    // resolve-or-download in the background — at launch, not at the onboarding
    // readiness screen, so a git-less machine is usually ready before the user
    // gets there.
    git_dist::init(data_dir.join("git-dist"));
    // Seed the in-process GitHub token so API calls and git network auth work
    // without a DB handle (updated on sign-in).
    seed_secret_mirror(&db, github::TOKEN_SETTING, github::seed_token);
    // Same for the Linear API key, so the issue adapters — reached from poll
    // paths with no DB handle — see a key pasted in a previous run.
    seed_secret_mirror(&db, linear::TOKEN_SETTING, linear::seed_token);
    {
        let db = db.clone();
        git_dist::set_identity_source(Box::new(move || database::get_account_identity(&db.lock())));
    }
    {
        let sink = sink.clone();
        crate::host::spawn(git_dist::startup(move |payload| {
            crate::host::emit(sink.as_ref(), "git-dist:state", &payload);
        }));
    }

    // Forward container image-build progress to the UI — either runtime's,
    // through one sink. The build runs deep in the spawn path (no ctx there), so
    // it emits through a process-wide sink installed here — mirroring the
    // git-dist emitter above. Every phase carries its `runtime` and the frontend
    // routes on it: with two runtimes sharing this stream it's the lifecycle key,
    // not decoration. The `docker:build-progress` event name is the established
    // wire contract and stays as-is.
    {
        let sink = sink.clone();
        sandbox::docker::set_build_sink(move |event| {
            crate::host::emit(sink.as_ref(), "docker:build-progress", &event);
        });
    }

    // One-time move of the legacy on-disk checkouts root
    // (`~/.fletch/worktrees` → `~/.fletch/workspaces`) for installs that predate
    // the rename. Best-effort; runs before the supervisor provisions any checkout
    // so restores resolve to the new location.
    workspace::migrate_default_checkouts_root();

    let workspace = Arc::new(WorkspaceManager::new(db.clone()));

    // Drop RPC mailboxes left by agents that are gone. Teardown removes them
    // now, but installs predating that carry one leaked dir per agent ever
    // spawned. Runs before anything spawns, so every mailbox present is either a
    // live agent's or an orphan.
    match workspace.live_agent_ids() {
        Ok(live) => rpc::sweep_orphan_mailboxes(&live),
        Err(e) => tracing::warn!(error = %e, "skipping rpc mailbox sweep"),
    }

    let supervisor = Arc::new(Supervisor::new(workspace.clone()));
    ctx.set_supervisor(supervisor.clone());

    // The host's own pre-resume work (the desktop's activity monitor). Before
    // the workflow service, for the reason `on_supervisor` documents.
    if let Some(hook) = on_supervisor {
        hook(&supervisor);
    }

    // Workflow engine (S4): the run scheduler + active-run registry. Its driver
    // wraps the supervisor; runs left `pending`/`running` by a prior session are
    // re-driven now (paused runs wait for a user action).
    let wf_driver: Arc<dyn crate::workflow::driver::AgentDriver> = Arc::new(
        crate::workflow::driver::SupervisorDriver::new(supervisor.clone(), ctx.clone()),
    );
    let workflows = Arc::new(WorkflowService::new(db.clone(), wf_driver, ctx.clone()));
    ctx.set_workflows(workflows.clone());
    workflows.resume_active_runs();
    // The roadmap queue drainer: turns `queued` roadmap items into runs through
    // the service above, and settles finished runs back onto the board. Started
    // after `resume_active_runs` so a run this process is already re-driving is
    // counted against the per-project concurrency cap before the first tick can
    // dispatch anything.
    crate::roadmap::drainer::spawn(ctx.clone(), db.clone(), workflows.clone());
    // The other end of the same loop: watch the PRs of items already `in_review`
    // and ship them when they merge. Host-side on purpose — the webview's PR
    // polling stops with the window, and a queue whose dependants unblock only
    // while you are looking at the board is not an autonomous queue. Sweeps once
    // now (a PR may well have merged while the app was closed), then sleeps
    // until there is something to watch.
    crate::roadmap::merge_sweep::spawn(ctx.clone(), db.clone());
    // Reload follow-ups that were queued behind an in-flight turn when a prior
    // run exited, so a mid-turn message survives a restart. They rest in the
    // queue and flush on the user's next send (no auto-spawn).
    supervisor.rehydrate_pending_messages();

    // Paired-device remote access. The event taps go in unconditionally (with
    // nothing connected, forwarding short-circuits before it touches a payload);
    // whether the listener starts is the one thing the two hosts disagree about.
    // On the desktop it starts only if the user turned it on, and a failure is
    // never fatal — the app is a desktop app first. Headless it always starts,
    // and a failure ends the boot, because a host nothing can reach has no
    // second purpose to fall back on.
    let remote = match remote {
        RemoteBoot::Off => None,
        // Both hosts build the same thing; only what they do with it differs.
        serving => {
            let mut dispatch =
                crate::remote::SupervisorDispatch::new(ctx.clone(), supervisor.clone());
            if let Some(build) = dictation {
                dispatch = dispatch.with_dictation(build(ctx.clone()));
            }
            let dispatch = Arc::new(dispatch);
            // `RemoteState::new` cannot fail: the state has to be managed even
            // when the device store or the host key is unusable, or
            // `remote_status` panics the moment Settings opens. Such a failure
            // travels as `RemoteStatus::error` instead.
            let state = crate::remote::RemoteState::new(&data_dir.join("remote"), dispatch);
            // Forwarding rides the broadcast; the push-alert tap has to run
            // inside each emit, so it comes back as a sink and joins the fanout
            // here (see `remote::push`).
            sink.add(crate::remote::install_taps(
                &ctx,
                state.clone(),
                events.subscribe(),
            ));
            let (enabled, port, relay_url) = {
                let conn = db.lock();
                (
                    crate::remote::parse_enabled(
                        database::get_setting(&conn, crate::remote::ENABLED_SETTING).as_deref(),
                    ),
                    crate::remote::parse_port(
                        database::get_setting(&conn, crate::remote::PORT_SETTING).as_deref(),
                    ),
                    database::get_setting(&conn, crate::remote::RELAY_URL_SETTING),
                )
            };
            match serving {
                RemoteBoot::Off => unreachable!("handled above"),
                RemoteBoot::Desktop => {
                    // The URL is stored before the autostart, so `start` brings
                    // the host link up with the listener. Safe outside the async
                    // runtime: nothing is enabled yet, so this cannot spawn the
                    // link task here.
                    if let Err(e) = state.set_relay(relay_url) {
                        tracing::warn!(error = %e, "remote: stored relay url rejected");
                    }
                    if enabled {
                        // `start` binds synchronously but spawns onto the async
                        // runtime, so it has to run inside it.
                        let state = state.clone();
                        crate::host::spawn(async move {
                            if let Err(e) = state.start(port) {
                                tracing::error!(error = %e, "remote: autostart failed");
                            }
                        });
                    }
                }
                RemoteBoot::Headless {
                    port: port_override,
                    relay,
                    name,
                } => {
                    if let Some(name) = name.as_deref() {
                        crate::remote::set_name_override(name);
                    }
                    let relay_url = match relay {
                        HeadlessRelay::Stored => relay_url,
                        HeadlessRelay::Url(url) => Some(url),
                        HeadlessRelay::Off => None,
                    };
                    // A URL the CLI passed is a typo worth refusing outright,
                    // unlike a stored one the settings pane already validated.
                    if let Err(e) = state.set_relay(relay_url) {
                        return Err(BootError::Remote(e));
                    }
                    // Unconditional, and synchronous so a port clash is the
                    // process's exit status rather than a log line nobody reads:
                    // `serve` runs `boot` inside its runtime for this reason.
                    // The stored `remote.enabled` is not consulted and not
                    // written — a host serves devices by definition.
                    state
                        .start(port_override.unwrap_or(port))
                        .map_err(BootError::Remote)?;
                }
            }
            Some(state)
        }
    };

    // Reclaim nested-Fletch RPC mailbox and checkout roots left in the temp dir
    // by dead instances (dogfooding runs). Live instances' roots are pid-keyed
    // and skipped, so a side-by-side Fletch is left untouched.
    sandbox::cleanup_nested_rpc_roots();
    sandbox::cleanup_nested_checkouts_roots();
    // Same reclamation for containers left by dead instances, one sweep per
    // container runtime — each probe-gated and on its own thread, so startup
    // never waits on either.
    sandbox::docker::sweep_orphans_at_startup();
    sandbox::podman::sweep_orphans_at_startup();

    // Quitting the desktop normally goes through `RunEvent::ExitRequested`, but
    // a SIGINT (Ctrl-C under `tauri dev`) or SIGTERM (sent by the OS on
    // logout/restart/shutdown, and by an init system to a host service) bypasses
    // it. Catch both via an async listener — safe to do real work here, unlike a
    // raw signal handler — kill the children, then let the host exit cleanly.
    // SIGKILL/crash can't be caught; for those the kernel closes our PTY masters
    // on death, which SIGHUPs each agent's process group as a backstop.
    #[cfg(unix)]
    if let Some(exit) = signals {
        let supervisor = supervisor.clone();
        crate::host::spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigint = signal(SignalKind::interrupt()).expect("install SIGINT handler");
            let mut sigterm = signal(SignalKind::terminate()).expect("install SIGTERM handler");
            tokio::select! {
                _ = sigint.recv() => {}
                _ = sigterm.recv() => {}
            }
            tracing::info!("termination signal received; killing child processes");
            supervisor.shutdown();
            exit();
        });
    }

    // Agents rest at Idle on boot — no process is spawned. The supervisor brings
    // one up lazily on the user's next interaction (the frontend resumes on
    // send), so nothing auto-spawns here.
    Ok(Engine {
        ctx,
        db,
        workspace,
        supervisor,
        workflows,
        remote,
        events,
    })
}

/// The sandbox a headless host uses off macOS when the user has never chosen
/// one: Docker if the daemon answers, else Podman, else nothing.
///
/// There is no safe default to fall back to. `sandbox-exec` — the engine the
/// desktop starts with — is a macOS binary, and `engine_for` deliberately fails
/// closed rather than launching an agent outside the boundary the record claims,
/// so a host that guessed wrong here would accept spawns and refuse every one of
/// them at the last moment. Refusing to boot says the same thing at the one
/// moment an operator is watching.
fn headless_container_engine() -> Result<crate::sandbox::EngineKind, BootError> {
    use crate::sandbox::{DockerAvailability, EngineKind, PodmanAvailability};

    let docker = sandbox::docker::availability();
    if matches!(docker, DockerAvailability::Available { .. }) {
        tracing::info!("no sandbox engine configured; using docker");
        return Ok(EngineKind::Docker);
    }
    let podman = sandbox::podman::availability();
    if matches!(podman, PodmanAvailability::Available { .. }) {
        tracing::info!("no sandbox engine configured; using podman");
        return Ok(EngineKind::Podman);
    }
    Err(BootError::Sandbox(format!(
        "no container runtime is available, and this host has no sandbox engine configured. \
         Docker: {docker:?}. Podman: {podman:?}. Install and start one of them, or set the \
         `{}` setting, then start fletch-host again.",
        sandbox::ENGINE_SETTING
    )))
}

/// Startup seed retry pacing for an unavailable secret store: start at 30s (the
/// realistic case — a login-item launch racing the keychain unlock — resolves
/// quickly) and back off to a slow probe that never gives up. An unavailable
/// store must stay distinct from "no token" for the app's whole lifetime, not
/// just a startup window: a saved login must never read as signed-out only
/// because the keychain stayed locked past some deadline.
const SECRET_SEED_RETRY_START: std::time::Duration = std::time::Duration::from_secs(30);
const SECRET_SEED_RETRY_CAP: std::time::Duration = std::time::Duration::from_secs(300);

/// Seed an in-process secret mirror from the store at startup. A definitive
/// answer (present or absent) applies immediately; an *unavailable* store
/// (`Err` — e.g. the keychain is locked) retries in the background until it gets
/// one. `apply` must be the mirror's `seed_*` variant (`github::seed_token`,
/// `linear::seed_token`, `docker::auth::seed_stored_token`), which no-ops once
/// any explicit set has run — a connect/disconnect that lands while a retry is
/// pending must win over the (possibly stale) value that retry read.
fn seed_secret_mirror(db: &Db, key: &'static str, apply: fn(Option<String>)) {
    match secrets::get(&db.lock(), key) {
        Ok(value) => apply(value),
        Err(e) => {
            tracing::warn!(key, error = %e, "secret store unavailable at startup; retrying");
            let db = db.clone();
            crate::host::spawn(async move {
                let mut delay = SECRET_SEED_RETRY_START;
                loop {
                    tokio::time::sleep(delay).await;
                    match secrets::get(&db.lock(), key) {
                        // A definitive answer ends the retry either way; the
                        // seed fn itself refuses to apply over an explicit set
                        // that happened while we waited (the mirror's seal), so
                        // a late read can't clobber fresher state. Applying a
                        // late None is skipped outright — the mirror already
                        // defaults to empty.
                        Ok(Some(value)) => return apply(Some(value)),
                        Ok(None) => return,
                        Err(_) => delay = (delay * 2).min(SECRET_SEED_RETRY_CAP),
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The runtime this test boots onto. Deliberately a `static`, not
    /// `#[tokio::test]`'s: `boot` publishes its handle process-wide
    /// (`host::runtime::init`, a `OnceLock`), so every *other* test's
    /// `host::spawn` in this binary lands on it too. A runtime that died with
    /// this test would leave their tasks unpolled.
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();

    /// The headless shape of a boot: a data dir, a sink the test can read, no
    /// remote listener, no signal handler. What the engine is supposed to hand
    /// back is a context its own code can reach the supervisor and the workflow
    /// service through — the two things it used to fetch out of Tauri's managed
    /// state.
    #[test]
    fn boot_returns_an_engine_whose_ctx_carries_the_services() {
        let runtime = RUNTIME.get_or_init(|| tokio::runtime::Runtime::new().unwrap());
        runtime.block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let sink = Arc::new(crate::host::sink::RecordingSink::new());
            let engine = boot(BootConfig {
                data_dir: dir.path().to_path_buf(),
                sink: sink.clone(),
                focus: Box::new(|| false),
                runtime: tokio::runtime::Handle::current(),
                remote: RemoteBoot::Off,
                dictation: None,
                recover_db: None,
                on_supervisor: None,
                #[cfg(unix)]
                signals: None,
            })
            .expect("boot");

            assert!(engine.ctx.supervisor().is_some());
            assert!(engine.ctx.workflows().is_some());
            // The same instances, not fresh ones: everything the engine spawned
            // during boot holds these.
            assert!(Arc::ptr_eq(
                &engine.ctx.supervisor().unwrap(),
                &engine.supervisor
            ));
            assert!(Arc::ptr_eq(
                &engine.ctx.workflows().unwrap(),
                &engine.workflows
            ));
            assert!(
                engine.remote.is_none(),
                "RemoteBoot::Off started a listener"
            );

            // The host's sink is still in the path after boot wrapped it in the
            // fanout that feeds `Engine::events`. Not an equality assertion:
            // boot's own tasks (git resolution) emit through the same sink.
            let mut rx = engine.events.subscribe();
            crate::host::emit(engine.ctx.sink.as_ref(), "wf:run-deleted", "run-1");
            assert!(sink
                .events()
                .contains(&("wf:run-deleted".to_string(), serde_json::json!("run-1"))));
            let forwarded = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    let (name, _) = rx.recv().await.unwrap();
                    if name.as_ref() == "wf:run-deleted" {
                        return;
                    }
                }
            })
            .await;
            assert!(
                forwarded.is_ok(),
                "the event never reached the boot broadcast"
            );
        });
    }
}
