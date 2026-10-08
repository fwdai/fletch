//! The host owns every claude login. A sandboxed claude can't refresh its own
//! OAuth token (the refresh needs lock files and a Keychain write the sandbox
//! denies, and a container has no Keychain at all), and it must never hold the
//! long-lived refresh token anyway. So the app reads each account's stored
//! login, refreshes it when it is about to lapse, writes the rotated pair back
//! where claude keeps it, and hands the agent only the short-lived access
//! token as `CLAUDE_CODE_OAUTH_TOKEN`.
//!
//! Refresh tokens rotate: the one used is revoked by the answer. Two refreshes
//! of one account racing would leave one of them holding a dead token, so
//! every refresh of an account runs single-flight, and the new pair is written
//! back before anyone else may read the store. A refresh is only started when
//! the answer is sure to fit back into the store, and a rotated pair the store
//! refused anyway is kept in memory and written on the next call: it is then
//! the account's only valid refresh token.

use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::{AfterRotation, Creds, HostLogin, LoginProvider};
use crate::agent::accounts;

pub(crate) use super::Demand;

pub const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";

/// Claude Code's public OAuth client — the one its own `/login` registers the
/// login under, so it is the only client its refresh tokens are valid for.
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

const SCOPES: &str =
    "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";

/// A refresh runs ahead of a launch, so a slow sign-in server must cost the
/// launch seconds, not its whole spawn budget.
const TIMEOUT: Duration = Duration::from_secs(5);

/// How much a refresh may grow the stored blob: new tokens of a different
/// length, a `refreshTokenExpiresAt` that wasn't there. The fit check before a
/// refresh adds it, since the exact answer is only known once the old refresh
/// token is already spent.
const GROWTH_SLACK: usize = 256;

/// How close to expiry a stored access token is refreshed before a launch, and
/// how close a live agent's token may get before its next turn relaunches it.
/// A launched claude can't swap its token, so this is the runway every turn
/// starts with: long enough that ordinary turns finish on it, short enough
/// that a ~8h token is refreshed about once a working day, keeping the
/// rotations (and the chance of racing the user's own terminal `claude` on
/// the default login) rare. A turn that outlives it hits a 401, which the
/// supervisor answers with one refresh-and-relaunch.
pub const REFRESH_MARGIN_MS: i64 = 30 * 60 * 1000;

/// A claude OAuth access token and when it lapses. `Debug` prints the expiry
/// only.
#[derive(Clone)]
pub struct AccessToken {
    secret: String,
    expires_at_ms: i64,
}

impl AccessToken {
    pub fn secret(&self) -> &str {
        &self.secret
    }

    pub fn expires_at_ms(&self) -> i64 {
        self.expires_at_ms
    }

    #[cfg(test)]
    pub(crate) fn for_test(secret: &str, expires_at_ms: i64) -> Self {
        Self {
            secret: secret.to_string(),
            expires_at_ms,
        }
    }
}

impl fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccessToken")
            .field("expires_at_ms", &self.expires_at_ms)
            .finish_non_exhaustive()
    }
}

/// Whether a token lapsing at `expires_at_ms` is inside [`REFRESH_MARGIN_MS`]
/// (or past it) at `now_ms`.
pub fn needs_refresh(expires_at_ms: i64, now_ms: i64) -> bool {
    expires_at_ms - now_ms <= REFRESH_MARGIN_MS
}

pub(crate) fn now_ms() -> i64 {
    super::now_ms()
}

/// Why no token could be had. Every message is fixed text plus, for
/// `Unavailable`, a transport reason — never anything read from a credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginError {
    SignedOut,
    /// The refresh token was refused: the login was revoked, or its ~30-day
    /// lifetime ran out. Only a new sign-in helps.
    Revoked,
    /// The token is past its expiry and the refresh could not be completed
    /// (network, a server error, an unreadable store, a login too large to
    /// store back). The stored login is untouched, so a later attempt may
    /// still succeed.
    Unavailable(String),
}

impl fmt::Display for LoginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SignedOut => f.write_str(
                "This Claude account isn't signed in. Sign it in under Settings → Providers.",
            ),
            Self::Revoked => f.write_str(
                "This Claude login has expired or was revoked. Sign in again under Settings → Providers.",
            ),
            Self::Unavailable(reason) => write!(f, "Couldn't use this Claude login: {reason}"),
        }
    }
}

impl From<super::LoginError> for LoginError {
    fn from(e: super::LoginError) -> Self {
        match e {
            super::LoginError::SignedOut => Self::SignedOut,
            super::LoginError::Revoked => Self::Revoked,
            super::LoginError::Unavailable(reason) => Self::Unavailable(reason),
        }
    }
}

impl From<LoginError> for crate::error::Error {
    fn from(e: LoginError) -> Self {
        Self::Other(e.to_string())
    }
}

/// The token a launch of claude under the account in `account_dir` (`None` =
/// the default account) runs with, refreshed first when it is inside
/// [`REFRESH_MARGIN_MS`]. A refresh that can't be made or fails for a reason
/// other than a revoked login still yields the stored token while it is
/// unexpired.
pub async fn access_token_for_launch(
    account_dir: Option<&Path>,
) -> Result<AccessToken, LoginError> {
    let store = HostStore::for_account(account_dir).ok_or(LoginError::SignedOut)?;
    token_with(
        &store.service.clone(),
        Arc::new(store),
        Arc::new(HttpEndpoint::default()),
        Demand::Launch,
    )
    .await
}

/// A token to replace one the API rejected (a 401) although it was not yet
/// due: the store's newer token if someone already refreshed past
/// `rejected_expires_at_ms`, else a forced refresh.
pub async fn replace_rejected_token(
    account_dir: Option<&Path>,
    rejected_expires_at_ms: i64,
) -> Result<AccessToken, LoginError> {
    let store = HostStore::for_account(account_dir).ok_or(LoginError::SignedOut)?;
    token_with(
        &store.service.clone(),
        Arc::new(store),
        Arc::new(HttpEndpoint::default()),
        Demand::Replace {
            rejected_expires_at_ms,
        },
    )
    .await
}

/// The token a claude agent stamped with `account` launches with. A managed
/// account without a usable login fails the launch: running it under any
/// other credential would be running it as someone else. The default account
/// degrades to no token, leaving the stored setup-token and shell credentials
/// (container chain) or claude's own lookup to answer, as before.
/// `rejected_expires_at_ms` is set when the agent's last token drew a 401.
pub async fn launch_token(
    account: Option<&str>,
    rejected_expires_at_ms: Option<i64>,
) -> crate::error::Result<Option<AccessToken>> {
    let dir = accounts::existing_account_dir("claude", account)?;
    let token = match rejected_expires_at_ms {
        Some(rejected) => replace_rejected_token(dir.as_deref(), rejected).await,
        None => access_token_for_launch(dir.as_deref()).await,
    };
    match (token, dir) {
        (Ok(token), _) => Ok(Some(token)),
        (Err(e), Some(_)) => Err(e.into()),
        (Err(LoginError::SignedOut), None) => Ok(None),
        (Err(e), None) => {
            tracing::warn!(error = %e, "default claude login unusable; launching without a host token");
            Ok(None)
        }
    }
}

