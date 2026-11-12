use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{read_json, STORE};

const WANTED: &str = "wanted";

/// A request for a workspace as its project's seed. Ordered by when it was
/// made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(super) struct Request(uuid::Uuid);

fn request_file(store: &Path, agent_id: &str) -> PathBuf {
    store.join(WANTED).join(agent_id)
}

/// Asks that `agent_id`'s workspace become the seed of its project, whose
/// store is `store`. [`super::offer`] answers it once the workspace is at
/// rest, and the request stands until a candidate taken after it was made
/// has been looked at.
pub fn want(store: &Path, agent_id: &str) -> io::Result<()> {
    let _store = STORE.lock();
    let request = Request(uuid::Uuid::now_v7());
    std::fs::create_dir_all(store.join(WANTED))?;
    std::fs::write(request_file(store, agent_id), serde_json::to_vec(&request)?)
}

pub fn is_wanted(store: &Path, agent_id: &str) -> bool {
    standing_request(store, agent_id).is_some()
}

pub(super) fn standing_request(store: &Path, agent_id: &str) -> Option<Request> {
    read_json(&request_file(store, agent_id))
}

/// Withdraws the request `answered`, unless a newer one has replaced it.
pub(super) fn withdraw(store: &Path, agent_id: &str, answered: Request) {
    let _store = STORE.lock();
    if standing_request(store, agent_id) == Some(answered) {
        let _ = std::fs::remove_file(request_file(store, agent_id));
    }
}

/// The workspaces still wanted as a seed in the stores under `stores_dir`,
/// each with its project's id and its agent's.
pub fn wanted(stores_dir: &Path) -> Vec<(String, String)> {
    let name = |entry: &std::fs::DirEntry| entry.file_name().to_string_lossy().into_owned();
    let stores = std::fs::read_dir(stores_dir).ok();
    stores
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|store| {
            let requests = std::fs::read_dir(store.path().join(WANTED)).ok();
            let project_id = name(&store);
            requests
                .into_iter()
                .flatten()
                .flatten()
                .map(move |request| (project_id.clone(), name(&request)))
        })
        .collect()
}

/// Forgets the request of `agent_id`, whose workspace goes, in whichever
/// store under `stores_dir` holds it.
pub fn forget(stores_dir: &Path, agent_id: &str) {
    let _store = STORE.lock();
    let stores = std::fs::read_dir(stores_dir).ok();
    for store in stores.into_iter().flatten().flatten() {
        let _ = std::fs::remove_file(request_file(&store.path(), agent_id));
    }
}
