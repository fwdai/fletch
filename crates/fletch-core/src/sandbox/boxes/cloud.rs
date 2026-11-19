//! Boxes whose machine is made at a cloud provider rather than on this
//! machine ([`crate::cloud`]): the same guest (`packaging/cloud-image`),
//! reached over ssh across the network. The engine, the workspace's record,
//! the GitHub login and the provider's API token all stay here; nothing
//! secret goes to the machine but what any box gets, each launch's env file.
//! A cloud box is a box: the registry boots, stops, idles out and discards it
//! as any other, and everything it does with a running box goes over the
//! channel. What differs is the machine (`registry::Machine::Cloud`, a
//! [`CloudMachine`]), and this module is what only a cloud box does. Nothing
//! here names a provider: each is a [`ProviderId`] with a label, its box a
//! [`Handle`], and every answer that differs per provider is its adapter's.
//!
//! - **Which box is one** is the workspace's record: `workspaces.cloud_provider`,
//!   stamped at spawn from the workspace type ([`backing`]). The registry and
//!   the launch path ask by agent id, through a database handle installed once
//!   at boot ([`install`], as `egress_log` is); with none installed (every
//!   unit test) no box is a cloud box and no provider is connected.
//! - **Its box at the provider** is named by a [`Handle`], kept in the box
//!   directory ([`HANDLE_FILE`], owner-only) from the moment the provider
//!   made it, and read at every boot. A box exists here once it has one
//!   (`boxes_in`), as a local box does once it has a data disk.
//! - **A boot** ([`machine_up`], then the channel) creates the box when there
//!   is no handle, else brings its machine to running: started when
//!   stopped, replaced on its disk when the machine alone was lost. A disk
//!   that is gone is the workspace's files gone, and is refused
//!   ([`Error::CloudGone`]), never replaced by an empty one; a provider that
//!   cannot run the machine now is refused with its reason, and nothing is
//!   destroyed. The guest's host key, kept on the box's disk, is read
//!   through the provider's authenticated exec and pinned at the box's first
//!   boot (and at any boot that finds no pin), then ssh is waited for: a new
//!   machine's name takes a while to resolve. Nothing local is made: no data
//!   disk, no dirty marker, no egress proxy, no tools disk.
//! - **A spare** ([`super::spares`]) is a box made ahead of any workspace,
//!   under a placeholder identity of its own, its CLIs installed and its
//!   machine parked. A box with nothing made for it yet takes one at its
//!   first boot and wakes its machine instead of making one. Its guest
//!   started with the placeholder's paths in its environment, which is never
//!   changed, so every boot binds the box's own paths in the guest over the
//!   channel where they are not bound yet ([`bind_identity`]): once after the
//!   take, and again after any cold start, while a box made for its
//!   workspace has them bound by its entrypoint and binds nothing.
//! - **Its CLIs** live on its own disk, under `/data/tools/<provider>`,
//!   where a tools disk would hold them (`fletch_box::tools::install_cli`):
//!   the owner's is installed at boot at the version a local box would get
//!   (`registry::tools_version`), and a tenant's of another provider on its
//!   first launch, with no restart ([`CloudMachine::ensure_cli`]). A version
//!   is recorded on the disk only once the guest's sync has put its files
//!   there, in the background of the launch, and a clean stop lets a record
//!   still being made finish first; a machine lost or killed before then has
//!   no record, and its next boot installs again. Codegraph
//!   is not installed in a cloud box (its bundle is per-architecture and
//!   built for local boxes), so a cloud box offers none (`codegraph_bin`).
//! - **A stop** syncs the guest's disks over the channel, then has the
//!   provider stop the machine ([`CloudMachine::halt`]). A box stopped for
//!   being idle has its machine parked instead ([`Client::park`]): at rest
//!   in whichever state the provider wakes it from soonest, everything on it
//!   kept, and woken by the next boot as a stopped one is; whether the guest
//!   then resumed or started afresh is the provider's, and nothing here
//!   depends on it. A stop or park that fails leaves a machine that may be
//!   running and billed, and says so in the log with its handle.
//! - **A discard** destroys the box at the provider before its directory
//!   goes (`registry`), and keeps the directory when it cannot: the handle
//!   and the key are the only way back to it. At startup a directory no live
//!   workspace owns is destroyed the same way first ([`destroy_box`]), and a
//!   live box's machine left running by an app that died is stopped
//!   (`BoxRegistry::stop_stray_machines`). Nothing is destroyed at a provider
//!   that this install has no directory for: two installs (a debug and a
//!   release build, two machines) may share one account.
//! - **Not in a cloud box**: the folders a user shares with a workspace
//!   (a machine elsewhere cannot mount them; [`no_shares`]), the egress
//!   proxy and its policy (the machine's network is its own), a project's
//!   seed, codegraph, and moving the workspace to or from this machine.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use fletch_box::channel::{self, Route};
use fletch_box::{guest, Channel, HostKey, PinnedImage, StopOutcome};
use parking_lot::Mutex;