/// Whether a refresh of the account in `account_dir` was refused and nothing
/// has changed in its store since — Settings' "sign in again". Costs nothing
/// unless a refusal is on record: only then is the store's stamp (the
/// Keychain item's metadata, or the file's) read, and never the password.
pub fn is_revoked(account_dir: Option<&Path>) -> bool {
    let Some(store) = HostStore::for_account(account_dir) else {
        return false;
    };
    let key = store.service.clone();
    HostLogin::new(ClaudeProvider::new(
        &key,
        Arc::new(store),
        Arc::new(HttpEndpoint::default()),
    ))
    .is_revoked()
}

/// The access token in a stored credential blob (`.credentials.json` or the
/// Keychain password, which share the shape), when it holds a login at all: a
/// non-empty token with `expiresAt > 0`. A macOS login leaves an `expiresAt: 0`
/// placeholder file on disk next to the real Keychain item; counting it would
/// launch an agent into a login prompt it can't answer. An expired token still
/// counts: the host refreshes it before any launch.
pub(crate) fn stored_access_token(contents: &[u8]) -> Option<String> {
    let json: Value = serde_json::from_slice(contents).ok()?;
    parse_pair(&json).map(|pair| pair.access)
}

// ── core ────────────────────────────────────────────────────────────────────

struct Pair {
    access: String,
    refresh: Option<String>,
    expires_at_ms: i64,
}

fn parse_pair(json: &Value) -> Option<Pair> {
    let oauth = json.get("claudeAiOauth")?;
    let access = oauth
        .get("accessToken")?
        .as_str()
        .map(str::trim)
        .filter(|t| !t.is_empty())?
        .to_string();
    let expires_at_ms = oauth.get("expiresAt")?.as_i64().filter(|e| *e > 0)?;
    let refresh = oauth
        .get("refreshToken")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    Some(Pair {
        access,
        refresh,
        expires_at_ms,
    })
}

/// Fold a refresh answer into the stored blob. Only the fields the refresh
/// owns change; everything else — scopes, plan, MCP logins, fields a newer
/// claude added — is written back exactly as read.
fn apply_grant(json: &mut Value, grant: &TokenGrant, now_ms: i64) -> Option<i64> {
    let oauth = json.get_mut("claudeAiOauth")?.as_object_mut()?;
    let expires_at_ms = now_ms + grant.expires_in_s * 1000;
    oauth.insert("accessToken".into(), grant.access_token.clone().into());
    oauth.insert("expiresAt".into(), expires_at_ms.into());
    if let Some(refresh) = &grant.refresh_token {
        oauth.insert("refreshToken".into(), refresh.clone().into());
    }
    if let Some(lifetime) = grant.refresh_token_expires_in_s {
        oauth.insert(
            "refreshTokenExpiresAt".into(),
            (now_ms + lifetime * 1000).into(),
        );
    }
    Some(expires_at_ms)
}

/// Where a login was read from, and so where it is written back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Place {
    /// The Keychain item, owned by `account`.
    Keychain {
        account: String,
    },
    File,
}

#[derive(Clone)]
pub(crate) struct Stored {
    json: String,
    place: Place,
}

/// One account's credential store. Blocking; the core calls it off the async
/// workers.
pub(crate) trait LoginStore: Send + Sync + 'static {
    /// A value that changes whenever the stored login does, without reading
    /// the secret. `None` = nothing stored.
    fn stamp(&self) -> Option<String>;
    fn load(&self) -> Result<Option<Stored>, String>;
    /// Whether a blob of `len` bytes can be written back to `place` whole.
    fn fits(&self, place: &Place, len: usize) -> bool;
    fn save(&self, place: &Place, json: &str) -> Result<(), String>;
}

pub(crate) struct TokenGrant {
    access_token: String,
    refresh_token: Option<String>,
    expires_in_s: i64,
    refresh_token_expires_in_s: Option<i64>,
    /// Whether the request that earned it carried the `scope` field; false
    /// once the server refused the field and the refresh went without it.
    scoped: bool,
}

impl fmt::Debug for TokenGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenGrant")
            .field("expires_in_s", &self.expires_in_s)
            .field("rotated", &self.refresh_token.is_some())
            .field("scoped", &self.scoped)
            .finish_non_exhaustive()
    }
}

fn parse_grant(body: &Value) -> Option<TokenGrant> {
    Some(TokenGrant {
        access_token: body
            .get("access_token")?
            .as_str()
            .filter(|t| !t.is_empty())?
            .to_string(),
        refresh_token: body
            .get("refresh_token")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map(str::to_string),
        expires_in_s: body.get("expires_in")?.as_i64().filter(|s| *s > 0)?,
        refresh_token_expires_in_s: body
            .get("refresh_token_expires_in")
            .and_then(Value::as_i64)
            .filter(|s| *s > 0),
        scoped: true,
    })
}

pub(crate) use super::RefreshFailure;

pub(crate) type RefreshFuture<'a> =
    Pin<Box<dyn Future<Output = Result<TokenGrant, RefreshFailure>> + Send + 'a>>;

pub(crate) trait TokenEndpoint: Send + Sync + 'static {
    fn refresh<'a>(&'a self, refresh_token: &'a str) -> RefreshFuture<'a>;
}

/// A claude login on the engine: its store and its sign-in server.
struct ClaudeProvider {
    key: String,
    store: Arc<dyn LoginStore>,
    endpoint: Arc<dyn TokenEndpoint>,
    /// Drives the async endpoint from the engine's blocking thread.
    runtime: Option<tokio::runtime::Handle>,
}

impl ClaudeProvider {
    fn new(key: &str, store: Arc<dyn LoginStore>, endpoint: Arc<dyn TokenEndpoint>) -> Self {
        Self {
            key: key.to_string(),
            store,
            endpoint,
            runtime: tokio::runtime::Handle::try_current().ok(),
        }
    }
}

impl LoginProvider for ClaudeProvider {
    type Place = Place;
    type Grant = TokenGrant;
    type Launch = AccessToken;
    const PROVIDER: &'static str = "claude";
    const AFTER_ROTATION: AfterRotation = AfterRotation::UseIfUnexpired;

    fn key(&self) -> String {
        self.key.clone()
    }

    fn stamp(&self) -> Option<String> {
        self.store.stamp()
    }

