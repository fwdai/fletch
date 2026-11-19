//! The runner: `fletch-host` on a machine at a cloud provider, always on,
//! supervising agents with no laptop online, each workspace in a provider
//! machine of its own, and serving every paired device through the relay
//! (`docs/cloud-runner-setup.md`). This module is the engine half of it, run
//! by the desktop: it creates, inspects, starts, stops and deletes the
//! runner's machine through the provider adapter ([`crate::cloud`]) and sets
//! it up over ssh. One runner per install. Its public functions are what the
//! desktop's commands call, each one call: [`status`], [`create`], [`start`],
//! [`stop`], [`delete`], [`pairing_link`].
//!
//! The runner's machine is made as a cloud box's is
//! (`sandbox::boxes::cloud`): the same cloud image, a disk mounted at `/data`
//! that outlives the machine, [`cloud::machine_up`] to make it or bring it to
//! running (replaced on its disk when the machine alone was lost, refused when
//! the disk is gone), its host key read through the provider's exec and
//! pinned, and the channel to its sshd ([`cloud::reach`]). Its provider keeps
//! it running ([`MachineSpec::keep_running`]); a stop asked for here stays a
//! stop.
//!
//! **Its directory** on this machine is [`dir`], beside the boxes root and
//! never inside it, so no sweep of boxes takes it. It holds what a cloud box's
//! directory holds — the ssh key pair the runner's sshd trusts, the pinned
//! host key, the handle at the provider ([`cloud::HANDLE_FILE`]) — and, once
//! the runner is set up, its record ([`RECORD_FILE`]).
//!
//! **On its disk** everything of the runner's lives, and outlives a stop and a
//! replaced machine (`setup`): `fletch-host` and its start script under
//! [`guest::CLOUD_HOST_DIR`], the Claude CLI under `/data/tools`, and the
//! host's data directory in the service's home ([`HOME`]), which the image's
//! entrypoint binds from `/data/home`. The entrypoint starts the service at
//! every start of the machine ([`guest::CLOUD_HOST_SERVICE`]): a cold start
//! needs no laptop.
//!
//! **Secrets** reach the runner over the channel alone, on a command's stdin
//! or as a file's content: the provider connection and the Claude token, never
//! in the machine's configuration, a command line, a log or an error. Its
//! GitHub token is minted on the runner, by its own device login, and never
//! leaves it.
//!
//! What is made is undone on failure: a create that fails at any step
//! destroys the machine and its disk and removes the directory, so a retry
//! starts clean. Deleting the runner first has it destroy every machine it
//! made at its providers, its spares and its workspaces' boxes
//! (`destroy_cloud_boxes`), then destroys its own; a runner that cannot be
//! reached, or that could not destroy them all, is deleted only when asked to
//! be anyway (`force`), and those machines may then remain. Nothing is
//! destroyed at a provider that this install has no directory for, and a
//! runner whose disk is gone is never made again on an empty one.
//!
//! **The runner's service is always ended cleanly before its machine goes
//! down on purpose**: [`stop`] has its start script end `fletch-host` (whose
//! own shutdown stops its workspaces' machines) and syncs the disk before the
//! provider stops the machine, and falls back to the provider's stop alone,
//! logged, only when the guest cannot be reached or the service does not end
//! in time. The stop and start inside [`create`] comes before the service
//! ever ran, and [`delete`] takes the disk with the machine. And while the
//! runner is at a provider, that provider is not disconnected on this
//! machine ([`check_disconnect`]): the connection is the only way to manage
//! the runner.

mod setup;
#[cfg(test)]
mod tests;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fletch_box::{channel, guest};
use serde::{Deserialize, Serialize};

use crate::cloud::{self, Client, Handle, MachineSpec, MachineState, ProviderId};
use crate::error::{Error, Result};
use crate::sandbox::boxes::cloud as boxes_cloud;
use crate::sandbox::boxes::{self, settings, CloudBoxesDestroyed};
use crate::DbState;

/// The runner's directory's name, beside the boxes root.
const DIR_NAME: &str = "cloud-runner";

/// The file in the runner's directory holding its [`RunnerRecord`], written
/// once the runner is set up.
pub const RECORD_FILE: &str = "runner.json";

/// Names a `fletch-host` binary on this machine to push to a new runner in
/// place of the release of this engine's version, which a development build
/// has none of. It must be built for the runner's machine (Linux, glibc), as
/// the release workflow builds it.
pub const HOST_BINARY_ENV: &str = "FLETCH_CLOUD_RUNNER_BINARY";

