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

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use parking_lot::RwLock;

use crate::error::{Error, Result};
use crate::sandbox::engine::{
    AgentLaunchCtx, EngineKind, Keepalive, KillHandle, KillPlan, LaunchPlan, SandboxEngine,
};
use crate::sandbox::policy::{
    codex_home_dir, opencode_config_dir, opencode_data_dir, resolve_existing_prefix,
    CLAUDE_CREDENTIALS_FILE, CLAUDE_EPHEMERAL_RUNTIME_SUBDIRS, CLAUDE_PROJECTS_SUBDIR,
};

use super::auth::{self, ContainerAuth};
use super::{cleanup, cli, image, DockerProvider};

/// Settings key overriding the container image (see [`image::resolve_image`]).
pub const IMAGE_SETTING: &str = "docker_image";
/// Settings key for the container memory limit (`docker run --memory`).
pub const MEMORY_SETTING: &str = "docker_memory";
/// Settings key for the container CPU limit (`docker run --cpus`).
pub const CPUS_SETTING: &str = "docker_cpus";

const DEFAULT_MEMORY: &str = "4g";
const DEFAULT_CPUS: &str = "2";

/// The one file under a claude config dir that stays writable when the dir is
/// bind-mounted read-only: claude's OAuth refresh rewrites the rotated token
/// here, and the `CredentialsFile` auth chain (see [`super::auth`]) needs that
/// write to land on the host. Mounted read-write on top of the read-only dir.
/// The name is shared with seatbelt via [`CLAUDE_CREDENTIALS_FILE`] so
/// the two engines can't drift.
const CREDENTIALS_FILE: &str = CLAUDE_CREDENTIALS_FILE;

/// Subdirs of a claude config dir that Claude Code creates and writes *afresh
/// every session* — the per-session env store (`mkdir session-env/<id>` at
/// startup) and the shell-environment snapshot the Bash tool sources. The
/// config dir is bind-mounted read-only (invariant 5), so a bare write here
/// fails with `EROFS` and aborts the agent before it runs. Each gets an
/// ephemeral **tmpfs** overlay instead: claude writes its own per-session
/// scaffolding into throwaway container-local storage, nothing reaches the
/// shared host config, and nothing persists across runs.
///
/// Deliberately narrow — only claude-regenerated, non-config, non-executable
/// state belongs here. Everything else under the config dir stays read-only so
/// a prompt-injected agent can't plant a host-executed `hook`/`plugin`/`skill`,
/// a `settings.json` permission grant, a `CLAUDE.md` instruction, or a
/// `projects/<cwd>/memory` entry a later session would trust (invariant 5).
/// `projects/` in particular is left read-only on purpose: the RO mount already
/// lets claude *read* config and memory, and its transcript writes are
/// best-effort (a failed write logs and continues), so it needs no overlay —
/// while making it writable would reopen exactly the injection surface this
/// design closes.
///
/// Shared with seatbelt via [`CLAUDE_EPHEMERAL_RUNTIME_SUBDIRS`] (which
/// grants the real dirs as writable islands) so the two engines can't drift.
const EPHEMERAL_RUNTIME_SUBDIRS: &[&str] = CLAUDE_EPHEMERAL_RUNTIME_SUBDIRS;

/// Claude's session-transcript subdir within a config dir (`<config-dir>/
/// projects/<slug>/<uuid>.jsonl`). Unlike [`EPHEMERAL_RUNTIME_SUBDIRS`], this
/// one is bind-mounted to a *persistent* per-agent host dir (not tmpfs) so
/// `--resume` survives container recreation — see [`push_claude_config_mount`].
/// Shared with seatbelt via [`CLAUDE_PROJECTS_SUBDIR`] so the two
/// engines can't drift.
const PROJECTS_SUBDIR: &str = CLAUDE_PROJECTS_SUBDIR;

/// Signal/removal docker calls during teardown.
const KILL_TIMEOUT: Duration = Duration::from_secs(10);
/// Liveness lookups (`docker inspect`).
const INSPECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a TERM'd container gets to exit before escalating to KILL —
/// same order as the session-side process-group escalation grace windows.
const TERM_GRACE: Duration = Duration::from_millis(500);

/// Launch knobs read from the `settings` table, mirrored in-process (the spawn
/// path has no DB handle — same pattern as `sandbox::set_selected_engine_kind`).
/// Seeded at startup in `lib.rs setup` and kept in sync by the settings
/// set-commands.
#[derive(Clone, Default)]
pub struct LaunchSettings {
    /// `docker_image` — a non-empty value is used verbatim, skipping the
    /// embedded image build entirely.
    pub image_override: Option<String>,
    /// `docker_memory` — `--memory` value; `None`/blank means [`DEFAULT_MEMORY`].
    pub memory: Option<String>,
    /// `docker_cpus` — `--cpus` value; `None`/blank means [`DEFAULT_CPUS`].
    pub cpus: Option<String>,
}

static LAUNCH_SETTINGS: RwLock<LaunchSettings> = RwLock::new(LaunchSettings {
    image_override: None,
    memory: None,
    cpus: None,
});

pub fn set_launch_settings(settings: LaunchSettings) {
    *LAUNCH_SETTINGS.write() = settings;
}

/// Settings key persisting the version-refresh loop guard: a JSON object of
/// `provider id → "host_version@image_tag"`, recording the last host/image
/// pairing a version-mismatch rebuild *succeeded* for. Not a user-facing
/// setting — private bookkeeping that must survive restarts: in the guarded
/// case (host CLI pinned away from the registry's latest, so the mismatch
/// persists even after a successful rebuild) an in-memory guard would decay
/// into one full `--no-cache` rebuild on every app run. One pair per provider
/// suffices — any change to either side legitimately warrants one fresh
/// attempt.
pub const VERSION_GUARD_SETTING: &str = "docker_version_refresh_guard";

/// Writes the guard map back to its settings row (installed by
/// [`init_version_refresh_guard`]).
type VersionGuardPersist = Box<dyn Fn(&std::collections::HashMap<String, String>) + Send + Sync>;

/// The version-refresh loop guard, mirrored in-process like
/// [`LAUNCH_SETTINGS`] (the image code that consults it runs on spawn paths
/// and background threads with no DB handle). Seeded and wired to a persister
/// at startup by [`init_version_refresh_guard`]; until then (tests, headless)
/// it's empty and unpersisted, and recording still guards the current
/// process run.
struct VersionGuard {
    /// provider id → `"host_version@image_tag"` last successfully rebuilt for.
    attempted: std::collections::HashMap<String, String>,
    /// Writes the whole map back to the settings row.
    persist: Option<VersionGuardPersist>,
}

static VERSION_GUARD: RwLock<Option<VersionGuard>> = RwLock::new(None);

/// Install the loop-guard state: `attempted` as loaded from
/// [`VERSION_GUARD_SETTING`], `persist` writing it back. The app wires this
/// to a `database::set_setting` closure at startup — the same mirror idiom as
/// [`set_launch_settings`] and `progress::set_build_sink`.
pub fn init_version_refresh_guard(
    attempted: std::collections::HashMap<String, String>,
    persist: impl Fn(&std::collections::HashMap<String, String>) + Send + Sync + 'static,
) {
    *VERSION_GUARD.write() = Some(VersionGuard {
        attempted,
        persist: Some(Box::new(persist)),
    });
}

/// Whether a version-mismatch rebuild already succeeded for exactly this
/// `pair` (`"host_version@image_tag"`). If so the trigger is inert: the
/// mismatch survived a rebuild, so it isn't rebuildable-away (pinned host).
pub(super) fn version_refresh_attempted(provider_id: &str, pair: &str) -> bool {
    VERSION_GUARD
        .read()
        .as_ref()
        .is_some_and(|g| g.attempted.get(provider_id).map(String::as_str) == Some(pair))
}

/// Record (and persist, when wired) that a version-mismatch rebuild succeeded
/// for `pair`. Called from the background rebuild thread on success only —
/// failures must retry on a later run, exactly like TTL rebuild failures.
pub(super) fn record_version_refresh(provider_id: &str, pair: String) {
    let mut guard = VERSION_GUARD.write();
    let state = guard.get_or_insert_with(|| VersionGuard {
        attempted: std::collections::HashMap::new(),
        persist: None,
    });
    state.attempted.insert(provider_id.to_string(), pair);
    if let Some(persist) = &state.persist {
        persist(&state.attempted);
    }
}

/// The current `docker_image` override, trimmed, `None` when unset/blank —
/// read by the image GC (`cleanup::sweep_stale_images`) to defensively exclude
/// the user's image from removal. Structurally it should never be a candidate
/// (Fletch never builds it, so it carries no `fletch.agent` label and lives
/// outside Fletch's repos), but a lifecycle we don't own gets a second fence.
pub(super) fn image_override() -> Option<String> {
    LAUNCH_SETTINGS
        .read()
        .image_override
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

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
                apply_container_auth(&mut env, auth::resolve())?;
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

/// Launch-blocking message when the container auth chain resolves nothing.
/// Kept as one stable, matchable string the frontend keys its Settings
/// call-to-action on; the wording tells the user exactly what to do.
const NO_CONTAINER_AUTH_MSG: &str = "No Anthropic credentials for containers — open Settings → General → Sandbox and connect Claude for containers (claude setup-token).";

/// Fold the D1 auth-chain outcome ([`auth::resolve`]) into the docker CLI's
/// process env, from which [`run_args`]' bare `-e NAME` flags forward it into
/// the container (invariant 3 — values never touch argv). An [`AuthSource`] is
/// logged (the enum variant only, never a token value); [`ContainerAuth`]'s
/// `Debug` redacts values so even `?source` cannot leak one. When the chain
/// yields nothing the launch fails fast with [`NO_CONTAINER_AUTH_MSG`].
///
/// [`AuthSource`]: super::auth::AuthSource
fn apply_container_auth(env: &mut Vec<(String, String)>, auth: ContainerAuth) -> Result<()> {
    match auth {
        ContainerAuth::Resolved {
            env: auth_env,
            source,
        } => {
            tracing::info!(target: "fletch::docker", ?source, "container auth resolved");
            env.extend(auth_env);
            Ok(())
        }
        ContainerAuth::Unavailable => Err(Error::Other(NO_CONTAINER_AUTH_MSG.to_string())),
    }
}

/// Launch-blocking message when codex has no usable credential: no
/// `auth.json` in its config dir and `OPENAI_API_KEY` unset. Mirrors
/// [`NO_CONTAINER_AUTH_MSG`]'s fail-fast: an unauthenticated container boots
/// straight into a login prompt it can't answer inside the sandbox.
const NO_CODEX_AUTH_MSG: &str =
    "No Codex credentials for containers — sign in with `codex` on the host (writes ~/.codex/auth.json) or set OPENAI_API_KEY.";

/// Launch-blocking message when opencode has no usable credential: no accounts DB
/// / auth.json on its data-dir mount and no known provider API key set. Same
/// fail-fast rationale as [`NO_CODEX_AUTH_MSG`].
const NO_OPENCODE_AUTH_MSG: &str =
    "No OpenCode credentials for containers — sign in with `opencode auth login` on the host or set a provider API key (e.g. ANTHROPIC_API_KEY or OPENAI_API_KEY).";

/// Launch-blocking message when pi has no usable credential: no
/// `~/.pi/agent/auth.json` on its mount and no known provider API key set.
const NO_PI_AUTH_MSG: &str =
    "No Pi credentials for containers — sign in with `pi` on the host (writes ~/.pi/agent/auth.json) or set a provider API key (e.g. ANTHROPIC_API_KEY or OPENAI_API_KEY).";

/// Launch-blocking message when cursor has no usable credential. Unlike the other
/// providers there is no mount-based fallback: `cursor-agent login` stores its
/// access/refresh tokens in the host OS keychain (macOS "Cursor Safe Storage"),
/// which a Linux container can't read, and `~/.cursor` carries only identity
/// metadata — not a bearer token. So `CURSOR_API_KEY` is the sole container
/// credential; fail fast (before touching the filesystem) when it's unset.
const NO_CURSOR_AUTH_MSG: &str =
    "No Cursor credentials for containers — set CURSOR_API_KEY (create one at cursor.com/dashboard). `cursor-agent login` stores its token in the host keychain, which containers can't read.";

/// Provider API-key env vars the multi-provider CLIs (opencode, pi) read to
/// authenticate. Whichever are set in the app's process env are forwarded by bare
/// `-e NAME` (invariant 3) so the in-container CLI can use them, and a set key
/// satisfies the auth requirement on its own. Curated to the mainstream providers
/// both CLIs honor (verified against each CLI's binary) — not exhaustive, but
/// enough that a user with any common key set can launch. Codex is excluded: it's
/// single-provider (OpenAI) and resolved separately.
const MULTI_PROVIDER_API_KEY_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "GEMINI_API_KEY",
    "GROQ_API_KEY",
    "XAI_API_KEY",
    "DEEPSEEK_API_KEY",
    "MISTRAL_API_KEY",
];

