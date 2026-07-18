//! The `docker run` argv builder: the provider-agnostic `--rm --init` shape,
//! the mounts at identical host paths (invariant 1), the per-provider config/
//! data mounts, and the bare `-e NAME` forwards (invariant 3). Pure over its
//! [`RunSpec`] so the argv shape is unit-testable without a daemon.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::sandbox::docker::cleanup;
use crate::sandbox::policy::{
    CLAUDE_CREDENTIALS_FILE, CLAUDE_EPHEMERAL_RUNTIME_SUBDIRS, CLAUDE_PROJECTS_SUBDIR,
};

/// The one file under a claude config dir that stays writable when the dir is
/// bind-mounted read-only: claude's OAuth refresh rewrites the rotated token
/// here, and the `CredentialsFile` auth chain (see [`super::auth`]) needs that
/// write to land on the host. Mounted read-write on top of the read-only dir.
/// The name is shared with seatbelt via [`CLAUDE_CREDENTIALS_FILE`] so
/// the two engines can't drift.
pub(super) const CREDENTIALS_FILE: &str = CLAUDE_CREDENTIALS_FILE;

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

/// The provider-specific config/data mounts and config-dir env a launch needs —
/// one variant per supported provider. Replaces the additive per-provider field
/// clusters `RunSpec` used to carry (claude_config_dir, codex_config_dir, …),
/// which grew unwieldy at four providers with three-quarters of them inert on any
/// given launch. Each variant holds exactly its own provider's data and
/// [`run_args`] matches once on the whole thing. Claude's read-only-except-
/// carve-outs treatment is unique; the other three are read-write binds at
/// identical host paths, differing only in which dirs they mount and which env
/// var (if any) they forward.
pub(super) enum ProviderMounts<'a> {
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
pub(super) struct RunSpec<'a> {
    pub interactive: bool,
    pub name: &'a str,
    pub agent_id: &'a str,
    pub writable_root: &'a Path,
    pub rpc_dir: &'a Path,
    pub home: &'a Path,
    pub cwd: &'a Path,
    /// A workflow step agent's blackboard directory, bind-mounted read-write at
    /// its identical host path and forwarded as `WF_BLACKBOARD`. `None` for
    /// ordinary agents.
    pub blackboard: Option<&'a Path>,
    /// The launching provider's config/data mounts + config-dir env forwards.
    pub mounts: ProviderMounts<'a>,
    /// Object stores borrowed via git alternates (a `--shared` clone),
    /// bind-mounted read-only at their identical host paths. Empty for a
    /// worktree or an old full-copy clone.
    pub borrowed_object_stores: &'a [PathBuf],
    pub memory: &'a str,
    pub cpus: &'a str,
    pub image: &'a str,
    pub agent_bin: &'a str,
    /// Auth var *names* the chain resolved ([`resolve`]), each forwarded
    /// with a bare `-e NAME` so its value (set on the docker CLI process env)
    /// never appears in argv. Only the resolved set is forwarded: an ambient
    /// credential the chain didn't pick must not reach the container and
    /// override the resolved login.
    ///
    /// [`resolve`]: crate::sandbox::docker::auth::resolve
    pub auth_vars: &'a [&'a str],
}

/// The `docker run` argv (everything after the docker binary), ending with
/// `<image> <agent_bin>` so the caller can append agent CLI args — the
/// `prefix_args` contract of [`SandboxEngine::launch_agent`].
///
/// [`SandboxEngine::launch_agent`]: crate::sandbox::engine::SandboxEngine::launch_agent
pub(super) fn run_args(spec: &RunSpec<'_>) -> Vec<String> {
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
pub(super) fn prepare_config_mount_dir(dir: &Path) -> Result<()> {
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
