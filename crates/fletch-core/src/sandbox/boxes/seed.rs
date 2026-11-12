//! A project's seed: the data disk a new box of the project starts with, so a
//! new workspace has the project's dependencies and downloads installed and
//! shares their blocks with every other workspace until it changes them.
//!
//! ```text
//! <project seeds dir>/<project id>/seed/disk.img            a workspace disk, reduced
//! <project seeds dir>/<project id>/seed/made.json           what it was made without, and for which request
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
//! all. They are read when they are used, never kept from an earlier look; a
//! seed made before a path was excluded is removed, not started from; and a
//! project whose file was not read yet neither makes a seed nor starts from
//! one.
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
use serde::{Deserialize, Serialize};

use super::registry::{BoxIdentity, DownBox, DATA_DISK};
use crate::error::{Error, Result};
use crate::project_file::SeedSettings;
use crate::sandbox::ToolchainCache;

const SEED: &str = "seed";
const CANDIDATE_PREFIX: &str = "candidate-";
const DISK: &str = "disk.img";
const MADE: &str = "made.json";
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

/// A request for a box's disk as its project's seed. Ordered by when it was
/// made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct Request(uuid::Uuid);

/// How a seed was made, kept beside its disk.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Made {
    /// The paths the project excluded then.
    without: Vec<String>,
    /// The request it answers; none for a project's first seed.
    for_request: Option<Request>,
}

/// Creates the data disk of a new box: a clone of its project's seed when
/// there is one of `bytes`, an empty disk otherwise.
pub fn new_data_disk(identity: &BoxIdentity, data: &Path, bytes: u64) -> io::Result<()> {
    let project = project_dir(identity);
    let settings = crate::project_file::seed_settings(&identity.project_id);
    let cloned = match (project, settings) {
        (Some(project), Some(settings)) => clone_seed(&project, &settings, data, bytes),
        _ => false,
    };
    if cloned {
        Ok(())
    } else {
        disks::create_sparse(data, bytes)
    }
}

fn clone_seed(project: &Path, settings: &SeedSettings, data: &Path, bytes: u64) -> bool {
    let _store = STORE.lock();
    let disk = project.join(SEED).join(DISK);
    seed_in_force(project, settings).is_some()
        && std::fs::metadata(&disk).is_ok_and(|disk| disk.len() == bytes)
        && disks::clone_sharing_blocks(&disk, data).is_ok()
}

