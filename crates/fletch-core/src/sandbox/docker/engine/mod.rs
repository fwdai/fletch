//! The Docker sandbox engine: one container per agent process, agent ≈ PID 1.
//!
//! Launch shape: every agent process is its own `docker run --rm --init` — no
//! long-lived container + `docker exec`, whose kill/exit-code semantics are
//! broken. The invariants this file carries:
//!
//! - **Path identity (invariant 1).** The three mounts — the agent's writable
//!   root, its RPC mailbox, and `~/.claude` — are bind-mounted at their exact
//!   host paths, and the container runs with `HOME=<host home>`; transcripts,
//!   RPC payloads, and diff paths all embed absolute host paths, so nothing in
//!   the app translates paths. The workspace and mailbox are read-write;
//!   `~/.claude` (and any non-default `CLAUDE_CONFIG_DIR`) enters **read-only
//!   except `.credentials.json`** (invariant 5).
//! - **The real repo's writable state and its hooks never enter the container;
//!   its object store enters read-only (invariant 2).** Only the agent's own
//!   parent dir is mounted writable; `supervisor::lifecycle` forces clone-mode
//!   workspaces for docker agents, so no linked-worktree `.git` pointer can
//!   reach the user's repo. A `--shared` clone borrows the source's object
//!   store via alternates (see `sandbox::provision`); that store — and only
//!   that store, never the source `.git` (config/hooks) — is bind-mounted
//!   **read-only** at its identical host path so in-container git can read
//!   history while a write attempt fails with `Read-only file system`.
//! - **`~/.claude` is not a write surface (invariant 5).** `~/.claude` is
//!   shared host state: its `settings.json` can define hooks Claude Code runs
//!   *on the host*, and it holds other agents' transcripts and MCP secrets. It
//!   is mounted read-only so a prompt-injected container agent cannot plant a
//!   host-executed hook. Two kinds of writable exception are layered on top,
//!   both ordered after the RO dir mount in argv: `.credentials.json` is
//!   remounted read-write so claude's own OAuth token refresh still persists to
//!   the host (the `CredentialsFile` auth chain in [`super::auth`] depends on
//!   it), and each [`EPHEMERAL_RUNTIME_SUBDIRS`] entry (`session-env`,
//!   `shell-snapshots`) gets an ephemeral **tmpfs** overlay so claude's
//!   per-session scaffolding — which it otherwise `mkdir`s under the RO dir and
//!   fails with `EROFS` — is written to throwaway container-local storage that
//!   never reaches the host. Neither exception is a persistent host write
//!   surface, so invariant 5 holds: nothing an agent writes under `~/.claude`
//!   survives to influence the host or a later session.
//! - **Secrets never in argv (invariant 3).** Auth vars are set on the docker
//!   *CLI process* environment (`LaunchPlan::env`) and forwarded into the
//!   container with bare `-e NAME` — the value never appears in `ps`.
//! - **No orphans (invariant 4).** Containers carry the `fletch.host-pid` /
//!   `fletch.agent-id` labels the startup sweep keys on (`super::cleanup`).
//!
//! Threat model. The container is *live-process containment*, not a trust
//! boundary against a determined attacker: the git clone + PR flow (invariant
//! 2) is the review gate that keeps agent output off the real repo, `~/.claude`
//! is read-only except the credential file (invariant 5) so a compromised agent
//! can neither plant a host-executed hook nor exfiltrate via config, and secrets
//! stay out of argv (invariant 3). Containers run as root in v1 (a known
//! limitation — see below), so in-container isolation is not relied upon.
//!
//! Containers run as root in v1: Docker Desktop's VirtioFS maps ownership so
//! mounted host files appear owned by the user. // TODO(linux-host): UID
//! mapping before supporting Linux hosts.
//!
//! Layout: this module folder splits the engine into
//! - [`settings`] — launch knobs and the version-refresh guard
//! - [`util`] — docker liveness lookups and its exit-code wording
//!
//! Everything else a container launch decides is runtime-neutral and lives
//! outside this folder: the per-launch env / mount / auth preparation
//! ([`container::launch`](crate::sandbox::container::launch)), per-provider auth
//! ([`container::launch_auth`](crate::sandbox::container::launch_auth)), the
//! `run` argv builder ([`container::run_args`](crate::sandbox::container::run_args)),
//! and container naming ([`container::util`](crate::sandbox::container::util)).
//!
//! The `DockerEngine` struct and its `SandboxEngine` impl stay here.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::error::{Error, Result};
use crate::sandbox::engine::{
    AgentLaunchCtx, EngineKind, KillHandle, KillPlan, LaunchPlan, SandboxEngine,
};

