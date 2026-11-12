use std::io;
use std::path::{Path, PathBuf};

use fletch_box::disks;

use super::{store, TREE};
use crate::sandbox::seed;
use crate::workspace::AgentRecord;

/// Starts `checkout`, a checkout of `record`'s workspace just made, from its
/// project's seed ([`seed::start_from`]), then cleans away what its base does
/// not ignore, as a box's provisioning does. A checkout that cannot start from
/// the seed, or cannot be cleaned after, stays as it was made.
pub async fn start(record: &AgentRecord, checkout: &Path) {
    if let Some(store) = store(&record.project_id) {
        start_checkout(store, record.project_id.clone(), checkout).await;
    }
}

/// [`start`] for a checkout of `project_id`, whose store is `store`.
pub(super) async fn start_checkout(store: PathBuf, project_id: String, checkout: &Path) {
    let into = checkout.to_path_buf();
    let planting = tokio::task::spawn_blocking(move || {
        let mut planted = Vec::new();
        seed::start_from(
            &store,
            &project_id,
            |_| true,
            |seed| {
                planted = plant(seed, &into);
                !planted.is_empty()
            },
        );
        planted
    });
    let planted = planting.await.unwrap_or_default();
    if planted.is_empty() {
        return;
    }
    if let Err(e) = crate::git::run_git(checkout, &["clean", "-ffdq"], "clean").await {
        tracing::warn!(checkout = %checkout.display(), error = %e, "could not clean what a checkout's seed left");
        unplant(&planted);
    }
}

/// Merges into `checkout` what `seed` keeps for a checkout of its name
/// ([`merge`]), and answers what it cloned. After a failure nothing cloned
/// is left.
fn plant(seed: &Path, checkout: &Path) -> Vec<PathBuf> {
    let mut planted = Vec::new();
    if let Err(e) = merge_seed(seed, checkout, &mut planted) {
        tracing::warn!(checkout = %checkout.display(), error = %e, "could not start a checkout from its project's seed");
        unplant(&planted);
        planted.clear();
    }
    planted
}

fn unplant(planted: &[PathBuf]) {
    for path in planted {
        let _ = super::remove(path);
    }
}

fn merge_seed(seed: &Path, checkout: &Path, planted: &mut Vec<PathBuf>) -> io::Result<()> {
    let Some(name) = checkout.file_name() else {
        return Ok(());
    };
    let kept = seed.join(TREE).join(name);
    if !kept.is_dir() {
        return Ok(());
    }
    merge(&kept.canonicalize()?, &checkout.canonicalize()?, planted)
}

/// Clones each entry of `from` that `into` lacks, whole, and merges each
/// directory both hold; anything else in `into` stays. What it clones is
/// pushed to `planted`.
fn merge(from: &Path, into: &Path, planted: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let (from, into) = (entry.path(), into.join(entry.file_name()));
        match std::fs::symlink_metadata(&into) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                planted.push(into.clone());
                disks::clone_tree_sharing_blocks(&from, &into)?;
            }
            Ok(there) if there.is_dir() && entry.file_type()?.is_dir() => {
                merge(&from, &into, planted)?;
            }
            Ok(_) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
