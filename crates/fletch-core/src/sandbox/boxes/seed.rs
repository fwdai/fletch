//! A project's seed on the box engine ([`crate::sandbox::seed`]): the data
//! disk a new box of the project starts with, sharing its blocks with every
//! other workspace until it changes them.
//!
//! ```text
//! <project seeds dir>/<project id>/seed/disk.img            a workspace disk, reduced
//! <project seeds dir>/<project id>/seed/made.json           what it was made without, and for which request
//! <project seeds dir>/<project id>/seed-built-at.json       the workspaces whose files it holds
//! <project seeds dir>/<project id>/candidate-<id>/disk.img  a clone of a box's disk, to be reduced
//! <box dir>/seed-wanted                                      this box's disk is to be the next seed
//! ```
//!
//! A box is at rest while it is down after a clean stop ([`DownBox`]). Its
//! disk is cloned then, and a workbench reduces the clone with `seed.sh` to
//! the large directories git ignores in each checkout and the project's
//! toolchain cache, with nothing else of that workspace left.
//!
//! A new box's data disk is a clone of the seed ([`new_data_disk`]), and
//! provisioning checks the base out over what the seed left (`provision`).
//! What a seed holds was built at the path of the workspace it came from, and
//! some of it says so (a virtualenv's scripts): provisioning links those
//! paths to the box's own root, so they still resolve ([`link_built_at`]).

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fletch_box::workbench::{BenchDir, Workbench};
use fletch_box::{disks, Channel, StopOutcome};
use parking_lot::Mutex;

use super::registry::{BoxIdentity, DownBox, DATA_DISK};
use crate::error::{Error, Result};
use crate::sandbox::seed::{self, read_json, AtRest, WORTH_INHERITING_KIB};
use crate::sandbox::ToolchainCache;

const DISK: &str = "disk.img";
const BUILT_AT: &str = "seed-built-at.json";
const BOX_WANTED: &str = "seed-wanted";
const REDUCE: &str = include_str!("seed.sh");

const REDUCE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// A box whose disk had nothing worth inheriting is asked again for its
/// project's first seed once the disk holds this many times as much.
const ASKED_AGAIN_FOR_A_DISK_TIMES: f64 = 1.5;

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

/// Creates the data disk of a new box: a clone of its project's seed when
/// there is one of `bytes`, an empty disk otherwise.
pub fn new_data_disk(identity: &BoxIdentity, data: &Path, bytes: u64) -> io::Result<()> {
    let cloned = project_dir(identity).is_some_and(|project| {
        seed::start_from(&project, &identity.project_id, |seed| {
            clone_seed(seed, data, bytes)
        })
    });
    if cloned {
        Ok(())
    } else {
        disks::create_sparse(data, bytes)
    }
}

fn clone_seed(seed: &Path, data: &Path, bytes: u64) -> bool {
    let disk = seed.join(DISK);
    std::fs::metadata(&disk).is_ok_and(|disk| disk.len() == bytes)
        && disks::clone_sharing_blocks(&disk, data).is_ok()
}

/// The writable roots of the workspaces whose files the project's seed
/// holds, the newest first. Every one the project ever had: a file kept
/// through any number of seeds still names the path it was built at.
fn built_at(project: &Path) -> Vec<PathBuf> {
    read_json(&project.join(BUILT_AT))
}

fn record_built_at(project: &Path, root: &Path) -> io::Result<()> {
    let mut built_at = built_at(project);
    built_at.retain(|at| at != root);
    built_at.insert(0, root.to_path_buf());
    std::fs::write(project.join(BUILT_AT), serde_json::to_vec(&built_at)?)
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

/// Asks that the disk of the box in `box_dir` become its project's seed
/// ([`seed::want`]).
pub fn want(box_dir: &Path) -> io::Result<()> {
    seed::want(&box_dir.join(BOX_WANTED))
}

pub fn is_wanted(box_dir: &Path) -> bool {
    seed::is_wanted(&box_dir.join(BOX_WANTED))
}

/// Offers the disk of a box that is down as its project's seed
/// ([`seed::offer`]). The disk is cloned here, while the box cannot boot; the
/// rest happens on a workbench, off the caller's path.
pub(super) fn offer(down: &DownBox<'_>) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let Some(project) = project_dir(down.identity) else {
        return;
    };
    let disk = down.dir.join(DATA_DISK);
    seed::offer(DiskAtRest {
        identity: down.identity.clone(),
        disk_kib: held_kib(&disk),
        disk,
        project,
        request_file: down.dir.join(BOX_WANTED),
    });
}

