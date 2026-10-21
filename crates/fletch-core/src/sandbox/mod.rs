pub mod cargo_home;
pub(crate) mod container;
pub mod docker;
mod engine;
pub mod guarantees;
pub mod podman;
pub mod policy;
pub mod provision;
mod seatbelt;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use parking_lot::RwLock;

use crate::error::{Error, Result};

pub use docker::{availability as docker_availability, DockerAvailability};
pub use engine::{AgentLaunchCtx, EngineKind, KillHandle, LaunchPlan, SandboxEngine};
pub use guarantees::{describe as describe_isolation, IsolationReport};
pub use podman::{
    availability as podman_availability, launch_blocker as podman_launch_blocker,
    PodmanAvailability,
};
pub use policy::{toolchain_cache_env, toolchain_cache_root, BoxPlatform};
pub use seatbelt::{
    build_run_profile, cleanup_nested_checkouts_roots, cleanup_nested_rpc_roots,
    nested_checkouts_root, nested_rpc_root, profile_args, SANDBOX_EXEC,
};

/// Everything a confined child needs to use the redirected toolchain caches:
/// the shared root created host-side, the cargo overlay prepared
/// ([`cargo_home`]), and the env pointing every toolchain at them. Agents and
/// Run commands call this alike, so both halves of the app share one warm
/// cache instead of maintaining two. `cwd` is where the child's commands run
/// (the agent's checkout, the Run target).
///
/// Creating the root here matters: the profile grants `(subpath <root>)`,
/// which lets a tool create the root itself but not its parent. Best-effort
/// throughout — a failure means the toolchain's writes fail the way they did
/// before the redirect existed, never that the sandbox is looser. Cargo in
/// particular is redirected only when its global config stays visible from
/// `cwd` and its overlay is in place ([`cargo_home::config_reachable`]);
/// otherwise the child gets cargo's *real* home as `CARGO_HOME`, set
/// explicitly rather than left to inheritance — a nested Fletch's own process
/// env carries the overlay it inherited, and a child that merely inherited
/// that would be redirected after all. Cargo then reads its real home exactly
/// as on the host, and its writes fail closed.
///
/// `platform` is what the child runs on — [`BoxPlatform::host`] for seatbelt,
/// [`BoxPlatform::linux`] for a container — so rows whose contents are
/// per-platform land in their own subdir of the one shared root.
pub fn prepare_toolchain_cache(
    home: &std::path::Path,
    cwd: &std::path::Path,
    platform: BoxPlatform,
) -> Vec<(String, String)> {
    let root = toolchain_cache_root(home);
    let _ = std::fs::create_dir_all(&root);
    let mut env = toolchain_cache_env(&root, platform);
    let user_home = cargo_home::user_home(home);
    let keep_real_home = |env: &mut Vec<(String, String)>| {
        env.push((
            "CARGO_HOME".to_string(),
            user_home.to_string_lossy().into_owned(),
        ));
    };
    if !cargo_home::config_reachable(&user_home, cwd) {
        tracing::info!(
            cwd = %cwd.display(),
            cargo_home = %user_home.display(),
            "cargo keeps its real home: its global config would not be found from this directory"
        );
        keep_real_home(&mut env);
        return env;
    }
    match cargo_home::prepare(&root, &user_home) {
        Ok(()) => env.extend(cargo_home::env(&root, &user_home)),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "could not prepare the sandboxed cargo home; cargo writes will fail closed"
            );
            keep_real_home(&mut env);
        }
    }
    env
}

/// The `settings` key holding the user's engine choice; values are
/// [`EngineKind::as_setting`] spellings, default `sandbox-exec`.
pub const ENGINE_SETTING: &str = "sandbox_engine";

/// The user's engine selection, mirrored from the `sandbox_engine` setting so
/// spawn paths (deep in agent code, no DB handle) can read it. Seeded at
/// startup (`lib.rs setup`) and updated by the `set_sandbox_engine` command.
/// Teardown never consults this — sessions carry a `KillHandle` bound to the
/// engine that launched them.
static SELECTED_ENGINE: RwLock<EngineKind> = RwLock::new(EngineKind::SandboxExec);

pub fn set_selected_engine_kind(kind: EngineKind) {
    *SELECTED_ENGINE.write() = kind;
}

/// The engine a *new* agent would be stamped with right now. Existing agents
/// keep the kind stamped on their record at creation (see
/// `supervisor::lifecycle::spawn_agent`), so a settings change never
/// re-engines them.
pub fn selected_engine_kind() -> EngineKind {
    *SELECTED_ENGINE.read()
}

/// This engine's own data directory ([`crate::host::BootConfig::data_dir`]),
/// mirrored process-wide so the macOS profile builder — reached on the spawn
/// path, with no handle on the boot config — can carve it out of what a confined
/// agent may read or write. Same mirror idiom as [`set_selected_engine_kind`].
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

/// The seatbelt engine, shared process-wide: it is stateless, and per-launch
/// state (profile tempfile) lives on the `LaunchPlan`.
fn seatbelt_engine() -> Arc<dyn SandboxEngine> {
    static ENGINE: OnceLock<Arc<dyn SandboxEngine>> = OnceLock::new();
    ENGINE
        .get_or_init(|| Arc::new(seatbelt::SandboxExecEngine))
        .clone()
}
