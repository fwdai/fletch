//! The Fly.io Machines adapter: one Fly app per cloud box, holding one Fly
//! Volume — the box's disk — and one Machine that mounts it.
//!
//! One app per box because of how Fly routes: a shared IPv4 address carries
//! many apps' TLS traffic and Fly's proxy picks the app by the name the client
//! asks for, so `<app>.fly.dev:443` reaches this box's machine and no other,
//! with no port bookkeeping. An orphan sweep is a listing of the apps named
//! with [`APP_PREFIX`].
//!
//! The volume is mounted at `/data`, where the guest keeps everything that is
//! the box's: the agent's trees, its home, the ssh host key, the provider CLIs.
//! It is the box's persistent storage — Fly's, the kind it offers for data,
//! where the root filesystem is a cache it may wipe. A Fly volume is a slice of
//! one host's NVMe drive — not replicated, not network storage — so the
//! machine always runs on the volume's host, and a machine lost while its
//! volume survives (destroyed from the dashboard, or by Fly) is replaced by a
//! new one mounting the same volume ([`Client::replace_machine`]). If the host
//! itself fails, the volume is unreachable with it until it returns, and lost
//! if it never does; Fly takes a snapshot of the volume daily and keeps it
//! [`SNAPSHOT_RETENTION_DAYS`] days, and restoring one into a new volume is
//! Fly's documented recovery — it misses whatever was written since the
//! snapshot, and it is not done here. Fly does not move a volume by itself;
//! forking one copies it to another host, which needs its host reachable. The machine's root filesystem holds only the image and the system
//! changes made on it (`sudo apt-get install`): it is kept across a stop and a
//! start on a best-effort basis — Fly may wipe it for maintenance — and lost
//! when the machine is replaced.
//!
//! The machine runs the cloud image with one service: Fly's proxy accepts TLS
//! on port 443 (its `tls` handler, with the `*.fly.dev` certificate) and
//! forwards the plaintext to the guest's sshd, so the engine reaches the guest
//! with ssh behind a TLS client ([`Endpoint::Tls`]). Fly never starts, stops or
//! restarts the machine by itself — no restart policy, no autostop, no
//! autostart — because the engine owns its lifecycle.
//!
//! Every request has a bounded timeout. The Machines API rate-limits each
//! action per machine (one request a second, bursts of three); a 429 is retried
//! a few times with backoff, which is safe for every request because a
//! rate-limited request was not carried out. Nothing else is retried: creating
//! an app, a volume or a machine is not idempotent, and [`Client::create`]
//! cleans up after a failure instead.
//!
//! The API is documented at <https://docs.fly.io/machines/api/>, with its
//! OpenAPI description at <https://docs.fly.io/api/machines/openapi.json>;
//! volumes at <https://docs.fly.io/volumes/overview/>.

use std::time::{Duration, Instant};

use rand::Rng;
use reqwest::header::{HeaderValue, AUTHORIZATION, RETRY_AFTER};
use reqwest::{Method, StatusCode};
use serde_json::{json, Map, Value};

use super::{
    Connection, Created, Endpoint, ExecOutput, Handle, Listed, MachineSpec, MachineState,
    DEFAULT_DISK_GIB,
};
use crate::error::{Error, Result};

/// The Machines API's public address.
pub const API_BASE: &str = "https://api.machines.dev";

/// The start of every app name this integration creates, which is how
/// [`Client::list`] tells its apps from the account's others. Narrower than
/// `fletch-` so it does not catch an app a person named for themselves.
pub const APP_PREFIX: &str = "fletch-box-";

/// How long Fly keeps each daily snapshot of a box's volume: Fly's default.
/// Snapshots are what a volume lost with its host is restored from.
pub const SNAPSHOT_RETENTION_DAYS: u32 = 5;

/// The machine metadata key marking a machine as a Fletch cloud box, valued
/// with the box's name: what a person sees in Fly's dashboard, and one of the
/// marks [`Client::list`] checks before calling an app ours.
const LABEL: &str = "fletch_box";

/// The name of every box's volume (Fly: at most 30 lowercase letters, digits
/// and underscores). The volume is mounted by its id; the name marks it as a
/// box's, for [`Client::list`] and a person in Fly's dashboard.
const VOLUME_NAME: &str = "fletch_data";

/// Volume states Fly reports for a volume that is going or gone.
const VOLUME_GONE_STATES: &[&str] = &[
    "scheduling_destroy",
    "pending_destroy",
    "destroying",
    "destroyed",
];

/// Fly app names are lowercase letters, digits and dashes, and one DNS label
/// long, since the app is reached at `<app>.fly.dev`.
const APP_NAME_MAX: usize = 63;

/// The random characters ending every app name. App names are global across
/// Fly, so a name must be unique without asking Fly first.
const SUFFIX_LEN: usize = 8;
const SUFFIX_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// The port Fly's proxy accepts a box's TLS on.
const TLS_PORT: u16 = 443;

/// `rootfs.persist` (and its deprecated twin, `guest.persist_rootfs`): Fly's
/// `restart` keeps the root filesystem "across stops and restarts" and resets
/// it "when the Machine is updated". The root holds only system changes — the
/// workspace is on the volume — which is the cache-like use Fly offers it for:
/// worth keeping across a stop, not worth keeping over a new image.
const PERSIST_ROOTFS: &str = "restart";

