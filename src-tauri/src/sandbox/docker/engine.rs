//! The Docker sandbox engine: one container per agent process, agent ≈ PID 1.
//!
//! Launch shape (fixed in the sandbox plan, slice B2): every agent process is
//! its own `docker run --rm --init` — no long-lived container + `docker exec`,
//! whose kill/exit-code semantics are broken. The plan invariants this file
//! carries:
//!
//! - **Path identity (invariant 1).** The three mounts — the agent's writable
//!   root, its RPC mailbox, and `~/.claude` — are bind-mounted at their exact
//!   host paths, and the container runs with `HOME=<host home>`; transcripts,
//!   RPC payloads, and diff paths all embed absolute host paths, so nothing in
//!   the app translates paths.
//! - **The real repo never enters the container (invariant 2).** Only the
//!   agent's own parent dir is mounted; `supervisor::lifecycle` forces
//!   clone-mode workspaces for docker agents, so no linked-worktree `.git`
//!   pointer can reach the user's repo.
//! - **Secrets never in argv (invariant 3).** Auth vars are set on the docker
//!   *CLI process* environment (`LaunchPlan::env`) and forwarded into the
//!   container with bare `-e NAME` — the value never appears in `ps`.
//! - **No orphans (invariant 4).** Containers carry the `fletch.host-pid` /
//!   `fletch.agent-id` labels the startup sweep keys on (`super::cleanup`).
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

/// Auth variables forwarded into containers with bare `-e NAME` (values ride
/// the docker CLI's process env — invariant 3). `ANTHROPIC_BASE_URL` /
/// `ANTHROPIC_AUTH_TOKEN` cover proxy setups; a bare `-e` for an unset variable
/// forwards nothing, so the list is passed unconditionally and argv stays
/// deterministic. Must stay a superset of every var [`auth::resolve`] (D1) can
/// emit — a resolved value with no matching bare `-e` would sit on the CLI
/// process env yet never reach the container.
const AUTH_ENV_VARS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
];

/// Signal/removal docker calls during teardown.
const KILL_TIMEOUT: Duration = Duration::from_secs(10);
/// Liveness lookups (`docker inspect`).
const INSPECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a TERM'd container gets to exit before escalating to KILL —
/// same order as the session-side process-group escalation grace windows.
const TERM_GRACE: Duration = Duration::from_millis(500);

/// Launch knobs read from the `settings` table, mirrored in-process (the spawn
/// path has no DB handle — same pattern as `sandbox::set_selected_engine_kind`).
/// Seeded at startup in `lib.rs setup`; slice C2 adds the settings UI whose
/// set-commands will keep the mirror in sync mid-run.
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
        // Build progress goes to the log until slice C2 wires it to UI events.
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
        std::fs::create_dir_all(&claude_dir).map_err(|e| {
            Error::Other(format!(
                "preparing Docker sandbox config mount {} failed: {e}",
                claude_dir.display()
            ))
        })?;
        if let Some(dir) = &claude_config_dir {
            std::fs::create_dir_all(dir).map_err(|e| {
                Error::Other(format!(
                    "preparing Docker sandbox config mount {} failed: {e}",
                    dir.display()
                ))
            })?;
        }

        let prefix_args = run_args(&RunSpec {
            interactive: ctx.interactive,
            name: &name,
            agent_id: ctx.agent_id,
            writable_root: ctx.writable_root,
            rpc_dir: ctx.rpc_dir,
            home: ctx.home,
            cwd: ctx.cwd,
            claude_config_dir: claude_config_dir.as_deref(),
            memory: non_blank(settings.memory.as_deref()).unwrap_or(DEFAULT_MEMORY),
            cpus: non_blank(settings.cpus.as_deref()).unwrap_or(DEFAULT_CPUS),
            image: &image,
            agent_bin,
        });

        // Values for the bare `-e NAME` forwards above — set on the docker
        // CLI process, never in argv (invariant 3).
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
        apply_container_auth(&mut env, auth::resolve())?;

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
        let KillPlan::Container { name } = plan else {
            return Ok(());
        };
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

    /// Containers die independently of the host (daemon stop, OOM kill), so
    /// liveness asks the daemon rather than the local CLI child. Any failure
    /// to answer — container gone, daemon down, timeout — reads as dead: the
    /// user's remedy is the same, and this is the health surface the UI polls
    /// (slice C2).
    fn is_alive(&self, plan: &KillPlan) -> bool {
        let KillPlan::Container { name } = plan else {
            return true;
        };
        container_running(name)
    }

    fn describe_exit(&self, _plan: &KillPlan, code: i32) -> Option<String> {
        describe_exit_code(code)
    }
}

