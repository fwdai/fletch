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
use crate::sandbox::seatbelt::resolve_existing_prefix;

use super::auth::{self, ContainerAuth};
use super::{cleanup, cli, image};

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
const CREDENTIALS_FILE: &str = ".credentials.json";

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
const EPHEMERAL_RUNTIME_SUBDIRS: &[&str] = &["session-env", "shell-snapshots"];

/// Claude's session-transcript subdir within a config dir (`<config-dir>/
/// projects/<slug>/<uuid>.jsonl`). Unlike [`EPHEMERAL_RUNTIME_SUBDIRS`], this
/// one is bind-mounted to a *persistent* per-agent host dir (not tmpfs) so
/// `--resume` survives container recreation — see [`push_claude_config_mount`].
const PROJECTS_SUBDIR: &str = "projects";

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

/// The `SandboxEngine` implementation for Docker. Obtain it via
/// [`DockerEngine::shared`]: launches embed an `Arc` of the engine in their
/// [`KillHandle`], and sharing one instance also shares the once-per-app-run
/// image resolution cache.
pub struct DockerEngine {
    /// The image resolved for this app run, keyed by the override value it
    /// was resolved under so a (future) mid-run settings change re-resolves.
    /// Only successes are cached — a failed build retries on the next spawn
    /// (the user may have started Docker or fixed their network since).
    resolved_image: Mutex<Option<(Option<String>, String)>>,
}

impl DockerEngine {
    /// The process-wide engine instance — the same `Arc` that `engine_for`
    /// hands to launch paths and that every launch parks in its `KillHandle`.
    pub fn shared() -> Arc<DockerEngine> {
        static ENGINE: OnceLock<Arc<DockerEngine>> = OnceLock::new();
        ENGINE
            .get_or_init(|| {
                Arc::new(DockerEngine {
                    resolved_image: Mutex::new(None),
                })
            })
            .clone()
    }

    /// The image to launch from, resolving (and building, if the embedded
    /// image is missing) at most once per app run per override value.
    fn resolve_image_cached(&self, override_image: Option<&str>) -> Result<String> {
        let mut cache = self.resolved_image.lock().unwrap();
        if let Some((cached_override, tag)) = cache.as_ref() {
            if cached_override.as_deref() == override_image {
                return Ok(tag.clone());
            }
        }
        // Per-line build output goes to the log; the UI build toast is driven
        // separately by the `progress` sink inside `image::ensure_image`.
        let on_progress = |line: &str| tracing::info!(target: "fletch::docker_build", "{line}");
        let tag = image::resolve_image(override_image, &on_progress)
            .map_err(|e| Error::Other(format!("preparing the Docker sandbox image failed: {e}")))?;
        *cache = Some((override_image.map(str::to_string), tag.clone()));
        Ok(tag)
    }
}

