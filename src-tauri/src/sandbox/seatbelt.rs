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
//! on the allow-list below — otherwise it can't persist its own state. What
//! goes on that list is *policy*, not a hand-maintained list local to this
//! file: the agent profile's write allowance is exactly the engine-independent
//! [`super::policy`] grants — every provider's host-persistence dirs
//! ([`policy::all_provider_state_dirs`]) plus the host-scratch cache/data dirs a
//! host-FS-sharing engine must additionally allow ([`policy::agent_scratch_dirs`],
//! which cover XDG `~/.cache`/`~/.local/share`/`~/.local/state` and their
//! macOS-native `~/Library/Caches`/`~/Library/Application Support` forms). Those
//! grants deliberately never include a PATH-resolved bin dir (`~/.local/bin`) or
//! a config *root* (`~/.config`) — see the policy module's invariants — which is
//! why this profile grants `~/.local/share`+`~/.local/state` rather than all of
//! `~/.local`, and only `~/.config/opencode` rather than all of `~/.config`. The
//! agent CLIs' own sandboxes are disabled (e.g. codex runs `danger-full-access`)
//! so the two don't fight, leaving `sandbox-exec` as the sole boundary.
//!
//! One region is carved back *out* of the broad `Application Support` grant:
//! the app's own data dir (`~/Library/Application Support/<BUNDLE_ID>`, holding
//! `fletch.db` — transcripts, settings). Both reads (exfiltration) and writes
//! (forging state) are denied there, so a prompt-injected agent can't touch
//! app state even though its parent is writable. The Run profile keeps the same
//! deny but re-allows the `dev` subdir, so a nested *dev* Fletch launched from
//! the Run panel can still open its own database.

use std::path::{Path, PathBuf};

use super::engine::{AgentLaunchCtx, EngineKind, Keepalive, KillHandle, LaunchPlan, SandboxEngine};
use super::policy;
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

/// SBPL rule carving the app's own data dir back *out* of the broad
/// `Application Support` write grant — and out of `allow default` for reads.
/// `fletch.db` (agent transcripts, settings) lives here, so no confined process
/// should read it (exfiltration) or write it (forging state). Emitted as a
/// single multi-operation deny (verified to parse) and MUST follow the
/// `(allow file-write* …)` block: SBPL is last-match-wins, so a later deny
/// overrides the earlier read/write grants. `BUNDLE_ID` must match the folder
/// macOS derives from `tauri.conf.json`'s `identifier`.
fn deny_app_data_dir(home_s: &str) -> String {
    let app_data = sbpl_string(&format!(
        "{home_s}/Library/Application Support/{}",
        crate::BUNDLE_ID
    ));
    format!(
        ";; The app's own data dir (fletch.db: transcripts, settings) must be opaque\n\
         ;; to confined processes: deny reads (exfiltration) and writes (forging\n\
         ;; state), even though the broad Application Support grant above covers its\n\
         ;; parent. Last-match-wins, so this must come after the allow block.\n\
         (deny file-read* file-write* (subpath {app_data}))"
    )
}

