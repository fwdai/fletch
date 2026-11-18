//! The Fly.io Machines adapter: one Fly app per cloud box, holding exactly one
//! Machine.
//!
//! One app per machine because of how Fly routes: a shared IPv4 address carries
//! many apps' TLS traffic and Fly's proxy picks the app by the name the client
//! asks for, so `<app>.fly.dev:443` reaches this machine and no other, with no
//! port bookkeeping. Destroying a box is then one call — deleting its app —
//! which takes the machine and its addresses with it and cannot leak either,
//! and an orphan sweep is a listing of the apps named with [`APP_PREFIX`].
//!
//! The machine runs the cloud image with one service: Fly's proxy accepts TLS
//! on port 443 (its `tls` handler, with the `*.fly.dev` certificate) and
//! forwards the plaintext to the guest's sshd, so the engine reaches the guest
//! with ssh behind a TLS client ([`Endpoint::Tls`]). Fly never starts, stops or
//! restarts the machine by itself — no restart policy, no autostop, no
//! autostart — because the engine owns its lifecycle, and the machine's root
//! filesystem, where the workspace lives, is kept across a stop and a start.
//!
//! Every request has a bounded timeout. The Machines API rate-limits each
//! action per machine (one request a second, bursts of three); a 429 is retried
//! a few times with backoff, which is safe for every request because a
//! rate-limited request was not carried out. Nothing else is retried: creating
//! an app or a machine is not idempotent, and [`Client::create`] cleans up
//! after a failure instead.
//!
//! The API is documented at <https://docs.fly.io/machines/api/>, with its
//! OpenAPI description at <https://docs.fly.io/api/machines/openapi.json>.

use std::time::{Duration, Instant};

use rand::Rng;
use reqwest::header::{HeaderValue, AUTHORIZATION, RETRY_AFTER};
use reqwest::{Method, StatusCode};
use serde_json::{json, Map, Value};

use super::{Connection, Created, Endpoint, ExecOutput, Handle, Listed, MachineSpec, MachineState};
use crate::error::{Error, Result};

/// The Machines API's public address.
pub const API_BASE: &str = "https://api.machines.dev";

/// The start of every app name this integration creates, which is how
/// [`Client::list`] tells its apps from the account's others. Narrower than
/// `fletch-` so it does not catch an app a person named for themselves.
pub const APP_PREFIX: &str = "fletch-box-";

/// The machine metadata key marking a machine as a Fletch cloud box, valued
/// with the box's name: what a person sees in Fly's dashboard, and the check
/// after the app prefix that [`Client::list`] makes before calling a machine
/// ours.
const LABEL: &str = "fletch_box";

/// Fly app names are lowercase letters, digits and dashes, and one DNS label
/// long, since the app is reached at `<app>.fly.dev`.
const APP_NAME_MAX: usize = 63;

/// The random characters ending every app name. App names are global across
/// Fly, so a name must be unique without asking Fly first.
const SUFFIX_LEN: usize = 8;
const SUFFIX_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// The port Fly's proxy accepts a box's TLS on.
const TLS_PORT: u16 = 443;

/// `rootfs.persist` (and its deprecated twin, `guest.persist_rootfs`): Fly
/// documents `restart` as "kept across stops and restarts" and `always` as
/// "also kept across updates". `always`, because the workspace lives on the
/// root filesystem: the engine never updates a machine, but a person editing
/// it in Fly's dashboard or with flyctl would, and that must not wipe it.
const PERSIST_ROOTFS: &str = "always";

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

/// A state `start` or `stop` drives a machine to.
#[derive(Clone, Copy)]
enum Target {
    Started,
    Stopped,
}