/// Whether `$CODEX_HOME` is set to a dir other than the default `~/.codex`
/// (which the container already resolves via `HOME`). Only a non-default value
/// is forwarded, mirroring [`nondefault_claude_config_dir`]; both sides go
/// through [`resolve_existing_prefix`] so a symlink can't read as non-default.
/// Blank counts as unset, matching [`codex_home_dir`]'s resolution —
/// forwarding a blank value the resolver ignored would desync the two.
fn codex_home_is_nondefault(home: &Path) -> bool {
    match std::env::var_os("CODEX_HOME") {
        Some(v) if !v.is_empty() => {
            resolve_existing_prefix(&PathBuf::from(v))
                != resolve_existing_prefix(&home.join(".codex"))
        }
        _ => false,
    }
}

// Codex's `$CODEX_HOME` resolution (`codex_home_dir`) and opencode's data/
// config dir resolution (`opencode_data_dir`, `opencode_config_dir`, their
// shared `xdg_base`) now live in [`crate::sandbox::policy`] — they're class-1
// host-persistence dirs both engines share (Docker mounts them; seatbelt
// grants them), so the policy module is their single source of truth.
// Imported at the top of this file.

/// Whether `$var` points to an XDG base other than the default `home/<default_rel>`
/// the container already resolves via `HOME`. Only a non-default base is forwarded,
/// mirroring [`codex_home_is_nondefault`]; both sides canonicalize via
/// [`resolve_existing_prefix`] so a symlink can't read as non-default. This stays
/// docker-local: it's launch-time env-forwarding logic (does the container need a
/// `-e XDG_*`?), not a write-policy question.
fn xdg_base_is_nondefault(var: &str, home: &Path, default_rel: &str) -> bool {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => {
            resolve_existing_prefix(&PathBuf::from(v))
                != resolve_existing_prefix(&home.join(default_rel))
        }
        _ => false,
    }
}

/// Fold codex's container auth into the docker CLI's process env, then make
/// sure the config dir exists so the read-write bind has a host source. Codex's
/// primary credential is the mounted `~/.codex/auth.json` (the read-write mount
/// carries it and token refresh persists to the host); an `OPENAI_API_KEY` in
/// the app's process env is forwarded when set (by bare `-e`, so its value never
/// touches argv — invariant 3). Either alone suffices: a key-only user may
/// never have run codex on the host, so the dir is created rather than
/// required — mounting a fresh dir keeps session rollouts landing where
/// `find_codex_rollouts` reads them. Fails the launch when neither credential
/// is present (before touching the filesystem), so an unauthenticated
/// container never boots into an unanswerable login prompt.
///
/// Unlike claude, no ANTHROPIC_*/CLAUDE_* var is injected: codex authenticates
/// against OpenAI, and forwarding those would be dead weight at best.
fn prepare_codex_launch(
    env: &mut Vec<(String, String)>,
    config_dir: &Path,
    api_key: Option<&str>,
) -> Result<()> {
    let auth_file = config_dir.join("auth.json").is_file();
    let resolved = codex_auth_env(api_key, auth_file)?;
    // Booleans only — never a token value.
    tracing::info!(
        target: "fletch::docker",
        auth_file,
        api_key = !resolved.is_empty(),
        "codex container auth resolved"
    );
    env.extend(resolved);
    std::fs::create_dir_all(config_dir).map_err(|e| {
        Error::Other(format!(
            "Couldn't create Codex config dir {}: {e}",
            config_dir.display()
        ))
    })?;
    Ok(())
}

/// Pure core of [`prepare_codex_launch`]: the auth env to forward given the process
/// `OPENAI_API_KEY` (if any) and whether `auth.json` exists on the mount. A
/// non-blank key is forwarded (trimmed); the mounted `auth.json` carries auth on
/// its own with nothing to inject. Neither present → the launch-blocking error.
fn codex_auth_env(api_key: Option<&str>, auth_file: bool) -> Result<Vec<(String, String)>> {
    let api_key = api_key.map(str::trim).filter(|k| !k.is_empty());
    if let Some(key) = api_key {
        return Ok(vec![("OPENAI_API_KEY".to_string(), key.to_string())]);
    }
    if auth_file {
        return Ok(Vec::new());
    }
    Err(Error::Other(NO_CODEX_AUTH_MSG.to_string()))
}

/// The subset of [`MULTI_PROVIDER_API_KEY_ENV`] present and non-blank via
/// `lookup` (a var name → value resolver, `std::env::var` in production, a fixture
/// in tests), each as a `(name, value)` to forward. Order follows the constant so
/// forwarding is deterministic.
fn present_api_keys(lookup: impl Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    MULTI_PROVIDER_API_KEY_ENV
        .iter()
        .filter_map(|&name| {
            let value = lookup(name)?;
            let value = value.trim();
            (!value.is_empty()).then(|| (name.to_string(), value.to_string()))
        })
        .collect()
}

/// Shared auth rule for the multi-provider CLIs (opencode, pi): a forwarded
/// provider key OR a credential carried on the read-write mount suffices; neither
/// present → the caller's launch-blocking message. Returns the keys to forward
/// (empty when the mount carries the login and no key is set), mirroring
/// [`codex_auth_env`]'s shape.
fn multi_provider_auth_env(
    api_keys: Vec<(String, String)>,
    credential_on_mount: bool,
    no_auth_msg: &str,
) -> Result<Vec<(String, String)>> {
    if !api_keys.is_empty() || credential_on_mount {
        Ok(api_keys)
    } else {
        Err(Error::Other(no_auth_msg.to_string()))
    }
}

/// Fold opencode's container auth into the docker CLI's process env, then make
/// sure the data dir exists so the read-write bind has a host source. OpenCode's
/// login lives in its data-dir mount (the accounts DB `opencode.db`, or a legacy
/// `auth.json`); a provider API key in the app's env is forwarded when set. Either
/// alone suffices, so — like codex — a key-only user who never ran opencode gets
/// the dir created rather than required. Neither present → fail the launch before
/// touching the filesystem. Auth values ride the process env and forward by bare
/// `-e` (invariant 3); only booleans are logged.
fn prepare_opencode_launch(
    env: &mut Vec<(String, String)>,
    data_dir: &Path,
    api_keys: Vec<(String, String)>,
) -> Result<()> {
    let auth_file = data_dir.join("auth.json").is_file();
    let auth_db = data_dir.join("opencode.db").is_file();
    let has_keys = !api_keys.is_empty();
    let resolved = multi_provider_auth_env(api_keys, auth_file || auth_db, NO_OPENCODE_AUTH_MSG)?;
    tracing::info!(
        target: "fletch::docker",
        auth_file,
        auth_db,
        api_keys = has_keys,
        "opencode container auth resolved"
    );
    env.extend(resolved);
    std::fs::create_dir_all(data_dir).map_err(|e| {
        Error::Other(format!(
            "Couldn't create OpenCode data dir {}: {e}",
            data_dir.display()
        ))
    })?;
    Ok(())
}

/// Fold pi's container auth into the docker CLI's process env, then make sure
/// `~/.pi` exists so the read-write bind has a host source. Pi's login lives in
/// `~/.pi/agent/auth.json` on that mount; a provider API key in the app's env is
/// forwarded when set. Either alone suffices, so a key-only user who never ran pi
/// gets `~/.pi` created rather than required. Neither present → fail the launch
/// before touching the filesystem. Only booleans are logged.
fn prepare_pi_launch(
    env: &mut Vec<(String, String)>,
    data_dir: &Path,
    api_keys: Vec<(String, String)>,
) -> Result<()> {
    let auth_file = data_dir.join("agent/auth.json").is_file();
    let has_keys = !api_keys.is_empty();
    let resolved = multi_provider_auth_env(api_keys, auth_file, NO_PI_AUTH_MSG)?;
    tracing::info!(
        target: "fletch::docker",
        auth_file,
        api_keys = has_keys,
        "pi container auth resolved"
    );
    env.extend(resolved);
    std::fs::create_dir_all(data_dir).map_err(|e| {
        Error::Other(format!(
            "Couldn't create Pi data dir {}: {e}",
            data_dir.display()
        ))
    })?;
    Ok(())
}

/// Fold cursor's container auth into the docker CLI's process env, then make sure
/// `~/.cursor` exists so the read-write bind has a host source. Cursor is a
/// single-provider CLI, so — like codex — no cross-provider key set applies;
/// unlike every other provider, though, its credential can't ride the mount:
/// `cursor-agent login` writes its tokens to the host OS keychain (see
/// [`NO_CURSOR_AUTH_MSG`]), so `CURSOR_API_KEY` (forwarded by bare `-e` — invariant
/// 3) is the only container credential. The `~/.cursor` mount still matters: it's
/// where cursor writes session transcripts (`agent::cursor_locate` reads them at
/// the identical host path), so the dir is created for the bind even though it
/// carries no auth. Fails the launch when `CURSOR_API_KEY` is unset, before
/// touching the filesystem. Only a boolean is logged, never the key.
fn prepare_cursor_launch(
    env: &mut Vec<(String, String)>,
    config_dir: &Path,
    api_key: Option<&str>,
) -> Result<()> {
    let resolved = cursor_auth_env(api_key)?;
    tracing::info!(
        target: "fletch::docker",
        api_key = !resolved.is_empty(),
        "cursor container auth resolved"
    );
    env.extend(resolved);
    std::fs::create_dir_all(config_dir).map_err(|e| {
        Error::Other(format!(
            "Couldn't create Cursor config dir {}: {e}",
            config_dir.display()
        ))
    })?;
    Ok(())
}

/// Pure core of [`prepare_cursor_launch`]: the auth env to forward given the
/// process `CURSOR_API_KEY`. A non-blank key is forwarded (trimmed); anything
/// else — unset or blank — is the launch-blocking error, because cursor's login
/// token lives in the host keychain and can't reach the container by any other
/// path (see [`NO_CURSOR_AUTH_MSG`]).
fn cursor_auth_env(api_key: Option<&str>) -> Result<Vec<(String, String)>> {
    match api_key.map(str::trim).filter(|k| !k.is_empty()) {
        Some(key) => Ok(vec![("CURSOR_API_KEY".to_string(), key.to_string())]),
        None => Err(Error::Other(NO_CURSOR_AUTH_MSG.to_string())),
    }
}

