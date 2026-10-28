//! One entry per box-stamped agent: its identity (the paths the guest mirrors),
//! its phase, and while the VM runs the channel, proxy and relays
//! (docs/boxes-spec.md §7.7). Process-wide like the container engines.
//!
//! A box's writable root is routed to its executor for as long as the registry
//! knows the box, not only while the VM runs: a routed operation on a stopped
//! box (`git status` for the Git panel) boots it again instead of failing on a
//! host path that holds nothing. Unregistering happens on [`BoxRegistry::discard`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use fletch_box::channel;
use fletch_box::disks::{self, Dirty};
use fletch_box::egress::{EgressEvent, EgressPolicy, EgressProxy};
use fletch_box::{guest, Channel, StopOutcome, Vm, VmSpec};
use parking_lot::Mutex;
use serde::Serialize;

use crate::error::{Error, Result};
use crate::workspace::AgentRecord;

use super::executor::BoxExecutor;
use super::{relay, settings, tools_root, EgressEventPayload, EVENT_STATE, EVENT_TOOLS};

pub const DATA_DISK: &str = "data.img";
pub const CONSOLE: &str = "console.log";
pub const SSH_SOCK: &str = "ssh.sock";
pub const CTL_SOCK: &str = "ctl.sock";
pub const EGRESS_SOCK: &str = "egress.sock";
pub const CLAUDE_PROJECTS_DIR: &str = "claude-projects";
pub const MIRROR_DIR: &str = "mirror.git";

/// `FLETCH_READY` must arrive within this, else the boot fails closed.
const READY_TIMEOUT: Duration = Duration::from_secs(10);
/// The stop ladder's rungs (§7.7).
const CONTROL_GRACE: Duration = Duration::from_secs(10);
const SYSRQ_GRACE: Duration = Duration::from_secs(5);
const TERM_GRACE: Duration = Duration::from_secs(3);
const KILL_GRACE: Duration = Duration::from_secs(5);

/// The paths one box mirrors from the host, and who it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxIdentity {
    pub agent_id: String,
    pub project_id: String,
    pub provider: String,
    pub writable_root: PathBuf,
    pub rpc_dir: PathBuf,
    pub home: PathBuf,
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
        })
    }

    pub fn for_record(record: &AgentRecord) -> Result<Self> {
        Self::new(&record.id, &record.project_id, &record.provider)
    }

    pub fn box_dir(&self) -> Result<PathBuf> {
        super::box_dir(&self.agent_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxPhase {
    Stopped,
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
}

/// `box:state`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BoxStateReport {
    pub agent_id: String,
    pub phase: &'static str,
    pub since: Option<i64>,
    pub reason: Option<String>,
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
        }
    }
}

/// `box:tools`.
#[derive(Debug, Clone, Serialize)]
struct ToolsEventPayload {
    provider: String,
    version: String,
    phase: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// A box with its VM up.
pub struct RunningBox {
    pub identity: BoxIdentity,
    pub dir: PathBuf,
    pub channel: Arc<Channel>,
    pub since: i64,
    /// The provider's CLI disk, when one was built by boot time.
    pub tools_disk: Option<PathBuf>,
    vm: Vm,
    proxy: Mutex<Option<EgressProxy>>,
    relays: relay::Relays,
    dirty: Dirty,
}

impl RunningBox {
    pub fn agent_id(&self) -> &str {
        &self.identity.agent_id
    }

    /// `box:<path>` for git over the channel.
    pub fn git_url(&self, guest_path: &Path) -> String {
        format!("{}:{}", guest::SSH_HOST_ALIAS, guest_path.display())
    }
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
        }
    }
}

pub struct BoxRegistry {
    entries: Mutex<HashMap<String, Entry>>,
    /// The image is hashed once per process before the first boot
    /// (`ImageStore::verify`); a damaged image is removed and reported.
    image_verified: Mutex<HashSet<String>>,
    /// Tools disks being built right now, by `<provider>-<version>`.
    tools_building: Mutex<HashSet<String>>,
}

impl BoxRegistry {
    pub fn global() -> &'static BoxRegistry {
        static REGISTRY: OnceLock<BoxRegistry> = OnceLock::new();
        REGISTRY.get_or_init(|| BoxRegistry {
            entries: Mutex::new(HashMap::new()),
            image_verified: Mutex::new(HashSet::new()),
            tools_building: Mutex::new(HashSet::new()),
        })
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

