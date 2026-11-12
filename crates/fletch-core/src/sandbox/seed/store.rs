use std::io;
use std::path::Path;

use fletch_box::disks;
use serde::{Deserialize, Serialize};

use super::request::Request;
use super::{read_json, STORE};
use crate::project_file::SeedSettings;

pub(super) const SEED: &str = "seed";
pub(super) const CANDIDATE_PREFIX: &str = "candidate-";
pub(super) const MADE: &str = "made.json";

/// How a seed was made, kept beside what it holds.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct Made {
    /// The paths the project excluded then.
    pub(super) without: Vec<String>,
    /// The request it answers; none for a project's first seed.
    pub(super) for_request: Option<Request>,
}

/// Starts a workspace of `project_id` from the project's seed in `store`,
/// while its settings allow a workspace to start from one and the engine
/// can (`usable`): `start` is given the seed, under the store's lock, and
/// answers whether it started from it.
pub fn start_from(
    store: &Path,
    project_id: &str,
    usable: impl FnOnce(&Path) -> bool,
    start: impl FnOnce(&Path) -> bool,
) -> bool {
    let Some(settings) = crate::project_file::seed_settings(project_id) else {
        return false;
    };
    let _store = STORE.lock();
    seed_in_force(store, &settings, usable).is_some() && start(&store.join(SEED))
}

/// How the project's seed in `store` was made, while `settings` allow a
/// workspace to start from it and the engine can (`usable`): seeds are on,
/// and it was made without every path they exclude now. A seed either
/// refuses is removed. The caller holds [`STORE`].
pub(super) fn seed_in_force(
    store: &Path,
    settings: &SeedSettings,
    usable: impl FnOnce(&Path) -> bool,
) -> Option<Made> {
    let seed = store.join(SEED);
    if !seed.exists() {
        return None;
    }
    let made: Made = read_json(&seed.join(MADE));
    let allowed = settings.enabled
        && settings
            .exclude
            .iter()
            .all(|path| made.without.contains(path));
    if allowed && usable(&seed) {
        return Some(made);
    }
    if let Err(e) = std::fs::remove_dir_all(&seed) {
        tracing::warn!(seed = %seed.display(), error = %e, "could not remove a seed that may no longer be used");
    }
    None
}

/// Makes `candidate` the project's seed in `store`, with how it was `made`
/// beside what it holds, once `publishing` has run. The seed there was is
/// exchanged for it in one step, so the project is never without one.
pub(super) fn publish(
    store: &Path,
    candidate: &Path,
    made: &Made,
    publishing: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let _store = STORE.lock();
    publishing()?;
    std::fs::write(candidate.join(MADE), serde_json::to_vec(made)?)?;
    let seed = store.join(SEED);
    if !seed.exists() {
        return std::fs::rename(candidate, seed);
    }
    disks::exchange(candidate, &seed)?;
    let previous_seed = candidate;
    let _ = std::fs::remove_dir_all(previous_seed);
    Ok(())
}

/// Removes the candidates an earlier run of the app left in the stores under
/// `stores_dir`, one per project: a candidate lives only as long as the task
/// that settles it.
pub fn remove_abandoned_candidates(stores_dir: &Path) {
    let projects = std::fs::read_dir(stores_dir).ok();
    let entries = projects
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|project| std::fs::read_dir(project.path()).ok())
        .flatten()
        .flatten();
    for entry in entries {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(CANDIDATE_PREFIX)
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}