const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
/// Creating a machine may not answer until Fly has resolved its image, and a
/// request given up on is a create abandoned.
const MACHINE_CREATE_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Added to the HTTP timeout of a request that itself waits (a wait, an exec).
const WAIT_SLACK: Duration = Duration::from_secs(15);
/// The longest one wait request blocks. Fly allows 60 s; half that, so a
/// machine that fails instead of reaching the state is noticed (its state is
/// read between waits) within half a minute.
const WAIT_SLICE: Duration = Duration::from_secs(30);
/// How long a new machine may take to start: its first boot pulls the image.
const CREATE_BOUND: Duration = Duration::from_secs(300);
const START_BOUND: Duration = Duration::from_secs(120);
const STOP_BOUND: Duration = Duration::from_secs(60);
/// The longest Fly runs an exec'd command.
const EXEC_MAX: Duration = Duration::from_secs(600);
/// Retries of a rate-limited request before its refusal stands.
const RATE_LIMIT_RETRIES: u32 = 3;
/// The longest `Retry-After` honoured; a request bounded in seconds does not
/// sleep for minutes on the server's say-so.
const RETRY_AFTER_MAX: Duration = Duration::from_secs(10);

/// Where a box's sshd is reached: TLS to the app's `fly.dev` name.
pub(super) fn endpoint(app: &str) -> Endpoint {
    Endpoint::Tls {
        host: format!("{app}.fly.dev"),
        port: TLS_PORT,
    }
}

/// A Machines API client for one Fly org.
pub struct Client {
    http: reqwest::Client,
    base: String,
    auth: HeaderValue,
    /// Kept only to redact it from every message built here.
    token: String,
    org: String,
    region: Option<String>,
    /// The unit of backoff after a 429 and of the pause between state reads
    /// while waiting.
    pace: Duration,
}

/// A response: the status and the JSON body, `Null` when there was none.
struct Reply {
    status: StatusCode,
    body: Value,
}

/// What a machine was seen as.
struct Seen {
    state: MachineState,
    /// Fly's own name for the state, for messages.
    raw: String,
    /// The machine's version, which waiting for `stopped` names.
    version: Option<String>,
}

/// What a box's volume was seen as.
enum Disk {
    /// There, in `region`.
    Present { region: String },
    /// Deleted, being deleted, or never there.
    Gone,
    /// There, but not usable now (failed, or on a host Fly cannot reach).
    Failed(String),
}

/// A state a machine is driven to and waited for.
#[derive(Clone, Copy)]
enum Target {
    Started,
    Stopped,
    Destroyed,
}

impl Target {
    fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Stopped => "stopped",
            Self::Destroyed => "destroyed",
        }
    }

    /// The request that drives a machine there; `Destroyed` is a `DELETE`
    /// and never asked for through [`Client::request`].
    fn action(self) -> &'static str {
        match self {
            Self::Started => "start",
            Self::Stopped => "stop",
            Self::Destroyed => "destroy",
        }
    }

    fn reached(self, state: &MachineState) -> bool {
        match self {
            Self::Started => *state == MachineState::Running,
            // A failed machine is not running either.
            Self::Stopped => matches!(state, MachineState::Stopped | MachineState::Failed(_)),
            Self::Destroyed => *state == MachineState::Gone,
        }
    }

    /// Reached, or on its way there with no further request.
    fn underway(self, state: &MachineState) -> bool {
        self.reached(state)
            || match self {
                Self::Started => *state == MachineState::Starting,
                Self::Stopped => *state == MachineState::Stopping,
                Self::Destroyed => false,
            }
    }
}