/// The runner's machine's name at its provider ([`MachineSpec::name`]), and
/// the box id its guest logs.
const MACHINE_NAME: &str = "runner";

/// The runner's size: a box's defaults. What a runner needs is unmeasured.
const CPUS: u32 = settings::DEFAULT_CPUS as u32;
const MEMORY_MIB: u32 = settings::DEFAULT_MEMORY_MIB;
const DISK_GIB: u32 = settings::DEFAULT_DISK_GIB;

/// The service's home on the runner: the disk's `/data/home`, which the
/// entrypoint binds here (`FLETCH_BOX_HOME`). Not a path under `/data`: the
/// runner's home and the directories in its data directory are its
/// workspaces' identity paths, which a box's guest refuses under `/data`, and
/// `/home` is in the image.
const HOME: &str = "/home/agent";

/// The runner's `fletch-host` data directory, in its home: its database, host
/// key, paired devices, logins and provider connection, and the roots of its
/// workspaces' checkouts, mailboxes and boxes.
const DATA_DIR: &str = "/home/agent/fletch-host";

/// The entrypoint's two other binds (`FLETCH_BOX_RPC_DIR`,
/// `FLETCH_BOX_WRITABLE_ROOT`), which its contract requires and the runner
/// does not use: its mailboxes and checkouts are its data directory's.
const UNUSED_RPC_DIR: &str = "/home/agent/.fletch-runner/rpc";
const UNUSED_WRITABLE_ROOT: &str = "/home/agent/.fletch-runner/ws";

/// What paired devices call the runner when [`CreateInputs::name`] is blank.
const DEFAULT_NAME: &str = "Fletch runner";

/// What [`create`] is given beside the provider. No `Debug`: the Claude token
/// is in it.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateInputs {
    /// The calling device's public key (`fletch_proto::keys`: base64url, no
    /// padding, 43 characters), authorised on the runner with full access, so
    /// the device reaches it with no pairing code.
    pub device_public_key: String,
    /// What the runner's device list calls the calling device.
    pub device_name: Option<String>,
    /// What the calling device runs on (`macos`).
    pub device_platform: Option<String>,
    /// A long-lived Claude Code token minted on this machine
    /// (`claude setup-token`). Kept on the runner's disk, owner-only, and
    /// given to its `fletch-host` as `CLAUDE_CODE_OAUTH_TOKEN`, which signs
    /// in every launch of its workspaces.
    pub claude_token: String,
    /// What paired devices call the runner; [`DEFAULT_NAME`] when absent or
    /// blank.
    pub name: Option<String>,
}

/// A step of [`create`], for the caller to show as it happens. In order; the
/// GitHub step carries what the person types and where.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum Progress {
    /// Creating the machine and its disk at the provider, and reading its
    /// host key.
    CreatingMachine,
    /// Waiting for the machine to answer over ssh.
    Connecting,
    /// Installing `fletch-host` on the disk: the release of `version`, or
    /// `None` for the binary [`HOST_BINARY_ENV`] names.
    InstallingHost { version: Option<String> },
    /// Installing an agent CLI (`cli` is its label) at `version`.
    InstallingCli { cli: String, version: String },
    /// Writing the Claude token and the start script to the disk.
    Configuring,
    /// Restarting the machine, so its image starts the service as it will at
    /// every start, and waiting for the service to answer.
    StartingService,
    /// Giving the runner this machine's provider connection.
    ConnectingProvider,
    /// Authorising the calling device on the runner.
    AuthorizingDevice,
    /// The runner's GitHub sign-in: the person enters `user_code` at
    /// `verification_url` within `expires_in` seconds. The token stays on the
    /// runner.
    GithubLogin {
        #[serde(rename = "userCode")]
        user_code: String,
        #[serde(rename = "verificationUrl")]
        verification_url: String,
        #[serde(rename = "expiresIn")]
        expires_in: u64,
    },
}

