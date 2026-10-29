//! The box sandbox engine: one libkrun micro-VM per agent, reached only over
//! ssh on vsock and an egress proxy, behind the `boxes_enabled` developer flag.
//! The guest holds the agent's checkout at the same absolute paths the host
//! uses, so host code that touches the tree is routed into the guest, never
//! given a translated path. Nothing of the host's enters the guest except the
//! env file written for each launch, and a step that cannot keep that promise
//! fails closed. With the flag off nothing here is reachable:
//! [`availability`] refuses first, and `engine_for(Box)` asks it.
//!
//! Layout:
//! - [`settings`]: the flag and knobs, mirrored in-process.
//! - [`registry`]: `BoxRegistry`, one entry per box-stamped agent: boot, stop
//!   ladder, idle timer, relays, discard.
//! - [`executor`]: the `RemoteExecutor` the path-routed seam (`crate::exec`)
//!   dispatches to for a box's writable root.
//! - [`relay`]: the mailbox and transcript relays over the channel.
//! - [`provision`]: a checkout provisioned *inside* the guest.
//! - [`mirror`]: the host-side bare mirror publish and transport read from.
//! - [`setup`]: the image install and the tools prebuild, and how a launch
//!   waits on them.
//!
//! Host-side layout per box (`~/.fletch/[dev/]boxes/<agent id>/`, a sibling of
//! the workspaces root rather than inside it, so no host file sits under a path
//! the seam routes remotely): `data.img`, `dirty`, `key`, `key.pub`,
//! `known_hosts`, `ssh_config`, `ssh.sock`, `ctl.sock`, `egress.sock`,
//! `console.log`, `mirror.git/`, `claude-projects/`.

pub mod executor;
pub mod mirror;
pub mod provision;
pub mod registry;
pub mod relay;
pub mod settings;
pub mod setup;
#[cfg(test)]
mod tests;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use fletch_box::{ImageStore, InstalledImage, PinnedImage, Probe, RuntimePaths};
use serde::Serialize;

use crate::error::{Error, Result};
use crate::host::Sink;
use crate::sandbox::engine::{
    AgentLaunchCtx, EngineKind, EnvDelivery, KillHandle, KillPlan, LaunchPlan, SandboxEngine,
};

pub use registry::{BoxIdentity, BoxPhase, BoxRegistry, RunningBox, StopReason};

/// The ssh client every launch runs through.
const SSH: &str = fletch_box::channel::SSH;
/// The bash that quotes a launch's arguments for the guest shell (see
/// [`executor::quoting_wrapper`]).
pub(crate) const BASH: &str = "/bin/bash";

/// Where this host keeps its boxes: `<checkouts base>/boxes`, next to the
/// `workspaces` root and under the same per-build split, so a debug and a
/// release build never see each other's VMs.
pub fn boxes_root() -> Result<PathBuf> {
    let root = crate::workspace::checkouts_root()?;
    Ok(root
        .parent()
        .map(|p| p.join("boxes"))
        .unwrap_or_else(|| root.join("boxes")))
}

/// One box's directory.
pub fn box_dir(agent_id: &str) -> Result<PathBuf> {
    Ok(boxes_root()?.join(agent_id))
}

/// The box dir of the agent whose writable root is `writable_root`: the
/// agent id is the root's last component (`workspace::agent_parent_dir`).
pub(crate) fn box_dir_for_root(writable_root: &Path) -> Option<PathBuf> {
    let agent_id = writable_root.file_name()?.to_str()?;
    box_dir(agent_id).ok()
}

/// The transcript mirror of a box (`sandbox::transcript_root` for `Box`).
pub(crate) fn transcript_dir_for_root(writable_root: &Path) -> Option<PathBuf> {
    box_dir_for_root(writable_root).map(|dir| dir.join(registry::CLAUDE_PROJECTS_DIR))
}

/// The engine's box data: images and tools disks under `<data dir>/box`.
pub fn data_root() -> PathBuf {
    crate::sandbox::configured_data_dir()
        .unwrap_or_else(crate::data_dir)
        .join("box")
}

