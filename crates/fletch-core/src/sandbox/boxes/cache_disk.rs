//! A project's toolchain cache as a disk the project's boxes share.
//!
//! ```text
//! <project caches dir>/<project id>/<generation>.img   the project's cache
//! <box dir>/cache.img                                  one box's clone of it
//! <box dir>/cache.json                                 that clone's Lineage
//! ```
//!
//! No VM ever mounts the project's cache. A box mounts a clone of it, which
//! costs no space until the box writes ([`BoxCache::attach`]). When a box
//! that downloaded something stops cleanly, its disk becomes the project's
//! next generation ([`BoxCache::settle`]), and every box that added nothing of
//! its own starts from that generation next time.
//!
//! Two boxes may both add to the same generation. The first to stop
//! publishes; the other keeps what it added on its own disk and is not
//! published, since publishing it would drop what the first one added.
//!
//! The cache is never why a box fails to boot: wherever this cannot give a
//! box a disk it gives none, and the guest caches on its data disk.

use std::io;
use std::path::{Path, PathBuf};

use fletch_box::runtime::console::CacheUse;
use fletch_box::{disks, CacheDisk};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::registry::BoxIdentity;
use crate::sandbox::ToolchainCache;

const BOX_DISK: &str = "cache.img";
const BOX_LINEAGE: &str = "cache.json";

/// Growth below this is what a toolchain writes on every run (logs, an
/// update check), not something another box would wait to download.
const WORTH_SHARING_KIB: u64 = 1024;

/// Serializes every read and write of a project's generations and a box's
/// lineage; each is a few file operations.
static STORE: Mutex<()> = Mutex::new(());

/// Where every project's cache disks are kept, one directory per project id.
/// Beside the boxes, so a clone never crosses a volume.
pub fn project_caches_dir() -> Option<PathBuf> {
    Some(super::boxes_root().ok()?.with_file_name("box-cache"))
}

/// One box's cache disk.
pub struct BoxCache {
    disk: PathBuf,
    lineage: PathBuf,
    guest_path: PathBuf,
    project: ProjectCache,
}

impl BoxCache {
    /// The disk `identity`'s box boots with: a clone of the project's newest
    /// generation for a new box and for one that holds nothing of its own,
    /// the box's disk as it is otherwise. `stopped_cleanly` is whether the
    /// box's last VM did; one that did not may have written the disk without
    /// saying so.
    pub fn attach(identity: &BoxIdentity, box_dir: &Path, stopped_cleanly: bool) -> Option<Self> {
        if !cfg!(target_os = "macos") {
            return None;
        }
        let guest_path = ToolchainCache::in_guest(&identity.home, &identity.project_id)
            .ok()?
            .root;
        if !guest_path.starts_with(&identity.home) {
            return None;
        }
        let project = ProjectCache {
            dir: project_caches_dir()?.join(&identity.project_id),
        };
        let cache = Self::in_dirs(box_dir, project, guest_path);
        match cache.prepare(stopped_cleanly) {
            Ok(()) => Some(cache),
            Err(e) => {
                tracing::warn!(
                    agent_id = identity.agent_id,
                    error = %e,
                    "no cache disk for this box; it caches on its data disk"
                );
                None
            }
        }
    }

    fn in_dirs(box_dir: &Path, project: ProjectCache, guest_path: PathBuf) -> Self {
        Self {
            disk: box_dir.join(BOX_DISK),
            lineage: box_dir.join(BOX_LINEAGE),
            guest_path,
            project,
        }
    }

    pub fn vm_disk(&self) -> CacheDisk {
        CacheDisk {
            disk: self.disk.clone(),
            guest_path: self.guest_path.clone(),
        }
    }

    fn prepare(&self, stopped_cleanly: bool) -> io::Result<()> {
        let _store = STORE.lock();
        let newest = self.project.newest_generation();
        let lineage = match Lineage::read(&self.lineage).filter(|_| self.disk.exists()) {
            None => {
                self.start_from(newest)?;
                Lineage::of(newest)
            }
            Some(lineage) if !stopped_cleanly => lineage.with_additions(),
            Some(lineage) if !lineage.has_additions && lineage.generation < newest => {
                self.start_from(newest)?;
                Lineage::of(newest)
            }
            Some(lineage) => lineage,
        };
        lineage.write(&self.lineage)
    }

