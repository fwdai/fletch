pub mod boxes;
pub mod cargo_home;
pub(crate) mod container;
pub mod docker;
mod engine;
pub mod guarantees;
pub mod placement;
pub mod podman;
pub mod policy;
pub mod provision;
mod seatbelt;
pub mod toolchain_cache;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use parking_lot::RwLock;

use crate::error::{Error, Result};

pub use docker::{availability as docker_availability, DockerAvailability};
pub use engine::{
    AgentLaunchCtx, EngineKind, EnvDelivery, KillGuard, KillHandle, LaunchPlan, SandboxEngine,
    TurnPrep,
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