pub fn image_store() -> ImageStore {
    ImageStore::new(&data_root())
}

pub fn tools_root() -> PathBuf {
    data_root().join("tools")
}

/// This build's pinned image, from the `box_image_url` setting or the release.
pub fn pinned_image() -> PinnedImage {
    PinnedImage::this_build(settings::current().image_url.as_deref())
}

/// Where the helper and libkrun are, in order: the `FLETCH_BOX_HELPER` /
/// `FLETCH_BOX_LIB_DIR` override, the app bundle (`Contents/Helpers`,
/// `Contents/Frameworks`), else the dev checkout the running binary is inside
/// (`packaging/box-bundle/out`, staged by `ensure-dev.sh` on `tauri dev`).
/// `None` only outside all three; a dev checkout resolves even before the
/// runtime is built, so the probe can say what to run.
pub fn runtime_paths() -> Option<RuntimePaths> {
    let run_root = boxes_root().ok()?;
    if let Some(mut paths) = RuntimePaths::from_env() {
        paths.run_root = run_root;
        return Some(paths);
    }
    let exe = std::env::current_exe().ok()?;
    RuntimePaths::bundled(&exe, run_root.clone()).or_else(|| RuntimePaths::dev(&exe, run_root))
}

/// Everything a boot needs, checked.
pub struct Ready {
    pub paths: RuntimePaths,
    pub image: InstalledImage,
}

/// The sentence for a host with no runtime location at all.
pub const RUNTIME_NOT_FOUND: &str = "the box runtime was not found: no fletch-vm helper in the \
                                     app bundle, in packaging/box-bundle/out (run \
                                     packaging/box-bundle/ensure-dev.sh), or named by \
                                     FLETCH_BOX_HELPER";

/// Why `engine_for(Box)` refuses, else the runtime a boot uses. Fail closed at
/// every rung: the flag, the machine, the helper and dylibs. The image is not
/// a rung: a launch waits for its download ([`setup::ready`]) rather than
/// refusing, so the engine is handed out while it is still on its way.
pub fn availability() -> Result<RuntimePaths> {
    if !settings::enabled() {
        return Err(Error::SandboxUnavailable(
            "box sandbox is off; enable it in Developer settings".into(),
        ));
    }
    let paths =
        runtime_paths().ok_or_else(|| Error::SandboxUnavailable(RUNTIME_NOT_FOUND.into()))?;
    let probe = paths.probe();
    if !probe.ok() {
        return Err(Error::SandboxUnavailable(format!(
            "the box runtime is not usable: {}.",
            probe.problems().join("; ")
        )));
    }
    Ok(paths)
}

/// This build's image, when installed.
pub fn installed_image() -> Result<Option<InstalledImage>> {
    // This build's version, never the newest on disk: a newer image left by a
    // later build would boot a guest whose init contract this build does not
    // know. The file is hashed against its recorded sha256 before the first
    // boot of a session (the registry), which is where damage is caught.
    Ok(image_store().find(&pinned_image().version))
}

