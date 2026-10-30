//! One entry per box-stamped agent: its identity (the paths the guest mirrors),
//! its phase, and while the VM runs the channel, proxy and relays. Process-wide
//! like the container engines.
//!
//! A box's writable root is routed to its executor for as long as the registry
//! knows the box, not only while the VM runs: a routed operation on a stopped
//! box (reading a file for the file panel) boots it again instead of failing
//! on a host path that holds nothing; a [passive](crate::exec::passive) one
//! (a poll's `git status`) fails instead. Unregistering happens on
//! [`BoxRegistry::discard`].
//!
//! A box also has *tenants* ([`Tenant`]): agents with no box of their own that
//! run in this one because the tree they adopted lives here — the local
//! reviewer of the box's owner. A tenant's own writable root is routed to the
//! host's box the same way, so what the host writes there for the launch (the
//! profile) lands where the tenant runs; its mailbox gets a relay of its own.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use fletch_box::channel;
use fletch_box::disks::{self, Dirty};
use fletch_box::egress::{EgressEvent, EgressPolicy, EgressProxy};
use fletch_box::tools::{self, DiskMarker, DiskState, FoundDisk};
use fletch_box::{guest, Channel, StopOutcome, Vm, VmSpec};
use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::watch;

use crate::codegraph::CODEGRAPH_VERSION;
use crate::error::{Error, Result};
use crate::workspace::{AgentRecord, FolderGrant};

use super::executor::BoxExecutor;
use super::{relay, settings, setup, tools_root, EgressEventPayload, EVENT_STATE, EVENT_TOOLS};

pub const DATA_DISK: &str = "data.img";
pub const CONSOLE: &str = "console.log";
pub const SSH_SOCK: &str = "ssh.sock";
pub const CTL_SOCK: &str = "ctl.sock";
pub const EGRESS_SOCK: &str = "egress.sock";
pub const MIRROR_DIR: &str = "mirror.git";

/// `FLETCH_READY` must arrive within this, else the boot fails closed.
const READY_TIMEOUT: Duration = Duration::from_secs(10);
/// VMs started at once ([`BoxRegistry::boot_slot`]); further boots queue.
const MAX_CONCURRENT_BOOTS: usize = 2;
/// The `Preparing` detail of a box waiting for a boot slot.
const WAITING_FOR_SLOT: &str = "waiting for another box to start";
/// The stop ladder's rungs (§7.7).
const CONTROL_GRACE: Duration = Duration::from_secs(10);
const SYSRQ_GRACE: Duration = Duration::from_secs(5);
const TERM_GRACE: Duration = Duration::from_secs(3);
const KILL_GRACE: Duration = Duration::from_secs(5);
/// A [`StopReason::Shutdown`] stop's bound on the lifecycle lock wait, on the
/// sessions pull, and from `send_stop` on, on the clean rungs together; then
/// TERM/KILL.
const SHUTDOWN_GRACE: Duration = CONTROL_GRACE;

/// The paths one box mirrors from the host, and who it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxIdentity {
    pub agent_id: String,
    pub project_id: String,
    pub provider: String,
    pub writable_root: PathBuf,
    pub rpc_dir: PathBuf,
    pub home: PathBuf,
    /// The folders the user granted the agent, mounted from virtio-fs devices
    /// at their own paths, parents first ([`Self::with_shares`]). A device
    /// can only be added at boot, so a box whose shares differ from its
    /// agent's grants is restarted by the next launch.
    pub shares: Vec<FolderGrant>,
}

impl BoxIdentity {
    /// The identity of `agent_id`'s box, derived the way the launch context
    /// derives it: the writable root is the agent's parent dir, the mailbox
    /// its rpc dir, the home the user's.
    pub fn new(agent_id: &str, project_id: &str, provider: &str) -> Result<Self> {
        Ok(Self {
            agent_id: agent_id.to_string(),
            project_id: project_id.to_string(),
            provider: provider.to_string(),
            writable_root: crate::workspace::agent_parent_dir(agent_id)?,
            rpc_dir: crate::rpc::mailbox_dir(agent_id)?,
            home: dirs::home_dir()
                .ok_or_else(|| Error::Other("HOME directory not available".into()))?,
            shares: Vec::new(),
        })
    }

    pub fn for_record(record: &AgentRecord) -> Result<Self> {
        Ok(Self::new(&record.id, &record.project_id, &record.provider)?
            .with_shares(&record.folder_grants))
    }

    /// This identity sharing `grants` that still exist, in mount order: a
    /// folder before the folders inside it (fewer components first), so a
    /// nested mount lands on top of its parent rather than under it.
    pub fn with_shares(mut self, grants: &[FolderGrant]) -> Self {
        let mut shares = crate::workspace::folder_grants::present(grants);
        shares.sort_by_cached_key(|g| (g.path.components().count(), g.path.clone()));
        self.shares = shares;
        self
    }

    pub fn box_dir(&self) -> Result<PathBuf> {
        super::box_dir(&self.agent_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxPhase {
    Stopped,
    /// Waiting for the host's setup before the first boot: `detail` is what
    /// for ("downloading image 43%", "installing Claude Code 2.1.312").
    Preparing {
        detail: String,
    },
    Starting,
    /// `since` is unix milliseconds, for the UI.
    Running {
        since: i64,
    },
    Stopping,
    Failed {
        reason: String,
    },
}

impl BoxPhase {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Preparing { .. } => "preparing",
            Self::Starting => "starting",
            Self::Running { .. } => "running",
            Self::Stopping => "stopping",
            Self::Failed { .. } => "failed",
        }
    }
}

/// Why a box is being stopped; decides whether the mirror is refreshed first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// No launch attached for `box_idle_stop_minutes`.
    Idle,
    /// `box_stop` from the UI.
    User,
    /// The box is being deleted; nothing to mirror.
    Discard,
    /// App exit.
    Shutdown,
    /// A launch needs the tools disk that was built since this box booted.
    Reattach,
    /// A launch needs the folders the user shares with this box now, which
    /// differ from the ones it booted with.
    Reshare,
}

/// `box:state`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BoxStateReport {
    pub agent_id: String,
    pub phase: &'static str,
    pub since: Option<i64>,
    pub reason: Option<String>,
    /// What a `preparing` box waits on.
    pub detail: Option<String>,
}

impl BoxStateReport {
    fn new(agent_id: &str, phase: &BoxPhase) -> Self {
        Self {
            agent_id: agent_id.to_string(),
            phase: phase.name(),
            since: match phase {
                BoxPhase::Running { since } => Some(*since),
                _ => None,
            },
            reason: match phase {
                BoxPhase::Failed { reason } => Some(reason.clone()),
                _ => None,
            },
            detail: match phase {
                BoxPhase::Preparing { detail } => Some(detail.clone()),
                _ => None,
            },
        }
    }
}

/// One provider's tools disk: the `box:tools` event and a row of
/// `box_runtime_probe`. `phase` is `missing` | `building` | `ready` | `failed`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ToolsStatus {
    pub provider: String,
    pub version: String,
    pub phase: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// With `failed`: until when (unix ms) failed builds hold a launch's or
    /// the prebuild's next build off ([`tools_backoff`]). Nothing builds at
    /// that time by itself; absent once it has passed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<i64>,
}

/// Where one tools disk build stands, as its waiters see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolsBuild {
    Building,
    Ready,
    Failed(String),
}

/// A box with its VM up.
pub struct RunningBox {
    pub identity: BoxIdentity,
    pub dir: PathBuf,
    pub channel: Arc<Channel>,
    pub since: i64,
    /// The provider's CLI disk, held against the tools sweep for as long as
    /// this box lives, its stop included.
    tools_lease: Option<ToolsLease>,
    vm: Vm,
    proxy: Mutex<Option<EgressProxy>>,
    relays: relay::Relays,
    dirty: Dirty,
}

impl RunningBox {
    pub fn agent_id(&self) -> &str {
        &self.identity.agent_id
    }

    /// The provider's CLI disk, when one was built by boot time.
    pub fn tools_disk(&self) -> Option<&Path> {
        self.tools_lease.as_ref().map(|lease| lease.path.as_path())
    }

    /// `box:<path>` for git over the channel.
    pub fn git_url(&self, guest_path: &Path) -> String {
        format!("{}:{}", guest::SSH_HOST_ALIAS, guest_path.display())
    }

    /// Relay a tenant's mailbox out of this box too (`relay::Relays::add_mailbox`).
    pub fn relay_mailbox(&self, rpc_dir: &Path, tenant_agent_id: &str) {
        self.relays
            .add_mailbox(self.channel.clone(), &self.dir, rpc_dir, tenant_agent_id);
    }

    /// Tail a tenant's sessions dir out of this box into the tenant's own
    /// mirror too (`relay::Relays::add_tail`).
    pub fn relay_tail(&self, guest_sessions: &Path, mirror: &Path) {
        self.relays
            .add_tail(self.channel.clone(), guest_sessions, mirror);
    }
}

/// An agent that runs in another agent's box because the tree it adopted
/// lives there (`exec::for_path` routes its own writable root there too).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tenant {
    /// The agent whose box it runs in.
    pub host_agent_id: String,
    /// The tenant's own writable root, routed to the host's box.
    pub writable_root: PathBuf,
}

struct Entry {
    identity: BoxIdentity,
    phase: BoxPhase,
    running: Option<Arc<RunningBox>>,
    /// Launch ids with a live session; the idle timer arms when this empties.
    attached: HashSet<String>,
    /// Bumped by every attach/detach/boot, so a sleeping idle timer whose
    /// generation moved on does nothing.
    idle_gen: u64,
    /// Serializes boots and stops of this one box.
    lifecycle: Arc<tokio::sync::Mutex<()>>,
    /// Set by [`BoxRegistry::discard`]: no boot starts from then on.
    discarding: bool,
}

impl Entry {
    fn new(identity: BoxIdentity) -> Self {
        Self {
            identity,
            phase: BoxPhase::Stopped,
            running: None,
            attached: HashSet::new(),
            idle_gen: 0,
            lifecycle: Arc::new(tokio::sync::Mutex::new(())),
            discarding: false,
        }
    }
}

pub struct BoxRegistry {
    entries: Mutex<HashMap<String, Entry>>,
    /// Agents that run in another agent's box, by their own id ([`Tenant`]).
    tenants: Mutex<HashMap<String, Tenant>>,
    /// The image is hashed once per process before the first boot
    /// (`ImageStore::verify`); a damaged image is removed and reported.
    image_verified: Mutex<HashSet<String>>,
    /// The tools disk build running or last run per `<provider>-<version>`:
    /// a launch waits on a running one, the UI reads a failed one, and the
    /// failures since the last success hold the next build off.
    tools_builds: Builds,
    /// Tools disks in use ([`ToolsLease`]), with how many boxes use each.
    tools_leases: Leases,
    /// Tools disks hashed against their marker this process, or being
    /// hashed, or built by it: by path and the marker's hash
    /// ([`BoxRegistry::verify_tools_disks`]).
    tools_verified: Verified,
    /// Set by [`BoxRegistry::close`] (app exit): no box boots from then on.
    closing: AtomicBool,
    /// [`MAX_CONCURRENT_BOOTS`] permits, one held per VM start.
    boots: tokio::sync::Semaphore,
}

type Leases = Arc<Mutex<HashMap<PathBuf, usize>>>;
type Builds = Arc<Mutex<HashMap<String, ToolsBuildSlot>>>;
type Verified = Arc<Mutex<HashSet<(PathBuf, String)>>>;

