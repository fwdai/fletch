//! Engine-independent **write policy** for sandboxed agents: which host
//! directories a coding agent is allowed to write. It's the single source of
//! truth both sandbox engines consume — macOS seatbelt (SBPL `(subpath …)`
//! rules) and Docker (`-v host:host` bind mounts) — instead of each engine
//! hand-maintaining its own allow-list and drifting apart.
//!
//! The split is **policy vs mechanism**: this module answers *what* an agent
//! may write; each engine decides *how* to enforce it. Grants fall into two
//! classes:
//!
//! - **Host-persistence grants** (class 1 — [`provider_state_dirs`] /
//!   [`all_provider_state_dirs`]): host dirs a provider must persist real state
//!   to — its session store, auth, config. BOTH engines honor these (Docker as
//!   read-write mounts, seatbelt as writable subpaths), because that state has
//!   to land on the *host* to survive the process/container and be read back by
//!   the host-side transcript/auth readers.
//! - **Host-scratch grants** (class 2 — [`agent_scratch_dirs`]): caches and XDG
//!   data/state dirs a toolchain needs writable *somewhere*, but not
//!   necessarily on a host-visible path. Only engines that share the host
//!   filesystem (seatbelt) consume these; Docker satisfies them with the
//!   container's own writable filesystem and must NOT mount them back to the
//!   host.
//!
//! Two invariants hold across every path any function here returns; the guard
//! test `policy_grants_never_expose_bin_or_config_root` encodes them so a future
//! provider or scratch dir can't silently regress the class:
//!
//! 1. **No PATH-resolved bin dir is ever agent-writable** (the `~/.local/bin`
//!    class). Nothing returns `~/.local/bin`, anything under it, or any
//!    component-final `bin` dir under home. Were one writable, a prompt-injected
//!    agent could drop `~/.local/bin/git`, and the user's next *host* shell
//!    command would run attacker code entirely outside the sandbox.
//! 2. **No host config *root* is ever agent-writable** (the `~/.config` class).
//!    Nothing returns bare `~/.config`. A provider gets specific config
//!    *subdirs* (e.g. `~/.config/opencode`), never the root — so an agent can't
//!    poison `~/.config/git` (`core.hooksPath`), `~/.config/fish`,
//!    `~/.config/gh` (aliases carry shell commands), etc.
//!
//! One piece of claude's persistence surface is deliberately *not* modeled
//! here: the top-level `~/.claude.json` state **file**. It's a single file, not
//! a dir, and threading a file-literal grant through this dir-oriented API buys
//! nothing, so the seatbelt caller keeps it as a local literal grant. Noted
//! here so the whole persistence surface is documented in one place.

use std::path::{Path, PathBuf};

/// Every provider whose host-persistence dirs seatbelt's provider-agnostic
/// profile grants — the enumeration [`all_provider_state_dirs`] folds over.
/// A superset of Docker's supported providers (Docker has no `gemini` image
/// yet, but the seatbelt agent still runs the gemini CLI and must let it
/// persist `~/.gemini`), so this list, not [`super::docker::DockerProvider`],
/// is the authority for the seatbelt side.
const STATE_DIR_PROVIDERS: &[&str] = &["claude", "codex", "cursor", "gemini", "pi", "opencode"];

/// Class-1 host-persistence dirs a single `provider` must write on the host:
/// its session store, auth, and config. Docker mounts exactly these read-write
/// at their host paths; the seatbelt profile folds them in provider-agnostically
/// via [`all_provider_state_dirs`]. An unrecognized provider yields an empty
/// list — Docker already gates on a known [`super::docker::DockerProvider`], and
/// the seatbelt side only ever passes ids from [`STATE_DIR_PROVIDERS`].
///
/// Note the omissions this policy makes on purpose: claude's grant is its
/// config *dir* only — the top-level `~/.claude.json` file stays a
/// seatbelt-local literal (see the module doc), and a non-default
/// `CLAUDE_CONFIG_DIR` is likewise handled by the seatbelt caller, which needs
/// its own symlink-resolution to keep the emitted SBPL path matching the
/// sandbox's resolved write path. Codex's and opencode's env relocations
/// (`$CODEX_HOME`, `$XDG_*`), by contrast, ARE resolved here — both engines
/// need the same dir, with no engine-specific handling on top.
pub fn provider_state_dirs(provider: &str, home: &Path) -> Vec<PathBuf> {
    match provider {
        "claude" => vec![home.join(".claude")],
        // Codex's state dir moves with `$CODEX_HOME`. The old blanket
        // `~/.config` agent grant incidentally covered e.g.
        // `CODEX_HOME=$HOME/.config/codex`; with the narrowing, this
        // env-resolved grant is what keeps that supported relocation writable.
        "codex" => vec![codex_home_dir(home)],
        "cursor" => vec![home.join(".cursor")],
        "gemini" => vec![home.join(".gemini")],
        "pi" => vec![home.join(".pi")],
        // OpenCode keeps its session store / auth under an XDG *data* dir and
        // its custom-provider/plugin config under an XDG *config* dir. The
        // config dir is a specific `~/.config` subdir (invariant 2), never the
        // root.
        "opencode" => vec![opencode_data_dir(home), opencode_config_dir(home)],
        _ => Vec::new(),
    }
}

