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
//! provider or scratch dir can't silently regress the class. For the
//! env-relocatable dirs (`$CODEX_HOME`, `$XDG_*` — and `$CLAUDE_CONFIG_DIR`,
//! resolved by the seatbelt caller) invariant 1 is additionally *enforced at
//! resolution time*: [`env_state_dir`] rejects a value that sits inside a
//! `bin`/`sbin` dir — checked on both the raw value and its symlink-resolved
//! form — and falls back to the default, so the provider fails closed under
//! that configuration instead of the profile granting a PATH dir:
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
//! Claude's config dir gets a third treatment that is really invariant 2
//! applied to `~/.claude` itself. `~/.claude` *is* a config root — its
//! `settings.json` defines hooks Claude Code runs **on the host**, and it holds
//! `plugins/`/`skills/`/`commands/`/`agents/`, `CLAUDE.md`, and MCP config — so
//! granting it whole was exactly the config-poisoning surface invariant 2 closes
//! for `~/.config`. So claude's grant is no longer the config-dir *root* but a
//! set of **writable islands** beneath it ([`claude_write_island_dirs`] + the
//! [`CLAUDE_CREDENTIALS_FILE`] file): claude-regenerated, non-executable,
//! non-config state only. This mirrors Docker's invariant 5 (`~/.claude` mounted
//! read-only with writable exceptions layered on); the shared island-name
//! constants ([`CLAUDE_PROJECTS_SUBDIR`], [`CLAUDE_EPHEMERAL_RUNTIME_SUBDIRS`],
//! [`CLAUDE_CREDENTIALS_FILE`]) are the same ones the docker engine mounts.
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

/// The one **file** under a claude config dir that stays writable when the dir
/// is otherwise read-only: claude's OAuth refresh rewrites the rotated token
/// here, and the host-side `CredentialsFile` auth chain needs that write to
/// land on the host. Docker remounts it read-write over the read-only config
/// dir; seatbelt grants it as a file rule (see the seatbelt caller). Shared so
/// the two engines can't drift on the name.
pub const CLAUDE_CREDENTIALS_FILE: &str = ".credentials.json";

/// Claude's session-transcript subdir (`<config-dir>/projects/<slug>/*.jsonl`).
/// Docker binds it to a persistent per-agent host dir so `--resume` survives
/// container recreation; seatbelt grants the real one as an island (see the
/// residual note on [`claude_write_island_dirs`]). Shared so the two engines
/// agree on the name.
pub const CLAUDE_PROJECTS_SUBDIR: &str = "projects";

/// Subdirs claude creates and writes *afresh every session*: the per-session
/// env store (`session-env/<id>`) and the shell-environment snapshot the Bash
/// tool sources (`shell-snapshots`). Docker overlays each with an ephemeral
/// tmpfs (claude otherwise `mkdir`s them under the read-only config dir and
/// fails `EROFS`); seatbelt grants the real ones as islands. Shared so the two
/// engines agree on the names.
pub const CLAUDE_EPHEMERAL_RUNTIME_SUBDIRS: &[&str] = &["session-env", "shell-snapshots"];

/// Extra pure-state subdirs seatbelt grants writable that docker leaves
/// read-only. Docker leaves *everything* but the credential file, the projects
/// bind, and the tmpfs overlays read-only, and claude's writes there are
/// best-effort (a failed write logs and continues). Seatbelt shares the host
/// filesystem, so these writes actually land on the real `~/.claude` and are
/// worth allowing — but only for claude-regenerated, non-executable, non-config
/// state that can neither run code on the host nor steer a later host session:
/// per-session todo lists, the feature-flag/analytics cache, file edit-history
/// (undo state), and diagnostic logs. Anything that could influence a later
/// session's trust/behavior (`backups/` of `~/.claude.json`, `plans/`,
/// `sessions/`, `cache/`, `paste-cache/`) or execute (`downloads/` of the
/// claude binary, `chrome/`, `daemon/`) is deliberately NOT here — deny by
/// default (fail-closed) is the correct treatment for those.
const CLAUDE_SEATBELT_STATE_SUBDIRS: &[&str] = &["todos", "statsig", "file-history", "debug"];

