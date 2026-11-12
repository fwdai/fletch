//! A project's seed: the data disk a new box of the project starts with, so a
//! new workspace has the project's dependencies and downloads installed and
//! shares their blocks with every other workspace until it changes them.
//!
//! ```text
//! <project seeds dir>/<project id>/seed.img             a workspace disk, reduced
//! <project seeds dir>/<project id>/seed-built-at.json   the workspaces whose files it holds
//! <box dir>/seed-wanted                                  this box's disk is to be the next seed
//! ```
//!
//! A project's first seed is made of the first box to stop cleanly with
//! something worth inheriting, so its workspaces share from as early as they
//! can. After that, what a workspace installed matters to the others once its
//! work is in the base: when its pull request merges into the repository's
//! default branch, its disk is wanted as the seed ([`want`]) and replaces it
//! the next time its box is down after a clean stop. Both go through
//! [`offer`]: a clone of the disk is reduced on a workbench by `seed.sh` to
//! the directories git ignores in each checkout and the project's toolchain
//! cache, with nothing else of that workspace left; a disk the script did
//! not finish on is discarded.
//!
//! A new box's data disk is a clone of the seed ([`new_data_disk`]), and
//! provisioning checks the base out over what the seed left (`provision`).
//! What a seed holds was built at the path of the workspace it came from, and
//! some of it says so (a virtualenv's scripts): provisioning links those
//! paths to the box's own root, so they still resolve ([`link_built_at`]).

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fletch_box::workbench::Workbench;
use fletch_box::{disks, Channel, StopOutcome};
use parking_lot::Mutex;

use super::registry::{BoxIdentity, DATA_DISK};
use crate::error::{Error, Result};
use crate::sandbox::ToolchainCache;

const SEED_DISK: &str = "seed.img";
const SEED_BUILT_AT: &str = "seed-built-at.json";
const BOX_WANTED: &str = "seed-wanted";
const REDUCE: &str = include_str!("seed.sh");

/// A disk that keeps less than this had nothing installed worth inheriting.
const WORTH_INHERITING_KIB: u64 = 1024;
const REDUCE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// A box whose disk had nothing worth inheriting is asked again for its
/// project's first seed once the disk holds this many times as much.
const ASKED_AGAIN_FOR_A_DISK_TIMES: f64 = 1.5;

/// Serializes every read and write of a project's seed; each is a few file
/// operations.
static STORE: Mutex<()> = Mutex::new(());
/// The projects whose seed is being made, one at a time each.
static BEING_MADE: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
/// The boxes whose disk had nothing worth inheriting, by agent id, with the
/// KiB the disk held then. Forgotten when the app restarts.
static HAD_NOTHING: Mutex<Vec<(String, u64)>> = Mutex::new(Vec::new());

/// Where every project's seed is kept, one directory per project id. Beside
/// the boxes, so a clone never crosses a volume.
pub fn project_seeds_dir() -> Option<PathBuf> {
    Some(super::boxes_root().ok()?.with_file_name("box-seeds"))
}

/// `None` for a project id that cannot name a directory.
fn project_dir(identity: &BoxIdentity) -> Option<PathBuf> {
    crate::sandbox::toolchain_cache_root(&identity.home, &identity.project_id).ok()?;
    Some(project_seeds_dir()?.join(&identity.project_id))
}

/// The writable roots of the workspaces whose files the project's seed
/// holds, the newest first. Every one the project ever had: a file kept
/// through any number of seeds still names the path it was built at.
fn built_at(project: &Path) -> Vec<PathBuf> {
    std::fs::read(project.join(SEED_BUILT_AT))
        .ok()
        .and_then(|json| serde_json::from_slice(&json).ok())
        .unwrap_or_default()
}

/// Creates the data disk of a new box: a clone of its project's seed when
/// there is one of `bytes`, an empty disk otherwise.
pub fn new_data_disk(identity: &BoxIdentity, data: &Path, bytes: u64) -> io::Result<()> {
    let cloned = project_dir(identity).is_some_and(|project| clone_seed(&project, data, bytes));
    if cloned {
        Ok(())
    } else {
        disks::create_sparse(data, bytes)
    }
}