/// One `<provider>-<version>`'s tools disk builds.
#[derive(Debug)]
struct ToolsBuildSlot {
    /// The build running, or the last one's outcome.
    build: watch::Receiver<ToolsBuild>,
    /// The failures in a row since the last success; `None` after one, after
    /// a user's Retry ([`BoxRegistry::reset_tools_backoff`]), and for a key
    /// no build of has failed. A new CLI version is a new key: it starts clean.
    failures: Option<BuildFailures>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BuildFailures {
    count: u32,
    /// No launch or prebuild starts a build of the key before this.
    retry_at: Instant,
}

/// How long the `failures`-th failed build in a row holds the next one off.
/// A persistent failure (the registry down, an install script the CLI
/// outgrew) otherwise rebuilt ~4 GB on every launch; with growing waits,
/// later launches still try again, and the user's Retry skips the wait.
fn tools_backoff(failures: u32) -> Duration {
    const MINUTE: Duration = Duration::from_secs(60);
    match failures {
        0 | 1 => MINUTE,
        2 => 5 * MINUTE,
        3 => 30 * MINUTE,
        _ => 120 * MINUTE,
    }
}

/// Claim `key` for a new build at `now`: registers it, the failures before it
/// kept, and hands back the sender of its outcome. `Err` with the receiver to
/// follow instead while a build of `key` runs, or while failed ones hold the
/// next off ([`tools_backoff`]).
fn claim_tools_build(
    builds: &mut HashMap<String, ToolsBuildSlot>,
    key: &str,
    now: Instant,
) -> std::result::Result<watch::Sender<ToolsBuild>, watch::Receiver<ToolsBuild>> {
    let failures = match builds.get(key) {
        Some(slot) if *slot.build.borrow() == ToolsBuild::Building => {
            return Err(slot.build.clone())
        }
        Some(slot) if slot.failures.is_some_and(|f| now < f.retry_at) => {
            return Err(slot.build.clone())
        }
        Some(slot) => slot.failures,
        None => None,
    };
    let (tx, rx) = watch::channel(ToolsBuild::Building);
    builds.insert(
        key.to_string(),
        ToolsBuildSlot {
            build: rx,
            failures,
        },
    );
    Ok(tx)
}

/// Record how `key`'s build ended at `now`: a success clears its failures, a
/// failure adds one and holds the next build off ([`tools_backoff`]).
/// Answers until when, after a failure.
fn record_tools_build(builds: &Builds, key: &str, ok: bool, now: Instant) -> Option<Instant> {
    let mut builds = builds.lock();
    let slot = builds.get_mut(key)?;
    slot.failures = (!ok).then(|| {
        let count = slot.failures.map_or(1, |f| f.count + 1);
        BuildFailures {
            count,
            retry_at: now + tools_backoff(count),
        }
    });
    slot.failures.map(|f| f.retry_at)
}

/// `at` as unix milliseconds for the UI, while it is ahead of `now`.
fn unix_ms_ahead(at: Instant, now: Instant) -> Option<i64> {
    let ahead = at.checked_duration_since(now).filter(|d| !d.is_zero())?;
    Some(chrono::Utc::now().timestamp_millis() + ahead.as_millis() as i64)
}

/// Puts a box back to `Stopped` when its wait for a boot slot is abandoned,
/// so it does not report waiting forever.
struct StopOnDrop<'a> {
    registry: &'a BoxRegistry,
    agent_id: &'a str,
}

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.registry.set_phase(self.agent_id, BoxPhase::Stopped);
    }
}

/// One box's hold on its tools disk, from the moment [`BoxRegistry::tools_disk`]
/// picks it until its [`RunningBox`] is dropped or its boot fails. The sweep
/// never deletes a leased disk, and picking and leasing happen under the lock
/// the sweep holds, so a disk handed to a boot cannot vanish before it opens.
#[derive(Debug)]
struct ToolsLease {
    leases: Leases,
    path: PathBuf,
}

impl Drop for ToolsLease {
    fn drop(&mut self) {
        let mut leases = self.leases.lock();
        if let Some(count) = leases.get_mut(&self.path) {
            *count -= 1;
            if *count == 0 {
                leases.remove(&self.path);
            }
        }
    }
}

