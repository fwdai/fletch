//! A project's seed: the data disk a new box of the project starts with, so a
//! new workspace has the project's dependencies and downloads installed and
//! shares their blocks with every other workspace until it changes them.
//!
//! ```text
//! <project seeds dir>/<project id>/seed/disk.img            a workspace disk, reduced
//! <project seeds dir>/<project id>/seed/made-without.json   the paths excluded when it was made
//! <project seeds dir>/<project id>/seed-built-at.json       the workspaces whose files it holds
//! <project seeds dir>/<project id>/candidate-<id>/disk.img  a clone of a box's disk, to be reduced
//! <box dir>/seed-wanted                                      this box's disk is to be the next seed
//! ```
//!
//! A project's first seed is made of the first box to stop cleanly with
//! something worth inheriting, so its workspaces share from as early as they
//! can. After that, what a workspace installed matters to the others once its
//! work is in the base: when its pull request merges into the repository's
//! default branch, its disk is wanted as the seed ([`want`]) and replaces it
//! the next time its box is down after a clean stop. Both go through
//! [`offer`]: the disk is cloned as a [`Candidate`], which a workbench
//! reduces with `seed.sh` to the large directories git ignores in each
//! checkout and the project's toolchain cache, with nothing else of that
//! workspace left. Only a candidate the script finished on is published.
//!
//! The project's settings file says what a seed may hold
//! ([`crate::project_file::SeedSettings`]): paths never kept, or no seed at
//! all. A seed made before a path was excluded is removed, not started from.
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
use crate::project_file::SeedSettings;
use crate::sandbox::ToolchainCache;

const SEED: &str = "seed";
const CANDIDATE_PREFIX: &str = "candidate-";
const DISK: &str = "disk.img";
const MADE_WITHOUT: &str = "made-without.json";
const BUILT_AT: &str = "seed-built-at.json";
const BOX_WANTED: &str = "seed-wanted";
const REDUCE: &str = include_str!("seed.sh");

/// A directory, or a whole disk, that keeps less than this has nothing worth
/// inheriting.
const WORTH_INHERITING_KIB: u64 = 1024;
const REDUCE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// A box whose disk had nothing worth inheriting is asked again for its
/// project's first seed once the disk holds this many times as much.
const ASKED_AGAIN_FOR_A_DISK_TIMES: f64 = 1.5;

/// Serializes every read and write of a project's seed and of a box's
/// request; each is a few file operations.
static STORE: Mutex<()> = Mutex::new(());
/// One candidate is on a workbench at a time; the others wait their turn in
/// the order they were cloned.
static WORKBENCH_TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
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
    let settings = crate::project_file::seed_settings(&identity.project_id);
    let cloned =
        project_dir(identity).is_some_and(|project| clone_seed(&project, &settings, data, bytes));
    if cloned {
        Ok(())
    } else {
        disks::create_sparse(data, bytes)
    }
}

fn clone_seed(project: &Path, settings: &SeedSettings, data: &Path, bytes: u64) -> bool {
    let _store = STORE.lock();
    seed_in_force(project, settings).is_some_and(|seed| {
        std::fs::metadata(&seed).is_ok_and(|seed| seed.len() == bytes)
            && disks::clone_sharing_blocks(&seed, data).is_ok()
    })
}

