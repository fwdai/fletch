//! Deleting caches from a directory that holds one per project, named by
//! project id.

use std::collections::HashSet;
use std::path::Path;

use super::vars::TOOLCHAIN_CACHE_VARS;

pub(super) fn remove_project(dir: &Path, project_id: &str) {
    match super::layout::project_root_in(dir, project_id) {
        Ok(root) if std::fs::symlink_metadata(&root).is_ok() => {
            remove(&root, "a deleted project's toolchain cache")
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(project_id, error = %e, "not reclaiming a toolchain cache"),
    }
}

/// Only UUID-named entries are candidates, since that is what a project id is;
/// anything else in `dir` is never touched.
pub(super) fn remove_orphans(dir: &Path, live_project_ids: &HashSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
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

/// Removes from `dir` only the names the flat layout wrote.
pub(super) fn remove_flat_layout(dir: &Path) {
    for name in flat_layout_names() {
        let path = dir.join(name);
        if std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir()) {
            remove(&path, "the pre-project toolchain cache layout");
        }
    }
}

/// The top-level directories the flat layout wrote: each table row's first
/// component, plus the cargo overlay. Derived rather than listed, so the set
/// is exactly what the table could have produced. `dev` is the one interim
/// shape: a debug build briefly put its segment *after* the leaf
/// (`toolchain-cache/dev/<id>`) before the layout was aligned with checkouts,
/// and nothing lives at that name now.
fn flat_layout_names() -> impl Iterator<Item = &'static str> {
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
    use super::super::layout::toolchain_cache_dir_in;
    use super::*;

    const LIVE: &str = "0b9f3c1e-6a4d-4e8b-9c2f-1d7e5a3b8c90";
    const DELETED: &str = "7d2a6f10-3c4b-4f5e-8a9d-0e1f2a3b4c5d";

    fn mkdir(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
    }

    /// The database decides: a UUID root it no longer lists goes, a live one
    /// and anything not shaped like a project id stay, and of the flat
    /// layout's directory only that layout's names are removed.
    #[test]
    fn reclaim_removes_only_deleted_projects_and_the_flat_layout() {
        let td = tempfile::tempdir().unwrap();
        let dir = toolchain_cache_dir_in(td.path());
        let flat = td.path().join("toolchain-cache");
        for name in [LIVE, DELETED, "something-else"] {
            mkdir(&dir.join(name).join("npm"));
        }
        for name in ["npm", "cargo", "playwright", "unknown"] {
            mkdir(&flat.join(name).join("inner"));
        }

        remove_orphans(&dir, &HashSet::from([LIVE.to_string()]));
        remove_flat_layout(&flat);

        assert!(dir.join(LIVE).join("npm").is_dir());
        assert!(!dir.join(DELETED).exists());
        assert!(dir.join("something-else").is_dir());
        for name in ["npm", "cargo", "playwright"] {
            assert!(!flat.join(name).exists(), "{name} survived");
        }
        assert!(flat.join("unknown").is_dir());
    }

    /// Removing orphans alone leaves a name the flat layout wrote: an
    /// overridden base never held that layout, so there it is someone's own.
    #[test]
    fn removing_orphans_spares_flat_layout_names() {
        let td = tempfile::tempdir().unwrap();
        let dir = toolchain_cache_dir_in(td.path());
        mkdir(&dir.join(DELETED));
        mkdir(&dir.join("npm"));

        remove_orphans(&dir, &HashSet::new());

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
        let flat = td.path().join("flat");
        mkdir(&dir.join(DELETED));
        mkdir(&flat);
        std::os::unix::fs::symlink(&target, dir.join(DELETED).join("cargo")).unwrap();
        let swapped = "c3d4e5f6-a7b8-4c9d-8e0f-1a2b3c4d5e6f";
        std::os::unix::fs::symlink(&target, dir.join(swapped)).unwrap();
        std::os::unix::fs::symlink(&target, flat.join("npm")).unwrap();

        remove_orphans(&dir, &HashSet::new());
        remove_flat_layout(&flat);

        assert!(!dir.join(DELETED).exists());
        assert!(std::fs::symlink_metadata(dir.join(swapped)).is_err());
        assert!(
            target.join("registry").is_dir(),
            "the link's target was removed"
        );
        // A flat-layout name that is a link is not that layout's directory.
        assert!(std::fs::symlink_metadata(flat.join("npm")).is_ok());
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

        remove_project(&dir, DELETED);
        remove_project(&dir, "6e5d4c3b-2a19-4f8e-9d7c-6b5a4f3e2d1c");
        remove_project(&dir, ".");

        assert!(!dir.join(DELETED).exists());
        assert!(dir.join(LIVE).join("npm").is_dir());
    }

    /// The flat layout's names come from the table, so a row added later is covered
    /// without anyone retyping the list.
    #[test]
    fn flat_layout_names_cover_every_table_row_and_cargo() {
        let names: HashSet<_> = flat_layout_names().collect();
        for name in ["bun", "npm", "pnpm", "go", "poetry", "playwright", "cargo"] {
            assert!(names.contains(name), "{name} missing");
        }
        assert!(names.iter().all(|n| !n.contains('/') && !n.is_empty()));
    }
}