impl BoxRegistry {
    pub fn global() -> &'static BoxRegistry {
        static REGISTRY: OnceLock<BoxRegistry> = OnceLock::new();
        REGISTRY.get_or_init(|| BoxRegistry {
            entries: Mutex::new(HashMap::new()),
            tenants: Mutex::new(HashMap::new()),
            image_verified: Mutex::new(HashSet::new()),
            tools_builds: Builds::default(),
            tools_leases: Leases::default(),
            tools_verified: Verified::default(),
            closing: AtomicBool::new(false),
            boots: tokio::sync::Semaphore::new(MAX_CONCURRENT_BOOTS),
        })
    }

    /// Make `tenant_agent_id` a tenant of `host_agent_id`'s box: its own
    /// writable root (where the host writes its profile — skills, the MCP
    /// config) is routed to that box, so those land in the guest the tenant
    /// runs in. Idempotent; a tenant moving to another host is re-routed.
    pub fn register_tenant(
        &self,
        tenant_agent_id: &str,
        writable_root: PathBuf,
        host_agent_id: &str,
    ) {
        let tenant = Tenant {
            host_agent_id: host_agent_id.to_string(),
            writable_root: writable_root.clone(),
        };
        let previous = self
            .tenants
            .lock()
            .insert(tenant_agent_id.to_string(), tenant);
        if let Some(previous) = previous.filter(|p| p.writable_root != writable_root) {
            crate::exec::unregister_root(&previous.writable_root);
        }
        crate::exec::register_root(writable_root, BoxExecutor::new(host_agent_id));
    }

    /// The host of `tenant_agent_id`, when it is a tenant.
    pub fn tenant_of(&self, tenant_agent_id: &str) -> Option<Tenant> {
        self.tenants.lock().get(tenant_agent_id).cloned()
    }

    /// Stop routing `tenant_agent_id`'s writable root; a no-op for an agent
    /// that is no tenant. `true` when it was one.
    pub fn forget_tenant(&self, tenant_agent_id: &str) -> bool {
        match self.tenants.lock().remove(tenant_agent_id) {
            Some(tenant) => {
                crate::exec::unregister_root(&tenant.writable_root);
                true
            }
            None => false,
        }
    }

    /// Forget every tenant of `host_agent_id`'s box: the box is going.
    /// The tenants of `host_agent_id`'s box, by agent id.
    pub fn tenants_of(&self, host_agent_id: &str) -> Vec<(String, Tenant)> {
        self.tenants
            .lock()
            .iter()
            .filter(|(_, t)| t.host_agent_id == host_agent_id)
            .map(|(id, t)| (id.clone(), t.clone()))
            .collect()
    }

    fn forget_tenants_of(&self, host_agent_id: &str) {
        let gone: Vec<Tenant> = {
            let mut tenants = self.tenants.lock();
            let ids: Vec<String> = tenants
                .iter()
                .filter(|(_, t)| t.host_agent_id == host_agent_id)
                .map(|(id, _)| id.clone())
                .collect();
            ids.iter().filter_map(|id| tenants.remove(id)).collect()
        };
        for tenant in gone {
            crate::exec::unregister_root(&tenant.writable_root);
        }
    }

    /// Know `identity`'s box and route its writable root to it. Idempotent;
    /// a changed identity for a known agent replaces the old one.
    pub fn register(&self, identity: BoxIdentity) {
        let mut entries = self.entries.lock();
        let entry = entries
            .entry(identity.agent_id.clone())
            .or_insert_with(|| Entry::new(identity.clone()));
        entry.identity = identity.clone();
        crate::exec::register_root(
            identity.writable_root.clone(),
            BoxExecutor::new(&identity.agent_id),
        );
    }

    pub fn knows(&self, agent_id: &str) -> bool {
        self.entries.lock().contains_key(agent_id)
    }

    pub fn state(&self, agent_id: &str) -> BoxPhase {
        self.entries
            .lock()
            .get(agent_id)
            .map_or(BoxPhase::Stopped, |e| e.phase.clone())
    }

    pub fn report(&self, agent_id: &str) -> BoxStateReport {
        BoxStateReport::new(agent_id, &self.state(agent_id))
    }

    pub fn running_ids(&self) -> Vec<String> {
        self.entries
            .lock()
            .iter()
            .filter(|(_, e)| e.running.is_some())
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// The box of `agent_id` while its VM is up; never boots it.
    pub(super) fn running(&self, agent_id: &str) -> Option<Arc<RunningBox>> {
        self.entries.lock().get(agent_id)?.running.clone()
    }

    /// The identity of the box whose writable root holds `path`, if any.
    pub fn identity_for_path(&self, path: &Path) -> Option<BoxIdentity> {
        self.entries
            .lock()
            .values()
            .find(|e| path.starts_with(&e.identity.writable_root))
            .map(|e| e.identity.clone())
    }

    /// The channel of a running box; an error for a stopped or unknown one.
    pub fn channel_for_running(&self, agent_id: &str) -> Result<Arc<Channel>> {
        self.running(agent_id)
            .map(|r| r.channel.clone())
            .ok_or_else(|| Error::Other(format!("box {agent_id} is not running")))
    }

    /// The channel of `agent_id`'s box, booting it when it is stopped. For the
    /// executor and the mirror, which are synchronous: the boot runs on the
    /// engine runtime from a scoped thread (`boxes::block_on`).
    pub fn channel_for(&self, agent_id: &str) -> Result<Arc<Channel>> {
        if let Some(running) = self.running(agent_id) {
            return Ok(running.channel.clone());
        }
        let identity = self
            .entries
            .lock()
            .get(agent_id)
            .map(|e| e.identity.clone())
            .ok_or_else(|| Error::Other(format!("no box is registered for {agent_id}")))?;
        super::block_on(self.ensure_running(&identity)).map(|r| r.channel.clone())
    }

    /// The running box for `agent_id`, booting it when stopped (async callers).
    pub async fn running_or_start(&self, agent_id: &str) -> Result<Arc<RunningBox>> {
        if let Some(running) = self.running(agent_id) {
            return Ok(running);
        }
        let identity = self
            .entries
            .lock()
            .get(agent_id)
            .map(|e| e.identity.clone())
            .ok_or_else(|| Error::Other(format!("no box is registered for {agent_id}")))?;
        self.ensure_running(&identity).await
    }

    /// Boot `identity`'s box unless it runs already. While the host is still
    /// being set up — the image downloading,
    /// the provider's tools disk being built — this waits for it (`setup`),
    /// reporting the box as `Preparing` with what it waits on, so a launch
    /// made right after the flag went on is late rather than refused. Fails
    /// closed with the reason, the console tail included when the guest never
    /// became ready.
    pub async fn ensure_running(&self, identity: &BoxIdentity) -> Result<Arc<RunningBox>> {
        self.register(identity.clone());
        let lifecycle = {
            let entries = self.entries.lock();
            let entry = entries.get(&identity.agent_id).expect("registered above");
            if let Some(running) = &entry.running {
                return Ok(running.clone());
            }
            entry.lifecycle.clone()
        };
        let _guard = lifecycle.lock().await;
        if let Some(running) = self.running(&identity.agent_id) {
            return Ok(running);
        }
        let agent_id = &identity.agent_id;
        // A discard waits on this lock before it forgets the box, so a boot
        // that reaches it once one has begun must not start a VM for an agent
        // that is going: checked here and again after the (long) preparation.
        // Likewise once the app is quitting ([`Self::close`]).
        if let Some(refused) = self.refuse_boot(agent_id) {
            return Err(refused);
        }
        let prepared = async {
            let ready = setup::ready(|detail| {
                self.set_phase(agent_id, BoxPhase::Preparing { detail });
            })
            .await?;
            self.verify_image_once(&ready).await?;
            let tools_disk = self
                .tools_disk(&ready, agent_id, &identity.provider)
                .await?;
            Ok::<_, Error>((ready, tools_disk))
        }
        .await;
        let (ready, tools_disk) = match prepared {
            Ok(prepared) => prepared,
            Err(e) => {
                self.set_phase(
                    agent_id,
                    BoxPhase::Failed {
                        reason: e.to_string(),
                    },
                );
                return Err(e);
            }
        };
        // Not held across the preparation, which can wait minutes on a tools
        // disk. A discard or app exit begun during the wait is honored below.
        let slot = self.boot_slot(agent_id).await;
        if let Some(refused) = self.refuse_boot(agent_id) {
            return Err(refused);
        }
        self.set_phase(agent_id, BoxPhase::Starting);
        let booted = boot(identity, &ready, Some(tools_disk)).await;
        drop(slot);
        match booted {
            Ok(running) => {
                let running = Arc::new(running);
                let published = 'publish: {
                    let mut entries = self.entries.lock();
                    // Read under the lock `close` sets it under, so this box is
                    // either running before shutdown snapshots the running
                    // boxes, or never published at all.
                    if self.closing.load(Ordering::SeqCst) {
                        break 'publish None;
                    }
                    let entry = entries
                        .get_mut(&identity.agent_id)
                        .expect("registered above");
                    entry.running = Some(running.clone());
                    entry.phase = BoxPhase::Running {
                        since: running.since,
                    };
                    entry.idle_gen += 1;
                    Some(entry.attached.is_empty().then_some(entry.idle_gen))
                };
                let Some(arm_idle) = published else {
                    // The app began quitting during the boot, after
                    // `shutdown_all` listed the running boxes: end it here.
                    self.finish_stop(&running, kill_rungs(&running.vm)).await;
                    return Err(closing_error(agent_id));
                };
                self.emit_state(&identity.agent_id);
                // A box booted by a read (no launch attached) must not run
                // forever either.
                if let Some(gen) = arm_idle {
                    self.arm_idle(&identity.agent_id, gen);
                }
                Ok(running)
            }
            Err(e) => {
                tracing::warn!(agent_id = %identity.agent_id, error = %e, "box boot failed");
                self.set_phase(
                    &identity.agent_id,
                    BoxPhase::Failed {
                        reason: e.to_string(),
                    },
                );
                Err(Error::SandboxUnavailable(format!(
                    "the box for {} did not start: {e}",
                    identity.agent_id
                )))
            }
        }
    }

    /// One of the [`MAX_CONCURRENT_BOOTS`] VM starts, held until the permit
    /// drops: each start reserves a VM's memory, and after an app restart
    /// every box would otherwise start at once. A box waiting for one reports
    /// so, and goes back to `Stopped` if the wait is abandoned. Lock order:
    /// the box's lifecycle lock (held by the caller), then this.
    async fn boot_slot(&self, agent_id: &str) -> tokio::sync::SemaphorePermit<'_> {
        if let Ok(slot) = self.boots.try_acquire() {
            return slot;
        }
        self.set_phase(
            agent_id,
            BoxPhase::Preparing {
                detail: WAITING_FOR_SLOT.into(),
            },
        );
        let abandoned = StopOnDrop {
            registry: self,
            agent_id,
        };
        let slot = self.boots.acquire().await;
        std::mem::forget(abandoned);
        slot.expect("the boot semaphore is never closed")
    }

    /// Hash the installed image against its pin once per process, off the
    /// runtime (800 MB takes a second or two).
    async fn verify_image_once(&self, ready: &super::Ready) -> Result<()> {
        if self.image_verified.lock().contains(&ready.image.version) {
            return Ok(());
        }
        let image = ready.image.clone();
        tokio::task::spawn_blocking(move || super::image_store().verify(&image))
            .await
            .map_err(|e| Error::Other(e.to_string()))?
            .map_err(|e| {
                Error::SandboxUnavailable(format!("the box image failed verification: {e}"))
            })?;
        self.image_verified
            .lock()
            .insert(ready.image.version.clone());
        Ok(())
    }

    /// The tools disk a box of `provider` attaches. Neither codegraph nor a
    /// host CLI update holds a box up: the disk at hand boots
    /// ([`bootable_tools_disk`]) while the wanted one builds. Only a provider
    /// with no disk waits for its build, up to [`setup::tools_wait`] (the
    /// spawn watchdog allows for it, `setup::setup_allowance`), reported
    /// `Preparing`; while failed builds hold the next off ([`tools_backoff`])
    /// it fails at once instead.
    async fn tools_disk(
        &self,
        ready: &super::Ready,
        agent_id: &str,
        provider: &str,
    ) -> Result<ToolsLease> {
        // Probed here, never under the lease lock (it may run `--version`).
        let version = tools_version(provider);
        let root = tools_root();
        let built =
            || tools::built_disk_in(&root, provider, &version, CODEGRAPH_VERSION).map(|d| d.path);
        if let Some(lease) = self.lease_tools_disk(built) {
            if !tools_disk_settled(provider) {
                drop(self.start_tools_build(ready, provider));
            }
            return Ok(lease);
        }
        let build = self.start_tools_build(ready, provider);
        let bootable = || {
            tools::bootable_disk_in(&root, provider, &version, CODEGRAPH_VERSION).map(|d| d.path)
        };
        if let Some(lease) = self.lease_tools_disk(bootable) {
            tracing::info!(
                agent_id,
                provider,
                version,
                disk = %lease.path.display(),
                "no tools disk for this CLI version yet; booting the newest built while it builds"
            );
            return Ok(lease);
        }
        let label = crate::agent::provider_bin_label(provider).map_or(provider, |(_, l)| l);
        // A fresh build reads `Building`: `Failed` is the backoff's verdict.
        let failed = match &*build.borrow() {
            ToolsBuild::Failed(e) => Some(e.clone()),
            _ => None,
        };
        if let Some(e) = failed {
            let minutes = self
                .tools_backoff_left(provider)
                .map_or(1, |left| left.as_secs().div_ceil(60).max(1));
            return Err(Error::SandboxUnavailable(format!(
                "installing {label} {version} failed: {e}. Retry now in Settings, or it's \
                 tried again on a launch after {minutes} min"
            )));
        }
        self.set_phase(
            agent_id,
            BoxPhase::Preparing {
                detail: format!("installing {label} {version}"),
            },
        );
        wait_tools_build(build, setup::tools_wait()).await?;
        self.lease_tools_disk(built).ok_or_else(|| {
            Error::SandboxUnavailable(format!(
                "the {provider} tools disk was reported built but {} is missing",
                fletch_box::tools::disk_path(&root, provider, &version, None).display()
            ))
        })
    }

    /// Lease the disk `pick` names, picked under the lease lock so a sweep
    /// cannot delete it between the pick and the lease. `pick` only reads the
    /// file system: it must not probe the CLI version.
    fn lease_tools_disk(&self, pick: impl FnOnce() -> Option<PathBuf>) -> Option<ToolsLease> {
        let mut leases = self.tools_leases.lock();
        let path = pick()?;
        *leases.entry(path.clone()).or_default() += 1;
        Some(ToolsLease {
            leases: self.tools_leases.clone(),
            path,
        })
    }

    /// Hash `provider`'s bootable tools disk against its marker, off the
    /// launch path: a launch trusts a marker whose size matches (the
    /// install's fsync order makes one proof the install finished), and this
    /// catches a disk damaged in place since. Blocking, seconds per disk; run
    /// once per process from `boxes::startup`. A damaged disk no box has
    /// leased is removed, and the disk rebuilt.
    pub fn verify_tools_disks(&self, provider: &str) {
        let version = tools_version(provider);
        if !self.verify_tools_disks_in(&tools_root(), provider, &version)
            || tools_disk_settled(provider)
        {
            return;
        }
        match setup::ready_now() {
            Ok(ready) => drop(self.start_tools_build(&ready, provider)),
            Err(e) => tracing::info!(
                provider,
                error = %e,
                "no tools rebuild after a damaged disk: the box runtime is not ready"
            ),
        }
    }

    /// [`Self::verify_tools_disks`] in `root`: the disk the next boot takes,
    /// then, after removing a damaged one, the next. Single-flight per path
    /// and marker hash (`tools_verified`). Whether it removed a disk.
    fn verify_tools_disks_in(&self, root: &Path, provider: &str, version: &str) -> bool {
        let mut removed = false;
        loop {
            // A disk with no marker has nothing to be checked against.
            let Some(FoundDisk {
                path,
                marker: Some(marker),
            }) = tools::bootable_disk_in(root, provider, version, CODEGRAPH_VERSION)
            else {
                return removed;
            };
            let key = (path.clone(), marker.sha256.clone());
            if !self.tools_verified.lock().insert(key.clone()) {
                return removed;
            }
            match fletch_box::image_store::file_matches(&path, Some(marker.size), &marker.sha256) {
                Ok(true) => return removed,
                Ok(false) => {}
                Err(e) => {
                    self.tools_verified.lock().remove(&key);
                    tracing::warn!(
                        provider,
                        disk = %path.display(),
                        error = %e,
                        "tools disk could not be verified"
                    );
                    return removed;
                }
            }
            if !self.remove_damaged_tools_disk(&path, &marker) {
                return removed;
            }
            removed = true;
        }
    }

    /// Remove `disk`, which does not hash to `marker`, unless a box has
    /// leased it or a build replaced it since. Under the lease lock, so no
    /// boot picks it meanwhile. Whether it was removed.
    fn remove_damaged_tools_disk(&self, disk: &Path, marker: &DiskMarker) -> bool {
        let leases = self.tools_leases.lock();
        if leases.contains_key(disk) {
            tracing::warn!(
                disk = %disk.display(),
                "tools disk does not match its marker; a box runs on it, so it stays"
            );
            return false;
        }
        if tools::disk_state(disk) != DiskState::Built(marker.clone()) {
            return false;
        }
        tracing::warn!(
            disk = %disk.display(),
            "tools disk does not match its marker; removing it"
        );
        match tools::remove_disk(disk) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(
                    disk = %disk.display(),
                    error = %e,
                    "removing a damaged tools disk failed"
                );
                false
            }
        }
    }

    /// Delete `provider`'s tools disks that no boot takes again
    /// ([`tools::superseded_files`]), so each CLI update does not leave a
    /// ~4 GB disk behind. Holds the lease lock (no disk is picked meanwhile)
    /// and the build table (no build starts staging meanwhile). A disk skipped
    /// for being in use goes at a later sweep. Best effort: failures are
    /// logged.
    pub fn sweep_tools_disks(&self, provider: &str) {
        // Probed before taking any lock (it may run `--version`).
        let version = tools_version(provider);
        let leases = self.tools_leases.lock();
        let builds = self.tools_builds.lock();
        let building: HashSet<String> = builds
            .iter()
            .filter(|(_, slot)| *slot.build.borrow() == ToolsBuild::Building)
            .filter_map(|(key, _)| Some(key.strip_prefix(provider)?.strip_prefix('-')?.to_string()))
            .collect();
        let in_use: HashSet<PathBuf> = leases.keys().cloned().collect();
        let swept = tools::superseded_files(
            &tools_root(),
            provider,
            &version,
            CODEGRAPH_VERSION,
            &in_use,
            &building,
        );
        for (disk, kind) in &swept.unreadable {
            tracing::warn!(
                provider,
                disk = %disk.display(),
                error = %kind,
                "a tools disk's marker could not be read; leaving the disk"
            );
        }
        let removed = swept
            .disks
            .iter()
            .map(|disk| (disk, tools::remove_disk(disk)))
            .chain(
                swept
                    .files
                    .iter()
                    .map(|file| (file, fletch_box::fsutil::remove_if_exists(file))),
            );
        for (path, outcome) in removed {
            match outcome {
                Ok(()) => tracing::info!(
                    provider,
                    path = %path.display(),
                    "removed a superseded tools disk file"
                ),
                Err(e) => tracing::warn!(
                    provider,
                    path = %path.display(),
                    error = %e,
                    "removing a superseded tools disk file failed"
                ),
            }
        }
    }

    /// The tools disk `agent_id`'s box runs with, while it runs with one.
    pub fn attached_tools_disk(&self, agent_id: &str) -> Option<PathBuf> {
        Some(self.running(agent_id)?.tools_disk()?.to_path_buf())
    }

    /// Where `provider`'s tools disk stands, for the probe: the disk for the
    /// current CLI version. While another version's disk stands in for it
    /// ([`bootable_tools_disk`]) this still reads `building` (or `failed`,
    /// `missing`), which is the truth about the disk the next boots want.
    pub fn tools_status(&self, provider: &str) -> ToolsStatus {
        let version = tools_version(provider);
        let (phase, error, retry_at) = if built_tools_disk(provider).is_some() {
            ("ready", None, None)
        } else {
            let now = Instant::now();
            match self
                .tools_builds
                .lock()
                .get(&format!("{provider}-{version}"))
                .map(|slot| (slot.build.borrow().clone(), slot.failures))
            {
                Some((ToolsBuild::Building, _)) => ("building", None, None),
                Some((ToolsBuild::Failed(e), failures)) => (
                    "failed",
                    Some(e),
                    failures.and_then(|f| unix_ms_ahead(f.retry_at, now)),
                ),
                // `Ready` with no file: the disk was removed since; a launch
                // builds it again.
                Some((ToolsBuild::Ready, _)) | None => ("missing", None, None),
            }
        };
        ToolsStatus {
            provider: provider.to_string(),
            version,
            phase,
            error,
            retry_at,
        }
    }

    /// How long failed builds still hold off the next build of `provider`'s
    /// disk for the current version ([`tools_backoff`]).
    fn tools_backoff_left(&self, provider: &str) -> Option<Duration> {
        let key = format!("{provider}-{}", tools_version(provider));
        let failures = self.tools_builds.lock().get(&key)?.failures?;
        failures.retry_at.checked_duration_since(Instant::now())
    }

    /// Forget `provider`'s failed builds, every CLI version's: the user asked
    /// for a build now (`setup::retry_tools`), so the next one starts whatever
    /// the backoff said. A build already running is left to finish.
    pub fn reset_tools_backoff(&self, provider: &str) {
        for (key, slot) in self.tools_builds.lock().iter_mut() {
            if key
                .strip_prefix(provider)
                .is_some_and(|rest| rest.starts_with('-'))
            {
                slot.failures = None;
            }
        }
    }

    /// Build `provider`'s tools disk for the current version in the background
    /// (`box:tools` events) and hand back a receiver of its outcome. Joins a
    /// build already running; a finished one (failed, or built and since
    /// removed) is replaced by a fresh attempt — after a failure, only once
    /// the backoff has passed ([`tools_backoff`]): until then this hands back
    /// the failed build's receiver and starts nothing. The disk carries
    /// codegraph when indexing is on and its bundle can be had, and is built
    /// plain otherwise — unless the plain one is there already, when there is
    /// nothing to build.
    pub fn start_tools_build(
        &self,
        ready: &super::Ready,
        provider: &str,
    ) -> watch::Receiver<ToolsBuild> {
        let version = tools_version(provider);
        // One key for both variants, so they never build (and share the build
        // box's run dir) at once.
        let key = format!("{provider}-{version}");
        let claimed = claim_tools_build(&mut self.tools_builds.lock(), &key, Instant::now());
        let tx = match claimed {
            Ok(tx) => tx,
            Err(rx) => return rx,
        };
        let rx = tx.subscribe();
        let builds = self.tools_builds.clone();
        let verified = self.tools_verified.clone();
        let (paths, image) = (ready.paths.clone(), ready.image.path());
        let provider = provider.to_string();
        crate::host::spawn(async move {
            let event = |phase, error: Option<String>, retry_at: Option<Instant>| {
                super::emit(
                    EVENT_TOOLS,
                    &ToolsStatus {
                        provider: provider.clone(),
                        version: version.clone(),
                        phase,
                        error,
                        retry_at: retry_at.and_then(|at| unix_ms_ahead(at, Instant::now())),
                    },
                )
            };
            event("building", None, None);
            let bundle = codegraph_bundle_for_build().await;
            let out = tools_disk_path(&provider, bundle.is_some());
            if matches!(tools::disk_state(&out), DiskState::Built(_)) {
                // A build for codegraph whose bundle could not be had: the
                // plain disk it fell back to is the one already there.
                record_tools_build(&builds, &key, true, Instant::now());
                tx.send_replace(ToolsBuild::Ready);
                event("ready", None, None);
                return;
            }
            let codegraph = bundle
                .as_deref()
                .map(|bundle| fletch_box::tools::Codegraph {
                    version: crate::codegraph::CODEGRAPH_VERSION,
                    bundle,
                });
            let built = fletch_box::tools::build(
                &paths,
                &image,
                &provider,
                &version,
                codegraph,
                &out,
                |line| tracing::info!(target: "fletch::box_tools", provider, line, "tools build"),
            )
            .await;
            // The failures are recorded before the verdict goes out, so a
            // waiter that reads it and asks for a build again sees the backoff.
            match built {
                Ok(marker) => {
                    tracing::info!(provider, version, path = %out.display(), "tools disk built");
                    // Hashed as it was installed: no need to hash it again.
                    verified.lock().insert((out.clone(), marker.sha256));
                    record_tools_build(&builds, &key, true, Instant::now());
                    tx.send_replace(ToolsBuild::Ready);
                    event("ready", None, None);
                    sweep_tools_disks_in_background(provider.clone());
                }
                Err(e) => {
                    let retry_at = record_tools_build(&builds, &key, false, Instant::now());
                    tracing::warn!(
                        provider,
                        version,
                        error = %e,
                        retry_in_secs = retry_at.map(|at| at.saturating_duration_since(Instant::now()).as_secs()),
                        "tools disk build failed"
                    );
                    tx.send_replace(ToolsBuild::Failed(e.to_string()));
                    event("failed", Some(e.to_string()), retry_at);
                }
            }
        });
        rx
    }

    /// Stop `agent_id`'s VM through the ladder (§7.7): the mirror refreshed
    /// first so publish has the objects while the box is down, then `stop` on
    /// the control port, the ssh sysrq rung, SIGTERM, SIGKILL. `dirty` is
    /// cleared only after a clean rung.
    pub async fn stop(&self, agent_id: &str, reason: StopReason) -> Result<StopOutcome> {
        match self.take_running(agent_id, |_| true) {
            Some(taken) => self.stop_taken(agent_id, reason, taken).await,
            None => Ok(StopOutcome::AlreadyExited),
        }
    }

    /// Take `agent_id`'s running VM for a stop, if `may_stop` allows it: both
    /// under one lock, so what `may_stop` checks still holds when the VM is
    /// taken. The idle timer's "nothing attached" must: a session attaching
    /// between a separate check and the take would have its VM stopped under
    /// it. `None` when there is no running VM or `may_stop` refuses.
    fn take_running(
        &self,
        agent_id: &str,
        may_stop: impl FnOnce(&Entry) -> bool,
    ) -> Option<(Arc<RunningBox>, Arc<tokio::sync::Mutex<()>>)> {
        let taken = {
            let mut entries = self.entries.lock();
            let entry = entries.get_mut(agent_id)?;
            if !may_stop(entry) {
                return None;
            }
            let running = entry.running.take()?;
            entry.phase = BoxPhase::Stopping;
            entry.idle_gen += 1;
            (running, entry.lifecycle.clone())
        };
        self.emit_state(agent_id);
        Some(taken)
    }

    /// The ladder for a VM [`Self::take_running`] took.
    ///
    /// A [`StopReason::Shutdown`] stop is bounded by [`SHUTDOWN_GRACE`] (the
    /// lock wait, the sessions pull, and the clean rungs from `send_stop` on)
    /// and skips the mirror refresh: every turn checkpoint already refreshed
    /// it. Nothing holds the lifecycle lock long once a box runs, but if
    /// something did, the VM is killed without it. On app exit nothing can boot
    /// meanwhile ([`Self::close`]); with the box flag going off, a boot could
    /// start a second VM on the same disk while this one is killed.
    async fn stop_taken(
        &self,
        agent_id: &str,
        reason: StopReason,
        (running, lifecycle): (Arc<RunningBox>, Arc<tokio::sync::Mutex<()>>),
    ) -> Result<StopOutcome> {
        let bound = (reason == StopReason::Shutdown).then_some(SHUTDOWN_GRACE);
        let guard = match bound {
            Some(bound) => tokio::time::timeout(bound, lifecycle.lock()).await.ok(),
            None => Some(lifecycle.lock().await),
        };
        let outcome = if let Some(_guard) = guard {
            match reason {
                StopReason::Discard => {}
                StopReason::Shutdown => {
                    let pulled = super::home::pull_before_stop(&running);
                    if tokio::time::timeout(SHUTDOWN_GRACE, pulled).await.is_err() {
                        tracing::warn!(agent_id, "sessions pull before shutdown timed out");
                    }
                }
                _ => {
                    if let Err(e) = super::mirror::refresh_running(&running).await {
                        tracing::info!(agent_id, error = %e, "mirror refresh before stop failed");
                    }
                    super::home::pull_before_stop(&running).await;
                }
            }
            self.finish_stop(&running, stop_ladder(&running.vm, &running.channel, bound))
                .await
        } else {
            tracing::warn!(agent_id, "box lifecycle lock still held; killing its VM");
            self.finish_stop(&running, kill_rungs(&running.vm)).await
        };
        tracing::info!(agent_id, ?reason, ?outcome, "box stopped");
        // Its disk's lease goes with it (unless another holder of the box
        // outlives this), so a disk an update superseded can go now rather
        // than at the next app start. Not on app exit: the next start sweeps.
        let provider = running.identity.provider.clone();
        let had_disk = running.tools_disk().is_some();
        drop(running);
        if had_disk && reason != StopReason::Shutdown {
            sweep_tools_disks_in_background(provider);
        }
        Ok(outcome)
    }

    /// End `running`'s VM with `halt` (the ladder or the kill rungs) and clean
    /// up after it: relays stopped first, so none reconnects to a guest going
    /// down; then the proxy dropped, the dirty marker cleared after a clean
    /// outcome, and the phase `Stopped` unless a newer box was published.
    async fn finish_stop(
        &self,
        running: &RunningBox,
        halt: impl Future<Output = StopOutcome>,
    ) -> StopOutcome {
        let agent_id = running.agent_id();
        running.relays.stop().await;
        let outcome = halt.await;
        drop(running.proxy.lock().take());
        if outcome.clean() {
            if let Err(e) = running.dirty.clear() {
                tracing::warn!(agent_id, error = %e, "clearing the dirty marker failed");
            }
        } else {
            tracing::warn!(agent_id, "box was killed; its disk is marked dirty");
        }
        if let Some(entry) = self.entries.lock().get_mut(agent_id) {
            if entry.running.is_none() {
                entry.phase = BoxPhase::Stopped;
            }
        }
        self.emit_state(agent_id);
        outcome
    }

    /// Stop the box and delete its directory: the agent is gone. Forgets the
    /// box and stops routing its root.
    ///
    /// No boot starts once this has begun ([`Self::ensure_running`] checks),
    /// and one already under way — or a stop — is waited out first: either
    /// holds the box's lifecycle lock until its VM is up or down. `stop` alone
    /// read a box still booting as already exited, so its directory was
    /// deleted under the boot and the VM came up after the agent was gone.
    pub async fn discard(&self, agent_id: &str) -> Result<()> {
        let lifecycle = {
            let mut entries = self.entries.lock();
            entries.get_mut(agent_id).map(|entry| {
                entry.discarding = true;
                entry.lifecycle.clone()
            })
        };
        if let Some(lifecycle) = lifecycle {
            drop(lifecycle.lock().await);
        }
        let _ = self.stop(agent_id, StopReason::Discard).await?;
        let identity = self.entries.lock().remove(agent_id).map(|e| e.identity);
        if let Some(identity) = &identity {
            crate::exec::unregister_root(&identity.writable_root);
        }
        // Its tenants' roots pointed into a guest that no longer exists.
        self.forget_tenants_of(agent_id);
        remove_box_dir(&super::box_dir(agent_id)?).await
    }

    /// Whether `agent_id`'s box is being discarded, or already was (it is no
    /// longer known).
    pub(super) fn discarded(&self, agent_id: &str) -> bool {
        self.entries
            .lock()
            .get(agent_id)
            .map_or(true, |e| e.discarding)
    }

    /// Why no boot may start for `agent_id` now: the app is quitting
    /// ([`Self::close`]), or its box is being discarded.
    fn refuse_boot(&self, agent_id: &str) -> Option<Error> {
        if self.closing.load(Ordering::SeqCst) {
            return Some(closing_error(agent_id));
        }
        self.discarded(agent_id).then(|| discarded_error(agent_id))
    }

    /// App exit: no box boots from here on; a boot already past its checks
    /// kills its VM rather than publish it. Set under the entries lock a boot
    /// publishes under, so a later [`Self::shutdown_all`] sees every box that
    /// will ever run. One-way, so only the app's exit paths call it, never
    /// `Supervisor::shutdown` (unit tests call that) or the box flag going off.
    pub fn close(&self) {
        let _entries = self.entries.lock();
        self.closing.store(true, Ordering::SeqCst);
    }

    /// A session in the box is starting; the idle timer stands down. Attach
    /// *before* reaching the box (`boxes::Hold`): the idle stop only spares a
    /// box that is attached when it takes the VM, so a session that attaches
    /// once it has the channel can still be handed a VM being stopped.
    pub fn attach(&self, agent_id: &str, launch_id: &str) {
        let mut entries = self.entries.lock();
        if let Some(entry) = entries.get_mut(agent_id) {
            entry.attached.insert(launch_id.to_string());
            entry.idle_gen += 1;
        }
    }

    /// A launch's session ended; with none left the idle timer arms.
    pub fn detach(&self, agent_id: &str, launch_id: &str) {
        let arm = {
            let mut entries = self.entries.lock();
            let Some(entry) = entries.get_mut(agent_id) else {
                return;
            };
            entry.attached.remove(launch_id);
            entry.idle_gen += 1;
            (entry.attached.is_empty() && entry.running.is_some()).then_some(entry.idle_gen)
        };
        if let Some(gen) = arm {
            self.arm_idle(agent_id, gen);
        }
    }

    fn arm_idle(&self, agent_id: &str, gen: u64) {
        let Some(idle) = settings::current().idle_stop else {
            return;
        };
        let agent_id = agent_id.to_string();
        crate::host::spawn(async move {
            tokio::time::sleep(idle).await;
            let registry = BoxRegistry::global();
            let Some(taken) =
                registry.take_running(&agent_id, |e| e.idle_gen == gen && e.attached.is_empty())
            else {
                return;
            };
            if let Err(e) = registry
                .stop_taken(&agent_id, StopReason::Idle, taken)
                .await
            {
                tracing::warn!(agent_id, error = %e, "idle stop failed");
            }
        });
    }

    /// Stop every running box, all at once, each bounded by
    /// [`SHUTDOWN_GRACE`] ([`Self::stop_taken`]); app exit and the box flag
    /// going off.
    pub async fn shutdown_all(&self) {
        let ids = self.running_ids();
        if ids.is_empty() {
            return;
        }
        let started = std::time::Instant::now();
        let stops = futures_util::future::join_all(ids.iter().map(|id| async move {
            let stopped = self.stop(id, StopReason::Shutdown).await;
            if let Err(e) = &stopped {
                tracing::warn!(agent_id = %id, error = %e, "box shutdown failed");
            }
            stopped
        }))
        .await;
        let tally = ShutdownTally::of(&stops);
        tracing::info!(
            clean = tally.clean,
            killed = tally.killed,
            failed = tally.failed,
            elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "boxes shut down"
        );
    }

    /// A project's egress additions changed: every running box of that
    /// project serves the new policy from its next connection.
    /// Every running box serves the policy of its project from its next
    /// connection: the global list changed.
    pub fn update_egress_all(&self) {
        let projects: std::collections::HashSet<String> = self
            .entries
            .lock()
            .values()
            .filter_map(|e| e.running.as_ref().map(|r| r.identity.project_id.clone()))
            .collect();
        for project_id in projects {
            self.update_egress(&project_id);
        }
    }

    pub fn update_egress(&self, project_id: &str) {
        let policy = egress_policy(project_id);
        for running in self
            .entries
            .lock()
            .values()
            .filter_map(|e| e.running.clone())
        {
            if running.identity.project_id == project_id {
                if let Some(proxy) = running.proxy.lock().as_ref() {
                    proxy.update(policy.clone());
                }
            }
        }
    }

    fn set_phase(&self, agent_id: &str, phase: BoxPhase) {
        if let Some(entry) = self.entries.lock().get_mut(agent_id) {
            entry.phase = phase;
        }
        self.emit_state(agent_id);
    }

    fn emit_state(&self, agent_id: &str) {
        super::emit(EVENT_STATE, &self.report(agent_id));
    }
}