/// Where the runner is, as [`status`] reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RunnerState {
    /// There is no runner.
    None,
    /// [`create`] is setting it up now.
    SettingUp,
    /// A set-up that did not finish (the app quit during it); its machine
    /// may be at the provider. [`delete`] clears it.
    Incomplete,
    /// The machine's state, as the provider reports it.
    Starting,
    Running,
    Stopping,
    Stopped,
    /// The machine is gone and the disk is not: [`start`] makes a new machine
    /// on the disk, and it is the same runner.
    MachineLost,
    /// The disk is gone, and with it the runner's history, logins and
    /// pairings. Never made again on an empty disk: [`delete`] it and create
    /// a new runner.
    DiskLost,
    /// The provider cannot run it now, with why. The disk may be intact.
    Failed {
        reason: String,
    },
}

/// What [`status`] answers: where the runner is, the provider its machine is
/// at ([`ProviderId::as_str`]), and its record once it is set up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunnerStatus {
    #[serde(flatten)]
    pub state: RunnerState,
    pub provider: Option<&'static str>,
    pub record: Option<RunnerRecord>,
}

/// A runner that is set up, as its `fletch-host` reported itself at the end
/// of [`create`]: what a device needs to reach it through the relay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerRecord {
    /// The runner's host public key (base64url): its identity, which a device
    /// pins and finds it by at the relay.
    pub host_id: String,
    /// What paired devices call it.
    pub name: String,
    /// The relay it dials.
    pub relay: String,
}

/// What [`delete`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Deleted {
    /// The machines the runner destroyed at their provider: its workspaces'
    /// boxes by agent id, its spares by their placeholder id (`spare-…`).
    pub destroyed: Vec<String>,
    /// Set when the runner was deleted with `force` without having destroyed
    /// all the machines it made: what may remain at the provider, for the
    /// person to delete there.
    pub warning: Option<String>,
}

/// A pairing code for another device, minted by the runner
/// ([`pairing_link`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerPairing {
    /// The `fletch://pair` link, for a QR code: it names the runner's host
    /// key, its relay and the code.
    pub url: String,
    /// The code, to type.
    #[serde(alias = "token")]
    pub code: String,
    /// When the code lapses (RFC 3339).
    pub expires_at: String,
    /// The access it grants (`full`, `control`).
    pub preset: String,
}

/// The runner's machine once it runs, as the set-up over ssh is handed it.
struct Machine {
    dir: PathBuf,
    client: Arc<Client>,
    handle: Handle,
    spec: MachineSpec,
}

/// A change to the runner under way in this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    Create,
    Other,
}

/// The one change to the runner under way in this process, if any: what
/// [`status`] reports as [`RunnerState::SettingUp`], and what keeps two
/// changes from running at once.
static UNDER_WAY: parking_lot::Mutex<Option<Change>> = parking_lot::const_mutex(None);

/// Held while a change runs; lets go of [`UNDER_WAY`] when dropped.
struct UnderWay;

impl UnderWay {
    fn begin(change: Change) -> Result<Self> {
        let mut slot = UNDER_WAY.lock();
        if slot.is_some() {
            return Err(Error::Other(
                "the runner is being changed already; wait for that to finish".into(),
            ));
        }
        *slot = Some(change);
        Ok(Self)
    }
}

impl Drop for UnderWay {
    fn drop(&mut self) {
        *UNDER_WAY.lock() = None;
    }
}

/// The runner's directory: `<checkouts base>/cloud-runner`, beside the boxes
/// root and under the same per-build split, so a debug and a release build
/// each have their own runner.
pub fn dir() -> Result<PathBuf> {
    Ok(boxes::boxes_root()?.with_file_name(DIR_NAME))
}

/// Where the runner is: none, being set up, or its machine's state as its
/// provider reports it, with its record. Reads the directory and asks the
/// provider; nothing over ssh.
pub async fn status(db: &DbState) -> Result<RunnerStatus> {
    if *UNDER_WAY.lock() == Some(Change::Create) {
        return Ok(RunnerStatus {
            state: RunnerState::SettingUp,
            provider: None,
            record: None,
        });
    }
    status_in(&dir()?, |p| client_for(db, p)).await
}