    /// Replaces the box's disk with a clone of `generation`, or with an empty
    /// disk when the project has none yet.
    fn start_from(&self, generation: u64) -> io::Result<()> {
        let fresh = self.disk.with_extension("img.new");
        remove_if_exists(&fresh)?;
        if generation == NO_GENERATION {
            disks::create_sparse(&fresh, disks::CACHE_DISK_BYTES)?;
        } else {
            disks::clone_sharing_blocks(&self.project.generation(generation), &fresh)?;
        }
        std::fs::rename(&fresh, &self.disk)
    }

    /// After the box's VM stopped cleanly, with what init said its cache disk
    /// held (`None` when it said nothing, and nothing is concluded). A disk
    /// with additions becomes the project's next generation when it was
    /// cloned from the newest one.
    pub fn settle(&self, used: Option<CacheUse>) {
        let Some(used) = used else {
            return;
        };
        let grew = used.stopped_kib >= used.mounted_kib + WORTH_SHARING_KIB;
        let _store = STORE.lock();
        let Some(mut lineage) = Lineage::read(&self.lineage) else {
            return;
        };
        if grew {
            lineage = lineage.with_additions();
        }
        if lineage.has_additions && lineage.generation == self.project.newest_generation() {
            let next = lineage.generation + 1;
            match self.project.publish(&self.disk, next) {
                Ok(()) => lineage = Lineage::of(next),
                Err(e) => tracing::warn!(
                    project = %self.project.dir.display(),
                    error = %e,
                    "could not publish this box's cache to its project"
                ),
            }
        }
        if let Err(e) = lineage.write(&self.lineage) {
            tracing::warn!(lineage = %self.lineage.display(), error = %e, "could not record a box's cache lineage");
        }
    }
}

/// Which of its project's generations a box's disk came from, and whether the
/// box has put anything on it since.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Lineage {
    generation: u64,
    has_additions: bool,
}

impl Lineage {
    fn of(generation: u64) -> Self {
        Self {
            generation,
            has_additions: false,
        }
    }

    fn with_additions(self) -> Self {
        Self {
            has_additions: true,
            ..self
        }
    }

    fn read(file: &Path) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(file).ok()?).ok()
    }

    fn write(self, file: &Path) -> io::Result<()> {
        std::fs::write(file, serde_json::to_vec(&self)?)
    }
}

/// The generation of a project that has published none; a box that starts
/// there starts with an empty disk.
const NO_GENERATION: u64 = 0;

/// One project's cache: numbered disks, the highest the newest, each written
/// once and from then on only cloned.
struct ProjectCache {
    dir: PathBuf,
}

impl ProjectCache {
    fn generation(&self, generation: u64) -> PathBuf {
        self.dir.join(format!("{generation}.img"))
    }

