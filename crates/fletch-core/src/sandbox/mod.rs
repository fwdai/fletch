pub mod boxes;
pub mod cargo_home;
pub(crate) mod container;
pub mod docker;
mod engine;
pub mod guarantees;
pub mod placement;
pub mod podman;
pub mod policy;
pub mod provider;
pub mod provision;
mod seatbelt;
pub mod toolchain_cache;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::RwLock;

use crate::error::{Error, Result};
use crate::workspace::AgentRecord;

// The name in an agent's writable root where a confined box tenant keeps its
// private copies of what it shares with its host: Fletch's own, which no
// checkout may take (`workspace::allocate_repo_subdir`).
pub(crate) use boxes::COW_DIR;
pub use docker::{availability as docker_availability, DockerAvailability};
pub use engine::{
    AgentLaunchCtx, EngineCaps, EngineKind, EnvDelivery, KillGuard, KillHandle, LaunchPlan,
    SandboxEngine, TurnPrep,
};
pub use guarantees::{describe as describe_isolation, IsolationReport};
pub use podman::{
    availability as podman_availability, launch_blocker as podman_launch_blocker,
    PodmanAvailability,
};
pub(crate) use seatbelt::pid_alive;
pub use seatbelt::{
    build_run_profile, cleanup_nested_checkouts_roots, cleanup_nested_rpc_roots,
    nested_checkouts_root, nested_rpc_root, profile_args, SANDBOX_EXEC,
};
pub use toolchain_cache::{
    prepare_toolchain_cache, reclaim_toolchain_caches, remove_project_cache, toolchain_cache_dir,
    toolchain_cache_root, BoxPlatform, ToolchainCache, TOOLCHAIN_CACHE_ROOT_ENV,
};

/// The `settings` key holding the user's engine choice; values are
/// [`EngineKind::as_setting`] spellings or [`ENGINE_AUTO`], the default.
pub const ENGINE_SETTING: &str = "sandbox_engine";

/// The `sandbox_engine` value that leaves the choice to Fletch: a box when
/// one can boot right now, the macOS sandbox otherwise
/// (`boxes::engine_for_new_agent`). What an absent row means too.
pub const ENGINE_AUTO: &str = "auto";

/// The user's engine selection, mirrored from the `sandbox_engine` setting so
/// spawn paths (deep in agent code, no DB handle) can read it: `None` is
/// automatic. Seeded at startup (`host::boot`) and updated by the
/// `set_sandbox_engine` command. Teardown never consults this — sessions
/// carry a `KillHandle` bound to the engine that launched them.
static SELECTED_ENGINE: RwLock<Option<EngineKind>> = RwLock::new(None);

pub fn set_selected_engine(choice: Option<EngineKind>) {
    *SELECTED_ENGINE.write() = choice;
}

/// The engine the user pinned for *new* agents, or `None` for automatic.
/// Existing agents keep the kind stamped on their record at creation (see
/// `supervisor::lifecycle::spawn_agent`), so a settings change never
/// re-engines them.
pub fn selected_engine() -> Option<EngineKind> {
    *SELECTED_ENGINE.read()
}

/// The stored spelling of the current choice.
pub fn selected_engine_setting() -> &'static str {
    selected_engine().map_or(ENGINE_AUTO, EngineKind::as_setting)
}

/// Parse a `sandbox_engine` value: `Some(None)` for [`ENGINE_AUTO`],
/// `Some(Some(kind))` for a pinned engine, `None` for anything else.
pub fn parse_engine_setting(value: &str) -> Option<Option<EngineKind>> {
    if value == ENGINE_AUTO {
        Some(None)
    } else {
        EngineKind::from_setting(value).map(Some)
    }
}

/// The engine an agent that *adopts* `tree` — works in an existing checkout
/// instead of provisioning one (the local reviewer in a coder's checkout) —
/// is stamped with: **where the tree lives**, not what Settings selects. A
/// tree inside a box is reachable only from that box, so the agent runs there
/// as its tenant whatever the host's setting; a host tree is invisible to a
/// guest, so automatic and a pinned `box` both land on seatbelt, as they do
/// for a provider boxes cannot run. The caller still gates the provider
/// against the result.
pub fn engine_for_adopted_tree(tree: &Path) -> EngineKind {
    if boxes::box_holding(tree).is_some() {
        return EngineKind::Box;
    }
    match selected_engine() {
        Some(EngineKind::Box) | None => EngineKind::SandboxExec,
        Some(kind) => kind,
    }
}

/// This engine's own data directory ([`crate::host::BootConfig::data_dir`]),
/// mirrored process-wide so the macOS profile builder — reached on the spawn
/// path, with no handle on the boot config — can carve it out of what a confined
/// agent may read or write. Same mirror idiom as [`set_selected_engine`].
///
/// It cannot be derived. [`crate::data_dir`] is the *desktop's* directory, keyed
/// by [`crate::BUNDLE_ID`]; a `fletch-host` keeps its database (the GitHub token
/// in plaintext, since a service has no unlocked keychain), its Noise
/// `remote/host_key` and its `remote/devices.json` under
/// `~/Library/Application Support/fletch-host` or wherever `--data-dir` points.
///
/// `None` until a host publishes one — which is what every unit test and every
/// caller that is not [`crate::host::boot`] sees, and the profile then carries
/// the bundle-id deny alone, exactly as it always did.
static DATA_DIR: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Publish the data dir this engine was configured with. Called once from
/// [`crate::host::boot`], before anything can spawn an agent.
pub fn set_data_dir(dir: PathBuf) {
    *DATA_DIR.write() = Some(dir);
}

/// The data dir [`set_data_dir`] published, if a host published one.
pub(crate) fn configured_data_dir() -> Option<PathBuf> {
    DATA_DIR.read().clone()
}

