//! Cloud providers: machines created at a provider rather than on this Mac,
//! running the same guest a local box runs and reached the same way — over
//! ssh, here across the internet.
//!
//! This module is the provider client and nothing more: create a machine,
//! start and stop it, read its state, run one command in it, destroy it, and
//! list what this integration created so an orphan can be found. Which box a
//! machine backs and when it starts or stops is the box registry's business;
//! it keeps each machine's [`Handle`] and passes it back for every call.
//!
//! Providers are enums matched exhaustively ([`ProviderId`], [`Client`],
//! [`Handle`]) — the house style for engines and executors — so a second
//! provider makes the compiler name every place that needs its answer. Fly.io
//! Machines is the first ([`fly`]).
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

impl ProviderId {
    /// Every provider, in the order Settings lists them.
    pub const ALL: &'static [Self] = &[Self::Fly];

    /// The provider's spelling on the command surface (the `provider` argument
    /// of the `cloud_*` commands, [`CloudProviderStatus::id`]) and in its
    /// stored keys.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fly => "fly",
        }
    }

    /// The name a person knows the provider by.
    pub fn label(self) -> &'static str {
        match self {
            Self::Fly => "Fly.io",
        }
    }

    /// The provider an [`as_str`](Self::as_str) spelling names; `None` for any
    /// other string, so callers choose their own error.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|p| p.as_str() == s)
    }

    /// The `crate::secrets` key holding the provider's API token.
    fn token_key(self) -> &'static str {
        match self {
            Self::Fly => "cloud.fly.token",
        }
    }

    /// The `settings` key holding the account the token acts in: Fly's org
    /// slug.
    fn account_key(self) -> &'static str {
        match self {
            Self::Fly => "cloud.fly.org",
        }
    }

    /// The `settings` key holding the region new machines are created in.
    fn region_key(self) -> &'static str {
        match self {
            Self::Fly => "cloud.fly.region",
        }
    }

    /// The account a connection acts in when none is given: Fly's `personal`
    /// org, which every Fly user has and the API accepts as a slug.
    fn default_account(self) -> &'static str {
        match self {
            Self::Fly => "personal",
        }
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

/// A machine to create. Nothing in it is secret: the environment carries paths
/// and one public ssh key, never a credential.
#[derive(Debug, Clone)]
pub struct MachineSpec {
    /// The caller's name for the machine (its box's agent id). A provider folds
    /// it into its own naming rules and makes it unique; it is there so a
    /// person can tell the machine apart in the provider's dashboard.
    pub name: String,
    /// The image the machine runs (`ghcr.io/fwdai/fletch-cloud:<tag>`).
    pub image: String,
    pub env: Vec<(String, String)>,
    pub cpus: u32,
    pub memory_mib: u32,
    /// The size of the disk the workspace lives on; `None` takes the
    /// provider's default.
    pub disk_gib: Option<u32>,
    /// The port the guest's sshd listens on.
    pub ssh_port: u16,
}

/// A created machine, as its box keeps it: the `Display` form is the opaque
/// string the caller persists, and [`Handle::parse`] reads it back.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Handle {
    /// A Fly.io machine and the app that holds it alone. `machine` is empty
    /// only for an app [`Client::list`] found holding no machine.
    Fly { app: String, machine: String },
}

impl Handle {
    pub fn provider(&self) -> ProviderId {
        match self {
            Self::Fly { .. } => ProviderId::Fly,
        }
    }

    /// Read a handle back from its `Display` form.
    pub fn parse(s: &str) -> Result<Self> {
        let invalid = || Error::Other(format!("not a cloud machine handle: {s:?}"));
        let (provider, rest) = s.split_once(':').ok_or_else(invalid)?;
        match ProviderId::parse(provider).ok_or_else(invalid)? {
            ProviderId::Fly => {
                let (app, machine) = rest.split_once('/').ok_or_else(invalid)?;
                let app_ok = !app.is_empty()
                    && app
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
                if !app_ok || !machine.chars().all(|c| c.is_ascii_alphanumeric()) {
                    return Err(invalid());
                }
                Ok(Self::Fly {
                    app: app.to_string(),
                    machine: machine.to_string(),
                })
            }
        }
    }

