//! Reclaiming toolchain cache roots ([`super::toolchain_cache_root`]) that no
//! project owns any more: a deleted project's browsers and registries, and the
//! flat layout from before the cache was keyed by project.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use super::policy::{
    default_toolchain_cache_parent, toolchain_cache_build_dir_in, toolchain_cache_parent,
    TOOLCHAIN_CACHE_VARS,
};

/// Set once by `host::boot` when this engine owns its state roots
/// (`StateRoots::Owned`). Until then [`reclaim_toolchain_caches`] does nothing:
/// a nested Fletch's or a test engine's database knows nothing about the caches
/// beside its own, and would read every one of them as a deleted project's.
static OWNED: AtomicBool = AtomicBool::new(false);

/// Publish that this engine's database is the only authority over its
/// toolchain cache directory — the same claim `StateRoots::Owned` makes for
/// the checkout and mailbox roots.
pub fn own_toolchain_caches() {
    OWNED.store(true, Ordering::Relaxed);
}

/// Remove every project root in this build's cache directory
/// ([`super::policy::toolchain_cache_build_dir`]) whose project is not in
/// `live_project_ids`, plus the pre-project flat layout. Called at boot and
/// after a project is deleted, with the ids the database holds at that moment.
///
/// Only UUID-named entries are candidates, since that is what a project id is;
/// anything else under the directory (a debug build's `dev/` beside release's
/// roots, whatever a later layout adds) is never touched. The legacy layout is
/// a fixed set of names at the parent level, only ever written under the
/// default parent, so an overridden parent is spared that half.
///
/// Safe against what a sandbox can plant: profiles and mounts grant a
/// project's own root, never the build directory or the parent, so the entry
/// listed is the directory the host created or, if an agent replaced its own
/// root, a symlink, which `remove_dir_all` removes without following. Nor does
/// it follow links inside the tree, so a link to `~/.cargo` planted in a
/// doomed root takes the link and leaves the target. And no agent of the
/// project is running: deletion refuses while one does, and at boot nothing
/// has started yet.
///
/// Best-effort, like the other startup sweeps: removals are logged, failures
/// warned about, and nothing is returned for a caller to handle.
pub fn reclaim_toolchain_caches(home: &Path, live_project_ids: &HashSet<String>) {
    if !OWNED.load(Ordering::Relaxed) {
        return;
    }
    let parent = toolchain_cache_parent(home);
    let legacy = parent == default_toolchain_cache_parent(home);
    reclaim_in(&parent, legacy, live_project_ids);
}

/// Remove one deleted project's root ([`super::toolchain_cache_root`]), the
/// deletion-time half of the reclaim. Project deletion knows the id it just
/// removed, so it needs no listing and no live set: nothing can race it, since
/// a re-created project gets a fresh id. The caller runs it off the async
/// runtime and after its lifecycle locks are released (`remove_dir_all` of a
/// registry or a browser is seconds of blocking I/O), which is also why it is
/// not folded into the full sweep. Same ownership gate and best-effort posture
/// as [`reclaim_toolchain_caches`]; a bad id is refused by the root function
/// and logged, never resolved to anything.
pub fn remove_project_cache(home: &Path, project_id: &str) {
    if !OWNED.load(Ordering::Relaxed) {
        return;
    }
    match super::toolchain_cache_root(home, project_id) {
        Ok(root) if std::fs::symlink_metadata(&root).is_ok() => {
            remove(&root, "a deleted project's toolchain cache")
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(project_id, error = %e, "not reclaiming a toolchain cache"),
    }
}

/// [`reclaim_toolchain_caches`] under an explicit parent, ownership already
/// established, so tests need neither the process env nor a real home.
fn reclaim_in(parent: &Path, sweep_legacy: bool, live_project_ids: &HashSet<String>) {
    let build_dir = toolchain_cache_build_dir_in(parent);
    if let Ok(entries) = std::fs::read_dir(&build_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let orphaned = name.to_str().is_some_and(|name| {
                uuid::Uuid::parse_str(name).is_ok() && !live_project_ids.contains(name)
            });
            if orphaned {
                remove(&entry.path(), "a deleted project's toolchain cache");
            }
        }
    }
    if !sweep_legacy {
        return;
    }
    for name in legacy_names() {
        let path = parent.join(name);
        // Not following links: a link by one of these names is not a directory
        // the old layout wrote.
        if std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir()) {
            remove(&path, "the pre-project toolchain cache layout");
        }
    }
}

/// The top-level directories the flat layout wrote: each table row's first
/// component, plus the cargo overlay. Derived rather than listed, so the set
/// is exactly what the table could have produced.
fn legacy_names() -> impl Iterator<Item = &'static str> {
    TOOLCHAIN_CACHE_VARS
        .iter()
        .filter_map(|(_, subdir)| subdir.split('/').next())
        .chain(std::iter::once(super::cargo_home::SUBDIR))
}

