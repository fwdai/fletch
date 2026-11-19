//! Spare cloud boxes: a box at a cloud provider made ahead of any workspace,
//! so that a new workspace's box wakes a machine instead of waiting for one
//! to be made and for its agent CLI to be installed.
//!
//! - **What a spare is**: a whole cloud box with no owner. Its machine and
//!   disk are at the provider; on this machine it is a directory in the
//!   spares root ([`root`], beside the boxes root, so a rename into it is
//!   atomic) holding what a cloud box's directory holds before it first
//!   runs (the ssh key pair, the pinned host key, the handle file) and,
//!   last, its marker ([`READY_FILE`]): it is ready, and what it was built
//!   for ([`Build`]). It is made under a placeholder identity of its own
//!   (`spare-<random>`, never a workspace's id), whose paths its guest binds.
//!   It never enters the box registry: no relays, no idle timer, no mailbox.
//!   Nothing that lists boxes looks in the spares root, the sweep of boxes no
//!   workspace owns included.
//! - **Preparing one** ([`prepare`]) is a cloud box's boot in the spare's
//!   directory (`cloud::bring_up`, `cloud::reach`), then every CLI this
//!   supervisor launches in a box and is signed in to installed on its disk
//!   (`CloudMachine::ensure_cli`), then the machine parked
//!   (`CloudMachine::halt` for an idle stop); the marker is written once the
//!   provider confirmed the park and the ssh master is closed. The installs
//!   run with a home outside the guest's disk ([`PREPARE_HOME`]), so the
//!   guest's home and checkout directories hold nothing of them. A
//!   preparation that fails destroys what it made and removes its directory.
//! - **Fresh or stale**: a spare is fresh while its marker equals what a box
//!   made now would get: the provider, the image, the size, and each CLI at
//!   the version a launch installs. A stale spare, and a directory left with
//!   no marker (a preparation that died), is destroyed at its provider and
//!   its directory removed; a destroy that fails keeps the directory, the
//!   only way back to a billed machine.
//! - **Taking one** ([`take`]) is the first boot of a box with nothing made
//!   for it: under a lock held only for the pick and the rename, a fresh
//!   spare's directory becomes the box's and its marker goes. The boot goes
//!   on as for any box with a handle: its machine is woken, its CLI found on
//!   its disk, and its own paths bound in its guest (`cloud::bind_identity`).
//!   With none ready, a box directory that holds anything, or a rename that
//!   fails, the box is made directly; a spawn never waits for a spare. A
//!   taken machine is its workspace's until the workspace is archived, and
//!   is destroyed then like any box's: nothing returns to the spares.
//! - **How many** is a setting (`settings::cloud_spares`), per connected
//!   provider. [`replenish`] brings each connected provider's spares to it,
//!   one run at a time; it runs at startup, after a take, and when the
//!   setting changes, and a failure waits for the next of those.
//!   [`destroy_all`] destroys every spare, of one provider or all.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fletch_box::Channel;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::agent::AuthStatus;
use crate::cloud::{Client, Handle, ProviderId};
use crate::error::{Error, Result};

use super::cloud::{self, CloudMachine};
use super::registry::{BoxIdentity, StopReason};
use super::settings;

/// A spare's marker, written last: the [`Build`] it was made for.
const READY_FILE: &str = "ready.json";

/// What every spare's placeholder id starts with. A workspace's id is a
/// place name (`names`), which never does.
const ID_PREFIX: &str = "spare-";

/// The home a preparation's CLI installs run with: a directory on the
/// machine's own root, not on its disk, so whatever an install or a CLI's
/// first run writes there is in neither the home nor the checkout directory
/// the workspace that takes the spare finds.
const PREPARE_HOME: &str = "/tmp";

/// Held for the pick and the rename of a take, and for the marker's removal
/// when a spare is retired, so that a spare goes to one taker or to the
/// sweep, never to both. Never held across a network call.
static TAKING: Mutex<()> = Mutex::new(());

/// The one [`replenish`] that may run at a time.
static REPLENISHING: Mutex<Flight> = Mutex::new(Flight::new());