/// The disk of the project's seed, while `settings` allow a box to start from
/// it: seeds are on, and it was made without every path they exclude now. A
/// seed they do not allow is removed. The caller holds [`STORE`].
fn seed_in_force(project: &Path, settings: &SeedSettings) -> Option<PathBuf> {
    let seed = project.join(SEED);
    if !seed.exists() {
        return None;
    }
    let made_without: Vec<String> = read_json(&seed.join(MADE_WITHOUT));
    let allowed = settings.enabled
        && settings
            .exclude
            .iter()
            .all(|path| made_without.contains(path));
    if allowed {
        return Some(seed.join(DISK));
    }
    if let Err(e) = std::fs::remove_dir_all(&seed) {
        tracing::warn!(seed = %seed.display(), error = %e, "could not remove a seed its project's settings no longer allow");
    }
    None
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

/// Asks that the disk of the box in `box_dir` become its project's seed.
/// [`offer`] answers it once the box is down, and the request stands until a
/// workbench has looked at a disk cloned after it was made.
pub fn want(box_dir: &Path) -> io::Result<()> {
    let _store = STORE.lock();
    std::fs::write(box_dir.join(BOX_WANTED), uuid::Uuid::new_v4().to_string())
}

pub fn is_wanted(box_dir: &Path) -> bool {
    standing_request(box_dir).is_some()
}

/// The request standing for the box in `box_dir`: what [`want`] wrote, which
/// no other request has.
fn standing_request(box_dir: &Path) -> Option<String> {
    std::fs::read_to_string(box_dir.join(BOX_WANTED)).ok()
}

/// Withdraws the request `answered`, unless a newer one has replaced it.
fn withdraw(box_dir: &Path, answered: &str) {
    let _store = STORE.lock();
    if standing_request(box_dir).as_deref() == Some(answered) {
        let _ = std::fs::remove_file(box_dir.join(BOX_WANTED));
    }
}

/// Makes the disk of a box that is down its project's seed, when it is wanted
/// ([`want`]) or the project has none yet. The disk is cloned here, while the
/// box cannot boot; the rest happens on a workbench, off the caller's path.
pub(super) fn offer(down: &DownBox<'_>) {
    if let Some(candidate) = Candidate::clone_of(down) {
        crate::host::spawn(candidate.settle());
    }
}

/// Removes the candidates an earlier run of the app left: one lives only as
/// long as the task that settles it.
pub fn remove_abandoned_candidates() {
    let projects = project_seeds_dir().and_then(|dir| std::fs::read_dir(dir).ok());
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

/// A clone of a down box's disk, on its way to being its project's seed. The
/// clone goes when this does.
struct Candidate {
    identity: BoxIdentity,
    box_dir: PathBuf,
    project: PathBuf,
    settings: SeedSettings,
    dir: PathBuf,
    /// What the box's own disk held when it was cloned.
    box_disk_kib: u64,
    /// The request the disk was cloned under, when one stood.
    request: Option<String>,
}

impl Candidate {
    fn clone_of(down: &DownBox<'_>) -> Option<Candidate> {
        if !cfg!(target_os = "macos") {
            return None;
        }
        let project = project_dir(down.identity)?;
        let box_disk = down.dir.join(DATA_DISK);
        let candidate = Candidate {
            identity: down.identity.clone(),
            box_dir: down.dir.to_path_buf(),
            settings: crate::project_file::seed_settings(&down.identity.project_id),
            dir: project.join(format!("{CANDIDATE_PREFIX}{}", uuid::Uuid::new_v4())),
            box_disk_kib: held_kib(&box_disk),
            request: standing_request(down.dir),
            project,
        };
        if !candidate.is_needed() {
            return None;
        }
        match disks::clone_sharing_blocks(&box_disk, &candidate.dir.join(DISK)) {
            Ok(()) => Some(candidate),
            Err(e) => {
                tracing::warn!(error = %e, "could not clone a box's disk to make a seed of it");
                None
            }
        }
    }

    /// Whether the project has use for this disk: seeds are on, and the disk
    /// was asked for or may be the first seed of a project without one.
    fn is_needed(&self) -> bool {
        let _store = STORE.lock();
        let has_seed = seed_in_force(&self.project, &self.settings).is_some();
        let may_be_the_first =
            !has_seed && may_have_something_now(&self.identity.agent_id, self.box_disk_kib);
        self.settings.enabled && (self.request.is_some() || may_be_the_first)
    }

    /// Takes the candidate to a workbench in its turn. The request it was
    /// cloned under is withdrawn once the workbench has looked at the disk and
    /// what it found is settled; after any failure the request stands, and
    /// the box is offered again.
    async fn settle(self) {
        let _turn = WORKBENCH_TURN.lock().await;
        if !self.is_needed() {
            return;
        }
        let agent_id = &self.identity.agent_id;
        match self.examine().await {
            Ok(kept_kib) => {
                tracing::info!(
                    agent_id,
                    kept_kib,
                    "looked at a box's disk for its project's seed"
                );
                if let Some(request) = &self.request {
                    withdraw(&self.box_dir, request);
                }
            }
            Err(e) => {
                tracing::warn!(agent_id, error = %e, "could not make a seed of a box's disk");
            }
        }
    }

    /// Reduces the candidate on a workbench and publishes it when it kept
    /// something worth inheriting. Answers the KiB it kept.
    async fn examine(&self) -> Result<u64> {
        let kept_kib = reduce(&self.identity, &self.dir.join(DISK), &self.settings.exclude).await?;
        if kept_kib >= WORTH_INHERITING_KIB {
            publish(
                &self.project,
                &self.dir,
                &self.identity.writable_root,
                &self.settings.exclude,
            )?;
        } else {
            had_nothing(&self.identity.agent_id, self.box_disk_kib);
        }
        Ok(kept_kib)
    }
}

impl Drop for Candidate {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Makes `candidate`, reduced from the workspace at `root` without the paths
/// in `excluded`, the project's seed. What it was made without goes with its
/// disk in one rename. Where it was built is recorded first: a root recorded
/// for a candidate that did not become the seed is one link nothing uses, a
/// seed without its root is files whose paths do not resolve.
fn publish(project: &Path, candidate: &Path, root: &Path, excluded: &[String]) -> io::Result<()> {
    let _store = STORE.lock();
    record_built_at(project, root)?;
    std::fs::write(candidate.join(MADE_WITHOUT), serde_json::to_vec(excluded)?)?;
    let seed = project.join(SEED);
    if seed.exists() {
        std::fs::remove_dir_all(&seed)?;
    }
    std::fs::rename(candidate, seed)
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

/// What the JSON file at `path` holds; the default when there is none.
fn read_json<T: serde::de::DeserializeOwned + Default>(path: &Path) -> T {
    std::fs::read(path)
        .ok()
        .and_then(|json| serde_json::from_slice(&json).ok())
        .unwrap_or_default()
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    fn project_with_seed(dir: &Path, disk: &[u8], made_without: &[&str]) -> PathBuf {
        let project = dir.join("project");
        let seed = project.join(SEED);
        std::fs::create_dir_all(&seed).unwrap();
        std::fs::write(seed.join(DISK), disk).unwrap();
        std::fs::write(
            seed.join(MADE_WITHOUT),
            serde_json::to_vec(made_without).unwrap(),
        )
        .unwrap();
        project
    }

    fn candidate(project: &Path, disk: &str) -> PathBuf {
        let candidate = project.join(format!("{CANDIDATE_PREFIX}test"));
        std::fs::create_dir_all(&candidate).unwrap();
        std::fs::write(candidate.join(DISK), disk).unwrap();
        candidate
    }

    fn seed_disk(project: &Path) -> Option<String> {
        std::fs::read_to_string(project.join(SEED).join(DISK)).ok()
    }

    fn excluding(paths: &[&str]) -> SeedSettings {
        SeedSettings {
            enabled: true,
            exclude: paths.iter().map(|path| path.to_string()).collect(),
        }
    }

    fn roots(roots: &[&str]) -> Vec<PathBuf> {
        roots.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn a_new_box_gets_a_clone_of_a_seed_its_size() {
        let dir = tempfile::tempdir().unwrap();
        let project = project_with_seed(dir.path(), b"seed", &[]);
        let data = dir.path().join("data.img");
        let settings = SeedSettings::default();

        assert!(!clone_seed(&project, &settings, &data, 8));
        assert!(!data.exists());
        assert!(clone_seed(&project, &settings, &data, 4));
        assert_eq!(std::fs::read(&data).unwrap(), b"seed");
    }

    #[test]
    fn a_seed_made_before_a_path_was_excluded_is_removed_not_started_from() {
        let dir = tempfile::tempdir().unwrap();
        let project = project_with_seed(dir.path(), b"seed", &["uploads"]);
        let data = dir.path().join("data.img");

        assert!(clone_seed(&project, &excluding(&[]), &data, 4));
        std::fs::remove_file(&data).unwrap();
        assert!(clone_seed(&project, &excluding(&["uploads"]), &data, 4));
        std::fs::remove_file(&data).unwrap();

        assert!(!clone_seed(
            &project,
            &excluding(&["uploads", "certs"]),
            &data,
            4
        ));
        assert!(!data.exists());
        assert!(!project.join(SEED).exists());
    }

    #[test]
    fn a_project_that_turned_seeds_off_loses_the_one_it_had() {
        let dir = tempfile::tempdir().unwrap();
        let project = project_with_seed(dir.path(), b"seed", &[]);
        let data = dir.path().join("data.img");
        let off = SeedSettings {
            enabled: false,
            exclude: Vec::new(),
        };

        assert!(!clone_seed(&project, &off, &data, 4));
        assert!(!data.exists());
        assert!(!project.join(SEED).exists());
    }

    #[test]
    fn a_published_candidate_replaces_the_seed_with_what_it_was_made_without() {
        let dir = tempfile::tempdir().unwrap();
        let project = project_with_seed(dir.path(), b"old seed", &[]);
        let excluded = vec!["uploads".to_string()];

        let second = candidate(&project, "second seed");
        publish(&project, &second, Path::new("/w/kansai"), &excluded).unwrap();

        assert_eq!(seed_disk(&project).as_deref(), Some("second seed"));
        assert!(!second.exists());
        let data = dir.path().join("data.img");
        assert!(clone_seed(&project, &excluding(&["uploads"]), &data, 11));
    }

    #[test]
    fn a_seed_remembers_every_workspace_it_was_ever_built_at_the_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let project = project_with_seed(dir.path(), b"seed", &[]);
        let all: Vec<String> = (0..40).map(|n| format!("/w/workspace-{n}")).collect();
        for root in &all {
            publish(&project, &candidate(&project, root), Path::new(root), &[]).unwrap();
        }
        let mut newest_first: Vec<PathBuf> = all.iter().map(PathBuf::from).collect();
        newest_first.reverse();
        assert_eq!(built_at(&project), newest_first);

        let again = candidate(&project, "again");
        publish(&project, &again, Path::new("/w/workspace-0"), &[]).unwrap();
        assert_eq!(
            built_at(&project)[..2],
            roots(&["/w/workspace-0", "/w/workspace-39"])
        );
        assert_eq!(built_at(&project).len(), 40);
    }

    #[test]
    fn a_candidate_that_cannot_replace_the_seed_fails_and_leaves_it() {
        let dir = tempfile::tempdir().unwrap();
        let project = project_with_seed(dir.path(), b"seed", &[]);
        let missing = project.join(format!("{CANDIDATE_PREFIX}missing"));

        assert!(publish(&project, &missing, Path::new("/w/chiapas"), &[]).is_err());

        assert_eq!(seed_disk(&project).as_deref(), Some("seed"));
        assert_eq!(built_at(&project), roots(&["/w/chiapas"]));
    }

    #[test]
    fn a_request_is_withdrawn_only_by_the_answer_to_it() {
        let box_dir = tempfile::tempdir().unwrap();
        assert!(!is_wanted(box_dir.path()));

        want(box_dir.path()).unwrap();
        let cloned_under = standing_request(box_dir.path()).unwrap();
        want(box_dir.path()).unwrap();
        withdraw(box_dir.path(), &cloned_under);
        assert!(is_wanted(box_dir.path()), "a newer request stands");

        let newer = standing_request(box_dir.path()).unwrap();
        withdraw(box_dir.path(), &newer);
        assert!(!is_wanted(box_dir.path()));
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