/// How the project's seed was made, while `settings` allow a box to start
/// from it: seeds are on, and it was made without every path they exclude
/// now. A seed they do not allow is removed. The caller holds [`STORE`].
fn seed_in_force(project: &Path, settings: &SeedSettings) -> Option<Made> {
    let seed = project.join(SEED);
    if !seed.exists() {
        return None;
    }
    let made: Made = read_json(&seed.join(MADE));
    let allowed = settings.enabled
        && settings
            .exclude
            .iter()
            .all(|path| made.without.contains(path));
    if allowed {
        return Some(made);
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
    let request = Request(uuid::Uuid::now_v7());
    std::fs::write(box_dir.join(BOX_WANTED), serde_json::to_vec(&request)?)
}

pub fn is_wanted(box_dir: &Path) -> bool {
    standing_request(box_dir).is_some()
}

fn standing_request(box_dir: &Path) -> Option<Request> {
    read_json(&box_dir.join(BOX_WANTED))
}

/// Withdraws the request `answered`, unless a newer one has replaced it.
fn withdraw(box_dir: &Path, answered: Request) {
    let _store = STORE.lock();
    if standing_request(box_dir) == Some(answered) {
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
    dir: PathBuf,
    /// What the box's own disk held when it was cloned.
    box_disk_kib: u64,
    /// The request the disk was cloned under, when one stood.
    request: Option<Request>,
}

impl Candidate {
    fn clone_of(down: &DownBox<'_>) -> Option<Candidate> {
        if !cfg!(target_os = "macos") {
            return None;
        }
        let project = project_dir(down.identity)?;
        let settings = crate::project_file::seed_settings(&down.identity.project_id)?;
        let box_disk = down.dir.join(DATA_DISK);
        let candidate = Candidate {
            identity: down.identity.clone(),
            box_dir: down.dir.to_path_buf(),
            dir: project.join(format!("{CANDIDATE_PREFIX}{}", uuid::Uuid::new_v4())),
            box_disk_kib: held_kib(&box_disk),
            request: standing_request(down.dir),
            project,
        };
        if !candidate.is_needed(&settings) {
            candidate.withdraw_request();
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

    /// Whether the project has use for this disk under `settings`: it answers
    /// a request newer than the one the seed answers, or the project has no
    /// seed, may have one, and the disk was asked for or may be its first.
    fn is_needed(&self, settings: &SeedSettings) -> bool {
        let _store = STORE.lock();
        match seed_in_force(&self.project, settings) {
            Some(seed) => self.request > seed.for_request,
            None => {
                let may_be_the_first =
                    may_have_something_now(&self.identity.agent_id, self.box_disk_kib);
                settings.enabled && (self.request.is_some() || may_be_the_first)
            }
        }
    }

    /// Takes the candidate to a workbench in its turn, under the settings
    /// that stand then. The request it was cloned under is withdrawn once it
    /// is answered: the project has no use for the disk, or the workbench has
    /// looked at it and what it found is settled. After any failure the
    /// request stands, and the box is offered again.
    async fn settle(self) {
        let _turn = WORKBENCH_TURN.lock().await;
        let agent_id = &self.identity.agent_id;
        let Some(settings) = crate::project_file::seed_settings(&self.identity.project_id) else {
            return;
        };
        if !self.is_needed(&settings) {
            self.withdraw_request();
            return;
        }
        match self.examine(&settings).await {
            Ok(kept_kib) => {
                tracing::info!(
                    agent_id,
                    kept_kib,
                    "looked at a box's disk for its project's seed"
                );
                self.withdraw_request();
            }
            Err(e) => {
                tracing::warn!(agent_id, error = %e, "could not make a seed of a box's disk");
            }
        }
    }

    /// Reduces the candidate on a workbench under `settings` and publishes it
    /// when it kept something worth inheriting and the settings still stand.
    /// Answers the KiB it kept.
    async fn examine(&self, settings: &SeedSettings) -> Result<u64> {
        let kept_kib = reduce(&self.identity, &self.dir.join(DISK), &settings.exclude).await?;
        if kept_kib < WORTH_INHERITING_KIB {
            had_nothing(&self.identity.agent_id, self.box_disk_kib);
            return Ok(kept_kib);
        }
        let settings_now = crate::project_file::seed_settings(&self.identity.project_id);
        if settings_now.as_ref() != Some(settings) {
            return Err(Error::Other(
                "the project's seed settings changed while its seed was made".into(),
            ));
        }
        let made = Made {
            without: settings.exclude.clone(),
            for_request: self.request,
        };
        publish(
            &self.project,
            &self.dir,
            &self.identity.writable_root,
            &made,
        )?;
        Ok(kept_kib)
    }

    fn withdraw_request(&self) {
        if let Some(request) = self.request {
            withdraw(&self.box_dir, request);
        }
    }
}

impl Drop for Candidate {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Makes `candidate`, reduced from the workspace at `root`, the project's
/// seed, with how it was `made` beside its disk. The seed there was is
/// exchanged for it in one step, so the project is never without one. Where
/// it was built is recorded first: a root recorded for a candidate that did
/// not become the seed is one link nothing uses, a seed without its root is
/// files whose paths do not resolve.
fn publish(project: &Path, candidate: &Path, root: &Path, made: &Made) -> io::Result<()> {
    let _store = STORE.lock();
    record_built_at(project, root)?;
    std::fs::write(candidate.join(MADE), serde_json::to_vec(made)?)?;
    let seed = project.join(SEED);
    if !seed.exists() {
        return std::fs::rename(candidate, seed);
    }
    disks::exchange(candidate, &seed)?;
    let previous_seed = candidate;
    let _ = std::fs::remove_dir_all(previous_seed);
    Ok(())
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

    fn made(without: &[&str], for_request: Option<Request>) -> Made {
        Made {
            without: without.iter().map(|path| path.to_string()).collect(),
            for_request,
        }
    }

    fn project_with_seed(dir: &Path, disk: &[u8], made: &Made) -> PathBuf {
        let project = dir.join("project");
        let seed = project.join(SEED);
        std::fs::create_dir_all(&seed).unwrap();
        std::fs::write(seed.join(DISK), disk).unwrap();
        std::fs::write(seed.join(MADE), serde_json::to_vec(made).unwrap()).unwrap();
        project
    }

    fn candidate_dir(project: &Path, disk: &str) -> PathBuf {
        let candidate = project.join(format!("{CANDIDATE_PREFIX}test"));
        std::fs::create_dir_all(&candidate).unwrap();
        std::fs::write(candidate.join(DISK), disk).unwrap();
        candidate
    }

    fn candidate(project: &Path, agent_id: &str, request: Option<Request>) -> Candidate {
        Candidate {
            identity: BoxIdentity {
                agent_id: agent_id.into(),
                project_id: "project".into(),
                provider: "claude".into(),
                writable_root: "/w/kansai".into(),
                rpc_dir: "/rpc/kansai".into(),
                home: "/home".into(),
                shares: Vec::new(),
            },
            box_dir: project.join("box"),
            project: project.to_path_buf(),
            dir: project.join(format!("{CANDIDATE_PREFIX}{agent_id}")),
            box_disk_kib: 100,
            request,
        }
    }

    fn new_request(box_dir: &Path) -> Request {
        want(box_dir).unwrap();
        standing_request(box_dir).unwrap()
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

    fn seeds_off() -> SeedSettings {
        SeedSettings {
            enabled: false,
            exclude: Vec::new(),
        }
    }

    fn roots(roots: &[&str]) -> Vec<PathBuf> {
        roots.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn a_new_box_gets_a_clone_of_a_seed_its_size() {
        let dir = tempfile::tempdir().unwrap();
        let project = project_with_seed(dir.path(), b"seed", &made(&[], None));
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
        let project = project_with_seed(dir.path(), b"seed", &made(&["uploads"], None));
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
        let project = project_with_seed(dir.path(), b"seed", &made(&[], None));
        let data = dir.path().join("data.img");

        assert!(!clone_seed(&project, &seeds_off(), &data, 4));
        assert!(!data.exists());
        assert!(!project.join(SEED).exists());
    }

    #[test]
    fn a_published_candidate_takes_the_seeds_place_with_how_it_was_made() {
        let dir = tempfile::tempdir().unwrap();
        let project = project_with_seed(dir.path(), b"old seed", &made(&[], None));

        let second = candidate_dir(&project, "second seed");
        let how = made(&["uploads"], None);
        publish(&project, &second, Path::new("/w/kansai"), &how).unwrap();

        assert_eq!(seed_disk(&project).as_deref(), Some("second seed"));
        assert!(!second.exists(), "the seed there was is gone");
        let data = dir.path().join("data.img");
        assert!(clone_seed(&project, &excluding(&["uploads"]), &data, 11));
    }

    #[test]
    fn a_projects_first_candidate_is_published_where_there_was_no_seed() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");

        let first = candidate_dir(&project, "first seed");
        publish(&project, &first, Path::new("/w/kansai"), &made(&[], None)).unwrap();

        assert_eq!(seed_disk(&project).as_deref(), Some("first seed"));
        assert_eq!(built_at(&project), roots(&["/w/kansai"]));
    }

    #[test]
    fn a_seed_remembers_every_workspace_it_was_ever_built_at_the_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let project = project_with_seed(dir.path(), b"seed", &made(&[], None));
        let all: Vec<String> = (0..40).map(|n| format!("/w/workspace-{n}")).collect();
        for root in &all {
            let candidate = candidate_dir(&project, root);
            publish(&project, &candidate, Path::new(root), &made(&[], None)).unwrap();
        }
        let mut newest_first: Vec<PathBuf> = all.iter().map(PathBuf::from).collect();
        newest_first.reverse();
        assert_eq!(built_at(&project), newest_first);

        let again = candidate_dir(&project, "again");
        publish(
            &project,
            &again,
            Path::new("/w/workspace-0"),
            &made(&[], None),
        )
        .unwrap();
        assert_eq!(
            built_at(&project)[..2],
            roots(&["/w/workspace-0", "/w/workspace-39"])
        );
        assert_eq!(built_at(&project).len(), 40);
    }

    #[test]
    fn a_candidate_that_cannot_take_the_seeds_place_fails_and_leaves_it() {
        let dir = tempfile::tempdir().unwrap();
        let project = project_with_seed(dir.path(), b"seed", &made(&[], None));
        let missing = project.join(format!("{CANDIDATE_PREFIX}missing"));

        assert!(publish(
            &project,
            &missing,
            Path::new("/w/chiapas"),
            &made(&[], None)
        )
        .is_err());

        assert_eq!(seed_disk(&project).as_deref(), Some("seed"));
        assert_eq!(built_at(&project), roots(&["/w/chiapas"]));
    }

    #[test]
    fn a_request_is_withdrawn_only_by_the_answer_to_it() {
        let box_dir = tempfile::tempdir().unwrap();
        assert!(!is_wanted(box_dir.path()));

        let cloned_under = new_request(box_dir.path());
        let newer = new_request(box_dir.path());
        assert!(newer > cloned_under);
        withdraw(box_dir.path(), cloned_under);
        assert!(is_wanted(box_dir.path()), "a newer request stands");

        withdraw(box_dir.path(), newer);
        assert!(!is_wanted(box_dir.path()));
    }

    #[test]
    fn a_disk_is_needed_for_a_request_newer_than_the_one_the_seed_answers() {
        let dir = tempfile::tempdir().unwrap();
        let box_dir = dir.path().join("box");
        std::fs::create_dir_all(&box_dir).unwrap();
        let (older, answered, newer) = (
            new_request(&box_dir),
            new_request(&box_dir),
            new_request(&box_dir),
        );
        let project = project_with_seed(dir.path(), b"seed", &made(&[], Some(answered)));
        let on = SeedSettings::default();
        let needed =
            |request: Option<Request>| candidate(&project, "a-box-asked", request).is_needed(&on);

        assert!(needed(Some(newer)));
        assert!(
            !needed(Some(older)),
            "an older merge never replaces a newer"
        );
        assert!(!needed(None), "a project with a seed needs no first one");
    }

    #[test]
    fn a_project_without_a_seed_needs_a_disk_that_was_asked_for_or_may_be_its_first() {
        let dir = tempfile::tempdir().unwrap();
        let box_dir = dir.path().join("box");
        std::fs::create_dir_all(&box_dir).unwrap();
        let project = dir.path().join("project");
        let request = Some(new_request(&box_dir));
        let on = SeedSettings::default();

        assert!(candidate(&project, "a-box-never-looked-at", None).is_needed(&on));
        assert!(!candidate(&project, "a-box-never-looked-at", None).is_needed(&seeds_off()));
        assert!(!candidate(&project, "a-box-never-looked-at", request).is_needed(&seeds_off()));

        had_nothing("a-box-found-empty", 100);
        assert!(!candidate(&project, "a-box-found-empty", None).is_needed(&on));
        assert!(candidate(&project, "a-box-found-empty", request).is_needed(&on));
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