fn clone_seed(project: &Path, data: &Path, bytes: u64) -> bool {
    let _store = STORE.lock();
    let seed = project.join(SEED_DISK);
    std::fs::metadata(&seed).is_ok_and(|seed| seed.len() == bytes)
        && disks::clone_sharing_blocks(&seed, data).is_ok()
}

/// Links each path the project's seed was built at to `identity`'s own root,
/// in its box, where nothing is at that path yet.
pub async fn link_built_at(channel: &Channel, identity: &BoxIdentity) {
    let built_at = project_dir(identity).map_or_else(Vec::new, |project| built_at(&project));
    let links: Vec<String> = built_at
        .iter()
        .filter(|at| **at != identity.writable_root && at.starts_with(&identity.home))
        .map(|at| at.to_string_lossy().into_owned())
        .collect();
    if links.is_empty() {
        return;
    }
    let root = identity.writable_root.to_string_lossy();
    let link =
        r#"for at; do [ -e "$at" ] || { mkdir -p "$(dirname "$at")" && ln -s "$0" "$at"; }; done"#;
    let mut words = vec!["sh", "-c", link, &root];
    words.extend(links.iter().map(String::as_str));
    let failed = match channel.exec(None, None, &words).await {
        Ok(out) if out.status.success() => return,
        Ok(out) => String::from_utf8_lossy(&out.stderr).trim().to_string(),
        Err(e) => e.to_string(),
    };
    tracing::warn!(
        agent_id = identity.agent_id,
        error = %failed,
        "could not link the paths this box's seed was built at"
    );
}

/// Asks that the disk of the box in `box_dir` become its project's seed.
/// [`offer`] answers it once the box is down, and the request stays until a
/// workbench has looked at the disk.
pub fn want(box_dir: &Path) -> io::Result<()> {
    std::fs::write(box_dir.join(BOX_WANTED), "")
}

pub fn is_wanted(box_dir: &Path) -> bool {
    box_dir.join(BOX_WANTED).exists()
}

/// Makes the disk of `identity`'s box its project's seed, when it is wanted
/// ([`want`]) or the project has none yet. For a box that is down after a
/// clean stop, and stays down until this returns: the disk is cloned here,
/// the rest happens on a workbench, off the caller's path.
pub fn offer(identity: &BoxIdentity, box_dir: &Path) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let Some(project) = project_dir(identity) else {
        return;
    };
    let disk = box_dir.join(DATA_DISK);
    let disk_kib = held_kib(&disk);
    let would_be_the_first =
        !project.join(SEED_DISK).exists() && may_have_something_now(&identity.agent_id, disk_kib);
    if !is_wanted(box_dir) && !would_be_the_first {
        return;
    }
    let staged = project.join("seed.img.new");
    {
        let _store = STORE.lock();
        if BEING_MADE.lock().contains(&project) {
            return;
        }
        let cloned =
            remove_if_exists(&staged).and_then(|()| disks::clone_sharing_blocks(&disk, &staged));
        if let Err(e) = cloned {
            tracing::warn!(error = %e, "could not clone a box's disk to make a seed of it");
            return;
        }
        BEING_MADE.lock().push(project.clone());
    }
    let (identity, wanted) = (identity.clone(), box_dir.join(BOX_WANTED));
    tokio::spawn(async move {
        let kept_kib = reduce(&identity, &staged).await;
        tracing::info!(
            agent_id = identity.agent_id,
            ?kept_kib,
            "reduced a box's disk to make a seed of it"
        );
        // A disk the workbench could not reduce was not looked at: the
        // request stays, and the box is asked again at its next stop.
        if let Ok(kept_kib) = kept_kib {
            let _ = std::fs::remove_file(&wanted);
            if kept_kib < WORTH_INHERITING_KIB {
                had_nothing(&identity.agent_id, disk_kib);
            }
        }
        let worth_inheriting = kept_kib.is_ok_and(|kib| kib >= WORTH_INHERITING_KIB);
        settle(&project, &staged, &identity.writable_root, worth_inheriting);
        BEING_MADE
            .lock()
            .retain(|being_made| *being_made != project);
    });
}