/// Class-1 union across every provider — for an engine whose confinement is
/// built once, provider-agnostically (the seatbelt profile covers *all* agents
/// with one profile, so it must grant every provider's state dirs). Deduped
/// while preserving first-seen order so the emitted allow-list is stable.
pub fn all_provider_state_dirs(home: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for provider in STATE_DIR_PROVIDERS {
        for dir in provider_state_dirs(provider, home) {
            if !out.contains(&dir) {
                out.push(dir);
            }
        }
    }
    out
}

/// Class-2 host-scratch dirs a host-FS-sharing engine (seatbelt) must
/// additionally allow: package-manager caches and the XDG data/state +
/// macOS-native cache/app-support dirs the agents' subprocess toolchains and
/// macOS frameworks write to. A denied write here ranges from a harmless cache
/// miss to a fatal auth-token or state write, so they're granted on the
/// "per-user app state, not source/system" basis.
///
/// Crucially this is the narrow replacement for the old blanket `~/.local` and
/// `~/.config` grants: `~/.local/share` + `~/.local/state` instead of all of
/// `~/.local` (which contains `~/.local/bin`, a PATH dir — invariant 1), and
/// *nothing* under `~/.config` (config poisoning — invariant 2; providers get
/// their specific config subdirs via [`provider_state_dirs`] instead).
///
/// Docker does NOT consume these: a container has its own writable filesystem
/// for scratch, and mounting these back would needlessly expose host state.
pub fn agent_scratch_dirs(home: &Path) -> Vec<PathBuf> {
    [
        ".npm",                      // npm's package cache
        ".cache",                    // XDG cache root
        ".local/share",             // XDG data root — NOT ~/.local (holds ~/.local/bin)
        ".local/state",             // XDG state root — NOT ~/.local (holds ~/.local/bin)
        "Library/Caches",            // macOS-native cache root
        "Library/Application Support", // macOS-native per-app state root
    ]
    .iter()
    .map(|d| home.join(d))
    .collect()
}

/// OpenCode's data dir: `$XDG_DATA_HOME/opencode` if set, else
/// `~/.local/share/opencode`. This is both the dir Docker bind-mounts read-write
/// and where `agent::opencode_locate` reads its session storage on the host, so
/// the two must agree — this is the shared resolution.
pub fn opencode_data_dir(home: &Path) -> PathBuf {
    xdg_base(home, "XDG_DATA_HOME", ".local/share").join("opencode")
}

/// OpenCode's config dir: `$XDG_CONFIG_HOME/opencode` if set, else
/// `~/.config/opencode` (custom providers + plugin installs). A specific
/// `~/.config` subdir — never the root (invariant 2). Docker mounts it only
/// when it already exists (see the docker launch path).
pub fn opencode_config_dir(home: &Path) -> PathBuf {
    xdg_base(home, "XDG_CONFIG_HOME", ".config").join("opencode")
}

/// Codex's config dir: `$CODEX_HOME` if set non-blank, else `~/.codex`. This
/// is both the dir Docker bind-mounts read-write and where
/// `transcripts::find_codex_rollouts` reads transcripts on the host, so the
/// two must agree — this is the shared resolution (it used to live in the
/// docker engine, leaving seatbelt blind to the relocation).
pub fn codex_home_dir(home: &Path) -> PathBuf {
    codex_home_from(std::env::var_os("CODEX_HOME"), home)
}

/// Pure core of [`codex_home_dir`] — the same env-seam split as
/// [`xdg_base_from`], for the same hermetic-test reason. Blank counts as
/// unset, matching the XDG handling (the docker engine's old resolution took
/// a blank value verbatim, i.e. produced an empty mount source that failed
/// the launch — falling back to the default is the strict improvement).
fn codex_home_from(value: Option<std::ffi::OsString>, home: &Path) -> PathBuf {
    value
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"))
}

