//! The toolchain cache: one Fletch-owned directory per project that every
//! sandboxed toolchain is redirected into, on every engine.
//!
//! **Why redirect.** A seatbelt agent runs the host's toolchains, and those
//! write caches and stores outside the checkout: bun to `~/.bun/install/cache`,
//! cargo to `~/.cargo/registry`, go to `~/go/pkg/mod`. Denied, they fail closed,
//! or (observed in the wild) an agent "helpfully" redirects the cache into the
//! checkout, where it becomes thousands of untracked files the fleet-wide git
//! polls walk on every tick. Granting each default location instead never
//! terminates (every new toolchain is a new grant), and most of them are unsafe
//! to grant whole: `~/.cargo`, `~/go`, `~/.bun` and `~/.gem` hold PATH-resolved
//! `bin` dirs, so each would need its own analysis against policy invariant 1.
//! Redirecting inverts both problems. The sandbox grants exactly one path, and
//! adding a toolchain is a row in [`vars`] with no policy change; invariant 1
//! stops applying rather than needing to be checked, since Fletch owns this
//! layout and it is never on the user's PATH. A container binds the same root
//! at its host path, so the engines share one warm cache.
//!
//! **Why per project.** Everything under a root is executed by sandboxed
//! processes (cargo compiles `registry/src`, pnpm hardlinks its store into
//! every checkout, `npx` runs what `_npx/` holds, Playwright launches the
//! browser it downloaded), so every writer of a root can plant what those run.
//! A root shared across projects let an agent of project A plant code that
//! project B's Run command then executed under the looser Run profile, with
//! B's shared `.env`. Keyed by project, the trust domain, no other project can
//! write what this one executes: every profile and container mount names the
//! project's own root, never [`toolchain_cache_dir`]. Within a project the
//! agents and the Run panel share the root on purpose: one warm cache, and
//! pnpm's store still dedupes across the project's many workspaces. That
//! leaves an agent able to plant what its own project's Run executes, which is
//! the documented agent-to-Run trade-off (an agent already edits the
//! `package.json` scripts Run runs; see `seatbelt::RUN_TOOLCHAIN_DIRS`), not a
//! new one. The host never runs anything from here.
//!
//! **Why per build.** Like every root holding state keyed by a database's ids
//! ([`crate::build_state_subpath`]): a debug instance and a release install
//! mint project ids from separate databases, so they never shared a warm cache
//! anyway, and from one directory each one's reclaim would read the other's
//! live projects as deleted ones. The layout is the checkouts root's
//! ([`crate::workspace::checkouts_root`]): `<base>/toolchain-cache/<project id>`
//! in release and `<base>/dev/toolchain-cache/<project id>` in debug, with the
//! base `~/.fletch` unless [`TOOLCHAIN_CACHE_ROOT_ENV`] overrides it. The
//! segment is appended to an overridden base too, so neither path can bypass
//! the split.
//!
//! **What is platform-keyed.** A macOS agent and a Linux container of one
//! project share the root, so the rows whose contents differ per platform land
//! in a [`BoxPlatform::key`] subdir ([`vars`] says which and why), and so does
//! the cargo overlay ([`super::cargo_home`]).
//!
//! **How it is reclaimed** ([`gc`]): at boot, every root whose project the
//! database no longer holds, plus the flat layout from before the per-project
//! key; when a project is deleted, that project's root.
//!
//! **Who moves the base.** [`TOOLCHAIN_CACHE_ROOT_ENV`] is set the way the
//! checkout and RPC roots' overrides are, by two callers. A `fletch-host`
//! points it at its data dir: its database is not the desktop's, so one shared
//! directory would have each one's reclaim delete the other's caches. And the
//! Run panel points a nested Fletch (dogfooding) at a base inside the outer
//! project's own root: the nested instance mints its own project ids, and the
//! Run profile grants only the outer project's root, so it could not create a
//! sibling. Inside that root it can write, and it is still the outer project's
//! trust domain, since the nested instance *is* that project's Run process.

mod gc;
mod vars;