use crate::cloud::{self, Client, Endpoint, Handle, MachineSpec, MachineState, ProviderId};
use crate::error::{Error, Result};
use crate::roadmap::Db;

use super::registry::{BoxIdentity, BoxPhase, BoxRegistry, StopReason};
use super::settings;

/// The file in a cloud box's directory holding its [`Handle`].
pub const HANDLE_FILE: &str = "cloud";

/// Replaces the pinned cloud image with a whole image reference, for testing
/// with an image of one's own (`packaging/cloud-image/README.md`).
pub const IMAGE_ENV: &str = "FLETCH_CLOUD_IMAGE";

/// How long a machine on its way to a state is waited for before a boot
/// gives up on it.
const SETTLE_BOUND: Duration = Duration::from_secs(120);
/// The pause between two reads of a machine on its way.
const SETTLE_PACE: Duration = Duration::from_secs(2);
/// How long the guest's host key may take to appear once the machine runs
/// (the entrypoint writes it before sshd starts), and the pause between
/// reads.
const HOST_KEY_BOUND: Duration = Duration::from_secs(30);
const HOST_KEY_PACE: Duration = Duration::from_secs(2);
const HOST_KEY_EXEC: Duration = Duration::from_secs(10);
/// How long the guest may take to answer over ssh once its machine runs: a
/// new box's name takes a while to resolve. The pause between two tries
/// starts short, for a machine that answers a moment after the first, and
/// doubles up to the longest ([`ssh_pauses`]).
const SSH_BOUND: Duration = Duration::from_secs(90);
const SSH_FIRST_PACE: Duration = Duration::from_millis(250);
const SSH_PACE: Duration = Duration::from_secs(3);
/// The `sync` a stop asks of the guest before the provider stops it.
const SYNC_BOUND: Duration = Duration::from_secs(5);
/// How long a stop with no bound of its own waits for CLI records still being
/// written: a sync of what an install wrote takes seconds, and a guest that
/// stopped answering must not keep its machine running.
const RECORD_BOUND: Duration = Duration::from_secs(60);
/// How long a kill waits for the provider to confirm the stop; the request
/// is made whatever comes of the wait.
const KILL_BOUND: Duration = Duration::from_secs(10);
/// The spawn watchdog's extra time for a cloud box: making one (the box, its
/// image pulled, the CLI installed), or starting one that exists.
const CREATE_ALLOWANCE: Duration = Duration::from_secs(10 * 60);
const START_ALLOWANCE: Duration = Duration::from_secs(3 * 60);

static DB: OnceLock<Db> = OnceLock::new();

/// Where the workspace records and the provider connections are read from.
/// Installed once at boot.
pub fn install(db: Db) {
    if DB.set(db).is_err() {
        tracing::warn!("cloud box database already set");
    }
}