/// Create the runner at `provider`, which this machine has a connection to,
/// and set it up; answers its record. Refused while a runner exists. `progress`
/// hears each step as it starts ([`Progress`]); the GitHub step waits until
/// the person has entered the code it carries, or it lapses. A failure at any
/// step destroys what was made and removes the directory.
pub async fn create(
    db: &DbState,
    provider: ProviderId,
    inputs: CreateInputs,
    progress: &(dyn Fn(Progress) + Sync),
) -> Result<RunnerRecord> {
    let _under_way = UnderWay::begin(Change::Create)?;
    check_device_key(&inputs.device_public_key)?;
    let claude_token = setup::check_token(&inputs.claude_token)?;
    let (connection, relay) = {
        let conn = db.lock();
        (cloud::connection(&conn, provider)?, relay_url(&conn))
    };
    let connection = connection
        .ok_or_else(|| Error::SandboxUnavailable(boxes_cloud::not_connected(provider)))?;
    let setup = setup::Setup {
        connection,
        relay,
        name: inputs
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(DEFAULT_NAME)
            .to_string(),
        device_public_key: inputs.device_public_key.trim().to_string(),
        device_name: inputs.device_name,
        device_platform: inputs.device_platform,
        claude_token,
    };
    create_in(
        &dir()?,
        |p| client_for(db, p),
        provider,
        progress,
        |machine| setup::configure(machine, &setup, progress),
    )
    .await
}

/// Start the runner's machine and wait until it runs, its host key pinned
/// again. A machine that was lost is made again on the runner's disk, the
/// same runner; a disk that is gone is refused ([`Error::CloudGone`]).
pub async fn start(db: &DbState) -> Result<()> {
    let _under_way = UnderWay::begin(Change::Other)?;
    start_in(&dir()?, |p| client_for(db, p)).await
}

/// Stop the runner's machine and wait until it has stopped. A running
/// runner's service is ended cleanly first, over ssh (`setup::end_service`):
/// its start script is told to stop, `fletch-host` shuts down as it does on
/// a TERM (stopping its workspaces' machines), and the disk is synced; only
/// then is the provider asked to stop the machine. A guest that cannot be
/// reached, or a service that does not end in time, does not keep the
/// machine running: it is stopped all the same, and that is logged. Its
/// provider leaves it stopped until [`start`].
pub async fn stop(db: &DbState) -> Result<()> {
    let _under_way = UnderWay::begin(Change::Other)?;
    let dir = dir()?;
    let at = dir.as_path();
    stop_in(
        at,
        |p| client_for(db, p),
        |handle| async move { setup::end_service(at, &handle).await },
    )
    .await
}

/// Refuse to disconnect `provider` while this install's runner is at it: the
/// runner, billed while it exists, can only be managed through that
/// connection. Decided by the runner's directory and handle alone, nothing
/// asked of the provider; a host with no runner of its own (a runner itself)
/// has nothing here.
pub fn check_disconnect(provider: ProviderId) -> Result<()> {
    check_disconnect_in(&dir()?, provider)
}

/// Delete the runner: first it destroys, at their provider, every machine it
/// made, its spares and its workspaces' boxes, and makes no spare after
/// (started for that if it is stopped), then its own machine and disk go,
/// history, logins and pairings with them, and the directory.
/// When the runner cannot be reached (its disk lost, a machine that will not
/// start) or could not destroy them all, it is refused unless `force`, and
/// with `force` [`Deleted::warning`] says what may remain.
pub async fn delete(db: &DbState, force: bool) -> Result<Deleted> {
    let _under_way = UnderWay::begin(Change::Other)?;
    let dir = dir()?;
    let clients = |p| client_for(db, p);
    delete_in(&dir, &clients, force, || setup::wind_down(&dir, &clients)).await
}

/// A pairing code for another device, minted by the running runner with the
/// access `preset` names (`full` when `None`).
pub async fn pairing_link(db: &DbState, preset: Option<&str>) -> Result<RunnerPairing> {
    let dir = dir()?;
    if read_record(&dir)?.is_none() {
        return Err(Error::Other(
            "there is no runner set up to pair with".into(),
        ));
    }
    let handle = boxes_cloud::read_handle(&dir)?.ok_or_else(no_runner)?;
    let client = client_for(db, handle.provider())?;
    match client.state(&handle).await? {
        MachineState::Running => setup::pairing(&dir, &handle, preset).await,
        state => Err(Error::Other(format!(
            "the runner is not running ({}); start it first",
            describe(&state_of(state))
        ))),
    }
}

/// [`status`] for the runner in `dir`, with the client for a provider from
/// `client_for`.
async fn status_in(
    dir: &Path,
    client_for: impl Fn(ProviderId) -> Result<Client>,
) -> Result<RunnerStatus> {
    let record = read_record(dir)?;
    let handle = boxes_cloud::read_handle(dir)?;
    let (state, provider) = match (&handle, &record) {
        (None, None) => (RunnerState::None, None),
        (None, Some(_)) => (RunnerState::Incomplete, None),
        (Some(handle), None) => (RunnerState::Incomplete, Some(handle.provider())),
        (Some(handle), Some(_)) => {
            let state = client_for(handle.provider())?.state(handle).await?;
            (state_of(state), Some(handle.provider()))
        }
    };
    Ok(RunnerStatus {
        state,
        provider: provider.map(ProviderId::as_str),
        record,
    })
}