impl Client {
    /// A client for `connection`'s org at `base`: [`API_BASE`], or a stand-in
    /// in tests.
    pub fn new(connection: &Connection, base: &str) -> Result<Self> {
        let token = connection.token().to_string();
        let mut auth = HeaderValue::from_str(&authorization(&token)).map_err(|_| {
            Error::Other(
                "the Fly.io token holds characters an HTTP header cannot carry — paste it again"
                    .into(),
            )
        })?;
        auth.set_sensitive(true);
        let http = reqwest::Client::builder()
            .user_agent("Fletch")
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|e| Error::Other(format!("http client: {e}")))?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            auth,
            token,
            org: connection.account().to_string(),
            region: connection.region().map(str::to_string),
            pace: Duration::from_secs(1),
        })
    }

    /// The same client pacing its retries and state reads by `pace`, so tests
    /// against a stand-in server do not sleep for seconds.
    #[cfg(test)]
    pub(crate) fn with_pace(mut self, pace: Duration) -> Self {
        self.pace = pace;
        self
    }

    /// List the org's apps: accepted only for a token that can act in the
    /// org. A refused token is a 401; Fly's answer for an org the token cannot
    /// use is not documented, so each plausible one reads as that.
    pub async fn validate(&self) -> Result<()> {
        let reply = self
            .call(
                Method::GET,
                "/v1/apps",
                &[("org_slug", &self.org)],
                None,
                HTTP_TIMEOUT,
            )
            .await?;
        match reply.status.as_u16() {
            200..=299 => Ok(()),
            400 | 403 | 404 | 422 => Err(self.error(format!(
                "Fly.io has no organization \"{}\" this token can use{} — check the \
                 organization's slug (`fly orgs list`)",
                self.org,
                detail(&reply),
            ))),
            _ => Err(self.refusal(&reply, "list the organization's apps")),
        }
    }

    /// Create the box's app, give it its addresses, create its volume and then
    /// its machine on the volume's host, and wait for the machine to start; on
    /// any failure once the app may exist, destroy the app with everything in
    /// it before returning (see `cloud::Client::create`).
    pub async fn create(&self, spec: &MachineSpec) -> Result<Created> {
        let app = app_name(&spec.name);
        // `name` is the field the OpenAPI description gives, `app_name` the one
        // the published guide gives; both carry the same name, so either
        // reading of the request creates the same app.
        let body = json!({ "name": app, "app_name": app, "org_slug": self.org });
        let reply = match self
            .call(Method::POST, "/v1/apps", &[], Some(&body), HTTP_TIMEOUT)
            .await
        {
            Ok(reply) => reply,
            // Lost on the way: the app may exist all the same.
            Err(e) => return Err(self.abandon(&app, e).await),
        };
        if !reply.status.is_success() {
            let error = self.refusal(&reply, "create an app");
            // A 4xx refused the request outright, so there is no app — and a
            // name taken is someone else's, never to be deleted. A 5xx may
            // have created it before failing.
            return Err(if reply.status.is_server_error() {
                self.abandon(&app, error).await
            } else {
                error
            });
        }
        match self.provision(&app, spec).await {
            Ok((volume, machine)) => {
                tracing::info!(app, volume, machine, "created a Fly.io cloud box");
                Ok(Created {
                    endpoint: endpoint(&app),
                    handle: Handle::Fly {
                        org: self.org.clone(),
                        app,
                        volume,
                        machine,
                    },
                })
            }
            Err(e) => Err(self.abandon(&app, e).await),
        }
    }

    /// Everything `create` does once the app exists; returns the volume's and
    /// the machine's ids.
    async fn provision(&self, app: &str, spec: &MachineSpec) -> Result<(String, String)> {
        // An app created through the Machines API gets no address of its own.
        // A shared IPv4 is what clients reach `<app>.fly.dev` by, and the
        // IPv6 is what `fly deploy` gives every public app; both are free.
        for kind in ["shared_v4", "v6"] {
            let reply = self
                .call(
                    Method::POST,
                    &format!("/v1/apps/{app}/ip_assignments"),
                    &[],
                    Some(&json!({ "type": kind })),
                    HTTP_TIMEOUT,
                )
                .await?;
            if !reply.status.is_success() {
                return Err(self.refusal(&reply, &format!("give the app an address ({kind})")));
            }
        }
        let reply = self
            .call(
                Method::POST,
                &format!("/v1/apps/{app}/volumes"),
                &[],
                Some(&volume_body(spec, self.region.as_deref())),
                HTTP_TIMEOUT,
            )
            .await?;
        if !reply.status.is_success() {
            return Err(self.refusal(&reply, "create the box's disk"));
        }
        let (Some(volume), Some(region)) = (
            reply.body["id"].as_str().filter(|v| !v.is_empty()),
            reply.body["region"].as_str().filter(|r| !r.is_empty()),
        ) else {
            return Err(self
                .error("Fly.io created the box's disk but did not say its id and region".into()));
        };
        // The machine goes where the volume is: Fly places a volume in the
        // region asked for, or near the API server without one, and a machine
        // can only mount a volume on its own host.
        let machine = self.create_machine(app, spec, volume, region).await?;
        self.wait_for(app, &machine, Target::Started, None, CREATE_BOUND)
            .await?;
        Ok((volume.to_string(), machine))
    }

    /// Create the box's machine, mounting `volume` in `region`; returns its id.
    async fn create_machine(
        &self,
        app: &str,
        spec: &MachineSpec,
        volume: &str,
        region: &str,
    ) -> Result<String> {
        let reply = self
            .call(
                Method::POST,
                &format!("/v1/apps/{app}/machines"),
                &[],
                Some(&machine_body(app, spec, volume, region)),
                MACHINE_CREATE_TIMEOUT,
            )
            .await?;
        if !reply.status.is_success() {
            return Err(self.pinned_refusal(&reply, "create the machine", Some(region)));
        }
        reply.body["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .ok_or_else(|| self.error("Fly.io created a machine but did not say its id".into()))
    }

    /// Destroy `app` after `create` failed partway, returning the error to
    /// report. If that fails too, the error says so and names the app.
    async fn abandon(&self, app: &str, error: Error) -> Error {
        match self.destroy(app).await {
            Ok(()) => error,
            Err(cleanup) => {
                tracing::warn!(app, error = %cleanup, "could not delete the app a failed create left");
                self.error(format!(
                    "{error} — and the Fly.io app {app} it left could not be deleted ({cleanup}); \
                     delete it in the Fly.io dashboard"
                ))
            }
        }
    }

    /// The box's state, from its volume and its machine (and its app, when
    /// neither answers). `org` is the org the box was created in: when nothing
    /// answers and the connection is to another org, this token may simply not
    /// see the box, which is an error rather than [`MachineState::Gone`].
    pub async fn state(
        &self,
        org: &str,
        app: &str,
        volume: &str,
        machine: &str,
    ) -> Result<MachineState> {
        let disk = self.disk(app, volume).await?;
        let machine_state = self.observe(app, machine).await?.map(|seen| seen.state);
        Ok(match (disk, machine_state) {
            (Disk::Failed(reason), _) => MachineState::Failed(reason),
            (Disk::Present { .. }, None | Some(MachineState::Gone)) => MachineState::MachineGone,
            (Disk::Present { .. }, Some(state)) => state,
            // The machine answers, so the app is there: the disk alone went.
            (Disk::Gone, Some(_)) => MachineState::DiskGone,
            (Disk::Gone, None) => {
                if self.app_exists(app).await? {
                    MachineState::DiskGone
                } else if org != self.org {
                    return Err(self.error(format!(
                        "Fly.io shows nothing of this box to a connection to the organization \
                         \"{}\"; it was created in \"{org}\" — connect Fly.io to \"{org}\" to \
                         reach it",
                        self.org
                    )));
                } else {
                    MachineState::Gone
                }
            }
        })
    }

    pub async fn start(&self, app: &str, machine: &str) -> Result<()> {
        let seen = self
            .observe(app, machine)
            .await?
            .ok_or_else(|| gone(app, machine))?;
        match seen.state {
            MachineState::Running => return Ok(()),
            MachineState::Gone | MachineState::MachineGone | MachineState::DiskGone => {
                return Err(gone(app, machine))
            }
            MachineState::Starting => {}
            MachineState::Stopping => {
                let version = seen.version.as_deref();
                self.wait_for(app, machine, Target::Stopped, version, STOP_BOUND)
                    .await?;
                self.request(app, machine, Target::Started).await?;
            }
            MachineState::Stopped | MachineState::Failed(_) => {
                self.request(app, machine, Target::Started).await?;
            }
        }
        self.wait_for(app, machine, Target::Started, None, START_BOUND)
            .await
    }

    pub async fn stop(&self, app: &str, machine: &str) -> Result<()> {
        let seen = self
            .observe(app, machine)
            .await?
            .ok_or_else(|| gone(app, machine))?;
        match seen.state {
            MachineState::Stopped | MachineState::Failed(_) => return Ok(()),
            MachineState::Gone | MachineState::MachineGone | MachineState::DiskGone => {
                return Err(gone(app, machine))
            }
            MachineState::Stopping => {}
            MachineState::Running | MachineState::Starting => {
                self.request(app, machine, Target::Stopped).await?;
            }
        }
        let version = seen.version.as_deref();
        self.wait_for(app, machine, Target::Stopped, version, STOP_BOUND)
            .await
    }

    /// A new machine for a box whose machine is gone, in `app`, mounting the
    /// box's existing `volume` in the volume's region, from `spec` (as
    /// `create` was given it). Refused while the old machine still exists (a
    /// volume mounts on one machine at a time); refused with
    /// [`Error::CloudGone`] when the volume is gone — never a fresh, empty
    /// one in its place. A failure after the new machine exists deletes that
    /// machine and nothing else.
    pub async fn replace_machine(
        &self,
        org: &str,
        app: &str,
        volume: &str,
        machine: &str,
        spec: &MachineSpec,
    ) -> Result<Created> {
        if let Some(seen) = self.observe(app, machine).await? {
            if seen.state != MachineState::Gone {
                return Err(self.error(format!(
                    "the Fly.io machine {machine} (app {app}) still exists (Fly.io reports it \
                     {}); only a machine that is gone is replaced",
                    seen.raw
                )));
            }
        }
        let region = match self.disk(app, volume).await? {
            Disk::Present { region } => region,
            Disk::Gone => return Err(disk_gone(app, volume)),
            Disk::Failed(reason) => {
                return Err(self.error(format!(
                    "the Fly.io volume {volume} (app {app}) holding the box's workspace cannot be \
                     used now: {reason}"
                )))
            }
        };
        let new = self.create_machine(app, spec, volume, &region).await?;
        if let Err(e) = self
            .wait_for(app, &new, Target::Started, None, CREATE_BOUND)
            .await
        {
            return Err(self.discard_machine(app, &new, e).await);
        }
        tracing::info!(
            app,
            volume,
            machine = new,
            "replaced a Fly.io cloud box's machine"
        );
        Ok(Created {
            endpoint: endpoint(app),
            handle: Handle::Fly {
                org: org.to_string(),
                app: app.to_string(),
                volume: volume.to_string(),
                machine: new,
            },
        })
    }

    /// Delete a machine `replace_machine` made and could not start, returning
    /// the error to report. The volume and the app stay.
    async fn discard_machine(&self, app: &str, machine: &str, error: Error) -> Error {
        match self.destroy_machine(app, machine).await {
            Ok(()) => error,
            Err(cleanup) => {
                tracing::warn!(app, machine, error = %cleanup, "could not delete a replacement machine that failed");
                self.error(format!(
                    "{error} — and the new Fly.io machine {machine} could not be deleted \
                     ({cleanup}); it holds the box's disk until it is"
                ))
            }
        }
    }

    /// Delete the box's app with everything in it, in the order Fly allows:
    /// the machines (a volume a machine holds cannot be deleted), then the
    /// volumes, then the app. Fly documents that destroying an app takes its
    /// volumes and their snapshots with it, but only for `fly apps destroy`;
    /// deleting each volume here leaves none behind whatever the API's app
    /// delete does. Only ever an app named with [`APP_PREFIX`], whatever a
    /// handle says; anything already gone is success.
    pub async fn destroy(&self, app: &str) -> Result<()> {
        if !app.starts_with(APP_PREFIX) {
            return Err(Error::Other(format!(
                "{app} is not a Fletch cloud box's app; refusing to delete it"
            )));
        }
        let Some(machines) = self.app_list(app, "machines").await? else {
            return Ok(());
        };
        for machine in machines.iter().filter_map(|m| m["id"].as_str()) {
            self.destroy_machine(app, machine).await?;
        }
        let Some(volumes) = self.app_list(app, "volumes").await? else {
            return Ok(());
        };
        for volume in volumes.iter().filter(|v| volume_live(v)) {
            let Some(id) = volume["id"].as_str() else {
                continue;
            };
            let reply = self
                .call(
                    Method::DELETE,
                    &format!("/v1/apps/{app}/volumes/{id}"),
                    &[],
                    None,
                    HTTP_TIMEOUT,
                )
                .await?;
            if !reply.status.is_success() && reply.status != StatusCode::NOT_FOUND {
                return Err(self.refusal(&reply, "delete the box's disk"));
            }
        }
        let reply = self
            .call(
                Method::DELETE,
                &format!("/v1/apps/{app}"),
                &[("force", "true")],
                None,
                HTTP_TIMEOUT,
            )
            .await?;
        if reply.status.is_success() || reply.status == StatusCode::NOT_FOUND {
            return Ok(());
        }
        Err(self.refusal(&reply, "delete the app"))
    }

    /// Delete one machine, killing it if it runs, and wait until Fly reports
    /// it destroyed, so its volume is free.
    async fn destroy_machine(&self, app: &str, machine: &str) -> Result<()> {
        let reply = self
            .call(
                Method::DELETE,
                &format!("/v1/apps/{app}/machines/{machine}"),
                &[("force", "true")],
                None,
                HTTP_TIMEOUT,
            )
            .await?;
        if reply.status == StatusCode::NOT_FOUND {
            return Ok(());
        }
        if !reply.status.is_success() {
            return Err(self.refusal(&reply, "delete the machine"));
        }
        self.wait_for(app, machine, Target::Destroyed, None, STOP_BOUND)
            .await
    }

    /// Run `argv` through Fly's synchronous exec, with `timeout` (clamped to
    /// what Fly allows) passed to Fly rather than left at its 10 s default.
    pub async fn exec(
        &self,
        app: &str,
        machine: &str,
        argv: &[String],
        timeout: Duration,
    ) -> Result<ExecOutput> {
        let Some(program) = argv.first() else {
            return Err(Error::Other("no command to run".into()));
        };
        if machine.is_empty() {
            return Err(gone(app, machine));
        }
        let secs = timeout.clamp(Duration::from_secs(1), EXEC_MAX).as_secs();
        let reply = self
            .call(
                Method::POST,
                &format!("/v1/apps/{app}/machines/{machine}/exec"),
                &[],
                Some(&json!({ "command": argv, "timeout": secs })),
                Duration::from_secs(secs) + WAIT_SLACK,
            )
            .await?;
        match reply.status {
            s if s.is_success() => Ok(exec_output(&reply.body)),
            StatusCode::NOT_FOUND => Err(gone(app, machine)),
            // Fly's 412: the machine is not running, or the command could not
            // start at all (missing, not executable).
            StatusCode::PRECONDITION_FAILED => Err(self.error(format!(
                "Fly.io could not run {program} in the machine {machine}: it is not running, \
                 or the command could not start{}",
                detail(&reply),
            ))),
            _ => Err(self.refusal(&reply, &format!("run {program} in the machine"))),
        }
    }

    /// The org's box apps, each as one [`Listed`] per machine — or one with an
    /// empty machine id when it holds none. An app is a box's only if it is
    /// named with [`APP_PREFIX`], every machine in it carries [`LABEL`], every
    /// volume in it is named [`VOLUME_NAME`] and at most one of them is live;
    /// any other app is left out. That includes a box's app holding two live
    /// volumes (a fork or a restore someone made by hand: Fly lets volumes
    /// share a name): one entry cannot stand for both, and a caller matching
    /// its boxes with [`Handle::same_box`] must never find the volume a box
    /// refers to missing from an app it is shown.
    /// The state says what is left of the box: a machine's own state (its
    /// volume there), [`MachineState::MachineGone`] for a volume with no
    /// machine — a workspace a box may still refer to, recoverable with
    /// `replace_machine` — [`MachineState::DiskGone`] for a machine with no
    /// volume, and [`MachineState::Gone`] for an empty app (what a `create`
    /// whose cleanup failed leaves).
    pub async fn list(&self) -> Result<Vec<Listed>> {
        let reply = self
            .call(
                Method::GET,
                "/v1/apps",
                &[("org_slug", &self.org)],
                None,
                HTTP_TIMEOUT,
            )
            .await?;
        if !reply.status.is_success() {
            return Err(self.refusal(&reply, "list the organization's apps"));
        }
        let mut listed = Vec::new();
        for app in reply.body["apps"].as_array().into_iter().flatten() {
            let Some(name) = app["name"].as_str().filter(|n| n.starts_with(APP_PREFIX)) else {
                continue;
            };
            // Deleted since the listing: nothing to report.
            let Some(machines) = self.app_list(name, "machines").await? else {
                continue;
            };
            let Some(volumes) = self.app_list(name, "volumes").await? else {
                continue;
            };
            let volumes: Vec<&Value> = volumes.iter().filter(|v| volume_live(v)).collect();
            if !machines.iter().all(labelled)
                || !volumes
                    .iter()
                    .all(|v| v["name"].as_str() == Some(VOLUME_NAME))
                || volumes.len() > 1
            {
                continue;
            }
            let volume = volumes
                .first()
                .and_then(|v| v["id"].as_str())
                .unwrap_or_default();
            let handle = |machine: &str| Handle::Fly {
                org: self.org.clone(),
                app: name.to_string(),
                volume: volume.to_string(),
                machine: machine.to_string(),
            };
            if machines.is_empty() {
                listed.push(Listed {
                    handle: handle(""),
                    state: if volume.is_empty() {
                        MachineState::Gone
                    } else {
                        MachineState::MachineGone
                    },
                });
                continue;
            }
            for machine in &machines {
                let Some(id) = machine["id"].as_str().filter(|id| !id.is_empty()) else {
                    continue;
                };
                let state = machine_state(machine["state"].as_str().unwrap_or_default());
                listed.push(Listed {
                    handle: handle(id),
                    state: match state {
                        _ if volume.is_empty() => MachineState::DiskGone,
                        MachineState::Gone => MachineState::MachineGone,
                        state => state,
                    },
                });
            }
        }
        Ok(listed)
    }

    /// An app's machines or volumes (`what`), `None` when the app is gone.
    async fn app_list(&self, app: &str, what: &str) -> Result<Option<Vec<Value>>> {
        let reply = self
            .call(
                Method::GET,
                &format!("/v1/apps/{app}/{what}"),
                &[],
                None,
                HTTP_TIMEOUT,
            )
            .await?;
        if reply.status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !reply.status.is_success() {
            return Err(self.refusal(&reply, &format!("list the app's {what}")));
        }
        Ok(Some(reply.body.as_array().cloned().unwrap_or_default()))
    }

    /// Whether Fly has the app at all (its documented existence check).
    async fn app_exists(&self, app: &str) -> Result<bool> {
        let reply = self
            .call(
                Method::GET,
                &format!("/v1/apps/{app}"),
                &[],
                None,
                HTTP_TIMEOUT,
            )
            .await?;
        match reply.status {
            StatusCode::NOT_FOUND => Ok(false),
            s if s.is_success() => Ok(true),
            _ => Err(self.refusal(&reply, "read the app")),
        }
    }

    /// The box's volume as Fly reports it now. A 404 is [`Disk::Gone`]
    /// whether the volume or its whole app went.
    async fn disk(&self, app: &str, volume: &str) -> Result<Disk> {
        if volume.is_empty() {
            return Ok(Disk::Gone);
        }
        let reply = self
            .call(
                Method::GET,
                &format!("/v1/apps/{app}/volumes/{volume}"),
                &[],
                None,
                HTTP_TIMEOUT,
            )
            .await?;
        if reply.status == StatusCode::NOT_FOUND {
            return Ok(Disk::Gone);
        }
        if !reply.status.is_success() {
            return Err(self.refusal(&reply, "read the box's disk"));
        }
        let body = &reply.body;
        if !volume_live(body) {
            return Ok(Disk::Gone);
        }
        if body["state"].as_str() == Some("failed") {
            return Ok(Disk::Failed("Fly.io reports the box's disk failed".into()));
        }
        if host_unreachable(body) {
            return Ok(Disk::Failed(
                "the Fly.io host holding the box's disk is unreachable; the disk comes back \
                 with the host, or is restored from a snapshot"
                    .into(),
            ));
        }
        Ok(Disk::Present {
            region: body["region"].as_str().unwrap_or_default().to_string(),
        })
    }

    /// The machine as Fly reports it now; `None` when it does not exist. A
    /// machine on a host Fly cannot reach is [`MachineState::Failed`], since
    /// its state is only the last one known.
    async fn observe(&self, app: &str, machine: &str) -> Result<Option<Seen>> {
        if machine.is_empty() {
            return Ok(None);
        }
        let reply = self
            .call(
                Method::GET,
                &format!("/v1/apps/{app}/machines/{machine}"),
                &[],
                None,
                HTTP_TIMEOUT,
            )
            .await?;
        if reply.status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !reply.status.is_success() {
            return Err(self.refusal(&reply, "read the machine's state"));
        }
        let raw = reply.body["state"].as_str().unwrap_or_default().to_string();
        let version = ["version", "instance_id"]
            .iter()
            .find_map(|key| reply.body[*key].as_str().filter(|v| !v.is_empty()))
            .map(str::to_string);
        let state = if host_unreachable(&reply.body) {
            MachineState::Failed(
                "the Fly.io host the machine is on is unreachable; it comes back with the host"
                    .into(),
            )
        } else {
            machine_state(&raw)
        };
        Ok(Some(Seen {
            state,
            raw,
            version,
        }))
    }

    /// Ask Fly to start or stop the machine. A refusal stands only if the
    /// machine is not already there or on its way (a stop that raced the
    /// machine's own exit, a start of one already starting).
    async fn request(&self, app: &str, machine: &str, target: Target) -> Result<()> {
        let path = format!("/v1/apps/{app}/machines/{machine}/{}", target.action());
        let reply = self
            .call(Method::POST, &path, &[], None, HTTP_TIMEOUT)
            .await?;
        if reply.status.is_success() {
            return Ok(());
        }
        if reply.status == StatusCode::NOT_FOUND {
            return Err(gone(app, machine));
        }
        match self.observe(app, machine).await? {
            Some(seen) if target.underway(&seen.state) => Ok(()),
            _ => {
                Err(self.pinned_refusal(&reply, &format!("{} the machine", target.action()), None))
            }
        }
    }

    /// Wait until the machine reaches `target`, within `bound`. Fly's wait
    /// blocks a slice at a time; whenever it comes back any other way than
    /// reached — timed out, refused, lost — the machine's state is read, so a
    /// machine that failed or went is reported then rather than at the bound.
    /// Waiting for `stopped` names the machine's version, as Fly asks.
    async fn wait_for(
        &self,
        app: &str,
        machine: &str,
        target: Target,
        version: Option<&str>,
        bound: Duration,
    ) -> Result<()> {
        let deadline = Instant::now() + bound;
        let path = format!("/v1/apps/{app}/machines/{machine}/wait");
        loop {
            let slice = deadline
                .saturating_duration_since(Instant::now())
                .min(WAIT_SLICE)
                .as_secs()
                .max(1);
            let timeout = slice.to_string();
            let mut query = vec![("state", target.as_str()), ("timeout", timeout.as_str())];
            if let Some(version) = version {
                query.push(("instance_id", version));
            }
            let http_timeout = Duration::from_secs(slice) + WAIT_SLACK;
            match self
                .call(Method::GET, &path, &query, None, http_timeout)
                .await
            {
                Ok(reply) if reply.status.is_success() => return Ok(()),
                Ok(reply) if reply.status == StatusCode::NOT_FOUND => {
                    return match target {
                        Target::Destroyed => Ok(()),
                        _ => Err(gone(app, machine)),
                    }
                }
                _ => {}
            }
            let (state, raw) = match self.observe(app, machine).await? {
                Some(seen) => (seen.state, seen.raw),
                None => (MachineState::Gone, "gone".to_string()),
            };
            if target.reached(&state) {
                return Ok(());
            }
            match (&state, target) {
                (MachineState::Gone, _) => return Err(gone(app, machine)),
                (MachineState::Failed(reason), Target::Started) => {
                    return Err(self.error(format!(
                        "the Fly.io machine {machine} did not start: {reason}"
                    )))
                }
                _ => {}
            }
            if Instant::now() >= deadline {
                return Err(self.error(format!(
                    "the Fly.io machine {machine} was not {} after {} s (Fly.io reports it {raw})",
                    target.as_str(),
                    bound.as_secs(),
                )));
            }
            tokio::time::sleep(self.pace).await;
        }
    }

    /// One API request. A transport failure is an error; an HTTP failure is
    /// not — what a 404 or a 412 means is the caller's to say — except that a
    /// 429 is retried with backoff (honouring a short `Retry-After`) before it
    /// is returned.
    async fn call(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<&Value>,
        timeout: Duration,
    ) -> Result<Reply> {
        let mut url = reqwest::Url::parse(&format!("{}{path}", self.base))
            .map_err(|e| Error::Other(format!("Fly.io request address: {e}")))?;
        // Percent-encoded here: an org slug is the user's to type. Only when
        // there is a query, or the URL would end in a bare `?`.
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        let mut retries = 0;
        loop {
            let mut request = self
                .http
                .request(method.clone(), url.clone())
                .header(AUTHORIZATION, self.auth.clone())
                .timeout(timeout);
            if let Some(body) = body {
                request = request.json(body);
            }
            let response = request
                .send()
                .await
                .map_err(|e| self.error(format!("could not reach Fly.io: {e}")))?;
            let status = response.status();
            let retry_after = response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok());
            // Some answers have no body (a 202 for a deleted app): null.
            let body = response.json::<Value>().await.unwrap_or(Value::Null);
            if status == StatusCode::TOO_MANY_REQUESTS && retries < RATE_LIMIT_RETRIES {
                let backoff = retry_after.map_or(self.pace * 2u32.pow(retries), |secs| {
                    Duration::from_secs(secs).min(RETRY_AFTER_MAX)
                });
                retries += 1;
                tracing::debug!(path, retries, "Fly.io rate-limited a request; retrying");
                tokio::time::sleep(backoff).await;
                continue;
            }
            return Ok(Reply { status, body });
        }
    }

    /// [`refusal`](Self::refusal) for a request bound to the volume's host (a
    /// machine on the box's disk): no capacity there is the host's, not the
    /// region's, and waiting is the remedy.
    fn pinned_refusal(&self, reply: &Reply, doing: &str, region: Option<&str>) -> Error {
        if reply.body["status"].as_str() == Some("insufficient_capacity") {
            let region = region.map(|r| format!(" (in {r})")).unwrap_or_default();
            return self.error(format!(
                "Fly.io has no room right now on the host that holds this box's disk{region}, \
                 so it could not {doing} — try again later"
            ));
        }
        self.refusal(reply, doing)
    }

    /// The error for a request Fly answered with a failure, for the person who
    /// reads it. `doing` completes "refused to …": "create the machine".
    fn refusal(&self, reply: &Reply, doing: &str) -> Error {
        let status = reply.status.as_u16();
        let detail = detail(reply);
        let message = if reply.body["status"].as_str() == Some("insufficient_capacity") {
            format!(
                "Fly.io has no capacity in {} right now — try again later, or choose another \
                 region",
                self.region.as_deref().unwrap_or("the region it picked"),
            )
        } else {
            match status {
                401 => "Fly.io refused the token — it is invalid, expired or revoked; connect \
                        Fly.io again with a new one"
                    .to_string(),
                403 => format!(
                    "the Fly.io token may not {doing} in the organization \"{}\"{detail}",
                    self.org
                ),
                429 => "Fly.io is rate-limiting Fletch's requests — try again in a minute".into(),
                500.. => format!("Fly.io failed to {doing} (HTTP {status}){detail} — try again"),
                _ => format!("Fly.io refused to {doing} (HTTP {status}){detail}"),
            }
        };
        self.error(message)
    }

    /// An error carrying `message` with the token taken out of it. Fly's own
    /// error text is quoted in messages, and nothing promises it never echoes
    /// a credential back.
    fn error(&self, message: String) -> Error {
        Error::Other(redact(message, &self.token))
    }
}