/// The runtime probe alone, for the Settings page: `None` when no runtime
/// location exists at all.
pub fn probe() -> Option<Probe> {
    runtime_paths().map(|paths| paths.probe())
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

pub const EVENT_STATE: &str = "box:state";
pub const EVENT_EGRESS: &str = "box:egress";
pub const EVENT_IMAGE: &str = "box:image";
pub const EVENT_TOOLS: &str = "box:tools";

static SINK: OnceLock<Sink> = OnceLock::new();

/// Where the engine's `box:*` events go. Installed once at boot, like the
/// container build sink: the registry emits from paths with no ctx.
pub fn set_event_sink(sink: Sink) {
    if SINK.set(sink).is_err() {
        tracing::warn!("box event sink already set");
    }
}

pub(crate) fn emit<T: Serialize>(event: &str, payload: &T) {
    if let Some(sink) = SINK.get() {
        crate::host::emit(sink.as_ref(), event, payload);
    }
}

/// `box:egress`.
#[derive(Debug, Clone, Serialize)]
pub struct EgressEventPayload {
    pub agent_id: String,
    pub host: String,
    pub port: u16,
    pub allowed: bool,
}

// ---------------------------------------------------------------------------
// Waiting on async work from the sync engine seams
// ---------------------------------------------------------------------------

/// Run `fut` to completion from a synchronous caller (`launch_agent`,
/// `deliver_env`, the executor) on the engine's runtime, from a scoped thread
/// so a runtime worker never blocks on itself.
pub(crate) fn block_on<T: Send>(fut: impl Future<Output = T> + Send) -> T {
    let handle = crate::host::runtime::handle();
    std::thread::scope(|scope| {
        scope
            .spawn(move || match handle {
                Some(handle) => handle.block_on(fut),
                // No runtime at all (a bare unit test): a throwaway one.
                None => tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("build a runtime")
                    .block_on(fut),
            })
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

// ---------------------------------------------------------------------------
// The engine
// ---------------------------------------------------------------------------

/// Names of host variables that never reach a guest env file, whatever the
/// assembled env holds: the user's credentials and agents.
pub const ENV_DENY: &[&str] = &[
    "SSH_AUTH_SOCK",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
    "GNUPGHOME",
];

/// What a launch needs to deliver its env file after the plan was built.
struct PendingLaunch {
    agent_id: String,
    provider: String,
    home: PathBuf,
}

/// `EngineKind::Box`. One instance per process ([`BoxEngine::shared`]), like
/// the container engines; launches park an `Arc` of it in their `KillHandle`.
pub struct BoxEngine {
    pending: Mutex<std::collections::HashMap<String, PendingLaunch>>,
}

impl BoxEngine {
    pub fn shared() -> Arc<BoxEngine> {
        static ENGINE: OnceLock<Arc<BoxEngine>> = OnceLock::new();
        ENGINE
            .get_or_init(|| {
                Arc::new(BoxEngine {
                    pending: Mutex::new(std::collections::HashMap::new()),
                })
            })
            .clone()
    }
}

/// Which providers run in a box so far: the ones with a tools-disk script
/// *and* a launch whose credential is one host-resolved env var. Codex, Cursor
/// and OpenCode have a script but their logins live in files the host writes
/// into a host-side overlay; delivering those into a guest is later work.
pub fn supports_provider(provider: &str) -> bool {
    provider == "claude"
}

/// Why `provider` cannot launch in a box, for the spawn gate.
pub fn provider_refusal(provider: &str) -> Option<String> {
    (!supports_provider(provider)).then(|| {
        let label = crate::agent::provider_bin_label(provider).map_or(provider, |(_, l)| l);
        format!("{label} isn't available in box sandboxes yet")
    })
}

/// The engine a new agent of `provider` is stamped with when `selected` is the
/// host's choice: `Box` only for a provider boxes can launch, otherwise the
/// engine the flag replaced ([`settings::fallback_engine`]: Docker stays
/// Docker, a hand-picked `box` falls back to the default). While the pilot runs
/// Claude Code alone, enabling it must not make every other provider
/// unsendable or quietly weaken the isolation they had; their capsule badge
/// shows the engine they got.
pub fn engine_for_new_agent(selected: EngineKind, provider: &str) -> EngineKind {
    match selected {
        EngineKind::Box if !supports_provider(provider) => {
            let fallback = settings::fallback_engine();
            tracing::info!(
                provider,
                engine = fallback.as_setting(),
                "box engine does not run this provider yet; using the previous engine"
            );
            fallback
        }
        kind => kind,
    }
}

/// The identity a launch context names.
fn identity_of(ctx: &AgentLaunchCtx) -> BoxIdentity {
    BoxIdentity {
        agent_id: ctx.agent_id.to_string(),
        project_id: ctx.project_id.to_string(),
        provider: ctx.provider.to_string(),
        writable_root: ctx.writable_root.to_path_buf(),
        rpc_dir: ctx.rpc_dir.to_path_buf(),
        home: ctx.home.to_path_buf(),
    }
}

impl SandboxEngine for BoxEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::Box
    }

    /// Boot (or reuse) the agent's box and plan an ssh session into it running
    /// `agent_bin` under `fletch-guest exec` with the env file [`deliver_env`]
    /// writes next. The launcher is `bash -c` quoting the agent's arguments for
    /// the guest shell, then `exec ssh`.
    fn launch_agent(&self, ctx: &AgentLaunchCtx, agent_bin: &str) -> Result<LaunchPlan> {
        if let Some(why) = provider_refusal(ctx.provider) {
            return Err(Error::Other(why));
        }
        if ctx.adopted_workspace().is_some() || ctx.blackboard.is_some() {
            return Err(Error::Other(
                "workflow step agents don't run in box sandboxes yet".into(),
            ));
        }
        let identity = identity_of(ctx);
        let registry = BoxRegistry::global();
        // Waits for the image and the tools disk when they are still being
        // set up (`setup`), so a launch during setup is late, never refused.
        let mut running = block_on(registry.ensure_running(&identity))?;
        if running.tools_disk.is_none() {
            // Booted before the disk existed (an older build, or a host CLI
            // upgrade since): a restart attaches the disk `ensure_running`
            // now waits for.
            block_on(registry.stop(ctx.agent_id, StopReason::Reattach))?;
            running = block_on(registry.ensure_running(&identity))?;
            if running.tools_disk.is_none() {
                return Err(Error::SandboxUnavailable(
                    "the box restarted without its tools disk".into(),
                ));
            }
        }
        let launch_id = uuid::Uuid::new_v4().to_string();
        let env_file = env_file_path(&launch_id);

        let remote =
            fletch_box::Channel::remote_command(Some(ctx.cwd), Some(&env_file), &[agent_bin]);
        let tty = if ctx.interactive { "-tt" } else { "-T" };
        let prefix_args = executor::quoting_wrapper(
            SSH,
            &[
                "-F".to_string(),
                running.channel.config.to_string_lossy().into_owned(),
                tty.to_string(),
                fletch_box::guest::SSH_HOST_ALIAS.to_string(),
            ],
            &remote,
        );

        // The claude login the host resolved (`agent::host_login`), as the
        // seatbelt engine passes it; the container auth chain adds the stored
        // setup-token or the shell's own key when there is no host login.
        let mut env = Vec::new();
        match crate::sandbox::container::auth::resolve(ctx.oauth_token) {
            crate::sandbox::container::auth::ContainerAuth::Resolved { env: auth, .. } => {
                env.extend(auth)
            }
            crate::sandbox::container::auth::ContainerAuth::Unavailable => {}
        }
        env.push(("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()));

        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(
                launch_id.clone(),
                PendingLaunch {
                    agent_id: ctx.agent_id.to_string(),
                    provider: ctx.provider.to_string(),
                    home: ctx.home.to_path_buf(),
                },
            );
        BoxRegistry::global().attach(ctx.agent_id, &launch_id);
        Ok(LaunchPlan {
            program: PathBuf::from(BASH),
            prefix_args,
            env,
            env_remove: ENV_DENY.iter().map(|v| v.to_string()).collect(),
            env_delivery: EnvDelivery::GuestFile {
                launch_id: launch_id.clone(),
            },
            kill: KillHandle::Engine {
                engine: BoxEngine::shared(),
                plan: KillPlan::Box {
                    agent_id: ctx.agent_id.to_string(),
                    launch_id,
                },
            },
        })
    }

    /// Write the launch's env file into the guest (`/data/run/<launch>.env`,
    /// 0600) and return its path. This file is the only way host state reaches
    /// the agent's process: sshd accepts no client environment.
    fn deliver_env(
        &self,
        agent_id: &str,
        launch_id: &str,
        env: &[(String, String)],
    ) -> Result<PathBuf> {
        let pending = self
            .pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(launch_id)
            .ok_or_else(|| Error::Other(format!("no box launch {launch_id} for {agent_id}")))?;
        if pending.agent_id != agent_id {
            return Err(Error::Other(format!(
                "box launch {launch_id} belongs to {}, not {agent_id}",
                pending.agent_id
            )));
        }
        let contents = render_env_file(env, &pending.provider, &pending.home, ENV_DENY)?;
        let channel = BoxRegistry::global().channel_for(agent_id)?;
        let path = env_file_path(launch_id);
        executor::put(&channel, contents.as_bytes(), &path, 0o600)?;
        Ok(path)
    }

    /// The session's own kill ends the ssh client (and so the guest PTY); the
    /// engine detaches the launch so the idle timer may stop the VM, and
    /// removes the env file the launch's token sat in.
    fn kill(&self, plan: &KillPlan) -> Result<()> {
        let KillPlan::Box {
            agent_id,
            launch_id,
        } = plan
        else {
            return Ok(());
        };
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(launch_id);
        let registry = BoxRegistry::global();
        if let Ok(channel) = registry.channel_for_running(agent_id) {
            let path = env_file_path(launch_id);
            let _ = executor::run(&channel, &["rm", "-f", &path.to_string_lossy()], None);
        }
        registry.detach(agent_id, launch_id);
        Ok(())
    }
}

/// `/data/run/<launch id>.env`.
fn env_file_path(launch_id: &str) -> PathBuf {
    PathBuf::from(fletch_box::guest::ENV_FILE_DIR).join(format!("{launch_id}.env"))
}

/// The env file for a launch: the assembled env minus the deny-list, plus what
/// the guest needs that the host never sets (`HOME` by path identity, the
/// tools disk on `PATH`, the loopback proxy, a shell). Guest values win over
/// the host's where both name a key, since a host `PATH` means nothing there.
/// A value with a newline cannot be a `KEY=VALUE` line and is refused rather
/// than truncated.
pub fn render_env_file(
    env: &[(String, String)],
    provider: &str,
    home: &Path,
    deny: &[&str],
) -> Result<String> {
    let term = std::env::var("TERM")
        .ok()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "xterm-256color".to_string());
    let guest: Vec<(String, String)> = vec![
        ("HOME".into(), home.to_string_lossy().into_owned()),
        ("USER".into(), fletch_box::guest::USER.into()),
        ("LOGNAME".into(), fletch_box::guest::USER.into()),
        (
            "PATH".into(),
            format!(
                "{}:/usr/local/bin:/usr/bin:/bin",
                fletch_box::tools::bin_dir(provider)
            ),
        ),
        ("SHELL".into(), "/bin/bash".into()),
        ("TERM".into(), term),
        ("LANG".into(), "C.UTF-8".into()),
        (
            "HTTPS_PROXY".into(),
            fletch_box::guest::LOOPBACK_PROXY.into(),
        ),
        (
            "HTTP_PROXY".into(),
            fletch_box::guest::LOOPBACK_PROXY.into(),
        ),
        (
            "https_proxy".into(),
            fletch_box::guest::LOOPBACK_PROXY.into(),
        ),
        (
            "http_proxy".into(),
            fletch_box::guest::LOOPBACK_PROXY.into(),
        ),
        ("NO_PROXY".into(), "127.0.0.1,localhost".into()),
        ("no_proxy".into(), "127.0.0.1,localhost".into()),
        ("GIT_TERMINAL_PROMPT".into(), "0".into()),
    ];
    let guest_keys: Vec<&str> = guest.iter().map(|(k, _)| k.as_str()).collect();
    let mut lines = String::new();
    for (key, value) in env {
        if deny.contains(&key.as_str()) || guest_keys.contains(&key.as_str()) {
            continue;
        }
        push_line(&mut lines, key, value)?;
    }
    for (key, value) in &guest {
        push_line(&mut lines, key, value)?;
    }
    Ok(lines)
}

fn push_line(lines: &mut String, key: &str, value: &str) -> Result<()> {
    if key.is_empty() || key.contains('=') || key.contains('\n') || key.contains('\0') {
        return Err(Error::Other(format!("invalid env name {key:?}")));
    }
    if value.contains('\n') || value.contains('\0') {
        return Err(Error::Other(format!(
            "env var {key} has a value with a newline, which an env file cannot carry"
        )));
    }
    lines.push_str(key);
    lines.push('=');
    lines.push_str(value);
    lines.push('\n');
    Ok(())
}

// ---------------------------------------------------------------------------
// Lifecycle hooks for the rest of the engine
// ---------------------------------------------------------------------------

/// Stop and delete the box of `agent_id` if it has one; nothing to do for an
/// agent of another engine. Teardown calls this for every agent, so it must
/// stay cheap when there is no box: one directory probe. An error is logged
/// and returned, for archive to record on the row; a box directory it leaves
/// is reclaimed at the next start ([`startup`]).
pub async fn discard_if_any(agent_id: &str) -> Result<()> {
    let dir = box_dir(agent_id)?;
    let registry = BoxRegistry::global();
    if !dir.exists() && !registry.knows(agent_id) {
        return Ok(());
    }
    registry.discard(agent_id).await.inspect_err(|e| {
        tracing::warn!(agent_id, error = %e, "box discard failed");
    })
}

/// Stop every running box cleanly; app exit (`Supervisor::shutdown`).
pub fn shutdown_all() {
    if BoxRegistry::global().running_ids().is_empty() {
        return;
    }
    block_on(BoxRegistry::global().shutdown_all());
}

/// At boot: route every live box-stamped agent's writable root to its box
/// (so a Git panel read before the first launch boots it lazily), log boxes
/// whose last VM did not stop cleanly, and kill `fletch-vm` processes whose
/// owning Fletch is dead. Off the boot thread for the process scan.
pub fn startup(workspace: &crate::workspace::WorkspaceManager) {
    if settings::enabled() {
        if let Ok(ids) = workspace.live_agent_ids() {
            for id in ids {
                let Ok(record) = workspace.agent(&id) else {
                    continue;
                };
                if record.sandbox_engine.as_deref() != Some(EngineKind::Box.as_setting()) {
                    continue;
                }
                let identity = BoxIdentity::for_record(&record);
                match identity {
                    Ok(identity) => BoxRegistry::global().register(identity),
                    Err(e) => tracing::warn!(agent_id = %id, error = %e, "box identity failed"),
                }
            }
        }
    }
    let leaked = leaked_box_dirs(workspace);
    std::thread::spawn(move || {
        // Helpers first: a leaked box may still have its VM up.
        registry::sweep_orphans();
        for dir in leaked {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => {
                    tracing::info!(box_dir = %dir.display(), "removed a box no live agent owns")
                }
                Err(e) => {
                    tracing::warn!(box_dir = %dir.display(), error = %e, "removing a leaked box failed")
                }
            }
        }
    });
}

/// Box directories no live agent owns: left by a discard that failed, or by
/// an archive the app died in the middle of (`begin_archive` has already
/// marked the row). Each holds a whole data disk, so nothing else would ever
/// reclaim it. Only a directory with a data disk counts — the root also holds
/// a tools build's throwaway `tools-<pid>` box — and the directories are
/// listed before the agents are read, so a box made in between belongs to a
/// row that is already live.
fn leaked_box_dirs(workspace: &crate::workspace::WorkspaceManager) -> Vec<PathBuf> {
    let Some(entries) = boxes_root()
        .ok()
        .and_then(|root| std::fs::read_dir(root).ok())
    else {
        return Vec::new();
    };
    let boxes: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let path = entry.path();
            (!name.starts_with("tools-") && path.join(registry::DATA_DISK).is_file())
                .then_some((name, path))
        })
        .collect();
    if boxes.is_empty() {
        return Vec::new();
    }
    let Ok(live) = workspace.live_agent_ids() else {
        return Vec::new();
    };
    boxes
        .into_iter()
        .filter(|(agent_id, _)| !live.contains(agent_id))
        .map(|(_, dir)| dir)
        .collect()
}