/// `Some(v)` only when `v` is present and non-blank — settings rows can hold
/// empty strings, which must fall back to defaults.
fn non_blank(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

/// A non-default `CLAUDE_CONFIG_DIR` from the app environment, mounted and
/// forwarded so claude writes its config/transcripts/auth where the host
/// expects them. `None` when unset or when it resolves to the default
/// `~/.claude` (already mounted).
///
/// The default check canonicalizes *both* sides via [`resolve_existing_prefix`],
/// so a symlink or trailing-slash in the config dir or the home path can't make
/// a dir that really points at `~/.claude` read as non-default (a redundant
/// mount + `CLAUDE_CONFIG_DIR` forward). Canonicalizing both sides is safe here
/// — unlike seatbelt's literal-path SBPL allow-list, which compares against the
/// *raw* default — because the default `~/.claude` bind mount follows its
/// symlink source, so a config dir pointing at the resolved target is still
/// covered by that mount. The *original* path is returned for a genuinely
/// non-default dir, so the mount/forward stay at the host path (invariant 1).
fn nondefault_claude_config_dir(home: &Path) -> Option<PathBuf> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from)?;
    (!config_dir_is_default(&dir, home)).then_some(dir)
}

/// Whether `dir` resolves to the default `~/.claude`. Both sides go through
/// [`resolve_existing_prefix`] — see [`nondefault_claude_config_dir`] for why.
/// Pure over its inputs so the comparison rule is directly testable.
fn config_dir_is_default(dir: &Path, home: &Path) -> bool {
    resolve_existing_prefix(dir) == resolve_existing_prefix(&home.join(".claude"))
}

/// Every object store borrowed via git alternates by any checkout under the
/// agent's `writable_root` — each an absolute path to mount read-only.
///
/// `writable_root` is the agent's parent dir, holding one checkout per tracked
/// repo at `<root>/<subdir>/`. Each `--shared` clone records its source's
/// objects in `<subdir>/.git/objects/info/alternates`; a multi-repo agent has
/// several, so scanning only the primary `cwd` would leave secondary checkouts'
/// borrowed objects unmounted and break git (log/diff/checkout/commit) there.
///
/// For each checkout the chain is followed transitively: `git clone --shared`
/// records only the immediate source, so a chained source (B borrowed from A)
/// leaves the checkout pointing at B while git resolves B→A at runtime — A must
/// be mounted too or in-container git fails to normalize the alternate. Results
/// are deduped (repos may share a base). No alternates anywhere (old full-copy
/// clones, worktrees) → empty, so no extra mount is added — backward
/// compatible. Reading the files rather than reconstructing paths keeps fresh
/// spawn, resume, and view-switch uniform.
fn borrowed_object_stores(writable_root: &Path) -> Vec<PathBuf> {
    /// The alternates listed in `<objects_dir>/info/alternates`, if any.
    fn read_alternates(objects_dir: &Path) -> Vec<PathBuf> {
        let Ok(contents) = std::fs::read_to_string(objects_dir.join("info/alternates")) else {
            return Vec::new();
        };
        contents
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(PathBuf::from)
            .collect()
    }

    // Seed the chain walk from every checkout's own object store. Sort the
    // subdirs so the mount order is deterministic (read_dir order isn't).
    let mut checkouts: Vec<PathBuf> = match std::fs::read_dir(writable_root) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect(),
        Err(_) => Vec::new(),
    };
    checkouts.sort();

    let mut out: Vec<PathBuf> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    // BFS over the alternates chains: each checkout's own alternates first (in
    // file order), then each borrowed store's own. `seen` dedups shared bases
    // and guards against a cyclic alternates chain.
    let mut queue: std::collections::VecDeque<PathBuf> = checkouts
        .iter()
        .flat_map(|c| read_alternates(&c.join(".git/objects")))
        .collect();
    while let Some(store) = queue.pop_front() {
        if !seen.insert(store.clone()) {
            continue;
        }
        for next in read_alternates(&store) {
            queue.push_back(next);
        }
        out.push(store);
    }
    out
}

/// The provider-specific config/data mounts and config-dir env a launch needs —
/// one variant per supported provider. Replaces the additive per-provider field
/// clusters `RunSpec` used to carry (claude_config_dir, codex_config_dir, …),
/// which grew unwieldy at four providers with three-quarters of them inert on any
/// given launch. Each variant holds exactly its own provider's data and
/// [`run_args`] matches once on the whole thing. Claude's read-only-except-
/// carve-outs treatment is unique; the other three are read-write binds at
/// identical host paths, differing only in which dirs they mount and which env
/// var (if any) they forward.
enum ProviderMounts<'a> {
    /// Claude: `~/.claude` (and any non-default `CLAUDE_CONFIG_DIR`) bind-mounted
    /// **read-only** except a writable `.credentials.json` overlay and the
    /// per-agent `projects/` transcript bind (invariant 5); see
    /// [`push_claude_config_mount`].
    Claude {
        /// Non-default `CLAUDE_CONFIG_DIR`, mounted + forwarded alongside
        /// `~/.claude`. `None` when unset or resolving to the default.
        config_dir: Option<&'a Path>,
        /// Whether `~/.claude/.credentials.json` exists — gates its RW overlay.
        credentials_rw: bool,
        /// Same, for the non-default `CLAUDE_CONFIG_DIR` (meaningful only when
        /// `config_dir` is `Some`).
        config_dir_credentials_rw: bool,
        /// Per-agent host dir (under `writable_root`) bound read-write over each
        /// config dir's `projects/` so the session transcript survives container
        /// recreation while the shared `~/.claude` stays read-only.
        projects_src: &'a Path,
    },
    /// Codex: `$CODEX_HOME`/`~/.codex` bind-mounted **read-write** at its host
    /// path (auth.json refresh + session rollouts must persist).
    Codex {
        config_dir: &'a Path,
        /// Forward `CODEX_HOME` (a non-default `$CODEX_HOME` only).
        forward_home: bool,
    },
    /// OpenCode: its data dir (`$XDG_DATA_HOME/opencode` else
    /// `~/.local/share/opencode`) bind-mounted **read-write** — it carries the
    /// accounts DB / `auth.json` and the session storage the host transcript
    /// reader tails — plus its config dir (`$XDG_CONFIG_HOME/opencode` else
    /// `~/.config/opencode`) when it exists (custom providers + plugin installs,
    /// which opencode writes → also RW).
    Opencode {
        data_dir: &'a Path,
        config_dir: Option<&'a Path>,
        /// Forward `XDG_DATA_HOME` / `XDG_CONFIG_HOME` (non-default bases only).
        forward_xdg_data_home: bool,
        forward_xdg_config_home: bool,
    },
    /// Pi: `~/.pi` bind-mounted **read-write** — it holds `agent/auth.json`,
    /// `agent/settings.json`, and the `agent/sessions/` transcripts the host
    /// reader tails at the identical path.
    Pi { data_dir: &'a Path },
    /// Cursor: `~/.cursor` bind-mounted **read-write** — it holds the
    /// `projects/<slug>/agent-transcripts/` session logs the host reader tails at
    /// the identical path. Carries no credential (cursor's login token is
    /// keychain-bound); auth is the forwarded `CURSOR_API_KEY` only.
    Cursor { data_dir: &'a Path },
}

/// Everything [`run_args`] needs, bundled so the builder is pure and the argv
/// shape unit-testable without a daemon.
struct RunSpec<'a> {
    interactive: bool,
    name: &'a str,
    agent_id: &'a str,
    writable_root: &'a Path,
    rpc_dir: &'a Path,
    home: &'a Path,
    cwd: &'a Path,
    /// A workflow step agent's blackboard directory, bind-mounted read-write at
    /// its identical host path and forwarded as `WF_BLACKBOARD`. `None` for
    /// ordinary agents.
    blackboard: Option<&'a Path>,
    /// The launching provider's config/data mounts + config-dir env forwards.
    mounts: ProviderMounts<'a>,
    /// Object stores borrowed via git alternates (a `--shared` clone),
    /// bind-mounted read-only at their identical host paths. Empty for a
    /// worktree or an old full-copy clone.
    borrowed_object_stores: &'a [PathBuf],
    memory: &'a str,
    cpus: &'a str,
    image: &'a str,
    agent_bin: &'a str,
    /// Auth var *names* the chain resolved ([`auth::resolve`]), each forwarded
    /// with a bare `-e NAME` so its value (set on the docker CLI process env)
    /// never appears in argv. Only the resolved set is forwarded: an ambient
    /// credential the chain didn't pick must not reach the container and
    /// override the resolved login.
    auth_vars: &'a [&'a str],
}

/// The `docker run` argv (everything after the docker binary), ending with
/// `<image> <agent_bin>` so the caller can append agent CLI args — the
/// `prefix_args` contract of [`SandboxEngine::launch_agent`].
fn run_args(spec: &RunSpec<'_>) -> Vec<String> {
    let mut args: Vec<String> = vec!["run".into(), "--rm".into(), "--init".into()];
    if spec.interactive {
        args.push("-t".into());
    }
    args.push("-i".into());
    args.push("--name".into());
    args.push(spec.name.into());
    args.push("--label".into());
    args.push(cleanup::host_pid_label());
    args.push("--label".into());
    args.push(cleanup::agent_id_label(spec.agent_id));
    // Mounts at identical host paths (invariant 1). Exactly these — nothing
    // else from the host enters the container. The workspace and RPC mailbox
    // are read-write; the claude config dir(s) are read-only except their
    // `.credentials.json` (invariant 5) — see [`push_claude_config_mount`].
    for path in [spec.writable_root, spec.rpc_dir] {
        let path = path.to_string_lossy();
        args.push("-v".into());
        args.push(format!("{path}:{path}"));
    }
    // A workflow step agent's blackboard, bind-mounted read-write at its
    // identical host path (invariant 1) so `$WF_BLACKBOARD` resolves the same
    // in-container as on the host — the same shape as the RPC mailbox mount.
    if let Some(board) = spec.blackboard {
        push_rw_bind(&mut args, board);
    }
    // Object stores borrowed by a --shared clone, mounted read-only at their
    // identical host path so the alternates file resolves in-container with no
    // rewriting. RO keeps invariant 2: borrowed history is readable, but the
    // source store (and, since we mount only `objects`, never `.git/hooks` or
    // config) can't be mutated from inside the container. The list already
    // includes any transitively-chained stores (see `borrowed_object_stores`).
    for store in spec.borrowed_object_stores {
        let path = store.to_string_lossy();
        args.push("-v".into());
        args.push(format!("{path}:{path}:ro"));
    }
    // Provider config-dir mount(s), layered after the workspace/mailbox/object
    // stores. Claude gets the read-only-except-carve-outs treatment; the other
    // three get read-write binds at their identical host paths (see the
    // `ProviderMounts` variant docs).
    match &spec.mounts {
        ProviderMounts::Claude {
            config_dir,
            credentials_rw,
            config_dir_credentials_rw,
            projects_src,
        } => {
            push_claude_config_mount(
                &mut args,
                &spec.home.join(".claude"),
                *credentials_rw,
                projects_src,
            );
            if let Some(dir) = config_dir {
                push_claude_config_mount(&mut args, dir, *config_dir_credentials_rw, projects_src);
            }
        }
        ProviderMounts::Codex { config_dir, .. } => push_rw_bind(&mut args, config_dir),
        ProviderMounts::Opencode {
            data_dir,
            config_dir,
            ..
        } => {
            push_rw_bind(&mut args, data_dir);
            if let Some(dir) = config_dir {
                push_rw_bind(&mut args, dir);
            }
        }
        ProviderMounts::Pi { data_dir } => push_rw_bind(&mut args, data_dir),
        ProviderMounts::Cursor { data_dir } => push_rw_bind(&mut args, data_dir),
    }
    args.push("-w".into());
    args.push(spec.cwd.to_string_lossy().into_owned());
    // Bare `-e NAME` forwards from the docker CLI's own environment without
    // the value ever appearing in argv (invariant 3 for the auth vars). Auth
    // vars come from `spec.auth_vars` — the set the chain actually resolved.
    let mut forwarded: Vec<&str> = vec!["HOME", "FLETCH_RPC_DIR", "TERM", "COLORTERM"];
    if spec.blackboard.is_some() {
        forwarded.push(crate::workflow::blackboard::WF_BLACKBOARD_ENV);
    }
    match &spec.mounts {
        ProviderMounts::Claude { config_dir, .. } => {
            if config_dir.is_some() {
                forwarded.push("CLAUDE_CONFIG_DIR");
            }
        }
        ProviderMounts::Codex { forward_home, .. } => {
            if *forward_home {
                forwarded.push("CODEX_HOME");
            }
        }
        ProviderMounts::Opencode {
            forward_xdg_data_home,
            forward_xdg_config_home,
            ..
        } => {
            if *forward_xdg_data_home {
                forwarded.push("XDG_DATA_HOME");
            }
            if *forward_xdg_config_home {
                forwarded.push("XDG_CONFIG_HOME");
            }
        }
        ProviderMounts::Pi { .. } => {}
        // Cursor forwards no config-dir env; its sole auth var (CURSOR_API_KEY)
        // rides `spec.auth_vars` like every other resolved credential.
        ProviderMounts::Cursor { .. } => {}
    }
    forwarded.extend(spec.auth_vars.iter().copied());
    for var in forwarded {
        args.push("-e".into());
        args.push(var.into());
    }
    args.push("--memory".into());
    args.push(spec.memory.into());
    args.push("--cpus".into());
    args.push(spec.cpus.into());
    args.push(spec.image.into());
    args.push(spec.agent_bin.into());
    args
}

