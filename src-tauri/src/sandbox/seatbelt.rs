//! Per-agent macOS sandbox profile — the single, unified isolation layer for
//! every agent Fletch runs.
//!
//! The app launches each agent (Claude *and* the per-turn agents — codex,
//! cursor, opencode, pi, antigravity) under `sandbox-exec` with this profile,
//! rather than relying on each CLI's own sandbox. `sandbox-exec` is just the
//! process wrapper around the PTY/exec child, so terminal streaming and startup
//! timing are unchanged while *writes* are constrained to the agent's parent dir
//! (under `~/.fletch/workspaces/<id>/`) plus standard state/cache locations and
//! each agent's own on-disk session store. The agent's per-repo checkouts live
//! as subdirs of that parent, so each inherits the writable allowance.
//!
//! Because confinement is by *write* path (reads and network stay open via
//! `allow default`), each agent that the wrapper covers must have its
//! out-of-checkout write locations (session transcripts, config, auth refresh)
//! on the allow-list below — otherwise it can't persist its own state. That
//! covers the agents' own dot-dir stores plus the standard per-user
//! cache/state dirs in both XDG (`~/.cache`, `~/.config`, `~/.local`) and
//! macOS-native (`~/Library/Caches`, `~/Library/Application Support`) form,
//! since the agents' subprocess toolchains and macOS frameworks write to the
//! latter. The agent CLIs' own sandboxes are disabled (e.g. codex runs
//! `danger-full-access`) so the two don't fight, leaving `sandbox-exec` as the
//! sole boundary.

use std::path::{Path, PathBuf};

use super::engine::{AgentLaunchCtx, EngineKind, Keepalive, KillHandle, LaunchPlan, SandboxEngine};
use crate::error::{Error, Result};

pub struct SandboxExecEngine;

impl SandboxEngine for SandboxExecEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::SandboxExec
    }

    fn launch_agent(&self, ctx: &AgentLaunchCtx, agent_bin: &str) -> Result<LaunchPlan> {
        let claude_config_dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from);
        let profile_text = build_profile(
            ctx.writable_root,
            ctx.rpc_dir,
            ctx.home,
            claude_config_dir.as_deref(),
        )?;
        let profile_file = profile_tempfile(&profile_text)?;
        let profile_path = profile_file
            .path()
            .to_str()
            .ok_or_else(|| Error::Other("profile path not utf-8".into()))?
            .to_string();
        Ok(LaunchPlan {
            program: PathBuf::from(SANDBOX_EXEC),
            prefix_args: vec!["-f".into(), profile_path, agent_bin.to_string()],
            env: vec![],
            keepalive: Keepalive::Profile(profile_file),
            // sandbox-exec is a plain process wrapper — the session's own
            // process-group escalation tears everything down; the trait's
            // default no-op `kill` applies.
            kill: KillHandle::ProcessGroup,
        })
    }
}