/// Where the spares are kept, one directory each: beside the boxes, on the
/// same volume, so a spare becomes a box by a rename.
pub fn root() -> Result<PathBuf> {
    Ok(super::boxes_root()?.with_file_name("box-spares"))
}

/// What a spare was built for, as its marker records it, and what a box made
/// now would get ([`Build::now`]); the two equal is a fresh spare.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Build {
    /// The provider's id (`ProviderId::as_str`).
    provider: String,
    /// The cloud image the machine runs (`cloud::image`).
    image: String,
    cpus: u32,
    memory_mib: u32,
    disk_gib: u32,
    /// Each CLI on the disk, by provider, at its version.
    clis: BTreeMap<String, String>,
}

impl Build {
    /// What a box made now at `provider` gets: the cloud image, the size the
    /// box settings give, and each of `clis` at the version a launch of it
    /// installs (`registry::tools_version`).
    fn now(provider: ProviderId, clis: &[String]) -> Self {
        let knobs = settings::current();
        Self {
            provider: provider.as_str().to_string(),
            image: cloud::image(),
            cpus: u32::from(knobs.cpus),
            memory_mib: knobs.memory_mib,
            disk_gib: knobs.disk_gib,
            clis: clis
                .iter()
                .map(|cli| (cli.clone(), super::registry::tools_version(cli)))
                .collect(),
        }
    }

    fn provider(&self) -> Option<ProviderId> {
        ProviderId::parse(&self.provider)
    }
}

/// The CLIs a spare carries: every provider this supervisor launches in a
/// box (`boxes::supports_provider`) and is signed in to here
/// (`agent::probe_all_provider_auth`).
async fn wanted_clis() -> Vec<String> {
    crate::agent::probe_all_provider_auth()
        .await
        .into_iter()
        .filter(|probe| probe.status == AuthStatus::SignedIn && super::supports_provider(&probe.id))
        .map(|probe| probe.id)
        .collect()
}

/// One directory in the spares root, as read now.
struct Found {
    dir: PathBuf,
    handle: Result<Option<Handle>>,
    built: Option<Build>,
}

/// Every spare directory in `root`. Anything else there is left alone.
fn found_in(root: &Path) -> Vec<Found> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(ID_PREFIX))
        .map(|entry| entry.path())
        .filter(|dir| dir.is_dir())
        .map(|dir| Found {
            handle: cloud::read_handle(&dir),
            built: read_built(&dir),
            dir,
        })
        .collect()
}

/// The marker in `dir`, `None` when it has none or one that does not read.
fn read_built(dir: &Path) -> Option<Build> {
    serde_json::from_slice(&std::fs::read(dir.join(READY_FILE)).ok()?).ok()
}

/// Write `build` as the marker in `dir`, whole: a reader finds it or not.
fn write_ready(dir: &Path, build: &Build) -> Result<()> {
    let staging = dir.join(format!("{READY_FILE}.building"));
    std::fs::write(&staging, serde_json::to_vec(build)?)?;
    std::fs::rename(&staging, dir.join(READY_FILE))?;
    Ok(())
}