/// Removes the candidates an earlier run of the app left
/// ([`seed::remove_abandoned_candidates`]).
pub fn remove_abandoned_candidates() {
    if let Some(dir) = project_seeds_dir() {
        seed::remove_abandoned_candidates(&dir);
    }
}

/// The data disk of a box that is down after a clean stop.
struct DiskAtRest {
    identity: BoxIdentity,
    disk: PathBuf,
    /// What the disk held when the box was offered.
    disk_kib: u64,
    project: PathBuf,
    request_file: PathBuf,
}

impl AtRest for DiskAtRest {
    fn agent_id(&self) -> &str {
        &self.identity.agent_id
    }

    fn project_id(&self) -> &str {
        &self.identity.project_id
    }

    fn store(&self) -> &Path {
        &self.project
    }

    fn request_file(&self) -> &Path {
        &self.request_file
    }

    fn may_be_first(&self) -> bool {
        may_have_something_now(&self.identity.agent_id, self.disk_kib)
    }

    fn take(&self, candidate: &Path) -> io::Result<()> {
        disks::clone_sharing_blocks(&self.disk, &candidate.join(DISK))
    }

    async fn reduce(&self, candidate: &Path, excluded: &[String]) -> Result<u64> {
        reduce(&self.identity, &candidate.join(DISK), excluded).await
    }

    fn kept_nothing(&self) {
        had_nothing(&self.identity.agent_id, self.disk_kib);
    }

    /// Records where the seed was built before it is published: a root
    /// recorded for a candidate that did not become the seed is one link
    /// nothing uses, a seed without its root is files whose paths do not
    /// resolve.
    fn publishing(&self) -> io::Result<()> {
        record_built_at(&self.project, &self.identity.writable_root)
    }
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

/// Runs `seed.sh` on a workbench booted on `disk`, a clone of the data disk
/// of `identity`'s box, keeping nothing at the paths in `excluded`, and
/// answers the KiB it kept. An error unless the script finished and the bench
/// stopped cleanly.
async fn reduce(identity: &BoxIdentity, disk: &Path, excluded: &[String]) -> Result<u64> {
    let ready = super::setup::ready_now()?;
    let dir = BenchDir::fresh(&ready.paths, "seed")?;
    let bench = Workbench::boot(&ready.paths, &ready.image.path(), dir, disk, None).await?;
    let cache = toolchain_cache_on_disk(identity);
    let worth_inheriting_kib = WORTH_INHERITING_KIB.to_string();
    let mut words = vec!["bash", "/tmp/seed.sh", &cache, &worth_inheriting_kib];
    words.extend(excluded.iter().map(String::as_str));
    let ran = async {
        let script = Path::new("/tmp/seed.sh");
        bench.channel.put(REDUCE.as_bytes(), script, 0o755).await?;
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

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    fn roots(roots: &[&str]) -> Vec<PathBuf> {
        roots.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn a_new_box_gets_a_clone_of_a_seed_its_size() {
        let dir = tempfile::tempdir().unwrap();
        let seed = dir.path().join("seed");
        std::fs::create_dir_all(&seed).unwrap();
        std::fs::write(seed.join(DISK), b"seed").unwrap();
        let data = dir.path().join("data.img");

        assert!(!clone_seed(&seed, &data, 8));
        assert!(!data.exists());
        assert!(clone_seed(&seed, &data, 4));
        assert_eq!(std::fs::read(&data).unwrap(), b"seed");
    }

    #[test]
    fn a_seed_remembers_every_workspace_it_was_ever_built_at_the_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        let all: Vec<String> = (0..40).map(|n| format!("/w/workspace-{n}")).collect();
        for root in &all {
            record_built_at(project, Path::new(root)).unwrap();
        }
        let mut newest_first: Vec<PathBuf> = all.iter().map(PathBuf::from).collect();
        newest_first.reverse();
        assert_eq!(built_at(project), newest_first);

        record_built_at(project, Path::new("/w/workspace-0")).unwrap();
        assert_eq!(
            built_at(project)[..2],
            roots(&["/w/workspace-0", "/w/workspace-39"])
        );
        assert_eq!(built_at(project).len(), 40);
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
}