/// The `Authorization` value for a token. A token from `fly tokens create`
/// already carries its scheme (`FlyV1 fm2_…`) and is sent as it is; any other
/// (`fly auth token`) is a bearer token.
fn authorization(token: &str) -> String {
    if token.starts_with("FlyV1 ") {
        token.to_string()
    } else {
        format!("Bearer {token}")
    }
}

/// `message` with every piece of `token` long enough to identify it replaced:
/// the whole, the part after a `FlyV1 ` scheme, and each comma-separated
/// macaroon of it.
fn redact(mut message: String, token: &str) -> String {
    let bare = token.strip_prefix("FlyV1 ").unwrap_or(token);
    let mut pieces: Vec<&str> = vec![token, bare];
    pieces.extend(bare.split(','));
    for piece in pieces.into_iter().map(str::trim).filter(|p| p.len() >= 8) {
        message = message.replace(piece, "[token]");
    }
    message
}

/// ": <Fly's error text>", or nothing when the body has none.
fn detail(reply: &Reply) -> String {
    reply.body["error"]
        .as_str()
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(|e| format!(": {e}"))
        .unwrap_or_default()
}

fn gone(app: &str, machine: &str) -> Error {
    Error::CloudGone(format!(
        "the Fly.io machine {machine} (app {app}) no longer exists"
    ))
}