/// Toolchain + broad-state dirs the Run panel additionally grants so real
/// project builds succeed. These hold package caches, downloaded toolchains,
/// and — for some — PATH-resolved binaries (`~/.cargo/bin`, `~/go/bin`,
/// `~/.rbenv/shims`, and everything under `~/.local/bin`). That last part is a
/// residual hijack surface, which is why this superset is **Run-only** and
/// deliberately kept off the agent profile: a running project legitimately
/// needs its toolchain to write here, whereas an agent editing source does not.
///
/// The two broadest entries — the whole of `~/.config` and `~/.local` — are the
/// ones the agent profile pointedly narrows (to `~/.config/opencode` and
/// `~/.local/share`+`~/.local/state`; see [`super::policy`]). Run re-adds them
/// whole because build steps write arbitrary config/state (`~/.config/<tool>`,
/// `~/.local/bin` installs). Note the residual surface is reachable from an
/// agent *indirectly*: an agent can edit e.g. a `package.json` script or a
/// `Makefile` target that a later Run command executes, so Run's looseness can
/// be triggered by agent-authored content. That's an accepted, documented
/// trade-off — the Run panel runs project code the user chose to run, under a
/// weaker boundary by design — not a hole in the agent profile.
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
    ".config",        // Run-only: whole config root (agent gets only subdirs)
    ".local",         // Run-only: whole ~/.local incl. ~/.local/bin (agent gets share/state)
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
    // Host-scratch dirs (package/XDG/macOS caches) — the same class the agent
    // profile grants, sourced from the shared policy so the two can't drift.
    subpaths.extend(
        policy::agent_scratch_dirs(&home)
            .iter()
            .map(|p| sbpl_string(&p.to_string_lossy())),
    );
    // Run-only toolchain + broad-state dirs, including the whole `~/.config`
    // and `~/.local` the agent profile pointedly withholds (see the const's
    // doc-comment). `~/.local` here supersets the scratch `~/.local/share`/
    // `~/.local/state` above — a harmless redundancy that keeps Run's write set
    // byte-for-byte what it was before the agent-profile narrowing.
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

    let deny_app_data = deny_app_data_dir(&home_s);
    let app_data_dev = sbpl_string(&format!(
        "{home_s}/Library/Application Support/{}/dev",
        crate::BUNDLE_ID
    ));

    Ok(format!(
        r#"(version 1)
(allow default)

;; Block writes everywhere by default, then re-allow specific subpaths.
(deny file-write*)
(allow file-write*
  {writable_block})

{deny_app_data}
;; Exception: a nested *dev* Fletch launched from the Run panel stores its data
;; under `<data dir>/dev` (see lib.rs setup) and must open its own database, so
;; re-allow just that subtree (last-match-wins). A Run-panel process can thus
;; touch the dev instance's state — acceptable because it's dev-only and the Run
;; panel already runs arbitrary project code the developer chose to run.
(allow file-read* file-write* (subpath {app_data_dev}))

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

    let claude_json = sbpl_string(&format!("{home_s}/.claude.json"));
    // A non-default `CLAUDE_CONFIG_DIR` is where claude actually writes its
    // config/transcripts/auth, so grant it too. Resolve symlinks first so the
    // SBPL path matches what the sandbox sees at write time (every other entry
    // is canonical); then skip it only when it equals the default `{home}/.claude`
    // (which the policy state-dir list grants below), to avoid a redundant entry.
    // `home` is already canonical, but the policy's `.claude` leaf is NOT
    // symlink-resolved — the state-dir grant is that literal path — so compare
    // against it un-resolved. If `~/.claude` is itself a symlink and the config
    // dir points at its resolved target, resolving the leaf here too would treat
    // it as default and drop the grant, yet the literal state-dir rule wouldn't
    // cover the target, denying claude's writes. (Docker can resolve both sides
    // because its `~/.claude` bind mount follows the symlink source; the SBPL
    // allow-list can't.) The `~/.claude.json` top-level state *file* stays a
    // seatbelt-local literal grant: it's a file, not a dir, so the dir-oriented
    // policy API doesn't model it (see the policy module doc).
    let claude_config_extra = claude_config_dir
        // A bin-resident relocation (`CLAUDE_CONFIG_DIR=$HOME/.local/bin/…`)
        // would put an agent-writable subtree on the user's PATH — the same
        // rejection every env-relocated policy dir gets (invariant 1;
        // fail-closed: claude's config writes are denied, never a hijack).
        .filter(|p| !policy::bin_resident(p))
        .map(policy::resolve_existing_prefix)
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|p| *p != format!("{home_s}/.claude"))
        .map(|p| format!("\n  (subpath {})", sbpl_string(&p)))
        .unwrap_or_default();
    // The write allow-list is the engine-independent policy, not a list local to
    // this file: every provider's host-persistence dirs (`~/.claude`, `~/.codex`,
    // `~/.cursor`, `~/.gemini`, `~/.pi`, opencode's XDG data+config subdirs) plus
    // the host-scratch cache/data dirs (`~/.npm`, `~/.cache`, `~/.local/share`,
    // `~/.local/state`, `~/Library/Caches`, `~/Library/Application Support`).
    // Crucially this is *not* the old blanket `~/.local`/`~/.config` grant: the
    // policy withholds every PATH-resolved bin dir (`~/.local/bin`) and config
    // root (`~/.config`), granting only `~/.local/share`+`~/.local/state` and the
    // specific `~/.config/opencode` — see the policy module's invariants.
    let policy_dirs = subpath_grants(
        policy::all_provider_state_dirs(&home)
            .into_iter()
            .chain(policy::agent_scratch_dirs(&home)),
    )
    .join("\n");

    // No `dev` exception here (unlike the Run profile): agents never legitimately
    // touch any Fletch data dir, dev or otherwise.
    let deny_app_data = deny_app_data_dir(&home_s);

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
  (literal {claude_json}){claude_config_extra}
{policy_dirs})