/// Resolve the engine for an agent stamped with `kind`, availability-checked
/// at spawn time. A docker-stamped agent whose daemon is unreachable is a hard
/// error: silently falling back to seatbelt would launch the process *outside*
/// the container boundary the user selected, while the record and UI keep
/// showing Docker — an isolation downgrade the caller never sees. Fail closed
/// instead so the spawn surfaces the unavailability rather than degrading.
pub fn engine_for(kind: EngineKind) -> Result<Arc<dyn SandboxEngine>> {
    match kind {
        EngineKind::SandboxExec => Ok(seatbelt_engine()),
        // Behind the `boxes_enabled` switch, then the runtime probe and the
        // image: `boxes::availability` refuses with the first thing missing,
        // so a box-stamped agent never launches elsewhere.
        EngineKind::Box => boxes::availability().map(|_| boxes::BoxEngine::shared() as Arc<_>),
        EngineKind::Docker => match docker::availability() {
            DockerAvailability::Available { .. } => Ok(docker::DockerEngine::shared()),
            status => {
                tracing::warn!(
                    ?status,
                    "docker engine selected but unavailable; refusing to launch outside the container boundary"
                );
                Err(Error::SandboxUnavailable(format!(
                    "Docker sandbox is selected but unavailable ({status:?}); \
                     start Docker or switch the sandbox engine before launching."
                )))
            }
        },
        EngineKind::Podman => match podman::availability() {
            PodmanAvailability::Available { .. } => Ok(podman::PodmanEngine::shared()),
            status => {
                tracing::warn!(
                    ?status,
                    "podman engine selected but unavailable; refusing to launch outside the container boundary"
                );
                let remedy = match status {
                    PodmanAvailability::MachineDown => "run `podman machine start`",
                    _ => "install Podman",
                };
                Err(Error::SandboxUnavailable(format!(
                    "Podman sandbox is selected but unavailable ({status:?}); \
                     {remedy} or switch the sandbox engine before launching."
                )))
            }
        },
    }
}

/// The `projects` dir `kind` gives claude in place of the shared
/// `~/.claude/projects`, for the agent whose writable root is `writable_root`;
/// `None` when the engine lets claude write the shared one (seatbelt). The
/// container engines bind this dir over the image's config dir
/// (`container::launch`); the box engine mirrors the guest's projects dir into
/// `<box dir>/claude-projects` over its transcript relay. The transcript
/// writer (`transcripts::claude_projects_dir`) and reader (`transcripts::
/// find_session_jsonl`) consult this, so neither names an engine.
pub(crate) fn transcript_root(kind: EngineKind, writable_root: &Path) -> Option<PathBuf> {
    match kind {
        EngineKind::Docker | EngineKind::Podman => {
            Some(writable_root.join(crate::transcripts::DOCKER_CLAUDE_PROJECTS_DIRNAME))
        }
        // The box dir is a sibling of the writable root, named by the same
        // agent id (its last component).
        EngineKind::Box => boxes::transcript_dir_for_root(writable_root),
        EngineKind::SandboxExec => None,
    }
}

/// Every per-agent transcript root some engine keeps under `writable_root`
/// ([`transcript_root`]), for a reader that does not know the agent's
/// engine. Deduped: the container engines share one.
pub(crate) fn all_transcript_roots(writable_root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for &kind in EngineKind::ALL {
        if let Some(dir) = transcript_root(kind, writable_root) {
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
    }
    dirs
}

/// Where the host reads `provider`'s sessions for `agent_id` when they live
/// somewhere other than the host's own dirs: the mirror under a box agent's
/// box dir, refreshed first from a running box for a provider whose relay
/// pulls rather than tails. For every other engine's agent the directory is
/// absent (or `None`), so a reader falls through to the provider's own dirs.
pub(crate) fn session_mirror(provider: &str, agent_id: &str) -> Option<PathBuf> {
    boxes::home::sessions_mirror(provider, agent_id)
}

/// Create the `provider` session file `rel` (relative to its sessions dir)
/// holding `lines`, for the agent `agent_id` running in `cwd` under `engine`,
/// when the engine keeps the agent's sessions itself: a box writes it in the
/// guest, where the CLI that resumes it reads, and in the host's mirror, and
/// answers the mirror's path. `None` when the provider writes it on the host
/// as usual.
pub(crate) fn create_session_file(
    provider: &str,
    engine: EngineKind,
    agent_id: &str,
    cwd: &Path,
    rel: &Path,
    lines: &[serde_json::Value],
) -> Result<Option<PathBuf>> {
    match engine {
        EngineKind::Box => {
            let home = self::provider::require(provider)?;
            boxes::home::create_session_file(home, agent_id, cwd, rel, lines).map(Some)
        }
        EngineKind::SandboxExec | EngineKind::Docker | EngineKind::Podman => Ok(None),
    }
}

/// The engine stamped on an agent's record at creation. Records from before
/// engine selection existed (NULL) always ran under sandbox-exec, so that
/// stays their permanent kind — never re-derived from the live setting.
pub fn stamped_engine(record: &AgentRecord) -> EngineKind {
    record
        .sandbox_engine
        .as_deref()
        .and_then(EngineKind::from_setting)
        .unwrap_or(EngineKind::SandboxExec)
}

/// A spawn's per-session engine override (`SpawnRequest::workspace_type`,
/// docs/sandbox-foundation.md rule 8), resolved by [`engine_for_spawn`].
///
/// `Host` and `Box` are the user-facing choice the composer offers
/// ([`workspace_types`]), read off the wire by [`WorkspaceType::parse`].
/// `Stamped` is the exact engine an existing record was stamped with, built
/// only here ([`WorkspaceType::stamped`]): a fork passes its parent's, so the
/// child runs under the parent's engine, whichever it is, even where the
/// automatic choice would now pick another. It is not the project's
/// placement setting (`placement::Placement`), which only feeds the choice
/// made when there is no override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceType {
    /// The macOS sandbox.
    Host,
    /// A box, or a refusal with the reason when there can be none.
    Box,
    /// The engine an existing record was stamped with.
    Stamped(EngineKind),
}