fn remove(path: &Path, what: &str) {
    match std::fs::remove_dir_all(path) {
        Ok(()) => tracing::info!(path = %path.display(), "reclaimed {what}"),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "could not reclaim {what}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIVE: &str = "0b9f3c1e-6a4d-4e8b-9c2f-1d7e5a3b8c90";
    const DELETED: &str = "7d2a6f10-3c4b-4f5e-8a9d-0e1f2a3b4c5d";

    fn mkdir(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
    }

    /// The database decides: a UUID root it no longer lists goes, a live one
    /// and anything not shaped like a project id stay, and of the parent level
    /// only the old layout's names are removed.
    #[test]
    fn reclaim_removes_only_deleted_projects_and_the_legacy_layout() {
        let td = tempfile::tempdir().unwrap();
        let parent = td.path().join("toolchain-cache");
        let build_dir = toolchain_cache_build_dir_in(&parent);
        for name in [LIVE, DELETED, "dev", "something-else"] {
            mkdir(&build_dir.join(name).join("npm"));
        }
        for name in ["npm", "cargo", "playwright", "unknown"] {
            mkdir(&parent.join(name).join("inner"));
        }

        reclaim_in(&parent, true, &HashSet::from([LIVE.to_string()]));

        assert!(build_dir.join(LIVE).join("npm").is_dir());
        assert!(!build_dir.join(DELETED).exists());
        assert!(build_dir.join("dev").is_dir());
        assert!(build_dir.join("something-else").is_dir());
        for name in ["npm", "cargo", "playwright"] {
            assert!(!parent.join(name).exists(), "{name} survived");
        }
        assert!(parent.join("unknown").is_dir());
    }

    /// An overridden parent never held the flat layout, so its legacy-looking
    /// names are left alone while its orphaned roots still go.
    #[test]
    fn reclaim_spares_legacy_names_under_an_overridden_parent() {
        let td = tempfile::tempdir().unwrap();
        let parent = td.path().join("nested");
        let build_dir = toolchain_cache_build_dir_in(&parent);
        mkdir(&build_dir.join(DELETED));
        mkdir(&parent.join("npm"));

        reclaim_in(&parent, false, &HashSet::new());

        assert!(!build_dir.join(DELETED).exists());
        assert!(parent.join("npm").is_dir());
    }

    /// A link an agent planted inside its doomed root, or the root itself
    /// swapped for a link, takes the link and never what it points at.
    #[cfg(unix)]
    #[test]
    fn reclaim_does_not_follow_symlinks() {
        let td = tempfile::tempdir().unwrap();
        let target = td.path().join("real-cargo-home");
        mkdir(&target.join("registry"));
        let parent = td.path().join("toolchain-cache");
        let build_dir = toolchain_cache_build_dir_in(&parent);
        mkdir(&build_dir.join(DELETED));
        std::os::unix::fs::symlink(&target, build_dir.join(DELETED).join("cargo")).unwrap();
        let swapped = "c3d4e5f6-a7b8-4c9d-8e0f-1a2b3c4d5e6f";
        std::os::unix::fs::symlink(&target, build_dir.join(swapped)).unwrap();
        std::os::unix::fs::symlink(&target, parent.join("npm")).unwrap();

        reclaim_in(&parent, true, &HashSet::new());

        assert!(!build_dir.join(DELETED).exists());
        assert!(std::fs::symlink_metadata(build_dir.join(swapped)).is_err());
        assert!(
            target.join("registry").is_dir(),
            "the link's target was removed"
        );
        // A legacy name that is a link is not the old layout's directory.
        assert!(std::fs::symlink_metadata(parent.join("npm")).is_ok());
    }

    /// Deletion removes exactly the one root it was told about: a sibling
    /// project's root and the legacy layout are untouched, and a missing root
    /// is not an error. Ownership is a process-wide claim, so this test
    /// establishes it and never un-claims (nothing else here depends on it
    /// being unset).
    #[test]
    fn remove_project_cache_takes_only_that_projects_root() {
        let td = tempfile::tempdir().unwrap();
        let home = td.path();
        let root = |id: &str| super::super::toolchain_cache_root(home, id).unwrap();
        let (doomed, live) = (root(DELETED), root(LIVE));
        let parent = toolchain_cache_parent(home);
        for dir in [&doomed, &live] {
            mkdir(&dir.join("npm"));
        }
        mkdir(&parent.join("npm"));

        own_toolchain_caches();
        remove_project_cache(home, DELETED);
        remove_project_cache(home, "6e5d4c3b-2a19-4f8e-9d7c-6b5a4f3e2d1c");

        assert!(!doomed.exists());
        assert!(live.join("npm").is_dir());
        assert!(parent.join("npm").is_dir());
    }

    /// The legacy names come from the table, so a row added later is covered
    /// without anyone retyping the list.
    #[test]
    fn legacy_names_cover_every_table_row_and_cargo() {
        let names: HashSet<_> = legacy_names().collect();
        for name in ["bun", "npm", "pnpm", "go", "poetry", "playwright", "cargo"] {
            assert!(names.contains(name), "{name} missing");
        }
        assert!(names.iter().all(|n| !n.contains('/') && !n.is_empty()));
    }
}
