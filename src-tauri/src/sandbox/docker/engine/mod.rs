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
//! - [`auth`] — per-provider container auth
//! - [`config_dir`] — non-default config-dir detection and borrowed object stores
//! - [`run_args`] — the `docker run` argv builder
//! - [`util`] — naming, liveness, exit codes
//!
//! The `DockerEngine` struct and its `SandboxEngine` impl stay here.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::error::{Error, Result};
use crate::sandbox::engine::{
    AgentLaunchCtx, EngineKind, Keepalive, KillHandle, KillPlan, LaunchPlan, SandboxEngine,
};
use crate::sandbox::policy::{codex_home_dir, opencode_config_dir, opencode_data_dir};

use super::{cli, image, DockerProvider};

mod auth;
mod config_dir;
mod run_args;
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

use auth::{
    apply_container_auth, prepare_codex_launch, prepare_cursor_launch, prepare_opencode_launch,
    prepare_pi_launch, present_api_keys,
};
use config_dir::{
    borrowed_object_stores, codex_home_is_nondefault, nondefault_claude_config_dir,
    xdg_base_is_nondefault,
};
use run_args::{prepare_config_mount_dir, run_args, ProviderMounts, RunSpec, CREDENTIALS_FILE};
use settings::{DEFAULT_CPUS, DEFAULT_MEMORY, LAUNCH_SETTINGS};
use util::{container_gone_within, container_name, describe_exit_code, non_blank};

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
        let mut cache = self.resolved_image.lock().unwrap();
        if let Some(tag) = cache.get(&key) {
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
        cache.insert(key, tag.clone());
        Ok(tag)
    }
}