fn disk_gone(app: &str, volume: &str) -> Error {
    Error::CloudGone(format!(
        "the Fly.io volume {volume} (app {app}) that held this box's workspace no longer exists; \
         what was on it is lost"
    ))
}

/// Whether a listed machine carries [`LABEL`]. Its `config` is null when its
/// host could not be reached, with the last known one in `incomplete_config`.
fn labelled(machine: &Value) -> bool {
    ["config", "incomplete_config"]
        .iter()
        .any(|key| machine[*key]["metadata"].get(LABEL).is_some())
}

/// Whether a volume is there, not deleted or on its way to being deleted.
fn volume_live(volume: &Value) -> bool {
    !VOLUME_GONE_STATES.contains(&volume["state"].as_str().unwrap_or_default())
}

/// Whether Fly reports it could not reach the host of a machine or volume.
fn host_unreachable(body: &Value) -> bool {
    body["host_status"].as_str() == Some("unreachable")
}

/// A new app's name: [`APP_PREFIX`], the box's name folded into Fly's rules,
/// and a random suffix.
fn app_name(name: &str) -> String {
    let mut rng = rand::rng();
    let suffix: String = (0..SUFFIX_LEN)
        .map(|_| SUFFIX_ALPHABET[rng.random_range(0..SUFFIX_ALPHABET.len())] as char)
        .collect();
    fold_app_name(name, &suffix)
}