/// Wait for one tools build's outcome: the disk, the build's own error, or
/// `timeout` passing. A build whose sender is gone without a verdict counts
/// as failed.
async fn wait_tools_build(mut build: watch::Receiver<ToolsBuild>, timeout: Duration) -> Result<()> {
    let outcome = tokio::time::timeout(timeout, async {
        loop {
            match build.borrow_and_update().clone() {
                ToolsBuild::Building => {}
                ToolsBuild::Ready => return Ok(()),
                ToolsBuild::Failed(e) => return Err(e),
            }
            if build.changed().await.is_err() {
                return Err("the build ended without a result".to_string());
            }
        }
    })
    .await;
    match outcome {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(Error::SandboxUnavailable(format!(
            "the tools disk build failed: {e}. Retry in Settings to build it again"
        ))),
        Err(_) => Err(Error::SandboxUnavailable(format!(
            "the tools disk build did not finish within {} minutes",
            timeout.as_secs() / 60
        ))),
    }
}

/// The CLI version a provider's tools disk installs: the host CLI's, as the
/// container engines' version-parity check reads it, else
/// [`tools::LATEST_VERSION`].
pub fn tools_version(provider: &str) -> String {
    crate::agent::cached_provider_version(provider)
        .unwrap_or_else(|| tools::LATEST_VERSION.to_string())
}