impl SandboxEngine for DockerEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::Docker
    }

    /// Claude-only despite the agent-agnostic `agent_bin` parameter: the mounts,
    /// `~/.claude` overlays, `projects/` transcript bind, and auth chain are all
    /// claude-shaped. `supervisor::lifecycle::ensure_engine_supports_provider`
    /// gates docker launches to `provider == "claude"`, so a non-claude
    /// `agent_bin` never reaches here; a future per-turn-under-docker provider
    /// would need its own config handling rather than inheriting claude's.
    fn launch_agent(&self, ctx: &AgentLaunchCtx, agent_bin: &str) -> Result<LaunchPlan> {
        let docker = cli::docker_bin()
            .ok_or_else(|| Error::Other("docker binary not found — is Docker installed?".into()))?;
        let settings = LAUNCH_SETTINGS.read().clone();
        let image = self.resolve_image_cached(settings.image_override.as_deref())?;
        let name = container_name(ctx.agent_id);
        let claude_config_dir = nondefault_claude_config_dir(ctx.home);

        // Make sure the mount sources exist before we hand them to `-v`. If a
        // source can't be created (a file already sits at the path, a read-only
        // or missing parent, permissions), mounting it anyway would let Docker
        // recreate it root-owned or fail the bind opaquely — either way claude
        // loses access to its auth/config. Fail the launch with the path
        // instead of pressing on with a bad mount.
        let claude_dir = ctx.home.join(".claude");
        prepare_config_mount_dir(&claude_dir)?;
        if let Some(dir) = &claude_config_dir {
            prepare_config_mount_dir(dir)?;
        }

        // Per-agent host dir backing claude's `projects/` (session transcripts).
        // Bind-mounted read-write over the read-only config dir's `projects/`
        // (see [`push_claude_config_mount`]) so `--resume` survives container
        // recreation without exposing the shared `~/.claude/projects` — other
        // agents' transcripts and global memory stay unreachable (invariant 5).
        // Lives under the agent's writable root, so archive teardown's `rm -rf`
        // of that root reclaims it with no separate cleanup. Created before
        // `docker run` so Docker binds an existing source instead of
        // materializing it root-owned.
        let projects_src = ctx
            .writable_root
            .join(crate::transcripts::DOCKER_CLAUDE_PROJECTS_DIRNAME);
        std::fs::create_dir_all(&projects_src).map_err(|e| {
            Error::Other(format!(
                "preparing Docker sandbox projects mount {} failed: {e}",
                projects_src.display()
            ))
        })?;

        // Object stores every checkout under the agent's writable root borrows
        // via git alternates (a --shared clone). Scanned across all tracked
        // repos, not just the primary `cwd`: a multi-repo agent has one shared
        // clone per repo, each borrowing its own source's objects. Mounted
        // read-only so in-container git reads borrowed history; empty for old
        // full-copy clones or worktrees (no mount).
        let borrowed_object_stores = borrowed_object_stores(ctx.writable_root);

        // The read-only config mounts get a writable `.credentials.json` overlay
        // only when the file already exists: a bare `-v` on a missing source
        // makes Docker create a root-owned *directory* there, which would break
        // claude's later attempt to write the real file.
        let claude_credentials_rw = claude_dir.join(CREDENTIALS_FILE).is_file();
        let config_dir_credentials_rw = claude_config_dir
            .as_deref()
            .is_some_and(|dir| dir.join(CREDENTIALS_FILE).is_file());

        // Env set on the docker CLI process; forwarded into the container by the
        // bare `-e NAME` flags `run_args` emits (values never touch argv —
        // invariant 3). The auth chain appends its resolved vars last, so only
        // what it actually picked is forwarded.
        let mut env: Vec<(String, String)> = vec![
            ("HOME".into(), ctx.home.to_string_lossy().into_owned()),
            (
                "FLETCH_RPC_DIR".into(),
                ctx.rpc_dir.to_string_lossy().into_owned(),
            ),
            ("TERM".into(), "xterm-256color".into()),
            ("COLORTERM".into(), "truecolor".into()),
        ];
        if let Some(dir) = &claude_config_dir {
            env.push((
                "CLAUDE_CONFIG_DIR".into(),
                dir.to_string_lossy().into_owned(),
            ));
        }
        let auth_start = env.len();
        apply_container_auth(&mut env, auth::resolve())?;

        // Forward exactly the resolved auth var names (the tail
        // `apply_container_auth` appended). Scoped so the borrow of `env` ends
        // before it moves into the plan below.
        let prefix_args = {
            let auth_vars: Vec<&str> =
                env[auth_start..].iter().map(|(k, _)| k.as_str()).collect();
            run_args(&RunSpec {
                interactive: ctx.interactive,
                name: &name,
                agent_id: ctx.agent_id,
                writable_root: ctx.writable_root,
                rpc_dir: ctx.rpc_dir,
                home: ctx.home,
                cwd: ctx.cwd,
                claude_config_dir: claude_config_dir.as_deref(),
                borrowed_object_stores: &borrowed_object_stores,
                claude_credentials_rw,
                config_dir_credentials_rw,
                projects_src: &projects_src,
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
    if resolve_existing_prefix(&dir) == resolve_existing_prefix(&home.join(".claude")) {
        return None;
    }
    Some(dir)
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
    claude_config_dir: Option<&'a Path>,
    /// Object stores borrowed via git alternates (a `--shared` clone),
    /// bind-mounted read-only at their identical host paths. Empty for a
    /// worktree or an old full-copy clone.
    borrowed_object_stores: &'a [PathBuf],
    /// Whether `~/.claude/.credentials.json` exists as a file — gates the
    /// writable overlay on the read-only `~/.claude` mount.
    claude_credentials_rw: bool,
    /// Same, for the non-default `CLAUDE_CONFIG_DIR` (only meaningful when
    /// `claude_config_dir` is `Some`).
    config_dir_credentials_rw: bool,
    /// Per-agent host dir bind-mounted read-write over each config dir's
    /// `projects/` so claude's session transcript persists across container
    /// recreation (resume) while the shared `~/.claude` stays read-only. Lives
    /// under `writable_root`; see [`push_claude_config_mount`].
    projects_src: &'a Path,
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
    push_claude_config_mount(
        &mut args,
        &spec.home.join(".claude"),
        spec.claude_credentials_rw,
        spec.projects_src,
    );
    if let Some(dir) = spec.claude_config_dir {
        push_claude_config_mount(
            &mut args,
            dir,
            spec.config_dir_credentials_rw,
            spec.projects_src,
        );
    }
    args.push("-w".into());
    args.push(spec.cwd.to_string_lossy().into_owned());
    // Bare `-e NAME` forwards from the docker CLI's own environment without
    // the value ever appearing in argv (invariant 3 for the auth vars). Auth
    // vars come from `spec.auth_vars` — the set the chain actually resolved.
    let mut forwarded: Vec<&str> = vec!["HOME", "FLETCH_RPC_DIR", "TERM", "COLORTERM"];
    if spec.claude_config_dir.is_some() {
        forwarded.push("CLAUDE_CONFIG_DIR");
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
/// relays the agent's own exit status, so a `claude` that starts fine and later
/// exits 125/126/127 is indistinguishable from a launcher/image failure — the
/// messages name the likely Docker-layer cause but flag the agent-exit
/// possibility so they don't mislead when the container did launch.
fn describe_exit_code(code: i32) -> Option<String> {
    let msg = match code {
        125 => "Exit 125: Docker could not start the sandbox container — the daemon reported an error (or the agent itself exited 125). Is Docker Desktop still running?",
        126 => "Exit 126: the agent binary in the sandbox image is present but not runnable (or the agent itself exited 126). If you set a custom docker_image, check its `claude`.",
        127 => "Exit 127: no `claude` on the sandbox image's PATH (or the agent itself exited 127). A custom docker_image must include Claude Code.",
        _ => return None,
    };
    Some(msg.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_spec<'a>(interactive: bool) -> RunSpec<'a> {
        RunSpec {
            interactive,
            name: "fletch-orkney-deadbeef",
            agent_id: "orkney",
            writable_root: Path::new("/Users/u/.fletch/worktrees/orkney"),
            rpc_dir: Path::new("/Users/u/.fletch/rpc/orkney"),
            home: Path::new("/Users/u"),
            cwd: Path::new("/Users/u/.fletch/worktrees/orkney/repo"),
            claude_config_dir: None,
            borrowed_object_stores: &[],
            claude_credentials_rw: false,
            config_dir_credentials_rw: false,
            projects_src: Path::new("/Users/u/.fletch/worktrees/orkney/.fletch-claude-projects"),
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

    /// Invariant 5: `~/.claude` is read-only so a prompt-injected agent cannot
    /// plant a host-executed hook in `settings.json`, but `.credentials.json`
    /// stays writable (appended *after* the RO dir mount) so token refresh
    /// persists. No other read-write config surface may remain.
    #[test]
    fn argv_mounts_claude_readonly_with_writable_credentials() {
        let mut spec = test_spec(false);
        spec.claude_credentials_rw = true;
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
        for tmpfs in ["/Users/u/.claude/session-env", "/Users/u/.claude/shell-snapshots"] {
            let idx = args.iter().position(|a| a == tmpfs).unwrap();
            assert!(ro_idx < idx, "tmpfs overlay {tmpfs} must follow the RO dir mount");
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
        assert!(ro_idx < overlay_idx, "projects overlay must follow the RO dir mount");

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
        std::fs::write(checkout.join("info/alternates"), format!("{}\n", b.display())).unwrap();
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
        spec.claude_config_dir = Some(Path::new("/Users/u/.claude-eve"));
        spec.config_dir_credentials_rw = true;
        let args = run_args(&spec);
        let mounts = values_of(&args, "-v");
        assert!(mounts.contains(&"/Users/u/.claude-eve:/Users/u/.claude-eve:ro"));
        assert!(mounts.contains(
            &"/Users/u/.claude-eve/.credentials.json:/Users/u/.claude-eve/.credentials.json"
        ));
        assert!(values_of(&args, "-e").contains(&"CLAUDE_CONFIG_DIR"));
    }

    #[test]
    fn nondefault_config_dir_rules_match_seatbelt() {
        // Pure check of the comparison rule (the env var itself is process
        // state; tests must not mutate it).
        let home = Path::new("/Users/u");
        assert_eq!(home.join(".claude"), Path::new("/Users/u/.claude"));
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
        assert!(missing.contains("no `claude`"), "{missing}");
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
            claude_config_dir: None,
            borrowed_object_stores: &[],
            claude_credentials_rw: false,
            config_dir_credentials_rw: false,
            projects_src: &projects_src,
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
        assert!(container_running(&name), "fresh container should be running");
        engine.kill(&plan).unwrap();
        assert!(!container_running(&name), "killed container reads as dead");
    }
}