/// [`create`] in `dir`, with the client for a provider from `client_for`, the
/// set-up over ssh `configure` (`setup::configure`; a test's stand-in), and
/// the cleanup of everything made when either fails.
async fn create_in<F, Fut>(
    dir: &Path,
    client_for: impl Fn(ProviderId) -> Result<Client>,
    provider: ProviderId,
    progress: &(dyn Fn(Progress) + Sync),
    configure: F,
) -> Result<RunnerRecord>
where
    F: FnOnce(Machine) -> Fut,
    Fut: Future<Output = Result<RunnerRecord>>,
{
    if exists(dir) {
        return Err(Error::Other(
            "there is a runner already; delete it before creating another".into(),
        ));
    }
    // What a create that never made a machine may have left: nothing at the
    // provider refers to it.
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    let made = async {
        create_private_dir(dir)?;
        let client = Arc::new(client_for(provider)?);
        let spec = spec_of(dir)?;
        progress(Progress::CreatingMachine);
        let handle = boxes_cloud::machine_up(&client, dir, &spec, &|_| {}).await?;
        let record = configure(Machine {
            dir: dir.to_path_buf(),
            client,
            handle,
            spec,
        })
        .await?;
        write_record(dir, &record)?;
        Ok(record)
    }
    .await;
    match made {
        Ok(record) => {
            tracing::info!(host_id = record.host_id, "cloud runner created");
            Ok(record)
        }
        Err(e) => Err(abandon(dir, &client_for, e).await),
    }
}

/// Destroy what a failed [`create_in`] made in `dir` and remove the directory,
/// returning the error to report. When the machine cannot be destroyed the
/// directory stays, since its handle and key are the only way back to it, and
/// the error says so.
async fn abandon(
    dir: &Path,
    client_for: &impl Fn(ProviderId) -> Result<Client>,
    error: Error,
) -> Error {
    match boxes_cloud::destroy_box_with(dir, client_for).await {
        Ok(()) => {
            if let Err(e) = std::fs::remove_dir_all(dir) {
                tracing::warn!(dir = %dir.display(), error = %e, "a failed runner's directory could not be removed");
            }
            error
        }
        Err(cleanup) => {
            tracing::warn!(dir = %dir.display(), error = %cleanup, "a failed runner's machine could not be destroyed");
            Error::Other(format!(
                "{error} — and the runner's machine could not be deleted ({cleanup}); delete the \
                 runner to try again"
            ))
        }
    }
}

/// [`start`] for the runner in `dir`.
async fn start_in(dir: &Path, client_for: impl Fn(ProviderId) -> Result<Client>) -> Result<()> {
    let handle = boxes_cloud::read_handle(dir)?.ok_or_else(no_runner)?;
    let client = Arc::new(client_for(handle.provider())?);
    bring_up(dir, &client, &handle).await?;
    Ok(())
}

/// [`stop`] for the runner in `dir`: a running machine's service is ended
/// first by `end_service` (`setup::end_service`; a test's stand-in), and the
/// provider's stop follows whatever came of it, a failure logged. A machine
/// that is not running has nothing to end: the provider's stop is all.
async fn stop_in<E, Fut>(
    dir: &Path,
    client_for: impl Fn(ProviderId) -> Result<Client>,
    end_service: E,
) -> Result<()>
where
    E: FnOnce(Handle) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let handle = boxes_cloud::read_handle(dir)?.ok_or_else(no_runner)?;
    let client = client_for(handle.provider())?;
    if client.state(&handle).await? == MachineState::Running {
        if let Err(e) = end_service(handle.clone()).await {
            tracing::warn!(
                handle = %handle,
                error = %e,
                "the cloud runner's service was not ended cleanly before its machine was \
                 stopped; its databases rely on their own journals"
            );
        }
    }
    client.stop(&handle).await
}