/// [`app_name`] with a given suffix: lowercase ASCII letters and digits kept,
/// every run of anything else one dash, no dash at either end, and the whole
/// within one DNS label.
fn fold_app_name(name: &str, suffix: &str) -> String {
    let budget = APP_NAME_MAX - APP_PREFIX.len() - suffix.len() - 1;
    let mut folded = String::new();
    for c in name.chars().map(|c| c.to_ascii_lowercase()) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            folded.push(c);
        } else if !folded.is_empty() && !folded.ends_with('-') {
            folded.push('-');
        }
    }
    folded.truncate(budget);
    match folded.trim_end_matches('-') {
        "" => format!("{APP_PREFIX}{suffix}"),
        folded => format!("{APP_PREFIX}{folded}-{suffix}"),
    }
}

/// The create-volume request for a box's disk: `disk_gib` (or
/// [`DEFAULT_DISK_GIB`]) of ext4, encrypted, snapshotted daily and kept
/// [`SNAPSHOT_RETENTION_DAYS`] days, placed on a host that can also run the
/// box's machine (`compute`: without it Fly may pick a host with room for the
/// volume and none for the machine). Without a region Fly places it in the
/// region of the API server that takes the request, usually the nearest.
///
/// What it costs, as Fly prices it (<https://docs.fly.io/about/pricing/>,
/// read November 2026): a volume is billed for its provisioned size whether
/// its machine runs or not — "$0.15/GB per month … including while detached
/// or attached to a stopped Machine" — so the default 8 GiB is $1.20 a month
/// for as long as the box exists. Snapshots add $0.08/GB a month of data
/// stored, the first 10 GB a month free; a stopped machine's root filesystem
/// is billed at $0.15/GB a month as well.
fn volume_body(spec: &MachineSpec, region: Option<&str>) -> Value {
    let mut body = json!({
        "name": VOLUME_NAME,
        "size_gb": spec.disk_gib.unwrap_or(DEFAULT_DISK_GIB),
        "fstype": "ext4",
        "encrypted": true,
        "auto_backup_enabled": true,
        "snapshot_retention": SNAPSHOT_RETENTION_DAYS,
        // One volume per app: no other volume to keep it apart from, and a
        // unique zone can only make placement fail.
        "require_unique_zone": false,
        "compute": guest(spec),
    });
    if let Some(region) = region {
        body["region"] = json!(region);
    }
    body
}

