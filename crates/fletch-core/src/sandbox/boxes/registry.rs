//! One entry per box-stamped agent: its identity (the paths the guest mirrors),
//! its phase, and while the VM runs the channel, proxy and relays. Process-wide
//! like the container engines.
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
use tokio::sync::watch;

use crate::error::{Error, Result};
use crate::workspace::AgentRecord;

use super::executor::BoxExecutor;
use super::{relay, settings, setup, tools_root, EgressEventPayload, EVENT_STATE, EVENT_TOOLS};

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
    /// The image is hashed once per process before the first boot
    /// (`ImageStore::verify`); a damaged image is removed and reported.
    image_verified: Mutex<HashSet<String>>,
    /// The tools disk build running or last run per `<provider>-<version>`:
    /// a launch waits on a running one, the UI reads a failed one.
    tools_builds: Mutex<HashMap<String, watch::Receiver<ToolsBuild>>>,
}

impl BoxRegistry {
    pub fn global() -> &'static BoxRegistry {
        static REGISTRY: OnceLock<BoxRegistry> = OnceLock::new();
        REGISTRY.get_or_init(|| BoxRegistry {
            entries: Mutex::new(HashMap::new()),
            image_verified: Mutex::new(HashSet::new()),
            tools_builds: Mutex::new(HashMap::new()),
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
        if self.discarded(agent_id) {
            return Err(discarded_error(agent_id));
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
        if self.discarded(agent_id) {
            return Err(discarded_error(agent_id));
        }
        self.set_phase(agent_id, BoxPhase::Starting);
        match boot(identity, &ready, Some(tools_disk)).await {
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

    /// The tools disk a box of `provider` attaches, built once per
    /// `<provider>-<version>`. A missing disk is built now (the toggle's
    /// prebuild normally has it done first) and waited for, up to
    /// [`setup::TOOLS_WAIT`], with the box reported `Preparing` meanwhile;
    /// the spawn watchdog allows for this (`setup::setup_allowance`).
    async fn tools_disk(
        &self,
        ready: &super::Ready,
        agent_id: &str,
        provider: &str,
    ) -> Result<PathBuf> {
        let path = tools_disk_path(provider);
        if path.is_file() {
            return Ok(path);
        }
        let build = self.start_tools_build(ready, provider);
        let label = crate::agent::provider_bin_label(provider).map_or(provider, |(_, l)| l);
        self.set_phase(
            agent_id,
            BoxPhase::Preparing {
                detail: format!("installing {label} {}", tools_version(provider)),
            },
        );
        wait_tools_build(build, setup::TOOLS_WAIT).await?;
        if !path.is_file() {
            return Err(Error::SandboxUnavailable(format!(
                "the {provider} tools disk was reported built but {} is missing",
                path.display()
            )));
        }
        Ok(path)
    }

    /// Where `provider`'s tools disk stands, for the probe.
    pub fn tools_status(&self, provider: &str) -> ToolsStatus {
        let version = tools_version(provider);
        let (phase, error) = if tools_disk_path(provider).is_file() {
            ("ready", None)
        } else {
            match self
                .tools_builds
                .lock()
                .get(&format!("{provider}-{version}"))
                .map(|rx| rx.borrow().clone())
            {
                Some(ToolsBuild::Building) => ("building", None),
                Some(ToolsBuild::Failed(e)) => ("failed", Some(e)),
                // `Ready` with no file: the disk was removed since; a launch
                // builds it again.
                Some(ToolsBuild::Ready) | None => ("missing", None),
            }
        };
        ToolsStatus {
            provider: provider.to_string(),
            version,
            phase,
            error,
        }
    }

    /// Build `provider`'s tools disk for the current version in the background
    /// (`box:tools` events) and hand back a receiver of its outcome. Joins a
    /// build already running; a finished one (failed, or built and since
    /// removed) is replaced by a fresh attempt.
    pub fn start_tools_build(
        &self,
        ready: &super::Ready,
        provider: &str,
    ) -> watch::Receiver<ToolsBuild> {
        let version = tools_version(provider);
        let out = tools_disk_path(provider);
        let key = format!("{provider}-{version}");
        let mut builds = self.tools_builds.lock();
        if let Some(rx) = builds.get(&key) {
            if *rx.borrow() == ToolsBuild::Building {
                return rx.clone();
            }
        }
        let (tx, rx) = watch::channel(ToolsBuild::Building);
        builds.insert(key, rx.clone());
        drop(builds);
        let (paths, image) = (ready.paths.clone(), ready.image.path());
        let provider = provider.to_string();
        crate::host::spawn(async move {
            let event = |phase, error: Option<String>| {
                super::emit(
                    EVENT_TOOLS,
                    &ToolsStatus {
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
                    tx.send_replace(ToolsBuild::Ready);
                    event("ready", None);
                }
                Err(e) => {
                    tracing::warn!(provider, version, error = %e, "tools disk build failed");
                    tx.send_replace(ToolsBuild::Failed(e.to_string()));
                    event("failed", Some(e.to_string()));
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
        remove_box_dir(&super::box_dir(agent_id)?).await
    }

    /// Whether `agent_id`'s box is being discarded, or already was (it is no
    /// longer known).
    fn discarded(&self, agent_id: &str) -> bool {
        self.entries
            .lock()
            .get(agent_id)
            .map_or(true, |e| e.discarding)
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
            "the tools disk build failed: {e}"
        ))),
        Err(_) => Err(Error::SandboxUnavailable(format!(
            "the tools disk build did not finish within {} minutes",
            timeout.as_secs() / 60
        ))),
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
        &dir,
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
fn discarded_error(agent_id: &str) -> Error {
    Error::SandboxUnavailable(format!("the box for {agent_id} was discarded"))
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
        });
        // The fake build stands in for `start_tools_build`'s task: same key,
        // same channel shape.
        let (tx, rx) = watch::channel(ToolsBuild::Building);
        registry
            .tools_builds
            .lock()
            .insert(format!("{provider}-{}", tools_version(provider)), rx);
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