/// Launch-blocking message when the container auth chain (D1) resolves nothing.
/// Kept as one stable, matchable string so slice C2 can turn it into a Settings
/// call-to-action; the wording tells the user exactly what to do.
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
/// expects them — the same rule as seatbelt's `claude_config_extra`. `None`
/// when unset or when it's just the default `~/.claude` (already mounted).
fn nondefault_claude_config_dir(home: &Path) -> Option<PathBuf> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from)?;
    if dir == home.join(".claude") {
        return None;
    }
    Some(dir)
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
    memory: &'a str,
    cpus: &'a str,
    image: &'a str,
    agent_bin: &'a str,
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
    // else from the host enters the container.
    let mut mounts = vec![
        spec.writable_root.to_path_buf(),
        spec.rpc_dir.to_path_buf(),
        spec.home.join(".claude"),
    ];
    mounts.extend(spec.claude_config_dir.map(Path::to_path_buf));
    for mount in &mounts {
        let path = mount.to_string_lossy();
        args.push("-v".into());
        args.push(format!("{path}:{path}"));
    }
    args.push("-w".into());
    args.push(spec.cwd.to_string_lossy().into_owned());
    // Bare `-e NAME` forwards from the docker CLI's own environment without
    // the value ever appearing in argv (invariant 3 for the auth vars).
    let mut forwarded: Vec<&str> = vec!["HOME", "FLETCH_RPC_DIR", "TERM", "COLORTERM"];
    if spec.claude_config_dir.is_some() {
        forwarded.push("CLAUDE_CONFIG_DIR");
    }
    forwarded.extend(AUTH_ENV_VARS);
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
            memory: "4g",
            cpus: "2",
            image: "fletch-agent:abc123def456",
            agent_bin: "claude",
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
        let args = run_args(&test_spec(false));
        assert_eq!(
            values_of(&args, "-v"),
            vec![
                "/Users/u/.fletch/worktrees/orkney:/Users/u/.fletch/worktrees/orkney",
                "/Users/u/.fletch/rpc/orkney:/Users/u/.fletch/rpc/orkney",
                "/Users/u/.claude:/Users/u/.claude",
            ],
        );
        assert_eq!(
            values_of(&args, "-w"),
            vec!["/Users/u/.fletch/worktrees/orkney/repo"],
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

    /// Regression, live mirror → spawn: a token set through the same in-process
    /// mirror the `set_container_auth_token` command writes
    /// (`auth::set_stored_token`, called after its DB write) must reach the
    /// *next* launch's container env with no app restart. Unlike the
    /// constructed-`Resolved` tests above, this drives the real
    /// `set_stored_token` → `auth::resolve()` → `apply_container_auth` path, so
    /// it fails if `resolve` ever stops reading the live mirror at spawn time
    /// (the original bug: the launch path ignored the stored token until a
    /// restart re-seeded another source). Stored-token is first-hit in the
    /// chain, so this is deterministic regardless of the test host's shell env
    /// or `~/.claude`.
    #[test]
    fn pasted_token_reaches_next_launch_without_restart() {
        auth::set_stored_token(Some("sk-ant-oat-regression".into()));
        let mut env: Vec<(String, String)> = Vec::new();
        let applied = apply_container_auth(&mut env, auth::resolve());
        // Restore the process global before asserting so a failure can't leak
        // into other tests sharing the mirror.
        auth::set_stored_token(None);
        applied.expect("a stored token must resolve to usable auth");

        assert!(
            env.iter().any(|(k, v)| k == "CLAUDE_CODE_OAUTH_TOKEN"
                && v == "sk-ant-oat-regression"),
            "stored token missing from launch env: {:?}",
            env.iter().map(|(k, _)| k).collect::<Vec<_>>(),
        );
        // Present in env is not enough — it only enters the container if the
        // var also has a bare `-e` forward in the argv.
        assert!(
            AUTH_ENV_VARS.contains(&"CLAUDE_CODE_OAUTH_TOKEN"),
            "token var set on the CLI process but never forwarded into the container",
        );
    }

    #[test]
    fn argv_mounts_and_forwards_nondefault_claude_config_dir() {
        let mut spec = test_spec(false);
        spec.claude_config_dir = Some(Path::new("/Users/u/.claude-eve"));
        let args = run_args(&spec);
        assert!(values_of(&args, "-v").contains(&"/Users/u/.claude-eve:/Users/u/.claude-eve"));
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

    /// D1 swap, happy path: a resolved auth env lands on the docker CLI process
    /// env verbatim, and every var it carries has a matching bare `-e NAME` in
    /// argv — so the value forwards into the container yet never appears in
    /// argv (invariant 3). Guards against `AUTH_ENV_VARS` drifting behind the
    /// set `auth::resolve` can emit.
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

        // Every forwarded var name has a bare `-e NAME` in argv, and no value
        // (secret or otherwise) leaks into argv.
        let args = run_args(&test_spec(false));
        let forwarded = values_of(&args, "-e");
        for (name, _) in &env {
            assert!(
                forwarded.contains(&name.as_str()),
                "resolved var {name} has no bare -e in argv (AUTH_ENV_VARS drifted)",
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
        for d in [&root, &rpc, &home.join(".claude")] {
            std::fs::create_dir_all(d).unwrap();
        }
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
            memory: "256m",
            cpus: "1",
            image: "busybox",
            agent_bin: "echo",
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

    /// Integration: kill/is_alive against a live container.
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
        assert!(engine.is_alive(&plan), "fresh container should be running");
        engine.kill(&plan).unwrap();
        assert!(!engine.is_alive(&plan), "killed container reads as dead");
    }
}
