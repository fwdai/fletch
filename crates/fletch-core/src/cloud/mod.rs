//! Cloud providers: machines created at a provider rather than on this Mac,
//! running the same guest a local box runs and reached the same way — over
//! ssh, here across the internet.
//!
//! This module is the provider client and nothing more: create a box's machine
//! and its disk, start, stop and park the machine, read the box's state, run
//! one command in it, replace a machine that was lost, destroy the box, and
//! list what this integration created so an orphan can be found. Which box a
//! machine backs and when it starts, stops or parks is the box registry's
//! business; it keeps each box's [`Handle`] and passes it back for every call.
//!
//! A parked machine rests in whichever state its provider wakes it from
//! soonest, with everything on it kept ([`Client::park`]). Which state that
//! is, and the fallback to a plain stop where the provider has nothing faster
//! for the machine, is the adapter's answer: the caller never learns which it
//! got, [`Client::start`] wakes the machine from either, and a guest works the
//! same whether it resumed or started afresh, since a provider may cold-start
//! a parked machine.
//!
//! A cloud box is two things with two lifetimes: a disk, the provider's
//! persistent storage, holding everything that is the box's — its workspace,
//! its home, its ssh host key — and a machine that mounts it and can be lost
//! and replaced while the disk survives. The disk is as durable as the provider
//! makes it, which is not absolute: a Fly volume lives on one host's drive,
//! unreplicated, and a failed host takes it until the host returns or a daily
//! snapshot is restored (`fly`). What the machine's own root filesystem holds
//! (system changes) is best-effort and goes with the machine. [`MachineState`]
//! keeps the two apart: a machine gone is recoverable
//! ([`Client::replace_machine`]); a disk gone is the workspace lost, and is
//! never quietly replaced by an empty one.
//!
//! Providers are enums matched exhaustively ([`ProviderId`], [`Client`],
//! [`Handle`]) — the house style for engines and executors — so a second
//! provider makes the compiler name every place that needs its answer. Fly.io
//! Machines is the first ([`fly`]).
//!
//! Everything provider-specific is its adapter's: the module ([`fly`]) holds
//! the API, the handle's form, the stored keys and the words Settings shows
//! (its `FACTS`), and this module only dispatches to it. Nothing outside
//! `cloud/` names a provider: the box engine, the supervisor and the commands
//! see a [`ProviderId`] as an opaque id with a [`label`](ProviderId::label),
//! and a box through [`Client`], [`Handle`], [`Endpoint`], [`MachineState`]
//! and [`MachineSpec`], whatever the provider (`tests/neutral.rs` holds the
//! rest of the crate to that). A provider is added as a module here and its
//! variant in those enums.
//!
//! The connection to a provider account is stored per provider: its API token
//! in [`crate::secrets`] (`cloud.<provider>.token`; the keychain on release
//! macOS), the account it acts in in the `settings` table
//! (`cloud.<provider>.org`), and an optional region for new machines beside it
//! (`cloud.<provider>.region`; absent lets the provider choose). [`connection`]
//! reads it, [`connect`] validates a pasted token against the provider before
//! storing anything, [`disconnect`] forgets it. The engine on this machine
//! holds the token and drives the provider; nothing secret is ever sent to a
//! machine, and the commands over these are this machine's alone, never on
//! the remote wire.
//!
//! The token must never appear in logs, telemetry or error strings: every
//! message an adapter builds passes through its redaction, and
//! [`Connection`]'s `Debug` leaves the token out.

#[cfg(test)]
pub(crate) mod fake;
pub mod fly;
#[cfg(test)]
pub(crate) mod stand_in;

use std::fmt;
use std::time::Duration;

use crate::error::{Error, Result};

/// A cloud provider machines can be created at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderId {
    /// Fly.io Machines, one app per machine ([`fly`]).
    Fly,
}

