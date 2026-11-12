//! The box sandbox engine: one libkrun micro-VM per agent, reached only over
//! ssh on vsock and an egress proxy. The standard engine wherever a box can
//! boot: a new agent is stamped `box` the moment the image and its
//! provider's tools disk are in ([`engine_for_new_agent`]), and seatbelt
//! until then — the image downloads itself in the background
//! ([`setup::autostart`]) and no launch ever waits for it. The guest holds
//! the agent's checkout at the same absolute paths the host uses, so host
//! code that touches the tree is routed into the guest, never given a
//! translated path. Nothing of the host's enters the guest except the env
//! file written for each launch and the folders the user granted the
//! workspace (`workspace::folder_grants`, mounted as virtio-fs shares at their
//! own paths), and a step that cannot keep that promise fails closed. On a
//! machine that cannot boot a box nothing here is reachable: [`availability`]
//! refuses first, and `engine_for(Box)` asks it.
//!
//! Layout:
//! - [`settings`]: the knobs, mirrored in-process.
//! - [`registry`]: `BoxRegistry`, one entry per box-stamped agent: boot, stop
//!   ladder, idle timer, relays, discard.
//! - [`executor`]: the `RemoteExecutor` the path-routed seam (`crate::exec`)
//!   dispatches to for a box's writable root.
//! - [`relay`]: the mailbox and transcript relays over the channel.
//! - [`provision`]: a checkout provisioned *inside* the guest.
//! - [`mirror`]: the host-side bare mirror publish and transport read from.
//! - [`setup`]: the image install and the tools prebuild, and how a launch
//!   waits on them.
//! - [`ssh_access`]: the user's own ssh client reaching a box, for an
//!   editor's remote mode.
//! - [`run`]: the Run panel's process in the guest, its port forwarded to
//!   the host.
//!
//! Host-side layout per box (`~/.fletch/[dev/]boxes/<agent id>/`, a sibling of
//! the workspaces root rather than inside it, so no host file sits under a path
//! the seam routes remotely): `data.img`, `dirty`, `key`, `key.pub`,
//! `known_hosts`, `ssh_config`, `ssh.sock`, `ctl.sock`, `egress.sock`,
//! `console.log`, `mirror.git/`, the provider's sessions mirror
//! (`claude-projects/`, `codex-sessions/` — [`home`]), `relay/`.
//!
//! **The engine names no provider.** What a CLI keeps outside the checkout —
//! its config, its login, its transcripts — and how each reaches or leaves
//! the guest is one adapter per provider behind
//! [`ProviderHome`](crate::sandbox::provider::ProviderHome) (`sandbox::provider`,
//! shared with every engine); the engine seeds the guest home, delivers the
//! credential and mirrors the sessions through that one interface ([`home`]).
//!
//! **An agent runs where the tree it works in lives.** An agent that adopts
//! an existing checkout instead of provisioning one (the local reviewer, which
//! reads the coder's checkout) runs *in the box that holds it* when that box
//! is another agent's — as its tenant ([`registry::Tenant`]): same VM, its own
//! ssh session, env file, mailbox relay and profile dir, no box of its own.
//! It need not share the owner's provider: a box boots with every provider's
//! tools disk that is built, the owner's on top, so each CLI is at its usual
//! path; a tenant whose disk was not aboard restarts the box with it when no
//! other session is attached, and is refused otherwise
//! (`BoxRegistry::reattach_with_tools`).
//! Conversely an agent adopting a host tree never runs in a box, which cannot
//! see it ([`crate::sandbox::engine_for_adopted_tree`]). Inside the guest both
//! agents are one user, so a file mode cannot keep the reviewer off the
//! coder's files; its launch gets namespaces of its own instead (`fletch-guest
//! exec --read-only … --writable … --cow … --cow-file …`), in which the
//! reviewer writes nothing it shares with the coder ([`confined_view`]):
//! everything the guest exposes of the coder's — its writable root, the home,
//! its mailbox, the folders shared writable — is read-only, each subtree whole
//! with the mounts nested in it; the reviewer's own paths (its writable root,
//! its mailbox, its writable folder grants) are bound writable again; and
//! every shared location a CLI writes (the provider's state dirs, the scratch
//! dirs, the toolchain cache, `~/.claude.json`) is a private copy-on-write
//! view: an overlay whose upper layer, or a copy of the file, lies under the
//! reviewer's own root ([`COW_DIR`]). It reads the coder's state there and
//! nothing it writes reaches it, a file that did not exist before included,
//! so no per-provider list of what a CLI executes is needed; its session
//! transcripts are read from its upper layer ([`ConfinedView::private_path`]).
//! `/data` is hidden, `/proc` is private and no capability is left. This is
//! enforced with no fallback: a guest that predates the options or cannot set
//! them up refuses the launch, naming the box image to update. What it does
//! not cover is scratch the guest user may write anyway (`/tmp` and the other
//! tmpfs mounts) and the network namespace, which is the guest's.
//!
//! What touches a checkout from the host goes through the seam, or asks
//! [`box_holding`] and refuses with the reason. The Run panel's process runs
//! in the guest as a launch of the box, its port forwarded to the host
//! ([`run`]). Not in the guest yet, each refusing or skipping rather than
//! acting on the host's empty placeholder: verification (`verify`) and
//! workflow step agents (`BoxEngine::launch_agent` refuses a blackboard). Codegraph
//! runs in the guest from the tools disk ([`codegraph_bin`]), the attribution
//! hook refresh reaches the guest through the seam (`attribution::
//! refresh_hooks`), and a turn checkpoint crosses the box boundary through
//! the mirror and the channel (`sandbox::carry_checkpoint`: a fork of a box
//! agent with its code, or into one). The auto-archive sweep never boots a
//! box: each stop reads every checkout's disposal state after the mirror
//! refresh, within a time bound, and writes it to `<box dir>/disposal.json`,
//! which the box's start deletes before any setup wait; the sweep answers a
//! stopped box's checkouts from that file, and nothing for a box in any
//! other phase (`mirror::record_disposal`, `sandbox::disposal_at_stop`).

pub mod allowlist;
pub mod cache_disk;
pub mod egress_log;
pub mod egress_prompt;
pub mod executor;
pub mod home;
mod launch_files;
pub mod mirror;
pub mod provision;
pub mod registry;
pub mod relay;
pub mod run;
pub mod settings;
pub mod setup;
pub mod ssh_access;
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
    TurnPrep,
};
use crate::sandbox::placement::Placement;
use crate::sandbox::provider::GuestEntry;
use crate::sandbox::{policy, provider, ToolchainCache};

pub use registry::{BoxIdentity, BoxPhase, BoxRegistry, RunningBox, StopReason};

/// The ssh client every launch runs through.
const SSH: &str = fletch_box::channel::SSH;
/// The bash that quotes the arguments of a program run in the guest for the
/// user or for git for the guest shell ([`executor::quoting_wrapper`]). An
/// agent launch has none to quote: its arguments are staged as a file
/// ([`launch_files`]) and its launcher is ssh itself.
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

/// The claude transcript mirror of a box (`sandbox::transcript_root` for
/// `Box`, which the claude session locators walk). Codex rollouts are
/// located through [`home::mirror_for_agent`] instead.
pub(crate) fn transcript_dir_for_root(writable_root: &Path) -> Option<PathBuf> {
    home::mirror_for_root(&provider::claude::Claude, writable_root)
}

/// The box whose writable root holds `path`, when a registered one does: the
/// one answer to "which guest has this tree" — for transport and publish
/// (`mirror`), and for an agent that adopts the tree and so must run there.
/// Only boxes the registry knows (every live box-stamped agent's, from
/// [`startup`] on), so a path under no box reads as a host path.
pub fn box_holding(path: &Path) -> Option<BoxIdentity> {
    BoxRegistry::global().identity_for_path(path)
}

/// Whether `record` is a tenant: an agent whose primary checkout is adopted
/// from inside another agent's box. The box must be registered for the answer
/// to be yes, as for [`box_holding`].
fn tenancy_of(record: &crate::workspace::AgentRecord) -> Option<BoxIdentity> {
    let adopted = record.repos.first()?.adopted_checkout.as_deref()?;
    box_holding(adopted).filter(|host| host.agent_id != record.id)
}

