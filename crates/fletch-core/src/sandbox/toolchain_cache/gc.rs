//! Reclaiming project roots no project owns any more: a deleted project's
//! browsers and registries, and the flat layout from before the cache was keyed
//! by project.
//!
//! Safe against what a sandbox can plant: profiles and mounts grant a project's
//! own root, never the directory listed here, so each entry is the directory
//! the host created or, if an agent replaced its own root, a symlink, which
//! `remove_dir_all` removes without following. Nor does it follow links inside
//! the tree, so a link to `~/.cargo` planted in a doomed root takes the link and
//! leaves the target. And no agent of the project is running: deletion refuses
//! while one does, and at boot nothing has started yet.
//!
//! Best-effort, like the other startup sweeps: removals are logged, failures
//! warned about, and nothing is returned for a caller to handle.

use std::collections::HashSet;
use std::path::Path;

use super::vars::TOOLCHAIN_CACHE_VARS;

/// Remove every root in this build's directory ([`super::toolchain_cache_dir`])
/// whose project is not in `live_project_ids`, plus the pre-project flat
/// layout. Boot calls it, only for an engine that owns its state roots
/// (`host::StateRoots`): a nested instance's database knows nothing about the
/// caches beside its own.
pub fn reclaim_toolchain_caches(home: &Path, live_project_ids: &HashSet<String>) {
    // The flat layout was only ever written directly under the default
    // `~/.fletch/toolchain-cache`; an overridden base (a fletch-host's, a nested
    // instance's) never held it.
    let legacy_dir = super::base_override()
        .is_none()
        .then(|| home.join(".fletch").join("toolchain-cache"));
    reclaim_in(
        &super::toolchain_cache_dir(home),
        legacy_dir.as_deref(),
        live_project_ids,
    );
}

/// Remove one deleted project's root, the deletion-time half of the reclaim.
/// Deletion knows the id it just removed from this engine's own database, so
/// it needs no listing and no live set, and nothing can race it: a re-created
/// project gets a fresh id. The caller runs it off the async runtime and after
/// its lifecycle locks are released (`remove_dir_all` of a registry or a
/// browser is seconds of blocking I/O). A bad id is refused and logged, never
/// resolved to anything.
pub fn remove_project_cache(home: &Path, project_id: &str) {
    remove_in(&super::toolchain_cache_dir(home), project_id);
}

/// [`reclaim_toolchain_caches`] over explicit directories, so tests need
/// neither the process env nor a real home.
///
/// Only UUID-named entries are candidates, since that is what a project id is;
/// anything else in the directory is never touched. Of `legacy_dir`, only the
/// fixed set of names the flat layout wrote.
fn reclaim_in(dir: &Path, legacy_dir: Option<&Path>, live_project_ids: &HashSet<String>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
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
    let Some(legacy_dir) = legacy_dir else {
        return;
    };
    for name in legacy_names() {
        let path = legacy_dir.join(name);
        // Not following links: a link by one of these names is not a directory
        // the old layout wrote.
        if std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir()) {
            remove(&path, "the pre-project toolchain cache layout");
        }
    }
}