/// What an adapter says about its provider: its names, where its connection
/// is stored, and what Settings shows to connect it. One per provider, in its
/// module (`fly::FACTS`).
struct Facts {
    /// [`ProviderId::as_str`].
    id: &'static str,
    /// [`ProviderId::label`].
    label: &'static str,
    /// The `crate::secrets` key holding the API token.
    token_key: &'static str,
    /// The `settings` key holding the account the token acts in.
    account_key: &'static str,
    /// The `settings` key holding the region new machines are created in.
    region_key: &'static str,
    /// The account a connection acts in when none is given.
    default_account: &'static str,
    /// [`CloudProviderStatus::account_label`].
    account_label: &'static str,
    /// [`CloudProviderStatus::token_placeholder`].
    token_placeholder: &'static str,
    /// [`CloudProviderStatus::token_help`].
    token_help: &'static str,
    /// [`CloudProviderStatus::billing`].
    billing: &'static str,
}

impl ProviderId {
    /// Every provider, in the order Settings lists them.
    pub const ALL: &'static [Self] = &[Self::Fly];

    fn facts(self) -> &'static Facts {
        match self {
            Self::Fly => &fly::FACTS,
        }
    }

    /// The provider's spelling on the command surface (the `provider` argument
    /// of the `cloud_*` commands, [`CloudProviderStatus::id`]), in its stored
    /// keys, and in a workspace's record.
    pub fn as_str(self) -> &'static str {
        self.facts().id
    }

    /// The name a person knows the provider by.
    pub fn label(self) -> &'static str {
        self.facts().label
    }

    /// The provider an [`as_str`](Self::as_str) spelling names; `None` for any
    /// other string, so callers choose their own error.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|p| p.as_str() == s)
    }

    /// The `crate::secrets` key holding the provider's API token.
    fn token_key(self) -> &'static str {
        self.facts().token_key
    }

    /// The `settings` key holding the account the token acts in.
    fn account_key(self) -> &'static str {
        self.facts().account_key
    }

    /// The `settings` key holding the region new machines are created in.
    fn region_key(self) -> &'static str {
        self.facts().region_key
    }

    /// The account a connection acts in when none is given.
    fn default_account(self) -> &'static str {
        self.facts().default_account
    }
}

/// A connection to one provider account: what a [`Client`] is built from.
/// `Debug` leaves the token out.
#[derive(Clone)]
pub struct Connection {
    provider: ProviderId,
    token: String,
    account: String,
    region: Option<String>,
}

impl Connection {
    /// A connection with `token` (as pasted: surrounding whitespace is dropped)
    /// acting in `account`, the provider's default account when it is absent
    /// or blank. A blank token is an error.
    pub fn new(provider: ProviderId, token: &str, account: Option<&str>) -> Result<Self> {
        let token = token.trim();
        if token.is_empty() {
            return Err(Error::Other(format!(
                "paste a {} API token to connect",
                provider.label()
            )));
        }
        let account = account
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .unwrap_or(provider.default_account());
        Ok(Self {
            provider,
            token: token.to_string(),
            account: account.to_string(),
            region: None,
        })
    }

    /// The same connection creating machines in `region`; `None` or blank lets
    /// the provider choose.
    pub fn with_region(mut self, region: Option<&str>) -> Self {
        self.region = region
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(str::to_string);
        self
    }

    pub fn provider(&self) -> ProviderId {
        self.provider
    }

    /// The account the token acts in (Fly's org slug).
    pub fn account(&self) -> &str {
        &self.account
    }

    pub fn region(&self) -> Option<&str> {
        self.region.as_deref()
    }

    fn token(&self) -> &str {
        &self.token
    }
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection")
            .field("provider", &self.provider)
            .field("account", &self.account)
            .field("region", &self.region)
            .finish_non_exhaustive()
    }
}

/// The size of a cloud box's disk when [`MachineSpec::disk_gib`] gives none:
/// the size a local box's data disk is created with. A local disk is sparse
/// and costs only what is written; a provider bills a cloud disk for its whole
/// size for as long as it exists, the machine running or not (Fly.io's rate
/// is in `fly`).
pub const DEFAULT_DISK_GIB: u32 = (fletch_box::disks::DEFAULT_DATA_DISK_BYTES >> 30) as u32;