impl WorkspaceType {
    /// The composer's spelling: `host` or `box`. Anything else, or none, is
    /// no override.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "host" => Some(Self::Host),
            "box" => Some(Self::Box),
            _ => None,
        }
    }

    /// The engine `record` was stamped with, as an override: what a fork
    /// passes for its parent.
    pub fn stamped(record: &AgentRecord) -> Self {
        Self::Stamped(stamped_engine(record))
    }
}

/// Whether `provider` can launch under `engine`: the one support gate,
/// [`provider::supports`], as the spawn path's error. Checked before every
/// spawn — fresh spawns and resume/view-switch — so a record stamped with an
/// engine can never launch a provider that engine cannot run.
pub fn ensure_supports_provider(engine: EngineKind, provider: &str) -> Result<()> {
    self::provider::supports(engine, provider).map_err(Error::Other)
}

/// Whether the container engines run `provider`: it has a wired-up container
/// image, config mount and auth (`container::ContainerProvider`) — claude,
/// codex, opencode, pi and cursor so far. The container half of
/// [`provider::supports`], kept here with the engine registry so the provider
/// adapter names no container module.
pub(crate) fn runs_in_container(provider: &str) -> bool {
    container::ContainerProvider::from_id(provider).is_some()
}

/// The engine a new agent is stamped with (docs/sandbox-foundation.md rule
/// 8). An agent adopting `adopted_tree` runs where the tree lives
/// ([`engine_for_adopted_tree`]) and ignores `override_`: a tree in a box is
/// reachable only there, a host tree never from a guest. Anyone else gets
/// the session's own choice when there is one, else the host's choice over
/// the placement of the project at `repo`, whose stored setting is
/// `placement_setting`.
///
/// The override ([`WorkspaceType`]) stands over both the host's choice and
/// the project's placement for this spawn only: `Host` is the macOS sandbox;
/// `Box` is a box, waiting at launch for an image still on its way as a
/// project pinned to boxes does, and refused with the reason the user sees
/// (`box_refusal`: the switch off, a runtime that cannot boot, a provider
/// boxes do not run) when there can be no box at all — an explicit choice
/// never quietly lands somewhere else. `Stamped(kind)` is exactly `kind`: a
/// stamped box is checked as an explicit `Box` is, and a stamped container
/// engine is returned as it is (its launch fails closed when the daemon is
/// down, [`engine_for`]). Without an override this never errs.
///
/// Blocking: the automatic choice stats the repository and may run the
/// runtime probe once, so a spawn calls this off the async runtime.
pub fn engine_for_spawn(
    adopted_tree: Option<&Path>,
    placement_setting: Option<&str>,
    repo: &Path,
    provider: &str,
    override_: Option<WorkspaceType>,
) -> Result<EngineKind> {
    if let Some(tree) = adopted_tree {
        return Ok(engine_for_adopted_tree(tree));
    }
    match override_ {
        Some(WorkspaceType::Host | WorkspaceType::Stamped(EngineKind::SandboxExec)) => {
            Ok(EngineKind::SandboxExec)
        }
        Some(WorkspaceType::Box | WorkspaceType::Stamped(EngineKind::Box)) => {
            match box_refusal(provider) {
                Some(reason) => Err(Error::Other(reason)),
                None => Ok(EngineKind::Box),
            }
        }
        Some(WorkspaceType::Stamped(kind @ (EngineKind::Docker | EngineKind::Podman))) => Ok(kind),
        None => {
            let placement = placement::resolve(placement_setting, repo);
            Ok(boxes::engine_for_new_agent(
                selected_engine(),
                provider,
                placement,
            ))
        }
    }
}

/// Why a new agent of `provider` cannot run in a box, in the sentence the
/// user sees, or `None` when it can (an image still downloading is waited
/// for, not refused): the switch, then the machine and runtime, then the
/// provider gate.
fn box_refusal(provider: &str) -> Option<String> {
    if !boxes::settings::enabled() {
        return Some(boxes::SWITCHED_OFF.to_string());
    }
    if let Some(refusal) = boxes::runtime_refusal() {
        return Some(refusal);
    }
    self::provider::supports(EngineKind::Box, provider).err()
}

/// One workspace type a new agent may be given, as the composer offers it
/// ([`workspace_types`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WorkspaceTypeOption {
    /// `host` (the macOS sandbox) or `box` (an isolated VM): the spelling
    /// [`WorkspaceType::parse`] reads a spawn's override from.
    pub kind: &'static str,
    /// Whether a spawn may ask for it.
    pub available: bool,
    /// Why not, when it is not: the sentence the spawn would fail with.
    pub reason: Option<String>,
    /// What [`engine_for_spawn`] picks with no override. Neither option is
    /// the default while the host pins a container engine.
    pub default: bool,
}