/// [`remove_project_cache`] in an explicit directory.
fn remove_in(dir: &Path, project_id: &str) {
    match super::project_root_in(dir, project_id) {
        Ok(root) if std::fs::symlink_metadata(&root).is_ok() => {
            remove(&root, "a deleted project's toolchain cache")
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(project_id, error = %e, "not reclaiming a toolchain cache"),
    }
}

/// The top-level directories the flat layout wrote: each table row's first
/// component, plus the cargo overlay. Derived rather than listed, so the set
/// is exactly what the table could have produced. `dev` is the one interim
/// shape: a debug build briefly put its segment *after* the leaf
/// (`toolchain-cache/dev/<id>`) before the layout was aligned with checkouts,
/// and nothing lives at that name now.
fn legacy_names() -> impl Iterator<Item = &'static str> {
    TOOLCHAIN_CACHE_VARS
        .iter()
        .filter_map(|(_, subdir, _)| subdir.split('/').next())
        .chain([crate::sandbox::cargo_home::SUBDIR, "dev"])
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
    use super::super::toolchain_cache_dir_in;
    use super::*;

    const LIVE: &str = "0b9f3c1e-6a4d-4e8b-9c2f-1d7e5a3b8c90";
    const DELETED: &str = "7d2a6f10-3c4b-4f5e-8a9d-0e1f2a3b4c5d";

    fn mkdir(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
    }

    /// The database decides: a UUID root it no longer lists goes, a live one
    /// and anything not shaped like a project id stay, and of the legacy
    /// directory only the old layout's names are removed.
    #[test]
    fn reclaim_removes_only_deleted_projects_and_the_legacy_layout() {
        let td = tempfile::tempdir().unwrap();
        let dir = toolchain_cache_dir_in(td.path());
        let legacy = td.path().join("toolchain-cache");
        for name in [LIVE, DELETED, "something-else"] {
            mkdir(&dir.join(name).join("npm"));
        }
        for name in ["npm", "cargo", "playwright", "unknown"] {
            mkdir(&legacy.join(name).join("inner"));
        }

        reclaim_in(&dir, Some(&legacy), &HashSet::from([LIVE.to_string()]));

        assert!(dir.join(LIVE).join("npm").is_dir());
        assert!(!dir.join(DELETED).exists());
        assert!(dir.join("something-else").is_dir());
        for name in ["npm", "cargo", "playwright"] {
            assert!(!legacy.join(name).exists(), "{name} survived");
        }
        assert!(legacy.join("unknown").is_dir());
    }

    /// An overridden base never held the flat layout, so with no legacy dir its
    /// legacy-looking names are left alone while its orphaned roots still go.
    #[test]
    fn reclaim_without_a_legacy_dir_spares_legacy_names() {
        let td = tempfile::tempdir().unwrap();
        let dir = toolchain_cache_dir_in(td.path());
        mkdir(&dir.join(DELETED));
        mkdir(&dir.join("npm"));

        reclaim_in(&dir, None, &HashSet::new());

        assert!(!dir.join(DELETED).exists());
        assert!(dir.join("npm").is_dir());
    }

    /// A link an agent planted inside its doomed root, or the root itself
    /// swapped for a link, takes the link and never what it points at.
    #[cfg(unix)]
    #[test]
    fn reclaim_does_not_follow_symlinks() {
        let td = tempfile::tempdir().unwrap();
        let target = td.path().join("real-cargo-home");
        mkdir(&target.join("registry"));
        let dir = toolchain_cache_dir_in(&td.path().join("base"));
        let legacy = td.path().join("legacy");
        mkdir(&dir.join(DELETED));
        mkdir(&legacy);
        std::os::unix::fs::symlink(&target, dir.join(DELETED).join("cargo")).unwrap();
        let swapped = "c3d4e5f6-a7b8-4c9d-8e0f-1a2b3c4d5e6f";
        std::os::unix::fs::symlink(&target, dir.join(swapped)).unwrap();
        std::os::unix::fs::symlink(&target, legacy.join("npm")).unwrap();

        reclaim_in(&dir, Some(&legacy), &HashSet::new());

        assert!(!dir.join(DELETED).exists());
        assert!(std::fs::symlink_metadata(dir.join(swapped)).is_err());
        assert!(
            target.join("registry").is_dir(),
            "the link's target was removed"
        );
        // A legacy name that is a link is not the old layout's directory.
        assert!(std::fs::symlink_metadata(legacy.join("npm")).is_ok());
    }

    /// Deletion removes exactly the one root it was told about: a sibling
    /// project's root is untouched, a missing root is not an error, and a bad
    /// id removes nothing.
    #[test]
    fn remove_takes_only_that_projects_root() {
        let td = tempfile::tempdir().unwrap();
        let dir = toolchain_cache_dir_in(td.path());
        for id in [DELETED, LIVE] {
            mkdir(&dir.join(id).join("npm"));
        }

        remove_in(&dir, DELETED);
        remove_in(&dir, "6e5d4c3b-2a19-4f8e-9d7c-6b5a4f3e2d1c");
        remove_in(&dir, ".");

        assert!(!dir.join(DELETED).exists());
        assert!(dir.join(LIVE).join("npm").is_dir());
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
