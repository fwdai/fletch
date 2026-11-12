//! The toolchain cache: one Fletch-owned directory per project that every
//! sandboxed toolchain is redirected into, on every engine.
//!
//! A launch gets a [`ToolchainCache`] for where its child runs
//! ([`ToolchainCache::on_host`], [`ToolchainCache::in_guest`]), makes its
//! `root` writable for the child and adds its `env` to the child's. A deleted
//! project's caches go with it and boot sweeps what a deletion missed
//! ([`remove_project_cache`], [`reclaim_toolchain_caches`]); both cover every
//! engine, each of which says where it keeps a store per project on this
//! machine ([`stores_on_host`]). The layout and who moves it: [`layout`].
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
//! layout and it is never on the user's PATH.
//!
//! **Why per project.** Everything under a root is executed by sandboxed
//! processes (cargo compiles `registry/src`, pnpm hardlinks its store into
//! every checkout, `npx` runs what `_npx/` holds, Playwright launches the
//! browser it downloaded), so every writer of a root can plant what those run.
//! A root shared across projects let an agent of project A plant code that
//! project B's Run command then executed under the looser Run profile, with
//! B's shared `.env`. Keyed by project, the trust domain, no other project can
//! write what this one executes: every profile names the
//! project's own root, never [`toolchain_cache_dir`]. Within a project the
//! agents and the Run panel share the root on purpose: one warm cache, and
//! pnpm's store still dedupes across the project's many workspaces. That
//! leaves an agent able to plant what its own project's Run executes, which is
//! the documented agent-to-Run trade-off (an agent already edits the
//! `package.json` scripts Run runs; see `seatbelt::RUN_TOOLCHAIN_DIRS`), not a
//! new one. The host never runs anything from here.
//!
//! **What is platform-keyed.** A macOS agent and a Linux guest of one
//! project share the root, so the rows whose contents differ per platform land
//! in a [`Platform::key`] subdir ([`vars`] says which and why), and so does
//! the cargo overlay ([`super::cargo_home`]).

mod gc;
mod layout;
mod vars;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::cargo_home;
use super::engine::EngineKind;
use crate::error::{Error, Result};

pub(in crate::sandbox) use layout::project_root_in;
pub use layout::{toolchain_cache_dir, toolchain_cache_root, TOOLCHAIN_CACHE_ROOT_ENV};
pub use vars::Platform;

/// A project's cache as one launch uses it: the directory the child may
/// write and the env pointing the child's toolchains into it. One value
/// carries both, so the grant and the env can't name two different paths.
#[derive(Debug)]
pub struct ToolchainCache {
    pub root: PathBuf,
    pub env: Vec<(String, String)>,
}

impl ToolchainCache {
    /// For a child on this machine working in `cwd`: a seatbelt agent or a Run
    /// command. Creates the root, since a sandbox granted the root alone
    /// cannot create the directories above it.
    pub fn on_host(home: &Path, project_id: &str, cwd: &Path) -> Result<Self> {
        let root = toolchain_cache_root(home, project_id)?;
        std::fs::create_dir_all(&root).map_err(|e| {
            Error::Other(format!(
                "creating the toolchain cache {}: {e}",
                root.display()
            ))
        })?;
        let mut env = vars::toolchain_cache_env(&root, Platform::host());
        env.extend(cargo_env_on_host(&root, home, cwd));
        Ok(Self { root, env })
    }

    /// For a child in a box. A guest sees the host's paths as its own, so the
    /// root is the same path on the box's own disk, which starts as a clone
    /// of the project's seed (`boxes::seed`), and the box creates it.
    /// Cargo keeps its default home there: a guest has no host cargo home to
    /// overlay.
    pub fn in_guest(home: &Path, project_id: &str) -> Result<Self> {
        let root = toolchain_cache_root(home, project_id)?;
        let env = vars::toolchain_cache_env(&root, Platform::linux());
        Ok(Self { root, env })
    }
}

/// `CARGO_HOME` and what goes with it for a child on this machine: the
/// overlay under `root` ([`cargo_home`]) when cargo's global config stays
/// visible from `cwd` and the overlay is in place, else cargo's real home,
/// which it reads as on the host and where its writes fail closed. Set
/// explicitly either way: a nested Fletch's own env carries the overlay it
/// inherited, and a child that merely inherited that would be redirected
/// after all.
fn cargo_env_on_host(root: &Path, home: &Path, cwd: &Path) -> Vec<(String, String)> {
    let user_home = cargo_home::user_home(home);
    let real_home = vec![(
        "CARGO_HOME".to_string(),
        user_home.to_string_lossy().into_owned(),
    )];
    if !cargo_home::config_reachable(&user_home, cwd) {
        tracing::info!(
            cwd = %cwd.display(),
            cargo_home = %user_home.display(),
            "cargo keeps its real home: its global config would not be found from this directory"
        );
        return real_home;
    }
    match cargo_home::prepare(root, &user_home, Platform::host()) {
        Ok(()) => cargo_home::env(root, &user_home, Platform::host()).into(),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "could not prepare the sandboxed cargo home; cargo writes will fail closed"
            );
            real_home
        }
    }
}

/// Deletes a deleted project's cache on every engine. Seconds of blocking I/O
/// for a registry or a browser, so the caller runs it off the async runtime
/// and outside its lifecycle locks. A bad id is logged and removes nothing.
pub fn remove_project_cache(home: &Path, project_id: &str) {
    for dir in cache_dirs_on_host(home) {
        gc::remove_project(&dir, project_id);
    }
}

/// Deletes, on every engine, the cache of each project not in
/// `live_project_ids`, and what the layout before per-project roots left.
/// Boot calls it, only for an instance that owns its state roots
/// (`host::StateRoots`): a nested instance's database knows nothing about the
/// caches beside its own.
pub fn reclaim_toolchain_caches(home: &Path, live_project_ids: &HashSet<String>) {
    for dir in cache_dirs_on_host(home) {
        gc::remove_orphans(&dir, live_project_ids);
    }
    if let Some(dir) = layout::flat_layout_dir(home) {
        gc::remove_flat_layout(&dir);
    }
}

fn cache_dirs_on_host(home: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    EngineKind::ALL
        .iter()
        .flat_map(move |&engine| stores_on_host(engine, home))
}

/// The directories on this machine where `engine` keeps one store per
/// project, named by project id: the directories seatbelt's toolchains write
/// and the seeds its workspaces start from; the seeds a project's boxes start
/// from, which carry its cache.
fn stores_on_host(engine: EngineKind, home: &Path) -> Vec<PathBuf> {
    match engine {
        EngineKind::SandboxExec => std::iter::once(toolchain_cache_dir(home))
            .chain(super::seeds_dir(engine))
            .collect(),
        EngineKind::Box => super::seeds_dir(engine).into_iter().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cleanup looks in the directory host launches cache in, the one holding
    /// every root [`ToolchainCache::on_host`] hands out, and in the one their
    /// workspaces' seeds are kept in.
    #[test]
    fn cleanup_covers_the_roots_host_launches_use() {
        let home = Path::new("/Users/u");
        let root = toolchain_cache_root(home, "0b9f3c1e-6a4d-4e8b-9c2f-1d7e5a3b8c90").unwrap();
        let stores = stores_on_host(EngineKind::SandboxExec, home);
        assert!(stores
            .iter()
            .any(|dir| Some(dir.as_path()) == root.parent()));
        assert!(stores.contains(&crate::sandbox::seatbelt::seed::seeds_dir().unwrap()));
    }
}