/// A box to create. Nothing in it is secret: the environment carries paths and
/// one public ssh key, never a credential.
#[derive(Debug, Clone)]
pub struct MachineSpec {
    /// The caller's name for the box (its agent id). A provider folds it into
    /// its own naming rules and makes it unique; it is there so a person can
    /// tell the box apart in the provider's dashboard.
    pub name: String,
    /// The image the machine runs (`ghcr.io/fwdai/fletch-cloud:<tag>`).
    pub image: String,
    pub env: Vec<(String, String)>,
    pub cpus: u32,
    pub memory_mib: u32,
    /// The size of the box's disk — the durable storage its workspace, home
    /// and ssh host key live on, mounted at the guest's `/data` — in GiB;
    /// `None` is [`DEFAULT_DISK_GIB`]. Billed for its whole size while the box
    /// exists.
    pub disk_gib: Option<u32>,
    /// The port the guest's sshd listens on.
    pub ssh_port: u16,
}

/// A created box, as the box registry keeps it: the `Display` form is the
/// opaque string the caller persists, and [`Handle::parse`] reads it back. A
/// replaced machine gives a new handle, which replaces the old one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Handle {
    /// A Fly.io box: the org it was created in, the app that holds it alone,
    /// its volume and its machine. `machine` or `volume` is empty only where
    /// [`Client::list`] found none.
    Fly {
        org: String,
        app: String,
        volume: String,
        machine: String,
    },
}

impl Handle {
    pub fn provider(&self) -> ProviderId {
        match self {
            Self::Fly { .. } => ProviderId::Fly,
        }
    }

    /// Read a handle back from its `Display` form: `<provider>:<the rest>`,
    /// the rest in the provider's own form, which its adapter reads.
    pub fn parse(s: &str) -> Result<Self> {
        let invalid = || Error::Other(format!("not a cloud box handle: {s:?}"));
        let (provider, rest) = s.split_once(':').ok_or_else(invalid)?;
        match ProviderId::parse(provider).ok_or_else(invalid)? {
            ProviderId::Fly => fly::parse_handle(s, rest),
        }
    }

    /// Whether `other` names the same box — the same disk — whatever its
    /// machine. What an orphan sweep compares: a box whose machine was lost
    /// keeps its old handle until it is replaced, and [`Client::list`] reports
    /// its disk with no machine.
    pub fn same_box(&self, other: &Handle) -> bool {
        match (self, other) {
            (
                Self::Fly { app, volume, .. },
                Self::Fly {
                    app: other_app,
                    volume: other_volume,
                    ..
                },
            ) => app == other_app && volume == other_volume,
        }
    }

    /// Where the machine's sshd is reached. Fixed for the box's life — a
    /// replaced machine keeps it — so it needs no call to the provider.
    pub fn endpoint(&self) -> Endpoint {
        match self {
            Self::Fly { app, .. } => fly::endpoint(app),
        }
    }
}

impl fmt::Display for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fly {
                org,
                app,
                volume,
                machine,
            } => write!(f, "fly:{org}/{app}/{volume}/{machine}"),
        }
    }
}

/// How this machine reaches a cloud machine's sshd.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// ssh straight over a TCP connection to `host:port`.
    Tcp { host: String, port: u16 },
    /// ssh inside a TLS connection to `host:port`: the provider terminates TLS
    /// with a certificate for `host` and forwards the plaintext to sshd, so ssh
    /// needs a TLS client in front of it (a `ProxyCommand`).
    Tls { host: String, port: u16 },
}