/// Bind-mount `dir` **read-write** at its identical host path (no `:ro`). The
/// shape codex/opencode/pi share: the CLI refreshes its auth and writes session
/// state in place, and writing at the host path is what keeps the host-side
/// transcript reader working (invariant 1). Unlike claude's config mount there
/// are no read-only carve-outs — the user accepted a full read-write config dir
/// for these self-authenticating CLIs (same call as codex's `~/.codex`).
fn push_rw_bind(args: &mut Vec<String>, dir: &Path) {
    let path = dir.to_string_lossy();
    args.push("-v".into());
    args.push(format!("{path}:{path}"));
}

/// Create a claude config dir and its overlay mountpoints before it's handed to
/// `-v`. The dir itself must exist or Docker would materialize it root-owned;
/// each overlay target must exist *inside* it too, because the overlay
/// ([`push_claude_config_mount`]) mounts onto that subpath and the RO parent
/// bind can't grow a fresh mountpoint at run time. The overlays are the
/// [`EPHEMERAL_RUNTIME_SUBDIRS`] tmpfs targets plus `projects/` (the read-write
/// per-agent transcript bind). Creating empty dirs is harmless — the overlay
/// shadows each, so nothing the agent writes there lands on this shared dir.
fn prepare_config_mount_dir(dir: &Path) -> Result<()> {
    let overlays = EPHEMERAL_RUNTIME_SUBDIRS
        .iter()
        .copied()
        .chain(std::iter::once(PROJECTS_SUBDIR));
    for target in std::iter::once(dir.to_path_buf()).chain(overlays.map(|s| dir.join(s))) {
        std::fs::create_dir_all(&target).map_err(|e| {
            Error::Other(format!(
                "preparing Docker sandbox config mount {} failed: {e}",
                target.display()
            ))
        })?;
    }
    Ok(())
}

/// Bind-mount a claude config dir **read-only**, then layer the writable
/// exceptions on top. The dir is shared host state whose `settings.json` can
/// define hooks Claude Code executes on the host, so a container agent must not
/// be able to write it (invariant 5); these are the sole exceptions, and each is
/// deliberately either the host credential file, throwaway container-local
/// storage, or a per-agent host dir that never touches the shared config:
///
/// - `.credentials.json` (when `credentials_rw`) — remounted read-write so
///   claude's OAuth token refresh persists to the host. Skipped when the file
///   is absent (a bare `-v` on a missing source would have Docker create a
///   root-owned dir there).
/// - each [`EPHEMERAL_RUNTIME_SUBDIRS`] entry (`session-env`, `shell-snapshots`)
///   — an ephemeral tmpfs so claude's per-session scaffolding can be written
///   without a bare write to the RO dir failing with `EROFS`. Needs no source.
/// - `projects/` — `projects_src` (a per-agent host dir under `writable_root`)
///   bound read-write over it, so claude's session transcript persists across
///   container recreation (`--resume` after an app relaunch) while the shared
///   `~/.claude/projects` — other agents' transcripts, global memory — stays
///   unreadable and unwritable. This is a *non-identical-path* bind: it departs
///   from invariant 1's identical-host-path rule, which `projects/` doesn't need
///   since claude references it only through its config dir.
///
/// Every overlay is pushed *after* the RO dir mount so Docker layers it on top.
fn push_claude_config_mount(
    args: &mut Vec<String>,
    dir: &Path,
    credentials_rw: bool,
    projects_src: &Path,
) {
    let path = dir.to_string_lossy();
    args.push("-v".into());
    args.push(format!("{path}:{path}:ro"));
    if credentials_rw {
        let creds = dir.join(CREDENTIALS_FILE);
        let creds = creds.to_string_lossy();
        args.push("-v".into());
        args.push(format!("{creds}:{creds}"));
    }
    let projects_target = dir.join(PROJECTS_SUBDIR);
    args.push("-v".into());
    args.push(format!(
        "{}:{}",
        projects_src.to_string_lossy(),
        projects_target.to_string_lossy()
    ));
    for sub in EPHEMERAL_RUNTIME_SUBDIRS {
        args.push("--tmpfs".into());
        args.push(dir.join(sub).to_string_lossy().into_owned());
    }
}

/// `fletch-<agent_id>-<8-char nonce>`. The nonce keeps respawns (view switch,
/// binary swap) from colliding with a predecessor container of the same agent
/// that `--rm` hasn't finished reaping yet; hashing in the pid keeps two
/// side-by-side Fletch instances apart even for a same-named agent.
fn container_name(agent_id: &str) -> String {
    // Docker names must match [a-zA-Z0-9][a-zA-Z0-9_.-]*; the `fletch-`
    // prefix fixes the first char, sanitize the rest.
    let sanitized: String = agent_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("fletch-{sanitized}-{}", nonce())
}

/// 8 hex chars from (pid, monotonic counter): unique within a host across
/// concurrently running instances for the lifetime of any one container.
fn nonce() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::process::id().hash(&mut hasher);
    COUNTER.fetch_add(1, Ordering::Relaxed).hash(&mut hasher);
    let hex: String = format!("{:016x}", hasher.finish());
    hex[..8].to_string()
}

/// Whether the daemon says the container is currently running. Errors
/// (container gone, daemon down, timeout) read as not running.
fn container_running(name: &str) -> bool {
    match cli::run_docker(
        &["inspect", "-f", "{{.State.Running}}", name],
        INSPECT_TIMEOUT,
    ) {
        Ok(out) => out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true",
        Err(e) => {
            tracing::debug!(container = %name, error = %e, "docker inspect failed; treating as dead");
            false
        }
    }
}