impl SandboxEngine for DockerEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::Docker
    }

    /// Launch a container for `ctx.provider`. The provider-agnostic scaffolding —
    /// the writable-root and RPC mailbox mounts, borrowed git object stores, the
    /// `--rm --init` shape, naming, and teardown — is shared; the per-provider
    /// image, config-dir mount, and auth are selected by matching on
    /// [`DockerProvider`]. `agent_bin` is the in-image command name the caller
    /// already resolved for the docker boundary (`claude` / `codex`).
    /// `ensure_engine_supports_provider` gates this to a supported provider, so
    /// the `from_id` failure below is defensive only.
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

        // Object stores every checkout under the agent's writable root borrows
        // via git alternates (a --shared clone). Scanned across all tracked
        // repos, not just the primary `cwd`: a multi-repo agent has one shared
        // clone per repo, each borrowing its own source's objects. Mounted
        // read-only so in-container git reads borrowed history; empty for old
        // full-copy clones or worktrees (no mount). Docker forces Clone-mode
        // workspaces for every provider, so this is provider-agnostic.
        let borrowed_object_stores = borrowed_object_stores(ctx.writable_root);

        // Env set on the docker CLI process; forwarded into the container by the
        // bare `-e NAME` flags `run_args` emits (values never touch argv —
        // invariant 3). Config-dir + auth vars are appended per provider below.
        let mut env: Vec<(String, String)> = vec![
            ("HOME".into(), ctx.home.to_string_lossy().into_owned()),
            (
                "FLETCH_RPC_DIR".into(),
                ctx.rpc_dir.to_string_lossy().into_owned(),
            ),
            ("TERM".into(), "xterm-256color".into()),
            ("COLORTERM".into(), "truecolor".into()),
        ];
        // A workflow step agent's blackboard is bind-mounted at its identical
        // host path (invariant 1, like the RPC mailbox), so `WF_BLACKBOARD` is
        // the same host path under either engine. Pushed before the provider
        // match sets `auth_start` so it forwards as a plain env var, not an
        // auth var.
        if let Some(board) = ctx.blackboard {
            // Fail closed if the blackboard was not provisioned before launch:
            // a bare `-v` on a missing source has Docker create it *root-owned*,
            // leaving the host-side reader unable to read the agent's verdict/
            // handoff files. Seatbelt already fails here (its `canonicalize`
            // errors on a missing path); match that so an ordering bug in the
            // scheduler surfaces instead of silently corrupting the mount.
            if !board.is_dir() {
                return Err(Error::Other(format!(
                    "workflow blackboard not provisioned before launch: {}",
                    board.display()
                )));
            }
            env.push((
                crate::workflow::blackboard::WF_BLACKBOARD_ENV.into(),
                board.to_string_lossy().into_owned(),
            ));
        }

        // Owned per-provider mount inputs, borrowed into a `ProviderMounts` when
        // the `RunSpec` is built below. Defaults describe "no config surface";
        // the matched arm fills in what its provider needs.
        let mut claude_config_dir: Option<PathBuf> = None;
        let mut claude_credentials_rw = false;
        let mut config_dir_credentials_rw = false;
        let mut projects_src: Option<PathBuf> = None;
        let mut codex_config_dir: Option<PathBuf> = None;
        let mut forward_codex_home = false;
        let mut oc_data: Option<PathBuf> = None;
        let mut oc_config: Option<PathBuf> = None;
        let mut forward_xdg_data_home = false;
        let mut forward_xdg_config_home = false;
        let mut pi_data: Option<PathBuf> = None;
        let mut cursor_data: Option<PathBuf> = None;

        // The config-dir env (CLAUDE_CONFIG_DIR / CODEX_HOME / XDG_*) is pushed
        // before this mark so only the *auth* tail is forwarded as auth vars.
        let auth_start;
        match provider {
            DockerProvider::Claude => {
                let cfg = nondefault_claude_config_dir(ctx.home);

                // Make sure the mount sources exist before we hand them to `-v`.
                // If a source can't be created (a file already sits at the path,
                // a read-only or missing parent, permissions), mounting it anyway
                // would let Docker recreate it root-owned or fail the bind
                // opaquely — either way claude loses access to its auth/config.
                // Fail the launch with the path instead of a bad mount.
                let claude_dir = ctx.home.join(".claude");
                prepare_config_mount_dir(&claude_dir)?;
                if let Some(dir) = &cfg {
                    prepare_config_mount_dir(dir)?;
                }

                // Per-agent host dir backing claude's `projects/` (session
                // transcripts). Bind-mounted read-write over the read-only config
                // dir's `projects/` (see [`push_claude_config_mount`]) so
                // `--resume` survives container recreation without exposing the
                // shared `~/.claude/projects` — other agents' transcripts and
                // global memory stay unreachable (invariant 5). Lives under the
                // agent's writable root, so archive teardown's `rm -rf` reclaims
                // it with no separate cleanup. Created before `docker run` so
                // Docker binds an existing source instead of materializing it
                // root-owned.
                let ps = ctx
                    .writable_root
                    .join(crate::transcripts::DOCKER_CLAUDE_PROJECTS_DIRNAME);
                std::fs::create_dir_all(&ps).map_err(|e| {
                    Error::Other(format!(
                        "preparing Docker sandbox projects mount {} failed: {e}",
                        ps.display()
                    ))
                })?;

                // The read-only config mounts get a writable `.credentials.json`
                // overlay only when the file already exists: a bare `-v` on a
                // missing source makes Docker create a root-owned *directory*
                // there, which would break claude's later write of the real file.
                claude_credentials_rw = claude_dir.join(CREDENTIALS_FILE).is_file();
                config_dir_credentials_rw = cfg
                    .as_deref()
                    .is_some_and(|dir| dir.join(CREDENTIALS_FILE).is_file());
                if let Some(dir) = &cfg {
                    env.push((
                        "CLAUDE_CONFIG_DIR".into(),
                        dir.to_string_lossy().into_owned(),
                    ));
                }
                claude_config_dir = cfg;
                projects_src = Some(ps);

                auth_start = env.len();
                apply_container_auth(&mut env, crate::sandbox::docker::auth::resolve())?;
            }
            DockerProvider::Codex => {
                // Codex's config dir is bind-mounted read-write: auth.json token
                // refresh and the session rollout files it writes both need to
                // persist, and writing rollouts at the same host path keeps the
                // host-side transcript reader (`find_codex_rollouts`) working.
                let dir = codex_home_dir(ctx.home);
                // Forward CODEX_HOME only when it points somewhere other than the
                // default `~/.codex` the container already resolves via HOME —
                // mirrors `nondefault_claude_config_dir`.
                forward_codex_home = codex_home_is_nondefault(ctx.home);
                if forward_codex_home {
                    env.push(("CODEX_HOME".into(), dir.to_string_lossy().into_owned()));
                }

                auth_start = env.len();
                prepare_codex_launch(
                    &mut env,
                    &dir,
                    std::env::var("OPENAI_API_KEY").ok().as_deref(),
                )?;
                codex_config_dir = Some(dir);
            }
            DockerProvider::Opencode => {
                // OpenCode's data dir carries the accounts DB / auth.json and the
                // session storage the host transcript reader tails, so it's bound
                // read-write at its identical host path (mirrors codex's ~/.codex).
                let data = opencode_data_dir(ctx.home);
                // Forward XDG_DATA_HOME only when it points somewhere other than
                // the default `~/.local/share` the container resolves via HOME —
                // mirrors codex's CODEX_HOME handling.
                forward_xdg_data_home =
                    xdg_base_is_nondefault("XDG_DATA_HOME", ctx.home, ".local/share");
                if forward_xdg_data_home {
                    if let Some(v) = std::env::var_os("XDG_DATA_HOME") {
                        env.push(("XDG_DATA_HOME".into(), v.to_string_lossy().into_owned()));
                    }
                }
                // The config dir (custom providers + plugin installs opencode
                // writes) is optional: opencode runs without it, and binding a
                // missing source would have Docker create it root-owned. Mount +
                // forward it only when it already exists.
                let config = opencode_config_dir(ctx.home);
                if config.is_dir() {
                    forward_xdg_config_home =
                        xdg_base_is_nondefault("XDG_CONFIG_HOME", ctx.home, ".config");
                    if forward_xdg_config_home {
                        if let Some(v) = std::env::var_os("XDG_CONFIG_HOME") {
                            env.push(("XDG_CONFIG_HOME".into(), v.to_string_lossy().into_owned()));
                        }
                    }
                    oc_config = Some(config);
                }

                auth_start = env.len();
                let api_keys = present_api_keys(|n| std::env::var(n).ok());
                prepare_opencode_launch(&mut env, &data, api_keys)?;
                oc_data = Some(data);
            }
            DockerProvider::Pi => {
                // Pi keeps everything under `~/.pi` (agent/auth.json,
                // agent/settings.json, agent/sessions/), bound read-write at its
                // identical host path so auth persists and the host transcript
                // reader tails the sessions.
                let data = ctx.home.join(".pi");
                auth_start = env.len();
                let api_keys = present_api_keys(|n| std::env::var(n).ok());
                prepare_pi_launch(&mut env, &data, api_keys)?;
                pi_data = Some(data);
            }
            DockerProvider::Cursor => {
                // `~/.cursor` is bound read-write at its identical host path so
                // cursor's session transcripts land where the host reader
                // (`agent::cursor_locate`) tails them. It carries no credential
                // (the login token is keychain-bound); auth is CURSOR_API_KEY only.
                let data = ctx.home.join(".cursor");
                auth_start = env.len();
                prepare_cursor_launch(
                    &mut env,
                    &data,
                    std::env::var("CURSOR_API_KEY").ok().as_deref(),
                )?;
                cursor_data = Some(data);
            }
        }

        // Forward exactly the resolved auth var names (the tail appended after
        // `auth_start`). Scoped so the borrow of `env` ends before it moves into
        // the plan below.
        let prefix_args = {
            let auth_vars: Vec<&str> = env[auth_start..].iter().map(|(k, _)| k.as_str()).collect();
            // Assemble the provider's mount directives from the owned locals the
            // matched arm above filled in. Exactly one arm ran, so exactly one
            // variant's locals are populated.
            let mounts = match provider {
                DockerProvider::Claude => ProviderMounts::Claude {
                    config_dir: claude_config_dir.as_deref(),
                    credentials_rw: claude_credentials_rw,
                    config_dir_credentials_rw,
                    projects_src: projects_src
                        .as_deref()
                        .expect("claude launch must supply a projects_src"),
                },
                DockerProvider::Codex => ProviderMounts::Codex {
                    config_dir: codex_config_dir
                        .as_deref()
                        .expect("codex launch must supply a config_dir"),
                    forward_home: forward_codex_home,
                },
                DockerProvider::Opencode => ProviderMounts::Opencode {
                    data_dir: oc_data
                        .as_deref()
                        .expect("opencode launch must supply a data_dir"),
                    config_dir: oc_config.as_deref(),
                    forward_xdg_data_home,
                    forward_xdg_config_home,
                },
                DockerProvider::Pi => ProviderMounts::Pi {
                    data_dir: pi_data
                        .as_deref()
                        .expect("pi launch must supply a data_dir"),
                },
                DockerProvider::Cursor => ProviderMounts::Cursor {
                    data_dir: cursor_data
                        .as_deref()
                        .expect("cursor launch must supply a data_dir"),
                },
            };
            run_args(&RunSpec {
                interactive: ctx.interactive,
                name: &name,
                agent_id: ctx.agent_id,
                writable_root: ctx.writable_root,
                rpc_dir: ctx.rpc_dir,
                home: ctx.home,
                cwd: ctx.cwd,
                blackboard: ctx.blackboard,
                mounts,
                borrowed_object_stores: &borrowed_object_stores,
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
            env,
            keepalive: Keepalive::None,
            kill: KillHandle::Engine {
                engine: DockerEngine::shared(),
                plan: KillPlan::Container { name },
            },
        })
    }

    /// Tear the container down: TERM, a grace window, then KILL, then a
    /// best-effort `rm -f`. Best-effort throughout and always `Ok` — the
    /// container is usually already gone (`--rm`, daemon stopped, normal
    /// exit), and an error here would abort the caller's local process-group
    /// teardown of the docker CLI child.
    fn kill(&self, plan: &KillPlan) -> Result<()> {
        let KillPlan::Container { name } = plan;
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