/// A box's state, as much of it as the box registry acts on: its machine's,
/// while its disk is there, and otherwise what is left of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MachineState {
    /// On its way to running, or busy with something that settles by itself
    /// (an update, a move to another host). Ask again.
    Starting,
    /// Running: its sshd is reachable at the endpoint.
    Running,
    /// On its way to stopped, or to parked.
    Stopping,
    /// Not running: stopped, or parked ([`Client::park`]); [`Client::start`]
    /// starts it either way.
    Stopped,
    /// The machine is gone and the disk is not: deleted from the provider's
    /// dashboard, or by the provider. Nothing is lost —
    /// [`Client::replace_machine`] makes a new machine on the same disk.
    MachineGone,
    /// The disk is gone: everything on it — the workspace, its uncommitted
    /// work — is lost. Never recovered by making a new, empty disk; what a
    /// person does about it is theirs to decide.
    DiskGone,
    /// Nothing of the box is left: deleted, or never there. Only ever reported
    /// to a connection to the account the box was created in.
    Gone,
    /// The provider cannot run it now — it failed, its host is unreachable, or
    /// it is in a state this module does not know — with the reason. The disk
    /// may well be intact: failing is no reason to destroy the box.
    Failed(String),
}

/// What [`Client::create`] made, or [`Client::replace_machine`] made anew.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    pub handle: Handle,
    pub endpoint: Endpoint,
}

/// One box [`Client::list`] found: its handle (with an empty machine where it
/// has none) and what is left of it — a machine's state, or
/// [`MachineState::MachineGone`] for a disk without a machine (a workspace;
/// recoverable if a box refers to it, compared with [`Handle::same_box`]),
/// [`MachineState::DiskGone`] for a machine without a disk, and
/// [`MachineState::Gone`] for an app holding neither (what a failed create
/// could not clean up).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub handle: Handle,
    pub state: MachineState,
}

/// What a command run by [`Client::exec`] did. A command killed by a signal
/// exits `128 + signal`, the shell's reading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// A client for one provider account, built from its [`Connection`]. Every
/// call is bounded in time; every failure is the crate's [`Error`], worded for
/// the person who reads it and never holding the token. A machine or disk that
/// no longer exists is [`Error::CloudGone`] wherever it is not simply an
/// answer ([`state`](Self::state), [`destroy`](Self::destroy)); `state` says
/// which part of the box went.
pub enum Client {
    Fly(fly::Client),
}

impl Client {
    pub fn new(connection: &Connection) -> Result<Self> {
        match connection.provider {
            ProviderId::Fly => Ok(Self::Fly(fly::Client::new(connection, fly::API_BASE)?)),
        }
    }

    /// The provider this client talks to.
    pub fn provider(&self) -> ProviderId {
        match self {
            Self::Fly(_) => ProviderId::Fly,
        }
    }

    /// Whether the token is accepted and can act in the connection's account:
    /// one cheap authenticated read, so a wrong token and a wrong account fail
    /// with different messages where the provider tells them apart.
    pub async fn validate(&self) -> Result<()> {
        match self {
            Self::Fly(c) => c.validate().await,
        }
    }

    /// Create a box — its disk, then a machine on it — and return once the
    /// machine is running. The disk is empty; the guest lays it out at its
    /// first start.
    ///
    /// Not idempotent, and never retried here. A failure cleans up after
    /// itself: everything created on the way, the disk included, is deleted
    /// before the error returns, so the caller has nothing to undo and simply
    /// tries again (a new box never collides with what a failed attempt left).
    /// If the cleanup fails too, the error says so and names what was left; it
    /// shows in [`list`](Self::list), where an orphan sweep finds it, and until
    /// it is removed its disk is billed and a machine in it may be running.
    pub async fn create(&self, spec: &MachineSpec) -> Result<Created> {
        match self {
            Self::Fly(c) => c.create(spec).await,
        }
    }

    /// The box's state now, from its disk and its machine: the machine's
    /// state, or what is left of the box ([`MachineState::MachineGone`],
    /// [`MachineState::DiskGone`], [`MachineState::Gone`]) — an answer, not an
    /// error. A box this connection cannot see at all while it is a connection
    /// to another account than the one the box was created in is an error:
    /// the box may be there, out of this token's sight.
    pub async fn state(&self, handle: &Handle) -> Result<MachineState> {
        match (self, handle) {
            (
                Self::Fly(c),
                Handle::Fly {
                    org,
                    app,
                    volume,
                    machine,
                },
            ) => c.state(org, app, volume, machine).await,
        }
    }