/// Whether a box directory is one nothing was made for: absent, or empty.
fn vacant(dir: &Path) -> bool {
    match std::fs::read_dir(dir) {
        Ok(mut entries) => entries.next().is_none(),
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}

/// Give the box whose directory is `box_dir`, at `provider`, a fresh ready
/// spare if there is one and nothing was made for the box yet: the spare's
/// directory becomes the box's. Otherwise the box is made directly. Either
/// way, where there were spares, a replenish follows in the background: the
/// taken one's replacement, or the replacements of spares made for what
/// boxes are no longer made with.
pub(super) async fn take(provider: ProviderId, box_dir: &Path) {
    if !vacant(box_dir) {
        return;
    }
    let Ok(root) = root() else {
        return;
    };
    if found_in(&root).is_empty() {
        return;
    }
    let build = Build::now(provider, &wanted_clis().await);
    take_in(&root, box_dir, &build);
    replenish_in_background();
}

/// [`take`] from `root`, of a spare built as `build`; whether one was taken.
fn take_in(root: &Path, box_dir: &Path, build: &Build) -> bool {
    let _taking = TAKING.lock();
    if !vacant(box_dir) {
        return false;
    }
    let Some(spare) = found_in(root).into_iter().find(|found| {
        found.built.as_ref() == Some(build)
            && matches!(&found.handle, Ok(Some(handle)) if handle.provider().as_str() == build.provider)
    }) else {
        return false;
    };
    if let Some(parent) = box_dir.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!(box_dir = %box_dir.display(), error = %e, "a spare cloud box could not be taken");
            return false;
        }
    }
    if let Err(e) = std::fs::rename(&spare.dir, box_dir) {
        tracing::warn!(
            spare = %spare.dir.display(),
            box_dir = %box_dir.display(),
            error = %e,
            "a spare cloud box could not be taken; the box is made directly"
        );
        return false;
    }
    if let Err(e) = std::fs::remove_file(box_dir.join(READY_FILE)) {
        tracing::debug!(box_dir = %box_dir.display(), error = %e, "a taken spare's marker stayed");
    }
    tracing::info!(
        spare = %spare.dir.display(),
        box_dir = %box_dir.display(),
        "a new cloud box took a spare"
    );
    true
}

/// What preparing a spare does with its guest: reach it once its machine
/// runs, and install a CLI on its disk. [`Ssh`] is the engine's; a test
/// stands in for it.
trait Guest: Sync {
    fn reach(
        &self,
        dir: &Path,
        key: &Path,
        handle: &Handle,
    ) -> impl Future<Output = Result<Channel>> + Send;

    fn install(
        &self,
        machine: &CloudMachine,
        channel: &Channel,
        cli: &str,
    ) -> impl Future<Output = Result<()>> + Send;
}

/// The spare's own guest, over ssh.
struct Ssh;

impl Guest for Ssh {
    async fn reach(&self, dir: &Path, key: &Path, handle: &Handle) -> Result<Channel> {
        cloud::reach(dir, key, handle).await
    }

    async fn install(&self, machine: &CloudMachine, channel: &Channel, cli: &str) -> Result<()> {
        machine
            .ensure_cli(channel, Path::new(PREPARE_HOME), cli, &|_| {})
            .await
    }
}