/// Whether the disk of `agent_id`'s box, holding `disk_kib`, may have
/// something worth inheriting: it was never found with nothing, or holds
/// much more than when it was.
fn may_have_something_now(agent_id: &str, disk_kib: u64) -> bool {
    HAD_NOTHING
        .lock()
        .iter()
        .find(|(had_nothing, _)| had_nothing == agent_id)
        .map_or(true, |(_, then_kib)| {
            disk_kib as f64 >= *then_kib as f64 * ASKED_AGAIN_FOR_A_DISK_TIMES
        })
}

fn had_nothing(agent_id: &str, disk_kib: u64) {
    let mut boxes = HAD_NOTHING.lock();
    boxes.retain(|(had_nothing, _)| had_nothing != agent_id);
    boxes.push((agent_id.to_string(), disk_kib));
}

/// What `disk` takes on this machine: a sparse file's blocks, not its length.
fn held_kib(disk: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(disk).map_or(0, |disk| disk.blocks() / 2)
}

/// Ends the making of a seed: `staged`, reduced from the workspace at `root`,
/// becomes the project's seed when it is `worth_inheriting`, and is removed
/// otherwise, leaving the seed there was. Where it was built is recorded
/// before the seed is replaced: a root recorded for a disk that did not
/// become the seed is one link nothing uses, a seed without its root is
/// files whose paths do not resolve.
fn settle(project: &Path, staged: &Path, root: &Path, worth_inheriting: bool) {
    let _store = STORE.lock();
    if worth_inheriting {
        let published = record_built_at(project, root)
            .and_then(|()| std::fs::rename(staged, project.join(SEED_DISK)));
        if let Err(e) = published {
            tracing::warn!(project = %project.display(), error = %e, "could not publish a project's seed");
        }
    }
    let _ = remove_if_exists(staged);
}

fn record_built_at(project: &Path, root: &Path) -> io::Result<()> {
    let mut built_at = built_at(project);
    built_at.retain(|at| at != root);
    built_at.insert(0, root.to_path_buf());
    std::fs::write(project.join(SEED_BUILT_AT), serde_json::to_vec(&built_at)?)
}