    fn load(&self) -> Result<Option<(String, Place)>, String> {
        Ok(self.store.load()?.map(|stored| (stored.json, stored.place)))
    }

    fn parse(&self, json: &Value) -> Option<Creds> {
        let pair = parse_pair(json)?;
        Some(Creds {
            refresh: pair.refresh,
            expires_at_ms: Some(pair.expires_at_ms),
        })
    }

    fn due(&self, creds: &Creds, _json: &Value, now_ms: i64) -> bool {
        creds
            .expires_at_ms
            .map_or(true, |expires| needs_refresh(expires, now_ms))
    }

    /// `security -i` takes one line of about 4 KB, and a refresh may grow
    /// the login by up to [`GROWTH_SLACK`].
    fn fits(&self, place: &Place, len: usize) -> bool {
        self.store.fits(place, len + GROWTH_SLACK)
    }

    fn refresh(&self, refresh_token: &str) -> Result<TokenGrant, RefreshFailure> {
        let runtime = self.runtime.as_ref().ok_or_else(|| {
            RefreshFailure::Failed("no runtime to reach the sign-in server".into())
        })?;
        let grant = runtime.block_on(self.endpoint.refresh(refresh_token));
        if let Ok(grant) = &grant {
            tracing::info!(
                rotated = grant.refresh_token.is_some(),
                scoped = grant.scoped,
                "claude login refreshed"
            );
        }
        grant
    }

    fn apply(&self, json: &mut Value, grant: &TokenGrant, now_ms: i64) -> bool {
        apply_grant(json, grant, now_ms).is_some()
    }

    fn save(&self, place: &Place, json: &Value) -> Result<(), String> {
        self.store.save(place, &json.to_string())
    }

    fn launch(&self, json: &Value, creds: &Creds) -> AccessToken {
        AccessToken {
            secret: parse_pair(json).map(|pair| pair.access).unwrap_or_default(),
            expires_at_ms: creds.expires_at_ms.unwrap_or_default(),
        }
    }

    fn mark_of(&self, stamp: Option<&str>, _json: &Value) -> Option<String> {
        stamp.map(str::to_string)
    }

    fn current_mark(&self) -> Option<String> {
        self.store.stamp()
    }
}

/// The engine's flow for the login `store` holds, on a blocking task.
async fn token_with(
    key: &str,
    store: Arc<dyn LoginStore>,
    endpoint: Arc<dyn TokenEndpoint>,
    demand: Demand,
) -> Result<AccessToken, LoginError> {
    let login = HostLogin::new(ClaudeProvider::new(key, store, endpoint));
    tokio::task::spawn_blocking(move || login.credential(demand))
        .await
        .map_err(|e| LoginError::Unavailable(format!("credential store task failed: {e}")))?
        .map_err(LoginError::from)
}

// ── the real store and endpoint ─────────────────────────────────────────────

/// Where claude keeps a config dir's login: the Keychain item named for the
/// dir, else `<dir>/.credentials.json` (claude's fallback when it can't write
/// the Keychain, and the only store off macOS).
struct HostStore {
    service: String,
    file: PathBuf,
}

impl HostStore {
    fn for_account(account_dir: Option<&Path>) -> Option<Self> {
        let dir = match account_dir {
            Some(dir) => dir.to_path_buf(),
            None => accounts::shared_source_dir("claude", &dirs::home_dir()?),
        };
        Some(Self {
            service: crate::sandbox::container::auth::claude_keychain_service(account_dir),
            file: dir.join(".credentials.json"),
        })
    }

    fn file_stamp(&self) -> Option<String> {
        let meta = std::fs::metadata(&self.file).ok()?;
        let modified = meta
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?;
        Some(format!("file:{}:{}", modified.as_nanos(), meta.len()))
    }

    fn file_login(&self) -> Option<String> {
        let text = std::fs::read_to_string(&self.file).ok()?;
        stored_access_token(text.as_bytes()).map(|_| text)
    }
}

impl LoginStore for HostStore {
    fn stamp(&self) -> Option<String> {
        match crate::keychain::item_stamp(&self.service) {
            Some(item) => Some(format!("keychain:{}", item.modified)),
            None => self.file_login().and_then(|_| self.file_stamp()),
        }
    }

    fn load(&self) -> Result<Option<Stored>, String> {
        if let Some(item) = crate::keychain::item_stamp(&self.service) {
            match crate::keychain::read_password(&self.service) {
                Some(json) if stored_access_token(json.as_bytes()).is_some() => {
                    let account = item
                        .account
                        .or_else(|| std::env::var("USER").ok())
                        .ok_or_else(|| "no Keychain account name for the login".to_string())?;
                    return Ok(Some(Stored {
                        json,
                        place: Place::Keychain { account },
                    }));
                }
                // An item without a claude login (MCP logins only) defers to
                // the file, as claude itself does.
                Some(_) => {}
                None => {
                    return Err(
                        "the Keychain item couldn't be read (locked, or access was denied)".into(),
                    )
                }
            }
        }
        Ok(self.file_login().map(|json| Stored {
            json,
            place: Place::File,
        }))
    }

    fn fits(&self, place: &Place, len: usize) -> bool {
        match place {
            Place::Keychain { account } => {
                crate::keychain::password_fits(&self.service, account, len)
            }
            Place::File => true,
        }
    }

    fn save(&self, place: &Place, json: &str) -> Result<(), String> {
        match place {
            Place::Keychain { account } => {
                crate::keychain::write_password(&self.service, account, json)
            }
            Place::File => {
                crate::agent::credential_file::write_private_file(&self.file, json.as_bytes())
                    .map_err(|e| e.to_string())
            }
        }
    }
}

struct HttpEndpoint {
    url: String,
}

impl Default for HttpEndpoint {
    fn default() -> Self {
        Self {
            url: TOKEN_URL.to_string(),
        }
    }
}

impl HttpEndpoint {
    async fn post(
        &self,
        client: &reqwest::Client,
        refresh_token: &str,
        with_scope: bool,
    ) -> Result<(u16, Option<Value>), RefreshFailure> {
        let mut body = serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": CLIENT_ID,
        });
        if with_scope {
            body["scope"] = SCOPES.into();
        }
        let response = client
            .post(&self.url)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                RefreshFailure::Failed(format!("the sign-in server is unreachable ({e})"))
            })?;
        let status = response.status().as_u16();
        Ok((status, response.json::<Value>().await.ok()))
    }
}