    /// Start the machine and return once it is running; a running machine is
    /// left as it is.
    pub async fn start(&self, handle: &Handle) -> Result<()> {
        match (self, handle) {
            (Self::Fly(c), Handle::Fly { app, machine, .. }) => c.start(app, machine).await,
        }
    }

    /// Stop the machine and return once it has stopped; a machine that is not
    /// running is left as it is. The disk, and everything on it, is kept; the
    /// machine's own root filesystem (system changes) is kept on a
    /// best-effort basis.
    pub async fn stop(&self, handle: &Handle) -> Result<()> {
        match (self, handle) {
            (Self::Fly(c), Handle::Fly { app, machine, .. }) => c.stop(app, machine).await,
        }
    }

    /// Put the machine at rest in the state its provider wakes it from
    /// soonest, and return once it is at rest; a machine already at rest is
    /// left as it is, and one on its way to running or to rest is brought to
    /// rest as [`stop`](Self::stop) brings it. The disk and everything on it
    /// is kept, and the machine's own root filesystem at least as surely as
    /// `stop` keeps it. Which state that is is the adapter's to choose, a
    /// stop where the provider cannot rest the machine any faster way, and
    /// the caller is never told: [`start`](Self::start) brings the machine
    /// back from whichever it got, by a resume or by a fresh start, as the
    /// provider manages.
    pub async fn park(&self, handle: &Handle) -> Result<()> {
        match (self, handle) {
            (Self::Fly(c), Handle::Fly { app, machine, .. }) => c.park(app, machine).await,
        }
    }

    /// A new machine for a box whose machine is gone
    /// ([`MachineState::MachineGone`]), on the box's existing disk, from the
    /// same `spec` `create` was given; returns the new handle, which replaces
    /// the old. Refused while the old machine still exists, and refused with
    /// [`Error::CloudGone`] when the disk is gone: a box is never given a new,
    /// empty disk in place of its lost one. A failure deletes the new machine
    /// and nothing else.
    pub async fn replace_machine(&self, handle: &Handle, spec: &MachineSpec) -> Result<Created> {
        match (self, handle) {
            (
                Self::Fly(c),
                Handle::Fly {
                    org,
                    app,
                    volume,
                    machine,
                },
            ) => c.replace_machine(org, app, volume, machine, spec).await,
        }
    }

    /// Delete the box: its machine, its disk and everything created with them,
    /// running or not. Idempotent: a box that is already gone is success.
    pub async fn destroy(&self, handle: &Handle) -> Result<()> {
        match (self, handle) {
            (Self::Fly(c), Handle::Fly { app, .. }) => c.destroy(app).await,
        }
    }

    /// Run `argv` in the running machine (directly, no shell) and return what
    /// it did, waiting at most `timeout` for it to finish. A command that ran
    /// and failed is an `Ok` with its exit code.
    pub async fn exec(
        &self,
        handle: &Handle,
        argv: &[String],
        timeout: Duration,
    ) -> Result<ExecOutput> {
        match (self, handle) {
            (Self::Fly(c), Handle::Fly { app, machine, .. }) => {
                c.exec(app, machine, argv, timeout).await
            }
        }
    }

    /// Every box this integration created in the connection's account,
    /// whichever box directory refers to it — the input to an orphan sweep.
    /// Includes what a failed [`create`](Self::create) could not clean up.
    /// A sweep matches entries to the boxes it knows with
    /// [`Handle::same_box`], never by the whole handle: a box whose machine
    /// was lost lists with no machine, and destroying it would destroy its
    /// workspace.
    pub async fn list(&self) -> Result<Vec<Listed>> {
        match self {
            Self::Fly(c) => c.list().await,
        }
    }
}

