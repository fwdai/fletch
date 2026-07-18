//! Non-default config-dir detection (does the container need a `-e CLAUDE_CONFIG_DIR`
//! / `-e CODEX_HOME` / `-e XDG_*`?) and the borrowed git object stores a
//! `--shared` clone reaches through alternates.

use std::path::{Path, PathBuf};

use crate::sandbox::policy::resolve_existing_prefix;

/// Whether `$CODEX_HOME` is set to a dir other than the default `~/.codex`
/// (which the container already resolves via `HOME`). Only a non-default value
/// is forwarded, mirroring [`nondefault_claude_config_dir`]; both sides go
/// through [`resolve_existing_prefix`] so a symlink can't read as non-default.
/// Blank counts as unset, matching [`codex_home_dir`]'s resolution —
/// forwarding a blank value the resolver ignored would desync the two.
///
/// [`codex_home_dir`]: crate::sandbox::policy::codex_home_dir
pub(super) fn codex_home_is_nondefault(home: &Path) -> bool {
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
// Imported at the top of the engine module.

/// Whether `$var` points to an XDG base other than the default `home/<default_rel>`
/// the container already resolves via `HOME`. Only a non-default base is forwarded,
/// mirroring [`codex_home_is_nondefault`]; both sides canonicalize via
/// [`resolve_existing_prefix`] so a symlink can't read as non-default. This stays
/// docker-local: it's launch-time env-forwarding logic (does the container need a
/// `-e XDG_*`?), not a write-policy question.
pub(super) fn xdg_base_is_nondefault(var: &str, home: &Path, default_rel: &str) -> bool {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => {
            resolve_existing_prefix(&PathBuf::from(v))
                != resolve_existing_prefix(&home.join(default_rel))
        }
        _ => false,
    }
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
pub(super) fn nondefault_claude_config_dir(home: &Path) -> Option<PathBuf> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from)?;
    (!config_dir_is_default(&dir, home)).then_some(dir)
}

/// Whether `dir` resolves to the default `~/.claude`. Both sides go through
/// [`resolve_existing_prefix`] — see [`nondefault_claude_config_dir`] for why.
/// Pure over its inputs so the comparison rule is directly testable.
pub(super) fn config_dir_is_default(dir: &Path, home: &Path) -> bool {
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
pub(super) fn borrowed_object_stores(writable_root: &Path) -> Vec<PathBuf> {
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
