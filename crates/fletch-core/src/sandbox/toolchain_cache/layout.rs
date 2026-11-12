//! Where a project's cache is: `<base>/toolchain-cache/<project id>` in
//! release and `<base>/dev/toolchain-cache/<project id>` in debug, the layout
//! of the checkouts root ([`crate::workspace::checkouts_root`]).
//!
//! **Why per build.** Like every root holding state keyed by a database's ids
//! ([`crate::build_state_subpath`]): a debug instance and a release install
//! mint project ids from separate databases, so they never shared a warm cache
//! anyway, and from one directory each one's reclaim would read the other's
//! live projects as deleted ones.
//!
//! **Who moves the base.** It is `~/.fletch` unless [`TOOLCHAIN_CACHE_ROOT_ENV`]
//! overrides it, and the build segment is appended to an overridden base too,
//! so neither path can bypass the split. Two callers set it, the way the
//! checkout and RPC roots' overrides are set. A `fletch-host` points it at its
//! data dir: its database is not the desktop's, so one shared directory would
//! have each one's reclaim delete the other's caches. And the Run panel points
//! a nested Fletch (dogfooding) at a base inside the outer project's own root:
//! the nested instance mints its own project ids, and the Run profile grants
//! only the outer project's root, so it could not create a sibling. Inside
//! that root it can write, and it is still the outer project's trust domain,
//! since the nested instance *is* that project's Run process.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

pub const TOOLCHAIN_CACHE_ROOT_ENV: &str = "FLETCH_TOOLCHAIN_CACHE_ROOT";

/// This build's directory of project roots. Never granted to a sandbox: it
/// holds every project's root.
pub fn toolchain_cache_dir(home: &Path) -> PathBuf {
    toolchain_cache_dir_in(&base_override().unwrap_or_else(|| home.join(".fletch")))
}

pub(super) fn toolchain_cache_dir_in(base: &Path) -> PathBuf {
    base.join(crate::build_state_subpath("toolchain-cache"))
}

fn base_override() -> Option<PathBuf> {
    std::env::var_os(TOOLCHAIN_CACHE_ROOT_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// One project's root: the only part of the cache a launch of that project is
/// granted or mounted.
pub fn toolchain_cache_root(home: &Path, project_id: &str) -> Result<PathBuf> {
    project_root_in(&toolchain_cache_dir(home), project_id)
}

/// A project id is a UUID (`workspace::query`), so it is path-safe by
/// construction; anything that is not one plain component is refused here,
/// where it enters a path, so a bad id fails the launch instead of widening the
/// grant (`""` or `"."` would name the directory of every project's root,
/// `"../x"` a sibling).
pub(super) fn project_root_in(dir: &Path, project_id: &str) -> Result<PathBuf> {
    let mut parts = Path::new(project_id).components();
    match (parts.next(), parts.next()) {
        (Some(std::path::Component::Normal(_)), None) if !project_id.contains('/') => {
            Ok(dir.join(project_id))
        }
        _ => Err(Error::InvalidPath(format!(
            "project id {project_id:?} cannot key a toolchain cache root"
        ))),
    }
}

/// Where toolchains cached before roots were keyed by project: directly under
/// `~/.fletch/toolchain-cache`. An overridden base never held that layout.
pub(super) fn flat_layout_dir(home: &Path) -> Option<PathBuf> {
    base_override()
        .is_none()
        .then(|| home.join(".fletch").join("toolchain-cache"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The layout, pinned once: the build's segment then `toolchain-cache`
    /// under the base, as checkouts are, and each project one component below.
    /// An id that named the directory itself (`""`, `"."`) or escaped it would
    /// hand a profile every other project's cache, so those fail rather than
    /// resolve.
    #[test]
    fn a_project_root_is_one_component_under_this_builds_dir() {
        let base = Path::new("/Users/u/.fletch");
        let dir = toolchain_cache_dir_in(base);
        let expected = if cfg!(debug_assertions) {
            "/Users/u/.fletch/dev/toolchain-cache"
        } else {
            "/Users/u/.fletch/toolchain-cache"
        };
        assert_eq!(dir, Path::new(expected));

        let id = "0b9f3c1e-6a4d-4e8b-9c2f-1d7e5a3b8c90";
        assert_eq!(project_root_in(&dir, id).unwrap(), dir.join(id));
        for bad in ["", ".", "..", "../other", "a/b", "a/", "/abs", "a/../b"] {
            assert!(
                project_root_in(&dir, bad).is_err(),
                "{bad:?} must not key a cache root"
            );
        }
    }

    /// A nested Fletch's base sits inside the outer project's root (the Run
    /// panel's `<root>/nested`), and the inner instance appends its own build
    /// segment there, so every inner root stays inside the outer grant.
    #[test]
    fn a_nested_instance_caches_inside_the_outer_projects_root() {
        let outer = project_root_in(
            &toolchain_cache_dir_in(Path::new("/Users/u/.fletch")),
            "outer",
        )
        .unwrap();
        let inner =
            project_root_in(&toolchain_cache_dir_in(&outer.join("nested")), "inner").unwrap();
        assert_eq!(
            inner,
            outer
                .join("nested")
                .join(crate::build_state_subpath("toolchain-cache"))
                .join("inner")
        );
    }
}