impl TokenEndpoint for HttpEndpoint {
    fn refresh<'a>(&'a self, refresh_token: &'a str) -> RefreshFuture<'a> {
        Box::pin(async move {
            let client = reqwest::Client::builder()
                .user_agent("Fletch")
                .timeout(TIMEOUT)
                .build()
                .map_err(|e| RefreshFailure::Failed(format!("http client: {e}")))?;
            let mut answer = self.post(&client, refresh_token, true).await?;
            let mut scoped = true;
            if answer.0 == 400 && oauth_error(answer.1.as_ref()) == Some("invalid_scope") {
                scoped = false;
                answer = self.post(&client, refresh_token, false).await?;
            }
            let mut grant = interpret_refresh(answer.0, answer.1.as_ref())?;
            grant.scoped = scoped;
            Ok(grant)
        })
    }
}

fn oauth_error(body: Option<&Value>) -> Option<&str> {
    body?.get("error")?.as_str()
}

/// A 400 or 401 refuses the refresh token itself (`invalid_grant`: revoked,
/// expired, or already spent by a rotation); anything else leaves the stored
/// pair valid for a later try. Never carries the body, which may echo
/// credentials.
fn interpret_refresh(status: u16, body: Option<&Value>) -> Result<TokenGrant, RefreshFailure> {
    match status {
        200..=299 => body.and_then(parse_grant).ok_or_else(|| {
            RefreshFailure::Failed("the sign-in server answered without a token".into())
        }),
        400 | 401 => Err(RefreshFailure::Rejected),
        other => Err(RefreshFailure::Failed(format!(
            "the sign-in server answered {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Whether the engine has a refusal on record for `key` that `store`'s
    /// login still matches.
    fn revoked_at(key: &str, store: &Arc<MemStore>) -> bool {
        HostLogin::new(ClaudeProvider::new(
            key,
            store.clone(),
            Arc::new(MockEndpoint::new(Answer::Down)),
        ))
        .is_revoked()
    }

    fn is_kept(key: &str) -> bool {
        super::super::is_kept(&format!("claude:{key}"))
    }

    const HOUR_MS: i64 = 60 * 60 * 1000;

    #[derive(Default)]
    struct MemStore {
        json: Mutex<Option<String>>,
        version: AtomicUsize,
        saves: AtomicUsize,
        /// Stands in for a Keychain item owned by this account; `None` = a
        /// credentials file.
        keychain_account: Option<String>,
        fail_saves: std::sync::atomic::AtomicBool,
    }

    impl MemStore {
        fn holding(json: Value) -> Arc<Self> {
            Arc::new(Self {
                json: Mutex::new(Some(json.to_string())),
                ..Default::default()
            })
        }

        fn in_keychain(json: Value) -> Arc<Self> {
            Arc::new(Self {
                json: Mutex::new(Some(json.to_string())),
                keychain_account: Some("alex".into()),
                ..Default::default()
            })
        }

        fn fail_saves(&self, fail: bool) {
            self.fail_saves.store(fail, Ordering::SeqCst);
        }

        fn current(&self) -> Value {
            serde_json::from_str(self.json.lock().as_deref().unwrap()).unwrap()
        }

        fn replace(&self, json: Value) {
            *self.json.lock() = Some(json.to_string());
            self.version.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl LoginStore for MemStore {
        fn stamp(&self) -> Option<String> {
            self.json
                .lock()
                .as_ref()
                .map(|_| self.version.load(Ordering::SeqCst).to_string())
        }

        fn load(&self) -> Result<Option<Stored>, String> {
            let place = match &self.keychain_account {
                Some(account) => Place::Keychain {
                    account: account.clone(),
                },
                None => Place::File,
            };
            Ok(self.json.lock().clone().map(|json| Stored { json, place }))
        }

        fn fits(&self, place: &Place, len: usize) -> bool {
            match place {
                Place::Keychain { account } => {
                    crate::keychain::password_fits("Claude Code-credentials-e6d7ed77", account, len)
                }
                Place::File => true,
            }
        }

        fn save(&self, _place: &Place, json: &str) -> Result<(), String> {
            self.saves.fetch_add(1, Ordering::SeqCst);
            if self.fail_saves.load(Ordering::SeqCst) {
                return Err("the store refused the write".into());
            }
            self.replace(serde_json::from_str(json).unwrap());
            Ok(())
        }
    }

    enum Answer {
        Grant(Value),
        Rejected,
        Down,
    }

    struct MockEndpoint {
        answer: Answer,
        calls: AtomicUsize,
        seen: Mutex<Vec<String>>,
        delay: Duration,
        /// Runs while the request is out, standing in for another process.
        meanwhile: Option<Box<dyn Fn() + Send + Sync>>,
    }

    impl MockEndpoint {
        fn new(answer: Answer) -> Self {
            Self {
                answer,
                calls: AtomicUsize::new(0),
                seen: Mutex::new(Vec::new()),
                delay: Duration::ZERO,
                meanwhile: None,
            }
        }
    }

    impl TokenEndpoint for MockEndpoint {
        fn refresh<'a>(&'a self, refresh_token: &'a str) -> RefreshFuture<'a> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.seen.lock().push(refresh_token.to_string());
                tokio::time::sleep(self.delay).await;
                if let Some(f) = &self.meanwhile {
                    f();
                }
                match &self.answer {
                    Answer::Grant(body) => interpret_refresh(200, Some(body)),
                    Answer::Rejected => {
                        interpret_refresh(400, Some(&json!({"error": "invalid_grant"})))
                    }
                    Answer::Down => Err(RefreshFailure::Failed("unreachable".into())),
                }
            })
        }
    }

    fn login(access: &str, refresh: &str, expires_at_ms: i64) -> Value {
        json!({
            "claudeAiOauth": {
                "accessToken": access,
                "refreshToken": refresh,
                "expiresAt": expires_at_ms,
                "refreshTokenExpiresAt": expires_at_ms + 700 * HOUR_MS,
                "scopes": ["user:inference", "user:profile"],
                "subscriptionType": "team",
                "rateLimitTier": "default_claude_max_5x",
                "futureField": {"kept": true},
            },
            "mcpOAuth": {"server": {"accessToken": "mcp-secret"}},
        })
    }

    fn grant(access: &str, refresh: Option<&str>) -> Value {
        let mut body = json!({
            "token_type": "Bearer",
            "access_token": access,
            "expires_in": 28800,
            "scope": "user:inference user:profile",
        });
        if let Some(refresh) = refresh {
            body["refresh_token"] = refresh.into();
        }
        body
    }

    fn key(name: &str) -> String {
        format!("test-{name}-{}", uuid::Uuid::new_v4())
    }

    async fn launch(
        key: &str,
        store: &Arc<MemStore>,
        endpoint: &Arc<MockEndpoint>,
    ) -> Result<AccessToken, LoginError> {
        token_with(key, store.clone(), endpoint.clone(), Demand::Launch).await
    }

    #[tokio::test]
    async fn a_fresh_token_is_used_without_a_refresh() {
        let store = MemStore::holding(login("a1", "r1", now_ms() + 4 * HOUR_MS));
        let endpoint = Arc::new(MockEndpoint::new(Answer::Down));
        let token = launch(&key("fresh"), &store, &endpoint).await.unwrap();
        assert_eq!(token.secret(), "a1");
        assert_eq!(endpoint.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_token_inside_the_margin_is_refreshed_and_the_rotated_pair_stored() {
        let store = MemStore::holding(login("a1", "r1", now_ms() + 10 * 60 * 1000));
        let endpoint = Arc::new(MockEndpoint::new(Answer::Grant(grant("a2", Some("r2")))));
        let token = launch(&key("rotate"), &store, &endpoint).await.unwrap();
        assert_eq!(token.secret(), "a2");
        assert!(token.expires_at_ms() > now_ms() + 7 * HOUR_MS);
        assert_eq!(endpoint.seen.lock().as_slice(), ["r1".to_string()]);
        let oauth = &store.current()["claudeAiOauth"];
        assert_eq!(oauth["accessToken"], "a2");
        assert_eq!(oauth["refreshToken"], "r2");
        assert_eq!(oauth["expiresAt"], token.expires_at_ms());
    }

    #[tokio::test]
    async fn a_refresh_preserves_every_field_it_does_not_own() {
        let before = login("a1", "r1", now_ms() - HOUR_MS);
        let store = MemStore::holding(before.clone());
        let endpoint = Arc::new(MockEndpoint::new(Answer::Grant(grant("a2", Some("r2")))));
        launch(&key("preserve"), &store, &endpoint).await.unwrap();
        let after = store.current();
        assert_eq!(after["mcpOAuth"], before["mcpOAuth"]);
        for field in [
            "scopes",
            "subscriptionType",
            "rateLimitTier",
            "futureField",
            "refreshTokenExpiresAt",
        ] {
            assert_eq!(
                after["claudeAiOauth"][field], before["claudeAiOauth"][field],
                "{field}"
            );
        }
    }

    #[tokio::test]
    async fn a_refresh_without_rotation_keeps_the_stored_refresh_token() {
        let store = MemStore::holding(login("a1", "r1", now_ms() - HOUR_MS));
        let endpoint = Arc::new(MockEndpoint::new(Answer::Grant(grant("a2", None))));
        launch(&key("no-rotation"), &store, &endpoint)
            .await
            .unwrap();
        assert_eq!(store.current()["claudeAiOauth"]["refreshToken"], "r1");
    }

    #[tokio::test]
    async fn a_refused_refresh_reads_as_revoked_and_sticks_until_the_login_changes() {
        let k = key("revoked");
        let store = MemStore::holding(login("a1", "r1", now_ms() - HOUR_MS));
        let endpoint = Arc::new(MockEndpoint::new(Answer::Rejected));
        assert_eq!(
            launch(&k, &store, &endpoint).await.unwrap_err(),
            LoginError::Revoked
        );
        assert!(revoked_at(&k, &store));
        // A second launch doesn't spend another request on a known-dead token.
        assert_eq!(
            launch(&k, &store, &endpoint).await.unwrap_err(),
            LoginError::Revoked
        );
        assert_eq!(endpoint.calls.load(Ordering::SeqCst), 1);
        // A new sign-in clears it.
        store.replace(login("a9", "r9", now_ms() + 5 * HOUR_MS));
        assert_eq!(launch(&k, &store, &endpoint).await.unwrap().secret(), "a9");
    }

    #[tokio::test]
    async fn a_refusal_after_another_process_rotated_the_pair_uses_its_pair() {
        let store = MemStore::holding(login("a1", "r1", now_ms() - HOUR_MS));
        let mut endpoint = MockEndpoint::new(Answer::Rejected);
        let other = store.clone();
        endpoint.meanwhile = Some(Box::new(move || {
            other.replace(login("a5", "r5", now_ms() + 8 * HOUR_MS));
        }));
        let endpoint = Arc::new(endpoint);
        let token = launch(&key("raced"), &store, &endpoint).await.unwrap();
        assert_eq!(token.secret(), "a5");
    }

    #[tokio::test]
    async fn a_network_error_leaves_the_stored_pair_untouched() {
        let before = login("a1", "r1", now_ms() - HOUR_MS);
        let store = MemStore::holding(before.clone());
        let endpoint = Arc::new(MockEndpoint::new(Answer::Down));
        let err = launch(&key("down"), &store, &endpoint).await.unwrap_err();
        assert!(matches!(err, LoginError::Unavailable(_)), "{err:?}");
        assert_eq!(store.current(), before);
        assert_eq!(store.saves.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_network_error_still_launches_on_an_unexpired_token() {
        let store = MemStore::holding(login("a1", "r1", now_ms() + 5 * 60 * 1000));
        let endpoint = Arc::new(MockEndpoint::new(Answer::Down));
        let token = launch(&key("down-unexpired"), &store, &endpoint)
            .await
            .unwrap();
        assert_eq!(token.secret(), "a1");
    }

    #[tokio::test]
    async fn concurrent_launches_refresh_once() {
        let k = key("single-flight");
        let store = MemStore::holding(login("a1", "r1", now_ms() - HOUR_MS));
        let mut endpoint = MockEndpoint::new(Answer::Grant(grant("a2", Some("r2"))));
        endpoint.delay = Duration::from_millis(50);
        let endpoint = Arc::new(endpoint);
        let tasks: Vec<_> = (0..4)
            .map(|_| {
                let (k, store, endpoint) = (k.clone(), store.clone(), endpoint.clone());
                tokio::spawn(async move {
                    token_with(&k, store, endpoint, Demand::Launch)
                        .await
                        .unwrap()
                        .secret()
                        .to_string()
                })
            })
            .collect();
        for task in tasks {
            assert_eq!(task.await.unwrap(), "a2");
        }
        assert_eq!(endpoint.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn replacing_a_rejected_token_forces_a_refresh_of_a_fresh_looking_one() {
        let expires = now_ms() + 5 * HOUR_MS;
        let store = MemStore::holding(login("a1", "r1", expires));
        let endpoint = Arc::new(MockEndpoint::new(Answer::Grant(grant("a2", Some("r2")))));
        let demand = Demand::Replace {
            rejected_expires_at_ms: expires,
        };
        let token = token_with(&key("replace"), store.clone(), endpoint.clone(), demand)
            .await
            .unwrap();
        assert_eq!(token.secret(), "a2");
    }

    #[tokio::test]
    async fn replacing_a_rejected_token_takes_a_newer_stored_one_without_a_refresh() {
        let store = MemStore::holding(login("a2", "r2", now_ms() + 8 * HOUR_MS));
        let endpoint = Arc::new(MockEndpoint::new(Answer::Down));
        let demand = Demand::Replace {
            rejected_expires_at_ms: now_ms() + HOUR_MS,
        };
        let token = token_with(
            &key("replace-newer"),
            store.clone(),
            endpoint.clone(),
            demand,
        )
        .await
        .unwrap();
        assert_eq!(token.secret(), "a2");
        assert_eq!(endpoint.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn nothing_stored_is_signed_out() {
        let store = Arc::new(MemStore::default());
        let endpoint = Arc::new(MockEndpoint::new(Answer::Down));
        let k = key("empty");
        assert_eq!(
            launch(&k, &store, &endpoint).await.unwrap_err(),
            LoginError::SignedOut
        );
        assert!(!revoked_at(&k, &store));
    }

    /// The old refresh token is spent once the server answers, so a rotated
    /// pair the store won't take is kept and handed out, not dropped.
    #[tokio::test]
    async fn a_refreshed_login_the_store_refuses_is_kept_and_reused() {
        let k = key("unsaved");
        let store = MemStore::holding(login("a1", "r1", now_ms() - HOUR_MS));
        store.fail_saves(true);
        let endpoint = Arc::new(MockEndpoint::new(Answer::Grant(grant("a2", Some("r2")))));
        assert_eq!(launch(&k, &store, &endpoint).await.unwrap().secret(), "a2");
        assert_eq!(store.current()["claudeAiOauth"]["refreshToken"], "r1");
        assert_eq!(launch(&k, &store, &endpoint).await.unwrap().secret(), "a2");
        assert_eq!(
            endpoint.calls.load(Ordering::SeqCst),
            1,
            "r1 is never sent twice"
        );
    }

    /// A sign-in that lands between the refused write and its retry is the
    /// newer login: the pending pair is dropped, never written over it.
    #[tokio::test]
    async fn a_sign_in_after_a_refused_write_wins_over_the_kept_login() {
        let k = key("unsaved-signed-in");
        let store = MemStore::holding(login("a1", "r1", now_ms() - HOUR_MS));
        store.fail_saves(true);
        let endpoint = Arc::new(MockEndpoint::new(Answer::Grant(grant("a2", Some("r2")))));
        launch(&k, &store, &endpoint).await.unwrap();
        store.fail_saves(false);
        store.replace(login("a9", "r9", now_ms() + 5 * HOUR_MS));

        assert_eq!(launch(&k, &store, &endpoint).await.unwrap().secret(), "a9");
        assert_eq!(store.current()["claudeAiOauth"]["refreshToken"], "r9");
        assert_eq!(store.saves.load(Ordering::SeqCst), 1, "no retry write");
        assert!(!is_kept(&k));
    }

    #[tokio::test]
    async fn a_kept_login_is_written_once_the_store_takes_it() {
        let k = key("unsaved-retry");
        let store = MemStore::holding(login("a1", "r1", now_ms() - HOUR_MS));
        store.fail_saves(true);
        let endpoint = Arc::new(MockEndpoint::new(Answer::Grant(grant("a2", Some("r2")))));
        launch(&k, &store, &endpoint).await.unwrap();
        store.fail_saves(false);
        assert_eq!(launch(&k, &store, &endpoint).await.unwrap().secret(), "a2");
        assert_eq!(store.current()["claudeAiOauth"]["refreshToken"], "r2");
        assert!(!is_kept(&k));
    }

    fn oversized(access: &str, expires_at_ms: i64) -> Value {
        let mut blob = login(access, "r1", expires_at_ms);
        blob["mcpOAuth"]["big"] = json!({"accessToken": "m".repeat(4500)});
        blob
    }

    /// A Keychain login over 4 KB can't be written back through `security
    /// -i`, so its refresh token is never spent: the launch runs on the
    /// stored token while it lasts.
    #[tokio::test]
    async fn a_login_too_large_to_store_back_is_not_refreshed() {
        let store = MemStore::in_keychain(oversized("a1", now_ms() + 10 * 60 * 1000));
        let endpoint = Arc::new(MockEndpoint::new(Answer::Grant(grant("a2", Some("r2")))));
        let token = launch(&key("too-big"), &store, &endpoint).await.unwrap();
        assert_eq!(token.secret(), "a1");
        assert_eq!(endpoint.calls.load(Ordering::SeqCst), 0);
        assert_eq!(store.saves.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn an_expired_login_too_large_to_store_back_says_why() {
        let store = MemStore::in_keychain(oversized("a1", now_ms() - HOUR_MS));
        let endpoint = Arc::new(MockEndpoint::new(Answer::Grant(grant("a2", Some("r2")))));
        let err = launch(&key("too-big-expired"), &store, &endpoint)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("too large"), "{err}");
        assert_eq!(endpoint.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_keychain_login_with_room_is_refreshed() {
        let store = MemStore::in_keychain(login("a1", "r1", now_ms() - HOUR_MS));
        let endpoint = Arc::new(MockEndpoint::new(Answer::Grant(grant("a2", Some("r2")))));
        let token = launch(&key("keychain-room"), &store, &endpoint)
            .await
            .unwrap();
        assert_eq!(token.secret(), "a2");
    }

    #[test]
    fn the_margin_is_inclusive_and_covers_expired_tokens() {
        let now = 1_000_000_000;
        assert!(!needs_refresh(now + REFRESH_MARGIN_MS + 1, now));
        assert!(needs_refresh(now + REFRESH_MARGIN_MS, now));
        assert!(needs_refresh(now - 1, now));
    }

    #[test]
    fn a_placeholder_login_holds_no_token() {
        let placeholder = br#"{"claudeAiOauth":{"accessToken":"x","expiresAt":0}}"#;
        assert_eq!(stored_access_token(placeholder), None);
        assert_eq!(stored_access_token(b"not json"), None);
        let real = login("a1", "r1", 5).to_string();
        assert_eq!(stored_access_token(real.as_bytes()).as_deref(), Some("a1"));
    }

    #[test]
    fn refresh_answers_map_to_rejected_failed_or_a_grant() {
        assert!(matches!(
            interpret_refresh(400, Some(&json!({"error": "invalid_grant"}))),
            Err(RefreshFailure::Rejected)
        ));
        assert!(matches!(
            interpret_refresh(401, None),
            Err(RefreshFailure::Rejected)
        ));
        assert!(matches!(
            interpret_refresh(503, None),
            Err(RefreshFailure::Failed(_))
        ));
        assert!(matches!(
            interpret_refresh(200, Some(&json!({}))),
            Err(RefreshFailure::Failed(_))
        ));
        let ok = interpret_refresh(200, Some(&grant("a2", Some("r2")))).unwrap();
        assert_eq!(ok.expires_in_s, 28800);
    }

    #[test]
    fn debug_output_never_carries_a_token() {
        let token = AccessToken {
            secret: "sk-ant-oat-secret".into(),
            expires_at_ms: 7,
        };
        let printed = format!("{token:?}");
        assert!(!printed.contains("secret"), "{printed}");
        let grant = parse_grant(&grant("sk-ant-oat-secret", Some("sk-ant-ort-secret"))).unwrap();
        let printed = format!("{grant:?}");
        assert!(!printed.contains("sk-ant"), "{printed}");
    }

    #[test]
    fn a_credentials_file_is_replaced_whole_and_private() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join(".credentials.json");
        std::fs::write(&path, "old").unwrap();
        crate::agent::credential_file::write_private_file(&path, b"{\"new\":true}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"new\":true}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_eq!(std::fs::read_dir(td.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_file_login_is_loaded_and_written_back_to_the_file() {
        let td = tempfile::tempdir().unwrap();
        let store = HostStore {
            service: format!("fletch-test-absent-{}", uuid::Uuid::new_v4()),
            file: td.path().join(".credentials.json"),
        };
        assert_eq!(store.stamp(), None);
        let blob = login("a1", "r1", 5).to_string();
        std::fs::write(&store.file, &blob).unwrap();
        let before = store.stamp().expect("a file login has a stamp");
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(
            (loaded.json.as_str(), &loaded.place),
            (blob.as_str(), &Place::File)
        );
        store
            .save(&Place::File, &login("a2-longer", "r2", 6).to_string())
            .unwrap();
        assert_ne!(store.stamp().unwrap(), before);
        let saved: Value =
            serde_json::from_str(&std::fs::read_to_string(&store.file).unwrap()).unwrap();
        assert_eq!(saved["claudeAiOauth"]["accessToken"], "a2-longer");
    }

    /// Checks against a real login on this Mac. Ignored: they spend a real
    /// refresh and a real model turn. Name the managed account to use with
    /// `FLETCH_LIVE_CLAUDE_ACCOUNT`; nothing secret is printed, only expiries,
    /// stamps and short digests.
    #[cfg(target_os = "macos")]
    mod live {
        use super::*;

        fn account_dir() -> PathBuf {
            let id = std::env::var("FLETCH_LIVE_CLAUDE_ACCOUNT")
                .expect("set FLETCH_LIVE_CLAUDE_ACCOUNT to a managed claude account id");
            dirs::home_dir()
                .unwrap()
                .join(".fletch/accounts/claude")
                .join(id)
        }

        fn digest(secret: &str) -> String {
            use sha2::{Digest, Sha256};
            Sha256::digest(secret.as_bytes())
                .iter()
                .take(4)
                .map(|b| format!("{b:02x}"))
                .collect()
        }

        fn stored_pair(store: &HostStore) -> Pair {
            let text = store
                .load()
                .unwrap()
                .expect("the account is signed in")
                .json;
            parse_pair(&serde_json::from_str(&text).unwrap()).unwrap()
        }

        struct Watching {
            inner: HttpEndpoint,
            scoped: Mutex<Option<bool>>,
        }

        impl TokenEndpoint for Watching {
            fn refresh<'a>(&'a self, refresh_token: &'a str) -> RefreshFuture<'a> {
                Box::pin(async move {
                    let grant = self.inner.refresh(refresh_token).await;
                    if let Ok(grant) = &grant {
                        *self.scoped.lock() = Some(grant.scoped);
                    }
                    grant
                })
            }
        }

        /// A forced refresh rotates the pair, writes it back to the account's
        /// Keychain item (its `mdat` advances) and keeps every other field.
        #[tokio::test]
        #[ignore]
        async fn live_forced_refresh_rotates_and_stores_the_pair() {
            let dir = account_dir();
            let store = HostStore::for_account(Some(&dir)).unwrap();
            let service = store.service.clone();
            let before_stamp = store.stamp();
            let before = stored_pair(&store);
            let before_blob: Value =
                serde_json::from_str(&store.load().unwrap().unwrap().json).unwrap();
            let endpoint = Arc::new(Watching {
                inner: HttpEndpoint::default(),
                scoped: Mutex::new(None),
            });
            std::thread::sleep(Duration::from_millis(1100));
            let token = token_with(
                &service,
                Arc::new(store),
                endpoint.clone(),
                Demand::Replace {
                    rejected_expires_at_ms: before.expires_at_ms,
                },
            )
            .await
            .expect("refresh");

            let store = HostStore::for_account(Some(&dir)).unwrap();
            let after = stored_pair(&store);
            let after_blob: Value =
                serde_json::from_str(&store.load().unwrap().unwrap().json).unwrap();
            println!(
                "service={service} scoped={:?} access {}->{} refresh {}->{} \
                 expires_in_min {}->{} stamp {:?}->{:?}",
                endpoint.scoped.lock(),
                digest(&before.access),
                digest(&after.access),
                digest(before.refresh.as_deref().unwrap_or("")),
                digest(after.refresh.as_deref().unwrap_or("")),
                (before.expires_at_ms - now_ms()) / 60_000,
                (after.expires_at_ms - now_ms()) / 60_000,
                before_stamp,
                store.stamp(),
            );
            assert_eq!(token.secret(), after.access);
            assert_ne!(after.access, before.access);
            assert!(after.expires_at_ms > before.expires_at_ms);
            assert_ne!(store.stamp(), before_stamp);
            for field in ["scopes", "subscriptionType", "rateLimitTier"] {
                assert_eq!(
                    after_blob["claudeAiOauth"][field], before_blob["claudeAiOauth"][field],
                    "{field}"
                );
            }
        }

        /// One `claude -p` turn under the seatbelt profile a launch builds, in
        /// the shared default config dir, with an env holding nothing but the
        /// plan's own (the token included) and the bare essentials.
        fn seatbelt_turn(token: &AccessToken) -> (Value, Value, bool) {
            let td = tempfile::tempdir().unwrap();
            let root = td.path().join("agent");
            let cwd = root.join("repo");
            std::fs::create_dir_all(&cwd).unwrap();
            seatbelt_run(
                token,
                &root,
                &cwd,
                &["-p", "Reply with exactly the word: pong"],
            )
        }

        /// `claude <args> --output-format stream-json --verbose --model haiku`
        /// in `cwd` under the profile, with `root` as the writable root.
        fn seatbelt_run(
            token: &AccessToken,
            root: &Path,
            cwd: &Path,
            args: &[&str],
        ) -> (Value, Value, bool) {
            use crate::sandbox::AgentLaunchCtx;
            let home = dirs::home_dir().unwrap();
            let td = tempfile::tempdir().unwrap();
            let rpc = td.path().join("rpc");
            std::fs::create_dir_all(&rpc).unwrap();
            let claude =
                crate::agent::resolve_agent_bin("claude", "claude", "Claude Code", &home).unwrap();
            let ctx = AgentLaunchCtx {
                agent_id: "live",
                provider: "claude",
                writable_root: root,
                source_repos: &[],
                rpc_dir: &rpc,
                cwd,
                home: &home,
                interactive: false,
                blackboard: None,
                account_dir: None,
                oauth_token: Some(token),
                codex_home: None,
            };
            let plan = crate::sandbox::engine_for(crate::sandbox::EngineKind::SandboxExec)
                .unwrap()
                .launch_agent(&ctx, &claude)
                .unwrap();
            let out = std::process::Command::new(&plan.program)
                .args(&plan.prefix_args)
                .args(args)
                .args([
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--model",
                    "haiku",
                ])
                .current_dir(cwd)
                .env_clear()
                .env("HOME", &home)
                .env("PATH", std::env::var("PATH").unwrap())
                .env("USER", std::env::var("USER").unwrap())
                .env("TERM", "xterm-256color")
                .envs(plan.env.iter().map(|(k, v)| (k, v)))
                .output()
                .unwrap();
            let events: Vec<Value> = String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect();
            let find = |pred: &dyn Fn(&Value) -> bool| {
                events.iter().find(|e| pred(e)).cloned().expect("event")
            };
            let init = find(&|e| e["type"] == "system" && e["subtype"] == "init");
            let result = find(&|e| e["type"] == "result");
            let rate_limit = events.iter().any(|e| e["type"] == "rate_limit_event");
            (init, result, rate_limit)
        }

        /// The account's host-resolved token alone completes a turn under the
        /// sandbox, and it is what claude signs in with: the same run with an
        /// invalid token in its place is refused, though the shared config
        /// dir holds a valid `/login` of its own.
        #[tokio::test]
        #[ignore]
        async fn live_seatbelt_turn_runs_on_the_env_token_alone() {
            let token = access_token_for_launch(Some(&account_dir()))
                .await
                .expect("token");
            let (init, result, rate_limit) = seatbelt_turn(&token);
            println!(
                "real token: apiKeySource={} model={} | is_error={} text={} | rate_limit_event={rate_limit}",
                init["apiKeySource"], init["model"], result["is_error"], result["result"],
            );
            assert_eq!(result["is_error"], false, "{result}");

            let bogus = AccessToken::for_test("sk-ant-oat01-not-a-real-token", 0);
            let (_, result, _) = seatbelt_turn(&bogus);
            println!(
                "invalid token: is_error={} api_error_status={} text={}",
                result["is_error"], result["api_error_status"], result["result"],
            );
            assert_eq!(result["api_error_status"], 401, "{result}");
        }

        const MCP_PROBE: &str = r#"import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    mid = msg.get("id")
    method = msg.get("method")
    if mid is None:
        continue
    if method == "initialize":
        result = {"protocolVersion": msg["params"]["protocolVersion"],
                  "capabilities": {"tools": {}},
                  "serverInfo": {"name": "probe", "version": "1"}}
    elif method == "tools/list":
        result = {"tools": [{"name": "marker", "description": "Returns the marker word.",
                             "inputSchema": {"type": "object", "properties": {}}}]}
    elif method == "tools/call":
        result = {"content": [{"type": "text", "text": "zebra-quartz-41"}]}
    else:
        result = {}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": mid, "result": result}) + "\n")
    sys.stdout.flush()
"#;

        /// The Keychain deny doesn't cost claude its MCP servers: a stdio
        /// server's tool is called and answers under the same profile, on the
        /// env token alone.
        #[tokio::test]
        #[ignore]
        async fn live_seatbelt_turn_uses_a_stdio_mcp_server() {
            let token = access_token_for_launch(Some(&account_dir()))
                .await
                .expect("token");
            let td = tempfile::tempdir().unwrap();
            let root = td.path().join("agent");
            let cwd = root.join("repo");
            std::fs::create_dir_all(&cwd).unwrap();
            let server = root.join("probe_mcp.py");
            std::fs::write(&server, MCP_PROBE).unwrap();
            let config = root.join("mcp.json");
            let servers = serde_json::json!({"mcpServers": {"probe": {
                "type": "stdio", "command": "/usr/bin/python3", "args": [server],
            }}});
            std::fs::write(&config, servers.to_string()).unwrap();
            let config_arg = config.to_string_lossy().into_owned();
            let (init, result, _) = seatbelt_run(
                &token,
                &root,
                &cwd,
                &[
                    "--mcp-config",
                    &config_arg,
                    "--strict-mcp-config",
                    "--allowedTools",
                    "mcp__probe__marker",
                    "-p",
                    "Call the marker tool, then reply with exactly the text it returned.",
                ],
            );
            println!(
                "mcp_servers={} | is_error={} text={}",
                init["mcp_servers"], result["is_error"], result["result"]
            );
            assert_eq!(result["is_error"], false, "{result}");
            assert!(
                result["result"]
                    .as_str()
                    .is_some_and(|t| t.contains("zebra-quartz-41")),
                "{result}"
            );
        }

        /// A session written under the account dir before accounts became
        /// token sources resumes once moved into the default dir. Works on a
        /// copy, under a new id, of the session `FLETCH_LIVE_LEGACY_SESSION`
        /// names (a `.jsonl` under a claude account's `projects`), run in
        /// `FLETCH_LIVE_LEGACY_CWD`; the copy is removed afterwards.
        #[tokio::test]
        #[ignore]
        async fn live_legacy_account_session_resumes_from_the_default_dir() {
            let source = PathBuf::from(std::env::var("FLETCH_LIVE_LEGACY_SESSION").unwrap());
            let cwd = PathBuf::from(std::env::var("FLETCH_LIVE_LEGACY_CWD").unwrap());
            let old_id = source.file_stem().unwrap().to_string_lossy().into_owned();
            let new_id = uuid::Uuid::new_v4().to_string();
            let copy = source.with_file_name(format!("{new_id}.jsonl"));
            let text = std::fs::read_to_string(&source)
                .unwrap()
                .replace(&old_id, &new_id);
            std::fs::write(&copy, text).unwrap();

            let moved = crate::transcripts::adopt_account_session(&new_id, &cwd)
                .unwrap()
                .expect("the copy moves into the default dir");
            println!("moved to {}", moved.display());
            assert!(!copy.exists());

            let token = access_token_for_launch(Some(&account_dir()))
                .await
                .expect("token");
            let (_, result, _) = seatbelt_run(
                &token,
                &cwd,
                &cwd,
                &[
                    "--resume",
                    &new_id,
                    "-p",
                    "Reply with exactly the word: resumed",
                ],
            );
            let _ = std::fs::remove_file(&moved);
            println!(
                "resume: is_error={} num_turns={} text={}",
                result["is_error"], result["num_turns"], result["result"]
            );
            assert_eq!(result["is_error"], false, "{result}");
        }
    }
}