impl Target {
    fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Stopped => "stopped",
        }
    }

    fn action(self) -> &'static str {
        match self {
            Self::Started => "start",
            Self::Stopped => "stop",
        }
    }

    fn reached(self, state: &MachineState) -> bool {
        match self {
            Self::Started => *state == MachineState::Running,
            // A failed machine is not running either.
            Self::Stopped => matches!(state, MachineState::Stopped | MachineState::Failed(_)),
        }
    }

    /// Reached, or on its way there with no further request.
    fn underway(self, state: &MachineState) -> bool {
        self.reached(state)
            || match self {
                Self::Started => *state == MachineState::Starting,
                Self::Stopped => *state == MachineState::Stopping,
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

    /// Create the box's app, give it its addresses, create its machine and wait
    /// for it to start; on any failure once the app may exist, delete the app
    /// before returning (see `cloud::Client::create`).
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
            Ok(machine) => {
                tracing::info!(app, machine, "created a Fly.io machine");
                Ok(Created {
                    endpoint: endpoint(&app),
                    handle: Handle::Fly { app, machine },
                })
            }
            Err(e) => Err(self.abandon(&app, e).await),
        }
    }

    /// Everything `create` does once the app exists; returns the machine id.
    async fn provision(&self, app: &str, spec: &MachineSpec) -> Result<String> {
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
        let body = machine_body(app, spec, self.region.as_deref());
        let reply = self
            .call(
                Method::POST,
                &format!("/v1/apps/{app}/machines"),
                &[],
                Some(&body),
                MACHINE_CREATE_TIMEOUT,
            )
            .await?;
        if !reply.status.is_success() {
            return Err(self.refusal(&reply, "create the machine"));
        }
        let machine = reply.body["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| self.error("Fly.io created a machine but did not say its id".into()))?
            .to_string();
        self.wait_for(app, &machine, Target::Started, None, CREATE_BOUND)
            .await?;
        Ok(machine)
    }

    /// Delete `app` after `create` failed partway, returning the error to
    /// report. If the delete fails too, the error says so and names the app.
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

    pub async fn state(&self, app: &str, machine: &str) -> Result<MachineState> {
        Ok(self
            .observe(app, machine)
            .await?
            .map_or(MachineState::Gone, |seen| seen.state))
    }

    pub async fn start(&self, app: &str, machine: &str) -> Result<()> {
        let seen = self
            .observe(app, machine)
            .await?
            .ok_or_else(|| gone(app, machine))?;
        match seen.state {
            MachineState::Running => return Ok(()),
            MachineState::Gone => return Err(gone(app, machine)),
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
            MachineState::Gone => return Err(gone(app, machine)),
            MachineState::Stopping => {}
            MachineState::Running | MachineState::Starting => {
                self.request(app, machine, Target::Stopped).await?;
            }
        }
        let version = seen.version.as_deref();
        self.wait_for(app, machine, Target::Stopped, version, STOP_BOUND)
            .await
    }

    /// Delete the box's app with everything in it (`force` stops a running
    /// machine first). Only ever an app named with [`APP_PREFIX`], whatever a
    /// handle says. Fly deletes asynchronously: accepted is done.
    pub async fn destroy(&self, app: &str) -> Result<()> {
        if !app.starts_with(APP_PREFIX) {
            return Err(Error::Other(format!(
                "{app} is not a Fletch cloud box's app; refusing to delete it"
            )));
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

    /// The org's apps named with [`APP_PREFIX`], each with its machine. A
    /// machine must also carry [`LABEL`] to be listed. An app with no machine
    /// and no volume — what a `create` whose cleanup failed leaves — is listed
    /// with an empty machine id as [`MachineState::Gone`]; `destroy` removes
    /// it.
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
            let reply = self
                .call(
                    Method::GET,
                    &format!("/v1/apps/{name}/machines"),
                    &[],
                    None,
                    HTTP_TIMEOUT,
                )
                .await?;
            if reply.status == StatusCode::NOT_FOUND {
                // Deleted since the listing.
                continue;
            }
            if !reply.status.is_success() {
                return Err(self.refusal(&reply, "list an app's machines"));
            }
            let machines = reply.body.as_array().map(Vec::as_slice).unwrap_or_default();
            if machines.is_empty() {
                // A box never has a volume; an app holding one is not ours.
                if app["volume_count"].as_u64().unwrap_or(0) == 0 {
                    listed.push(Listed {
                        handle: Handle::Fly {
                            app: name.to_string(),
                            machine: String::new(),
                        },
                        state: MachineState::Gone,
                    });
                }
                continue;
            }
            for machine in machines.iter().filter(|m| labelled(m)) {
                let Some(id) = machine["id"].as_str().filter(|id| !id.is_empty()) else {
                    continue;
                };
                listed.push(Listed {
                    handle: Handle::Fly {
                        app: name.to_string(),
                        machine: id.to_string(),
                    },
                    state: machine_state(machine["state"].as_str().unwrap_or_default()),
                });
            }
        }
        Ok(listed)
    }

    /// The machine as Fly reports it now; `None` when it does not exist.
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
        Ok(Some(Seen {
            state: machine_state(&raw),
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
            _ => Err(self.refusal(&reply, &format!("{} the machine", target.action()))),
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
                    return Err(gone(app, machine))
                }
                _ => {}
            }
            let seen = self
                .observe(app, machine)
                .await?
                .ok_or_else(|| gone(app, machine))?;
            if target.reached(&seen.state) {
                return Ok(());
            }
            match &seen.state {
                MachineState::Gone => return Err(gone(app, machine)),
                MachineState::Failed(reason) => {
                    return Err(self.error(format!(
                        "the Fly.io machine {machine} did not start: {reason}"
                    )))
                }
                _ => {}
            }
            if Instant::now() >= deadline {
                return Err(self.error(format!(
                    "the Fly.io machine {machine} was not {} after {} s (Fly.io reports it {})",
                    target.as_str(),
                    bound.as_secs(),
                    seen.raw,
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

/// Whether a listed machine carries [`LABEL`]. Its `config` is null when its
/// host could not be reached, with the last known one in `incomplete_config`.
fn labelled(machine: &Value) -> bool {
    ["config", "incomplete_config"]
        .iter()
        .any(|key| machine[*key]["metadata"].get(LABEL).is_some())
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

/// The create-machine request for a box: the image with its environment and
/// size, one TCP service taking TLS on 443 to the guest's sshd, nothing that
/// would start, stop or restart the machine behind the engine's back, the root
/// filesystem kept, and the box's label. Without a region Fly places the
/// machine in the region of the API server that takes the request — usually
/// the one nearest this machine.
fn machine_body(app: &str, spec: &MachineSpec, region: Option<&str>) -> Value {
    let env: Map<String, Value> = spec
        .env
        .iter()
        .map(|(k, v)| (k.clone(), Value::from(v.as_str())))
        .collect();
    let mut metadata = Map::new();
    metadata.insert(LABEL.into(), Value::from(spec.name.as_str()));
    let mut rootfs = json!({ "persist": PERSIST_ROOTFS });
    if let Some(gib) = spec.disk_gib {
        rootfs["size_gb"] = json!(gib);
    }
    let mut body = json!({
        "name": app,
        "config": {
            "image": spec.image,
            "env": env,
            "guest": {
                "cpu_kind": "shared",
                "cpus": spec.cpus,
                "memory_mb": spec.memory_mib,
                "persist_rootfs": PERSIST_ROOTFS,
            },
            "rootfs": rootfs,
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
    });
    if let Some(region) = region {
        body["region"] = json!(region);
    }
    body
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

/// Every state Fly documents (the machine-states guide and the API's
/// description), mapped onto what the registry acts on. Transient states that
/// settle by themselves read as [`MachineState::Starting`] or
/// [`MachineState::Stopping`] by their direction; `replaced` and `migrated`
/// describe a superseded version, not the machine, so they too say "ask
/// again". A state Fly adds later is a [`MachineState::Failed`] naming it, not
/// a guess.
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