use super::{cli, image, DockerProvider};

mod settings;
#[cfg(test)]
mod tests;
mod util;

// Public surface consumed outside this module (re-exported again by
// `super::mod`); the `engine::X` paths callers already use stay valid.
pub use settings::{
    init_version_refresh_guard, set_launch_settings, LaunchSettings, CPUS_SETTING, IMAGE_SETTING,
    MEMORY_SETTING, VERSION_GUARD_SETTING,
};
// Consumed by sibling docker submodules (`image`, `cleanup`) at `engine::X`.
pub(super) use settings::{image_override, record_version_refresh, version_refresh_attempted};

use crate::sandbox::container::run_args::{run_args, RunSpec, DEFAULT_CPUS, DEFAULT_MEMORY};
use crate::sandbox::container::util::{container_name, non_blank};
use settings::LAUNCH_SETTINGS;
use util::{container_gone_within, describe_exit_code};

/// Signal/removal docker calls during teardown.
const KILL_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a TERM'd container gets to exit before escalating to KILL —
/// same order as the session-side process-group escalation grace windows.
const TERM_GRACE: Duration = Duration::from_millis(500);

/// The `SandboxEngine` implementation for Docker. Obtain it via
/// [`DockerEngine::shared`]: launches embed an `Arc` of the engine in their
/// [`KillHandle`], and sharing one instance also shares the once-per-app-run
/// image resolution cache.
pub struct DockerEngine {
    /// Images resolved for this app run, keyed by `(provider, override)` so each
    /// provider's per-provider image is resolved (and built) at most once, and a
    /// (future) mid-run settings change re-resolves. Only successes are cached —
    /// a failed build retries on the next spawn (the user may have started Docker
    /// or fixed their network since).
    resolved_image: Mutex<std::collections::HashMap<(DockerProvider, Option<String>), String>>,
}

impl DockerEngine {
    /// The process-wide engine instance — the same `Arc` that `engine_for`
    /// hands to launch paths and that every launch parks in its `KillHandle`.
    pub fn shared() -> Arc<DockerEngine> {
        static ENGINE: OnceLock<Arc<DockerEngine>> = OnceLock::new();
        ENGINE
            .get_or_init(|| {
                Arc::new(DockerEngine {
                    resolved_image: Mutex::new(std::collections::HashMap::new()),
                })
            })
            .clone()
    }

    /// The image to launch `provider` from, resolving (and building, if the
    /// embedded image is missing) at most once per app run per (provider,
    /// override) pair. Resolution also runs the background freshness checks
    /// (TTL + host/container version parity — see `image::resolve_image`),
    /// so their cadence is once per app run too. The host version comes from
    /// the existing memoized probe (`agent::cached_provider_version` — at
    /// most one `--version` subprocess per provider per run, shared with
    /// ingest); a machine with no host CLI yields `None` and the version
    /// trigger is simply inert, leaving the TTL as the backstop.
    fn resolve_image_cached(
        &self,
        provider: DockerProvider,
        override_image: Option<&str>,
    ) -> Result<String> {
        let key = (provider, override_image.map(str::to_string));
        // The lock is dropped before resolving: a build can take ten minutes,
        // and holding it would serialize every other provider's cache-hit launch
        // behind it. Two cold launches may then resolve the same key at once,
        // which is safe — `BUILD_LOCK` plus the re-check under it make the
        // second resolver a cheap no-op.
        if let Some(tag) = self.resolved_image.lock().unwrap().get(&key) {
            return Ok(tag.clone());
        }
        // Skip the host probe entirely on the override path: the user's image
        // is never inspected or refreshed, so there is nothing to compare.
        let host_cli_version = if override_image
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .is_none()
        {
            crate::agent::cached_provider_version(provider.id())
        } else {
            None
        };
        // Per-line build output goes to the log; the UI build toast is driven
        // separately by the `progress` sink inside `image::ensure_image`.
        // Free-form output rides in the `line` field (not the message) so the
        // sentry scrubber drops it — see the privacy invariant in `lib.rs`.
        let on_progress = |line: &str| tracing::info!(target: "fletch::docker_build", line = %line, "docker build output");
        let tag = image::resolve_image(
            provider,
            override_image,
            host_cli_version.as_deref(),
            &on_progress,
        )
        .map_err(|e| Error::Other(format!("preparing the Docker sandbox image failed: {e}")))?;
        self.resolved_image.lock().unwrap().insert(key, tag.clone());
        Ok(tag)
    }
}