/// The workspace types a new agent of `provider` in the project at `repo`
/// (stored placement `placement_setting`) may be given: the one list the UI
/// reads. The macOS sandbox is always available; a box exactly when
/// [`engine_for_spawn`] would accept it as an override, with the same
/// sentence when not. Blocking, like [`engine_for_spawn`].
pub fn workspace_types(
    placement_setting: Option<&str>,
    repo: &Path,
    provider: &str,
) -> Vec<WorkspaceTypeOption> {
    let box_reason = box_refusal(provider);
    // What `engine_for_spawn` picks with a `None` override: the host's choice
    // over the project's placement. A box refused above is never ready, and
    // a passing runtime is remembered, so the runtime is probed at most once
    // for the whole answer.
    let default = boxes::engine_for_new_agent_with(
        selected_engine(),
        provider,
        placement::resolve(placement_setting, repo),
        || box_reason.is_none() && boxes::ready_for(provider),
    );
    vec![
        WorkspaceTypeOption {
            kind: "host",
            available: true,
            reason: None,
            default: default == EngineKind::SandboxExec,
        },
        WorkspaceTypeOption {
            kind: "box",
            available: box_reason.is_none(),
            reason: box_reason,
            default: default == EngineKind::Box,
        },
    ]
}

/// Provision `spec.dest` for `record`, an agent stamped `engine`, at the base
/// branch `parent_branch` as `origin` has it, falling back to `spec.base_ref`.
/// A box's checkout goes into its guest (`git init` there, the base pushed
/// over the channel, the same fixups), the box booting with the shares the
/// agent's launch will ask for; every other engine's is a host clone,
/// `--shared` when `shared`.
pub async fn provision_checkout(
    engine: EngineKind,
    record: &AgentRecord,
    spec: &provision::CheckoutSpec<'_>,
    parent_branch: &str,
    shared: bool,
) -> Result<provision::BaseFreshness> {
    match engine {
        EngineKind::Box => {
            let identity = boxes::BoxIdentity::for_record(record)?;
            boxes::provision::provision(
                &identity,
                spec,
                Some(parent_branch),
                boxes::provision::Landing::Detached,
            )
            .await
        }
        EngineKind::SandboxExec | EngineKind::Docker | EngineKind::Podman => {
            provision::provision_at_remote_base(spec, parent_branch, shared).await
        }
    }
}

/// Restore `record`'s checkout `spec.dest`, an agent stamped `engine`, at its
/// archived tip `spec.base_ref`. With `branch` = `(name, fetch_from)` it
/// lands on branch `name`, the tip refetched from `origin` under
/// `fetch_from` when it must be; without one it lands detached. When the tip
/// is gone for good it opens detached at `parent_branch_sha`. Returns whether
/// it landed on the branch. A box rebuilds the checkout inside its guest from
/// what the source repository holds; every other engine walks the host
/// clone's restore ladder.
pub async fn restore_checkout(
    engine: EngineKind,
    record: &AgentRecord,
    spec: &provision::CheckoutSpec<'_>,
    branch: Option<(&str, &str)>,
    parent_branch_sha: Option<&str>,
) -> Result<bool> {
    match engine {
        EngineKind::Box => {
            let identity = boxes::BoxIdentity::for_record(record)?;
            let name = branch.map(|(name, _)| name);
            boxes::provision::restore(&identity, spec, name, parent_branch_sha).await
        }
        EngineKind::SandboxExec | EngineKind::Docker | EngineKind::Podman => match branch {
            Some((name, fetch_from)) => {
                provision::provision_on_branch(spec, name, fetch_from, parent_branch_sha).await
            }
            None => provision::provision_recovering(spec, parent_branch_sha)
                .await
                .map(|()| false),
        },
    }
}

/// The engine's step before every launch of `record`, run before the launch
/// writes anything under the agent's writable root. A box-stamped agent that
/// adopted its tree runs in the box that holds it (tenants), so it is routed
/// there first and the profile lands in the guest it will run in.
pub fn before_launch(record: &AgentRecord) -> Result<()> {
    match stamped_engine(record) {
        EngineKind::Box => boxes::route_tenant(record).map(|_| ()),
        EngineKind::SandboxExec | EngineKind::Docker | EngineKind::Podman => Ok(()),
    }
}

/// The extra time `record`'s spawn gets for the setup it may wait on: a box
/// agent may first have to wait for the host's setup (the image download,
/// its tools disk build), far longer than any process start. Zero for every
/// other engine and once the setup is done.
pub fn setup_allowance(record: &AgentRecord) -> Duration {
    match stamped_engine(record) {
        EngineKind::Box => boxes::setup::setup_allowance(&record.provider),
        EngineKind::SandboxExec | EngineKind::Docker | EngineKind::Podman => Duration::ZERO,
    }
}

/// Discard what the engine keeps for `agent_id` outside its host dirs: a box
/// agent's VM and box directory, data disk and checkouts included. A
/// directory probe for every other agent, so callers run it unconditionally.
pub async fn discard_agent_state(agent_id: &str) -> Result<()> {
    boxes::discard_if_any(agent_id).await
}