/// Prepare one spare of `build` in `root`, at its provider through a client
/// from `client_for`, its guest worked through `guest`; answers its
/// directory. A failure destroys what was made and removes the directory,
/// which is kept only when the destroy fails.
async fn prepare(
    root: &Path,
    build: &Build,
    client_for: &(impl Fn(ProviderId) -> Result<Client> + Sync),
    guest: &impl Guest,
) -> Result<PathBuf> {
    let id = format!(
        "{ID_PREFIX}{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    );
    let dir = root.join(&id);
    std::fs::create_dir_all(&dir)?;
    let made = async {
        let provider = build.provider().ok_or_else(|| {
            Error::Other(format!(
                "no cloud provider \"{}\" in this build",
                build.provider
            ))
        })?;
        make(&dir, &id, Arc::new(client_for(provider)?), build, guest).await
    }
    .await;
    let Err(e) = made else {
        return Ok(dir);
    };
    match cloud::destroy_box_with(&dir, client_for).await {
        Ok(()) => {
            if let Err(removed) = tokio::fs::remove_dir_all(&dir).await {
                tracing::warn!(spare = %dir.display(), error = %removed, "a failed spare's directory stayed");
            }
        }
        Err(destroyed) => tracing::warn!(
            spare = %dir.display(),
            error = %destroyed,
            "a spare cloud box whose preparation failed could not be destroyed; its directory is \
             kept until it can be"
        ),
    }
    Err(e)
}

/// [`prepare`]'s work in `dir`, for the placeholder `id`: the box brought up,
/// each CLI installed, the machine parked, the marker written.
async fn make(
    dir: &Path,
    id: &str,
    client: Arc<Client>,
    build: &Build,
    guest: &impl Guest,
) -> Result<()> {
    let identity = BoxIdentity::new(id, "", "")?;
    let (machine, key) = cloud::bring_up(client, &identity, dir, &|_| {}).await?;
    let channel = guest.reach(dir, &key, machine.handle()).await?;
    let filled = async {
        for cli in build.clis.keys() {
            guest.install(&machine, &channel, cli).await?;
        }
        let parked = machine.halt(&channel, StopReason::Idle, None).await;
        if !parked.clean() {
            return Err(Error::Other(
                "the provider did not confirm the spare's machine at rest".into(),
            ));
        }
        Ok(())
    }
    .await;
    if let Err(e) = channel.close_master().await {
        tracing::debug!(spare = id, error = %e, "ending a spare's ssh master failed");
    }
    filled?;
    write_ready(dir, build)
}

/// Take the spare in `dir` out of the spares, so no taker picks it from here
/// on, then destroy its box at its provider and remove the directory. A
/// destroy that fails keeps the directory. Nothing to do for a spare a taker
/// has already.
async fn retire(
    dir: &Path,
    client_for: &(impl Fn(ProviderId) -> Result<Client> + Sync),
) -> Result<()> {
    {
        let _taking = TAKING.lock();
        if !dir.exists() {
            return Ok(());
        }
        match std::fs::remove_file(dir.join(READY_FILE)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    cloud::destroy_box_with(dir, client_for).await?;
    match tokio::fs::remove_dir_all(dir).await {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// Bring each connected provider's spares to the number the setting asks
/// for: stale and unfinished ones destroyed, any past the number destroyed,
/// and what is missing prepared. One run at a time: a call while one runs
/// has that run go once more when it is done. Nothing for a provider that is
/// not connected. A failure is logged and left for the next call.
pub async fn replenish() {
    single_flight(&REPLENISHING, replenish_once).await;
}

/// [`replenish`] in a task of its own.
pub fn replenish_in_background() {
    drop(crate::host::spawn(replenish()));
}

/// One run of [`replenish`], against this machine's spares root, connected
/// providers, logins and settings.
async fn replenish_once() {
    let Ok(root) = root() else {
        return;
    };
    let count = settings::cloud_spares();
    if count == 0 && found_in(&root).is_empty() {
        return;
    }
    let connected: Vec<ProviderId> = ProviderId::ALL
        .iter()
        .copied()
        .filter(|&provider| matches!(cloud::connection(provider), Ok(Some(_))))
        .collect();
    if connected.is_empty() {
        return;
    }
    let clis = wanted_clis().await;
    let builds: Vec<Build> = connected
        .into_iter()
        .map(|provider| Build::now(provider, &clis))
        .collect();
    replenish_with(&root, &builds, count, &cloud::client, &Ssh).await;
}

/// [`replenish_once`] in `root`, keeping `count` spares for each of
/// `builds` (one per connected provider, as a box made now there gets).
async fn replenish_with(
    root: &Path,
    builds: &[Build],
    count: u32,
    client_for: &(impl Fn(ProviderId) -> Result<Client> + Sync),
    guest: &impl Guest,
) {
    if builds.is_empty() {
        return;
    }
    let mut fresh: HashMap<&str, Vec<PathBuf>> = HashMap::new();
    for found in found_in(root) {
        let handle = match found.handle {
            Ok(Some(handle)) => handle,
            // Nothing was made at a provider: a preparation that died before.
            Ok(None) => {
                if let Err(e) = tokio::fs::remove_dir_all(&found.dir).await {
                    tracing::warn!(spare = %found.dir.display(), error = %e, "removing an unfinished spare failed");
                }
                continue;
            }
            Err(e) => {
                tracing::warn!(spare = %found.dir.display(), error = %e, "a spare's handle does not read; it is kept");
                continue;
            }
        };
        let Some(build) = builds
            .iter()
            .find(|build| build.provider == handle.provider().as_str())
        else {
            continue;
        };
        if found.built.as_ref() == Some(build) {
            fresh
                .entry(build.provider.as_str())
                .or_default()
                .push(found.dir);
            continue;
        }
        match retire(&found.dir, client_for).await {
            Ok(()) => tracing::info!(
                spare = %found.dir.display(),
                ready = found.built.is_some(),
                "destroyed a spare cloud box that was stale or unfinished"
            ),
            Err(e) => tracing::warn!(
                spare = %found.dir.display(),
                error = %e,
                "a stale or unfinished spare cloud box could not be destroyed; it is kept"
            ),
        }
    }
    let count = count as usize;
    for build in builds {
        let ready = fresh.remove(build.provider.as_str()).unwrap_or_default();
        for surplus in ready.iter().skip(count) {
            match retire(surplus, client_for).await {
                Ok(()) => {
                    tracing::info!(spare = %surplus.display(), "destroyed a spare cloud box past the number kept")
                }
                Err(e) => tracing::warn!(
                    spare = %surplus.display(),
                    error = %e,
                    "a spare cloud box past the number kept could not be destroyed; it is kept"
                ),
            }
        }
        for _ in ready.len()..count {
            match prepare(root, build, client_for, guest).await {
                Ok(dir) => tracing::info!(spare = %dir.display(), "a spare cloud box is ready"),
                Err(e) => {
                    tracing::warn!(provider = build.provider, error = %e, "preparing a spare cloud box failed");
                    break;
                }
            }
        }
    }
}

/// Destroy every spare of `provider`, or of every provider, at its provider,
/// and remove its directory. A spare that cannot be destroyed keeps its
/// directory and makes this an error, once every other was tried.
pub async fn destroy_all(provider: Option<ProviderId>) -> Result<()> {
    destroy_all_in(&root()?, provider, &cloud::client).await
}

/// [`destroy_all`] in `root`, with clients from `client_for`. A directory
/// with no handle belongs to no provider: it goes when every provider's
/// spares go, and is left alone when one provider's do.
async fn destroy_all_in(
    root: &Path,
    provider: Option<ProviderId>,
    client_for: &(impl Fn(ProviderId) -> Result<Client> + Sync),
) -> Result<()> {
    let mut kept = Vec::new();
    for found in found_in(root) {
        let ours = match &found.handle {
            Ok(Some(handle)) => provider.map_or(true, |p| handle.provider() == p),
            Ok(None) => provider.is_none(),
            Err(e) => {
                kept.push(format!("{}: {e}", found.dir.display()));
                continue;
            }
        };
        if !ours {
            continue;
        }
        if let Err(e) = retire(&found.dir, client_for).await {
            kept.push(format!("{}: {e}", found.dir.display()));
        }
    }
    if kept.is_empty() {
        return Ok(());
    }
    Err(Error::Other(format!(
        "{} spare cloud box(es) could not be destroyed and are kept: {}",
        kept.len(),
        kept.join("; ")
    )))
}

/// Whether a run of something single-flight is under way, and whether it was
/// asked for again meanwhile.
struct Flight {
    running: bool,
    again: bool,
}

impl Flight {
    const fn new() -> Self {
        Self {
            running: false,
            again: false,
        }
    }
}

/// Run `work` unless a run under `flight` is already under way, in which case
/// that run does it once more when it finishes: never two at once, and no
/// call left without a run that started after it.
async fn single_flight<F, Fut>(flight: &Mutex<Flight>, work: F)
where
    F: Fn() -> Fut,
    Fut: Future<Output = ()>,
{
    {
        let mut state = flight.lock();
        if state.running {
            state.again = true;
            return;
        }
        state.running = true;
    }
    // A run dropped part way leaves the flight free for the next call.
    let grounded = Grounded(flight);
    loop {
        work().await;
        let mut state = flight.lock();
        if !state.again {
            state.running = false;
            break;
        }
        state.again = false;
    }
    std::mem::forget(grounded);
}

/// Frees a [`Flight`] whose run was dropped before it finished.
struct Grounded<'a>(&'a Mutex<Flight>);

impl Drop for Grounded<'_> {
    fn drop(&mut self) {
        *self.0.lock() = Flight::new();
    }
}

#[cfg(test)]
#[path = "spares_tests.rs"]
mod tests;