    /// Where the machine's sshd is reached. Fixed for the machine's life, so it
    /// needs no call to the provider.
    pub fn endpoint(&self) -> Endpoint {
        match self {
            Self::Fly { app, .. } => fly::endpoint(app),
        }
    }
}

impl fmt::Display for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fly { app, machine } => write!(f, "fly:{app}/{machine}"),
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

/// A machine's state, as much of it as the box registry acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MachineState {
    /// On its way to running, or busy with something that settles by itself
    /// (an update, a move to another host). Ask again.
    Starting,
    /// Running: its sshd is reachable at the endpoint.
    Running,
    /// On its way to stopped.
    Stopping,
    /// Not running; [`Client::start`] starts it.
    Stopped,
    /// Deleted, being deleted, or never there.
    Gone,
    /// The provider could not run it, or reports a state this module does not
    /// know, with the reason. What the machine holds may still be there.
    Failed(String),
}

/// What [`Client::create`] made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    pub handle: Handle,
    pub endpoint: Endpoint,
}

/// One machine [`Client::list`] found.
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
/// the person who reads it and never holding the token, and a machine that no
/// longer exists is [`Error::CloudGone`] wherever it is not simply an answer
/// ([`state`](Self::state), [`destroy`](Self::destroy)).
pub enum Client {
    Fly(fly::Client),
}

impl Client {
    pub fn new(connection: &Connection) -> Result<Self> {
        match connection.provider {
            ProviderId::Fly => Ok(Self::Fly(fly::Client::new(connection, fly::API_BASE)?)),
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

    /// Create a machine and return once it is running.
    ///
    /// Not idempotent, and never retried here. A failure cleans up after
    /// itself: everything created on the way is deleted before the error
    /// returns, so the caller has nothing to undo and simply tries again (a
    /// new machine never collides with what a failed attempt left). If the
    /// cleanup fails too, the error says so and names what was left; it shows
    /// in [`list`](Self::list), where an orphan sweep finds it, and until it is
    /// removed a machine in it may be running and billed.
    pub async fn create(&self, spec: &MachineSpec) -> Result<Created> {
        match self {
            Self::Fly(c) => c.create(spec).await,
        }
    }

    /// The machine's state now. A machine that no longer exists is
    /// [`MachineState::Gone`], not an error.
    pub async fn state(&self, handle: &Handle) -> Result<MachineState> {
        match (self, handle) {
            (Self::Fly(c), Handle::Fly { app, machine }) => c.state(app, machine).await,
        }
    }

    /// Start the machine and return once it is running; a running machine is
    /// left as it is.
    pub async fn start(&self, handle: &Handle) -> Result<()> {
        match (self, handle) {
            (Self::Fly(c), Handle::Fly { app, machine }) => c.start(app, machine).await,
        }
    }

    /// Stop the machine and return once it has stopped; a machine that is not
    /// running is left as it is. Its disk, and the workspace on it, is kept.
    pub async fn stop(&self, handle: &Handle) -> Result<()> {
        match (self, handle) {
            (Self::Fly(c), Handle::Fly { app, machine }) => c.stop(app, machine).await,
        }
    }

    /// Delete the machine and everything created with it, running or not.
    /// Idempotent: a machine that is already gone is success.
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
            (Self::Fly(c), Handle::Fly { app, machine }) => {
                c.exec(app, machine, argv, timeout).await
            }
        }
    }

    /// Every machine this integration created in the connection's account,
    /// whichever box refers to it — the input to an orphan sweep. Includes
    /// what a failed [`create`](Self::create) could not clean up.
    pub async fn list(&self) -> Result<Vec<Listed>> {
        match self {
            Self::Fly(c) => c.list().await,
        }
    }
}

/// One provider's connection state, as Settings shows it.
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
}

impl CloudProviderStatus {
    fn of(provider: ProviderId, connection: Option<&Connection>) -> Self {
        Self {
            id: provider.as_str(),
            label: provider.label(),
            connected: connection.is_some(),
            account: connection.map(|c| c.account.clone()),
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
async fn connect_with(
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