/// Best-effort teardown of any container still bearing this agent's
/// `fletch.agent-id` label, for an agent stamped `engine` (`None` when its
/// record is gone). The container counterpart to the disposal's checkout
/// teardown, and like it, failures never abort disposal — the caller's intent
/// is to get rid of the agent; `op` tags the logs.
///
/// Why it exists: `Supervisor::detach_runtime` reaches the container only
/// *indirectly*, through the in-memory `agents` entry — no entry, no
/// `KillHandle`, no runtime command at all. `run --rm` covers the ordinary exit
/// and the startup pid-liveness sweep covers a crashed instance, but neither
/// makes disposal itself self-sufficient. The label sweep does: it needs nothing
/// but the agent id. (Container names can't serve here — they carry a
/// launch-time nonce, see `container::util::container_name` — which is why the
/// id label exists.)
///
/// Three gates, cheapest first:
/// 1. The stamped engine, which also picks the runtime: an agent is reaped by
///    the runtime that launched it, never by whichever is selected now. A
///    seatbelt agent never had a container, so it never pays for a probe.
///    `engine: None` (discard tolerates a missing row) can't prove which
///    runtime launched it, so it asks both — the label match is exact, so
///    looking costs nothing but the queries.
/// 2. The runtime's own availability probe — a machine without that runtime
///    installed must never see one of its invocations, the same precedent the
///    startup sweeps set.
/// 3. Off the caller's path entirely. `spawn_blocking` rather than a bare
///    thread: the CLI call is a blocking process wait (up to the probe's 2s
///    plus the sweep's own timeouts) issued from inside the tokio runtime,
///    which is precisely the blocking pool's job — and unlike the startup
///    sweep, which runs before any runtime is a given, we always have one
///    here. Detached on purpose: archive must not wait on a container
///    round-trip to report success to the user.
pub fn reap_agent_containers(agent_id: &str, engine: Option<EngineKind>, op: &'static str) {
    // Seatbelt runs no container; neither does a box, whose VM goes with
    // [`discard_agent_state`] in the checkout teardown instead.
    if matches!(engine, Some(EngineKind::SandboxExec | EngineKind::Box)) {
        return;
    }
    let agent_id = agent_id.to_string();
    tokio::task::spawn_blocking(move || {
        let report = |runtime: &'static str, removed: crate::error::Result<usize>| match removed {
            Ok(0) => {}
            Ok(n) => {
                tracing::info!(agent_id = %agent_id, op, runtime, removed = n, "removed agent containers")
            }
            Err(e) => {
                tracing::warn!(agent_id = %agent_id, op, runtime, error = %e, "agent container removal failed")
            }
        };
        // A stamped engine narrows to one runtime; a rowless agent asks both.
        if engine != Some(EngineKind::Podman)
            && matches!(
                docker::availability(),
                docker::DockerAvailability::Available { .. }
            )
        {
            report("docker", docker::remove_agent_containers(&agent_id));
        }
        if engine.map_or(true, |k| k == EngineKind::Podman)
            && matches!(
                podman::availability(),
                podman::PodmanAvailability::Available { .. }
            )
        {
            report("podman", podman::remove_agent_containers(&agent_id));
        }
    });
}

/// Stop every engine-held VM cleanly as the app quits, once the agents'
/// sessions are gone, so no box disk is left dirty.
pub fn shutdown_all() {
    boxes::shutdown_all();
}

/// The engine's step after a turn checkpoint of `agent_id` was captured: a
/// box agent's checkpoint lives in the guest, and the host-side mirror
/// follows it so publish has the objects whenever the box next stops.
pub async fn after_checkpoint(agent_id: &str) -> Result<()> {
    boxes::mirror::refresh_agent(agent_id).await
}

/// The repository host git runs transport (fetch, push) over for `checkout`
/// (docs/sandbox-foundation.md rule 4): `None` for the checkout itself, on
/// the host; for a checkout in a box, its host-side mirror, refreshed from the
/// guest now (booting a stopped box). `source_repo` is the user's repository
/// the mirror's alternates point at.
pub async fn transport_repo(checkout: &Path, source_repo: &Path) -> Result<Option<PathBuf>> {
    boxes::mirror::for_transport(checkout, source_repo).await
}

/// After transport moved the tracking refs in `repo`, the stand-in
/// [`transport_repo`] gave for `checkout`, make them `checkout`'s too, where
/// the agent and the Git panel read them. A no-op for a host checkout.
pub async fn sync_tracking(repo: &Path, checkout: &Path) -> Result<()> {
    boxes::mirror::push_tracking(repo, checkout).await
}

/// Make `sha`, a commit the host wrote into the transport repository (a
/// signed copy on publish), an object in `checkout` before any ref there
/// names it. A no-op for a host checkout, whose store already has it.
pub async fn export_commit(checkout: &Path, sha: &str) -> Result<()> {
    boxes::mirror::export_commit(checkout, sha).await
}

/// Make `refname` (a turn checkpoint) in the checkout `source` exist in the
/// checkout `dest` under the same name, force-updated: a re-captured
/// checkpoint is never a fast-forward of the old one. Wherever each lives:
///
/// - on one filesystem (the host, or one box's guest): `dest` fetches from
///   `source` by path, where `dest` lives;
/// - out of a box: the one ref fetched from `source` in the guest into the
///   box's host-side mirror under a name of this carry's own
///   (docs/sandbox-foundation.md rule 4), which a host `dest` fetches from;
///   the name is dropped from the mirror afterwards;
/// - into a box: pushed over the box's channel, from the host `source` or
///   from the source box's mirror.
///
/// A box on either side is booted if it is stopped: carrying code is a user
/// action (a fork).
pub async fn carry_checkpoint(source: &Path, dest: &Path, refname: &str) -> Result<()> {
    let from_box = boxes::box_holding(source);
    let into_box = boxes::box_holding(dest);
    let one_place = match (&from_box, &into_box) {
        (None, None) => true,
        (Some(s), Some(d)) => s.agent_id == d.agent_id,
        (Some(_), None) | (None, Some(_)) => false,
    };
    if one_place {
        return fetch_ref(dest, source, &format!("+{refname}:{refname}")).await;
    }
    // Out of a box, the ref is read under the name `carry_out` gave it in the
    // mirror: the original name there is shared by every checkout of the box.
    let (from, carried) = match &from_box {
        Some(identity) => {
            let (mirror, carried) = boxes::mirror::carry_out(identity, source, refname).await?;
            (mirror, Some(carried))
        }
        None => (source.to_path_buf(), None),
    };
    let refspec = format!("+{}:{refname}", carried.as_deref().unwrap_or(refname));
    let result = match &into_box {
        Some(identity) => boxes::mirror::push_into(identity, dest, &from, &[refspec]).await,
        None => fetch_ref(dest, &from, &refspec).await,
    };
    if let Some(carried) = &carried {
        boxes::mirror::drop_carried(&from, carried).await;
    }
    result
}