/// [`check_disconnect`] for the runner in `dir`.
fn check_disconnect_in(dir: &Path, provider: ProviderId) -> Result<()> {
    match boxes_cloud::read_handle(dir)? {
        Some(handle) if handle.provider() == provider => Err(Error::Other(format!(
            "{label} can't be disconnected while this machine's cloud runner is at {label}: \
             the runner is billed while it exists and can only be managed through this \
             connection. Delete the runner first.",
            label = provider.label()
        ))),
        _ => Ok(()),
    }
}

/// [`delete`] for the runner in `dir`: `wind_down` has it destroy the
/// machines it made, its spares and its workspaces' boxes
/// (`setup::wind_down`; a test's stand-in), then its own machine and disk are
/// destroyed and the directory removed.
async fn delete_in<W, Fut>(
    dir: &Path,
    client_for: &impl Fn(ProviderId) -> Result<Client>,
    force: bool,
    wind_down: W,
) -> Result<Deleted>
where
    W: FnOnce() -> Fut,
    Fut: Future<Output = Result<CloudBoxesDestroyed>>,
{
    let Some(handle) = boxes_cloud::read_handle(dir)? else {
        // Nothing at a provider: what a create left before its machine.
        if dir.exists() {
            std::fs::remove_dir_all(dir)?;
        }
        return Ok(Deleted::default());
    };
    let label = handle.provider().label();
    let (destroyed, warning) = match wind_down().await {
        Ok(outcome) if outcome.failed.is_empty() => (outcome.destroyed, None),
        Ok(outcome) => {
            let left: Vec<String> = outcome
                .failed
                .iter()
                .map(|f| format!("{} ({})", f.agent_id, f.error))
                .collect();
            if !force {
                return Err(Error::Other(format!(
                    "the runner could not destroy these of its machines at {label} (its \
                     workspaces' by workspace, its spares as spare-…): {}. Try again, or delete \
                     the runner anyway and delete them in {label}'s dashboard",
                    left.join("; ")
                )));
            }
            let names: Vec<&str> = outcome.failed.iter().map(|f| f.agent_id.as_str()).collect();
            (
                outcome.destroyed,
                Some(format!(
                    "These of the runner's machines may remain at {label}: {}. Delete them in \
                     its dashboard.",
                    names.join(", ")
                )),
            )
        }
        Err(e) => {
            if !force {
                return Err(Error::Other(format!(
                    "Fletch can't reach the runner to have it destroy the machines it made (its \
                     workspaces' and its spares): {e}. Delete it anyway to destroy its own \
                     machine and disk; those machines then remain at {label}"
                )));
            }
            (
                Vec::new(),
                Some(format!(
                    "The runner could not be reached ({e}), so the machines it made (its \
                     workspaces' and its spares) may remain at {label}. Delete them in its \
                     dashboard."
                )),
            )
        }
    };
    boxes_cloud::destroy_box_with(dir, client_for).await?;
    std::fs::remove_dir_all(dir)?;
    tracing::info!(handle = %handle, forced = warning.is_some(), "cloud runner deleted");
    Ok(Deleted { destroyed, warning })
}

/// The runner's machine brought to running ([`cloud::machine_up`]), its
/// handle answered; a disk that is gone is refused in the runner's words.
async fn bring_up(dir: &Path, client: &Arc<Client>, handle: &Handle) -> Result<Handle> {
    if matches!(
        client.state(handle).await?,
        MachineState::DiskGone | MachineState::Gone
    ) {
        return Err(Error::CloudGone(disk_lost(handle.provider())));
    }
    boxes_cloud::machine_up(client, dir, &spec_of(dir)?, &|_| {}).await
}

/// Why a runner whose disk is gone is not started.
fn disk_lost(provider: ProviderId) -> String {
    format!(
        "The runner's {} disk is gone, and its history, logins and pairings with it. Fletch \
         won't start it on an empty one: delete the runner and create a new one.",
        provider.label()
    )
}

/// The runner's state from its machine's.
fn state_of(state: MachineState) -> RunnerState {
    match state {
        MachineState::Starting => RunnerState::Starting,
        MachineState::Running => RunnerState::Running,
        MachineState::Stopping => RunnerState::Stopping,
        MachineState::Stopped => RunnerState::Stopped,
        MachineState::MachineGone => RunnerState::MachineLost,
        MachineState::DiskGone | MachineState::Gone => RunnerState::DiskLost,
        MachineState::Failed(reason) => RunnerState::Failed { reason },
    }
}