/// The provider whose machine `agent_id`'s box is, from its record
/// (`workspaces.cloud_provider`); `Ok(None)` for a box on this machine, an
/// agent with no record, and before [`install`]. A provider this build does
/// not know, or a record that cannot be read, is an error, never a box on
/// this machine.
pub fn backing(agent_id: &str) -> Result<Option<ProviderId>> {
    use rusqlite::OptionalExtension;
    let Some(db) = DB.get() else {
        return Ok(None);
    };
    let raw: Option<Option<String>> = db
        .lock()
        .query_row(
            "SELECT cloud_provider FROM workspaces WHERE id = ?1",
            [agent_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(raw) = raw.flatten() else {
        return Ok(None);
    };
    ProviderId::parse(&raw).map(Some).ok_or_else(|| {
        Error::SandboxUnavailable(format!(
            "the box of {agent_id} is at a cloud provider this version of Fletch doesn't know \
             (\"{raw}\")"
        ))
    })
}

/// The provider whose machine holds the box `agent_id` runs in: its own, or
/// for a tenant its host's. `None` for a box on this machine, and where the
/// record says something [`backing`] refuses.
pub fn box_backing(agent_id: &str) -> Option<ProviderId> {
    let tenant = BoxRegistry::global().tenant_of(agent_id);
    let owner = tenant
        .as_ref()
        .map_or(agent_id, |t| t.host_agent_id.as_str());
    backing(owner).ok().flatten()
}

/// `provider`'s connection on this machine: `None` when it is not connected,
/// and before [`install`]. An unreadable secret store is an error.
pub fn connection(provider: ProviderId) -> Result<Option<cloud::Connection>> {
    match DB.get() {
        Some(db) => cloud::connection(&db.lock(), provider),
        None => Ok(None),
    }
}

/// A client for `provider`, refused with the sentence the user sees when it
/// is not connected on this machine.
pub fn client(provider: ProviderId) -> Result<Client> {
    match connection(provider)? {
        Some(connection) => Client::new(&connection),
        None => Err(Error::SandboxUnavailable(not_connected(provider))),
    }
}

/// Why a cloud box at `provider` cannot be had while it is not connected.
pub fn not_connected(provider: ProviderId) -> String {
    format!(
        "{} isn't connected on this machine. Connect it in Settings → Cloud.",
        provider.label()
    )
}

/// Why a box at `provider` gets no folder of this machine.
pub fn no_shares(provider: ProviderId) -> String {
    format!(
        "A {} workspace can't see folders on this machine: its machine is at the provider, with \
         no way to mount them. Remove the folders from this workspace, or start a workspace in \
         a Linux VM or on this Mac to work with them.",
        provider.label()
    )
}

/// The cloud image a new machine runs: the pinned one
/// (`PinnedImage::cloud_image`), or [`IMAGE_ENV`]'s.
pub fn image() -> String {
    std::env::var(IMAGE_ENV)
        .ok()
        .map(|image| image.trim().to_string())
        .filter(|image| !image.is_empty())
        .unwrap_or_else(PinnedImage::cloud_image)
}

/// The box `dir` holds a handle of, if any.
pub fn read_handle(dir: &Path) -> Result<Option<Handle>> {
    match std::fs::read_to_string(dir.join(HANDLE_FILE)) {
        Ok(text) => Handle::parse(text.trim()).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::Other(format!(
            "read {}: {e}",
            dir.join(HANDLE_FILE).display()
        ))),
    }
}

/// Keep `handle` in `dir`, replacing the one there, owner-only and whole.
fn write_handle(dir: &Path, handle: &Handle) -> Result<()> {
    super::ssh_access::write_private(&dir.join(HANDLE_FILE), &format!("{handle}\n"))
}

/// The box a new machine for `identity` is made from: the cloud image, the
/// guest's environment (the identity binds, the box's ssh key, the box id,
/// the sshd port) and the size the box settings give a VM. Nothing secret.
pub(super) fn spec(identity: &BoxIdentity, authorized_key: String) -> MachineSpec {
    let knobs = settings::current();
    let mut env = super::registry::identity_env(identity, authorized_key);
    env.extend([
        (guest::ENV_BOX_ID.to_string(), identity.agent_id.clone()),
        (
            guest::ENV_SSH_PORT.to_string(),
            guest::CLOUD_SSH_PORT.to_string(),
        ),
    ]);
    MachineSpec {
        name: identity.agent_id.clone(),
        image: image(),
        env,
        cpus: u32::from(knobs.cpus),
        memory_mib: knobs.memory_mib,
        disk_gib: Some(knobs.disk_gib),
        ssh_port: guest::CLOUD_SSH_PORT,
    }
}

/// Bring the machine of the box in `dir` to running, its guest's host key
/// pinned there, through `client`; answers the box's handle. With no handle
/// in `dir` the box is made from `spec` and its handle kept at once; with
/// one, its machine is started, waited for, or replaced on its disk, as its
/// state asks. A box whose disk is gone is refused ([`Error::CloudGone`])
/// and nothing is made in its place; a machine the provider cannot run is
/// refused with why, and nothing destroyed. `phase` hears what the boot
/// does. Once the machine runs, a failure stops it, its handle kept.
///
/// The guest keeps its host key on the box's disk, so the key is the box's
/// for its whole life, through a replaced machine too: it is read through
/// the provider for a box just made and for a directory that holds no pin,
/// and every other boot trusts the pin, which the ssh handshake checks
/// ([`reach`]).
pub async fn machine_up(
    client: &Arc<Client>,
    dir: &Path,
    spec: &MachineSpec,
    phase: &(dyn Fn(BoxPhase) + Sync),
) -> Result<Handle> {
    let label = client.provider().label();
    let pinned = std::fs::metadata(dir.join(channel::KNOWN_HOSTS_FILE)).is_ok_and(|m| m.len() > 0);
    let (handle, made) = match read_handle(dir)? {
        None => {
            phase(BoxPhase::Preparing {
                detail: format!("creating a {label} machine"),
            });
            let created = client.create(spec).await?;
            if let Err(e) = write_handle(dir, &created.handle) {
                tracing::warn!(
                    handle = %created.handle,
                    error = %e,
                    "a new cloud box's handle could not be kept; destroying the box"
                );
                if let Err(destroyed) = client.destroy(&created.handle).await {
                    tracing::warn!(
                        handle = %created.handle,
                        error = %destroyed,
                        "a cloud box whose handle was lost could not be destroyed; it is billed \
                         until it is deleted at the provider"
                    );
                }
                return Err(e);
            }
            (created.handle, true)
        }
        Some(handle) => (to_running(client, dir, handle, spec, phase).await?, false),
    };
    if pinned && !made {
        return Ok(handle);
    }
    if let Err(e) = pin_host_key(client, &handle, dir).await {
        drop(stop_in_background(
            client.clone(),
            handle.clone(),
            spec.name.clone(),
            false,
        ));
        return Err(e);
    }
    Ok(handle)
}

/// [`machine_up`] for a box that exists: its machine to running, whatever
/// state it is in; answers the handle it has now.
async fn to_running(
    client: &Client,
    dir: &Path,
    handle: Handle,
    spec: &MachineSpec,
    phase: &(dyn Fn(BoxPhase) + Sync),
) -> Result<Handle> {
    let label = client.provider().label();
    let deadline = Instant::now() + SETTLE_BOUND;
    loop {
        match client.state(&handle).await? {
            MachineState::Running => return Ok(handle),
            MachineState::Stopped => {
                client.start(&handle).await?;
                return Ok(handle);
            }
            MachineState::Starting | MachineState::Stopping => {
                if Instant::now() >= deadline {
                    return Err(Error::SandboxUnavailable(format!(
                        "the {label} machine of this workspace did not settle within {} s; try \
                         again",
                        SETTLE_BOUND.as_secs()
                    )));
                }
                tokio::time::sleep(SETTLE_PACE).await;
            }
            MachineState::MachineGone => {
                phase(BoxPhase::Preparing {
                    detail: format!("replacing the {label} machine"),
                });
                let replaced = client.replace_machine(&handle, spec).await?;
                write_handle(dir, &replaced.handle)?;
                tracing::info!(old = %handle, new = %replaced.handle, "replaced a cloud box's lost machine");
                return Ok(replaced.handle);
            }
            MachineState::DiskGone | MachineState::Gone => {
                return Err(Error::CloudGone(format!(
                    "This workspace's {label} disk is gone, and its files with it. Fletch won't \
                     start it on an empty one: discard the workspace, or keep it to read its \
                     history."
                )))
            }
            MachineState::Failed(reason) => {
                return Err(Error::SandboxUnavailable(format!(
                    "{label} can't run this workspace's machine now: {reason}"
                )))
            }
        }
    }
}

/// Read the guest's host key through the provider's exec and pin it as the
/// only key the box's channel trusts. Asked a few times: the entrypoint may
/// not have written it the instant the machine reports running.
async fn pin_host_key(client: &Client, handle: &Handle, dir: &Path) -> Result<()> {
    let argv = ["cat".to_string(), guest::CLOUD_HOST_KEY.to_string()];
    let deadline = Instant::now() + HOST_KEY_BOUND;
    loop {
        let why = match client.exec(handle, &argv, HOST_KEY_EXEC).await {
            Ok(out) if out.exit_code == 0 => match HostKey::from_public_key(out.stdout.trim()) {
                Ok(key) => {
                    return channel::write_known_hosts(&dir.join(channel::KNOWN_HOSTS_FILE), &key)
                        .map_err(|e| Error::Other(e.to_string()))
                }
                Err(e) => e.to_string(),
            },
            Ok(out) => format!("exit {}: {}", out.exit_code, out.stderr.trim()),
            Err(e) => e.to_string(),
        };
        if Instant::now() >= deadline {
            return Err(Error::SandboxUnavailable(format!(
                "could not read the cloud box's ssh host key: {why}"
            )));
        }
        tokio::time::sleep(HOST_KEY_PACE).await;
    }
}

/// The channel to the guest of the box `handle` names, authenticating with
/// `key` and trusting the host key pinned in `dir` ([`machine_up`]), once its
/// sshd answers. A guest whose key is not the pinned one is refused at once:
/// no wait makes it right.
pub(super) async fn reach(dir: &Path, key: &Path, handle: &Handle) -> Result<Channel> {
    let endpoint = handle.endpoint();
    let route = route(&endpoint)?;
    let known_hosts = dir.join(channel::KNOWN_HOSTS_FILE);
    let channel = Channel::with_route(dir, route, key, &known_hosts)
        .map_err(|e| Error::Other(e.to_string()))?;
    let label = handle.provider().label();
    let deadline = Instant::now() + SSH_BOUND;
    let mut pauses = ssh_pauses();
    loop {
        let why = match channel.exec(None, None, &["true"]).await {
            Ok(out) if out.status.success() => return Ok(channel),
            Ok(out) => String::from_utf8_lossy(&out.stderr).trim().to_string(),
            Err(e) => e.to_string(),
        };
        // What ssh says when the guest's key is not the one pinned.
        if why.contains("Host key verification failed") {
            return Err(Error::SandboxUnavailable(format!(
                "the {label} machine's ssh host key is not the one pinned for this workspace in {}, \
                 so Fletch won't connect to it. If you know why its key changed, delete that file \
                 and start the workspace again: the key the machine has is then pinned.",
                known_hosts.display()
            )));
        }
        if Instant::now() >= deadline {
            return Err(Error::SandboxUnavailable(format!(
                "the {label} machine did not answer over ssh within {} s: {why}",
                SSH_BOUND.as_secs()
            )));
        }
        tokio::time::sleep(pauses.next().unwrap_or(SSH_PACE)).await;
    }
}

/// The pauses between [`reach`]'s tries: [`SSH_FIRST_PACE`], doubling up to
/// [`SSH_PACE`].
fn ssh_pauses() -> impl Iterator<Item = Duration> {
    std::iter::successors(Some(SSH_FIRST_PACE), |pause| {
        Some((*pause * 2).min(SSH_PACE))
    })
}

/// How ssh reaches a guest at `endpoint`.
pub(super) fn route(endpoint: &Endpoint) -> Result<Route<'_>> {
    match endpoint {
        Endpoint::Tcp { host, port } => Route::tcp(host, *port),
        Endpoint::Tls { host, port } => Route::tls(host, *port),
    }
    .map_err(|e| Error::Other(e.to_string()))
}