/// `git fetch --no-tags <from> <refspec>` in `dest`, `from` named by path.
async fn fetch_ref(dest: &Path, from: &Path, refspec: &str) -> Result<()> {
    let from = from
        .to_str()
        .ok_or_else(|| Error::InvalidPath(from.display().to_string()))?;
    crate::git::run_git(
        dest,
        &["fetch", "--no-tags", from, refspec],
        "fetch checkpoint",
    )
    .await?;
    Ok(())
}

/// The interactive shell the user's workspace terminal runs for the checkout
/// `tree`, when the engine keeps it somewhere the host's shell can't reach:
/// `(program, args, kill)` for a host PTY to spawn, `None` for a host tree,
/// which gets the user's own shell. A box's is a shell in its guest over the
/// box's channel (booting a stopped box), interactive but not a login shell:
/// Debian's /etc/profile resets PATH, dropping the tools disk (claude,
/// codegraph) the guest env puts on it. The box is held like an agent's
/// session from before it is reached, so its idle stop never ends the
/// user's shell under them; `kill` releases it, and a return without a shell
/// drops the hold.
pub fn shell_program(tree: &Path) -> Result<Option<(PathBuf, Vec<String>, KillHandle)>> {
    let hold = boxes::hold_session(tree);
    Ok(
        boxes::guest_program(tree, "bash", &["-i".to_string()])?.map(|(program, args)| {
            let kill = hold.map_or(KillHandle::ProcessGroup, boxes::Hold::into_kill);
            (program, args, kill)
        }),
    )
}

/// Whether `path` is a checkout whose engine is not running, so work there
/// would boot it: a stopped box. `false` for a host path. A timer-driven read
/// skips such a checkout (docs/sandbox-foundation.md rule 2).
pub fn checkout_asleep(path: &Path) -> bool {
    boxes::asleep_for_path(path)
}

/// The disposal state of `path`, a checkout whose engine is not running
/// ([`checkout_asleep`]), as read when it stopped: a box records its
/// checkouts' state on every stop and drops the record as it starts. `None`
/// unless the box is stopped, and when nothing usable was recorded, which a
/// caller about to delete must treat as unknown.
pub fn disposal_at_stop(path: &Path) -> Option<crate::git_state::DisposalCheck> {
    boxes::mirror::disposal_at_stop(path)
}

/// Whether `path` is a checkout the engine keeps off the host (in a box,
/// running or not).
pub fn checkout_is_remote(path: &Path) -> bool {
    boxes::box_holding(path).is_some()
}

/// Whether `path` is in a box that is running, so work there would not boot
/// it.
pub fn checkout_awake(path: &Path) -> bool {
    boxes::running_for_path(path)
}

/// The agent binary handed to `launch_agent` under `engine`: under a
/// container engine the provider's in-image command name (the image carries
/// its own install; a host path would be meaningless, or missing, inside the
/// container), in a box the CLI on its tools disk, under seatbelt what
/// `host_resolve` finds on this host. `label` names the provider in a
/// refusal. A container engine reaches here only for a provider
/// [`ensure_supports_provider`] accepts, so its refusal is defensive.
pub fn agent_bin(
    engine: EngineKind,
    provider: &str,
    label: &str,
    host_resolve: impl FnOnce() -> Result<String>,
) -> Result<String> {
    match engine {
        EngineKind::Docker | EngineKind::Podman => docker::DockerProvider::from_id(provider)
            .map(|p| p.image_bin().to_string())
            .ok_or_else(|| {
                Error::Other(format!(
                    "{label} isn't available in container sandboxes yet"
                ))
            }),
        // The CLI on the box's tools disk; a host path would mean nothing in
        // the guest.
        EngineKind::Box => fletch_box::tools::bin_path(provider)
            .ok_or_else(|| Error::Other(format!("{label} isn't available in box sandboxes yet"))),
        EngineKind::SandboxExec => host_resolve(),
    }
}

/// The codegraph launcher as `agent_id`'s sandbox sees it under `engine`,
/// when it has one: what `host_bin` finds for a seatbelt agent, which runs on
/// this host; the Linux build on the tools disk the agent's box runs with;
/// nothing for a container, which can't exec the host's macOS binary and has
/// no other.
pub fn codegraph_bin(
    engine: EngineKind,
    agent_id: &str,
    provider: &str,
    host_bin: impl FnOnce() -> Option<String>,
) -> Option<String> {
    match engine {
        EngineKind::SandboxExec => host_bin(),
        EngineKind::Box => boxes::codegraph_bin(agent_id, provider),
        EngineKind::Docker | EngineKind::Podman => None,
    }
}

/// The seatbelt engine, shared process-wide: it is stateless, and per-launch
/// state (profile tempfile) lives on the `LaunchPlan`.
fn seatbelt_engine() -> Arc<dyn SandboxEngine> {
    static ENGINE: OnceLock<Arc<dyn SandboxEngine>> = OnceLock::new();
    ENGINE
        .get_or_init(|| Arc::new(seatbelt::SandboxExecEngine))
        .clone()
}

#[cfg(test)]
mod adopted_tree_tests {
    use super::*;