/// Where `provider`'s tools disk for the current version is (or will be), with
/// codegraph on it at the pinned version or without.
pub fn tools_disk_path(provider: &str, codegraph: bool) -> PathBuf {
    fletch_box::tools::disk_path(
        &tools_root(),
        provider,
        &tools_version(provider),
        codegraph.then_some(crate::codegraph::CODEGRAPH_VERSION),
    )
}

/// The tools disk a box of `provider` boots with now, `None` before one is
/// built ([`tools::built_disk_in`]). A disk with no marker, even under this
/// version's name, is no more than a fallback, so this version's disk is
/// built again with one and the sweep then removes the old one.
pub fn built_tools_disk(provider: &str) -> Option<PathBuf> {
    tools::built_disk_in(
        &tools_root(),
        provider,
        &tools_version(provider),
        CODEGRAPH_VERSION,
    )
    .map(|disk| disk.path)
}

/// The tools disk a box of `provider` can boot with right now
/// ([`tools::bootable_disk_in`]). It may have no marker: users have disks
/// from before markers, and refusing them would turn the first launch after
/// the upgrade into a wait for a build. A damaged one is never booted.
pub fn bootable_tools_disk(provider: &str) -> Option<PathBuf> {
    tools::bootable_disk_in(
        &tools_root(),
        provider,
        &tools_version(provider),
        CODEGRAPH_VERSION,
    )
    .map(|disk| disk.path)
}

/// [`BoxRegistry::sweep_tools_disks`] on a thread of its own, so cleanup
/// never holds up what triggered it.
pub(crate) fn sweep_tools_disks_in_background(provider: String) {
    tools_task("box-tools-sweep", move || {
        BoxRegistry::global().sweep_tools_disks(&provider)
    });
}

/// [`BoxRegistry::verify_tools_disks`] on a thread of its own.
pub(crate) fn verify_tools_disks_in_background(provider: String) {
    tools_task("box-tools-verify", move || {
        BoxRegistry::global().verify_tools_disks(&provider)
    });
}

fn tools_task(name: &str, task: impl FnOnce() + Send + 'static) {
    let spawned = std::thread::Builder::new().name(name.into()).spawn(task);
    if let Err(e) = spawned {
        tracing::warn!(task = name, error = %e, "a tools disk task did not start");
    }
}

/// Tell the UI `provider`'s tools disk could not be built before any build
/// started (`box:tools` `failed`): a Retry with the box runtime not ready.
pub(crate) fn emit_tools_failed(provider: &str, error: String) {
    super::emit(
        EVENT_TOOLS,
        &ToolsStatus {
            provider: provider.to_string(),
            version: tools_version(provider),
            phase: "failed",
            error: Some(error),
            retry_at: None,
        },
    );
}

/// Whether `provider`'s tools disk needs no build: one is built
/// ([`built_tools_disk`]), and it carries codegraph unless indexing is off.
pub fn tools_disk_settled(provider: &str) -> bool {
    built_tools_disk(provider).is_some_and(|disk| {
        fletch_box::tools::carries_codegraph(&disk) || !crate::codegraph::enabled()
    })
}

/// The verified Linux codegraph bundle a tools disk build should install, or
/// `None` when indexing is off or the bundle can't be had within
/// [`setup::CODEGRAPH_BUNDLE_WAIT`]: the disk is what lets a box boot at all,
/// codegraph is best-effort (a later boot asks for it again, see `tools_disk`).
async fn codegraph_bundle_for_build() -> Option<PathBuf> {
    if !crate::codegraph::enabled() {
        return None;
    }
    match tokio::time::timeout(
        setup::CODEGRAPH_BUNDLE_WAIT,
        crate::codegraph::ensure_guest_bundle(),
    )
    .await
    {
        Ok(Ok(bundle)) => Some(bundle),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "no codegraph bundle; the tools disk is built without it");
            None
        }
        Err(_) => {
            tracing::warn!(
                "the codegraph bundle download timed out; the tools disk is built without it"
            );
            None
        }
    }
}

/// What `project_id`'s boxes may reach, every layer applied
/// (`allowlist::effective`): the template, the user's global list, the
/// project's settings files (approved additions, turned-off entries) and its
/// in-app additions.
pub fn egress_entries(project_id: &str) -> Vec<super::allowlist::Entry> {
    use super::allowlist::{effective, Delta, Source};
    let global = settings::global_egress();
    let file = crate::project_file::egress_delta(project_id);
    let in_app = Delta {
        add: settings::egress_allow(project_id),
        remove: Vec::new(),
    };
    effective(&[
        (Source::Global, &global),
        (Source::ProjectFile, &file),
        (Source::Project, &in_app),
    ])
}

/// [`egress_entries`] as the proxy's policy. Every layer was normalized when
/// stored, so a pattern that no longer parses is dropped with a warning
/// rather than blocking the launch.
pub fn egress_policy(project_id: &str) -> EgressPolicy {
    let patterns: Vec<String> = egress_entries(project_id)
        .into_iter()
        .map(|e| e.pattern)
        .collect();
    match EgressPolicy::new(&patterns) {
        Ok(policy) => policy,
        Err(e) => {
            tracing::warn!(project_id, error = %e, "ignoring the project's egress layers");
            EgressPolicy::default()
        }
    }
}

/// Boot one box (§7.2 step 1).
async fn boot(
    identity: &BoxIdentity,
    ready: &super::Ready,
    tools_lease: Option<ToolsLease>,
) -> Result<RunningBox> {
    let agent_id = identity.agent_id.clone();
    let dir = identity.box_dir()?;
    std::fs::create_dir_all(&dir)?;
    // `sun_path` is 104 bytes; the longest socket name decides.
    let longest = dir.join(EGRESS_SOCK);
    if longest.as_os_str().len() > fletch_box::runtime::MAX_SOCKET_PATH {
        return Err(Error::Other(format!(
            "the box directory {} is too deep for a unix socket path (over {} bytes)",
            dir.display(),
            fletch_box::runtime::MAX_SOCKET_PATH
        )));
    }
    let key = channel::keygen(&dir)?;
    let authorized = channel::authorized_key(&key)?;
    let data = dir.join(DATA_DISK);
    if !data.exists() {
        disks::create_sparse(&data, disks::DEFAULT_DATA_DISK_BYTES)?;
    }
    let dirty = Dirty::in_dir(&dir);
    if dirty.set()? {
        tracing::warn!(
            agent_id,
            "the last VM on this disk did not stop cleanly; init will fsck"
        );
    }
    // The mailbox the relay mirrors into exists before anything can write it.
    crate::rpc::ensure_mailbox(&identity.rpc_dir)?;

    let proxy = {
        let agent_id = agent_id.clone();
        EgressProxy::start(
            &dir.join(EGRESS_SOCK),
            egress_policy(&identity.project_id),
            move |event| on_egress(&agent_id, event),
        )
        .map_err(|e| Error::Other(format!("start the egress proxy: {e}")))?
    };

    let knobs = settings::current();
    let mut spec = VmSpec::new(&agent_id, &ready.image.path(), &data, &dir.join(CONSOLE));
    spec.cpus = knobs.cpus;
    spec.memory_mib = knobs.memory_mib;
    spec.tools_disk = tools_lease.as_ref().map(|lease| lease.path.clone());
    spec.shares = identity
        .shares
        .iter()
        .map(|grant| (grant.path.clone(), !grant.writable))
        .collect();
    spec.listen_ports = vec![
        (guest::SSH_PORT, dir.join(SSH_SOCK)),
        (guest::CONTROL_PORT, dir.join(CTL_SOCK)),
    ];
    spec.connect_ports = vec![(guest::EGRESS_PORT, proxy.socket().to_path_buf())];
    spec.env.extend([
        (
            guest::ENV_WRITABLE_ROOT.to_string(),
            identity.writable_root.to_string_lossy().into_owned(),
        ),
        (
            guest::ENV_RPC_DIR.to_string(),
            identity.rpc_dir.to_string_lossy().into_owned(),
        ),
        (
            guest::ENV_HOME.to_string(),
            identity.home.to_string_lossy().into_owned(),
        ),
        (guest::ENV_AUTHORIZED_KEY.to_string(), authorized),
    ]);

    let started = std::time::Instant::now();
    let vm = Vm::start(&ready.paths, &spec).map_err(|e| Error::Other(e.to_string()))?;
    let host_key = match vm.wait_ready(READY_TIMEOUT).await {
        Ok(key) => key,
        Err(e) => {
            vm.kill();
            return Err(Error::Other(e.to_string()));
        }
    };
    let known_hosts = dir.join(channel::KNOWN_HOSTS_FILE);
    channel::write_known_hosts(&known_hosts, &host_key)?;
    let channel = Arc::new(Channel::new(
        &dir,
        &dir.join(SSH_SOCK),
        &key,
        &known_hosts,
        &ready.paths.helper,
    )?);
    tracing::info!(
        agent_id,
        pid = vm.pid(),
        ready_in = ?started.elapsed(),
        "box ready"
    );

    // An image whose init predates shared folders boots without mounting
    // them, and the agent would find empty directories where the user's
    // folders should be: refuse the box instead, with what to do about it.
    if let Err(e) = verify_shares(&channel, &identity.shares).await {
        vm.kill();
        return Err(e);
    }

    // The guest's mailbox laid out like the host's (`rpc::ensure_mailbox`):
    // the agent writes straight into `requests/`, and on images whose relay
    // does not make it every box agent's first request failed.
    let rpc = identity.rpc_dir.to_string_lossy();
    let (requests, responses) = (format!("{rpc}/requests"), format!("{rpc}/responses"));
    match channel
        .exec(
            None,
            None,
            &["mkdir", "-p", "-m", "700", "--", &requests, &responses],
        )
        .await
    {
        Ok(out) if out.status.success() => {}
        Ok(out) => tracing::warn!(
            agent_id,
            stderr = %String::from_utf8_lossy(&out.stderr).trim(),
            "could not create the guest's rpc mailbox"
        ),
        Err(e) => tracing::warn!(agent_id, error = %e, "could not create the guest's rpc mailbox"),
    }

    // The owner's sessions dir — where the provider's CLI writes its
    // transcripts in this guest — tailed into its mirror under the box dir
    // when the provider's adapter has it tailed (`home::Transport`).
    let provider_home = super::home::require(&identity.provider)?;
    let sessions = provider_home.sessions_dir(&identity.home, &identity.writable_root);
    let mirror = dir.join(provider_home.mirror_dirname());
    let tail = (provider_home.transport() == super::home::Transport::Tail)
        .then_some((sessions.as_path(), mirror.as_path()));
    let relays = relay::Relays::start(channel.clone(), &dir, &identity.rpc_dir, tail);
    Ok(RunningBox {
        identity: identity.clone(),
        dir,
        channel,
        since: chrono::Utc::now().timestamp_millis(),
        tools_lease,
        vm,
        proxy: Mutex::new(Some(proxy)),
        relays,
        dirty,
    })
}

