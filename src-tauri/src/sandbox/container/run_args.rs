//! The container `run` argv builder. Pure over its [`RunSpec`] so the argv
//! shape is unit-testable without a daemon.
//!
//! Invariants it encodes: mounts land at identical host paths (1), borrowed
//! history is read-only (2), credential values reach the container as bare
//! `-e NAME` and never appear in argv (3), claude's shared config dir is
//! read-only but for its named carve-outs (5).

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::sandbox::policy::{
    CLAUDE_CREDENTIALS_FILE, CLAUDE_EPHEMERAL_RUNTIME_SUBDIRS, CLAUDE_PROJECTS_SUBDIR,
};

use super::labels;

/// The one file under a claude config dir that stays writable when the dir is
/// bind-mounted read-only: claude's OAuth refresh must land on the host for the
/// `CredentialsFile` chain (see [`super::auth`]) to see the rotated token.
/// Shared with seatbelt via [`CLAUDE_CREDENTIALS_FILE`] so the two can't drift.
pub(crate) const CREDENTIALS_FILE: &str = CLAUDE_CREDENTIALS_FILE;

/// Resource caps when the user has set none — a property of what an agent needs
/// to build and test, so every runtime shares them.
pub(crate) const DEFAULT_MEMORY: &str = "4g";
pub(crate) const DEFAULT_CPUS: &str = "2";

/// Subdirs Claude Code rewrites every session, which a bare write to the
/// read-only config dir would fail with `EROFS`; each gets a throwaway tmpfs
/// overlay instead. Deliberately narrow: everything else stays read-only so a
/// prompt-injected agent can't plant a host-executed hook/plugin/skill, a
/// `settings.json` grant, or a memory entry a later session would trust
/// (invariant 5). Shared with seatbelt via [`CLAUDE_EPHEMERAL_RUNTIME_SUBDIRS`].
const EPHEMERAL_RUNTIME_SUBDIRS: &[&str] = CLAUDE_EPHEMERAL_RUNTIME_SUBDIRS;

/// Claude's session-transcript subdir. Unlike [`EPHEMERAL_RUNTIME_SUBDIRS`] it
/// is bound to a *persistent* per-agent host dir so `--resume` survives
/// container recreation — see [`push_claude_config_mount`]. Shared with seatbelt
/// via [`CLAUDE_PROJECTS_SUBDIR`].
const PROJECTS_SUBDIR: &str = CLAUDE_PROJECTS_SUBDIR;

/// The provider-specific config/data mounts and config-dir env a launch needs.
/// Exactly one variant is populated per launch and [`run_args`] matches once on
/// the whole thing.
pub(crate) enum ProviderMounts<'a> {
    /// Claude: `~/.claude` (and any non-default `CLAUDE_CONFIG_DIR`) bind-mounted
    /// **read-only** but for the carve-outs in [`push_claude_config_mount`]
    /// (invariant 5).
    Claude {
        /// Non-default `CLAUDE_CONFIG_DIR`, mounted + forwarded alongside
        /// `~/.claude`. `None` when unset or resolving to the default.
        config_dir: Option<&'a Path>,
        /// Whether `~/.claude/.credentials.json` exists — gates its RW overlay.
        credentials_rw: bool,
        /// Same, for the non-default `CLAUDE_CONFIG_DIR`.
        config_dir_credentials_rw: bool,
        /// Per-agent host dir bound read-write over each config dir's
        /// `projects/`, so transcripts survive container recreation while the
        /// shared `~/.claude` stays read-only.
        projects_src: &'a Path,
    },
    /// Codex: `$CODEX_HOME`/`~/.codex` bind-mounted **read-write** at its host
    /// path (auth.json refresh + session rollouts must persist).
    Codex {
        config_dir: &'a Path,
        /// Forward `CODEX_HOME` (a non-default `$CODEX_HOME` only).
        forward_home: bool,
    },
    /// OpenCode: its data dir (accounts DB / `auth.json` / session storage the
    /// host transcript reader tails) plus its config dir when it exists, both
    /// bind-mounted **read-write**.
    Opencode {
        data_dir: &'a Path,
        config_dir: Option<&'a Path>,
        /// Forward `XDG_DATA_HOME` / `XDG_CONFIG_HOME` (non-default bases only).
        forward_xdg_data_home: bool,
        forward_xdg_config_home: bool,
    },
    /// Pi: `~/.pi` bind-mounted **read-write** — auth, settings, and the
    /// `agent/sessions/` transcripts the host reader tails at the identical path.
    Pi { data_dir: &'a Path },
    /// Cursor: `~/.cursor` bind-mounted **read-write** for the session logs the
    /// host reader tails. Carries no credential (the login token is
    /// keychain-bound); auth is the forwarded `CURSOR_API_KEY` only.
    Cursor { data_dir: &'a Path },
}

/// Everything [`run_args`] needs, bundled so the builder stays pure.
pub(crate) struct RunSpec<'a> {
    pub interactive: bool,
    pub name: &'a str,
    pub agent_id: &'a str,
    pub writable_root: &'a Path,
    pub rpc_dir: &'a Path,
    pub home: &'a Path,
    pub cwd: &'a Path,
    /// A workflow step agent's blackboard, bound read-write at its identical
    /// host path and forwarded as `WF_BLACKBOARD`. `None` for ordinary agents.
    pub blackboard: Option<&'a Path>,
    /// The launching provider's config/data mounts + config-dir env forwards.
    pub mounts: ProviderMounts<'a>,
    /// Object stores borrowed via git alternates, bound read-only at their
    /// identical host paths. Empty for a worktree or a full-copy clone.
    pub borrowed_object_stores: &'a [PathBuf],
    pub memory: &'a str,
    pub cpus: &'a str,
    pub image: &'a str,
    pub agent_bin: &'a str,
    /// Auth var *names* the chain resolved ([`resolve`]), forwarded as bare
    /// `-e NAME` so values never appear in argv (invariant 3). Only the resolved
    /// set: an ambient credential the chain didn't pick must not reach the
    /// container and override the resolved login.
    ///
    /// [`resolve`]: crate::sandbox::container::auth::resolve
    pub auth_vars: &'a [&'a str],
}