/// The provider's stop of `handle`'s machine, or its park when `park`, in a
/// task of its own: the request is made whatever the caller's wait comes
/// to. `true` once the provider confirmed it; a failure is logged, since the
/// machine may still be running and billed.
fn stop_in_background(
    client: Arc<Client>,
    handle: Handle,
    agent_id: String,
    park: bool,
) -> tokio::task::JoinHandle<bool> {
    crate::host::spawn(async move {
        let stopped = if park {
            client.park(&handle).await
        } else {
            client.stop(&handle).await
        };
        match stopped {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(
                    agent_id,
                    handle = %handle,
                    park,
                    error = %e,
                    "a cloud machine's stop failed; it may still be running and billed"
                );
                false
            }
        }
    })
}

/// A cloud box's machine, while the box runs: the provider's client, the
/// box's handle, and the CLIs its disk holds.
pub(super) struct CloudMachine {
    agent_id: String,
    client: Arc<Client>,
    handle: Handle,
    /// The CLI versions known to be installed on the box's disk, by
    /// provider: read from the disk the first time a launch asks, and
    /// recorded by every install.
    clis: Mutex<HashMap<String, String>>,
    /// Held across a read and an install, so two launches never install one
    /// CLI at once.
    installing: tokio::sync::Mutex<()>,
    /// The records of installed CLIs still being made on the box's disk
    /// (`ensure_cli`), each the guest's sync and then its version marker:
    /// what a clean stop lets finish first ([`Self::halt`]). A kill or an
    /// abort waits for none, and a record they cut short leaves no marker.
    records: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl CloudMachine {
    pub(super) fn new(agent_id: &str, client: Arc<Client>, handle: Handle) -> Self {
        Self {
            agent_id: agent_id.to_string(),
            client,
            handle,
            clis: Mutex::new(HashMap::new()),
            installing: tokio::sync::Mutex::new(()),
            records: Mutex::new(Vec::new()),
        }
    }

    pub(super) fn provider(&self) -> ProviderId {
        self.handle.provider()
    }

    pub(super) fn handle(&self) -> &Handle {
        &self.handle
    }

    /// Stop the machine cleanly: the CLI records still being made let finish
    /// ([`Self::finish_records`]), the guest's disks synced over `channel`,
    /// then the provider's stop, or for a box stopped for being idle
    /// ([`StopReason::Idle`]) its park, so that it wakes the soonest when it
    /// is wanted again. With a `bound`, they all get that long; the request
    /// is made whatever comes of the wait, and an unconfirmed stop or park is
    /// not a clean one.
    pub(super) async fn halt(
        &self,
        channel: &Channel,
        reason: StopReason,
        bound: Option<Duration>,
    ) -> StopOutcome {
        let started = Instant::now();
        self.finish_records(bound).await;
        let sync = bound.map_or(SYNC_BOUND, |bound| bound.min(SYNC_BOUND));
        let synced = tokio::time::timeout(sync, channel.exec(None, None, &["sync"])).await;
        if !matches!(synced, Ok(Ok(ref out)) if out.status.success()) {
            tracing::debug!(
                agent_id = self.agent_id,
                "the cloud box's disks were not synced before its stop"
            );
        }
        let park = reason == StopReason::Idle;
        self.stop(
            bound.map(|bound| bound.saturating_sub(started.elapsed())),
            park,
        )
        .await
    }

    /// Stop the machine without asking its guest, waiting a short while for
    /// the provider to confirm it.
    pub(super) async fn kill(&self) -> StopOutcome {
        self.stop(Some(KILL_BOUND), false).await
    }

    /// Have the provider stop the machine, waiting for nothing.
    pub(super) fn abort(&self) {
        drop(self.send(false));
    }

    /// Have the provider stop the machine, or park it when `park`.
    async fn stop(&self, bound: Option<Duration>, park: bool) -> StopOutcome {
        let stopping = self.send(park);
        let confirmed = match bound {
            Some(bound) => tokio::time::timeout(bound, stopping).await.ok(),
            None => Some(stopping.await),
        };
        match confirmed {
            Some(Ok(true)) => StopOutcome::Control,
            Some(_) => StopOutcome::Killed,
            None => {
                tracing::warn!(
                    agent_id = self.agent_id,
                    handle = %self.handle,
                    park,
                    "the provider did not confirm a cloud machine's stop in time; it may still be \
                     running and billed"
                );
                StopOutcome::Killed
            }
        }
    }

    fn send(&self, park: bool) -> tokio::task::JoinHandle<bool> {
        stop_in_background(
            self.client.clone(),
            self.handle.clone(),
            self.agent_id.clone(),
            park,
        )
    }

    /// The version of `provider`'s CLI known to be on the box's disk, if
    /// any is known yet.
    pub(super) fn cli_version(&self, provider: &str) -> Option<String> {
        self.clis.lock().get(provider).cloned()
    }

    /// What names `provider`'s CLI on the box's disk for what is remembered
    /// about a CLI (`super::guest_thinking_summaries`): its version, and
    /// nothing of the box. Every cloud box runs the same image and installs
    /// the same package at a version, so what one learnt of a version holds
    /// in all of them. `None` while no version is known.
    pub(super) fn cli_identity(&self, provider: &str) -> Option<String> {
        self.cli_version(provider)
            .map(|version| format!("cloud box {provider} {version}"))
    }

    /// Make `provider`'s CLI present on the box's disk at the version a
    /// launch of it runs (`registry::tools_version`), over `channel`, as the
    /// guest user whose home is `home`: read from the disk the first time it
    /// is asked, installed when it is missing or another version.
    /// `preparing` hears an install as it starts. An ordinary launch costs
    /// no round trip once the box has answered once.
    ///
    /// An install returns once the CLI is usable, its version known for this
    /// run of the box; recording it on the disk waits for the guest's sync,
    /// which the launch does not, so the record is made in the background
    /// ([`Self::record_in_background`]).
    pub(super) async fn ensure_cli(
        &self,
        channel: &Channel,
        home: &Path,
        provider: &str,
        preparing: &(dyn Fn(String) + Sync),
    ) -> Result<()> {
        let wanted = super::registry::tools_version(provider);
        if self.cli_version(provider).as_deref() == Some(wanted.as_str()) {
            return Ok(());
        }
        let _installing = self.installing.lock().await;
        if self.cli_version(provider).is_none() {
            let found = fletch_box::tools::installed_cli_version(channel, provider)
                .await
                .map_err(|e| Error::Other(e.to_string()))?;
            if let Some(found) = found {
                self.clis.lock().insert(provider.to_string(), found);
            }
        }
        if self.cli_version(provider).as_deref() == Some(wanted.as_str()) {
            return Ok(());
        }
        let label = crate::agent::provider_bin_label(provider).map_or(provider, |(_, l)| l);
        preparing(format!("installing {label} {wanted}"));
        let agent_id = self.agent_id.as_str();
        fletch_box::tools::run_cli_install(
            channel,
            provider,
            &wanted,
            home,
            guest::Egress::Direct,
            |line| tracing::info!(target: "fletch::box_tools", agent_id, provider, line, "cloud CLI install"),
        )
        .await
        .map_err(|e| {
            Error::SandboxUnavailable(format!(
                "installing {label} {wanted} in the {} machine failed: {e}",
                self.provider().label()
            ))
        })?;
        tracing::info!(
            agent_id,
            provider,
            version = wanted,
            "CLI installed in a cloud box"
        );
        self.record_in_background(channel, provider, &wanted);
        self.clis.lock().insert(provider.to_string(), wanted);
        Ok(())
    }

    /// Record `provider`'s CLI at `version` on the box's disk, in a task of
    /// its own that the machine keeps until a stop lets it finish: the
    /// guest's disks synced, then the version marker written
    /// (`fletch_box::tools::record_cli_version`). A record that fails, or is
    /// cut short by the machine going down, leaves no marker, and the next
    /// boot installs the CLI again.
    fn record_in_background(&self, channel: &Channel, provider: &str, version: &str) {
        let (channel, agent_id) = (channel.clone(), self.agent_id.clone());
        let (provider, version) = (provider.to_string(), version.to_string());
        let record = crate::host::spawn(async move {
            if let Err(e) =
                fletch_box::tools::record_cli_version(&channel, &provider, &version).await
            {
                tracing::warn!(
                    agent_id,
                    provider,
                    version,
                    error = %e,
                    "a CLI installed in a cloud box was not recorded on its disk; its next boot \
                     installs it again"
                );
            }
        });
        let mut records = self.records.lock();
        records.retain(|record| !record.is_finished());
        records.push(record);
    }

    /// Wait for the CLI records still being made ([`Self::record_in_background`])
    /// to finish, for a clean stop, so the machine keeps each version it
    /// recorded. With a `bound` they get what of it the stop's own sync does
    /// not take, and with none [`RECORD_BOUND`]; one still unfinished then
    /// races the machine going down, and writes its marker only if its sync
    /// finishes first.
    async fn finish_records(&self, bound: Option<Duration>) {
        let records = std::mem::take(&mut *self.records.lock());
        let wait = bound.map_or(RECORD_BOUND, |bound| bound.saturating_sub(SYNC_BOUND));
        let deadline = Instant::now() + wait;
        for record in records {
            let left = deadline.saturating_duration_since(Instant::now());
            let finished = tokio::time::timeout(left, record).await.is_ok();
            if !finished {
                tracing::debug!(
                    agent_id = self.agent_id,
                    "a CLI record did not finish before its cloud box's stop"
                );
            }
        }
    }
}

/// Destroy, at its provider, the box whose handle the box directory `dir`
/// holds; nothing to do for a directory with none. Idempotent. A provider
/// not connected on this machine is an error: the box is still there.
pub async fn destroy_box(dir: &Path) -> Result<()> {
    destroy_box_with(dir, client).await
}

/// [`destroy_box`] with the client for a provider from `client_for`.
pub(super) async fn destroy_box_with(
    dir: &Path,
    client_for: impl FnOnce(ProviderId) -> Result<Client>,
) -> Result<()> {
    let Some(handle) = read_handle(dir)? else {
        return Ok(());
    };
    client_for(handle.provider())?.destroy(&handle).await?;
    tracing::info!(handle = %handle, box_dir = %dir.display(), "cloud box destroyed");
    Ok(())
}

/// Stop the machine of `handle` when its provider reports it running or on
/// its way there; for a box that does not run here. Logged, never an error.
pub(super) async fn stop_if_running(client: &Client, handle: &Handle, agent_id: &str) {
    match client.state(handle).await {
        Ok(MachineState::Running | MachineState::Starting) => match client.stop(handle).await {
            Ok(()) => tracing::info!(
                agent_id,
                handle = %handle,
                "stopped a cloud machine left running by an earlier run"
            ),
            Err(e) => tracing::warn!(
                agent_id,
                handle = %handle,
                error = %e,
                "a cloud machine left running could not be stopped; it is billed while it runs"
            ),
        },
        Ok(_) => {}
        Err(e) => {
            tracing::info!(agent_id, handle = %handle, error = %e, "a cloud box's state could not be read")
        }
    }
}

/// The extra time the spawn watchdog gives a launch of `agent_id` whose box
/// is at a provider: making the box when it has no handle yet, else
/// starting it. `None` for every other agent.
pub fn setup_allowance(agent_id: &str) -> Option<Duration> {
    box_backing(agent_id)?;
    let owner = BoxRegistry::global()
        .tenant_of(agent_id)
        .map_or_else(|| agent_id.to_string(), |t| t.host_agent_id);
    let made = super::box_dir(&owner).is_ok_and(|dir| dir.join(HANDLE_FILE).is_file());
    Some(if made {
        START_ALLOWANCE
    } else {
        CREATE_ALLOWANCE
    })
}

/// Whether the box directory `dir` is a cloud box's: it holds a handle.
pub fn is_cloud_dir(dir: &Path) -> bool {
    dir.join(HANDLE_FILE).is_file()
}

/// Bring up the machine of the box in `dir` as `identity`'s, through
/// `client`: the box's ssh key made in `dir` when it holds none, then its
/// machine made from [`spec`] or brought to running ([`machine_up`]).
/// Answers the machine and the key its guest is reached with ([`reach`]).
/// What a box's boot starts with in the box's directory
/// (`registry::provision_cloud`), and a spare's preparation in a directory
/// of its own (`spares`). `phase` hears what it does.
pub(super) async fn bring_up(
    client: Arc<Client>,
    identity: &BoxIdentity,
    dir: &Path,
    phase: &(dyn Fn(BoxPhase) + Sync),
) -> Result<(CloudMachine, PathBuf)> {
    let key = channel::keygen(dir)?;
    let authorized = channel::authorized_key(&key)?;
    let spec = spec(identity, authorized);
    let handle = machine_up(&client, dir, &spec, phase).await?;
    Ok((CloudMachine::new(&identity.agent_id, client, handle), key))
}

/// Binds a box's paths in its guest where they are not bound yet, as the
/// entrypoint binds them at the machine's start (`bind_identity` in
/// `packaging/cloud-image/entrypoint`), with its refusals and its ownership
/// of the directories on the way: the home, the rpc dir and the writable
/// root, in that order, each from its directory under `/data`. Run as the
/// guest user, whose `sudo` needs no password; a path that is a mount point
/// already is left alone, so a guest started with the box's own identity
/// runs nothing privileged. Arguments: the three paths.
const BIND_IDENTITY: &str = r#"set -eu
bind() {
  mountpoint -q "$1" && return
  case "$1" in /?*/?*) ;; *) echo "$1 is not an absolute path with two components or more" >&2; exit 1;; esac
  top=${1#/}; top=/${top%%/*}
  case "$top" in
    /bin|/boot|/data|/dev|/etc|/lib|/lib64|/opt|/proc|/run|/sbin|/sys|/tmp|/usr|/var)
      echo "cannot place $1 under $top: it would shadow the guest's own files" >&2; exit 1;;
  esac
  [ -d "$top" ] || { echo "no $top directory in the guest to mount $1 under" >&2; exit 1; }
  sudo -n mkdir -p "$1"
  dir=${1%/*}
  while [ "$dir" != "$top" ]; do
    sudo -n chown -h agent:agent "$dir"
    dir=${dir%/*}
  done
  sudo -n mount --bind "$2" "$1"
}
bind "$1" /data/home
bind "$2" /data/rpc
bind "$3" /data/ws
"#;

/// The command [`bind_identity`] runs in the guest for `identity`.
fn bind_identity_argv(identity: &BoxIdentity) -> Vec<String> {
    ["sh", "-c", BIND_IDENTITY, "sh"]
        .into_iter()
        .map(String::from)
        .chain(
            [&identity.home, &identity.rpc_dir, &identity.writable_root]
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned()),
        )
        .collect()
}

/// Make sure `identity`'s paths are bound in the guest `channel` reaches,
/// binding those that are not ([`BIND_IDENTITY`]), in one ssh round trip.
/// A box's guest started with another identity's paths when it was made as
/// a spare (`spares`), and its entrypoint binds only those, at every start:
/// so a boot of a cloud box asks every time, and the guest of a box made
/// for its workspace answers that there is nothing to do. Bind mounts, so
/// each path resolves to itself; the placeholder's stay as aliases.
pub(super) async fn bind_identity(channel: &Channel, identity: &BoxIdentity) -> Result<()> {
    let argv = bind_identity_argv(identity);
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    let out = channel
        .exec(None, None, &argv)
        .await
        .map_err(|e| Error::Other(format!("bind the box's paths in its guest: {e}")))?;
    if out.status.success() {
        return Ok(());
    }
    Err(Error::SandboxUnavailable(format!(
        "the box's paths could not be bound in its guest ({}): {}",
        out.status,
        String::from_utf8_lossy(&out.stderr).trim()
    )))
}

#[cfg(test)]
#[path = "cloud_tests.rs"]
mod tests;