/// Every shared folder is a mountpoint in the guest (`mountpoint -q`, one ssh
/// round trip for all of them).
async fn verify_shares(channel: &Channel, shares: &[FolderGrant]) -> Result<()> {
    if shares.is_empty() {
        return Ok(());
    }
    let script = r#"for p in "$@"; do mountpoint -q -- "$p" || { echo "$p"; exit 1; }; done"#;
    let argv: Vec<String> = ["sh", "-c", script, "sh"]
        .into_iter()
        .map(String::from)
        .chain(shares.iter().map(|s| s.path.to_string_lossy().into_owned()))
        .collect();
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    let out = channel
        .exec(None, None, &argv)
        .await
        .map_err(|e| Error::Other(format!("check the shared folders: {e}")))?;
    if out.status.success() {
        return Ok(());
    }
    let missing = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Err(Error::SandboxUnavailable(format!(
        "the box did not mount the shared folder {missing}: its image predates shared \
         folders. Update the box image, or remove the folder from this workspace."
    )))
}

fn on_egress(agent_id: &str, event: EgressEvent) {
    match event {
        EgressEvent::Allowed { host, port, .. } => super::emit(
            super::EVENT_EGRESS,
            &EgressEventPayload {
                agent_id: agent_id.to_string(),
                host,
                port,
                allowed: true,
                quiet: false,
            },
        ),
        EgressEvent::Denied { host, port, reason } => {
            let quiet = fletch_box::egress::is_quiet(&host);
            tracing::info!(agent_id, host, port, %reason, quiet, "box egress denied");
            super::emit(
                super::EVENT_EGRESS,
                &EgressEventPayload {
                    agent_id: agent_id.to_string(),
                    host,
                    port,
                    allowed: false,
                    quiet,
                },
            )
        }
        EgressEvent::Error { host, message } => {
            tracing::info!(agent_id, ?host, message, "box egress error")
        }
    }
}

/// The ladder, rung by rung. The sysrq rung needs root in the guest, which the
/// product image's `agent` user is not, so on it the rung is inert and only
/// its wait is skipped; it stays for an image that allows it.
fn discarded_error(agent_id: &str) -> Error {
    Error::SandboxUnavailable(format!("the box for {agent_id} was discarded"))
}

fn closing_error(agent_id: &str) -> Error {
    Error::SandboxUnavailable(format!(
        "the box for {agent_id} did not start: the app is quitting"
    ))
}

/// Delete a box directory — its data disk, the agent's whole tree — retrying
/// briefly: a helper just killed can still be releasing its files.
async fn remove_box_dir(dir: &Path) -> Result<()> {
    const ATTEMPTS: u32 = 3;
    for attempt in 1..=ATTEMPTS {
        match tokio::fs::remove_dir_all(dir).await {
            Ok(()) => {
                tracing::info!(box_dir = %dir.display(), "box directory removed");
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) if attempt < ATTEMPTS => {
                tracing::warn!(box_dir = %dir.display(), attempt, error = %e, "box dir removal failed; retrying");
                tokio::time::sleep(Duration::from_millis(200 * u64::from(attempt))).await;
            }
            Err(e) => {
                return Err(Error::Other(format!(
                    "remove box dir {}: {e}",
                    dir.display()
                )))
            }
        }
    }
    unreachable!("the last attempt returns")
}

/// The stop ladder (§7.7). With a `bound`, the clean rungs together get that
/// long from `send_stop` on; the TERM/KILL rungs follow either way.
async fn stop_ladder(vm: &Vm, channel: &Channel, bound: Option<Duration>) -> StopOutcome {
    if exited(vm) {
        return StopOutcome::AlreadyExited;
    }
    cut_short(bound, clean_rungs(vm, channel), || kill_rungs(vm)).await
}

/// `stop` on the control port, then the ssh sysrq rung; `None` when neither
/// brought the VM down.
async fn clean_rungs(vm: &Vm, channel: &Channel) -> Option<StopOutcome> {
    match vm.send_stop().await {
        Ok(()) => {
            if vm.wait_exit(CONTROL_GRACE).await.is_some() {
                return Some(StopOutcome::Control);
            }
        }
        Err(e) => tracing::info!(error = %e, "control stop failed; trying sysrq"),
    }
    let sysrq = channel
        .exec(
            None,
            None,
            &[
                "sh",
                "-c",
                "sync; echo u > /proc/sysrq-trigger; echo o > /proc/sysrq-trigger",
            ],
        )
        .await;
    (matches!(sysrq, Ok(out) if out.status.success()) && vm.wait_exit(SYSRQ_GRACE).await.is_some())
        .then_some(StopOutcome::Sysrq)
}

/// `clean`, given up at `bound` if one is set, then `kill` unless it ended
/// the VM.
async fn cut_short<Kill: Future<Output = StopOutcome>>(
    bound: Option<Duration>,
    clean: impl Future<Output = Option<StopOutcome>>,
    kill: impl FnOnce() -> Kill,
) -> StopOutcome {
    let clean = match bound {
        Some(bound) => tokio::time::timeout(bound, clean).await.ok().flatten(),
        None => clean.await,
    };
    match clean {
        Some(outcome) => outcome,
        None => kill().await,
    }
}

/// The ladder's last two rungs, SIGTERM then SIGKILL. Checks the VM first: a
/// helper already reaped must not be signalled by a pid that may be reused.
async fn kill_rungs(vm: &Vm) -> StopOutcome {
    if exited(vm) {
        return StopOutcome::AlreadyExited;
    }
    vm.terminate();
    if vm.wait_exit(TERM_GRACE).await.is_some() {
        return StopOutcome::Term;
    }
    vm.kill();
    vm.wait_exit(KILL_GRACE).await;
    StopOutcome::Killed
}

fn exited(vm: &Vm) -> bool {
    matches!(vm.try_wait(), Ok(Some(_)))
}

/// How the boxes of one [`BoxRegistry::shutdown_all`] went.
#[derive(Debug, Default, PartialEq, Eq)]
struct ShutdownTally {
    /// Down through a clean outcome (`StopOutcome::clean`).
    clean: usize,
    killed: usize,
    /// Their stop returned an error.
    failed: usize,
}

impl ShutdownTally {
    fn of(stops: &[Result<StopOutcome>]) -> Self {
        let mut tally = Self::default();
        for stop in stops {
            match stop {
                Ok(outcome) if outcome.clean() => tally.clean += 1,
                Ok(_) => tally.killed += 1,
                Err(_) => tally.failed += 1,
            }
        }
        tally
    }
}

/// Boot-time housekeeping on a thread: log boxes whose last VM was killed, and
/// end `fletch-vm` helpers whose owning Fletch is dead (the lifeline normally
/// does this; a SIGKILLed host can leave one). The helper labels itself
/// `--id <box> --host-pid <pid>` for exactly this.
pub fn sweep_orphans() {
    if let Ok(root) = super::boxes_root() {
        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten() {
                if Dirty::in_dir(&entry.path()).is_set() {
                    tracing::info!(
                        box_dir = %entry.path().display(),
                        "box disk marked dirty: its last VM did not stop cleanly; the next boot fscks"
                    );
                }
            }
        }
    }
    let Ok(out) = std::process::Command::new("ps")
        .args(["-axo", "pid=,args="])
        .output()
    else {
        return;
    };
    for (pid, host_pid) in orphan_helpers(&String::from_utf8_lossy(&out.stdout), |pid| {
        crate::sandbox::pid_alive(pid)
    }) {
        tracing::info!(pid, host_pid, "killing a fletch-vm whose host is gone");
        // Only a `fletch-vm` command line with a dead host pid qualifies: a
        // reused pid is not ours to kill.
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
}

/// `(helper pid, host pid)` of every `fletch-vm --id … --host-pid N` line in
/// `ps` output whose host pid is provably dead.
fn orphan_helpers(ps: &str, alive: impl Fn(i32) -> bool) -> Vec<(i32, i32)> {
    ps.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let pid: i32 = words.next()?.parse().ok()?;
            let program = words.next()?;
            if !program.ends_with("fletch-vm") {
                return None;
            }
            let rest: Vec<&str> = words.collect();
            let host_pid: i32 = rest
                .iter()
                .position(|w| *w == "--host-pid")
                .and_then(|i| rest.get(i + 1))?
                .parse()
                .ok()?;
            (!alive(host_pid)).then_some((pid, host_pid))
        })
        .collect()
}