/// One provider's connection state, as Settings shows it, with what Settings
/// needs to connect it: every word is the adapter's, so the pane names no
/// provider.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CloudProviderStatus {
    /// [`ProviderId::as_str`].
    pub id: &'static str,
    /// [`ProviderId::label`].
    pub label: &'static str,
    pub connected: bool,
    /// The account the connection acts in (Fly's org slug); `None` when not
    /// connected.
    pub account: Option<String>,
    /// What the provider calls the account a token acts in ("Organization").
    pub account_label: &'static str,
    /// The account a connection acts in when the field is left blank.
    pub account_placeholder: &'static str,
    /// What a token looks like, for the empty field.
    pub token_placeholder: &'static str,
    /// One sentence on where to get a token.
    pub token_help: &'static str,
    /// One sentence on what the provider bills for a workspace's box.
    pub billing: &'static str,
}

impl CloudProviderStatus {
    fn of(provider: ProviderId, connection: Option<&Connection>) -> Self {
        let facts = provider.facts();
        Self {
            id: facts.id,
            label: facts.label,
            connected: connection.is_some(),
            account: connection.map(|c| c.account.clone()),
            account_label: facts.account_label,
            account_placeholder: facts.default_account,
            token_placeholder: facts.token_placeholder,
            token_help: facts.token_help,
            billing: facts.billing,
        }
    }
}

/// The stored connection to `provider`, `None` when there is no token. An
/// unavailable secret store (a locked keychain) is an error, never "not
/// connected": the connection is still there, only unreadable for now.
pub fn connection(conn: &rusqlite::Connection, provider: ProviderId) -> Result<Option<Connection>> {
    let Some(token) = crate::secrets::get(conn, provider.token_key())? else {
        return Ok(None);
    };
    let account = crate::database::get_setting(conn, provider.account_key());
    let region = crate::database::get_setting(conn, provider.region_key());
    Ok(Some(
        Connection::new(provider, &token, account.as_deref())?.with_region(region.as_deref()),
    ))
}

/// Every provider's connection state, connected or not. Reads what is stored
/// and nothing else — never the network — so Settings renders at once.
pub fn statuses(conn: &rusqlite::Connection) -> Result<Vec<CloudProviderStatus>> {
    ProviderId::ALL
        .iter()
        .map(|&p| Ok(CloudProviderStatus::of(p, connection(conn, p)?.as_ref())))
        .collect()
}

/// Connect `provider` with a pasted `token`, acting in `account` (absent or
/// blank: the provider's default). The token is validated against the provider
/// *before* anything is stored: a bad paste fails loudly and leaves whatever
/// was stored before as it was.
pub async fn connect(
    db: &crate::DbState,
    provider: ProviderId,
    token: &str,
    account: Option<&str>,
) -> Result<CloudProviderStatus> {
    let connection = Connection::new(provider, token, account)?;
    let client = Client::new(&connection)?;
    connect_with(db, connection, &client).await
}

/// [`connect`] through a given client — a stand-in server's, in tests.
pub(crate) async fn connect_with(
    db: &crate::DbState,
    connection: Connection,
    client: &Client,
) -> Result<CloudProviderStatus> {
    client.validate().await?;
    let conn = db.lock();
    crate::secrets::set(&conn, connection.provider.token_key(), &connection.token)?;
    crate::database::set_setting(
        &conn,
        connection.provider.account_key(),
        &connection.account,
    )?;
    Ok(CloudProviderStatus::of(
        connection.provider,
        Some(&connection),
    ))
}

/// Forget `provider`'s token and account. Machines created under the
/// connection are not touched — they stay at the provider, running or
/// stopped, and this machine can no longer act on them until it is connected
/// again; what becomes of a box whose provider was disconnected is the box
/// registry's decision. The region preference stays.
pub fn disconnect(conn: &rusqlite::Connection, provider: ProviderId) -> Result<()> {
    // The token first: if its delete fails the account stays too, and the
    // provider still reads as connected — which it is.
    crate::secrets::delete(conn, provider.token_key())?;
    crate::database::set_setting(conn, provider.account_key(), "")
}

#[cfg(test)]
#[path = "tests/connection.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/neutral.rs"]
mod neutral_tests;