/// The machine's size, for the machine itself and for placing its volume.
fn guest(spec: &MachineSpec) -> Value {
    json!({ "cpu_kind": "shared", "cpus": spec.cpus, "memory_mb": spec.memory_mib })
}

/// The create-machine request for a box: the image with its environment and
/// size, in the volume's region with the volume mounted where the guest keeps
/// the box's files, one TCP service taking TLS on 443 to the guest's sshd,
/// nothing that would start, stop or restart the machine behind the engine's
/// back, the root filesystem kept across a stop (best effort), and the box's
/// label.
fn machine_body(app: &str, spec: &MachineSpec, volume: &str, region: &str) -> Value {
    let env: Map<String, Value> = spec
        .env
        .iter()
        .map(|(k, v)| (k.clone(), Value::from(v.as_str())))
        .collect();
    let mut metadata = Map::new();
    metadata.insert(LABEL.into(), Value::from(spec.name.as_str()));
    let mut sizing = guest(spec);
    sizing["persist_rootfs"] = json!(PERSIST_ROOTFS);
    json!({
        "name": app,
        "region": region,
        "config": {
            "image": spec.image,
            "env": env,
            "guest": sizing,
            "rootfs": { "persist": PERSIST_ROOTFS },
            "mounts": [{ "volume": volume, "path": fletch_box::guest::DATA_MOUNT }],
            "restart": { "policy": "no" },
            "auto_destroy": false,
            "metadata": metadata,
            "services": [{
                "protocol": "tcp",
                "internal_port": spec.ssh_port,
                "autostart": false,
                "autostop": "off",
                "ports": [{ "port": TLS_PORT, "handlers": ["tls"] }],
            }],
        },
    })
}