/// The XDG base dir named by `var` (`$var` if set non-blank, else
/// `home/<default_rel>`). Shared by [`opencode_data_dir`]/[`opencode_config_dir`].
/// This thin wrapper is the env *seam*; the resolution itself is the pure
/// [`xdg_base_from`], so it can be tested hermetically — CI runners export
/// their own `XDG_*` vars, and mutating the process env in parallel tests
/// races other tests that read it.
pub fn xdg_base(home: &Path, var: &str, default_rel: &str) -> PathBuf {
    xdg_base_from(std::env::var_os(var), home, default_rel)
}

/// Pure core of [`xdg_base`]: resolve from an explicit (possibly absent) env
/// value. A blank value counts as unset, matching the XDG spec's treatment of
/// empty base-dir vars.
fn xdg_base_from(value: Option<std::ffi::OsString>, home: &Path, default_rel: &str) -> PathBuf {
    value
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(default_rel))
}

/// Resolve symlinks in the longest existing prefix of `p`, then re-append the
/// not-yet-existing tail. `fs::canonicalize` alone can't be used because it
/// requires the whole path to exist, but a config dir (e.g. `CLAUDE_CONFIG_DIR`)
/// may point at a dir the CLI hasn't created yet. Resolving the existing prefix
/// still collapses the well-known macOS symlinks (`/tmp` → `/private/tmp`,
/// `/var` → `/private/var`), so the emitted SBPL path matches the sandbox's
/// resolved write path. Falls back to `p` unchanged if nothing resolves (e.g. a
/// bogus path).
///
/// Lives here — not in either engine — because both need it: seatbelt to emit a
/// canonical SBPL allow entry for a non-default config dir, docker to compare a
/// config/XDG dir against its default. It used to live in `seatbelt` and be
/// imported *up* into `docker`, a cross-engine dependency pointing the wrong
/// way; the shared policy module is its correct home.
pub fn resolve_existing_prefix(p: &Path) -> PathBuf {
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
    use std::ffi::OsStr;

    /// The invariant guard — the whole reason this module centralizes the
    /// allow-list. It encodes the *class* (not just today's instances): every
    /// dir any grant function returns must be neither the `~/.local/bin`
    /// PATH-hijack surface (invariant 1) nor a host config/local *root*
    /// (invariant 2). A future provider or scratch dir that reintroduces either
    /// fails here before it can reach a sandbox profile.
    #[test]
    fn policy_grants_never_expose_bin_or_config_root() {
        let home = Path::new("/Users/u");
        let mut dirs = all_provider_state_dirs(home);
        dirs.extend(agent_scratch_dirs(home));

        let local_bin = home.join(".local/bin");
        let config_root = home.join(".config");
        let local_root = home.join(".local");

        for dir in &dirs {
            // (i) never ~/.local/bin, nor anything nested under it.
            assert!(
                !dir.starts_with(&local_bin),
                "{} exposes the PATH bin dir ~/.local/bin (invariant 1)",
                dir.display()
            );
            // (ii) never the bare config/local roots.
            assert_ne!(
                *dir, config_root,
                "the ~/.config root must never be granted (invariant 2)"
            );
            assert_ne!(
                *dir, local_root,
                "the ~/.local root must never be granted (invariant 1)"
            );
            // (iii) no component-final `bin` dir under home — the general
            // PATH-hijack class, not just the ~/.local/bin instance.
            assert_ne!(
                dir.file_name(),
                Some(OsStr::new("bin")),
                "{} is a component-final bin dir — a PATH-hijack surface (invariant 1)",
                dir.display()
            );
        }
    }

    /// Class-1 per-provider content, and the union's dedup + coverage.
    #[test]
    fn provider_state_dirs_cover_each_provider() {
        let home = Path::new("/Users/u");
        assert_eq!(provider_state_dirs("claude", home), vec![home.join(".claude")]);
        // Codex: env-dependent (`$CODEX_HOME`), so assert identity with the
        // canonical resolver — same treatment as opencode below; the
        // resolution itself is covered by the `codex_home_from` test.
        assert_eq!(provider_state_dirs("codex", home), vec![codex_home_dir(home)]);
        assert_eq!(provider_state_dirs("cursor", home), vec![home.join(".cursor")]);
        assert_eq!(provider_state_dirs("gemini", home), vec![home.join(".gemini")]);
        assert_eq!(provider_state_dirs("pi", home), vec![home.join(".pi")]);
        // OpenCode: XDG data + config subdirs. The exact paths are
        // env-dependent (`$XDG_DATA_HOME`/`$XDG_CONFIG_HOME` — CI runners
        // export their own), so assert identity with the canonical resolvers;
        // the resolution logic itself is covered hermetically by the
        // `xdg_base_from` test below.
        assert_eq!(
            provider_state_dirs("opencode", home),
            vec![opencode_data_dir(home), opencode_config_dir(home)],
        );
        // Unknown provider → empty, never a panic.
        assert!(provider_state_dirs("nope", home).is_empty());

        // The union carries every provider's dirs and is deduped.
        let all = all_provider_state_dirs(home);
        for expected in [
            home.join(".claude"),
            codex_home_dir(home),
            home.join(".cursor"),
            home.join(".gemini"),
            home.join(".pi"),
            opencode_data_dir(home),
            opencode_config_dir(home),
        ] {
            assert!(all.contains(&expected), "union missing {}", expected.display());
        }
        let mut deduped = all.clone();
        deduped.dedup();
        assert_eq!(deduped.len(), all.len(), "union must be free of duplicates");
    }

    /// The only `~/.config` grant the policy ever emits is a specific subdir
    /// (`opencode`), and the only `~/.local` grants are `share`/`state` — the
    /// concrete narrowing this security fix is about. Assertions are filtered
    /// against the fake home, so a CI runner's own `$XDG_*` vars (which point
    /// outside it, relocating opencode's dirs) can't perturb them.
    #[test]
    fn config_and_local_grants_are_narrow_subdirs() {
        let home = Path::new("/Users/u");
        let mut dirs = all_provider_state_dirs(home);
        dirs.extend(agent_scratch_dirs(home));

        // Anything under ~/.config must be exactly opencode's default config
        // subdir — never the root, never another subdir smuggled in outside
        // the policy. (With a custom `$XDG_CONFIG_HOME` opencode's dir lives
        // elsewhere and nothing under ~/.config is granted at all.)
        for d in dirs.iter().filter(|d| d.starts_with(home.join(".config"))) {
            assert_eq!(
                *d,
                home.join(".config/opencode"),
                "the only ~/.config grant may be ~/.config/opencode"
            );
        }

        let under_local: Vec<_> = dirs
            .iter()
            .filter(|d| d.starts_with(home.join(".local")))
            .collect();
        // `.local/share` (scratch), `.local/state` (scratch), and
        // `.local/share/opencode` (opencode data) — all narrow, none is `.local`.
        for d in &under_local {
            assert_ne!(**d, home.join(".local"), "must never be the bare ~/.local");
            assert!(d.starts_with(home.join(".local/share")) || d.starts_with(home.join(".local/state")));
        }
    }

    /// Hermetic coverage of the XDG resolution [`xdg_base`] performs at its
    /// env seam. The tests above deliberately never read the live env for
    /// expected values — CI runners export their own `XDG_*` vars — and don't
    /// mutate it either (parallel tests race on the process env), so the pure
    /// core carries the resolution contract.
    #[test]
    fn xdg_base_from_prefers_nonblank_value_else_default() {
        let home = Path::new("/Users/u");
        assert_eq!(
            xdg_base_from(Some("/custom/data".into()), home, ".local/share"),
            PathBuf::from("/custom/data")
        );
        assert_eq!(
            xdg_base_from(None, home, ".local/share"),
            home.join(".local/share")
        );
        // Blank counts as unset, per the XDG spec.
        assert_eq!(
            xdg_base_from(Some("".into()), home, ".config"),
            home.join(".config")
        );
    }

    /// Same hermetic env-seam coverage for codex's `$CODEX_HOME` relocation —
    /// the regression this guards: policy hardcoding `~/.codex` while docker
    /// resolved the env var left a relocated codex home write-denied under
    /// seatbelt (the old blanket `~/.config` grant had covered e.g.
    /// `CODEX_HOME=$HOME/.config/codex`).
    #[test]
    fn codex_home_from_prefers_nonblank_value_else_default() {
        let home = Path::new("/Users/u");
        assert_eq!(
            codex_home_from(Some("/Users/u/.config/codex".into()), home),
            PathBuf::from("/Users/u/.config/codex")
        );
        assert_eq!(codex_home_from(None, home), home.join(".codex"));
        // Blank counts as unset — the old docker-local resolution took it
        // verbatim and produced an empty, launch-failing mount source.
        assert_eq!(codex_home_from(Some("".into()), home), home.join(".codex"));
    }

    #[test]
    fn resolve_existing_prefix_resolves_symlinks_through_missing_leaf() {
        // A config dir may point at a dir the CLI hasn't created yet, under a
        // symlinked prefix (the /tmp → /private/tmp case). The existing prefix
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
}