    /// An agent adopting a tree runs where the tree lives: in the box that
    /// holds it, and never in a box for a tree no box holds.
    #[tokio::test]
    async fn an_adopting_agent_runs_where_its_tree_lives() {
        let registry = boxes::BoxRegistry::global();
        let root = PathBuf::from("/adopted-tree-test/coder-root");
        registry.register(boxes::BoxIdentity {
            agent_id: "adopted-tree-test-coder".into(),
            project_id: "p".into(),
            provider: "claude".into(),
            writable_root: root.clone(),
            rpc_dir: PathBuf::from("/adopted-tree-test/rpc"),
            home: PathBuf::from("/adopted-tree-test/home"),
            shares: Vec::new(),
        });
        assert_eq!(engine_for_adopted_tree(&root.join("repo")), EngineKind::Box);
        // Whatever Settings selects, a host tree never resolves to a box.
        assert_ne!(
            engine_for_adopted_tree(Path::new("/adopted-tree-test/runs/run-1/repo")),
            EngineKind::Box
        );
        registry.discard("adopted-tree-test-coder").await.unwrap();
        assert_ne!(engine_for_adopted_tree(&root.join("repo")), EngineKind::Box);
    }
}

#[cfg(test)]
mod workspace_type_tests {
    use super::*;

    /// The composer's spelling is `host` or `box`; nothing else is an
    /// override, and no spelling reaches a stamped engine.
    #[test]
    fn only_host_and_box_parse() {
        assert_eq!(WorkspaceType::parse("host"), Some(WorkspaceType::Host));
        assert_eq!(WorkspaceType::parse(" box "), Some(WorkspaceType::Box));
        for raw in ["", "auto", "docker", "podman", "sandbox-exec", "vm"] {
            assert_eq!(WorkspaceType::parse(raw), None, "{raw}");
        }
    }

    /// A session's workspace type stands over the host's choice and the
    /// project's placement, an explicit box is never quietly something else,
    /// and an adopted tree keeps running where it lives. Written to hold
    /// whatever the boxes switch and the selected engine read while other
    /// tests flip them.
    #[test]
    fn a_session_choice_is_honoured_or_refused() {
        let repo = tempfile::tempdir().unwrap();
        // A project pinned to boxes still gets this Mac when the session asks.
        assert_eq!(
            engine_for_spawn(
                None,
                Some("box"),
                repo.path(),
                "claude",
                Some(WorkspaceType::Host)
            )
            .unwrap(),
            EngineKind::SandboxExec
        );
        // A provider boxes do not run: refused whatever the switch and the
        // runtime say, never stamped seatbelt instead.
        assert!(engine_for_spawn(
            None,
            None,
            repo.path(),
            "antigravity",
            Some(WorkspaceType::Box)
        )
        .is_err());
        // Without an override nothing errs.
        assert!(engine_for_spawn(None, None, repo.path(), "antigravity", None).is_ok());
        // An adopted host tree ignores the override.
        let tree = Path::new("/workspace-type-test/runs/run-1/repo");
        let adopted = engine_for_spawn(
            Some(tree),
            None,
            repo.path(),
            "claude",
            Some(WorkspaceType::Box),
        )
        .unwrap();
        assert_ne!(adopted, EngineKind::Box);
    }

    /// A stamped engine is kept exactly, whatever the project's placement
    /// says: a fork of a container-stamped parent stays in its container
    /// engine, and one of a seatbelt parent on this Mac.
    #[test]
    fn a_stamped_engine_is_kept() {
        let repo = tempfile::tempdir().unwrap();
        for kind in [
            EngineKind::Docker,
            EngineKind::Podman,
            EngineKind::SandboxExec,
        ] {
            assert_eq!(
                engine_for_spawn(
                    None,
                    Some("box"),
                    repo.path(),
                    "claude",
                    Some(WorkspaceType::Stamped(kind))
                )
                .unwrap(),
                kind
            );
        }
    }

    /// A stamped box that cannot exist refuses with the reason, as an
    /// explicit box does, never falling back to this Mac.
    #[test]
    fn a_stamped_box_with_the_switch_off_is_refused() {
        let _serial = boxes::settings::TEST_LOCK.blocking_lock();
        let before = boxes::settings::current();
        boxes::settings::set(boxes::settings::BoxSettings {
            enabled: false,
            ..before.clone()
        });
        let repo = tempfile::tempdir().unwrap();
        let result = engine_for_spawn(
            None,
            Some("host"),
            repo.path(),
            "claude",
            Some(WorkspaceType::Stamped(EngineKind::Box)),
        );
        boxes::settings::set(before);
        let err = result.expect_err("a stamped box with the switch off must refuse");
        assert!(err.to_string().contains(boxes::SWITCHED_OFF), "{err}");
    }

    /// The list offers both types, this Mac always, and a box only when the
    /// override would be accepted, with the refusal as its reason.
    #[test]
    fn the_list_says_why_a_box_is_not_offered() {
        let repo = tempfile::tempdir().unwrap();
        let options = workspace_types(None, repo.path(), "antigravity");
        let kinds: Vec<&str> = options.iter().map(|o| o.kind).collect();
        assert_eq!(kinds, ["host", "box"]);
        assert!(kinds.iter().all(|k| WorkspaceType::parse(k).is_some()));
        assert!(options[0].available && options[0].reason.is_none());
        assert!(!options[1].available);
        assert!(options[1].reason.as_deref().is_some_and(|r| !r.is_empty()));
        assert!(
            !options[1].default,
            "nothing defaults to a refused box here"
        );
    }
}

#[cfg(test)]
mod provider_gate_tests {
    use super::*;

