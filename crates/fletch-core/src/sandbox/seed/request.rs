use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{read_json, STORE};

/// A request for a workspace as its project's seed. Ordered by when it was
/// made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(super) struct Request(uuid::Uuid);

/// Asks that the workspace whose request is kept at `request_file` become its
/// project's seed. [`super::offer`] answers it once the workspace is at rest,
/// and the request stands until a candidate taken after it was made has been
/// looked at.
pub fn want(request_file: &Path) -> io::Result<()> {
    let _store = STORE.lock();
    let request = Request(uuid::Uuid::now_v7());
    std::fs::write(request_file, serde_json::to_vec(&request)?)
}

pub fn is_wanted(request_file: &Path) -> bool {
    standing_request(request_file).is_some()
}

pub(super) fn standing_request(request_file: &Path) -> Option<Request> {
    read_json(request_file)
}

/// Withdraws the request `answered`, unless a newer one has replaced it.
pub(super) fn withdraw(request_file: &Path, answered: Request) {
    let _store = STORE.lock();
    if standing_request(request_file) == Some(answered) {
        let _ = std::fs::remove_file(request_file);
    }
}