{deny_app_data}

{DEVICE_WRITE_RULES}
"#
    ))
}

/// SBPL `(subpath …)` grant lines for the policy dirs, each emitted in its
/// literal form and — when different — its symlink-resolved form (deduped).
/// The sandbox checks *resolved* write paths, so an env-relocated provider dir
/// that passes through a symlink (`CODEX_HOME=/tmp/codex` → writes observed at
/// `/private/tmp/codex`) is denied by the raw grant alone. The literal form is
/// kept alongside: for the default home-relative dirs both forms are equal
/// (home is canonical), and when a leaf like `~/.claude` is itself a symlink
/// the literal path is what the `claude_config_extra` dedup compares against.
///
/// Every candidate — literal and resolved — passes [`policy::bin_resident`]
/// before it's emitted: a default dir whose leaf symlinks into a PATH-style
/// bin dir (`~/.claude` → `~/.local/bin/claude`) must not have its resolved
/// form granted, or writes through the symlink would land agent-controlled
/// files on the user's PATH (invariant 1). Env-relocated dirs are already
/// rejected at resolution time, but the default home-relative dirs never pass
/// through that check, so it's re-applied here at the emission seam. Skipping
/// is fail-closed: with the resolved form denied, the provider's writes
/// through the symlink are refused rather than hijackable.
fn subpath_grants(dirs: impl IntoIterator<Item = PathBuf>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for dir in dirs {
        let resolved = policy::resolve_existing_prefix(&dir);
        for p in [dir, resolved] {
            if policy::bin_resident(&p) {
                continue;
            }
            let line = format!("  (subpath {})", sbpl_string(&p.to_string_lossy()));
            if !out.contains(&line) {
                out.push(line);
            }
        }
    }
    out
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

    #[test]
    fn agent_profile_narrows_local_and_config_away_from_bin_and_root() {
        // The security fix: the agent profile must NOT grant blanket `~/.local`
        // (it contains `~/.local/bin`, a PATH dir → host-command hijack) or
        // blanket `~/.config` (config poisoning: git core.hooksPath, fish, gh).
        // It grants the narrow replacements instead, and every provider dot-dir
        // and cache dir stays exactly as before.
        let (_td, root, rpc, home) = sandbox_dirs();
        let profile = build_profile(&root, &rpc, &home, None).unwrap();
        let canonical_home = std::fs::canonicalize(&home).unwrap();
        let h = canonical_home.display();

        // Blanket roots ABSENT (the whole point of the fix).
        assert!(
            !profile.contains(&format!("(subpath \"{h}/.local\")")),
            "blanket ~/.local must not be on the agent allow-list"
        );
        assert!(
            !profile.contains(&format!("(subpath \"{h}/.config\")")),
            "blanket ~/.config must not be on the agent allow-list"
        );
        // No `~/.local/bin` grant may appear in any form.
        assert!(
            !profile.contains(&format!("{h}/.local/bin")),
            "no ~/.local/bin grant may appear"
        );

        // Narrow replacements PRESENT. The scratch dirs are fixed
        // home-relative paths; opencode's config dir is env-dependent
        // (`$XDG_CONFIG_HOME` — CI runners export their own), so assert it via
        // the same policy resolution the profile builder uses.
        for narrow in [".local/share", ".local/state"] {
            assert!(
                profile.contains(&format!("(subpath \"{h}/{narrow}\")")),
                "agent profile should grant the narrow {narrow}"
            );
        }
        let opencode_config = policy::opencode_config_dir(&canonical_home);
        assert!(
            profile.contains(&format!("(subpath \"{}\")", opencode_config.display())),
            "agent profile should grant the narrow opencode config dir"
        );
        // Codex's dir is env-relocatable too (`$CODEX_HOME`) — same treatment.
        let codex_home = policy::codex_home_dir(&canonical_home);
        assert!(
            profile.contains(&format!("(subpath \"{}\")", codex_home.display())),
            "agent profile should grant the codex home dir"
        );
        // Everything else unchanged: provider dot-dirs, caches, macOS-native.
        for dir in [
            ".claude",
            ".cursor",
            ".gemini",
            ".pi",
            ".npm",
            ".cache",
            "Library/Caches",
            "Library/Application Support",
        ] {
            assert!(
                profile.contains(&format!("(subpath \"{h}/{dir}\")")),
                "agent profile should still grant {dir}"
            );
        }
        // The `~/.claude.json` top-level state file stays a literal grant.
        assert!(
            profile.contains(&format!("(literal \"{h}/.claude.json\")")),
            "agent profile should keep the ~/.claude.json literal grant"
        );
    }

    #[test]
    fn subpath_grants_emit_resolved_form_for_symlinked_dirs() {
        // The sandbox checks resolved write paths: an env-relocated dir behind
        // a symlink (CODEX_HOME=/tmp/codex → /private/tmp/codex) must be
        // granted in resolved form too, or its writes are denied.
        let td = tempfile::tempdir().unwrap();
        let real = td.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = td.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let grants = subpath_grants([link.clone()]);
        let canonical_real = std::fs::canonicalize(&real).unwrap();
        assert!(
            grants.contains(&format!("  (subpath \"{}\")", link.display())),
            "literal form kept"
        );
        assert!(
            grants.contains(&format!("  (subpath \"{}\")", canonical_real.display())),
            "resolved form added"
        );

        // A dir that resolves to itself yields exactly one grant — no
        // duplicate lines for the common (canonical) case.
        assert_eq!(subpath_grants([canonical_real]).len(), 1);
    }

    #[test]
    fn subpath_grants_never_emit_bin_resident_paths() {
        // A default provider dir whose leaf symlinks into a PATH-style bin dir
        // (~/.claude → ~/.local/bin/claude) must not have its resolved form
        // emitted — that would grant an agent-writable subtree on the user's
        // PATH through the symlink (invariant 1). Fail closed instead.
        let td = tempfile::tempdir().unwrap();
        let target = td.path().join(".local/bin/claude");
        std::fs::create_dir_all(&target).unwrap();
        let link = td.path().join(".claude");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let grants = subpath_grants([link]);
        assert!(
            grants.is_empty(),
            "no grant may be emitted for a bin-resident dir, got: {grants:?}"
        );
    }

    #[test]
    fn profile_omits_provider_dirs_symlinked_into_bin() {
        // End-to-end through build_profile: with ~/.claude symlinked into
        // ~/.local/bin, neither the resolved bin subtree nor any other
        // .local/bin path may appear on the allow-list, while the remaining
        // provider dirs stay granted.
        let (_td, root, rpc, home) = sandbox_dirs();
        let target = home.join(".local/bin/claude");
        std::fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, home.join(".claude")).unwrap();

        let profile = build_profile(&root, &rpc, &home, None).unwrap();
        let canonical_home = std::fs::canonicalize(&home).unwrap();
        assert!(
            !profile.contains(".local/bin"),
            "a symlinked ~/.claude must not smuggle a bin subtree onto the allow-list"
        );
        assert!(
            profile.contains(&format!(
                "(subpath \"{}/.cursor\")",
                canonical_home.display()
            )),
            "other provider dirs stay granted"
        );
    }

    #[test]
    fn profile_rejects_bin_resident_claude_config_dir() {
        // CLAUDE_CONFIG_DIR pointed into a PATH-style bin dir must not become
        // a write grant (invariant 1) — claude fails closed instead.
        let (_td, root, rpc, home) = sandbox_dirs();
        let cfg = home.join(".local/bin/claude-cfg");
        std::fs::create_dir_all(&cfg).unwrap();

        let profile = build_profile(&root, &rpc, &home, Some(cfg.as_path())).unwrap();
        assert!(
            !profile.contains(".local/bin"),
            "bin-resident CLAUDE_CONFIG_DIR must not appear on the allow-list"
        );
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
    fn agent_profile_denies_app_data_dir_after_allow_block() {
        // The app's own data dir (fletch.db) must be opaque to agents: deny both
        // reads (exfiltration) and writes (forging state). The deny only bites if
        // it comes AFTER the write allow-list, since SBPL is last-match-wins.
        let (_td, root, rpc, home) = sandbox_dirs();
        let profile = build_profile(&root, &rpc, &home, None).unwrap();
        let canonical_home = std::fs::canonicalize(&home).unwrap();

        let deny = format!(
            "(deny file-read* file-write* (subpath \"{}/Library/Application Support/{}\"))",
            canonical_home.display(),
            crate::BUNDLE_ID
        );
        assert!(
            profile.contains(&deny),
            "agent profile must deny read+write on its own data dir: missing {deny}"
        );
        let allow_at = profile.find("(allow file-write*").unwrap();
        let deny_at = profile.find(&deny).unwrap();
        assert!(
            deny_at > allow_at,
            "the app-data deny must come after the allow block to override it"
        );
    }

    #[test]
    fn agent_profile_does_not_reallow_dev_data_dir() {
        // Agents never legitimately touch any Fletch data dir — no `dev`
        // exception (that carve-out is Run-profile-only).
        let (_td, root, rpc, home) = sandbox_dirs();
        let profile = build_profile(&root, &rpc, &home, None).unwrap();
        let canonical_home = std::fs::canonicalize(&home).unwrap();
        let dev = format!(
            "{}/Library/Application Support/{}/dev",
            canonical_home.display(),
            crate::BUNDLE_ID
        );
        assert!(
            !profile.contains(&dev),
            "agent profile must not re-allow the dev data subdir"
        );
    }

    #[test]
    fn run_profile_denies_app_data_but_reallows_dev_subdir() {
        // The Run profile carries the same app-data deny, but re-allows the `dev`
        // subtree AFTER it (last-match-wins) so a nested dev Fletch can open its
        // own database.
        let td = tempfile::tempdir().unwrap();
        let checkout = td.path().join("repo-checkout");
        let home = td.path().join("home");
        std::fs::create_dir_all(&checkout).unwrap();
        std::fs::create_dir_all(&home).unwrap();

        let profile = build_run_profile(&checkout, &home, &[]).unwrap();
        let canonical_home = std::fs::canonicalize(&home).unwrap();

        let deny = format!(
            "(deny file-read* file-write* (subpath \"{}/Library/Application Support/{}\"))",
            canonical_home.display(),
            crate::BUNDLE_ID
        );
        let dev_allow = format!(
            "(allow file-read* file-write* (subpath \"{}/Library/Application Support/{}/dev\"))",
            canonical_home.display(),
            crate::BUNDLE_ID
        );
        assert!(
            profile.contains(&deny),
            "run profile must deny the app data dir"
        );
        assert!(
            profile.contains(&dev_allow),
            "run profile must re-allow the dev subdir: missing {dev_allow}"
        );
        let deny_at = profile.find(&deny).unwrap();
        let dev_at = profile.find(&dev_allow).unwrap();
        assert!(
            dev_at > deny_at,
            "the dev re-allow must come after the deny to take effect"
        );
    }

    #[test]
    fn run_profile_confines_writes_to_checkout_and_toolchains() {
        let td = tempfile::tempdir().unwrap();
        let checkout = td.path().join("repo-checkout");
        let home = td.path().join("home");
        std::fs::create_dir_all(&checkout).unwrap();
        std::fs::create_dir_all(&home).unwrap();

        let profile = build_run_profile(&checkout, &home, &[]).unwrap();
        let canonical_checkout = std::fs::canonicalize(&checkout).unwrap();
        let canonical_home = std::fs::canonicalize(&home).unwrap();

        // Same deny-by-default posture as the agent profile.
        assert!(profile.contains("(allow default)"));
        assert!(profile.contains("(deny file-write*)"));
        // The run command writes freely inside its checkout.
        assert!(profile.contains(&format!("\"{}\"", canonical_checkout.display())));
        // Toolchain dirs the default detected commands need (cargo/go/pnpm/bundler),
        // plus the whole `~/.config` and `~/.local` the agent profile withholds —
        // Run keeps the looser grant so arbitrary build steps succeed.
        for dir in [
            ".cargo",
            "go",
            "Library/pnpm",
            ".bundle",
            ".rustup",
            ".bun",
            ".config",
            ".local",
        ] {
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
        let checkout = td.path().join("repo-checkout");
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
        let checkout = td.path().join("repo-checkout");
        let home = td.path().join("home");
        let common = td.path().join("source-repo/.git");
        for p in [&checkout, &home, &common] {
            std::fs::create_dir_all(p).unwrap();
        }
        let canonical_common = std::fs::canonicalize(&common).unwrap();

        let profile =
            build_run_profile(&checkout, &home, std::slice::from_ref(&canonical_common)).unwrap();
        assert!(
            profile.contains(&format!("(subpath \"{}\")", canonical_common.display())),
            "run profile should grant the target's git common dir"
        );
    }
}