/// What an exec response says. Fly may leave out a zero exit code or signal.
fn exec_output(body: &Value) -> ExecOutput {
    let code = body["exit_code"].as_i64().unwrap_or(0);
    let signal = body["exit_signal"].as_i64().unwrap_or(0);
    let exit_code = if signal != 0 { 128 + signal } else { code };
    ExecOutput {
        exit_code: i32::try_from(exit_code).unwrap_or(-1),
        stdout: body["stdout"].as_str().unwrap_or_default().to_string(),
        stderr: body["stderr"].as_str().unwrap_or_default().to_string(),
    }
}

/// Every machine state Fly documents (the machine-states guide and the API's
/// description), mapped onto what the registry acts on. Transient states that
/// settle by themselves read as [`MachineState::Starting`] or
/// [`MachineState::Stopping`] by their direction; `replaced` and `migrated`
/// describe a superseded version, not the machine, so they too say "ask
/// again". [`MachineState::Gone`] here is the machine alone; what is left of
/// the box beside it is `Client::state`'s to say. A state Fly adds later is a
/// [`MachineState::Failed`] naming it, not a guess.
fn machine_state(fly: &str) -> MachineState {
    match fly {
        "started" => MachineState::Running,
        // `created` before its first start; `suspended` resumes on start;
        // `archived` is off its host and rebuilt on the next start.
        "created" | "stopped" | "suspended" | "archived" => MachineState::Stopped,
        "creating" | "starting" | "restarting" | "updating" | "replacing" | "pending"
        | "migrating" | "replaced" | "migrated" => MachineState::Starting,
        "stopping" | "suspending" => MachineState::Stopping,
        "destroying" | "destroyed" => MachineState::Gone,
        "failed" => MachineState::Failed("Fly.io reports it failed and could not start".into()),
        "launch_failed" => MachineState::Failed("Fly.io could not launch it".into()),
        other => MachineState::Failed(format!(
            "Fly.io reports it in a state Fletch does not know ({other:?})"
        )),
    }
}

#[cfg(test)]
#[path = "tests/fly.rs"]
mod tests;
