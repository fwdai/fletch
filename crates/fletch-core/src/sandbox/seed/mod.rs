//! A project's seed: what a new workspace of the project starts with, so it
//! has the project's dependencies and downloads installed and shares them
//! with every other workspace until it changes them. The rules are here; an
//! engine keeps its seeds in a store of its own per project, takes and
//! reduces a workspace at rest ([`AtRest`]) and starts a new workspace from
//! the seed ([`start_from`]).
//!
//! ```text
//! <store>/seed/              what the engine kept of a workspace
//! <store>/seed/made.json     what it was made without, and for which request
//! <store>/candidate-<id>/    what the engine took of a workspace, to be reduced
//! <request file>             this workspace is to be the next seed
//! ```
//!
//! A project's first seed is made of the first workspace found at rest with
//! something worth inheriting, so its workspaces share from as early as they
//! can. After that, what a workspace installed matters to the others once its
//! work is in the base: when its pull request merges into the repository's
//! default branch, it is wanted as the seed ([`want`]) and replaces it the
//! next time it is at rest. Both go through [`offer`]: what the engine takes
//! of the workspace is a candidate, which the engine reduces in its turn to
//! what a seed keeps, with nothing else of that workspace left. Only a
//! candidate the engine finished reducing is published, by an exchange with
//! the seed there was, so a project that has a seed is never without one.
//!
//! The project's settings file says what a seed may hold
//! ([`crate::project_file::SeedSettings`]): paths never kept, or no seed at
//! all. They are read when they are used, never kept from an earlier look; a
//! seed made before a path was excluded is removed, not started from; and a
//! project whose file was not read yet neither makes a seed nor starts from
//! one.

mod candidate;
mod request;
mod store;
#[cfg(all(test, target_os = "macos"))]
mod tests;

use std::future::Future;
use std::io;
use std::path::Path;

use parking_lot::Mutex;

use crate::error::Result;

pub use candidate::offer;
pub use request::{is_wanted, want};
pub use store::{remove_abandoned_candidates, start_from};

/// A directory, or a whole workspace, that keeps less than this has nothing
/// worth inheriting.
pub const WORTH_INHERITING_KIB: u64 = 1024;

/// Serializes every read and write of a project's seed and of a workspace's
/// request; each is a few file operations.
static STORE: Mutex<()> = Mutex::new(());

/// A workspace at rest, as one engine sees it: what a seed is made of.
pub trait AtRest: Send + Sync + 'static {
    fn agent_id(&self) -> &str;
    fn project_id(&self) -> &str;
    /// This engine's store for the workspace's project.
    fn store(&self) -> &Path;
    /// Where the workspace's request is kept.
    fn request_file(&self) -> &Path;
    /// Whether the workspace may be its project's first seed.
    fn may_be_first(&self) -> bool;
    /// Whether this engine could start a workspace now from what `seed`
    /// holds: the project's seed, or a candidate. A seed it could not is
    /// removed; a candidate it could not is never made the seed.
    fn usable(&self, seed: &Path) -> bool;
    /// Takes what a seed is made of into `candidate`, a directory not made
    /// yet. Runs while the workspace is at rest, before [`offer`] returns.
    fn take(&self, candidate: &Path) -> io::Result<()>;
    /// Makes `candidate` hold what a seed keeps, with nothing at the
    /// `excluded` paths, and answers the KiB it kept. Runs in the
    /// candidate's turn.
    fn reduce(
        &self,
        candidate: &Path,
        excluded: &[String],
    ) -> impl Future<Output = Result<u64>> + Send;
    /// The workspace kept nothing worth inheriting.
    fn kept_nothing(&self);
    /// Runs just before the candidate becomes the seed, under the store's
    /// lock. An error leaves the seed there was.
    fn publishing(&self) -> io::Result<()>;
}

/// What the JSON file at `path` holds; the default when there is none.
pub fn read_json<T: serde::de::DeserializeOwned + Default>(path: &Path) -> T {
    std::fs::read(path)
        .ok()
        .and_then(|json| serde_json::from_slice(&json).ok())
        .unwrap_or_default()
}