/// A runner state in a few words, for a refusal.
fn describe(state: &RunnerState) -> String {
    match state {
        RunnerState::None => "there is none".into(),
        RunnerState::SettingUp => "being set up".into(),
        RunnerState::Incomplete => "its set-up did not finish".into(),
        RunnerState::Starting => "starting".into(),
        RunnerState::Running => "running".into(),
        RunnerState::Stopping => "stopping".into(),
        RunnerState::Stopped => "stopped".into(),
        RunnerState::MachineLost => "its machine was lost".into(),
        RunnerState::DiskLost => "its disk is gone".into(),
        RunnerState::Failed { reason } => format!("failed: {reason}"),
    }
}

/// The runner's machine: the cloud image, with the entrypoint's contract
/// satisfied by paths that put the service's home, and with it its data
/// directory, on the disk ([`HOME`]); the ssh key of the runner's directory;
/// a box's size; kept running by its provider. Nothing secret.
fn spec(authorized_key: String) -> MachineSpec {
    let env = [
        (guest::ENV_HOME, HOME.to_string()),
        (guest::ENV_RPC_DIR, UNUSED_RPC_DIR.to_string()),
        (guest::ENV_WRITABLE_ROOT, UNUSED_WRITABLE_ROOT.to_string()),
        (guest::ENV_AUTHORIZED_KEY, authorized_key),
        (guest::ENV_BOX_ID, MACHINE_NAME.to_string()),
        (guest::ENV_SSH_PORT, guest::CLOUD_SSH_PORT.to_string()),
    ];
    MachineSpec {
        name: MACHINE_NAME.to_string(),
        image: boxes_cloud::image(),
        env: env.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        cpus: CPUS,
        memory_mib: MEMORY_MIB,
        disk_gib: Some(DISK_GIB),
        ssh_port: guest::CLOUD_SSH_PORT,
        keep_running: true,
    }
}

/// [`spec`] with the ssh key of the runner's directory `dir`, made when it is
/// not there yet.
fn spec_of(dir: &Path) -> Result<MachineSpec> {
    let key = channel::keygen(dir).map_err(|e| Error::Other(e.to_string()))?;
    let authorized = channel::authorized_key(&key).map_err(|e| Error::Other(e.to_string()))?;
    Ok(spec(authorized))
}

/// Whether there is a runner in `dir`: a machine made (its handle), or a
/// record.
fn exists(dir: &Path) -> bool {
    boxes_cloud::is_cloud_dir(dir) || dir.join(RECORD_FILE).is_file()
}

/// The runner's directory, made owner-only: it holds the key its sshd trusts.
fn create_private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

/// The runner's record in `dir`, if it is set up.
fn read_record(dir: &Path) -> Result<Option<RunnerRecord>> {
    let path = dir.join(RECORD_FILE);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| Error::Other(format!("read {}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::Other(format!("read {}: {e}", path.display()))),
    }
}

/// Keep `record` in `dir`.
fn write_record(dir: &Path, record: &RunnerRecord) -> Result<()> {
    let text = serde_json::to_string_pretty(record).map_err(|e| Error::Other(e.to_string()))?;
    std::fs::write(dir.join(RECORD_FILE), text + "\n")?;
    Ok(())
}

/// A client for `provider`, from the connection this machine keeps in `db`.
fn client_for(db: &DbState, provider: ProviderId) -> Result<Client> {
    let connection = cloud::connection(&db.lock(), provider)?
        .ok_or_else(|| Error::SandboxUnavailable(boxes_cloud::not_connected(provider)))?;
    Client::new(&connection)
}

/// The relay the runner dials: this machine's own relay setting, or Fletch's
/// when none is stored.
fn relay_url(conn: &rusqlite::Connection) -> String {
    crate::database::get_setting(conn, crate::remote::RELAY_URL_SETTING)
        .map(|url| url.trim().to_string())
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| crate::remote::DEFAULT_RELAY_URL.to_string())
}

/// Refuse a device key the runner would refuse, before a machine is made for
/// it: 32 bytes as base64url without padding.
fn check_device_key(key: &str) -> Result<()> {
    use base64::Engine as _;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(key.trim());
    if !matches!(decoded, Ok(bytes) if bytes.len() == 32) {
        return Err(Error::Other(format!(
            "{key:?} is not a device's public key: expected 43 characters of base64url, without \
             padding"
        )));
    }
    Ok(())
}

fn no_runner() -> Error {
    Error::Other("there is no runner".into())
}