/// The `run` argv (everything after the runtime binary), ending with
/// `<image> <agent_bin>` so the caller can append agent CLI args — the
/// `prefix_args` contract of [`SandboxEngine::launch_agent`].
///
/// [`SandboxEngine::launch_agent`]: crate::sandbox::engine::SandboxEngine::launch_agent
pub(crate) fn run_args(spec: &RunSpec<'_>) -> Vec<String> {
    let mut args: Vec<String> = vec!["run".into(), "--rm".into(), "--init".into()];
    if spec.interactive {
        args.push("-t".into());
    }
    args.push("-i".into());
    args.push("--name".into());
    args.push(spec.name.into());
    args.push("--label".into());
    args.push(labels::host_pid_label());
    args.push("--label".into());
    args.push(labels::agent_id_label(spec.agent_id));
    // Mounts at identical host paths (invariant 1). Exactly these — nothing
    // else from the host enters the container.
    for path in [spec.writable_root, spec.rpc_dir] {
        let path = path.to_string_lossy();
        args.push("-v".into());
        args.push(format!("{path}:{path}"));
    }
    // At its identical host path so `$WF_BLACKBOARD` resolves the same
    // in-container as on the host (invariant 1).
    if let Some(board) = spec.blackboard {
        push_rw_bind(&mut args, board);
    }
    // Identical host path so the alternates file resolves in-container with no
    // rewriting; read-only so borrowed history is readable but the source store
    // can't be mutated from inside the container (invariant 2). Only `objects`,
    // never `.git/hooks` or config.
    for store in spec.borrowed_object_stores {
        let path = store.to_string_lossy();
        args.push("-v".into());
        args.push(format!("{path}:{path}:ro"));
    }
    // Provider config-dir mount(s), layered after the workspace/mailbox/object
    // stores.
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
    // Bare `-e NAME` forwards from the runtime CLI's own environment, so no
    // value appears in argv (invariant 3).
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
        // No config-dir env; CURSOR_API_KEY rides `spec.auth_vars`.
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

/// Every *host* path [`run_args`] turns into a bind mount, in argv order —
/// excluding the tmpfs overlays (no source) and the `projects/` target (its
/// source, `projects_src`, is listed). A runtime that vets mount sources before
/// launching (see `podman::machine`) must see exactly what will be bound, so a
/// mount added to [`run_args`] without an entry here would go unvetted.
pub(crate) fn mount_sources(spec: &RunSpec<'_>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = vec![spec.writable_root.into(), spec.rpc_dir.into()];
    if let Some(board) = spec.blackboard {
        out.push(board.into());
    }
    out.extend(spec.borrowed_object_stores.iter().cloned());
    match &spec.mounts {
        // The `.credentials.json` overlays are subpaths of the config dirs
        // listed here, so a containment check over these covers them too.
        ProviderMounts::Claude {
            config_dir,
            projects_src,
            ..
        } => {
            out.push(spec.home.join(".claude"));
            if let Some(dir) = config_dir {
                out.push((*dir).into());
            }
            out.push((*projects_src).into());
        }
        ProviderMounts::Codex { config_dir, .. } => out.push((*config_dir).into()),
        ProviderMounts::Opencode {
            data_dir,
            config_dir,
            ..
        } => {
            out.push((*data_dir).into());
            if let Some(dir) = config_dir {
                out.push((*dir).into());
            }
        }
        ProviderMounts::Pi { data_dir } => out.push((*data_dir).into()),
        ProviderMounts::Cursor { data_dir } => out.push((*data_dir).into()),
    }
    out
}

/// Bind-mount `dir` **read-write** at its identical host path (invariant 1), so
/// the CLI's in-place auth refresh and session writes stay visible to the
/// host-side transcript reader.
fn push_rw_bind(args: &mut Vec<String>, dir: &Path) {
    let path = dir.to_string_lossy();
    args.push("-v".into());
    args.push(format!("{path}:{path}"));
}

/// Create a claude config dir and its overlay mountpoints before `-v` sees it:
/// a missing source is materialized root-owned by the runtime, and each overlay
/// target must already exist *inside* the dir because the read-only parent bind
/// can't grow a fresh mountpoint at run time.
pub(crate) fn prepare_config_mount_dir(dir: &Path) -> Result<()> {
    let overlays = EPHEMERAL_RUNTIME_SUBDIRS
        .iter()
        .copied()
        .chain(std::iter::once(PROJECTS_SUBDIR));
    for target in std::iter::once(dir.to_path_buf()).chain(overlays.map(|s| dir.join(s))) {
        std::fs::create_dir_all(&target).map_err(|e| {
            Error::Other(format!(
                "preparing container sandbox config mount {} failed: {e}",
                target.display()
            ))
        })?;
    }
    Ok(())
}

/// Bind-mount a claude config dir **read-only**, then layer the sole writable
/// exceptions on top (order matters — the runtime layers later `-v`s over
/// earlier ones). The dir is shared host state whose `settings.json` can define
/// hooks Claude Code runs on the *host*, so a container agent must not be able
/// to write it (invariant 5). The exceptions:
///
/// - `.credentials.json` (only when `credentials_rw`, since a bare `-v` on a
///   missing source is materialized root-owned) — so OAuth refresh persists.
/// - each [`EPHEMERAL_RUNTIME_SUBDIRS`] entry — throwaway tmpfs, no host source.
/// - `projects/` — backed by the per-agent `projects_src` so `--resume` survives
///   container recreation while the shared `~/.claude/projects` (other agents'
///   transcripts, global memory) stays unreachable. The one bind that departs
///   from invariant 1's identical-host-path rule; claude reaches it only through
///   its config dir, so it doesn't need one.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Podman refuses a launch whose sources its machine doesn't share, so an
    /// unvetted `-v` would be bound and arrive empty.
    #[test]
    fn every_bind_source_is_covered_by_mount_sources() {
        let root = Path::new("/tmp/fletch-run-args/work");
        let rpc = Path::new("/tmp/fletch-run-args/rpc");
        let home = Path::new("/tmp/fletch-run-args/home");
        let board = Path::new("/tmp/fletch-run-args/board");
        let alt = Path::new("/tmp/fletch-run-args/alt");
        let stores = vec![
            PathBuf::from("/tmp/fletch-run-args/store-a/objects"),
            PathBuf::from("/tmp/fletch-run-args/store-b/objects"),
        ];
        let projects = root.join("claude-projects");

        let shapes = [
            ProviderMounts::Claude {
                config_dir: Some(alt),
                credentials_rw: true,
                config_dir_credentials_rw: true,
                projects_src: &projects,
            },
            ProviderMounts::Codex {
                config_dir: alt,
                forward_home: true,
            },
            ProviderMounts::Opencode {
                data_dir: alt,
                config_dir: Some(home),
                forward_xdg_data_home: true,
                forward_xdg_config_home: true,
            },
            ProviderMounts::Pi { data_dir: alt },
            ProviderMounts::Cursor { data_dir: alt },
        ];

        for mounts in shapes {
            let spec = RunSpec {
                interactive: true,
                name: "fletch-agent-test",
                agent_id: "agent-1",
                writable_root: root,
                rpc_dir: rpc,
                home,
                cwd: root,
                blackboard: Some(board),
                mounts,
                borrowed_object_stores: &stores,
                memory: DEFAULT_MEMORY,
                cpus: DEFAULT_CPUS,
                image: "fletch-agent:cafe00000000",
                agent_bin: "claude",
                auth_vars: &["ANTHROPIC_API_KEY"],
            };
            let sources = mount_sources(&spec);
            let args = run_args(&spec);

            let mut binds = 0;
            for (flag, value) in args.iter().zip(args.iter().skip(1)) {
                match flag.as_str() {
                    "-v" => {
                        binds += 1;
                        // `src:dst[:ro]` — the source is the leading segment.
                        let src = Path::new(value.split(':').next().unwrap());
                        assert!(
                            sources.iter().any(|s| src == s || src.starts_with(s)),
                            "unvetted bind source {src:?} (vetted: {sources:?})",
                        );
                    }
                    "--tmpfs" => assert!(
                        !sources.iter().any(|s| s == Path::new(value)),
                        "a tmpfs overlay has no host source: {value}",
                    ),
                    _ => {}
                }
            }
            assert!(
                binds >= 3,
                "expected the workspace/rpc/config binds at least"
            );
        }
    }
}