    fn generations(&self) -> Vec<u64> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| {
                entry
                    .file_name()
                    .to_str()?
                    .strip_suffix(".img")?
                    .parse()
                    .ok()
            })
            .collect()
    }

    fn newest_generation(&self) -> u64 {
        self.generations()
            .into_iter()
            .max()
            .unwrap_or(NO_GENERATION)
    }

    /// Makes a clone of `box_disk` the project's `generation` and drops the
    /// older ones: every box that came from one holds a clone of its own.
    fn publish(&self, box_disk: &Path, generation: u64) -> io::Result<()> {
        let older = self.generations();
        let staged = self.dir.join(format!("{generation}.img.new"));
        remove_if_exists(&staged)?;
        disks::clone_sharing_blocks(box_disk, &staged)?;
        std::fs::rename(&staged, self.generation(generation))?;
        for old in older {
            remove_if_exists(&self.generation(old))?;
        }
        Ok(())
    }
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

    struct Project {
        dir: tempfile::TempDir,
    }

    impl Project {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
            }
        }

        fn cache(&self) -> ProjectCache {
            ProjectCache {
                dir: self.dir.path().join("project"),
            }
        }

        fn a_box(&self, name: &str) -> BoxCache {
            let box_dir = self.dir.path().join(name);
            std::fs::create_dir_all(&box_dir).unwrap();
            BoxCache::in_dirs(&box_dir, self.cache(), "/guest/cache".into())
        }
    }

    /// What a guest would have done to its disk: here, the file's bytes.
    fn guest_writes(cache: &BoxCache, content: &str) {
        std::fs::write(&cache.disk, content).unwrap();
    }

    fn holds(cache: &BoxCache) -> String {
        std::fs::read_to_string(&cache.disk).unwrap()
    }

    const GREW: Option<CacheUse> = Some(CacheUse {
        mounted_kib: 2_000,
        stopped_kib: 500_000,
    });
    const SAME: Option<CacheUse> = Some(CacheUse {
        mounted_kib: 500_000,
        stopped_kib: 500_100,
    });

    #[test]
    fn a_projects_first_box_starts_empty_and_publishes_what_it_downloads() {
        let project = Project::new();
        let first = project.a_box("first");
        first.prepare(true).unwrap();
        assert_eq!(
            std::fs::metadata(&first.disk).unwrap().len(),
            disks::CACHE_DISK_BYTES
        );
        assert_eq!(project.cache().newest_generation(), NO_GENERATION);

        guest_writes(&first, "npm packages");
        first.settle(GREW);

        assert_eq!(project.cache().newest_generation(), 1);
        let second = project.a_box("second");
        second.prepare(true).unwrap();
        assert_eq!(holds(&second), "npm packages");
    }

    #[test]
    fn a_box_that_added_nothing_publishes_nothing_and_follows_its_project() {
        let project = Project::new();
        let (first, second) = (project.a_box("first"), project.a_box("second"));
        first.prepare(true).unwrap();
        guest_writes(&first, "one");
        first.settle(GREW);
        second.prepare(true).unwrap();
        second.settle(SAME);
        assert_eq!(project.cache().newest_generation(), 1);

        first.prepare(true).unwrap();
        guest_writes(&first, "one two");
        first.settle(GREW);
        assert_eq!(project.cache().newest_generation(), 2);

        second.prepare(true).unwrap();
        assert_eq!(holds(&second), "one two");
    }

    #[test]
    fn of_two_boxes_adding_to_one_generation_the_second_keeps_its_own() {
        let project = Project::new();
        let (first, second) = (project.a_box("first"), project.a_box("second"));
        first.prepare(true).unwrap();
        second.prepare(true).unwrap();
        guest_writes(&first, "from first");
        guest_writes(&second, "from second");

        first.settle(GREW);
        second.settle(GREW);

        assert_eq!(project.cache().newest_generation(), 1);
        second.prepare(true).unwrap();
        assert_eq!(holds(&second), "from second");
        let third = project.a_box("third");
        third.prepare(true).unwrap();
        assert_eq!(holds(&third), "from first");
    }

    #[test]
    fn a_killed_box_keeps_its_disk_and_publishes_it_at_its_next_clean_stop() {
        let project = Project::new();
        let (killed, other) = (project.a_box("killed"), project.a_box("other"));
        other.prepare(true).unwrap();
        guest_writes(&other, "published");
        other.settle(GREW);
        killed.prepare(true).unwrap();
        guest_writes(&killed, "published and more");

        other.prepare(true).unwrap();
        killed.prepare(false).unwrap();
        assert_eq!(holds(&killed), "published and more");

        killed.settle(SAME);
        assert_eq!(project.cache().newest_generation(), 2);
        other.settle(SAME);
        other.prepare(true).unwrap();
        assert_eq!(holds(&other), "published and more");
    }

    #[test]
    fn a_stop_that_said_nothing_about_the_cache_concludes_nothing() {
        let project = Project::new();
        let quiet = project.a_box("quiet");
        quiet.prepare(true).unwrap();
        guest_writes(&quiet, "unreported");
        quiet.settle(None);
        assert_eq!(project.cache().newest_generation(), NO_GENERATION);
        assert_eq!(Lineage::read(&quiet.lineage), Some(Lineage::of(0)));
    }

    #[test]
    fn publishing_leaves_only_the_newest_generation() {
        let project = Project::new();
        let first = project.a_box("first");
        for content in ["one", "one two", "one two three"] {
            first.prepare(true).unwrap();
            guest_writes(&first, content);
            first.settle(GREW);
        }
        assert_eq!(project.cache().generations(), [3]);
    }
}