    #[test]
    fn docker_supports_wired_providers_but_refuses_the_rest() {
        // All wired-up providers launch under docker.
        for provider in ["claude", "codex", "opencode", "pi", "cursor"] {
            assert!(
                ensure_supports_provider(EngineKind::Docker, provider).is_ok(),
                "{provider} should be docker-supported",
            );
        }
        // antigravity is the last still-gated provider — no container auth path.
        let err = ensure_supports_provider(EngineKind::Docker, "antigravity")
            .expect_err("antigravity must refuse under docker");
        assert!(
            err.to_string()
                .ends_with("isn't available in container sandboxes yet"),
            "unexpected refusal copy: {err}",
        );
        // The copy uses the human-facing product name, not the provider id.
        assert!(err.to_string().starts_with("Antigravity "), "{err}");

        // Seatbelt is unaffected.
        for provider in ["claude", "codex", "cursor", "antigravity"] {
            assert!(ensure_supports_provider(EngineKind::SandboxExec, provider).is_ok());
        }
    }
}

#[cfg(test)]
mod tests {
    /// Files outside `sandbox/` that name engine internals today. This list
    /// may only shrink: a feature that needs an engine fact asks
    /// `sandbox` for it instead of matching on the engine itself. These are
    /// the set named in docs/sandbox-foundation.md rule 1: the engine's own
    /// settings surface (`commands/boxes.rs`, `commands/settings.rs`, their
    /// re-export in `commands/mod.rs`, the per-project egress setting in
    /// `commands/project_settings.rs` and `project_file/mod.rs`), boot
    /// (`host/boot.rs`) and the event list (`remote/events.rs`). Paths are
    /// relative to this crate's `src/`.
    const ALLOWED: &[&str] = &[
        "commands/boxes.rs",
        "commands/mod.rs",
        "commands/project_settings.rs",
        "commands/settings.rs",
        "host/boot.rs",
        "project_file/mod.rs",
        "remote/events.rs",
    ];

    /// What marks a line as engine knowledge: matching on an engine variant,
    /// or reaching into the box engine's module.
    const TOKENS: &[&str] = &["EngineKind::", "sandbox::boxes", "boxes::", "box_holding("];

    /// Engine knowledge stays in `sandbox/` (docs/sandbox-foundation.md, rule
    /// 1). Every production line of every file outside it counts, comments
    /// and doc comments included: simple and conservative. A file that gains
    /// one of [`TOKENS`] fails this until the knowledge moves behind
    /// `sandbox`; a file that loses its last one fails it until it is struck
    /// from [`ALLOWED`], so the list cannot go stale.
    #[test]
    fn engine_knowledge_stays_in_sandbox() {
        let found: Vec<String> = crate::source_scan::production_sources()
            .into_iter()
            .filter(|(rel, text)| {
                !rel.starts_with("sandbox/") && TOKENS.iter().any(|t| text.contains(t))
            })
            .map(|(rel, _)| rel)
            .collect();
        let added: Vec<&String> = found
            .iter()
            .filter(|rel| !ALLOWED.contains(&rel.as_str()))
            .collect();
        let gone: Vec<&&str> = ALLOWED
            .iter()
            .filter(|rel| !found.iter().any(|f| f == *rel))
            .collect();
        assert!(
            added.is_empty() && gone.is_empty(),
            "engine knowledge outside sandbox/ must go through sandbox (docs/sandbox-foundation.md \
             rule 1).\nnew files naming engine internals: {added:?}\nno longer naming them, \
             strike from sandbox::tests::ALLOWED: {gone:?}"
        );
    }

    /// The container engines' own modules: what retiring Docker and Podman
    /// deletes (docs/sandbox-foundation.md, "Retiring the container engines").
    const CONTAINER_MODULES: &[&str] =
        &["sandbox/container/", "sandbox/docker/", "sandbox/podman/"];

    /// Files outside [`CONTAINER_MODULES`] that name them today: the engine
    /// registry and its availability re-exports (`sandbox/mod.rs`), the
    /// isolation report's container rows (`sandbox/guarantees.rs`), the
    /// container settings surface (`commands/settings.rs`) and boot
    /// (`host/boot.rs`). This list may only shrink. Paths are relative to
    /// this crate's `src/`.
    const CONTAINER_NAMERS: &[&str] = &[
        "commands/settings.rs",
        "host/boot.rs",
        "sandbox/guarantees.rs",
        "sandbox/mod.rs",
    ];

    /// What marks a line as naming a container module: a path into one of
    /// them, absolute (`sandbox::docker::…`) or relative (`docker::…`,
    /// `super::container::…`).
    const CONTAINER_TOKENS: &[&str] = &["container::", "docker::", "podman::"];

    /// Nothing depends on the container engines but the files in
    /// [`CONTAINER_NAMERS`], so retiring them is a deletion plus edits to
    /// those files. Like [`engine_knowledge_stays_in_sandbox`], every
    /// production line counts, comments included; a file that gains a
    /// reference fails this, and so does one that loses its last.
    #[test]
    fn container_engines_are_a_leaf() {
        let found: Vec<String> = crate::source_scan::production_sources()
            .into_iter()
            .filter(|(rel, text)| {
                !CONTAINER_MODULES.iter().any(|m| rel.starts_with(m))
                    && CONTAINER_TOKENS.iter().any(|t| text.contains(t))
            })
            .map(|(rel, _)| rel)
            .collect();
        let added: Vec<&String> = found
            .iter()
            .filter(|rel| !CONTAINER_NAMERS.contains(&rel.as_str()))
            .collect();
        let gone: Vec<&&str> = CONTAINER_NAMERS
            .iter()
            .filter(|rel| !found.iter().any(|f| f == *rel))
            .collect();
        assert!(
            added.is_empty() && gone.is_empty(),
            "the container engines must stay a leaf (docs/sandbox-foundation.md, \"Retiring the \
             container engines\").\nnew files naming sandbox::container, docker or podman: \
             {added:?}\nno longer naming them, strike from sandbox::tests::CONTAINER_NAMERS: \
             {gone:?}"
        );
    }
}
