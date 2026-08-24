//! Non-default config-dir detection (does the container need a `-e CLAUDE_CONFIG_DIR`
//! / `-e CODEX_HOME` / `-e XDG_*`?) and the borrowed git object stores a
//! `--shared` clone reaches through alternates. Runtime-neutral: every answer
//! here is a question about the *host* env and filesystem.

use std::path::{Path, PathBuf};

use crate::sandbox::policy::resolve_existing_prefix;

/// Whether `$CODEX_HOME` names a dir other than the default `~/.codex`. Both
/// sides resolve through [`resolve_existing_prefix`] so a symlink can't read as
/// non-default; blank counts as unset.
pub(crate) fn codex_home_is_nondefault(home: &Path) -> bool {
    match std::env::var_os("CODEX_HOME") {
        Some(v) if !v.is_empty() => {
            resolve_existing_prefix(&PathBuf::from(v))
                != resolve_existing_prefix(&home.join(".codex"))
        }
        _ => false,
    }
}

// The dirs themselves (`codex_home_dir`, `opencode_*_dir`, `xdg_base`) live in
// `crate::sandbox::policy` — every engine shares them.

/// Whether `$var` points to an XDG base other than the default
/// `home/<default_rel>`, resolved on both sides so a symlink can't read as
/// non-default.
pub(crate) fn xdg_base_is_nondefault(var: &str, home: &Path, default_rel: &str) -> bool {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => {
            resolve_existing_prefix(&PathBuf::from(v))
                != resolve_existing_prefix(&home.join(default_rel))
        }
        _ => false,
    }
}

/// A non-default `CLAUDE_CONFIG_DIR`, or `None` when unset or resolving to the
/// already-mounted `~/.claude`. Returns the *original* path, not the resolved
/// one, so mount and forwarded value stay at the host path (invariant 1).
pub(crate) fn nondefault_claude_config_dir(home: &Path) -> Option<PathBuf> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from)?;
    (!config_dir_is_default(&dir, home)).then_some(dir)
}

/// Whether `dir` resolves to the default `~/.claude` — resolved on both sides,
/// so a symlink or trailing slash can't read as non-default.
pub(crate) fn config_dir_is_default(dir: &Path, home: &Path) -> bool {
    resolve_existing_prefix(dir) == resolve_existing_prefix(&home.join(".claude"))
}

/// Every object store an agent's `--shared` checkouts borrow via git
/// alternates, to mount read-only. Walks all source repos (a multi-repo agent's
/// secondary checkouts break otherwise) and follows chains transitively, deduped
/// and cycle-guarded.
///
/// SECURITY: derived from `source_repos`, never from the checkout's own
/// `.git/objects/info/alternates` — that file is agent-writable, so trusting it
/// would let an agent name any host path and have a later launch mount it.
pub(crate) fn borrowed_object_stores(source_repos: &[PathBuf]) -> Vec<PathBuf> {
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

    let mut out: Vec<PathBuf> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    // The exact store a `--shared` clone of each source records in the checkout;
    // repo order (primary first) makes the result deterministic.
    let mut queue: std::collections::VecDeque<PathBuf> = source_repos
        .iter()
        .map(|repo| repo.join(".git/objects"))
        .collect();
    while let Some(store) = queue.pop_front() {
        if !seen.insert(store.clone()) {
            continue;
        }
        for next in read_alternates(&store) {
            queue.push_back(next);
        }
        // A missing store is never one in-container git needs, and mounting it
        // would have the runtime create the path *root-owned*.
        if store.is_dir() {
            out.push(store);
        }
    }
    out
}