/// The writable **islands** beneath a claude config `config_dir` — the write
/// grant that replaces the old whole-`~/.claude` grant (config-poisoning
/// narrowing; see the module doc). Each is claude-regenerated, non-executable,
/// non-config *state*: session transcripts, per-session runtime scaffolding, and
/// the pure-state dirs above. Everything else under the config dir
/// (`settings.json` → host hooks, `plugins`/`skills`/`commands`/`agents`,
/// `CLAUDE.md`, MCP config, …) stays deny-by-default, so a prompt-injected agent
/// can't plant a host-executed hook or a config a later HOST session trusts.
///
/// The credential *file* ([`CLAUDE_CREDENTIALS_FILE`]) is intentionally NOT
/// here: it's a file needing a regex rule (atomic temp-file writes), which the
/// seatbelt caller emits separately.
///
/// Two DELIBERATE residuals, left writable here and flagged for later hardening
/// rather than fixed now:
///
/// 1. **`projects/` stays writable.** Docker redirects it to a per-agent host
///    dir so the shared `~/.claude/projects` (other agents' transcripts, global
///    memory) is unreachable; seatbelt can't redirect without env surgery, and
///    the app's host-side transcript reader tails the real dir. Residual: an
///    agent can write a `projects/<slug>/memory` entry a later session trusts.
/// 2. **`~/.claude.json` stays a writable literal** (emitted by the seatbelt
///    caller, not here — it's outside the config dir). Claude requires it (state
///    writes every session), yet it carries per-project trust/settings. Both are
///    candidates for later hardening.
pub fn claude_write_island_dirs(config_dir: &Path) -> Vec<PathBuf> {
    std::iter::once(CLAUDE_PROJECTS_SUBDIR)
        .chain(CLAUDE_EPHEMERAL_RUNTIME_SUBDIRS.iter().copied())
        .chain(CLAUDE_SEATBELT_STATE_SUBDIRS.iter().copied())
        .map(|s| config_dir.join(s))
        .collect()
}

