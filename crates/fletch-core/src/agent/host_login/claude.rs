//! The host owns every claude login. A sandboxed claude can't refresh its own
//! OAuth token (the refresh needs lock files and a Keychain write the sandbox
//! denies, and a container has no Keychain at all), and it must never hold the
//! long-lived refresh token anyway. So the app reads each account's stored
//! login, refreshes it when it is about to lapse, writes the rotated pair back
//! where claude keeps it, and hands the agent only the short-lived access
//! token as `CLAUDE_CODE_OAUTH_TOKEN`.
//!
//! The flow (single-flight refresh, write-back, a rotated pair the store
//! refused kept in memory and retried, the refused-refresh mark) is the
//! engine's (`host_login`). What is claude's own: the store (the Keychain item
//! or `.credentials.json`), the placeholder rule, the 4 KB fit check, the
//! margin, and the sign-in server's request and answers.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use super::{Creds, Demand, HostLogin, LoginProvider, RefreshFailure};
use crate::agent::accounts;

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
    credential(account_dir, Demand::Launch).await
}

/// A token to replace one the API rejected (a 401) although it was not yet
/// due: the store's newer token if someone already refreshed past
/// `rejected_expires_at_ms`, else a forced refresh.
pub async fn replace_rejected_token(
    account_dir: Option<&Path>,
    rejected_expires_at_ms: i64,
) -> Result<AccessToken, LoginError> {
    credential(
        account_dir,
        Demand::Replace {
            rejected_expires_at_ms,
        },
    )
    .await
}

/// The engine's flow for the account in `account_dir`, on a blocking task.
async fn credential(account_dir: Option<&Path>, demand: Demand) -> Result<AccessToken, LoginError> {
    let login = ClaudeLogin::for_account(account_dir).ok_or(LoginError::SignedOut)?;
    tokio::task::spawn_blocking(move || HostLogin::new(login).credential(demand))
        .await
        .map_err(|e| LoginError::Unavailable(format!("credential store task failed: {e}")))?
        .map_err(LoginError::from)
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
    ClaudeLogin::for_account(account_dir).is_some_and(|login| HostLogin::new(login).is_revoked())
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

/// One account's claude login on the engine: its store and its sign-in
/// server.
struct ClaudeLogin {
    store: HostStore,
    token_url: String,
}

impl ClaudeLogin {
    fn for_account(account_dir: Option<&Path>) -> Option<Self> {
        Some(Self {
            store: HostStore::for_account(account_dir)?,
            token_url: TOKEN_URL.to_string(),
        })
    }
}

impl LoginProvider for ClaudeLogin {
    type Place = Place;
    type Grant = TokenGrant;
    type Launch = AccessToken;
    const PROVIDER: &'static str = "claude";

    fn key(&self) -> String {
        self.store.service.clone()
    }

    fn stamp(&self) -> Option<String> {
        self.store.stamp()
    }

    fn load(&self) -> Result<Option<(String, Place)>, String> {
        self.store.load()
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
        let grant = request_refresh(&self.token_url, refresh_token);
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

    /// Recorded so Settings still reads "sign in again" after a restart:
    /// the store's stamp only, never anything secret.
    fn mark_path(&self) -> Option<PathBuf> {
        super::mark_file("claude-revoked", &self.store.service)
    }
}

/// Where claude keeps a config dir's login: the Keychain item named for the
/// dir, else `<dir>/.credentials.json` (claude's fallback when it can't write
/// the Keychain, and the only store off macOS). Blocking.
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

    /// A value that changes whenever the stored login does, without reading
    /// the secret. `None` = nothing stored.
    fn stamp(&self) -> Option<String> {
        match crate::keychain::item_stamp(&self.service) {
            Some(item) => Some(format!("keychain:{}", item.modified)),
            None => self.file_login().and_then(|_| self.file_stamp()),
        }
    }

    fn load(&self) -> Result<Option<(String, Place)>, String> {
        if let Some(item) = crate::keychain::item_stamp(&self.service) {
            match crate::keychain::read_password(&self.service) {
                Some(json) if stored_access_token(json.as_bytes()).is_some() => {
                    let account = item
                        .account
                        .or_else(|| std::env::var("USER").ok())
                        .ok_or_else(|| "no Keychain account name for the login".to_string())?;
                    return Ok(Some((json, Place::Keychain { account })));
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
        Ok(self.file_login().map(|json| (json, Place::File)))
    }

    /// Whether a blob of `len` bytes can be written back to `place` whole.
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

/// One refresh at `url`, run to completion from the engine's blocking thread
/// (`super::run_request`). A server that refuses the `scope` field gets the
/// request again without it.
fn request_refresh(url: &str, refresh_token: &str) -> Result<TokenGrant, RefreshFailure> {
    let url = url.to_string();
    let refresh_token = refresh_token.to_string();
    super::run_request(async move {
        let client = reqwest::Client::builder()
            .user_agent("Fletch")
            .timeout(TIMEOUT)
            .build()
            .map_err(|e| RefreshFailure::Failed(format!("http client: {e}")))?;
        let mut answer = post(&client, &url, &refresh_token, true).await?;
        let mut scoped = true;
        if answer.0 == 400 && oauth_error(answer.1.as_ref()) == Some("invalid_scope") {
            scoped = false;
            answer = post(&client, &url, &refresh_token, false).await?;
        }
        let mut grant = interpret_refresh(answer.0, answer.1.as_ref())?;
        grant.scoped = scoped;
        Ok(grant)
    })
}

async fn post(
    client: &reqwest::Client,
    url: &str,
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
    let response =
        client.post(url).json(&body).send().await.map_err(|e| {
            RefreshFailure::Failed(format!("the sign-in server is unreachable ({e})"))
        })?;
    let status = response.status().as_u16();
    Ok((status, response.json::<Value>().await.ok()))
}

fn oauth_error(body: Option<&Value>) -> Option<&str> {
    body?.get("error")?.as_str()
}

/// A 401, or a 400 naming `invalid_grant` (revoked, expired, or already spent
/// by a rotation, per RFC 6749 §5.2), refuses the refresh token itself; any
/// other 400 is a malformed or unsupported request that a new sign-in would
/// not fix, so it, like anything else, leaves the stored pair valid for a
/// later try. The message names the status, and for a 400 only one of the
/// standard OAuth error codes; nothing else from the body, which may echo
/// credentials.
fn interpret_refresh(status: u16, body: Option<&Value>) -> Result<TokenGrant, RefreshFailure> {
    match status {
        200..=299 => body.and_then(parse_grant).ok_or_else(|| {
            RefreshFailure::Failed("the sign-in server answered without a token".into())
        }),
        401 => Err(RefreshFailure::Rejected),
        400 if oauth_error(body) == Some("invalid_grant") => Err(RefreshFailure::Rejected),
        400 => Err(RefreshFailure::Failed(match oauth_error(body) {
            Some(
                code @ ("invalid_request"
                | "unsupported_grant_type"
                | "invalid_client"
                | "invalid_scope"),
            ) => format!("the sign-in server answered 400 ({code})"),
            _ => "the sign-in server answered an unexpected 400".into(),
        })),
        other => Err(RefreshFailure::Failed(format!(
            "the sign-in server answered {other}"
        ))),
    }
}

#[cfg(test)]
mod tests;