impl SandboxEngine for DockerEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::Docker
    }

    /// Launch a container for `ctx.provider`. Everything but the binary and the
    /// resolved image is shared with the other container runtime: the env,
    /// mounts and auth come from
    /// [`container::launch::prepare`](crate::sandbox::container::launch::prepare)
    /// and the argv from
    /// [`container::run_args`](crate::sandbox::container::run_args). `agent_bin`
    /// is the in-image command name the caller already resolved for the docker
    /// boundary (`claude` / `codex`). `ensure_engine_supports_provider` gates
    /// this to a supported provider, so the `from_id` failure below is
    /// defensive only.
    fn launch_agent(&self, ctx: &AgentLaunchCtx, agent_bin: &str) -> Result<LaunchPlan> {
        let provider = DockerProvider::from_id(ctx.provider).ok_or_else(|| {
            Error::Other(format!(
                "Docker sandbox has no support for provider `{}`",
                ctx.provider
            ))
        })?;
        let docker = cli::docker_bin()
            .ok_or_else(|| Error::Other("docker binary not found — is Docker installed?".into()))?;
        let settings = LAUNCH_SETTINGS.read().clone();
        let image = self.resolve_image_cached(provider, settings.image_override.as_deref())?;
        let name = container_name(ctx.agent_id);
        // Everything about *what* to mount, set, and authenticate is
        // runtime-neutral policy; this engine only decides which binary carries
        // it out.
        let prep = crate::sandbox::container::launch::prepare(ctx, provider)?;

        let prefix_args = {
            let auth_vars = prep.auth_vars();
            run_args(&RunSpec {
                interactive: ctx.interactive,
                name: &name,
                agent_id: ctx.agent_id,
                writable_root: ctx.writable_root,
                rpc_dir: ctx.rpc_dir,
                home: ctx.home,
                cwd: ctx.cwd,
                blackboard: ctx.blackboard,
                adopted_workspace: ctx.adopted_workspace(),
                mounts: prep.mounts(),
                borrowed_object_stores: &prep.borrowed_object_stores,
                memory: non_blank(settings.memory.as_deref()).unwrap_or(DEFAULT_MEMORY),
                cpus: non_blank(settings.cpus.as_deref()).unwrap_or(DEFAULT_CPUS),
                image: &image,
                agent_bin,
                auth_vars: &auth_vars,
            })
        };

        Ok(LaunchPlan {
            program: docker,
            prefix_args,
            env: prep.env,
            kill: KillHandle::Engine {
                engine: DockerEngine::shared(),
                // One daemon endpoint: nothing to pin teardown to.
                plan: KillPlan::Container {
                    name,
                    connection: None,
                },
            },
        })
    }

    /// Tear the container down: TERM, a grace window, then KILL, then a
    /// best-effort `rm -f`. Best-effort throughout and always `Ok` — the
    /// container is usually already gone (`--rm`, daemon stopped, normal
    /// exit), and an error here would abort the caller's local process-group
    /// teardown of the docker CLI child.
    fn kill(&self, plan: &KillPlan) -> Result<()> {
        let KillPlan::Container { name, .. } = plan;
        match cli::run_docker(&["kill", "-s", "TERM", name], KILL_TIMEOUT) {
            Ok(out) if out.status.success() => {
                if !container_gone_within(name, TERM_GRACE) {
                    tracing::info!(container = %name, "container survived TERM grace; escalating to KILL");
                    let _ = cli::run_docker(&["kill", name], KILL_TIMEOUT);
                }
            }
            // Non-zero exit = "no such container" (already exited and
            // auto-removed) — nothing to escalate.
            Ok(_) => {}
            Err(e) => tracing::warn!(container = %name, error = %e, "docker kill failed"),
        }
        // Usually a no-op thanks to --rm; covers a wedged auto-remove.
        let _ = cli::run_docker(&["rm", "-f", name], KILL_TIMEOUT);
        Ok(())
    }

    fn describe_exit(&self, _plan: &KillPlan, code: i32) -> Option<String> {
        describe_exit_code(code)
    }
}