/// The macOS sandbox wrapper. Every confined process (agents *and* the Run
/// panel) is launched as `sandbox-exec -f <profile> <program> …`.
pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// PTY / device write rules shared by every profile — terminal programs need
/// these ttys and null/zero devices regardless of what else they may write.
const DEVICE_WRITE_RULES: &str = r#";; PTYs and basic device files are required for terminal programs.
(allow file-write* (literal "/dev/null") (literal "/dev/zero"))
(allow file-write*
  (regex #"^/dev/tty[^/]*$")
  (regex #"^/dev/ptmx$")
  (regex #"^/dev/pts/[0-9]+$"))"#;

/// Per-user cache/state dirs that both agents and Run processes must be able to
/// write: package-manager caches and XDG + macOS-native app state. Returned as
/// sbpl-quoted `subpath` argument strings (without the enclosing `(subpath …)`).
fn standard_state_dirs(home_s: &str) -> Vec<String> {
    [
        ".npm",
        ".cache",
        ".config",
        ".local",
        "Library/Caches",
        "Library/Application Support",
    ]
    .iter()
    .map(|d| sbpl_string(&format!("{home_s}/{d}")))
    .collect()
}

/// Toolchain state dirs the Run panel additionally grants so real project
/// builds succeed. These hold package caches, downloaded toolchains, and —
/// for some — PATH-resolved binaries (`~/.cargo/bin`, `~/go/bin`,
/// `~/.rbenv/shims`). That last part is a residual hijack surface, which is
/// why this superset is **Run-only** and deliberately kept off the agent
/// profile: a running project legitimately needs its toolchain to write here,
/// whereas an agent editing source does not.
const RUN_TOOLCHAIN_DIRS: &[&str] = &[
    ".cargo",         // Rust: registry, git checkouts, installed bins
    ".rustup",        // Rust: downloaded toolchains (rust-toolchain.toml)
    "go",             // Go: GOPATH — module cache (pkg/mod) + installed bins
    ".bun",           // Bun: global install cache
    "Library/pnpm",   // pnpm: content-addressable store (macOS default)
    ".bundle",        // Bundler: config + cache
    ".gem",           // RubyGems: default gem home
    ".rbenv",         // rbenv: shims + installed Ruby versions
    ".rvm",           // rvm: alternative Ruby version manager
    "Library/Python", // pip --user / no-venv user site-packages
];

/// Build the SBPL profile for a **Run-panel** process (setup/dev command).
///
/// Same shape as [`build_profile`] — reads and network stay open (`allow
/// default`); only *writes* are confined — but tuned for arbitrary project
/// build toolchains rather than agent CLIs. `writable_root` is the repo
/// checkout the command runs in (build artifacts, `node_modules`, `.venv`,
/// `target` all live inside it). On top of the checkout and the shared cache
/// dirs, it grants [`RUN_TOOLCHAIN_DIRS`] so cargo/go/bundler/pnpm/bun runs
/// don't fail-closed on their out-of-tree writes.
///
/// Unlike the agent profile it needs no rpc mailbox or agent state dirs — a
/// Run process neither speaks RPC nor persists agent transcripts.
///
/// `extra_writable` grants additional out-of-checkout paths the specific Run
/// target needs. The Run panel passes the target's resolved git *common dir*:
/// a project may write its own git metadata (objects, refs, `worktrees/`
/// admin data on `git worktree add`), and when the target is itself a linked
/// worktree that common dir lives outside `writable_root` — so without this a
/// nested Fletch's `git worktree add` (and later commits) fail closed. For a
/// normal repo the common dir is already inside `writable_root`, so it's a
/// harmless duplicate.
pub fn build_run_profile(
    writable_root: &Path,
    home: &Path,
    extra_writable: &[PathBuf],
) -> Result<String> {
    let writable_root = canonical(writable_root)?;
    let home = canonical(home)?;
    let writable_root_s = sbpl_string(&writable_root.to_string_lossy());
    let home_s = home.to_string_lossy();

    let mut subpaths = vec![
        writable_root_s,
        sbpl_string("/private/tmp"),
        sbpl_string("/private/var/folders"),
        sbpl_string("/private/var/tmp"),
    ];
    subpaths.extend(standard_state_dirs(&home_s));
    subpaths.extend(
        RUN_TOOLCHAIN_DIRS
            .iter()
            .map(|d| sbpl_string(&format!("{home_s}/{d}"))),
    );
    subpaths.extend(
        extra_writable
            .iter()
            .map(|p| sbpl_string(&p.to_string_lossy())),
    );
    let writable_block = subpaths
        .iter()
        .map(|s| format!("(subpath {s})"))
        .collect::<Vec<_>>()
        .join("\n  ");

    Ok(format!(
        r#"(version 1)
(allow default)

;; Block writes everywhere by default, then re-allow specific subpaths.
(deny file-write*)
(allow file-write*
  {writable_block})

{DEVICE_WRITE_RULES}
"#
    ))
}

/// Mailbox root (`$FLETCH_RPC_ROOT`) for a **nested** Fletch launched as a Run
/// process. The Run profile denies writes to the host's `~/.fletch/rpc`, so a
/// nested instance can't create its agents' mailboxes there. Redirect it under
/// the system temp dir, which [`build_run_profile`] already grants (macOS
/// `$TMPDIR` resolves under `/private/var/folders`). Keyed by a hash of the
/// checkout path so two nested instances never collide on a shared agent id,
/// and kept off the host's real mailbox root so nested traffic can't touch host
/// channels.
pub fn nested_rpc_root(writable_root: &Path) -> PathBuf {
    nested_state_root("rpc", writable_root)
}

/// Checkouts root (`$FLETCH_WORKSPACES_ROOT`) for a **nested** Fletch launched as
/// a Run process — the sibling of [`nested_rpc_root`] for the same reason: the
/// Run profile denies writes to the host's `~/.fletch/workspaces`, so a nested
/// instance can't create its agents' checkouts there. (The checkout's
/// git *admin* data lands in the source repo's git common dir, which the Run
/// profile grants separately — see `build_run_profile`.)
pub fn nested_checkouts_root(writable_root: &Path) -> PathBuf {
    nested_state_root("worktrees", writable_root)
}

/// Shared builder for a nested instance's redirected state root of a given
/// `kind` (`rpc`, `worktrees`): `<tmp>/fletch-<kind>/<host-pid>/<key>`.
fn nested_state_root(kind: &str, writable_root: &Path) -> PathBuf {
    // Hash the full path, not a char-sanitized form: sanitizing collides
    // (`my-app` vs `my.app` both → `my-app`). A readable last-component prefix
    // keeps the dir eyeball-able when debugging.
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    writable_root.to_string_lossy().hash(&mut hasher);
    let name: String = writable_root
        .file_name()
        .map(|n| {
            n.to_string_lossy()
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .collect()
        })
        .unwrap_or_default();
    let key = format!("{name}-{:016x}", hasher.finish());
    // Scope by host pid so a concurrently-running Fletch (or the nested Fletch
    // itself, which runs the same startup sweep) can tell our live roots from a
    // dead instance's leftovers — see `cleanup_nested_state_roots`.
    nested_state_base(kind)
        .join(std::process::id().to_string())
        .join(key)
}

/// Parent dir holding every host instance's nested `kind` roots (one subdir
/// per host pid).
fn nested_state_base(kind: &str) -> PathBuf {
    std::env::temp_dir().join(format!("fletch-{kind}"))
}

/// Best-effort sweep of nested mailbox roots left by *dead* host instances.
/// Call at app startup. Roots live under `<base>/<host-pid>/`, so we remove
/// only pid-subdirs whose owner is gone — never a live instance's (a second
/// Fletch open side-by-side, or our own), which would break its running nested
/// Fletch mid-read.
pub fn cleanup_nested_rpc_roots() {
    cleanup_nested_state_roots_in(&nested_state_base("rpc"));
}

/// Sibling of [`cleanup_nested_rpc_roots`] for redirected checkout roots — same
/// pid-keyed, dead-only reclamation.
pub fn cleanup_nested_checkouts_roots() {
    cleanup_nested_state_roots_in(&nested_state_base("worktrees"));
}

fn cleanup_nested_state_roots_in(base: &Path) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let dead = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<i32>().ok())
            .is_some_and(|pid| !pid_alive(pid));
        if dead {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Whether a process with `pid` currently exists — a signal-0 `kill` probe.
/// `Err` (ESRCH, or EPERM on a reused pid we don't own) is treated as gone,
/// which only ever under-reclaims; a live Fletch we own always probes `Ok`.
/// `pub(crate)` so the docker orphan sweep (`sandbox/docker/cleanup.rs`) can
/// share the exact liveness semantics instead of duplicating them.
#[cfg(unix)]
pub(crate) fn pid_alive(pid: i32) -> bool {
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
}

#[cfg(not(unix))]
pub(crate) fn pid_alive(_pid: i32) -> bool {
    true // can't probe — never reclaim
}

/// Build the SBPL profile. `writable_root` is the agent's parent dir;
/// `rpc_dir` is its private file-mailbox (`~/.fletch/rpc/<id>/`), which lives
/// outside the checkout tree and so needs its own allow entry.
/// `claude_config_dir` is the value of `CLAUDE_CONFIG_DIR` the agent runs with
/// (`None` = default `~/.claude`); when set elsewhere the agent writes its
/// config/transcripts/auth there, so it must be writable too.
pub fn build_profile(
    writable_root: &Path,
    rpc_dir: &Path,
    home: &Path,
    claude_config_dir: Option<&Path>,
) -> Result<String> {
    let writable_root = canonical(writable_root)?;
    let rpc_root = canonical(rpc_dir)?;
    let home = canonical(home)?;

    let writable_root_s = sbpl_string(&writable_root.to_string_lossy());
    let rpc_root_s = sbpl_string(&rpc_root.to_string_lossy());
    let home_s = home.to_string_lossy();

    let claude_state = sbpl_string(&format!("{home_s}/.claude"));
    let claude_json = sbpl_string(&format!("{home_s}/.claude.json"));
    // A non-default `CLAUDE_CONFIG_DIR` is where claude actually writes its
    // config/transcripts/auth, so grant it too. Resolve symlinks first so the
    // SBPL path matches what the sandbox sees at write time (every other entry
    // is canonical); then skip it only when it equals the default `{home}/.claude`
    // granted above (`claude_state`), to avoid a redundant entry. `home` is
    // already canonical, but the `.claude` leaf is NOT symlink-resolved —
    // `claude_state` grants that literal path — so compare against it un-resolved.
    // If `~/.claude` is itself a symlink and the config dir points at its
    // resolved target, resolving the leaf here too would treat it as default and
    // drop the grant, yet the literal `claude_state` rule wouldn't cover the
    // target, denying claude's writes. (Docker can resolve both sides because its
    // `~/.claude` bind mount follows the symlink source; the SBPL allow-list can't.)
    let claude_config_extra = claude_config_dir
        .map(resolve_existing_prefix)
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|p| *p != format!("{home_s}/.claude"))
        .map(|p| format!("\n  (subpath {})", sbpl_string(&p)))
        .unwrap_or_default();
    let npm_state = sbpl_string(&format!("{home_s}/.npm"));
    let cache_state = sbpl_string(&format!("{home_s}/.cache"));
    let config_state = sbpl_string(&format!("{home_s}/.config"));
    let local_state = sbpl_string(&format!("{home_s}/.local"));
    // macOS-native equivalents of the XDG cache/state dirs above. Native
    // toolchains the agents invoke (node/npm tooling, git, language SDKs) and
    // macOS framework caches (CFNetwork, fonts, per-bundle state) write here; a
    // denied write ranges from a harmless cache miss to a fatal auth-token
    // write, so allow them on the same "per-user app state, not source/system"
    // basis as `~/.cache`/`~/.config`.
    let library_caches = sbpl_string(&format!("{home_s}/Library/Caches"));
    let library_app_support = sbpl_string(&format!("{home_s}/Library/Application Support"));
    // Per-agent on-disk session stores (transcripts, config, auth) for the
    // per-turn agents now covered by this profile. OpenCode's store lives under
    // `~/.local/share/opencode`, already covered by `local_state`.
    let codex_state = sbpl_string(&format!("{home_s}/.codex"));
    let cursor_state = sbpl_string(&format!("{home_s}/.cursor"));
    let gemini_state = sbpl_string(&format!("{home_s}/.gemini"));
    let pi_state = sbpl_string(&format!("{home_s}/.pi"));

    Ok(format!(
        r#"(version 1)
(allow default)

;; Block writes everywhere by default, then re-allow specific subpaths.
(deny file-write*)
(allow file-write*
  (subpath {writable_root_s})
  (subpath {rpc_root_s})
  (subpath "/private/tmp")
  (subpath "/private/var/folders")
  (subpath "/private/var/tmp")
  (subpath {claude_state})
  (literal {claude_json}){claude_config_extra}
  (subpath {npm_state})
  (subpath {cache_state})
  (subpath {config_state})
  (subpath {local_state})
  (subpath {library_caches})
  (subpath {library_app_support})
  (subpath {codex_state})
  (subpath {cursor_state})
  (subpath {gemini_state})
  (subpath {pi_state}))

{DEVICE_WRITE_RULES}
"#
    ))
}

/// Write an SBPL profile to a private `.sb` tempfile. `sandbox-exec -f <path>`
/// reads it at launch, so it must live at least until the child execs; the
/// caller keeps the returned handle alive and dropping it unlinks the file.
pub fn profile_tempfile(text: &str) -> Result<tempfile::NamedTempFile> {
    use std::io::Write;
    let mut f = tempfile::Builder::new()
        .prefix("fletch-sandbox-")
        .suffix(".sb")
        .tempfile()
        .map_err(|e| Error::Other(format!("create sandbox profile tmp: {e}")))?;
    f.write_all(text.as_bytes())
        .map_err(|e| Error::Other(format!("write sandbox profile: {e}")))?;
    f.flush()
        .map_err(|e| Error::Other(format!("flush sandbox profile: {e}")))?;
    Ok(f)
}

fn sbpl_string(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn canonical(p: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(p).map_err(|e| Error::Other(format!("canonicalize {}: {e}", p.display())))
}

/// Resolve symlinks in the longest existing prefix of `p`, then re-append the
/// not-yet-existing tail. `fs::canonicalize` alone can't be used because it
/// requires the whole path to exist, but `CLAUDE_CONFIG_DIR` may point at a dir
/// claude hasn't created yet. Resolving the existing prefix still collapses the
/// well-known macOS symlinks (`/tmp` → `/private/tmp`, `/var` → `/private/var`),
/// so the emitted SBPL path matches the sandbox's resolved write path. Falls
/// back to `p` unchanged if nothing resolves (e.g. a bogus path).
pub(crate) fn resolve_existing_prefix(p: &Path) -> PathBuf {
    let mut cur = p.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(real) = std::fs::canonicalize(&cur) {
            let mut out = real;
            out.extend(tail.iter().rev());
            return out;
        }
        match cur.file_name() {
            Some(name) => tail.push(name.to_os_string()),
            None => return p.to_path_buf(),
        }
        if !cur.pop() {
            return p.to_path_buf();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn profile_includes_writable_root_and_denies_writes_by_default() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("agent-parent");
        let rpc = td.path().join("rpc");
        let home = td.path().join("home");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&rpc).unwrap();
        std::fs::create_dir_all(&home).unwrap();

        let profile = build_profile(&root, &rpc, &home, None).unwrap();
        let canonical_root = std::fs::canonicalize(&root).unwrap();
        let canonical_rpc = std::fs::canonicalize(&rpc).unwrap();

        assert!(profile.contains("(deny file-write*)"));
        assert!(profile.contains(&format!("\"{}\"", canonical_root.display())));
        // The mailbox lives outside the checkout tree, so it needs its own entry.
        assert!(profile.contains(&format!("\"{}\"", canonical_rpc.display())));
        // macOS-native per-user state dirs, needed by the agents' toolchains.
        assert!(profile.contains("/Library/Caches"));
        assert!(profile.contains("/Library/Application Support"));
    }

    fn sandbox_dirs() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("agent-parent");
        let rpc = td.path().join("rpc");
        let home = td.path().join("home");
        for p in [&root, &rpc, &home] {
            std::fs::create_dir_all(p).unwrap();
        }
        (td, root, rpc, home)
    }

    #[test]
    fn profile_grants_custom_claude_config_dir() {
        // Regression: a sandboxed agent running with CLAUDE_CONFIG_DIR outside
        // ~/.claude couldn't write its config/transcripts/auth, because only
        // ~/.claude was on the allow-list.
        let (_td, root, rpc, home) = sandbox_dirs();
        let cfg = home.join(".claude-eve");
        std::fs::create_dir_all(&cfg).unwrap();

        let profile = build_profile(&root, &rpc, &home, Some(cfg.as_path())).unwrap();
        // The emitted path must be canonical (symlink-resolved) so it matches
        // what the sandbox resolves at write time — e.g. on macOS the tempdir
        // lives under /var → /private/var.
        let canonical_cfg = std::fs::canonicalize(&cfg).unwrap();
        assert!(profile.contains(&format!("(subpath \"{}\")", canonical_cfg.display())));
    }

    #[test]
    fn resolve_existing_prefix_resolves_symlinks_through_missing_leaf() {
        // CLAUDE_CONFIG_DIR may point at a dir claude hasn't created yet, under
        // a symlinked prefix (the /tmp → /private/tmp case). The existing prefix
        // must be symlink-resolved and the missing leaf re-appended verbatim.
        let td = tempfile::tempdir().unwrap();
        let real = td.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = td.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let resolved = resolve_existing_prefix(&link.join("not-created-yet"));
        let expected = std::fs::canonicalize(&real)
            .unwrap()
            .join("not-created-yet");
        assert_eq!(resolved, expected);
    }

    #[test]
    fn resolve_existing_prefix_canonicalizes_an_existing_dir() {
        let td = tempfile::tempdir().unwrap();
        let dir = td.path().join("cfg");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            resolve_existing_prefix(&dir),
            std::fs::canonicalize(&dir).unwrap()
        );
    }

    #[test]
    fn profile_does_not_duplicate_default_config_dir() {
        // CLAUDE_CONFIG_DIR explicitly set to the default ~/.claude must not add
        // a second, redundant allow entry.
        let (_td, root, rpc, home) = sandbox_dirs();
        let default_claude = std::fs::canonicalize(&home).unwrap().join(".claude");

        let profile = build_profile(&root, &rpc, &home, Some(default_claude.as_path())).unwrap();
        let needle = format!("(subpath \"{}\")", default_claude.display());
        assert_eq!(
            profile.matches(&needle).count(),
            1,
            "default ~/.claude should appear exactly once"
        );
    }

    #[test]
    fn escapes_quotes_in_paths() {
        assert_eq!(sbpl_string(r#"/path/with"quote"#), r#""/path/with\"quote""#);
    }

    #[test]
    fn run_profile_confines_writes_to_worktree_and_toolchains() {
        let td = tempfile::tempdir().unwrap();
        let checkout = td.path().join("repo-worktree");
        let home = td.path().join("home");
        std::fs::create_dir_all(&checkout).unwrap();
        std::fs::create_dir_all(&home).unwrap();

        let profile = build_run_profile(&checkout, &home, &[]).unwrap();
        let canonical_worktree = std::fs::canonicalize(&checkout).unwrap();
        let canonical_home = std::fs::canonicalize(&home).unwrap();

        // Same deny-by-default posture as the agent profile.
        assert!(profile.contains("(allow default)"));
        assert!(profile.contains("(deny file-write*)"));
        // The run command writes freely inside its checkout.
        assert!(profile.contains(&format!("\"{}\"", canonical_worktree.display())));
        // Toolchain dirs the default detected commands need (cargo/go/pnpm/bundler).
        for dir in [".cargo", "go", "Library/pnpm", ".bundle", ".rustup", ".bun"] {
            let expected = format!("(subpath \"{}/{dir}\")", canonical_home.display());
            assert!(
                profile.contains(&expected),
                "run profile should grant {dir}: missing {expected}"
            );
        }
    }

    #[test]
    fn run_profile_omits_agent_only_state_dirs() {
        // A Run process neither speaks RPC nor persists agent transcripts, so
        // the agent-CLI state dirs must not be on its write allow-list.
        let td = tempfile::tempdir().unwrap();
        let checkout = td.path().join("repo-worktree");
        let home = td.path().join("home");
        std::fs::create_dir_all(&checkout).unwrap();
        std::fs::create_dir_all(&home).unwrap();

        let profile = build_run_profile(&checkout, &home, &[]).unwrap();
        let canonical_home = std::fs::canonicalize(&home).unwrap();
        for dir in [".claude", ".codex", ".cursor", ".gemini", ".pi"] {
            let unexpected = format!("(subpath \"{}/{dir}\")", canonical_home.display());
            assert!(
                !profile.contains(&unexpected),
                "run profile should not grant agent state dir {dir}"
            );
        }
    }

    #[test]
    fn nested_rpc_root_is_temp_scoped_and_keyed_by_worktree() {
        let a = nested_rpc_root(Path::new("/Users/x/.fletch/worktrees/taklamakan/repo"));
        let b = nested_rpc_root(Path::new("/Users/x/.fletch/worktrees/rhone/repo"));

        // Under the system temp root, which the Run profile grants — so a nested
        // Fletch can actually create its mailboxes there.
        let tmp = std::env::temp_dir().join("fletch-rpc");
        assert!(a.starts_with(&tmp) && b.starts_with(&tmp));
        // Distinct worktrees never share a root (no agent-id collisions), and
        // the key carries no path separators.
        assert_ne!(a, b);
        let key = a.file_name().unwrap().to_string_lossy();
        assert!(!key.contains('/') && !key.contains('.'));

        // Paths differing only in non-alphanumeric chars must not collide — a
        // char-sanitized key would map both to the same root.
        let c = nested_rpc_root(Path::new("/Users/alice/projects/my-app"));
        let d = nested_rpc_root(Path::new("/Users/alice/projects/my.app"));
        assert_ne!(c, d);
    }

    #[test]
    fn cleanup_removes_only_dead_instance_roots() {
        let td = tempfile::tempdir().unwrap();
        let base = td.path();
        let live = std::process::id().to_string();
        let dead = i32::MAX.to_string(); // out of pid range → never alive
        for pid in [&live, &dead] {
            std::fs::create_dir_all(base.join(pid).join("agent")).unwrap();
        }
        // A non-numeric entry isn't ours to reason about — leave it alone.
        std::fs::create_dir_all(base.join("scratch")).unwrap();

        cleanup_nested_state_roots_in(base);

        assert!(base.join(&live).exists(), "live instance root kept");
        assert!(!base.join(&dead).exists(), "dead instance root removed");
        assert!(base.join("scratch").exists(), "non-pid entry left alone");
    }

    #[test]
    fn nested_checkouts_root_is_temp_scoped_and_distinct_from_rpc() {
        let wt = Path::new("/Users/x/.fletch/worktrees/rhone/repo");
        let root = nested_checkouts_root(wt);
        // Under the system temp root the Run profile grants, so a nested Fletch
        // can create its checkouts there.
        assert!(root.starts_with(std::env::temp_dir().join("fletch-worktrees")));
        // Same checkout key, different kind → different root (rpc vs worktrees
        // never share a dir).
        assert_ne!(root, nested_rpc_root(wt));
    }

    #[test]
    fn run_profile_grants_extra_writable_common_dir() {
        let td = tempfile::tempdir().unwrap();
        let checkout = td.path().join("repo-worktree");
        let home = td.path().join("home");
        let common = td.path().join("source-repo/.git");
        for p in [&checkout, &home, &common] {
            std::fs::create_dir_all(p).unwrap();
        }
        let canonical_common = std::fs::canonicalize(&common).unwrap();

        let profile = build_run_profile(&checkout, &home, &[canonical_common.clone()]).unwrap();
        assert!(
            profile.contains(&format!("(subpath \"{}\")", canonical_common.display())),
            "run profile should grant the target's git common dir"
        );
    }
}
