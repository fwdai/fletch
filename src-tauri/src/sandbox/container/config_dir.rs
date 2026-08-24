//! Non-default config-dir detection (does the container need a `-e CLAUDE_CONFIG_DIR`
//! / `-e CODEX_HOME` / `-e XDG_*`?) and the borrowed git object stores a
//! `--shared` clone reaches through alternates.
//!
//! Shared by the Docker and Podman engines: this is launch-spec policy, not
//! runtime plumbing.

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
pub fn codex_home_is_nondefault(home: &Path) -> bool {
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
/// [`resolve_existing_prefix`] so a symlink can't read as non-default. Launch-time
/// env-forwarding logic (does the container need a `-e XDG_*`?), not a
/// write-policy question.
pub fn xdg_base_is_nondefault(var: &str, home: &Path, default_rel: &str) -> bool {
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
pub fn nondefault_claude_config_dir(home: &Path) -> Option<PathBuf> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from)?;
    (!config_dir_is_default(&dir, home)).then_some(dir)
}

/// Whether `dir` resolves to the default `~/.claude`. Both sides go through
/// [`resolve_existing_prefix`] — see [`nondefault_claude_config_dir`] for why.
/// Pure over its inputs so the comparison rule is directly testable.
pub fn config_dir_is_default(dir: &Path, home: &Path) -> bool {
    resolve_existing_prefix(dir) == resolve_existing_prefix(&home.join(".claude"))
}

/// Every object store an agent's `--shared` checkouts borrow via git
/// alternates — each an absolute path to mount read-only.
///
/// SECURITY: the mount set is derived from `source_repos`, Fletch's
/// authoritative record of each checkout's source repo
/// (`AgentRecord.repos[].repo_path` — the user's own repos, which the agent
/// cannot write). It is deliberately NOT derived from the checkout's own
/// `<subdir>/.git/objects/info/alternates`. Under a container engine the whole
/// checkout is bind-mounted read-write, so a container agent can overwrite that
/// alternates file to name any absolute host path (`~/.ssh`, `~/.aws`, Fletch's
/// own DB); were the mount set read from it, a later relaunch that reuses the
/// on-disk checkout without re-provisioning (`resume_agent` / `switch_view`)
/// would bind-mount the attacker's path read-only into the container and expose
/// it over the always-open network — defeating the ConfinedReads /
/// OpaqueAppData guarantees. Reading only the user-owned source repos keeps
/// that agent-writable file out of the trust boundary entirely.
///
/// This reproduces exactly what a `--shared` clone borrows: `git clone
/// --shared <source>` records `<source>/.git/objects` in the checkout's
/// alternates, so that store is what must be mounted. A multi-repo agent has
/// one source per repo, so every entry is walked, not just the primary — else
/// secondary checkouts' borrowed objects stay unmounted and git breaks
/// (log/diff/checkout/commit) there.
///
/// The chain is followed transitively from each SOURCE (safe — the source is
/// user-owned): a source may itself borrow (B from A), leaving the checkout
/// pointing at `<B>/.git/objects` while git resolves B→A at runtime, so A must
/// be mounted too or in-container git can't normalize the alternate. Results
/// are deduped (repos may share a base) and cycle-guarded. A missing store is
/// dropped, not mounted — see below.
pub fn borrowed_object_stores(source_repos: &[PathBuf]) -> Vec<PathBuf> {
    /// The alternates listed in `<objects_dir>/info/alternates`, if any. Only
    /// ever called on stores reached from a trusted source repo, so the file it
    /// reads is always under user-owned (non-agent-writable) state.
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
    // Seed the walk from each tracked SOURCE repo's own object store — the exact
    // store a `--shared` clone of that source records in the checkout. Order
    // follows the record's repo order (primary first), which is deterministic.
    let mut queue: std::collections::VecDeque<PathBuf> = source_repos
        .iter()
        .map(|repo| repo.join(".git/objects"))
        .collect();
    while let Some(store) = queue.pop_front() {
        if !seen.insert(store.clone()) {
            continue;
        }
        // Follow the source's own alternates chain before emitting the store,
        // so a chained base (A behind B) is discovered and mounted too.
        for next in read_alternates(&store) {
            queue.push_back(next);
        }
        // Only mount a store that exists on disk: a bare `-v <path>:<path>:ro`
        // on a missing source has the runtime create it *root-owned*, and a
        // `--shared` clone can only resolve objects that actually exist, so a
        // missing store is never one in-container git needs.
        if store.is_dir() {
            out.push(store);
        }
    }
    out
}