/// Runs `seed.sh` on a workbench booted on `disk`, a clone of the data disk
/// of `identity`'s box, and answers the KiB it kept. An error unless the
/// script finished and the bench stopped cleanly.
async fn reduce(identity: &BoxIdentity, disk: &Path) -> Result<u64> {
    let ready = super::setup::ready_now()?;
    let dir = Workbench::fresh_dir(&ready.paths, "seed")?;
    let bench = Workbench::boot(&ready.paths, &ready.image.path(), dir, disk, None).await?;
    let cache = toolchain_cache_on_disk(identity);
    let ran = async {
        let script = Path::new("/tmp/seed.sh");
        bench.channel.put(REDUCE.as_bytes(), script, 0o755).await?;
        let words = ["bash", "/tmp/seed.sh", cache.as_str()];
        let run = bench.channel.exec(None, None, &words);
        tokio::time::timeout(REDUCE_TIMEOUT, run)
            .await
            .map_err(|_| fletch_box::Error::Other("reducing the disk timed out".into()))?
    }
    .await;
    let stopped = bench.stop().await;
    let out = ran?;
    if !out.status.success() {
        return Err(Error::Other(format!(
            "reducing the disk failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    if stopped == StopOutcome::Killed {
        return Err(Error::Other(
            "the workbench had to be killed; the disk may be incomplete".into(),
        ));
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .last()
        .and_then(|kib| kib.trim().parse().ok())
        .ok_or_else(|| Error::Other("reducing the disk did not say what it kept".into()))
}

/// Where the project's toolchain cache lies on the data disk of `identity`'s
/// box, as a workbench sees that disk: the box's home is its `/data/home`.
/// Empty when the cache is not in the home.
fn toolchain_cache_on_disk(identity: &BoxIdentity) -> String {
    ToolchainCache::in_guest(&identity.home, &identity.project_id)
        .ok()
        .and_then(|cache| {
            let in_home = cache.root.strip_prefix(&identity.home).ok()?;
            Some(
                Path::new(fletch_box::guest::DATA_MOUNT)
                    .join("home")
                    .join(in_home)
                    .to_string_lossy()
                    .into_owned(),
            )
        })
        .unwrap_or_default()
}

fn remove_if_exists(file: &Path) -> io::Result<()> {
    match std::fs::remove_file(file) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    fn project_with_seed(dir: &Path, disk: &[u8]) -> PathBuf {
        let project = dir.join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join(SEED_DISK), disk).unwrap();
        project
    }

    fn roots(roots: &[&str]) -> Vec<PathBuf> {
        roots.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn a_new_box_gets_a_clone_of_a_seed_its_size() {
        let dir = tempfile::tempdir().unwrap();
        let project = project_with_seed(dir.path(), b"seed");
        let data = dir.path().join("data.img");

        assert!(!clone_seed(&project, &data, 8));
        assert!(!data.exists());
        assert!(clone_seed(&project, &data, 4));
        assert_eq!(std::fs::read(&data).unwrap(), b"seed");
    }

    #[test]
    fn a_disk_worth_inheriting_replaces_the_seed_and_leads_where_it_was_built() {
        let dir = tempfile::tempdir().unwrap();
        let project = project_with_seed(dir.path(), b"old seed");
        let staged = project.join("seed.img.new");
        for (root, disk) in [("/w/kansai", "second seed"), ("/w/chiapas", "third seed")] {
            std::fs::write(&staged, disk).unwrap();
            settle(&project, &staged, Path::new(root), true);
        }

        assert_eq!(
            std::fs::read(project.join(SEED_DISK)).unwrap(),
            b"third seed"
        );
        assert_eq!(built_at(&project), roots(&["/w/chiapas", "/w/kansai"]));

        std::fs::write(&staged, "fourth seed").unwrap();
        settle(&project, &staged, Path::new("/w/kansai"), true);
        assert_eq!(built_at(&project), roots(&["/w/kansai", "/w/chiapas"]));
    }

    #[test]
    fn a_seed_remembers_every_workspace_it_was_ever_built_at() {
        let dir = tempfile::tempdir().unwrap();
        let project = project_with_seed(dir.path(), b"seed");
        let staged = project.join("seed.img.new");
        let all: Vec<String> = (0..40).map(|n| format!("/w/workspace-{n}")).collect();
        for root in &all {
            std::fs::write(&staged, root).unwrap();
            settle(&project, &staged, Path::new(root), true);
        }
        let mut newest_first: Vec<PathBuf> = all.iter().map(PathBuf::from).collect();
        newest_first.reverse();
        assert_eq!(built_at(&project), newest_first);
    }

    #[test]
    fn where_a_seed_was_built_is_recorded_even_if_the_disk_cannot_be_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let project = project_with_seed(dir.path(), b"seed");
        let missing = project.join("seed.img.new");

        settle(&project, &missing, Path::new("/w/chiapas"), true);

        assert_eq!(std::fs::read(project.join(SEED_DISK)).unwrap(), b"seed");
        assert_eq!(built_at(&project), roots(&["/w/chiapas"]));
    }

    #[test]
    fn a_box_that_had_nothing_is_asked_again_once_its_disk_holds_much_more() {
        let agent = "a-box-with-nothing-yet";
        assert!(may_have_something_now(agent, 100));
        had_nothing(agent, 100);
        assert!(!may_have_something_now(agent, 140));
        assert!(may_have_something_now(agent, 150));
        assert!(may_have_something_now("another-box", 1));
    }

    #[test]
    fn a_disk_that_kept_nothing_leaves_the_seed_there_was() {
        let dir = tempfile::tempdir().unwrap();
        let project = project_with_seed(dir.path(), b"seed");
        let staged = project.join("seed.img.new");
        std::fs::write(&staged, b"reduced to nothing").unwrap();

        settle(&project, &staged, Path::new("/w/chiapas"), false);

        assert_eq!(std::fs::read(project.join(SEED_DISK)).unwrap(), b"seed");
        assert!(!staged.exists());
        assert_eq!(built_at(&project), roots(&[]));
    }
}