use std::path::{Path, PathBuf};

use super::cargo_home;
use crate::error::{Error, Result};

pub use gc::{reclaim_toolchain_caches, remove_project_cache};
pub use vars::{toolchain_cache_env, BoxPlatform};

/// Env var overriding the base the toolchain cache dir hangs off (default
/// `~/.fletch`), the twin of [`crate::workspace::WORKSPACES_ROOT_ENV`]. Who sets
/// it and why: the module doc.
pub const TOOLCHAIN_CACHE_ROOT_ENV: &str = "FLETCH_TOOLCHAIN_CACHE_ROOT";

/// This build's directory of project roots. Never granted to a sandbox: it
/// holds every project's root.
pub fn toolchain_cache_dir(home: &Path) -> PathBuf {
    toolchain_cache_dir_in(&base_override().unwrap_or_else(|| home.join(".fletch")))
}

/// [`toolchain_cache_dir`] under an explicit base, so the layout is testable
/// without the process environment.
fn toolchain_cache_dir_in(base: &Path) -> PathBuf {
    base.join(crate::build_state_subpath("toolchain-cache"))
}

/// [`TOOLCHAIN_CACHE_ROOT_ENV`], when set non-empty.
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
fn project_root_in(dir: &Path, project_id: &str) -> Result<PathBuf> {
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

/// A project's prepared cache: the root to grant or mount, and the env
/// pointing the child's toolchains into it.
#[derive(Debug)]
pub struct ToolchainCache {
    pub root: PathBuf,
    pub env: Vec<(String, String)>,
}

/// Everything a confined child needs to use its project's cache: the root
/// created host-side, the cargo overlay prepared, and the env pointing every
/// toolchain at them. Every launch calls this once and grants or mounts the
/// returned root, so the env and the grant can't name two different paths.
/// `cwd` is where the child's commands run (the agent's checkout, the Run
/// target); `platform` is what it runs on, [`BoxPlatform::host`] for seatbelt
/// and [`BoxPlatform::linux`] for a container.
///
/// Creating the root is part of the launch, on every engine: a seatbelt grant
/// of `(subpath <root>)` lets a tool create the root but not its parent, and a
/// missing bind source would be materialized root-owned, locking the user's
/// own seatbelt agents out of it. It is Fletch's own directory, so failing to
/// create it fails the launch.
///
/// Cargo is the best-effort part. It is redirected only when its global config
/// stays visible from `cwd` and its overlay is in place
/// ([`cargo_home::config_reachable`]); otherwise the child gets cargo's *real*
/// home as `CARGO_HOME`, set explicitly rather than left to inheritance: a
/// nested Fletch's own process env carries the overlay it inherited, and a
/// child that merely inherited that would be redirected after all. Cargo then
/// reads its real home exactly as on the host, and its writes fail closed.
pub fn prepare_toolchain_cache(
    home: &Path,
    project_id: &str,
    cwd: &Path,
    platform: BoxPlatform,
) -> Result<ToolchainCache> {
    let root = toolchain_cache_root(home, project_id)?;
    std::fs::create_dir_all(&root).map_err(|e| {
        Error::Other(format!(
            "creating the toolchain cache {}: {e}",
            root.display()
        ))
    })?;
    let mut env = toolchain_cache_env(&root, platform);
    let user_home = cargo_home::user_home(home);
    let real_home = (
        "CARGO_HOME".to_string(),
        user_home.to_string_lossy().into_owned(),
    );
    if !cargo_home::config_reachable(&user_home, cwd) {
        tracing::info!(
            cwd = %cwd.display(),
            cargo_home = %user_home.display(),
            "cargo keeps its real home: its global config would not be found from this directory"
        );
        env.push(real_home);
    } else {
        match cargo_home::prepare(&root, &user_home, platform) {
            Ok(()) => env.extend(cargo_home::env(&root, &user_home, platform)),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "could not prepare the sandboxed cargo home; cargo writes will fail closed"
                );
                env.push(real_home);
            }
        }
    }
    Ok(ToolchainCache { root, env })
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