/// Route a box-stamped `record` that is a tenant to its host's box, and say
/// whose. Called before every launch of a box-stamped agent and at
/// [`startup`], so the tenant's own writable root is routed when the host
/// writes the launch's profile under it. `Ok(None)` for an agent with a box
/// of its own; an error for a box-stamped agent adopting a tree that is in no
/// box — a guest cannot see a host tree, so that launch has nowhere to run.
pub fn route_tenant(record: &crate::workspace::AgentRecord) -> Result<Option<BoxIdentity>> {
    let Some(adopted) = record
        .repos
        .first()
        .and_then(|r| r.adopted_checkout.as_deref())
    else {
        return Ok(None);
    };
    let Some(host) = tenancy_of(record) else {
        return Err(Error::SandboxUnavailable(format!(
            "{} adopts {}, which is in no box; an agent in a box can only adopt a tree another box holds",
            record.id,
            adopted.display()
        )));
    };
    let writable_root = crate::workspace::agent_parent_dir(&record.id)?;
    // As the launch derives `adopted_read_only` (`supervisor::lifecycle`): a
    // reviewer reads the tree it adopts, and so runs confined.
    let confined = record.purpose.as_deref() == Some(crate::workspace::PURPOSE_REVIEWER);
    BoxRegistry::global().register_tenant(&record.id, writable_root, &host.agent_id, confined);
    Ok(Some(host))
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
/// `Contents/Frameworks`; on Linux, `fletch-vm` and the libraries beside the
/// running binary, as a `fletch-host` release installs them), else the dev
/// checkout the running binary is inside
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
#[cfg(not(target_os = "linux"))]
pub const RUNTIME_NOT_FOUND: &str = "the box runtime was not found: no fletch-vm helper in the \
                                     app bundle, in packaging/box-bundle/out (run \
                                     packaging/box-bundle/ensure-dev.sh), or named by \
                                     FLETCH_BOX_HELPER";
#[cfg(target_os = "linux")]
pub const RUNTIME_NOT_FOUND: &str = "the box runtime was not found: no fletch-vm helper beside \
                                     this executable (a fletch-host release installs it there \
                                     with libkrun.so.1 and libkrunfw.so.5), in \
                                     packaging/box-bundle/out (run \
                                     packaging/box-bundle/ensure-dev.sh), or named by \
                                     FLETCH_BOX_HELPER";

/// Why `engine_for(Box)` refuses, else the runtime a boot uses. Fail closed at
/// every rung: the machine, the helper and dylibs. The image is not
/// a rung: a launch waits for its download ([`setup::ready`]) rather than
/// refusing, so the engine is handed out while it is still on its way.
pub fn availability() -> Result<RuntimePaths> {
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

/// Whether this machine and runtime can boot a box, probed once per process:
/// the helper, its entitlement and dylibs, the architecture and the
/// hypervisor do not change while Fletch runs, and the probe spawns
/// `codesign` and `sysctl`, which every spawn's engine choice must not pay.
/// A passing probe is remembered, and so is one the machine itself fails
/// (Intel, no hypervisor); a runtime that is merely missing is asked again,
/// since a dev checkout can stage it while the app runs.
pub fn runtime_usable() -> bool {
    runtime_refusal().is_none()
}

/// Why this machine and runtime cannot boot a box, in the sentence the user
/// sees; `None` when they can. One probe answers both this and
/// [`runtime_usable`], remembered on the same terms: a pass, or a machine
/// that fails itself; a merely missing runtime is probed again next time.
pub fn runtime_refusal() -> Option<String> {
    static SETTLED: OnceLock<Option<String>> = OnceLock::new();
    if let Some(settled) = SETTLED.get() {
        return settled.clone();
    }
    let probe = probe();
    let refusal = match &probe {
        Some(probe) if probe.ok() => None,
        Some(probe) => Some(format!(
            "the box runtime is not usable: {}.",
            probe.problems().join("; ")
        )),
        None => Some(RUNTIME_NOT_FOUND.to_string()),
    };
    let settled = refusal.is_none() || probe.is_some_and(|p| !p.arch_ok || !p.hypervisor);
    if settled {
        let _ = SETTLED.set(refusal.clone());
    }
    refusal
}

/// Whether a new agent of `provider` could boot a box right now, without
/// waiting on anything: the provider, the runtime, this build's image, and
/// the provider's tools disk — the gate the automatic engine choice stands
/// behind. Everything here is a flag read or a file stat once the runtime
/// probe has passed.
pub fn ready_for(provider: &str) -> bool {
    supports_provider(provider)
        && runtime_usable()
        && matches!(installed_image(), Ok(Some(_)))
        && registry::built_tools_disk(provider).is_some()
}

/// The layers every box gets besides the base.
pub(crate) fn image_layers() -> Vec<String> {
    fletch_box::image_store::OPTIONAL_LAYERS
        .iter()
        .map(|l| l.to_string())
        .collect()
}

/// This build's image with its layers, when all of them are installed; `None`
/// while one is still on its way, so a launch waits for it like for the base.
pub fn installed_image() -> Result<Option<InstalledImage>> {
    // This build's version, never the newest on disk: a newer image left by a
    // later build would boot a guest whose init contract this build does not
    // know. The files are hashed against their recorded sha256 before the
    // first boot of a session (the registry), which is where damage is caught.
    Ok(image_store()
        .find(&pinned_image().version)
        .and_then(|image| image.with_layers(&image_layers())))
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
/// `box:policy { project_id }`: what boxes may reach changed — the global list
/// (`project_id: null`, every project) or one project's in-app layer. Reload
/// `box_egress_overview`. A project's settings files say so with
/// `project_file:changed` instead.
pub const EVENT_POLICY: &str = "box:policy";
/// The set of VMs changed: one was registered or discarded. Carries nothing:
/// a client re-reads `box_list`. A phase change is `box:state`, not this.
pub const EVENT_LIST: &str = "box:list";

static SINK: OnceLock<Sink> = OnceLock::new();

/// Where the engine's `box:*` events go. Installed once at boot, like the
/// other host sinks: the registry emits from paths with no ctx.
pub fn set_event_sink(sink: Sink) {
    if SINK.set(sink).is_err() {
        tracing::warn!("box event sink already set");
    }
}

pub(crate) fn emit<T: Serialize>(event: &str, payload: &T) {
    #[cfg(test)]
    tests::note_event(event);
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
    /// A denied host that is known noise (`fletch_box::egress::QUIET`):
    /// telemetry and the like, recorded but nothing to act on.
    pub quiet: bool,
}

/// `box:policy`.
#[derive(Debug, Clone, Serialize)]
pub struct PolicyEventPayload {
    pub project_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Waiting on async work from the sync engine seams
// ---------------------------------------------------------------------------

/// Run `fut` to completion from a synchronous caller (`launch_agent`,
/// `deliver_env`, the executor) on the engine's runtime.
pub(crate) fn block_on<T: Send>(fut: impl Future<Output = T> + Send) -> T {
    block_on_with(crate::host::runtime::handle(), fut)
}

/// [`block_on`] on `handle`'s runtime (a throwaway one when `None`). On a
/// multi-thread runtime the wait goes through `block_in_place`, which hands
/// this worker's queue to another thread and leaves the runtime context, so
/// the work `fut` waits on keeps being polled. Elsewhere `fut` runs on a
/// scoped thread, as `Handle::block_on` cannot nest in a runtime context.
pub(crate) fn block_on_with<T: Send>(
    handle: Option<tokio::runtime::Handle>,
    fut: impl Future<Output = T> + Send,
) -> T {
    let run = move || match handle {
        Some(handle) => handle.block_on(fut),
        // No runtime at all (a bare unit test): a throwaway one.
        None => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build a runtime")
            .block_on(fut),
    };
    let on_multi_thread = tokio::runtime::Handle::try_current().is_ok_and(|current| {
        current.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
    });
    if on_multi_thread {
        tokio::task::block_in_place(run)
    } else {
        std::thread::scope(|scope| {
            scope
                .spawn(run)
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
        })
    }
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
    /// The agent launching.
    agent_id: String,
    /// The agent whose box it runs in: itself, or its host when it is a
    /// tenant — whose channel the env file travels over.
    box_agent_id: String,
    provider: String,
    home: PathBuf,
    /// The launch's credential files, shipped after the env file
    /// ([`home::CredentialSync::launch`]) and kept in step for as long as
    /// the launch lives.
    credentials: Arc<Mutex<home::CredentialSync>>,
}

/// `EngineKind::Box`. One instance per process ([`BoxEngine::shared`]);
/// launches park an `Arc` of it in their `KillHandle`.
pub struct BoxEngine {
    pending: Mutex<std::collections::HashMap<String, PendingLaunch>>,
    /// Every live launch's files in its guest, by launch id: written and
    /// removed under each one's own lock ([`launch_files::LaunchFiles`]).
    files: Mutex<std::collections::HashMap<String, Arc<launch_files::LaunchFiles>>>,
    /// The port forward of every live Run launch, by launch id
    /// ([`run::GuestRun`]): dropped, and so ended, by the launch's kill.
    forwards: Mutex<std::collections::HashMap<String, fletch_box::Forward>>,
    /// Every live Run launch, by launch id, with its copy-on-write stores
    /// ([`run::GuestRun`]): its kill ends the command's process group in the
    /// guest itself, rather than leave that to the tty hangup the ssh
    /// client's end causes, and removes the stores.
    runs: Mutex<std::collections::HashMap<String, PathBuf>>,
}

impl BoxEngine {
    pub fn shared() -> Arc<BoxEngine> {
        static ENGINE: OnceLock<Arc<BoxEngine>> = OnceLock::new();
        ENGINE
            .get_or_init(|| {
                Arc::new(BoxEngine {
                    pending: Mutex::new(std::collections::HashMap::new()),
                    files: Mutex::new(std::collections::HashMap::new()),
                    forwards: Mutex::new(std::collections::HashMap::new()),
                    runs: Mutex::new(std::collections::HashMap::new()),
                })
            })
            .clone()
    }

    /// Keep `forward` for as long as launch `launch_id` lives: its kill drops
    /// it ([`SandboxEngine::kill`]).
    fn park_forward(&self, launch_id: &str, forward: fletch_box::Forward) {
        self.forwards
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(launch_id.to_string(), forward);
    }

    /// Launch `launch_id` is a Run with its copy-on-write stores at `stores`:
    /// its kill ends its process group in the guest and removes the stores
    /// ([`run::end_in_guest`]).
    fn note_run(&self, launch_id: &str, stores: PathBuf) {
        self.runs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(launch_id.to_string(), stores);
    }
}

/// A launch plan step over the launch's credential files: the box's channel,
/// then `step` on the sync. The credential state lives in the plan's own
/// closures and ends with the session, so no engine bookkeeping can run
/// ahead of, or outlive, the process it is about.
fn credential_step(
    box_agent_id: &str,
    credentials: &Arc<Mutex<home::CredentialSync>>,
    step: fn(&mut home::CredentialSync, &fletch_box::Channel) -> Result<()>,
) -> TurnPrep {
    let box_agent_id = box_agent_id.to_string();
    let credentials = credentials.clone();
    Arc::new(move || {
        let channel = BoxRegistry::global().channel_for(&box_agent_id)?;
        step(
            &mut credentials.lock().unwrap_or_else(|p| p.into_inner()),
            &channel,
        )
    })
}

/// Which providers run in a box: the box half of the support gate
/// ([`provider::supports`]: a provider adapter and a tools-disk script).
pub fn supports_provider(provider: &str) -> bool {
    provider::supports(EngineKind::Box, provider).is_ok()
}

/// The codegraph launcher's path in `agent_id`'s guest, when the tools disk
/// its launch of `provider` runs from carries codegraph: the disk attached
/// now, or for a box that is stopped (or will reattach, see `launch_agent`)
/// the one its next boot takes. That is the disk the agent's MCP server will
/// find, so the session is told about codegraph exactly when it is there.
pub fn codegraph_bin(agent_id: &str, provider: &str) -> Option<String> {
    let disk = BoxRegistry::global()
        .attached_tools_disk(agent_id, provider)
        .or_else(|| registry::bootable_tools_disk(provider))?;
    fletch_box::tools::carries_codegraph(&disk).then(fletch_box::tools::codegraph_bin_path)
}

/// Whether the box whose writable root holds `path` is running, so work there
/// would not boot it.
pub fn running_for_path(path: &Path) -> bool {
    let registry = BoxRegistry::global();
    registry
        .identity_for_path(path)
        .is_some_and(|identity| registry.channel_for_running(&identity.agent_id).is_ok())
}

/// Whether `path` lies in a box that is not running, so work there would boot
/// it. `false` for a host path.
pub fn asleep_for_path(path: &Path) -> bool {
    box_holding(path)
        .is_some_and(|identity| BoxRegistry::global().running(&identity.agent_id).is_none())
}

/// The engine a new agent of `provider` is stamped with, from the project's
/// placement.
///
/// Off macOS there is no seatbelt, so the box is the only engine: every new
/// agent is a box, its launch waiting for the image as a box-pinned
/// project's does (a Host placement means nothing there), and a machine that
/// cannot boot one refuses the spawn with why ([`availability`]).
///
/// On macOS the project's placement decides first — a project that runs on this Mac
/// (a macOS or iOS app, or the user's say-so) is seatbelt — then a box when
/// one can boot right now ([`ready_for`]) and seatbelt otherwise, so the
/// first workspace of a fresh install starts at once and the ones after the
/// download land in boxes. A project pinned to boxes on a machine that can
/// boot one waits at launch for an image still on its way (`setup::ready`).
pub fn engine_for_new_agent(provider: &str, placement: Placement) -> EngineKind {
    engine_for_new_agent_with(provider, placement, || ready_for(provider))
}

/// [`engine_for_new_agent`] with [`ready_for`] supplied by a caller that has
/// already answered part of it, so the runtime is probed once (asked only
/// when automatic gets that far).
pub fn engine_for_new_agent_with(
    provider: &str,
    placement: Placement,
    ready: impl Fn() -> bool,
) -> EngineKind {
    if cfg!(not(target_os = "macos")) {
        return EngineKind::Box;
    }
    match placement {
        Placement::Host => EngineKind::SandboxExec,
        Placement::Auto | Placement::Box if ready() => EngineKind::Box,
        // Pinned to boxes by the project but nothing to boot yet: the launch
        // waits for the image, on a machine that can boot a box at all.
        Placement::Box if supports_provider(provider) && runtime_usable() => EngineKind::Box,
        Placement::Auto | Placement::Box => EngineKind::SandboxExec,
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
        shares: Vec::new(),
    }
    .with_shares(ctx.folder_grants)
}

/// Whether the claude at `agent_bin` on the box's tools disk `disk` parses
/// `--thinking-display`: remembered per disk file (`agent::file_identity`:
/// the real path, size and modification time, so a `latest` disk rebuilt in
/// place is a new one), probed in the guest over the box's ssh channel — the
/// host's probe and reading (`agent::THINKING_DISPLAY_PROBE_ARGS`), on the
/// host probe's bounded runner, so a wedged guest can't hold a launch — the
/// first time a disk is seen. A probe that didn't run or finish is no answer:
/// nothing is remembered and the next launch asks again.
fn guest_thinking_summaries(channel: &fletch_box::Channel, disk: &Path, agent_bin: &str) -> bool {
    let identity = format!("box tools disk {}", crate::agent::file_identity(disk));
    if let Some(known) = crate::agent::thinking_display_known(&identity) {
        return known;
    }
    // `ssh -F <config> box <words…>`, each word quoted for the guest shell —
    // what `executor::run` sends, under a deadline.
    let config = channel.config.to_string_lossy().into_owned();
    let words: Vec<String> = std::iter::once(agent_bin)
        .chain(crate::agent::THINKING_DISPLAY_PROBE_ARGS)
        .map(fletch_box::channel::shell_quote)
        .collect();
    let mut args: Vec<&str> = vec!["-F", &config, fletch_box::guest::SSH_HOST_ALIAS];
    args.extend(words.iter().map(String::as_str));
    let Some(out) = crate::agent::run_probe_bounded(SSH, &args) else {
        return false;
    };
    let supported = crate::agent::probe_shows_thinking_display(&out);
    crate::agent::remember_thinking_display(&identity, supported);
    supported
}

/// Under a confined tenant's own writable root, the directory that holds its
/// private copies of what it shares with the box's owner (`confined_view`): a
/// store per shared directory, whose `upper` the guest fills with what the
/// tenant writes there (`fletch_box::guest::EXEC_COW`), and a copy per shared
/// file, each named by `cow_name`. The guest makes them, inside the
/// confinement. A name of Fletch's own beside the profile dir, which no
/// checkout is given (`workspace::allocate_repo_subdir`), and removed with the
/// tenant's root (`discard_if_any`).
pub(crate) const COW_DIR: &str = ".fletch-cow";

/// What a launch confined in a box may write, what it must only read, and
/// what it sees as a private copy (`fletch-guest exec --read-only …
/// --writable … --cow … --cow-file …`), as the guest takes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ConfinedView {
    /// Read-only whole, every mount below each included; none lies inside
    /// another.
    read_only: Vec<PathBuf>,
    /// Directories inside them the launch may write after all, parents first:
    /// its own.
    writable_dirs: Vec<PathBuf>,
    /// Directories inside them the launch sees as a private copy-on-write
    /// view, each with its store under the launch's own root; none lies inside
    /// another, or holds a mount.
    cow_dirs: Vec<(PathBuf, PathBuf)>,
    /// Files inside them the launch sees as a private copy, each with that
    /// copy under the launch's own root.
    cow_files: Vec<(PathBuf, PathBuf)>,
}

impl ConfinedView {
    /// Every `--writable` path in the order the guest binds them, parents
    /// first.
    fn writable(&self) -> Vec<&Path> {
        self.writable_dirs.iter().map(PathBuf::as_path).collect()
    }

    fn read_only(&self) -> Vec<&Path> {
        self.read_only.iter().map(PathBuf::as_path).collect()
    }

    /// Every `--cow` pair: the shared directory, then its store.
    fn cow_dirs(&self) -> Vec<(&Path, &Path)> {
        borrowed(&self.cow_dirs)
    }

    /// Every `--cow-file` pair: the shared file, then its copy.
    fn cow_files(&self) -> Vec<(&Path, &Path)> {
        borrowed(&self.cow_files)
    }

    /// The scope the host's own operations for a launch confined to this
    /// view run in ([`executor::Scope`]): inside the view, so what the host
    /// writes for the launch (its seed, its credential files) lands where the
    /// launch's own writes do, and what it reads back is what the launch
    /// sees, never the shared layer below a private copy.
    fn scope(&self) -> executor::Scope {
        executor::Scope::confined(self.exec_words())
    }

    /// `fletch-guest exec` with this view's options and an empty env file,
    /// through the `--` before a command; unquoted.
    fn exec_words(&self) -> Vec<String> {
        let mut words: Vec<String> = vec![
            fletch_box::guest::GUEST_AGENT.into(),
            fletch_box::guest::EXEC.into(),
        ];
        let options = [
            (fletch_box::guest::EXEC_READ_ONLY, self.read_only()),
            (fletch_box::guest::EXEC_WRITABLE, self.writable()),
        ];
        for (option, paths) in options {
            for path in paths {
                words.push(option.into());
                words.push(path.to_string_lossy().into_owned());
            }
        }
        let copies = [
            (fletch_box::guest::EXEC_COW, self.cow_dirs()),
            (fletch_box::guest::EXEC_COW_FILE, self.cow_files()),
        ];
        for (option, pairs) in copies {
            for (shared, private) in pairs {
                words.push(option.into());
                words.push(shared.to_string_lossy().into_owned());
                words.push(private.to_string_lossy().into_owned());
            }
        }
        let rest = ["--env-file", "/dev/null", "--"];
        words.extend(rest.map(String::from));
        words
    }

    /// Where a launch confined to this view finds what it wrote at `shared`,
    /// as the guest outside the confinement sees it: inside a copy-on-write
    /// directory, the same path in its store's upper layer; for a
    /// copy-on-write file, its copy. `None` for a path the view leaves where
    /// it is, writable or not.
    fn private_path(&self, shared: &Path) -> Option<PathBuf> {
        if let Some((_, copy)) = self.cow_files.iter().find(|(file, _)| file == shared) {
            return Some(copy.clone());
        }
        self.cow_dirs.iter().find_map(|(dir, store)| {
            let rel = shared.strip_prefix(dir).ok()?;
            let upper = store.join(fletch_box::guest::EXEC_COW_UPPER);
            if rel.as_os_str().is_empty() {
                Some(upper)
            } else {
                Some(upper.join(rel))
            }
        })
    }
}

/// `pairs` by reference, for the channel.
fn borrowed(pairs: &[(PathBuf, PathBuf)]) -> Vec<(&Path, &Path)> {
    pairs
        .iter()
        .map(|(shared, private)| (shared.as_path(), private.as_path()))
        .collect()
}

/// The view a launch in `host`'s box is confined to, when it is one: a tenant
/// that adopts its host's tree to read it (`adopted_read_only`, the local
/// reviewer). `None` for an owner, for a launch that adopts its tree to write
/// it (a workflow step), and for one that adopts nothing.
///
/// Inside the guest the reviewer is the coder's user, and the rule is that it
/// writes nothing it shares with the coder, existing or not:
///
/// - Read-only is everything the guest exposes of the coder's — the box's
///   three identity binds (its writable root, the whole home, and its
///   mailbox, which sits inside the home and so is covered by it), and every
///   folder the box shares writable that no writable grant of the reviewer's
///   covers, each whole with the mounts below it.
/// - Writable again, inside those, is what is the reviewer's own: its writable
///   root and its mailbox, a codex launch's `CODEX_HOME` (under that root),
///   and the folders it was granted writable. A grant inside a share of the
///   box (or equal to one) is carved out of the read-only share; a share
///   inside a grant is the reviewer's to write whole; a grant no share of the
///   box covers is not mounted in the guest at all. Read-only shares stay as
///   the host mounts them.
/// - Every shared location a CLI writes is a private copy-on-write view
///   ([`shared_state`]): its provider's state dirs as the adapter lays them
///   out in the guest, the agent scratch dirs
///   (`policy::agent_scratch_dirs`), the project's toolchain cache
///   (`toolchain_cache`, its guest path), each an overlay with its store
///   under [`COW_DIR`], and the provider's state files (claude's
///   `~/.claude.json`), each a private copy there. The reviewer reads the
///   coder's state through it, and what it writes lands in its own upper
///   layer: a later coder launch never sees a file it wrote, so nothing it
///   could plant there (an MCP config, a hook, a plugin) is executed by the
///   coder. What the host writes for the launch (its seed and its credential
///   files) is written in the same view ([`ConfinedView::scope`]), where the
///   reviewer reads it. That takes no list of each provider's
///   executable config, which seatbelt denies back by path
///   (`policy::provider_exec_config_denials`) and a mount could not, for a
///   path that does not exist yet. The toolchain cache is no exception: the
///   reviewer downloads what it needs into its own upper. A shared directory
///   that holds a mount (one of the box's shares or binds, or one of the
///   reviewer's writable paths) stays read-only instead, since an overlay's
///   lower layer shows one filesystem and would hide the mount.
///
/// Seatbelt's other grants have no counterpart in a guest: claude's login is
/// a token in the env file, not a credentials file, and runs with no
/// `CLAUDE_CONFIG_DIR` (`provider::claude`), a blackboard refuses the launch,
/// and `/tmp` and the other scratch mounts lie outside every bind. Each
/// writable path and shared directory must exist before the launch
/// ([`carve_out_script`]); the stores and copies the guest makes.
fn confined_view(
    ctx: &AgentLaunchCtx,
    is_tenant: bool,
    host: &BoxIdentity,
    toolchain_cache: &Path,
) -> Option<ConfinedView> {
    if !is_tenant || !ctx.adopted_read_only || ctx.adopted_workspace().is_none() {
        return None;
    }
    let grants: Vec<&Path> = ctx
        .folder_grants
        .iter()
        .filter(|grant| grant.writable)
        .map(|grant| grant.path.as_path())
        .collect();
    let shares: Vec<&Path> = host
        .shares
        .iter()
        .filter(|share| share.writable)
        .map(|share| share.path.as_path())
        .collect();
    let in_a_grant = |path: &Path| grants.iter().any(|&grant| path.starts_with(grant));
    let in_a_share = |path: &Path| shares.iter().any(|&share| path.starts_with(share));
    let coders_shares = shares.iter().copied().filter(|&share| !in_a_grant(share));
    let binds = [&host.writable_root, &host.home, &host.rpc_dir].map(PathBuf::as_path);
    let read_only = outermost(binds.into_iter().chain(coders_shares));
    // What lies outside every read-only tree keeps the guest's own mode.
    let protected = |path: &Path| read_only.iter().any(|dir| inside(path, dir));

    let codex_home = ctx.codex_home.filter(|_| ctx.provider == "codex");
    let own_grants = grants.iter().copied().filter(|&grant| in_a_share(grant));
    let own_shares = shares.iter().copied().filter(|&share| in_a_grant(share));
    let own = [ctx.writable_root, ctx.rpc_dir]
        .into_iter()
        .chain(codex_home)
        .chain(own_grants)
        .chain(own_shares);
    let mut writable: Vec<PathBuf> = Vec::new();
    for dir in own.filter(|&dir| protected(dir)) {
        if !writable.iter().any(|kept| kept == dir) {
            writable.push(dir.to_path_buf());
        }
    }
    // A bind of a directory copies what is already bound below it, so a
    // parent's bind must come first or it would cover its child's.
    writable.sort_by_key(|dir| dir.components().count());

    let (shared_dirs, shared_files) = shared_state(ctx.provider, ctx.home, Some(toolchain_cache));
    let mounts: Vec<&Path> = [&host.writable_root, &host.rpc_dir]
        .map(PathBuf::as_path)
        .into_iter()
        .chain(host.shares.iter().map(|share| share.path.as_path()))
        .chain(writable.iter().map(PathBuf::as_path))
        .collect();
    let owned = |path: &Path| writable.iter().any(|dir| path.starts_with(dir));
    let holds_a_mount = |dir: &Path| mounts.iter().any(|mount| mount.starts_with(dir));
    let cow_dirs = shared_dirs
        .iter()
        .map(PathBuf::as_path)
        .filter(|&dir| protected(dir) && !owned(dir) && !holds_a_mount(dir));
    let cow_dirs = cow_pairs(ctx.home, ctx.writable_root, outermost(cow_dirs));
    let cow_files = shared_files
        .into_iter()
        .filter(|file| protected(file) && !owned(file))
        .filter(|file| !cow_dirs.iter().any(|(dir, _)| file.starts_with(dir)))
        .collect();
    let cow_files = cow_pairs(ctx.home, ctx.writable_root, cow_files);
    Some(ConfinedView {
        read_only,
        writable_dirs: writable,
        cow_dirs,
        cow_files,
    })
}

/// What a launch in a box shares with every other launch there and writes:
/// the project's toolchain cache when it is given, its provider's shared
/// state as the adapter lays it out in the guest
/// ([`ProviderHome::shared_state`](provider::ProviderHome::shared_state),
/// which holds everything the seed and the credential files put outside the
/// writable root) and the agent scratch dirs (`policy::agent_scratch_dirs`),
/// then the provider's state files. A confined tenant sees each as a private
/// copy ([`confined_view`]).
fn shared_state(
    provider: &str,
    home: &Path,
    toolchain_cache: Option<&Path>,
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let state = provider::for_provider(provider)
        .map(|adapter| adapter.shared_state(home))
        .unwrap_or_default();
    let dirs = toolchain_cache
        .map(Path::to_path_buf)
        .into_iter()
        .chain(state.dirs)
        .chain(policy::agent_scratch_dirs(home))
        .collect();
    (dirs, state.files)
}

/// Each of `shared` with its private counterpart under `own_root`'s
/// [`COW_DIR`].
fn cow_pairs(home: &Path, own_root: &Path, shared: Vec<PathBuf>) -> Vec<(PathBuf, PathBuf)> {
    let dir = own_root.join(COW_DIR);
    shared
        .into_iter()
        .map(|path| {
            let private = dir.join(cow_name(home, &path));
            (path, private)
        })
        .collect()
}

/// The name of `shared`'s store or copy in [`COW_DIR`]: its path from the
/// home (from `/`, behind a leading `%2F`, when it lies outside the home),
/// with every `%` written `%25` and every `/` written `%2F`. One flat,
/// readable name per path, which no other path shares.
fn cow_name(home: &Path, shared: &Path) -> String {
    let (prefix, rel) = match shared.strip_prefix(home) {
        Ok(rel) if !rel.as_os_str().is_empty() => ("", rel),
        _ => ("%2F", shared.strip_prefix("/").unwrap_or(shared)),
    };
    let mut name = prefix.to_string();
    for c in rel.to_string_lossy().chars() {
        match c {
            '%' => name.push_str("%25"),
            '/' => name.push_str("%2F"),
            c => name.push(c),
        }
    }
    name
}

/// [`ConfinedView::private_path`] for a confined tenant whose own writable
/// root is `own_root`, without a launch's view: the names its launches give
/// their stores and copies are the same for every launch, so the host finds
/// what the tenant wrote between launches too (its sessions, which the host
/// pulls or writes, `home`). Over the shared state in the home, which every
/// launch's view holds read-only, less the toolchain cache: the one part that
/// depends on the project holds no sessions. What this cannot see is the rare
/// shared directory a launch leaves read-only because it holds a mount
/// (`confined_view`); a tenant writes no sessions there.
pub(crate) fn tenant_private_path(
    provider: &str,
    home: &Path,
    own_root: &Path,
    shared: &Path,
) -> Option<PathBuf> {
    let (mut dirs, mut files) = shared_state(provider, home, None);
    dirs.retain(|dir| inside(dir, home));
    files.retain(|file| inside(file, home));
    let dirs = outermost(dirs.iter().map(PathBuf::as_path));
    let view = ConfinedView {
        cow_dirs: cow_pairs(home, own_root, dirs),
        cow_files: cow_pairs(home, own_root, files),
        ..ConfinedView::default()
    };
    view.private_path(shared)
}

/// Whether `path` lies strictly below `dir`, by whole components.
fn inside(path: &Path, dir: &Path) -> bool {
    path != dir && path.starts_with(dir)
}

/// `paths` without repeats and without any that lies inside another of them,
/// in their order: a read-only tree takes every mount below it along, so a
/// nested one adds nothing.
fn outermost<'a>(paths: impl IntoIterator<Item = &'a Path>) -> Vec<PathBuf> {
    let paths: Vec<&Path> = paths.into_iter().collect();
    let mut out: Vec<PathBuf> = Vec::new();
    for path in &paths {
        let nested = paths.iter().any(|dir| inside(path, dir));
        if !nested && !out.iter().any(|kept| kept == path) {
            out.push(path.to_path_buf());
        }
    }
    out
}

/// The guest command that makes every directory `view` binds or overlays
/// exist, which a mount needs: the writable ones and the shared copy-on-write
/// ones, run before the launch in the guest's own view. A shared file the
/// view copies is made from the seed ([`mount_point_seed`]). The
/// stores and the copies are not made here but by the guest, inside the
/// confinement, where a link planted under the tenant's own root can only
/// lead to its own paths, never into the coder's.
fn carve_out_script(view: &ConfinedView) -> String {
    let dirs: Vec<String> = view
        .writable_dirs
        .iter()
        .chain(view.cow_dirs.iter().map(|(dir, _)| dir))
        .map(|dir| fletch_box::channel::shell_quote(&dir.to_string_lossy()))
        .collect();
    let mut script = String::from("umask 077");
    if !dirs.is_empty() {
        script.push_str(" && mkdir -p -- ");
        script.push_str(&dirs.join(" "));
    }
    script
}

/// Makes everything `view` mounts exist in `channel`'s guest: every directory
/// ([`carve_out_script`]), in one round trip, then every file it copies, from
/// the launch's `seed` ([`mount_point_seed`]).
fn make_carve_outs(
    channel: &fletch_box::Channel,
    view: &ConfinedView,
    seed: &[GuestEntry],
) -> Result<()> {
    let out = executor::run(channel, &["sh", "-c", &carve_out_script(view)], None)?;
    if !out.status.success() {
        return Err(Error::SandboxUnavailable(format!(
            "could not create the directories of the reviewer's view in the box: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    home::apply(
        channel,
        &executor::Scope::default(),
        &mount_point_seed(view, seed),
    )
}

/// The entries of a launch's `seed` for the files `view` copies
/// (`--cow-file`), each made only where nothing is yet. The guest binds the
/// launch's copy on the shared file, which must therefore exist before the
/// view does, and makes the copy from it the first time. Applied in the
/// guest's own view, they make no file the coder's own seed would not make,
/// with the same contents, and change none that exists; the rest of the seed
/// is applied in the view ([`ConfinedView::scope`]).
fn mount_point_seed(view: &ConfinedView, seed: &[GuestEntry]) -> Vec<GuestEntry> {
    let copied = |path: &Path| view.cow_files.iter().any(|(file, _)| file == path);
    let mut entries: Vec<GuestEntry> = seed
        .iter()
        .filter(|entry| matches!(entry, GuestEntry::File { path, .. } if copied(path)))
        .cloned()
        .collect();
    for entry in &mut entries {
        if let GuestEntry::File { keep_existing, .. } = entry {
            *keep_existing = true;
        }
    }
    entries
}

/// `view` once the guest has shown it enforces it (`guest_enforces`,
/// [`guest_confines`] in a launch); a refusal from a guest that predates
/// confinement. There is no unconfined fallback: every box this build boots
/// runs the image it pins, whose guest has the option, so a guest without it
/// is one to update, not one to run the reviewer in with write access.
fn enforced_view(
    view: Option<ConfinedView>,
    guest_enforces: impl FnOnce(&ConfinedView) -> Result<bool>,
) -> Result<Option<ConfinedView>> {
    let Some(view) = view else {
        return Ok(None);
    };
    if guest_enforces(&view)? {
        return Ok(Some(view));
    }
    Err(Error::SandboxUnavailable(
        "this box's guest predates the reviewer's confined view (`fletch-guest exec \
         --read-only`, `--cow`), so it cannot keep the reviewer off the coder's files; update \
         the box image"
            .into(),
    ))
}

/// Whether `running`'s guest confines a launch to a read-only view, asked the
/// first time a launch needs it and remembered for the boot: `fletch-guest
/// exec` with `view`'s options, the launch's own, running `/bin/true` from an
/// empty env file, which sets up the whole confinement and so also shows the
/// guest kernel allows it. A guest that has the options and cannot set them
/// up refuses the launch, as the launch itself would fail.
fn guest_confines(running: &RunningBox, view: &ConfinedView) -> Result<bool> {
    if let Some(known) = running.read_only_exec().get() {
        return Ok(*known);
    }
    let owned = probe_words(view);
    let words: Vec<&str> = owned.iter().map(String::as_str).collect();
    let out = executor::run(&running.channel, &words, None)?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    let Some(confines) = read_only_probe_answer(out.status.success(), &stderr) else {
        return Err(Error::SandboxUnavailable(format!(
            "the box could not confine the reviewer to a read-only view of the coder's files \
             ({}): {}; update the box image",
            out.status,
            stderr.trim()
        )));
    };
    let _ = running.read_only_exec().set(confines);
    Ok(confines)
}

/// The words of [`guest_confines`]'s probe, unquoted: `/bin/true` in the
/// view.
fn probe_words(view: &ConfinedView) -> Vec<String> {
    let mut words = view.exec_words();
    words.push("/bin/true".into());
    words
}

/// What a `guest_confines` probe says: `Some(true)` when the command ran
/// confined, `Some(false)` when the guest refused an option as unknown (it
/// predates it), `None` when it has the options and could not set them up
/// (exit `EXEC_CONFINE_FAILED`), or the probe never reached it.
fn read_only_probe_answer(success: bool, stderr: &str) -> Option<bool> {
    let unknown = |option: &str| stderr.contains(&format!("unknown option {option}"));
    let options = [
        fletch_box::guest::EXEC_READ_ONLY,
        fletch_box::guest::EXEC_WRITABLE,
        fletch_box::guest::EXEC_COW,
        fletch_box::guest::EXEC_COW_FILE,
    ];
    if success {
        Some(true)
    } else if options.into_iter().any(unknown) {
        Some(false)
    } else {
        None
    }
}

/// The remote command of an agent launch, `head <args file> tail`, each word
/// quoted for the guest shell: around the path of the file its arguments
/// arrive in, which is one of each process's own
/// ([`launch_files::LaunchFiles::stage`]).
#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentCommand {
    /// `fletch-guest exec --args-file`.
    head: Vec<String>,
    /// The rest of `exec`'s options, `--` and the agent.
    tail: Vec<String>,
}

impl AgentCommand {
    /// The command for the process whose arguments are in `args_file`.
    fn with_args_file(&self, args_file: &Path) -> String {
        let file = fletch_box::channel::shell_quote(&args_file.to_string_lossy());
        format!("{} {file} {}", self.head.join(" "), self.tail.join(" "))
    }
}

/// The remote command of an agent launch: `agent_bin` under `fletch-guest
/// exec` with the launch's env file and an args file, in `cwd`, confined to
/// `view`.
fn agent_remote_command(
    cwd: &Path,
    env_file: &Path,
    view: Option<&ConfinedView>,
    agent_bin: &str,
) -> AgentCommand {
    let quote = fletch_box::channel::shell_quote;
    let unconfined = ConfinedView::default();
    let view = view.unwrap_or(&unconfined);
    let head = [
        fletch_box::guest::GUEST_AGENT,
        fletch_box::guest::EXEC,
        fletch_box::guest::EXEC_ARGS_FILE,
    ];
    let mut tail = fletch_box::Channel::exec_options(
        Some(cwd),
        Some(env_file),
        &view.read_only(),
        &view.writable(),
        &view.cow_dirs(),
        &view.cow_files(),
    );
    tail.extend(["--".to_string(), agent_bin.to_string()]);
    AgentCommand {
        head: head.map(quote).to_vec(),
        tail: tail.iter().map(|w| quote(w)).collect(),
    }
}

/// The argv after [`SSH`] for one process of an agent launch, whose
/// arguments were staged in `args_file`: `command` run in the guest (`-tt`
/// when `interactive`, the PTY view), over the box's shared connection unless
/// the command is too long for it ([`executor::SHARED_CONNECTION_COMMAND_MAX`]).
/// The launcher is ssh itself: it writes nothing in the guest.
fn agent_launcher(
    ssh_config: &Path,
    interactive: bool,
    args_file: &Path,
    command: &AgentCommand,
) -> Vec<String> {
    let remote = command.with_args_file(args_file);
    let mut args = vec!["-F".to_string(), ssh_config.to_string_lossy().into_owned()];
    if remote.len() > executor::SHARED_CONNECTION_COMMAND_MAX {
        args.extend(["-S".to_string(), "none".to_string()]);
    }
    let tty = if interactive { "-tt" } else { "-T" };
    args.extend([
        tty.to_string(),
        fletch_box::guest::SSH_HOST_ALIAS.to_string(),
        remote,
    ]);
    args
}

impl SandboxEngine for BoxEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::Box
    }

    /// Boot (or reuse) the agent's box and plan an ssh session into it running
    /// `agent_bin` under `fletch-guest exec` with the env file [`deliver_env`]
    /// writes next. The launcher is ssh; each process's arguments are written
    /// into the guest first, by the host ([`agent_launcher`], [`launch_files`]).
    ///
    /// An agent whose cwd is a tree it adopted runs in the box that holds the
    /// tree, as that box's tenant (module docs): the host's VM, with the
    /// tenant's own env file, mailbox relay and launch attached to the host's
    /// idle timer. A tenant that adopts the tree to read it runs with
    /// nothing of its host's writable: its own paths are, and what it shares
    /// with its host it sees as private copies ([`confined_view`]); or it
    /// does not run at all.
    fn launch_agent(&self, ctx: &AgentLaunchCtx, agent_bin: &str) -> Result<LaunchPlan> {
        provider::supports(EngineKind::Box, ctx.provider).map_err(Error::Other)?;
        let provider_home = provider::require(ctx.provider)?;
        if ctx.blackboard.is_some() {
            return Err(Error::Other(
                "workflow step agents don't run in box sandboxes yet".into(),
            ));
        }
        let host = match ctx.adopted_workspace() {
            None => None,
            Some(tree) => Some(box_holding(tree).ok_or_else(|| {
                Error::SandboxUnavailable(format!(
                    "{} is in no box; an agent in a box can only adopt a tree another box holds",
                    tree.display()
                ))
            })?),
        };
        let identity = match &host {
            Some(host) if host.agent_id != ctx.agent_id => host.clone(),
            _ => identity_of(ctx),
        };
        let is_tenant = identity.agent_id != ctx.agent_id;
        let box_agent_id = identity.agent_id.clone();
        let registry = BoxRegistry::global();
        let launch_id = uuid::Uuid::new_v4().to_string();
        // Held on the box it runs in before reaching it, so the idle stop
        // cannot take the VM between handing it out and the session (a
        // tenant's session keeps its host's VM up too). Every failure below
        // drops the hold; after that the plan's kill releases it, which
        // `KillGuard` runs should the session never start.
        // Registered first: a first launch's box is not known yet, and a hold
        // on an unknown box holds nothing.
        registry.register(identity.clone());
        let hold = Hold::take(&box_agent_id, &launch_id);
        // Waits for the image and the tools disk when they are still being
        // set up (`setup`), so a launch during setup is late, never refused.
        let mut running = block_on(registry.ensure_running(&identity))?;
        if running.identity.shares != identity.shares {
            // The user granted or revoked a folder since this box booted (or a
            // read booted it before the grant was stored): virtio-fs devices
            // are fixed at boot, so a restart is what mounts the new set.
            block_on(registry.stop(&box_agent_id, StopReason::Reshare))?;
            running = block_on(registry.ensure_running(&identity))?;
        }
        if running.tools_disk_for(ctx.provider).is_none() {
            // Booted before this provider's disk existed (an older build, a
            // host CLI upgrade since, or a tenant of a provider the box had no
            // disk for): a restart attaches the disk, once it is built — but
            // never under another launch's session (`reattach_with_tools`).
            running = block_on(registry.reattach_with_tools(&identity, ctx.provider, &launch_id))?;
            if running.tools_disk_for(ctx.provider).is_none() {
                let label =
                    crate::agent::provider_bin_label(ctx.provider).map_or(ctx.provider, |(_, l)| l);
                return Err(Error::SandboxUnavailable(format!(
                    "the box restarted without the {label} tools disk"
                )));
            }
        }
        // The project's toolchain cache in the guest (made below), which a
        // confined launch sees as a copy of its own like the rest of what it
        // shares with the coder.
        let cache = ToolchainCache::in_guest(ctx.home, ctx.project_id)?;
        let view = confined_view(ctx, is_tenant, &identity, &cache.root);
        if is_tenant {
            // The tenant's writable root is routed to this box (its profile is
            // written there before the launch — `route_tenant` — but a launch
            // reached another way must not miss it), its mailbox exists in
            // the guest before its first request, and is relayed like the
            // owner's.
            let root = ctx.writable_root.to_path_buf();
            registry.register_tenant(ctx.agent_id, root, &box_agent_id, view.is_some());
            let rpc = ctx.rpc_dir.to_string_lossy();
            let (requests, responses) = (format!("{rpc}/requests"), format!("{rpc}/responses"));
            let made = executor::run(
                &running.channel,
                &["mkdir", "-p", "-m", "700", "--", &requests, &responses],
                None,
            )?;
            if !made.status.success() {
                return Err(Error::Other(format!(
                    "could not create the tenant's rpc mailbox in the box: {}",
                    String::from_utf8_lossy(&made.stderr).trim()
                )));
            }
            running.relay_mailbox(ctx.rpc_dir, ctx.agent_id);
            // Its sessions, tailed into its own mirror, as the owner's are
            // into the owner's (a pulled provider's are pulled per agent):
            // for a confined launch from its private copy, the only place
            // it writes them, which also leaves the coder's out.
            if provider_home.transport() == provider::Transport::Tail {
                if let Some(mirror) = home::mirror_for_agent(ctx.provider, ctx.agent_id) {
                    let shared = provider_home.sessions_dir(ctx.home, ctx.writable_root);
                    let sessions = view.as_ref().and_then(|view| view.private_path(&shared));
                    running.relay_tail(&sessions.unwrap_or(shared), &mirror);
                }
            }
        }
        // What a reviewer's view mounts exists before the guest is asked to
        // set it up, and the guest has shown it can.
        let seed = provider_home.seed(ctx.home, ctx.writable_root)?;
        if let Some(view) = &view {
            make_carve_outs(&running.channel, view, &seed)?;
        }
        let view = enforced_view(view, |view| guest_confines(&running, view))?;
        // What the host writes in the guest for the launch from here on,
        // its seed and its credential files, it writes where the launch
        // runs: in a confined launch's view, where the launch reads it and
        // where nothing of it reaches what the launch shares with the coder.
        let scope = view.as_ref().map(ConfinedView::scope).unwrap_or_default();
        // The provider's home in the guest, as its adapter lays it out:
        // before every launch, so the user's config changes reach it.
        home::seed(&running.channel, &scope, &seed)?;
        // The project's toolchain cache, made in the guest before the agent
        // runs, where the agent runs. A guest that cannot make it (or a view
        // that leaves it read-only) keeps the toolchains' defaults, which in
        // a box are its own disk anyway.
        let made = scope.run(
            &running.channel,
            &["mkdir", "-p", "--", &cache.root.to_string_lossy()],
            None,
        );
        let cache_env = match made {
            Ok(out) if out.status.success() => cache.env,
            failed => {
                let why = failed.map_or_else(
                    |e| e.to_string(),
                    |out| String::from_utf8_lossy(&out.stderr).trim().to_string(),
                );
                tracing::warn!(
                    root = %cache.root.display(),
                    error = %why,
                    "could not create the toolchain cache in the box; toolchains keep their defaults"
                );
                Vec::new()
            }
        };
        // The claude on the tools disk this box booted with — which may be an
        // older disk taken while the host's version builds — is the one this
        // launch runs, so it is the one asked.
        let claude_thinking_summaries = match running.tools_disk_for(ctx.provider) {
            Some(disk) if ctx.provider == "claude" => {
                guest_thinking_summaries(&running.channel, disk, agent_bin)
            }
            _ => false,
        };
        let env_file = launch_files::env_file(&launch_id);
        let command = agent_remote_command(ctx.cwd, &env_file, view.as_ref(), agent_bin);

        // The login as the host resolved it for this launch, the way the
        // provider's adapter spells it: in the env file, and/or files kept
        // in step between host and guest for as long as the launch lives
        // (`home::CredentialSync`).
        let provider::Credential { mut env, files } = provider_home.credential(ctx)?;
        env.push(("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()));
        // A read-only tree has nowhere for git's opportunistic index refresh
        // to write; as under seatbelt, git is told not to try.
        if view.is_some() {
            env.push(("GIT_OPTIONAL_LOCKS".to_string(), "0".to_string()));
        }
        env.extend(cache_env);
        let credentials = Arc::new(Mutex::new(home::CredentialSync::new(files, scope)));
        let synced = !credentials
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_empty();
        let before_turn = synced
            .then(|| credential_step(&box_agent_id, &credentials, home::CredentialSync::turn));
        let after_exit = synced.then(|| {
            credential_step(
                &box_agent_id,
                &credentials,
                home::CredentialSync::after_exit,
            )
        });

        // Each process's arguments, written into the guest by the host before
        // the process starts, under the launch's lock, never by its launcher
        // (`launch_files`): the launcher is ssh running a command that names
        // the file.
        let files = Arc::new(launch_files::LaunchFiles::new(&launch_id));
        let stage_args: crate::sandbox::StageArgs = {
            let (files, box_agent_id) = (files.clone(), box_agent_id.clone());
            let (ssh_config, interactive) = (running.channel.config.clone(), ctx.interactive);
            Arc::new(move |args: &[String]| {
                let channel = BoxRegistry::global().channel_for(&box_agent_id)?;
                let args_file = files.stage(args, |path, bytes| {
                    executor::put(&channel, bytes, path, 0o600)
                })?;
                Ok(agent_launcher(
                    &ssh_config,
                    interactive,
                    &args_file,
                    &command,
                ))
            })
        };

        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(
                launch_id.clone(),
                PendingLaunch {
                    agent_id: ctx.agent_id.to_string(),
                    box_agent_id,
                    provider: ctx.provider.to_string(),
                    home: ctx.home.to_path_buf(),
                    credentials,
                },
            );
        self.files
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(launch_id.clone(), files);
        Ok(LaunchPlan {
            program: PathBuf::from(SSH),
            claude_thinking_summaries,
            launcher_args: crate::sandbox::LauncherArgs::Staged(stage_args),
            env,
            env_remove: ENV_DENY.iter().map(|v| v.to_string()).collect(),
            env_delivery: EnvDelivery::GuestFile { launch_id },
            before_turn,
            after_exit,
            kill: hold.into_kill(),
        })
    }

    /// Write the launch's env file into the guest (`/data/run/<launch>.env`,
    /// 0600) and return its path. This file is the only way host state reaches
    /// the agent's process, bar the arguments staged beside it for each of its
    /// processes (`/data/run/<launch>.<n>.args`): sshd accepts no client
    /// environment. Written under the launch's lock, as they are
    /// ([`launch_files`]).
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
        let files = self
            .files
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(launch_id)
            .cloned()
            .ok_or_else(|| Error::Other(format!("box launch {launch_id} was torn down")))?;
        let channel = BoxRegistry::global().channel_for(&pending.box_agent_id)?;
        let path = files.write_env(contents.as_bytes(), |path, bytes| {
            executor::put(&channel, bytes, path, 0o600)
        })?;
        pending
            .credentials
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .launch(&channel)?;
        Ok(path)
    }

    /// The session's own kill ends the ssh client (and so the guest PTY); the
    /// engine detaches the launch so the idle timer may stop the VM, and
    /// removes the env file the launch's token sat in, with any args file no
    /// `fletch-guest exec` took (its ssh failed, or its turn was superseded in
    /// between). The removal waits for a write under way and refuses every
    /// later one ([`launch_files::LaunchFiles::close`]), so it holds whatever
    /// the session's processes are doing: none of them writes those files.
    /// The CLI's credential changes were written back as each process exited
    /// (`after_exit`), never here, where the process may still be running.
    /// A Run launch's port forward ends here too ([`run::GuestRun`]).
    fn kill(&self, plan: &KillPlan) -> Result<()> {
        let KillPlan::Box {
            agent_id,
            launch_id,
        } = plan;
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(launch_id);
        // Dropped: ssh is killed, and the host port is free again.
        drop(
            self.forwards
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(launch_id),
        );
        let files = self
            .files
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(launch_id)
            .unwrap_or_else(|| Arc::new(launch_files::LaunchFiles::new(launch_id)));
        let registry = BoxRegistry::global();
        let channel = registry.channel_for_running(agent_id).ok();
        // A Run's command is ended in the guest first, by name: a stop must
        // not depend on what its process tree makes of the tty hangup that
        // follows, and a stopped box has nothing left to end.
        let run = self
            .runs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(launch_id);
        if let (Some(stores), Some(channel)) = (run, &channel) {
            run::end_in_guest(channel, launch_id, Some(&stores));
        }
        files.close(|stem| {
            if let Some(channel) = &channel {
                let stem = stem.to_string_lossy();
                let words = ["sh", "-c", launch_files::REMOVE, "sh", &stem];
                let _ = executor::run(channel, &words, None);
            }
        });
        registry.detach(agent_id, launch_id);
        Ok(())
    }
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
    let guest = guest_env(provider, home);
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

/// What every process in a guest needs that the host never sets: `HOME` by
/// path identity, the provider's tools disk on `PATH`, the loopback egress
/// proxy, a shell and a locale. The env file of an agent launch ends with
/// these; a shell or a tool run in the guest for the user starts from them.
pub fn guest_env(provider: &str, home: &Path) -> Vec<(String, String)> {
    let term = std::env::var("TERM")
        .ok()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "xterm-256color".to_string());
    vec![
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
        // Node's own `fetch` takes the proxy from the env only when told to;
        // a box has no other way out (no DNS in the guest), and corepack —
        // the image's pnpm and yarn — downloads through nothing else.
        ("NODE_USE_ENV_PROXY".into(), "1".into()),
        // Corepack's shims ask on a tty before they download the version a
        // project pins; nothing in a box answers a prompt (the Run panel's
        // tty least of all), and the image put the shims there to download.
        ("COREPACK_ENABLE_DOWNLOAD_PROMPT".into(), "0".into()),
    ]
}

/// A program to run in the guest that holds `tree`, for the user rather than
/// an agent — the workspace terminal, a `claude doctor` — as the `(program,
/// args)` a host PTY or process spawns: `bash -c <wrapper> … ssh -F <cfg> -tt
/// box "cd <tree> && exec env <guest env> <program> <args>"`. `None` when
/// `tree` is a host path. Boots a stopped box, since the point is to look
/// inside it.
pub fn guest_program(
    tree: &Path,
    program: &str,
    args: &[String],
) -> Result<Option<(PathBuf, Vec<String>)>> {
    let Some(identity) = box_holding(tree) else {
        return Ok(None);
    };
    let channel = BoxRegistry::global().channel_for(&identity.agent_id)?;
    let quote = fletch_box::channel::shell_quote;
    let mut remote = vec![
        "cd".to_string(),
        quote(&tree.to_string_lossy()),
        "&&".to_string(),
        "exec".to_string(),
        "env".to_string(),
    ];
    remote.extend(
        guest_env(&identity.provider, &identity.home)
            .iter()
            .map(|(k, v)| quote(&format!("{k}={v}"))),
    );
    remote.push(quote(program));
    remote.extend(args.iter().map(|a| quote(a)));
    let wrapper = executor::quoting_wrapper(
        SSH,
        &[
            "-F".to_string(),
            channel.config.to_string_lossy().into_owned(),
            // The host side is a PTY or a pipe; a tty in the guest only
            // when the client has one, which is what the shell wants and
            // a captured command does not get.
            "-t".to_string(),
            fletch_box::guest::SSH_HOST_ALIAS.to_string(),
        ],
        &remote,
    );
    Ok(Some((PathBuf::from(BASH), wrapper)))
}

/// A session's claim on its box against the idle stop
/// ([`BoxRegistry::attach`]), taken *before* the session reaches the box: the
/// idle stop spares only a box attached when it takes the VM, so a session
/// that held first is never handed a VM being stopped idle. Dropped — any
/// failure on the way to the session — it detaches; [`Hold::into_kill`] hands
/// that to the session's kill instead ([`BoxEngine::kill`] detaches), which
/// must then run whether or not the session starts (`sandbox::KillGuard`).
#[must_use]
pub struct Hold {
    agent_id: String,
    launch_id: String,
    handed_off: bool,
}

impl Hold {
    /// Attach `launch_id` to `agent_id`'s box, which must be registered.
    pub(super) fn take(agent_id: &str, launch_id: &str) -> Hold {
        BoxRegistry::global().attach(agent_id, launch_id);
        Hold {
            agent_id: agent_id.to_string(),
            launch_id: launch_id.to_string(),
            handed_off: false,
        }
    }

    /// The kill handle of the session this hold was for, which releases it.
    pub fn into_kill(mut self) -> KillHandle {
        self.handed_off = true;
        KillHandle::Engine {
            engine: BoxEngine::shared(),
            plan: KillPlan::Box {
                agent_id: std::mem::take(&mut self.agent_id),
                launch_id: std::mem::take(&mut self.launch_id),
            },
        }
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        if !self.handed_off {
            BoxRegistry::global().detach(&self.agent_id, &self.launch_id);
        }
    }
}

/// Hold the box that holds `tree` for a session the user opens in it (the
/// workspace terminal), as an agent's launch is held. Take it before
/// [`guest_program`] reaches the box. `None` for a host tree.
pub fn hold_session(tree: &Path) -> Option<Hold> {
    let identity = box_holding(tree)?;
    Some(Hold::take(
        &identity.agent_id,
        &uuid::Uuid::new_v4().to_string(),
    ))
}

/// Create `guest_path` with `bytes` in the guest that holds `tree`, failing
/// when something is already there — the twin of a host `create_new` write,
/// for a file the host materializes for an agent outside the routed roots
/// (a session the provider's CLI will resume, `home::create_session_file`).
pub fn write_new_guest_file(tree: &Path, guest_path: &Path, bytes: &[u8]) -> Result<()> {
    let identity = box_holding(tree)
        .ok_or_else(|| Error::Other(format!("{} is in no box", tree.display())))?;
    let channel = BoxRegistry::global().channel_for(&identity.agent_id)?;
    // `set -C` (noclobber) makes `>` refuse an existing file.
    let script =
        "umask 077 && mkdir -p \"$(dirname \"$1\")\" && set -C && cat > \"$1\" && chmod 644 \"$1\"";
    let out = executor::run(
        &channel,
        &["sh", "-c", script, "sh", &guest_path.to_string_lossy()],
        Some(bytes),
    )?;
    if !out.status.success() {
        return Err(Error::Other(format!(
            "could not create {} in the box: {}",
            guest_path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
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
/// is reclaimed at the next start ([`startup`]). A tenant has no box to
/// delete: it stops being routed to its host's.
pub async fn discard_if_any(agent_id: &str) -> Result<()> {
    let registry = BoxRegistry::global();
    if let Some(tenant) = registry.tenant_of(agent_id) {
        // What the host wrote for its launches sits in the host's guest; it
        // is removed while that box is up, and otherwise left — a stale
        // profile dir there is harmless and the next tenant launch rewrites
        // it — rather than booting a VM to delete a directory.
        if let Ok(channel) = registry.channel_for_running(&tenant.host_agent_id) {
            let root = tenant.writable_root.to_string_lossy().into_owned();
            if let Err(e) = executor::run(&channel, &["rm", "-rf", "--", &root], None) {
                tracing::info!(agent_id, error = %e, "tenant's guest dir not removed");
            }
        }
        registry.forget_tenant(agent_id);
        return Ok(());
    }
    let dir = box_dir(agent_id)?;
    if !dir.exists() && !registry.knows(agent_id) {
        return Ok(());
    }
    let outcome = registry.discard(agent_id).await.inspect_err(|e| {
        tracing::warn!(agent_id, error = %e, "box discard failed");
    });
    // Whatever came of it: the workspace is on its way out either way.
    list_changed();
    outcome
}

/// A turn of `agent_id` ended, however it ended: a network opened for it
/// (`box_egress_open_turn`) closes. A no-op for every other agent.
pub fn turn_ended(agent_id: &str) {
    BoxRegistry::global().set_turn_open(agent_id, false);
}

/// Tell clients the set of VMs changed ([`EVENT_LIST`]).
pub(crate) fn list_changed() {
    emit(EVENT_LIST, &serde_json::json!({}));
}

/// Stop every running box cleanly; app exit (`Supervisor::shutdown`).
pub fn shutdown_all() {
    if BoxRegistry::global().running_ids().is_empty() {
        return;
    }
    block_on(BoxRegistry::global().shutdown_all());
}

/// App exit, before `Supervisor::shutdown`: no box boots from here on
/// (`BoxRegistry::close`), so that shutdown stops every box there will be.
pub fn close() {
    BoxRegistry::global().close();
}

/// At boot: route every live box-stamped agent's writable root to its box
/// (so a Git panel read before the first launch boots it lazily), route every
/// tenant to its host's box, log boxes whose last VM did not stop cleanly,
/// and kill `fletch-vm` processes whose owning Fletch is dead. Off the boot
/// thread for the process scan.
pub fn startup(workspace: &crate::workspace::WorkspaceManager) {
    if let Ok(ids) = workspace.live_agent_ids() {
        let records: Vec<_> = ids
            .iter()
            .filter_map(|id| workspace.agent(id).ok())
            .filter(|r| r.sandbox_engine.as_deref() == Some(EngineKind::Box.as_setting()))
            .collect();
        // Owners first: a tenant resolves its host through the registry,
        // so every box must be known before any tenant is placed. An
        // adopting record is a tenant by construction — a box-stamped
        // agent never adopts a tree outside a box (`engine_for_adopted_tree`).
        let (tenants, owners): (Vec<_>, Vec<_>) = records
            .into_iter()
            .partition(|r| r.repos.first().is_some_and(|repo| repo.is_adopted()));
        for record in owners {
            match BoxIdentity::for_record(&record) {
                Ok(identity) => BoxRegistry::global().register(identity),
                Err(e) => {
                    tracing::warn!(agent_id = %record.id, error = %e, "box identity failed")
                }
            }
        }
        for record in tenants {
            if let Err(e) = route_tenant(&record) {
                tracing::warn!(agent_id = %record.id, error = %e, "box tenant not routed");
            }
        }
    }
    // The CLI version a tools disk is keyed by is probed once per process,
    // so a CLI update since the last run shows here first: start its disk
    // now rather than at the first boot that misses it. Skips a settled disk.
    setup::prebuild_tools();
    // And the disks an earlier update superseded go, and the one the next
    // boot takes is hashed against its marker, off this thread: a launch
    // checks only the marker's size.
    for provider in fletch_box::tools::PROVIDERS {
        if supports_provider(provider) {
            registry::sweep_tools_disks_in_background(provider.to_string());
            registry::verify_tools_disks_in_background(provider.to_string());
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

/// Every agent's box directory, by agent id: a box exists once it has a data
/// disk. That survives a restart, which the registry's entries do not, and a
/// tenant has none of its own. The root also holds a tools build's throwaway
/// `tools-…` box, which is no agent's.
fn boxes_on_disk() -> Vec<(String, PathBuf)> {
    boxes_root().map(|root| boxes_in(&root)).unwrap_or_default()
}

/// [`boxes_on_disk`] with the root given.
fn boxes_in(root: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let path = entry.path();
            (!name.starts_with("tools-") && path.join(registry::DATA_DISK).is_file())
                .then_some((name, path))
        })
        .collect()
}

/// The agents that have a box, running or not, hidden workspaces included.
pub fn created_box_ids() -> Vec<String> {
    boxes_on_disk().into_iter().map(|(id, _)| id).collect()
}

/// Box directories no live agent owns: left by a discard that failed, or by
/// an archive the app died in the middle of (`begin_archive` has already
/// marked the row). Each holds a whole data disk, so nothing else would ever
/// reclaim it. The directories are listed before the agents are read, so a
/// box made in between belongs to a row that is already live.
fn leaked_box_dirs(workspace: &crate::workspace::WorkspaceManager) -> Vec<PathBuf> {
    let boxes = boxes_on_disk();
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