/// Class-1 host-persistence dirs a single `provider` must write on the host:
/// its session store, auth, and config. Docker mounts exactly these read-write
/// at their host paths; the seatbelt profile folds them in provider-agnostically
/// via [`all_provider_state_dirs`]. An unrecognized provider yields an empty
/// list — Docker already gates on a known [`super::docker::DockerProvider`], and
/// the seatbelt side only ever passes ids from [`STATE_DIR_PROVIDERS`].
///
/// Note the omissions this policy makes on purpose: claude's grant is NOT its
/// config-dir root (that root holds host-executed hooks / plugins / MCP config
/// — see the module doc) but the writable **islands** beneath it
/// ([`claude_write_island_dirs`]); the top-level `~/.claude.json` file and the
/// [`CLAUDE_CREDENTIALS_FILE`] file both stay seatbelt-local literals (a file,
/// not a dir, so the dir-oriented API doesn't model them), and a non-default
/// `CLAUDE_CONFIG_DIR` is likewise handled by the seatbelt caller, which needs
/// its own symlink-resolution to keep the emitted SBPL path matching the
/// sandbox's resolved write path. Codex's and opencode's env relocations
/// (`$CODEX_HOME`, `$XDG_*`), by contrast, ARE resolved here — both engines
/// need the same dir, with no engine-specific handling on top.
pub fn provider_state_dirs(provider: &str, home: &Path) -> Vec<PathBuf> {
    match provider {
        // Not the `~/.claude` root — only its writable islands (config-poisoning
        // narrowing, module doc). The credential *file* is added separately by
        // the seatbelt caller (it needs a regex rule, not a `(subpath …)`).
        "claude" => claude_write_island_dirs(&home.join(".claude")),
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
    env_state_dir(value, home.join(".codex"))
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
    env_state_dir(value, home.join(default_rel))
}

/// Shared resolution for every env-relocatable state dir: use the env `value`
/// unless it's absent, blank, or violates invariant 1. A value that sits
/// inside a `bin`/`sbin` directory is REJECTED in favor of `default` — a grant
/// there would put agent-writable files directly on the user's PATH (e.g.
/// `XDG_DATA_HOME=$HOME/.local/bin` would grant `~/.local/bin/opencode`,
/// letting an agent create an on-PATH executable named `opencode`; a
/// bin-resident `$CODEX_HOME` would grant the whole bin dir). The check runs
/// on the raw value AND its symlink-resolved form, so a link disguising a bin
/// dir doesn't slip through. Rejection is fail-closed: under such a
/// configuration the provider's writes are denied (it looks at the env var,
/// we grant the default) — a visible breakage, never a hijack surface.
fn env_state_dir(value: Option<std::ffi::OsString>, default: PathBuf) -> PathBuf {
    let Some(v) = value.filter(|v| !v.is_empty()) else {
        return default;
    };
    let raw = PathBuf::from(v);
    if bin_resident(&raw) {
        return default;
    }
    raw
}

/// Whether `p` — raw or symlink-resolved — has a `bin`/`sbin` path component,
/// i.e. is or lives inside a PATH-style binaries dir (invariant 1's rejection
/// predicate). Component equality, not substring: `~/binaries` is fine.
/// `pub(crate)` for the seatbelt `CLAUDE_CONFIG_DIR` handling, which resolves
/// its env value itself and must apply the same rejection.
pub(crate) fn bin_resident(p: &Path) -> bool {
    has_bin_component(p) || has_bin_component(&resolve_existing_prefix(p))
}

fn has_bin_component(p: &Path) -> bool {
    use std::path::Component;
    p.components()
        .any(|c| matches!(c, Component::Normal(n) if n == "bin" || n == "sbin"))
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

    /// The claude write islands are exactly the config-poisoning narrowing: each
    /// is a subdir *beneath* a config dir, never the config-dir root itself, and
    /// never a bin dir — the same invariants, applied to `~/.claude`.
    #[test]
    fn claude_islands_are_subdirs_never_the_config_root_or_bin() {
        let config_dir = Path::new("/Users/u/.claude");
        let islands = claude_write_island_dirs(config_dir);

        // The exact set (order-independent): the shared projects/ephemeral
        // names plus the seatbelt-only pure-state subdirs.
        let expected: Vec<PathBuf> = std::iter::once(CLAUDE_PROJECTS_SUBDIR)
            .chain(CLAUDE_EPHEMERAL_RUNTIME_SUBDIRS.iter().copied())
            .chain(CLAUDE_SEATBELT_STATE_SUBDIRS.iter().copied())
            .map(|s| config_dir.join(s))
            .collect();
        assert_eq!(islands, expected);
        // The credential *file* is not an island dir (it's a file rule).
        assert!(!islands.iter().any(|p| p.ends_with(CLAUDE_CREDENTIALS_FILE)));

        for island in &islands {
            assert!(
                island.starts_with(config_dir) && *island != config_dir,
                "{} must be a strict subdir of the config root, never the root",
                island.display()
            );
            assert!(!bin_resident(island), "{} must not be bin-resident", island.display());
            assert_ne!(island.file_name(), Some(OsStr::new("bin")));
        }
    }

    /// Class-1 per-provider content, and the union's dedup + coverage.
    #[test]
    fn provider_state_dirs_cover_each_provider() {
        let home = Path::new("/Users/u");
        // Claude's entry is no longer the `~/.claude` root — it's the writable
        // islands beneath it (config-poisoning narrowing).
        assert_eq!(
            provider_state_dirs("claude", home),
            claude_write_island_dirs(&home.join(".claude"))
        );
        assert!(
            !provider_state_dirs("claude", home).contains(&home.join(".claude")),
            "claude must not grant its config-dir root"
        );
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
        // Claude contributes its islands to the union (not the `~/.claude` root).
        assert!(!all.contains(&home.join(".claude")), "union must not carry the ~/.claude root");
        for island in claude_write_island_dirs(&home.join(".claude")) {
            assert!(all.contains(&island), "union missing claude island {}", island.display());
        }
        for expected in [
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

    /// Invariant 1 enforced at resolution time, not just asserted over the
    /// defaults: an env relocation into a `bin`/`sbin` dir must be rejected,
    /// or the profile would grant an agent-writable path directly on the
    /// user's PATH (`XDG_DATA_HOME=$HOME/.local/bin` → a writable on-PATH
    /// `opencode`; a bin-resident `$CODEX_HOME` → the whole bin dir).
    #[test]
    fn env_relocations_inside_bin_dirs_are_rejected() {
        let home = Path::new("/Users/u");

        assert_eq!(
            xdg_base_from(Some("/Users/u/.local/bin".into()), home, ".local/share"),
            home.join(".local/share"),
            "XDG base inside ~/.local/bin must fall back to the default"
        );
        assert_eq!(
            codex_home_from(Some("/Users/u/.local/bin/codex".into()), home),
            home.join(".codex"),
            "CODEX_HOME under a bin dir must fall back to the default"
        );
        // `sbin` is the same PATH-style class.
        assert_eq!(
            codex_home_from(Some("/usr/local/sbin/codex".into()), home),
            home.join(".codex")
        );
        // Component equality, not substring: a dir merely *named like* bin is
        // a legitimate relocation target.
        assert_eq!(
            codex_home_from(Some("/Users/u/binaries/codex".into()), home),
            PathBuf::from("/Users/u/binaries/codex")
        );

        // A symlink disguising a bin dir resolves into one → still rejected.
        let td = tempfile::tempdir().unwrap();
        let bin = td.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let link = td.path().join("data");
        std::os::unix::fs::symlink(&bin, &link).unwrap();
        assert_eq!(
            codex_home_from(Some(link.into_os_string()), home),
            home.join(".codex"),
            "a symlink resolving into a bin dir must be rejected too"
        );
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
