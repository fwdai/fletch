//! A project's seed on the macOS sandbox ([`crate::sandbox::seed`]): the large
//! directories git ignores in a workspace's checkouts, which a new checkout of
//! the project starts with, sharing their blocks with every other workspace
//! until it changes them.
//!
//! ```text
//! <checkouts base>/workspace-seeds/<project id>/seed/tree/<checkout>/…   what a workspace kept, where it was
//! <checkouts base>/workspace-seeds/<project id>/seed/made.json           what it was made without, and for which request
//! <checkouts base>/workspace-seeds/<project id>/candidate-<id>/tree/…    what is kept of a workspace, being made
//! <checkouts base>/workspace-seeds/<project id>/wanted/<agent id>        this workspace is to be the next seed
//! ```
//!
//! The store is beside the checkouts, so a clone never crosses a volume, and
//! under no sandbox's write grant. A workspace is at rest while no turn of its
//! agent runs ([`CheckoutsAtRest`]); nothing stops a turn from starting while
//! the workspace is taken, so a workspace in which one started meanwhile is
//! not made the seed. What a seed keeps: [`capture`]. A new checkout merges
//! what the seed keeps for it ([`start`]). Package downloads are not part of
//! it: a project's workspaces on this Mac share its toolchain cache.

mod capture;
mod start;
#[cfg(all(test, target_os = "macos"))]
mod tests;

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;

use crate::error::{Error, Result};
use crate::sandbox::seed::{self, AtRest};
use crate::sandbox::{stamped_engine, EngineKind};
use crate::workspace::{AgentRecord, WorkspaceManager};

pub use start::start;

const SEEDS: &str = "workspace-seeds";
/// In a seed or a candidate: one directory per checkout, named as it is.
const TREE: &str = "tree";
const WANTED: &str = "wanted";

/// The turns each agent started since the app started, by agent id.
static TURNS: Mutex<BTreeMap<String, Turns>> = Mutex::new(BTreeMap::new());

#[derive(Default)]
struct Turns {
    started: u64,
    running: bool,
}

/// Where every project's store is kept, one directory per project id.
pub fn seeds_dir() -> Option<PathBuf> {
    Some(
        crate::workspace::checkouts_root()
            .ok()?
            .with_file_name(SEEDS),
    )
}

/// `None` for a project id that cannot name a directory.
fn store(project_id: &str) -> Option<PathBuf> {
    crate::sandbox::toolchain_cache::project_root_in(&seeds_dir()?, project_id).ok()
}

fn request_file(store: &Path, agent_id: &str) -> PathBuf {
    store.join(WANTED).join(agent_id)
}

/// A turn of `agent_id` started: its workspace, if it is being taken for its
/// project's seed, is not made the seed.
pub fn turn_started(agent_id: &str) {
    let mut agents = TURNS.lock();
    let turns = agents.entry(agent_id.to_string()).or_default();
    turns.started += 1;
    turns.running = true;
}

/// The agent of `record` is not running a turn. When it was, its workspace
/// is offered as its project's seed ([`seed::offer`]).
pub fn turn_ended(record: &AgentRecord) {
    if let Some(started) = end_turn(&record.id) {
        offer(record, started);
    }
}

/// Ends the turn `agent_id` runs, if one does, and answers the turns it
/// started.
fn end_turn(agent_id: &str) -> Option<u64> {
    let mut agents = TURNS.lock();
    let turns = agents.get_mut(agent_id).filter(|turns| turns.running)?;
    turns.running = false;
    Some(turns.started)
}

/// `record`'s work is in its project's base: its workspace is wanted as the
/// seed ([`seed::want`]), and offered now unless a turn runs, whose end
/// offers it.
pub fn merged(record: &AgentRecord) {
    let Some(store) = store(&record.project_id) else {
        return;
    };
    let asked = std::fs::create_dir_all(store.join(WANTED))
        .and_then(|()| seed::want(&request_file(&store, &record.id)));
    if let Err(e) = asked {
        tracing::warn!(agent_id = %record.id, error = %e, "could not ask for a workspace as its project's seed");
        return;
    }
    if let Some(started) = turns_at_rest(&record.id) {
        offer(record, started);
    }
}