/// A never-booted box registered for a test, its writable root `/<id>/root`.
/// Forgotten on drop, so a failed assertion does not leave it routing paths
/// for the rest of the test binary.
#[cfg(test)]
pub(crate) struct TestBox(&'static str);

#[cfg(test)]
impl TestBox {
    pub(crate) fn register(id: &'static str) -> Self {
        BoxRegistry::global().register(BoxIdentity {
            agent_id: id.into(),
            project_id: "p".into(),
            provider: "claude".into(),
            writable_root: PathBuf::from(format!("/{id}/root")),
            rpc_dir: PathBuf::from(format!("/{id}/rpc")),
            home: PathBuf::from(format!("/{id}/home")),
            shares: Vec::new(),
        });
        Self(id)
    }

    pub(crate) fn root(&self) -> PathBuf {
        PathBuf::from(format!("/{}/root", self.0))
    }
}

#[cfg(test)]
impl Drop for TestBox {
    fn drop(&mut self) {
        if let Some(entry) = BoxRegistry::global().entries.lock().remove(self.0) {
            crate::exec::unregister_root(&entry.identity.writable_root);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_registry() -> BoxRegistry {
        BoxRegistry {
            entries: Mutex::new(HashMap::new()),
            tenants: Mutex::new(HashMap::new()),
            image_verified: Mutex::new(HashSet::new()),
            tools_builds: Builds::default(),
            tools_leases: Leases::default(),
            tools_verified: Verified::default(),
            closing: AtomicBool::new(false),
            boots: tokio::sync::Semaphore::new(MAX_CONCURRENT_BOOTS),
        }
    }

    /// A disk installed the way a build installs one.
    fn installed(disk: &Path) {
        let staging = disk.with_extension("img.building");
        std::fs::write(&staging, b"tools").unwrap();
        tools::install_disk(&staging, disk).unwrap();
    }

    /// Damage the read-only disk at `disk` in place: same size, other bytes.
    fn damage(disk: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(disk, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::write(disk, b"t00ls").unwrap();
    }

    /// The background hash removes a damaged disk no box uses and checks the
    /// disk the next boot takes instead; a disk in use stays; a disk hashed
    /// (or built) this process is not hashed again.
    #[test]
    fn the_background_hash_removes_a_damaged_disk_unless_it_is_in_use() {
        let registry = test_registry();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (good, bad) = (
            root.join("claude-v2.1.312.img"),
            root.join("claude-v2.1.314.img"),
        );
        installed(&good);
        installed(&bad);
        damage(&bad);
        let lease = registry.lease_tools_disk(|| Some(bad.clone())).unwrap();
        assert!(!registry.verify_tools_disks_in(root, "claude", "v2.1.314"));
        assert!(bad.exists(), "a box runs on it");
        drop(lease);

        // Once per process: forget it to check it again.
        registry.tools_verified.lock().clear();
        assert!(registry.verify_tools_disks_in(root, "claude", "v2.1.314"));
        assert_eq!(tools::disk_state(&bad), DiskState::Missing);
        assert!(!tools::marker_path(&bad).exists());
        assert!(good.exists());
        let verified = registry.tools_verified.lock().clone();
        assert!(verified.iter().any(|(path, _)| *path == good));

        // Damage after a match is not looked for again this process.
        damage(&good);
        assert!(!registry.verify_tools_disks_in(root, "claude", "v2.1.314"));
        assert!(good.exists());
    }

    /// A failed build holds the next one of its key off, longer after each
    /// failure in a row; a success, or the user's Retry, clears the count; a
    /// running build is joined; another CLI version is another key.
    #[test]
    fn failed_builds_back_off_until_a_success_or_a_retry() {
        let registry = test_registry();
        let builds = &registry.tools_builds;
        let key = "claude-v2.1.314";
        let t0 = Instant::now();
        let claim = |at: Instant| claim_tools_build(&mut builds.lock(), key, at);
        let fail = |tx: watch::Sender<ToolsBuild>, at: Instant| {
            let retry_at = record_tools_build(builds, key, false, at);
            tx.send_replace(ToolsBuild::Failed("npm exploded".into()));
            retry_at
        };

        let tx = claim(t0).unwrap();
        assert!(claim(t0).is_err(), "a running build is joined");
        let minute = Duration::from_secs(60);
        assert_eq!(fail(tx, t0), Some(t0 + minute));
        let held = claim(t0 + minute / 2).unwrap_err();
        assert_eq!(*held.borrow(), ToolsBuild::Failed("npm exploded".into()));
        // Another version is not held off.
        assert!(claim_tools_build(&mut builds.lock(), "claude-v2.1.315", t0).is_ok());

        let t1 = t0 + minute;
        let tx = claim(t1).unwrap();
        assert_eq!(fail(tx, t1), Some(t1 + 5 * minute));
        assert!(claim(t1 + 4 * minute).is_err());
        let t2 = t1 + 5 * minute;
        let tx = claim(t2).unwrap();
        assert_eq!(fail(tx, t2), Some(t2 + 30 * minute));

        // The user's Retry starts one now; a success clears the count.
        registry.reset_tools_backoff("claude");
        let tx = claim(t2).unwrap();
        assert_eq!(fail(tx, t2), Some(t2 + minute), "counted from zero again");
        let t3 = t2 + minute;
        let tx = claim(t3).unwrap();
        assert_eq!(record_tools_build(builds, key, true, t3), None);
        tx.send_replace(ToolsBuild::Ready);
        let tx = claim(t3).unwrap();
        assert_eq!(fail(tx, t3), Some(t3 + minute));
        // Another provider's Retry leaves this one's backoff alone.
        registry.reset_tools_backoff("codex");
        assert!(claim(t3).is_err());
    }

    #[test]
    fn the_tools_backoff_grows_to_two_hours() {
        let minutes = |n| tools_backoff(n).as_secs() / 60;
        assert_eq!(
            (1..=6).map(minutes).collect::<Vec<_>>(),
            [1, 5, 30, 120, 120, 120]
        );
    }

    /// While failed builds hold the next one off, the status says `failed`
    /// with the error and when a build may start again; past that, no
    /// `retry_at`.
    #[test]
    fn a_failed_status_says_when_the_next_build_may_start() {
        let registry = test_registry();
        let provider = "registry-unit-test-backoff";
        let key = format!("{provider}-{}", tools_version(provider));
        let now = Instant::now();
        let tx = claim_tools_build(&mut registry.tools_builds.lock(), &key, now).unwrap();
        record_tools_build(&registry.tools_builds, &key, false, now);
        tx.send_replace(ToolsBuild::Failed("npm exploded".into()));
        let status = registry.tools_status(provider);
        assert_eq!(status.phase, "failed");
        assert_eq!(status.error.as_deref(), Some("npm exploded"));
        let ahead = status.retry_at.unwrap() - chrono::Utc::now().timestamp_millis();
        assert!((50_000..=60_000).contains(&ahead), "{ahead}");
        assert!(registry.tools_backoff_left(provider).is_some());
        registry.reset_tools_backoff(provider);
        assert_eq!(registry.tools_status(provider).retry_at, None);
        assert_eq!(registry.tools_status(provider).phase, "failed");
    }

    /// A disk stays leased while any box holds it.
    #[test]
    fn a_lease_lasts_until_its_last_holder_is_gone() {
        let registry = test_registry();
        let disk = PathBuf::from("/tools/claude-v2.1.312.img");
        let first = registry.lease_tools_disk(|| Some(disk.clone())).unwrap();
        let second = registry.lease_tools_disk(|| Some(disk.clone())).unwrap();
        assert!(registry.lease_tools_disk(|| None).is_none());
        assert_eq!(registry.tools_leases.lock().get(&disk), Some(&2));
        drop(first);
        assert_eq!(registry.tools_leases.lock().get(&disk), Some(&1));
        drop(second);
        assert!(registry.tools_leases.lock().is_empty());
    }

    #[test]
    fn orphan_helpers_need_a_dead_host_pid() {
        let ps = "\
  101 /Applications/Fletch.app/Contents/Helpers/fletch-vm --id fuji --host-pid 500 --cpus 2 -- /opt/fletch/init
  102 /x/fletch-vm --id etna --host-pid 600 --cpus 2 -- /opt/fletch/init
  103 /x/fletch-vm pipe /b/ssh.sock
  104 /usr/bin/ssh -F /b/ssh_config box
";
        let orphans = orphan_helpers(ps, |pid| pid == 500);
        assert_eq!(orphans, vec![(102, 600)]);
    }

    /// A bounded ladder gives up its clean rungs at the bound and runs the
    /// kill rungs; an unbounded one waits them out. The tally counts what the
    /// kill rungs achieved (a `Term` is clean). Stand-ins for the rungs: they
    /// need a VM.
    #[tokio::test(start_paused = true)]
    async fn a_bounded_ladder_kills_at_its_bound_and_the_tally_counts_outcomes() {
        let bound = Duration::from_secs(10);
        let ladder = |bound: Option<Duration>, clean_after: Option<Duration>, kill: StopOutcome| {
            cut_short(
                bound,
                async move {
                    match clean_after {
                        Some(after) => {
                            tokio::time::sleep(after).await;
                            Some(StopOutcome::Control)
                        }
                        None => std::future::pending().await,
                    }
                },
                move || async move { kill },
            )
        };

        let started = tokio::time::Instant::now();
        let outcomes = futures_util::future::join_all([
            ladder(
                Some(bound),
                Some(Duration::from_secs(3)),
                StopOutcome::Killed,
            ),
            ladder(Some(bound), None, StopOutcome::Killed),
            ladder(Some(bound), None, StopOutcome::Term),
        ])
        .await;
        assert_eq!(started.elapsed(), bound);
        assert_eq!(
            outcomes,
            [StopOutcome::Control, StopOutcome::Killed, StopOutcome::Term]
        );
        let stops: Vec<Result<StopOutcome>> = outcomes
            .into_iter()
            .map(Ok)
            .chain([Err(Error::Other("gone".into()))])
            .collect();
        assert_eq!(
            ShutdownTally::of(&stops),
            ShutdownTally {
                clean: 2,
                killed: 1,
                failed: 1
            }
        );

        let started = tokio::time::Instant::now();
        let unbounded = ladder(None, Some(bound * 2), StopOutcome::Killed).await;
        assert_eq!(unbounded, StopOutcome::Control);
        assert_eq!(started.elapsed(), bound * 2);
    }

    /// Once the registry is closed (app exit), a boot refuses before it does
    /// anything, rather than start a VM behind the shutdown.
    #[tokio::test]
    async fn a_closed_registry_boots_nothing() {
        let registry = test_registry();
        let root = PathBuf::from("/registry-unit-test-closed/root");
        let identity = BoxIdentity {
            agent_id: "registry-unit-test-closed".into(),
            project_id: "p".into(),
            provider: "claude".into(),
            writable_root: root.clone(),
            rpc_dir: PathBuf::from("/registry-unit-test-closed/rpc"),
            home: PathBuf::from("/registry-unit-test-closed/home"),
            shares: Vec::new(),
        };
        registry.close();
        let err = registry
            .ensure_running(&identity)
            .await
            .map(drop)
            .unwrap_err()
            .to_string();
        crate::exec::unregister_root(&root);
        assert!(err.contains("quitting"), "{err}");
        assert_eq!(registry.state(&identity.agent_id), BoxPhase::Stopped);
    }

    #[test]
    fn phases_report_their_name_since_and_reason() {
        let running = BoxStateReport::new("fuji", &BoxPhase::Running { since: 42 });
        assert_eq!(
            (running.phase, running.since, running.reason),
            ("running", Some(42), None)
        );
        let failed = BoxStateReport::new(
            "fuji",
            &BoxPhase::Failed {
                reason: "no image".into(),
            },
        );
        assert_eq!(failed.phase, "failed");
        assert_eq!(failed.reason.as_deref(), Some("no image"));
        assert_eq!(
            BoxStateReport::new("x", &BoxPhase::Stopped).phase,
            "stopped"
        );
    }

    /// A waiter on a tools build gets the verdict, the build's own error, or
    /// the timeout — never a disk that is not there.
    #[tokio::test]
    async fn a_tools_build_wait_ends_with_the_verdict() {
        let (tx, rx) = watch::channel(ToolsBuild::Building);
        let waiter = tokio::spawn(wait_tools_build(rx, Duration::from_secs(5)));
        tokio::time::sleep(Duration::from_millis(20)).await;
        tx.send_replace(ToolsBuild::Ready);
        waiter.await.unwrap().unwrap();

        let (tx, rx) = watch::channel(ToolsBuild::Building);
        let waiter = tokio::spawn(wait_tools_build(rx, Duration::from_secs(5)));
        tokio::time::sleep(Duration::from_millis(20)).await;
        tx.send_replace(ToolsBuild::Failed("npm exploded".into()));
        let err = waiter.await.unwrap().unwrap_err().to_string();
        assert!(err.contains("npm exploded"), "{err}");

        let (_tx, rx) = watch::channel(ToolsBuild::Building);
        let err = wait_tools_build(rx, Duration::from_millis(30))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("did not finish"), "{err}");
    }

    /// A launch whose provider's disk is being built reports the box as
    /// Shares mount in the order the identity lists them, so a folder inside
    /// another granted folder must come after it whatever order the user
    /// granted them in — or the parent's mount would hide the nested one. A
    /// folder gone since it was granted is left out rather than failing boot.
    #[test]
    fn shares_list_parents_before_the_folders_inside_them() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        for dir in ["app/vendor/lib", "out"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        let grant = |p: &str| FolderGrant {
            path: root.join(p),
            writable: false,
        };
        let identity = BoxIdentity {
            agent_id: "a".into(),
            project_id: "p".into(),
            provider: "claude".into(),
            writable_root: PathBuf::from("/w"),
            rpc_dir: PathBuf::from("/r"),
            home: PathBuf::from("/Users/u"),
            shares: Vec::new(),
        }
        .with_shares(&[
            grant("app/vendor/lib"),
            grant("gone"),
            grant("out"),
            grant("app"),
        ]);
        let order: Vec<_> = identity.shares.iter().map(|g| g.path.clone()).collect();
        assert_eq!(
            order,
            [
                root.join("app"),
                root.join("out"),
                root.join("app/vendor/lib")
            ]
        );
    }

    /// `preparing` with what it installs, joins the running build instead of
    /// starting another, and fails with the build's error when that fails.
    #[tokio::test]
    async fn a_launch_waits_on_the_running_tools_build_and_surfaces_its_failure() {
        let registry = BoxRegistry::global();
        let provider = "registry-unit-test-provider";
        let agent_id = "registry-unit-test-waiter";
        registry.register(BoxIdentity {
            agent_id: agent_id.into(),
            project_id: "p".into(),
            provider: provider.into(),
            writable_root: PathBuf::from("/registry-unit-test-waiter/root"),
            rpc_dir: PathBuf::from("/registry-unit-test-waiter/rpc"),
            home: PathBuf::from("/registry-unit-test-waiter/home"),
            shares: Vec::new(),
        });
        // The fake build stands in for `start_tools_build`'s task: same key,
        // same channel shape.
        let (tx, rx) = watch::channel(ToolsBuild::Building);
        registry.tools_builds.lock().insert(
            format!("{provider}-{}", tools_version(provider)),
            ToolsBuildSlot {
                build: rx,
                failures: None,
            },
        );
        assert_eq!(registry.tools_status(provider).phase, "building");
        let ready = super::super::Ready {
            paths: fletch_box::RuntimePaths {
                helper: PathBuf::from("/nonexistent/fletch-vm"),
                lib_dir: None,
                run_root: PathBuf::from("/nonexistent/run"),
            },
            image: fletch_box::InstalledImage {
                dir: PathBuf::from("/nonexistent/image"),
                version: "0.0.0-test".into(),
                sha256: String::new(),
            },
        };
        let waiter = async { registry.tools_disk(&ready, agent_id, provider).await };
        let driver = async {
            tokio::time::sleep(Duration::from_millis(30)).await;
            assert_eq!(
                registry.state(agent_id),
                BoxPhase::Preparing {
                    detail: format!("installing {provider} latest")
                }
            );
            tx.send_replace(ToolsBuild::Failed("no install script".into()));
        };
        let (outcome, ()) = tokio::join!(waiter, driver);
        let err = outcome.unwrap_err().to_string();
        assert!(err.contains("no install script"), "{err}");
        let status = registry.tools_status(provider);
        assert_eq!(status.phase, "failed");
        assert_eq!(status.error.as_deref(), Some("no install script"));
        registry.discard(agent_id).await.unwrap();
    }

    /// A discard waits out a boot or stop in flight (the holder of the box's
    /// lifecycle lock), and a boot queued behind the discard starts no VM: it
    /// finds the box gone and fails, rather than booting for an agent that
    /// is being archived.
    #[tokio::test]
    async fn a_discard_waits_out_the_lifecycle_and_a_boot_behind_it_starts_nothing() {
        let registry = BoxRegistry::global();
        let id = "registry-unit-test-discard-race";
        let identity = BoxIdentity {
            agent_id: id.into(),
            project_id: "p".into(),
            provider: "claude".into(),
            writable_root: PathBuf::from("/registry-unit-test-discard-race/root"),
            rpc_dir: PathBuf::from("/registry-unit-test-discard-race/rpc"),
            home: PathBuf::from("/registry-unit-test-discard-race/home"),
            shares: Vec::new(),
        };
        registry.register(identity.clone());
        // A boot in flight, as far as the lock is concerned.
        let lifecycle = registry.entries.lock().get(id).unwrap().lifecycle.clone();
        let in_flight = lifecycle.lock().await;

        let discard = tokio::spawn(BoxRegistry::global().discard(id));
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(registry.knows(id), "the discard waits for the boot");
        let boot = tokio::spawn(async move {
            BoxRegistry::global()
                .ensure_running(&identity)
                .await
                .map(drop)
        });
        tokio::time::sleep(Duration::from_millis(30)).await;

        drop(in_flight);
        discard.await.unwrap().unwrap();
        let err = boot.await.unwrap().unwrap_err().to_string();
        assert!(err.contains("discarded"), "{err}");
        assert!(!registry.knows(id));
        assert!(matches!(
            crate::exec::for_path(Path::new("/registry-unit-test-discard-race/root/repo")),
            crate::exec::Executor::Local
        ));
    }

    /// No more than [`MAX_CONCURRENT_BOOTS`] VM starts hold a slot at once,
    /// however many contend; one waiting for a slot says so, and goes back to
    /// `Stopped` when its wait is abandoned; every slot comes back.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn boot_slots_admit_at_most_the_limit() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let registry = BoxRegistry::global();
        let id = "registry-unit-test-boot-slots";
        let _test_box = TestBox::register(id);

        let held: Vec<_> = futures_util::future::join_all(
            (0..MAX_CONCURRENT_BOOTS).map(|_| registry.boot_slot(id)),
        )
        .await;
        assert_eq!(registry.boots.available_permits(), 0);
        let mut third = Box::pin(registry.boot_slot(id));
        let waited = tokio::time::timeout(Duration::from_millis(30), &mut third).await;
        assert!(waited.is_err(), "a third boot waits for a slot");
        assert_eq!(
            registry.state(id),
            BoxPhase::Preparing {
                detail: WAITING_FOR_SLOT.into()
            }
        );
        drop(third);
        assert_eq!(registry.state(id), BoxPhase::Stopped);
        drop(held);
        assert_eq!(registry.boots.available_permits(), MAX_CONCURRENT_BOOTS);

        let (now, peak) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let mut boots = tokio::task::JoinSet::new();
        for _ in 0..8 {
            let (now, peak) = (now.clone(), peak.clone());
            boots.spawn(async move {
                let _slot = BoxRegistry::global().boot_slot(id).await;
                peak.fetch_max(now.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(10)).await;
                now.fetch_sub(1, Ordering::SeqCst);
            });
        }
        while boots.join_next().await.is_some() {}
        assert_eq!(peak.load(Ordering::SeqCst), MAX_CONCURRENT_BOOTS);
        assert_eq!(registry.boots.available_permits(), MAX_CONCURRENT_BOOTS);
    }

    /// An unknown agent is stopped and has no channel; registering routes its
    /// root and a stop of a box that never ran is `AlreadyExited`.
    #[tokio::test]
    async fn registry_knows_its_entries() {
        let registry = BoxRegistry::global();
        let id = "registry-unit-test";
        assert!(!registry.knows(id));
        assert_eq!(registry.state(id), BoxPhase::Stopped);
        assert!(registry.channel_for_running(id).is_err());
        registry.register(BoxIdentity {
            agent_id: id.into(),
            project_id: "p".into(),
            provider: "claude".into(),
            writable_root: PathBuf::from("/registry-unit-test/root"),
            rpc_dir: PathBuf::from("/registry-unit-test/rpc"),
            home: PathBuf::from("/registry-unit-test/home"),
            shares: Vec::new(),
        });
        assert!(registry.knows(id));
        assert!(matches!(
            crate::exec::for_path(Path::new("/registry-unit-test/root/repo")),
            crate::exec::Executor::Remote(_)
        ));
        assert_eq!(
            registry.stop(id, StopReason::User).await.unwrap(),
            StopOutcome::AlreadyExited
        );
        assert!(registry
            .identity_for_path(Path::new("/registry-unit-test/root/x"))
            .is_some());
        registry.discard(id).await.unwrap();
        assert!(!registry.knows(id));
        assert!(matches!(
            crate::exec::for_path(Path::new("/registry-unit-test/root/repo")),
            crate::exec::Executor::Local
        ));
    }

    /// A tenant's own root is routed to its host's box, the tenant resolves
    /// to its host, and the route ends with the tenant — or with the host's
    /// box, whichever goes first.
    #[tokio::test]
    async fn tenants_are_routed_to_their_host_and_forgotten_with_it() {
        let registry = BoxRegistry::global();
        let host = "tenant-test-host";
        let host_root = PathBuf::from("/tenant-test/host-root");
        registry.register(BoxIdentity {
            agent_id: host.into(),
            project_id: "p".into(),
            provider: "claude".into(),
            writable_root: host_root.clone(),
            rpc_dir: PathBuf::from("/tenant-test/host-rpc"),
            home: PathBuf::from("/tenant-test/home"),
            shares: Vec::new(),
        });
        let tenant_root = PathBuf::from("/tenant-test/tenant-root");
        assert!(matches!(
            crate::exec::for_path(&tenant_root.join(".fletch-profile")),
            crate::exec::Executor::Local
        ));

        registry.register_tenant("tenant-test-reviewer", tenant_root.clone(), host);
        assert!(matches!(
            crate::exec::for_path(&tenant_root.join(".fletch-profile")),
            crate::exec::Executor::Remote(_)
        ));
        assert_eq!(
            registry.tenant_of("tenant-test-reviewer"),
            Some(Tenant {
                host_agent_id: host.into(),
                writable_root: tenant_root.clone(),
            })
        );
        // A tenant is not a box: nothing to boot, no identity of its own.
        assert!(!registry.knows("tenant-test-reviewer"));
        assert!(registry.identity_for_path(&tenant_root).is_none());

        // Forgotten on its own.
        assert!(registry.forget_tenant("tenant-test-reviewer"));
        assert!(!registry.forget_tenant("tenant-test-reviewer"));
        assert!(matches!(
            crate::exec::for_path(&tenant_root.join(".fletch-profile")),
            crate::exec::Executor::Local
        ));

        // Forgotten with its host's box.
        registry.register_tenant("tenant-test-reviewer", tenant_root.clone(), host);
        registry.discard(host).await.unwrap();
        assert_eq!(registry.tenant_of("tenant-test-reviewer"), None);
        assert!(matches!(
            crate::exec::for_path(&tenant_root.join(".fletch-profile")),
            crate::exec::Executor::Local
        ));
    }

    /// A session's hold keeps its box attached until the session's kill, or
    /// until it is dropped when no session came of it; a host tree has no box
    /// to hold.
    #[tokio::test]
    async fn a_session_hold_is_released_by_its_kill_or_its_drop() {
        use crate::sandbox::KillHandle;
        let registry = BoxRegistry::global();
        let id = "session-hold-test";
        registry.register(BoxIdentity {
            agent_id: id.into(),
            project_id: "p".into(),
            provider: "claude".into(),
            writable_root: PathBuf::from("/session-hold-test/root"),
            rpc_dir: PathBuf::from("/session-hold-test/rpc"),
            home: PathBuf::from("/session-hold-test/home"),
            shares: Vec::new(),
        });
        let attached = || {
            registry
                .entries
                .lock()
                .get(id)
                .map_or(0, |e| e.attached.len())
        };

        let tree = Path::new("/session-hold-test/root/repo");
        assert!(super::super::hold_session(Path::new("/session-hold-test/elsewhere")).is_none());

        // Dropped on the way to a session that never started.
        let hold = super::super::hold_session(tree).expect("a box holds the tree");
        assert_eq!(attached(), 1);
        drop(hold);
        assert_eq!(attached(), 0);

        // Handed to the session, whose kill releases it.
        let kill = super::super::hold_session(tree).unwrap().into_kill();
        assert!(matches!(kill, KillHandle::Engine { .. }));
        assert_eq!(attached(), 1);
        kill.kill().unwrap();
        assert_eq!(attached(), 0);

        registry.discard(id).await.unwrap();
    }

    /// `built_tools_disk` asks in that order, with the codegraph disk named
    /// for the pinned release.
    #[test]
    fn the_codegraph_disk_is_named_for_the_pinned_release() {
        // A provider with no CLI to probe for its version.
        let with = tools_disk_path("no-such-provider", true);
        let plain = tools_disk_path("no-such-provider", false);
        assert!(fletch_box::tools::carries_codegraph(&with));
        assert!(!fletch_box::tools::carries_codegraph(&plain));
        assert!(with
            .to_string_lossy()
            .contains(crate::codegraph::CODEGRAPH_VERSION));
        assert_eq!(with.parent(), plain.parent());
    }

    /// Not running and no disk built: no codegraph for the session.
    #[test]
    fn a_box_without_a_codegraph_disk_offers_no_codegraph() {
        let td = tempfile::tempdir().unwrap();
        let disk = fletch_box::tools::disk_path(td.path(), "claude", "2.1.0", None);
        assert!(!fletch_box::tools::carries_codegraph(&disk));
        assert_eq!(
            super::super::codegraph_bin("registry-unit-test-no-box", "no-such-provider"),
            None
        );
    }
}