    fn running(&self, agent_id: &str) -> Option<Arc<RunningBox>> {
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

    /// Boot `identity`'s box unless it runs already (docs/boxes-spec.md §7.2
    /// step 1). Fails closed with the reason, the console tail included when
    /// the guest never became ready.
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
        let ready = super::availability()?;
        self.verify_image_once(&ready).await?;
        let tools_disk = self.tools_disk(&ready, &identity.provider);
        self.set_phase(&identity.agent_id, BoxPhase::Starting);
        match boot(identity, &ready, tools_disk).await {
            Ok(running) => {
                let running = Arc::new(running);
                let arm_idle = {
                    let mut entries = self.entries.lock();
                    let entry = entries
                        .get_mut(&identity.agent_id)
                        .expect("registered above");
                    entry.running = Some(running.clone());
                    entry.phase = BoxPhase::Running {
                        since: running.since,
                    };
                    entry.idle_gen += 1;
                    entry.attached.is_empty().then_some(entry.idle_gen)
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

    /// Hash the installed image against its pin once per process, off the
    /// runtime (800 MB takes a second or two).
    async fn verify_image_once(&self, ready: &super::Ready) -> Result<()> {
        if self.image_verified.lock().contains(&ready.image.version) {
            return Ok(());
        }
        let image = ready.image.clone();
        let pinned = super::pinned_image();
        tokio::task::spawn_blocking(move || super::image_store().verify(&image, Some(&pinned)))
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

    /// The tools disk a launch of `provider` attaches, built once per
    /// `<provider>-<version>`. A missing disk is not built on the spawn path —
    /// an npm install over the proxy takes longer than the spawn watchdog
    /// allows — but in the background (`box:tools` events), and the box boots
    /// without one meanwhile so provisioning can proceed; a *launch* needs the
    /// disk and restarts the box to attach it once built (`BoxEngine::launch_agent`).
    fn tools_disk(&self, ready: &super::Ready, provider: &str) -> Option<PathBuf> {
        let version = tools_version(provider);
        let path = tools_disk_path(provider);
        if path.is_file() {
            return Some(path);
        }
        tracing::info!(
            provider,
            version,
            "no tools disk yet; booting without one and building it"
        );
        self.build_tools_in_background(ready, provider, &version, &path);
        None
    }

    fn build_tools_in_background(
        &self,
        ready: &super::Ready,
        provider: &str,
        version: &str,
        out: &Path,
    ) {
        let key = format!("{provider}-{version}");
        if !self.tools_building.lock().insert(key.clone()) {
            return;
        }
        let (paths, image) = (ready.paths.clone(), ready.image.path());
        let (provider, version, out) =
            (provider.to_string(), version.to_string(), out.to_path_buf());
        crate::host::spawn(async move {
            let event = |phase, error: Option<String>| {
                super::emit(
                    EVENT_TOOLS,
                    &ToolsEventPayload {
                        provider: provider.clone(),
                        version: version.clone(),
                        phase,
                        error,
                    },
                )
            };
            event("building", None);
            let built = fletch_box::tools::build(
                &paths,
                &image,
                &provider,
                &version,
                &out,
                |line| tracing::info!(target: "fletch::box_tools", provider, line, "tools build"),
            )
            .await;
            match built {
                Ok(()) => {
                    tracing::info!(provider, version, path = %out.display(), "tools disk built");
                    event("ready", None);
                }
                Err(e) => {
                    tracing::warn!(provider, version, error = %e, "tools disk build failed");
                    event("failed", Some(e.to_string()));
                }
            }
            BoxRegistry::global().tools_building.lock().remove(&key);
        });
    }

    /// Stop `agent_id`'s VM through the ladder (§7.7): the mirror refreshed
    /// first so publish has the objects while the box is down, then `stop` on
    /// the control port, the ssh sysrq rung, SIGTERM, SIGKILL. `dirty` is
    /// cleared only after a clean rung.
    pub async fn stop(&self, agent_id: &str, reason: StopReason) -> Result<StopOutcome> {
        let (running, lifecycle) = {
            let mut entries = self.entries.lock();
            let Some(entry) = entries.get_mut(agent_id) else {
                return Ok(StopOutcome::AlreadyExited);
            };
            let Some(running) = entry.running.take() else {
                return Ok(StopOutcome::AlreadyExited);
            };
            entry.phase = BoxPhase::Stopping;
            entry.idle_gen += 1;
            (running, entry.lifecycle.clone())
        };
        self.emit_state(agent_id);
        let _guard = lifecycle.lock().await;
        if reason != StopReason::Discard {
            if let Err(e) = super::mirror::refresh_running(&running).await {
                tracing::info!(agent_id, error = %e, "mirror refresh before stop failed");
            }
        }
        running.relays.stop().await;
        let outcome = stop_ladder(&running.vm, &running.channel).await;
        if outcome.clean() {
            if let Err(e) = running.dirty.clear() {
                tracing::warn!(agent_id, error = %e, "clearing the dirty marker failed");
            }
        } else {
            tracing::warn!(agent_id, "box was killed; its disk is marked dirty");
        }
        drop(running.proxy.lock().take());
        tracing::info!(agent_id, ?reason, ?outcome, "box stopped");
        self.set_phase(agent_id, BoxPhase::Stopped);
        Ok(outcome)
    }

    /// Stop the box and delete its directory: the agent is gone. Forgets the
    /// box and stops routing its root.
    pub async fn discard(&self, agent_id: &str) -> Result<()> {
        let _ = self.stop(agent_id, StopReason::Discard).await?;
        let identity = self.entries.lock().remove(agent_id).map(|e| e.identity);
        if let Some(identity) = &identity {
            crate::exec::unregister_root(&identity.writable_root);
        }
        let dir = super::box_dir(agent_id)?;
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => tracing::info!(agent_id, "box directory removed"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(Error::Other(format!(
                    "remove box dir {}: {e}",
                    dir.display()
                )))
            }
        }
        Ok(())
    }

    /// A launch began a session in the box; the idle timer stands down.
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
            let still_idle =
                registry.entries.lock().get(&agent_id).is_some_and(|e| {
                    e.idle_gen == gen && e.attached.is_empty() && e.running.is_some()
                });
            if still_idle {
                if let Err(e) = registry.stop(&agent_id, StopReason::Idle).await {
                    tracing::warn!(agent_id, error = %e, "idle stop failed");
                }
            }
        });
    }

    /// Stop every running box; app exit.
    pub async fn shutdown_all(&self) {
        for id in self.running_ids() {
            if let Err(e) = self.stop(&id, StopReason::Shutdown).await {
                tracing::warn!(agent_id = %id, error = %e, "box shutdown failed");
            }
        }
    }

    /// A project's egress additions changed: every running box of that
    /// project serves the new policy from its next connection.
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

/// The CLI version a provider's tools disk installs: the host CLI's, as the
/// container engines' version-parity check reads it, else `latest`.
pub fn tools_version(provider: &str) -> String {
    crate::agent::cached_provider_version(provider).unwrap_or_else(|| "latest".to_string())
}

/// Where `provider`'s tools disk for the current version is (or will be).
pub fn tools_disk_path(provider: &str) -> PathBuf {
    fletch_box::tools::disk_path(&tools_root(), provider, &tools_version(provider))
}

/// The defaults plus the project's additions; a stored pattern that no longer
/// parses is dropped with a warning rather than blocking the launch.
pub fn egress_policy(project_id: &str) -> EgressPolicy {
    let extra = settings::egress_allow(project_id);
    match EgressPolicy::with_extra(&extra) {
        Ok(policy) => policy,
        Err(e) => {
            tracing::warn!(project_id, error = %e, "ignoring the project's egress additions");
            EgressPolicy::default()
        }
    }
}

/// Boot one box (§7.2 step 1).
async fn boot(
    identity: &BoxIdentity,
    ready: &super::Ready,
    tools_disk: Option<PathBuf>,
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
    spec.tools_disk = tools_disk.clone();
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

    let relays = relay::Relays::start(
        channel.clone(),
        &identity.rpc_dir,
        &identity.home.join(".claude").join("projects"),
        &dir.join(CLAUDE_PROJECTS_DIR),
    );
    Ok(RunningBox {
        identity: identity.clone(),
        dir,
        channel,
        since: chrono::Utc::now().timestamp_millis(),
        tools_disk,
        vm,
        proxy: Mutex::new(Some(proxy)),
        relays,
        dirty,
    })
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
            },
        ),
        EgressEvent::Denied { host, port, reason } => {
            tracing::info!(agent_id, host, port, %reason, "box egress denied");
            super::emit(
                super::EVENT_EGRESS,
                &EgressEventPayload {
                    agent_id: agent_id.to_string(),
                    host,
                    port,
                    allowed: false,
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
async fn stop_ladder(vm: &Vm, channel: &Channel) -> StopOutcome {
    if matches!(vm.try_wait(), Ok(Some(_))) {
        return StopOutcome::AlreadyExited;
    }
    match vm.send_stop().await {
        Ok(()) => {
            if vm.wait_exit(CONTROL_GRACE).await.is_some() {
                return StopOutcome::Control;
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
    if matches!(sysrq, Ok(out) if out.status.success()) && vm.wait_exit(SYSRQ_GRACE).await.is_some()
    {
        return StopOutcome::Sysrq;
    }
    vm.terminate();
    if vm.wait_exit(TERM_GRACE).await.is_some() {
        return StopOutcome::Term;
    }
    vm.kill();
    vm.wait_exit(KILL_GRACE).await;
    StopOutcome::Killed
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