/// Forgets `agent_id`, whose workspace goes: its turns, and its request.
pub fn forget(agent_id: &str) {
    TURNS.lock().remove(agent_id);
    let stores = seeds_dir().and_then(|dir| std::fs::read_dir(dir).ok());
    for store in stores.into_iter().flatten().flatten() {
        let _ = std::fs::remove_file(request_file(&store.path(), agent_id));
    }
}

/// At boot, once every project's settings file is read: removes the
/// candidates an earlier run left ([`seed::remove_abandoned_candidates`]) and
/// offers each workspace on this Mac still wanted as its project's seed.
pub fn startup(workspace: &WorkspaceManager) {
    if !cfg!(target_os = "macos") {
        return;
    }
    if let Some(dir) = seeds_dir() {
        seed::remove_abandoned_candidates(&dir);
    }
    let Ok(ids) = workspace.live_agent_ids() else {
        return;
    };
    let records = ids.iter().filter_map(|id| workspace.agent(id).ok());
    for record in records.filter(|record| stamped_engine(record) == EngineKind::SandboxExec) {
        let wanted = store(&record.project_id)
            .is_some_and(|store| seed::is_wanted(&request_file(&store, &record.id)));
        if wanted {
            offer(&record, turns_started(&record.id));
        }
    }
}

fn offer(record: &AgentRecord, turns: u64) {
    if !cfg!(target_os = "macos") {
        return;
    }
    if let Some(at_rest) = CheckoutsAtRest::of(record, turns) {
        offer_off_the_callers_path(at_rest);
    }
}

/// [`seed::offer`] waits for the store's lock, which is held for as long as a
/// checkout starts from the seed, and a turn's end must not wait for that.
fn offer_off_the_callers_path(at_rest: CheckoutsAtRest) {
    crate::host::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || seed::offer(at_rest)).await;
    });
}

fn turns_started(agent_id: &str) -> u64 {
    TURNS.lock().get(agent_id).map_or(0, |turns| turns.started)
}

/// The turns `agent_id` started, unless one runs now.
fn turns_at_rest(agent_id: &str) -> Option<u64> {
    match TURNS.lock().get(agent_id) {
        Some(turns) if turns.running => None,
        Some(turns) => Some(turns.started),
        None => Some(0),
    }
}

/// Removes what is at `path`, a directory with all it holds; a link is
/// removed, never followed.
fn remove(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(entry) if entry.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// The checkouts of a workspace on this Mac with no turn running.
struct CheckoutsAtRest {
    agent_id: String,
    project_id: String,
    /// The agent's directory: one checkout per repository, and Fletch's own.
    workspace: PathBuf,
    store: PathBuf,
    request_file: PathBuf,
    /// The turns the agent had started when the workspace was offered.
    turns: u64,
}

impl CheckoutsAtRest {
    fn of(record: &AgentRecord, turns: u64) -> Option<Self> {
        let store = store(&record.project_id)?;
        Some(Self {
            agent_id: record.id.clone(),
            project_id: record.project_id.clone(),
            workspace: crate::workspace::agent_parent_dir(&record.id).ok()?,
            request_file: request_file(&store, &record.id),
            store,
            turns,
        })
    }
}

impl AtRest for CheckoutsAtRest {
    fn agent_id(&self) -> &str {
        &self.agent_id
    }

    fn project_id(&self) -> &str {
        &self.project_id
    }

    fn store(&self) -> &Path {
        &self.store
    }

    fn request_file(&self) -> &Path {
        &self.request_file
    }

    fn take(&self, candidate: &Path) -> io::Result<()> {
        std::fs::create_dir_all(candidate)
    }

    /// An error when a turn started meanwhile: what was cloned may be half
    /// written.
    async fn reduce(&self, candidate: &Path, excluded: &[String]) -> Result<u64> {
        let tree = candidate.canonicalize()?.join(TREE);
        let kept_kib = capture::keep(&self.workspace, &tree, excluded).await?;
        if turns_started(&self.agent_id) != self.turns {
            return Err(Error::Other(
                "a turn started while the workspace was taken for its project's seed".into(),
            ));
        }
        Ok(kept_kib)
    }
}