/// Poll until the container stops running or `budget` elapses.
fn container_gone_within(name: &str, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    loop {
        if !container_running(name) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// User-readable meanings for the docker CLI's reserved exit codes; other
/// codes are the contained agent's own and pass through unmapped. `docker run`
/// relays the agent's own exit status, so an agent that starts fine and later
/// exits 125/126/127 is indistinguishable from a launcher/image failure — the
/// messages name the likely Docker-layer cause but flag the agent-exit
/// possibility so they don't mislead when the container did launch.
///
/// Provider-neutral: the teardown plan carries only the container name, and the
/// sandbox image now varies per provider, so these speak of "the agent binary"
/// rather than naming `claude`.
fn describe_exit_code(code: i32) -> Option<String> {
    let msg = match code {
        125 => "Exit 125: Docker could not start the sandbox container — the daemon reported an error (or the agent itself exited 125). Is Docker Desktop still running?",
        126 => "Exit 126: the agent binary in the sandbox image is present but not runnable (or the agent itself exited 126). If you set a custom docker_image, check its agent CLI.",
        127 => "Exit 127: no agent binary on the sandbox image's PATH (or the agent itself exited 127). A custom docker_image must include the launching agent's CLI.",
        _ => return None,
    };
    Some(msg.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The version-refresh loop guard: exact-pair matching, per-provider
    /// isolation, persistence callback on record, and safe recording before
    /// `init` is ever called. Touches the process-wide `VERSION_GUARD`
    /// static — the only test that does, so no serialization needed (the
    /// same shared-global contract as `progress`'s sink test).
    #[test]
    fn version_refresh_guard_round_trip() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        // Pre-init (headless/tests): nothing attempted, recording is safe and
        // guards the current process even without a persister.
        assert!(!version_refresh_attempted("claude", "v1@fletch-agent:aaa"));
        record_version_refresh("claude", "v1@fletch-agent:aaa".into());
        assert!(version_refresh_attempted("claude", "v1@fletch-agent:aaa"));

        // Init replaces state wholesale (the app seeds from the settings row).
        let persisted = Arc::new(AtomicUsize::new(0));
        let count = persisted.clone();
        init_version_refresh_guard(
            [("codex".to_string(), "v2@fletch-agent-codex:bbb".to_string())].into(),
            move |map| {
                count.store(map.len(), Ordering::SeqCst);
            },
        );
        assert!(version_refresh_attempted(
            "codex",
            "v2@fletch-agent-codex:bbb"
        ));
        // Exact pair only: a new host version or a new tag re-arms the trigger.
        assert!(!version_refresh_attempted(
            "codex",
            "v3@fletch-agent-codex:bbb"
        ));
        assert!(!version_refresh_attempted(
            "codex",
            "v2@fletch-agent-codex:ccc"
        ));
        // Per-provider isolation.
        assert!(!version_refresh_attempted(
            "claude",
            "v2@fletch-agent-codex:bbb"
        ));

        // Recording persists the whole map through the installed callback,
        // and one pair per provider suffices (newer replaces older).
        record_version_refresh("claude", "v9@fletch-agent:ddd".into());
        assert_eq!(
            persisted.load(Ordering::SeqCst),
            2,
            "persister sees both providers"
        );
        record_version_refresh("claude", "v10@fletch-agent:ddd".into());
        assert!(version_refresh_attempted("claude", "v10@fletch-agent:ddd"));
        assert!(!version_refresh_attempted("claude", "v9@fletch-agent:ddd"));
        assert_eq!(
            persisted.load(Ordering::SeqCst),
            2,
            "replaced, not accumulated"
        );
    }

    /// The per-agent claude transcript dir every claude spec shares.
    const CLAUDE_PROJECTS_SRC: &str = "/Users/u/.fletch/worktrees/orkney/.fletch-claude-projects";

    /// Claude mount directives with the two carve-out knobs the argv tests flex.
    fn claude_mounts<'a>(
        config_dir: Option<&'a Path>,
        credentials_rw: bool,
        config_dir_credentials_rw: bool,
    ) -> ProviderMounts<'a> {
        ProviderMounts::Claude {
            config_dir,
            credentials_rw,
            config_dir_credentials_rw,
            projects_src: Path::new(CLAUDE_PROJECTS_SRC),
        }
    }

    fn test_spec<'a>(interactive: bool) -> RunSpec<'a> {
        RunSpec {
            interactive,
            name: "fletch-orkney-deadbeef",
            agent_id: "orkney",
            writable_root: Path::new("/Users/u/.fletch/worktrees/orkney"),
            rpc_dir: Path::new("/Users/u/.fletch/rpc/orkney"),
            home: Path::new("/Users/u"),
            cwd: Path::new("/Users/u/.fletch/worktrees/orkney/repo"),
            blackboard: None,
            mounts: claude_mounts(None, false, false),
            borrowed_object_stores: &[],
            memory: "4g",
            cpus: "2",
            image: "fletch-agent:abc123def456",
            agent_bin: "claude",
            auth_vars: &[
                "ANTHROPIC_API_KEY",
                "CLAUDE_CODE_OAUTH_TOKEN",
                "ANTHROPIC_BASE_URL",
                "ANTHROPIC_AUTH_TOKEN",
            ],
        }
    }

    /// A `RunSpec` for a read-write-config provider (codex/opencode/pi): no claude
    /// config surface, `mounts`/`image`/`agent_bin`/`auth_vars` supplied by the
    /// caller. Keeps the codex/opencode/pi argv tests to their own differences.
    fn rw_config_spec<'a>(
        mounts: ProviderMounts<'a>,
        image: &'a str,
        agent_bin: &'a str,
        auth_vars: &'a [&'a str],
    ) -> RunSpec<'a> {
        RunSpec {
            interactive: false,
            name: "fletch-orkney-deadbeef",
            agent_id: "orkney",
            writable_root: Path::new("/Users/u/.fletch/worktrees/orkney"),
            rpc_dir: Path::new("/Users/u/.fletch/rpc/orkney"),
            home: Path::new("/Users/u"),
            cwd: Path::new("/Users/u/.fletch/worktrees/orkney/repo"),
            blackboard: None,
            mounts,
            borrowed_object_stores: &[],
            memory: "4g",
            cpus: "2",
            image,
            agent_bin,
            auth_vars,
        }
    }

    /// A codex `RunSpec`: a read-write `~/.codex` mount, `OPENAI_API_KEY` as the
    /// forwarded auth var, and the codex image.
    fn codex_spec<'a>() -> RunSpec<'a> {
        rw_config_spec(
            ProviderMounts::Codex {
                config_dir: Path::new("/Users/u/.codex"),
                forward_home: false,
            },
            "fletch-agent-codex:abc123def456",
            "codex",
            &["OPENAI_API_KEY"],
        )
    }

    /// Two-token flag lookup: the value following `flag` each time it appears.
    fn values_of<'a>(args: &'a [String], flag: &str) -> Vec<&'a str> {
        args.windows(2)
            .filter(|w| w[0] == flag)
            .map(|w| w[1].as_str())
            .collect()
    }

    #[test]
    fn argv_mounts_exactly_the_three_dirs_at_identical_paths() {
        // Workspace + mailbox read-write; `~/.claude` read-only (invariant 5),
        // followed by the read-write per-agent `projects/` transcript overlay.
        // No credentials file in this spec, so no credentials overlay; no
        // borrowed object stores, so no `.git/objects` RO mount either.
        let args = run_args(&test_spec(false));
        assert_eq!(
            values_of(&args, "-v"),
            vec![
                "/Users/u/.fletch/worktrees/orkney:/Users/u/.fletch/worktrees/orkney",
                "/Users/u/.fletch/rpc/orkney:/Users/u/.fletch/rpc/orkney",
                "/Users/u/.claude:/Users/u/.claude:ro",
                "/Users/u/.fletch/worktrees/orkney/.fletch-claude-projects:/Users/u/.claude/projects",
            ],
        );
        assert!(
            !args.iter().any(|a| a.contains("/objects")),
            "no object-store mount without borrowed stores"
        );
        assert_eq!(
            values_of(&args, "-w"),
            vec!["/Users/u/.fletch/worktrees/orkney/repo"],
        );
    }

    #[test]
    fn argv_mounts_blackboard_and_forwards_its_env_when_present() {
        let board = Path::new("/Users/u/.fletch/runs/run-1/blackboard");
        let mut spec = test_spec(false);
        spec.blackboard = Some(board);
        let args = run_args(&spec);

        // Bound read-write at its identical host path (invariant 1), the same
        // shape as the RPC mailbox mount.
        assert!(
            values_of(&args, "-v").contains(
                &"/Users/u/.fletch/runs/run-1/blackboard:/Users/u/.fletch/runs/run-1/blackboard"
            ),
            "blackboard must be bind-mounted at its identical host path, got {:?}",
            values_of(&args, "-v")
        );
        // Forwarded so the in-container agent finds the mount via `$WF_BLACKBOARD`.
        assert!(
            values_of(&args, "-e").contains(&"WF_BLACKBOARD"),
            "WF_BLACKBOARD must be forwarded into the container"
        );
    }

    #[test]
    fn argv_omits_blackboard_for_ordinary_agents() {
        let args = run_args(&test_spec(false));
        assert!(
            !args.iter().any(|a| a.contains("blackboard")),
            "no blackboard mount for a non-workflow agent"
        );
        assert!(!values_of(&args, "-e").contains(&"WF_BLACKBOARD"));
    }

    /// Invariant 5: `~/.claude` is read-only so a prompt-injected agent cannot
    /// plant a host-executed hook in `settings.json`, but `.credentials.json`
    /// stays writable (appended *after* the RO dir mount) so token refresh
    /// persists. No other read-write config surface may remain.
    #[test]
    fn argv_mounts_claude_readonly_with_writable_credentials() {
        let mut spec = test_spec(false);
        spec.mounts = claude_mounts(None, true, false);
        let args = run_args(&spec);
        let mounts = values_of(&args, "-v");

        // The dir is read-only; the credentials file is read-write on top.
        let dir_idx = mounts
            .iter()
            .position(|m| *m == "/Users/u/.claude:/Users/u/.claude:ro")
            .expect("~/.claude mounted read-only");
        let creds_idx = mounts
            .iter()
            .position(|m| {
                *m == "/Users/u/.claude/.credentials.json:/Users/u/.claude/.credentials.json"
            })
            .expect("credentials file mounted read-write");
        assert!(
            dir_idx < creds_idx,
            "RW credentials mount must follow the RO dir mount so Docker layers it on top",
        );

        // No read-write mount of any `~/.claude` path other than the credential
        // file — the whole point is that no config write surface survives.
        for mount in &mounts {
            let (src, _) = mount.split_once(':').unwrap();
            if src.starts_with("/Users/u/.claude") {
                assert!(
                    mount.ends_with(":ro") || src.ends_with("/.credentials.json"),
                    "unexpected read-write config surface: {mount}",
                );
            }
        }
    }

    /// Claude Code `mkdir`s `~/.claude/session-env/<id>` and writes a
    /// `shell-snapshots/` entry every session; under the RO `~/.claude` mount
    /// those fail with `EROFS` and abort the agent. Each gets an ephemeral
    /// tmpfs overlay at its exact host path, ordered *after* the RO dir mount so
    /// Docker layers it on top — and as `--tmpfs`, not `-v`, so no host write
    /// surface is added (invariant 5).
    #[test]
    fn argv_overlays_ephemeral_runtime_dirs_with_tmpfs() {
        let args = run_args(&test_spec(false));

        // Exactly the whitelisted subdirs, at their identical host paths.
        assert_eq!(
            values_of(&args, "--tmpfs"),
            vec![
                "/Users/u/.claude/session-env",
                "/Users/u/.claude/shell-snapshots",
            ],
        );

        // The RO dir mount precedes every tmpfs overlay so Docker layers them on
        // top rather than under the read-only bind.
        let ro_idx = args
            .iter()
            .position(|a| a == "/Users/u/.claude:/Users/u/.claude:ro")
            .expect("~/.claude mounted read-only");
        for tmpfs in [
            "/Users/u/.claude/session-env",
            "/Users/u/.claude/shell-snapshots",
        ] {
            let idx = args.iter().position(|a| a == tmpfs).unwrap();
            assert!(
                ro_idx < idx,
                "tmpfs overlay {tmpfs} must follow the RO dir mount"
            );
        }

        // The overlays are tmpfs, never a `-v` bind — no `~/.claude` write
        // surface reaches the host.
        assert!(
            !values_of(&args, "-v")
                .iter()
                .any(|m| m.contains("/session-env") || m.contains("/shell-snapshots")),
            "runtime dirs must be tmpfs overlays, not host bind mounts",
        );
    }

    /// Claude persists its session transcript at `<config-dir>/projects/<slug>/
    /// <uuid>.jsonl`; under the RO `~/.claude` mount that write fails, so
    /// `--resume` can't survive a container recreation. A read-write bind of the
    /// per-agent host dir (under `writable_root`) over `~/.claude/projects`
    /// fixes it *without* exposing the shared `~/.claude/projects` — the bind
    /// source is the isolated per-agent dir, not any host `~/.claude` path.
    #[test]
    fn argv_binds_per_agent_projects_dir_read_write() {
        let args = run_args(&test_spec(false));

        // The transcript overlay: per-agent host source → container projects/,
        // read-write (no `:ro` suffix).
        let overlay =
            "/Users/u/.fletch/worktrees/orkney/.fletch-claude-projects:/Users/u/.claude/projects";
        assert!(
            values_of(&args, "-v").contains(&overlay),
            "projects transcript overlay must be bound read-write",
        );

        // Ordered after the RO `~/.claude` mount so Docker layers it on top.
        let ro_idx = args
            .iter()
            .position(|a| a == "/Users/u/.claude:/Users/u/.claude:ro")
            .expect("~/.claude mounted read-only");
        let overlay_idx = args.iter().position(|a| a == overlay).unwrap();
        assert!(
            ro_idx < overlay_idx,
            "projects overlay must follow the RO dir mount"
        );

        // Invariant 5: no read-write bind draws from a host `~/.claude` path, so
        // the shared config's `projects/` (other agents' transcripts, global
        // memory) is unwritable. Only the read-only `~/.claude` bind may name it
        // as a source; this spec has no `.credentials.json`, its lone exception.
        for mount in values_of(&args, "-v") {
            if mount.ends_with(":ro") {
                continue;
            }
            let (src, _) = mount.split_once(':').unwrap();
            assert!(
                !src.starts_with("/Users/u/.claude"),
                "no host ~/.claude path may be a read-write bind source: {mount}",
            );
        }
    }

    /// A host with no credentials file still launches — the writable overlay is
    /// skipped rather than pointing `-v` at a missing source (which Docker would
    /// materialize as a root-owned directory).
    #[test]
    fn argv_omits_credentials_mount_when_file_absent() {
        let args = run_args(&test_spec(false)); // claude_credentials_rw: false
        assert!(
            !values_of(&args, "-v")
                .iter()
                .any(|m| m.contains("/.credentials.json")),
            "no credentials mount when the file is absent",
        );
    }

    #[test]
    fn argv_mounts_borrowed_object_store_read_only() {
        // A --shared clone borrows the source's objects: the base three plus a
        // single RO mount of the borrowed store at its identical host path.
        let stores = vec![PathBuf::from("/Users/u/repo/.git/objects")];
        let mut spec = test_spec(false);
        spec.borrowed_object_stores = &stores;
        let args = run_args(&spec);
        // Order: workspace RW, mailbox RW, borrowed store RO, `~/.claude` RO
        // (invariant 5), then the RW per-agent `projects/` transcript overlay.
        assert_eq!(
            values_of(&args, "-v"),
            vec![
                "/Users/u/.fletch/worktrees/orkney:/Users/u/.fletch/worktrees/orkney",
                "/Users/u/.fletch/rpc/orkney:/Users/u/.fletch/rpc/orkney",
                "/Users/u/repo/.git/objects:/Users/u/repo/.git/objects:ro",
                "/Users/u/.claude:/Users/u/.claude:ro",
                "/Users/u/.fletch/worktrees/orkney/.fletch-claude-projects:/Users/u/.claude/projects",
            ],
        );
    }

    /// Codex mounts its config dir read-write (auth refresh + rollout writes
    /// must reach the host) and launches the codex image + `codex` bin. There's
    /// no `~/.claude` read-only mount, no tmpfs overlay, and no `projects/`
    /// transcript bind — codex persists transcripts through this same RW mount.
    #[test]
    fn argv_codex_mounts_config_dir_read_write() {
        let args = run_args(&codex_spec());
        assert_eq!(
            values_of(&args, "-v"),
            vec![
                "/Users/u/.fletch/worktrees/orkney:/Users/u/.fletch/worktrees/orkney",
                "/Users/u/.fletch/rpc/orkney:/Users/u/.fletch/rpc/orkney",
                // ~/.codex read-write: no `:ro` suffix.
                "/Users/u/.codex:/Users/u/.codex",
            ],
        );
        // No claude-shaped surfaces leak into a codex launch.
        assert!(
            !args.iter().any(|a| a.contains("/.claude")),
            "codex must not mount any ~/.claude path"
        );
        assert!(
            values_of(&args, "--tmpfs").is_empty(),
            "codex has no tmpfs overlays"
        );
        assert!(
            !args.iter().any(|a| a.contains("fletch-claude-projects")),
            "codex has no projects/ transcript bind"
        );
        // Codex image + in-image bin, last (the prefix_args contract).
        assert_eq!(args[args.len() - 2], "fletch-agent-codex:abc123def456");
        assert_eq!(args[args.len() - 1], "codex");
    }

    /// Codex forwards `OPENAI_API_KEY` by bare name (invariant 3) and, unlike
    /// claude, no `CLAUDE_CONFIG_DIR`/Anthropic vars. `CODEX_HOME` forwards only
    /// for a non-default `$CODEX_HOME`.
    #[test]
    fn argv_codex_forwards_openai_key_and_optional_codex_home() {
        let args = run_args(&codex_spec());
        let forwarded = values_of(&args, "-e");
        assert!(
            forwarded.contains(&"OPENAI_API_KEY"),
            "missing bare -e OPENAI_API_KEY"
        );
        assert!(forwarded.contains(&"HOME"));
        assert!(!forwarded.contains(&"CLAUDE_CONFIG_DIR"));
        assert!(!forwarded.contains(&"ANTHROPIC_API_KEY"));
        // Default ~/.codex: CODEX_HOME is not forwarded (the mount + HOME cover it).
        assert!(
            !forwarded.contains(&"CODEX_HOME"),
            "default CODEX_HOME must not forward"
        );

        // A non-default $CODEX_HOME is forwarded so in-container codex reads it.
        let mut spec = codex_spec();
        spec.mounts = ProviderMounts::Codex {
            config_dir: Path::new("/Users/u/.codex"),
            forward_home: true,
        };
        assert!(values_of(&run_args(&spec), "-e").contains(&"CODEX_HOME"));

        // No token value anywhere in argv.
        for arg in &args {
            assert!(
                !arg.contains('=') || arg.starts_with("fletch."),
                "argv token `{arg}` carries a value — only label tokens may",
            );
        }
    }

    fn opencode_spec<'a>() -> RunSpec<'a> {
        rw_config_spec(
            ProviderMounts::Opencode {
                data_dir: Path::new("/Users/u/.local/share/opencode"),
                config_dir: None,
                forward_xdg_data_home: false,
                forward_xdg_config_home: false,
            },
            "fletch-agent-opencode:abc123def456",
            "opencode",
            &["ANTHROPIC_API_KEY"],
        )
    }

    fn pi_spec<'a>() -> RunSpec<'a> {
        rw_config_spec(
            ProviderMounts::Pi {
                data_dir: Path::new("/Users/u/.pi"),
            },
            "fletch-agent-pi:abc123def456",
            "pi",
            &["ANTHROPIC_API_KEY"],
        )
    }

    fn cursor_spec<'a>() -> RunSpec<'a> {
        rw_config_spec(
            ProviderMounts::Cursor {
                data_dir: Path::new("/Users/u/.cursor"),
            },
            "fletch-agent-cursor:abc123def456",
            "cursor-agent",
            &["CURSOR_API_KEY"],
        )
    }

    /// Assert no claude/codex config surface leaks into another provider's argv:
    /// no `~/.claude` mount, no tmpfs overlay, no `projects/` transcript bind, and
    /// no `~/.codex` mount. Shared by the opencode and pi mount tests.
    fn assert_no_claude_or_codex_surface(args: &[String]) {
        assert!(
            !args.iter().any(|a| a.contains("/.claude")),
            "no ~/.claude path"
        );
        assert!(
            !args.iter().any(|a| a.contains("/.codex")),
            "no ~/.codex path"
        );
        assert!(values_of(args, "--tmpfs").is_empty(), "no tmpfs overlays");
        assert!(
            !args.iter().any(|a| a.contains("fletch-claude-projects")),
            "no projects/ transcript bind",
        );
    }

    /// OpenCode mounts its data dir read-write (accounts DB + session storage must
    /// reach the host) and launches the opencode image + `opencode` bin. Its
    /// config dir is absent here (the common case), so only the data dir mounts.
    #[test]
    fn argv_opencode_mounts_data_dir_read_write() {
        let args = run_args(&opencode_spec());
        assert_eq!(
            values_of(&args, "-v"),
            vec![
                "/Users/u/.fletch/worktrees/orkney:/Users/u/.fletch/worktrees/orkney",
                "/Users/u/.fletch/rpc/orkney:/Users/u/.fletch/rpc/orkney",
                // data dir read-write: no `:ro` suffix.
                "/Users/u/.local/share/opencode:/Users/u/.local/share/opencode",
            ],
        );
        assert_no_claude_or_codex_surface(&args);
        assert_eq!(args[args.len() - 2], "fletch-agent-opencode:abc123def456");
        assert_eq!(args[args.len() - 1], "opencode");
    }

    /// OpenCode's config dir (when present) mounts read-write after the data dir,
    /// and a non-default XDG base forwards the matching var by bare name only.
    #[test]
    fn argv_opencode_mounts_config_dir_and_forwards_xdg() {
        let mut spec = opencode_spec();
        spec.mounts = ProviderMounts::Opencode {
            data_dir: Path::new("/xdg/data/opencode"),
            config_dir: Some(Path::new("/Users/u/.config/opencode")),
            forward_xdg_data_home: true,
            forward_xdg_config_home: false,
        };
        let args = run_args(&spec);
        let mounts = values_of(&args, "-v");
        assert!(mounts.contains(&"/xdg/data/opencode:/xdg/data/opencode"));
        assert!(mounts.contains(&"/Users/u/.config/opencode:/Users/u/.config/opencode"));

        let forwarded = values_of(&args, "-e");
        assert!(
            forwarded.contains(&"XDG_DATA_HOME"),
            "non-default XDG_DATA_HOME forwards"
        );
        assert!(
            !forwarded.contains(&"XDG_CONFIG_HOME"),
            "default XDG_CONFIG_HOME must not forward"
        );
        assert!(forwarded.contains(&"ANTHROPIC_API_KEY"));
        // No value token in argv (invariant 3).
        for arg in &args {
            assert!(
                !arg.contains('=') || arg.starts_with("fletch."),
                "argv token `{arg}` carries a value",
            );
        }
    }

    /// Pi mounts `~/.pi` read-write and launches the pi image + `pi` bin; no
    /// claude/codex surface, and the forwarded key rides by bare name only.
    #[test]
    fn argv_pi_mounts_dot_pi_read_write() {
        let args = run_args(&pi_spec());
        assert_eq!(
            values_of(&args, "-v"),
            vec![
                "/Users/u/.fletch/worktrees/orkney:/Users/u/.fletch/worktrees/orkney",
                "/Users/u/.fletch/rpc/orkney:/Users/u/.fletch/rpc/orkney",
                "/Users/u/.pi:/Users/u/.pi",
            ],
        );
        assert_no_claude_or_codex_surface(&args);
        let forwarded = values_of(&args, "-e");
        assert!(
            forwarded.contains(&"ANTHROPIC_API_KEY"),
            "missing bare -e ANTHROPIC_API_KEY"
        );
        assert!(!forwarded.contains(&"CLAUDE_CONFIG_DIR"));
        assert!(!forwarded.contains(&"CODEX_HOME"));
        for arg in &args {
            assert!(
                !arg.contains('=') || arg.starts_with("fletch."),
                "argv token `{arg}` carries a value",
            );
        }
        assert_eq!(args[args.len() - 2], "fletch-agent-pi:abc123def456");
        assert_eq!(args[args.len() - 1], "pi");
    }

    /// Cursor mounts `~/.cursor` read-write (session transcripts must reach the
    /// host) and launches the cursor image + `cursor-agent` bin; no claude/codex
    /// surface, and its sole auth var rides by bare name only (invariant 3).
    #[test]
    fn argv_cursor_mounts_dot_cursor_read_write() {
        let args = run_args(&cursor_spec());
        assert_eq!(
            values_of(&args, "-v"),
            vec![
                "/Users/u/.fletch/worktrees/orkney:/Users/u/.fletch/worktrees/orkney",
                "/Users/u/.fletch/rpc/orkney:/Users/u/.fletch/rpc/orkney",
                "/Users/u/.cursor:/Users/u/.cursor",
            ],
        );
        assert_no_claude_or_codex_surface(&args);
        let forwarded = values_of(&args, "-e");
        assert!(
            forwarded.contains(&"CURSOR_API_KEY"),
            "missing bare -e CURSOR_API_KEY"
        );
        assert!(!forwarded.contains(&"CLAUDE_CONFIG_DIR"));
        assert!(!forwarded.contains(&"CODEX_HOME"));
        // No token value in argv, only the label token may carry an `=`.
        for arg in &args {
            assert!(
                !arg.contains('=') || arg.starts_with("fletch."),
                "argv token `{arg}` carries a value",
            );
        }
        assert_eq!(args[args.len() - 2], "fletch-agent-cursor:abc123def456");
        assert_eq!(args[args.len() - 1], "cursor-agent");
    }

    #[test]
    fn argv_mounts_every_chained_alternate_read_only() {
        // Every store the workspace borrows (directly or transitively) gets its
        // own RO mount; `run_args` mounts whatever `borrowed_object_stores`
        // resolved.
        let stores = vec![
            PathBuf::from("/Users/u/repo/.git/objects"),
            PathBuf::from("/Users/u/shared-cache/objects"),
        ];
        let mut spec = test_spec(false);
        spec.borrowed_object_stores = &stores;
        let args = run_args(&spec);
        // Object-store RO mounts only (exclude the `~/.claude:ro` mount, which
        // also ends in `:ro` under invariant 5).
        let ro: Vec<&str> = values_of(&args, "-v")
            .into_iter()
            .filter(|m| m.ends_with(":ro") && m.contains("/objects"))
            .collect();
        assert_eq!(
            ro,
            vec![
                "/Users/u/repo/.git/objects:/Users/u/repo/.git/objects:ro",
                "/Users/u/shared-cache/objects:/Users/u/shared-cache/objects:ro",
            ],
        );
    }

    #[test]
    fn borrowed_object_stores_reads_alternates_lines() {
        // Layout: writable_root/<subdir>/.git/objects/info/alternates.
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        let info = root.join("repo/.git/objects/info");
        std::fs::create_dir_all(&info).unwrap();

        // Absent alternates → nothing to mount (worktree / full-copy clone).
        assert!(borrowed_object_stores(root).is_empty());

        std::fs::write(
            info.join("alternates"),
            "/src/a/.git/objects\n\n  /src/b/objects  \n",
        )
        .unwrap();
        assert_eq!(
            borrowed_object_stores(root),
            vec![
                PathBuf::from("/src/a/.git/objects"),
                PathBuf::from("/src/b/objects"),
            ],
        );
    }

    #[test]
    fn borrowed_object_stores_follows_chained_alternates() {
        // Model checkout --shared→ B --shared→ A: the checkout points only at
        // B; B points at A. Both B and A must be discovered so both get
        // mounted, or in-container git can't reach A's objects.
        let td = tempfile::tempdir().unwrap();
        let a = td.path().join("A/.git/objects");
        let b = td.path().join("B/.git/objects");
        // The checkout lives under the writable root as a subdir.
        let checkout = td.path().join("root/repo/.git/objects");
        for dir in [&a, &b, &checkout] {
            std::fs::create_dir_all(dir.join("info")).unwrap();
        }
        std::fs::write(
            checkout.join("info/alternates"),
            format!("{}\n", b.display()),
        )
        .unwrap();
        std::fs::write(b.join("info/alternates"), format!("{}\n", a.display())).unwrap();

        assert_eq!(
            borrowed_object_stores(&td.path().join("root")),
            vec![b, a],
            "chain must resolve checkout→B→A, mounting both borrowed stores"
        );
    }

    #[test]
    fn borrowed_object_stores_scans_every_repo_checkout() {
        // A multi-repo agent: two shared-clone checkouts under one writable
        // root, each borrowing a different source. Both borrowed stores must be
        // discovered — scanning only the primary would strand the secondary.
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("agent");
        let primary = root.join("app/.git/objects/info");
        let secondary = root.join("lib/.git/objects/info");
        std::fs::create_dir_all(&primary).unwrap();
        std::fs::create_dir_all(&secondary).unwrap();
        std::fs::write(primary.join("alternates"), "/src/app/.git/objects\n").unwrap();
        std::fs::write(secondary.join("alternates"), "/src/lib/.git/objects\n").unwrap();

        // Sorted by subdir name: `app` before `lib`.
        assert_eq!(
            borrowed_object_stores(&root),
            vec![
                PathBuf::from("/src/app/.git/objects"),
                PathBuf::from("/src/lib/.git/objects"),
            ],
        );
    }

    #[test]
    fn argv_shape_and_pid1_flags() {
        let args = run_args(&test_spec(false));
        assert_eq!(args[0], "run");
        assert!(args.contains(&"--rm".to_string()));
        assert!(args.contains(&"--init".to_string()));
        assert!(args.contains(&"-i".to_string()));
        assert!(
            !args.contains(&"-t".to_string()),
            "stdio launch must not allocate a tty"
        );
        // prefix_args contract: image then agent bin, last — the caller
        // appends agent CLI args directly after.
        assert_eq!(args[args.len() - 2], "fletch-agent:abc123def456");
        assert_eq!(args[args.len() - 1], "claude");
        assert_eq!(values_of(&args, "--name"), vec!["fletch-orkney-deadbeef"]);
        assert_eq!(values_of(&args, "--memory"), vec!["4g"]);
        assert_eq!(values_of(&args, "--cpus"), vec!["2"]);

        let interactive = run_args(&test_spec(true));
        assert!(
            interactive.contains(&"-t".to_string()),
            "pty launch gets a tty"
        );
    }

    #[test]
    fn argv_labels_carry_pid_and_agent_id() {
        let args = run_args(&test_spec(false));
        let labels = values_of(&args, "--label");
        assert!(labels.contains(&format!("fletch.host-pid={}", std::process::id()).as_str()));
        assert!(labels.contains(&"fletch.agent-id=orkney"));
    }

    /// Invariant 3: auth is forwarded by bare name only — no token value can
    /// ever appear in argv, whatever the environment holds.
    #[test]
    fn argv_forwards_auth_by_bare_name_never_value() {
        let args = run_args(&test_spec(false));
        let forwarded = values_of(&args, "-e");
        for var in [
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_AUTH_TOKEN",
        ] {
            assert!(forwarded.contains(&var), "missing bare -e {var}");
        }
        for arg in &args {
            assert!(
                !arg.contains('=') || arg.starts_with("fletch."),
                "argv token `{arg}` carries a value — only label tokens may",
            );
        }
        // Non-auth runtime env is forwarded the same way.
        for var in ["HOME", "FLETCH_RPC_DIR", "TERM", "COLORTERM"] {
            assert!(forwarded.contains(&var), "missing bare -e {var}");
        }
        assert!(
            !forwarded.contains(&"CLAUDE_CONFIG_DIR"),
            "default config dir must not be forwarded",
        );
    }

    #[test]
    fn argv_mounts_and_forwards_nondefault_claude_config_dir() {
        // The non-default config dir gets the same read-only-except-credentials
        // treatment as `~/.claude` (invariant 5).
        let mut spec = test_spec(false);
        spec.mounts = claude_mounts(Some(Path::new("/Users/u/.claude-eve")), false, true);
        let args = run_args(&spec);
        let mounts = values_of(&args, "-v");
        assert!(mounts.contains(&"/Users/u/.claude-eve:/Users/u/.claude-eve:ro"));
        assert!(mounts.contains(
            &"/Users/u/.claude-eve/.credentials.json:/Users/u/.claude-eve/.credentials.json"
        ));
        assert!(values_of(&args, "-e").contains(&"CLAUDE_CONFIG_DIR"));
    }

    #[test]
    fn config_dir_is_default_canonicalizes_both_sides() {
        let td = tempfile::tempdir().unwrap();
        let home = td.path();
        let default = home.join(".claude");
        std::fs::create_dir_all(&default).unwrap();

        // The literal default, and its trailing-slash spelling, are default.
        assert!(config_dir_is_default(&default, home));
        assert!(config_dir_is_default(&home.join(".claude/"), home));
        // A genuinely different dir is not.
        assert!(!config_dir_is_default(&home.join(".claude-eve"), home));

        // A symlink that resolves to the default is treated as default — the
        // both-sides canonicalization this test exists to prove. (On macOS the
        // tempdir itself lives under a `/var`→`/private/var` symlink, so the
        // home side is exercised too.)
        #[cfg(unix)]
        {
            let link = home.join("link-to-claude");
            std::os::unix::fs::symlink(&default, &link).unwrap();
            assert!(
                config_dir_is_default(&link, home),
                "a symlink resolving to ~/.claude must read as default"
            );
        }
    }

    #[test]
    fn container_name_shape_and_nonce_uniqueness() {
        let a = container_name("orkney");
        let b = container_name("orkney");
        for name in [&a, &b] {
            let nonce = name.strip_prefix("fletch-orkney-").expect("prefix");
            assert_eq!(nonce.len(), 8);
            assert!(nonce.chars().all(|c| c.is_ascii_hexdigit()));
        }
        assert_ne!(a, b, "respawns must never reuse a container name");

        // Ids are word-safe today; anything unexpected sanitizes to '-'.
        assert!(container_name("we ird/id").starts_with("fletch-we-ird-id-"));
    }

    #[test]
    fn exit_code_mapping_is_distinct_and_scoped() {
        let daemon = describe_exit_code(125).unwrap();
        let not_exec = describe_exit_code(126).unwrap();
        let missing = describe_exit_code(127).unwrap();
        assert!(daemon.contains("daemon"), "{daemon}");
        assert!(not_exec.contains("not runnable"), "{not_exec}");
        assert!(missing.contains("no agent binary"), "{missing}");
        let distinct: std::collections::HashSet<_> = [&daemon, &not_exec, &missing].into();
        assert_eq!(distinct.len(), 3);
        // Each hedges: docker relays the agent's own status, so these codes can
        // originate inside the container — the message must not over-claim.
        for msg in [&daemon, &not_exec, &missing] {
            assert!(msg.contains("agent itself exited"), "must hedge: {msg}");
        }
        for code in [0, 1, 2, 124, 128, 130, 137, 143] {
            assert_eq!(
                describe_exit_code(code),
                None,
                "code {code} must pass through"
            );
        }
    }

    #[test]
    fn blank_settings_fall_back_to_defaults() {
        assert_eq!(non_blank(None), None);
        assert_eq!(non_blank(Some("")), None);
        assert_eq!(non_blank(Some("  ")), None);
        assert_eq!(non_blank(Some(" 8g ")), Some("8g"));
    }

    /// Happy path: a resolved auth env lands on the docker CLI process env
    /// verbatim, and forwarding exactly those names puts a matching bare
    /// `-e NAME` in argv for each — so values forward into the container yet
    /// never appear in argv (invariant 3).
    #[test]
    fn resolved_auth_forwards_values_in_env_never_argv() {
        use super::super::auth::AuthSource;

        let secret = "sk-ant-oat-SECRET-VALUE";
        let resolved = ContainerAuth::Resolved {
            env: vec![
                ("CLAUDE_CODE_OAUTH_TOKEN".to_string(), secret.to_string()),
                (
                    "ANTHROPIC_AUTH_TOKEN".to_string(),
                    "proxy-secret".to_string(),
                ),
            ],
            source: AuthSource::StoredToken,
        };
        let mut env: Vec<(String, String)> = Vec::new();
        apply_container_auth(&mut env, resolved).expect("resolved auth applies");

        // Values ride the CLI process env.
        assert!(env
            .iter()
            .any(|(k, v)| k == "CLAUDE_CODE_OAUTH_TOKEN" && v == secret));
        assert!(env
            .iter()
            .any(|(k, v)| k == "ANTHROPIC_AUTH_TOKEN" && v == "proxy-secret"));

        // Forwarding exactly those names emits a bare `-e NAME` for each, with
        // no value (secret or otherwise) anywhere in argv.
        let auth_var_names: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        let mut spec = test_spec(false);
        spec.auth_vars = &auth_var_names;
        let args = run_args(&spec);
        let forwarded = values_of(&args, "-e");
        for (name, _) in &env {
            assert!(
                forwarded.contains(&name.as_str()),
                "resolved var {name} has no bare -e in argv",
            );
        }
        for arg in &args {
            assert!(!arg.contains(secret), "secret leaked into argv: {arg}");
            assert!(!arg.contains("proxy-secret"), "proxy secret in argv: {arg}");
        }
    }

    /// D1 swap, empty resolution (`CredentialsFile`): no env additions, no
    /// error — the `~/.claude` mount carries the credential.
    #[test]
    fn resolved_auth_with_empty_env_is_a_noop() {
        use super::super::auth::AuthSource;

        let mut env: Vec<(String, String)> = Vec::new();
        apply_container_auth(
            &mut env,
            ContainerAuth::Resolved {
                env: Vec::new(),
                source: AuthSource::CredentialsFile,
            },
        )
        .expect("credentials-file resolves");
        assert!(env.is_empty());
    }

    /// D1 swap, `Unavailable`: the launch fails fast with the settings pointer
    /// C2 keys its call-to-action on. Asserts the stable substrings so the
    /// wording can evolve without silently breaking the UI match.
    #[test]
    fn unavailable_auth_fails_launch_with_settings_pointer() {
        let mut env: Vec<(String, String)> = Vec::new();
        let err = apply_container_auth(&mut env, ContainerAuth::Unavailable)
            .expect_err("Unavailable must block the launch");
        let msg = err.to_string();
        assert!(msg.contains("Settings"), "no settings pointer: {msg}");
        assert!(msg.contains("setup-token"), "no setup-token hint: {msg}");
        assert_eq!(msg, NO_CONTAINER_AUTH_MSG);
        assert!(env.is_empty(), "a failed resolution must add no env");
    }

    /// Codex auth: a non-blank `OPENAI_API_KEY` is forwarded (trimmed); a bare
    /// `auth.json` resolves with nothing to inject (the mount carries it); a
    /// blank key falls back to the file; neither present fails the launch.
    #[test]
    fn codex_auth_env_resolves_key_file_or_fails() {
        // API key wins and is trimmed.
        assert_eq!(
            codex_auth_env(Some(" sk-openai \n"), false).unwrap(),
            vec![("OPENAI_API_KEY".to_string(), "sk-openai".to_string())],
        );
        // Key alongside a file still forwards the key.
        assert_eq!(
            codex_auth_env(Some("sk-openai"), true).unwrap(),
            vec![("OPENAI_API_KEY".to_string(), "sk-openai".to_string())],
        );
        // No key but a mounted auth.json: nothing to inject, but resolves.
        assert!(codex_auth_env(None, true).unwrap().is_empty());
        assert!(codex_auth_env(Some("   "), true).unwrap().is_empty());
        // Neither: fail-fast with the settings pointer.
        let err = codex_auth_env(None, false).unwrap_err().to_string();
        assert_eq!(err, NO_CODEX_AUTH_MSG);
        assert!(codex_auth_env(Some("  "), false).is_err());
    }

    /// Regression: a key-only user (`OPENAI_API_KEY` set, never ran codex on
    /// the host) must launch — the missing config dir is created for the RW
    /// mount, not treated as "no way to authenticate". With no credential at
    /// all the launch still fails, and before touching the filesystem.
    #[test]
    fn codex_key_only_launch_creates_missing_config_dir() {
        let td = tempfile::tempdir().unwrap();

        let dir = td.path().join(".codex");
        let mut env = Vec::new();
        prepare_codex_launch(&mut env, &dir, Some("sk-openai")).unwrap();
        assert!(dir.is_dir(), "config dir must exist for the RW bind mount");
        assert_eq!(
            env,
            vec![("OPENAI_API_KEY".to_string(), "sk-openai".to_string())],
        );

        let no_auth = td.path().join(".codex-no-auth");
        let err = prepare_codex_launch(&mut Vec::new(), &no_auth, None).unwrap_err();
        assert_eq!(err.to_string(), NO_CODEX_AUTH_MSG);
        assert!(!no_auth.exists(), "auth failure must not create the dir");
    }

    /// `present_api_keys` returns only the known, non-blank vars, trimmed, in the
    /// constant's order — and never a var outside the curated set.
    #[test]
    fn present_api_keys_filters_trims_and_orders() {
        let env: std::collections::HashMap<&str, &str> = [
            ("OPENAI_API_KEY", " sk-openai \n"),
            ("ANTHROPIC_API_KEY", "sk-ant"),
            ("GROQ_API_KEY", "   "),    // blank → dropped
            ("SOME_OTHER_KEY", "nope"), // not in the curated set → dropped
        ]
        .into_iter()
        .collect();
        let keys = present_api_keys(|n| env.get(n).map(|v| v.to_string()));
        assert_eq!(
            keys,
            vec![
                // ANTHROPIC precedes OPENAI in MULTI_PROVIDER_API_KEY_ENV.
                ("ANTHROPIC_API_KEY".to_string(), "sk-ant".to_string()),
                ("OPENAI_API_KEY".to_string(), "sk-openai".to_string()),
            ],
        );
        assert!(present_api_keys(|_| None).is_empty());
    }

    /// The shared multi-provider rule: a forwarded key OR a credential on the
    /// mount resolves (returning the keys to forward); neither → the given error.
    #[test]
    fn multi_provider_auth_env_key_or_mount_or_fail() {
        let key = vec![("ANTHROPIC_API_KEY".to_string(), "sk".to_string())];
        // Key present: forwarded, regardless of the mount.
        assert_eq!(
            multi_provider_auth_env(key.clone(), false, NO_OPENCODE_AUTH_MSG).unwrap(),
            key,
        );
        // No key but a credential on the mount: resolves with nothing to inject.
        assert!(multi_provider_auth_env(Vec::new(), true, NO_PI_AUTH_MSG)
            .unwrap()
            .is_empty());
        // Neither: the caller's fail-fast message.
        let err = multi_provider_auth_env(Vec::new(), false, NO_OPENCODE_AUTH_MSG).unwrap_err();
        assert_eq!(err.to_string(), NO_OPENCODE_AUTH_MSG);
    }

    /// Regression (mirrors the codex key-only case): an opencode user with only a
    /// provider API key set (never ran opencode) still launches — the missing data
    /// dir is created for the RW bind. A stored login on the mount (opencode.db or
    /// auth.json) also resolves. No credential at all fails before touching disk.
    #[test]
    fn opencode_key_only_launch_creates_missing_data_dir() {
        let td = tempfile::tempdir().unwrap();

        // Key-only: dir created, key forwarded.
        let dir = td.path().join("share/opencode");
        let mut env = Vec::new();
        let keys = vec![("OPENAI_API_KEY".to_string(), "sk".to_string())];
        prepare_opencode_launch(&mut env, &dir, keys.clone()).unwrap();
        assert!(dir.is_dir(), "data dir must exist for the RW bind mount");
        assert_eq!(env, keys);

        // Stored login on the mount (opencode.db), no key: resolves, nothing to inject.
        let db_dir = td.path().join("with-db/opencode");
        std::fs::create_dir_all(&db_dir).unwrap();
        std::fs::write(db_dir.join("opencode.db"), b"x").unwrap();
        let mut env2 = Vec::new();
        prepare_opencode_launch(&mut env2, &db_dir, Vec::new()).unwrap();
        assert!(env2.is_empty());

        // Neither: fail fast, no dir created.
        let none = td.path().join("no-auth/opencode");
        let err = prepare_opencode_launch(&mut Vec::new(), &none, Vec::new()).unwrap_err();
        assert_eq!(err.to_string(), NO_OPENCODE_AUTH_MSG);
        assert!(!none.exists(), "auth failure must not create the dir");
    }

    /// Regression for pi: key-only user launches (`~/.pi` created for the RW bind);
    /// `~/.pi/agent/auth.json` on the mount also resolves; neither fails fast.
    #[test]
    fn pi_key_only_launch_creates_missing_data_dir() {
        let td = tempfile::tempdir().unwrap();

        let dir = td.path().join(".pi");
        let mut env = Vec::new();
        let keys = vec![("ANTHROPIC_API_KEY".to_string(), "sk".to_string())];
        prepare_pi_launch(&mut env, &dir, keys.clone()).unwrap();
        assert!(dir.is_dir(), "~/.pi must exist for the RW bind mount");
        assert_eq!(env, keys);

        // auth.json on the mount, no key: resolves, nothing to inject.
        let with_auth = td.path().join(".pi-authed");
        std::fs::create_dir_all(with_auth.join("agent")).unwrap();
        std::fs::write(with_auth.join("agent/auth.json"), b"{}").unwrap();
        let mut env2 = Vec::new();
        prepare_pi_launch(&mut env2, &with_auth, Vec::new()).unwrap();
        assert!(env2.is_empty());

        let none = td.path().join(".pi-no-auth");
        let err = prepare_pi_launch(&mut Vec::new(), &none, Vec::new()).unwrap_err();
        assert_eq!(err.to_string(), NO_PI_AUTH_MSG);
        assert!(!none.exists(), "auth failure must not create the dir");
    }

    /// Cursor auth: a non-blank `CURSOR_API_KEY` is forwarded (trimmed); anything
    /// else fails the launch. Unlike the other providers there is *no* mount
    /// fallback — the keychain-bound login token can't reach a container — so a
    /// missing/blank key is the only outcome besides a forwarded key.
    #[test]
    fn cursor_auth_env_forwards_key_or_fails() {
        assert_eq!(
            cursor_auth_env(Some(" cur-key \n")).unwrap(),
            vec![("CURSOR_API_KEY".to_string(), "cur-key".to_string())],
        );
        // No mount fallback: unset and blank both fail with the settings pointer.
        assert_eq!(
            cursor_auth_env(None).unwrap_err().to_string(),
            NO_CURSOR_AUTH_MSG
        );
        assert_eq!(
            cursor_auth_env(Some("   ")).unwrap_err().to_string(),
            NO_CURSOR_AUTH_MSG
        );
    }

    /// Regression (mirrors the codex key-only case): a cursor user with
    /// `CURSOR_API_KEY` set launches — the missing `~/.cursor` is created for the
    /// RW transcript bind. With no key the launch fails, before touching disk
    /// (there is no mounted-credential path for cursor to fall back to).
    #[test]
    fn cursor_key_only_launch_creates_missing_config_dir() {
        let td = tempfile::tempdir().unwrap();

        let dir = td.path().join(".cursor");
        let mut env = Vec::new();
        prepare_cursor_launch(&mut env, &dir, Some("cur-key")).unwrap();
        assert!(dir.is_dir(), "~/.cursor must exist for the RW bind mount");
        assert_eq!(
            env,
            vec![("CURSOR_API_KEY".to_string(), "cur-key".to_string())]
        );

        let no_auth = td.path().join(".cursor-no-auth");
        let err = prepare_cursor_launch(&mut Vec::new(), &no_auth, None).unwrap_err();
        assert_eq!(err.to_string(), NO_CURSOR_AUTH_MSG);
        assert!(!no_auth.exists(), "auth failure must not create the dir");
    }

    /// Integration: a real `docker run` round-trip through the exact argv the
    /// engine builds — busybox standing in for the agent image, `echo` for
    /// the agent binary. `FLETCH_DOCKER_TESTS=1 cargo test -- --ignored`
    #[test]
    #[ignore = "requires Docker; opt in via FLETCH_DOCKER_TESTS=1"]
    fn docker_run_echo_round_trip() {
        if !crate::sandbox::docker::docker_tests_enabled() {
            return;
        }
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("root");
        let rpc = td.path().join("rpc");
        let home = td.path().join("home");
        for d in [&root, &rpc] {
            std::fs::create_dir_all(d).unwrap();
        }
        // Same mount-source/mountpoint prep `launch_agent` does, so the tmpfs
        // overlays and the `projects/` bind have targets under the RO `~/.claude`
        // bind and a source dir that isn't materialized root-owned.
        prepare_config_mount_dir(&home.join(".claude")).unwrap();
        let projects_src = root.join(crate::transcripts::DOCKER_CLAUDE_PROJECTS_DIRNAME);
        std::fs::create_dir_all(&projects_src).unwrap();
        let name = container_name("b2-int-test");
        let args = run_args(&RunSpec {
            interactive: false,
            name: &name,
            agent_id: "b2-int-test",
            writable_root: &root,
            rpc_dir: &rpc,
            home: &home,
            cwd: &root,
            blackboard: None,
            mounts: ProviderMounts::Claude {
                config_dir: None,
                credentials_rw: false,
                config_dir_credentials_rw: false,
                projects_src: &projects_src,
            },
            borrowed_object_stores: &[],
            memory: "256m",
            cpus: "1",
            image: "busybox",
            agent_bin: "echo",
            auth_vars: &[],
        });
        let docker = cli::docker_bin().expect("docker installed");
        let out = std::process::Command::new(docker)
            .args(&args)
            .arg("hello-from-container")
            .env("HOME", &home)
            .env("FLETCH_RPC_DIR", &rpc)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "docker run failed: {}",
            String::from_utf8_lossy(&out.stderr),
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "hello-from-container",
        );
    }

    /// Integration: kill and liveness against a live container.
    /// `FLETCH_DOCKER_TESTS=1 cargo test -- --ignored`
    #[test]
    #[ignore = "requires Docker; opt in via FLETCH_DOCKER_TESTS=1"]
    fn kill_and_liveness_against_live_container() {
        if !crate::sandbox::docker::docker_tests_enabled() {
            return;
        }
        let name = container_name("b2-kill-test");
        let out = cli::run_docker(
            &[
                "run", "-d", "--rm", "--name", &name, "busybox", "sleep", "60",
            ],
            Duration::from_secs(60),
        )
        .unwrap();
        assert!(
            out.status.success(),
            "docker run failed: {}",
            String::from_utf8_lossy(&out.stderr),
        );

        let engine = DockerEngine::shared();
        let plan = KillPlan::Container { name: name.clone() };
        assert!(
            container_running(&name),
            "fresh container should be running"
        );
        engine.kill(&plan).unwrap();
        assert!(!container_running(&name), "killed container reads as dead");
    }
}
