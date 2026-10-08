//! The host's side of a codex ChatGPT login: Fletch reads the account's
//! `auth.json`, refreshes it here when the access token nears expiry, and hands
//! each sandboxed launch a copy with the refresh token blanked, written into a
//! per-agent `CODEX_HOME` overlay the host assembles. The sandbox never holds
//! the refresh token, so it can neither refresh the login itself nor persist a
//! rotated token over the host's file.
//!
//! Format and behaviour verified against codex-cli 0.154.0 and openai/codex
//! `codex-rs/login/src/auth/manager.rs`:
//! - `tokens.access_token` is the bearer sent to the API, a JWT that lives ten
//!   days; `id_token` lives an hour and is only read for its claims.
//! - codex refreshes by itself only when the access token is within five
//!   minutes of `exp` (or, with no readable `exp`, when `last_refresh` is over
//!   eight days old), via `POST https://auth.openai.com/oauth/token` with a JSON
//!   body `{grant_type: "refresh_token", client_id, refresh_token}`. The answer
//!   may rotate the refresh token, and a reused one is refused
//!   (`refresh_token_reused`), so exactly one party may refresh a login. Before
//!   any refresh, proactive or after a 401, codex re-reads `auth.json` and uses
//!   a token another process wrote there instead ("guarded reload"), which is
//!   what lets the host rotate a login a running codex also holds.
//! - codex refuses an `auth.json` with no `refresh_token` field at all ("Missing
//!   bearer") but runs normally with an empty one; with an expired access token
//!   and an empty refresh token the turn fails with "Failed to refresh token:
//!   400 Bad Request: Invalid 'refresh_token': empty string".

use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use serde_json::Value;

use super::{Creds, Demand, HostLogin, LoginError, LoginProvider, RefreshFailure};

pub const REFRESH_URL: &str = "https://auth.openai.com/oauth/token";

/// codex's own override of [`REFRESH_URL`], honoured the same way so a test
/// rig that points codex elsewhere points the host refresh there too.
const REFRESH_URL_OVERRIDE_ENV: &str = "CODEX_REFRESH_TOKEN_URL_OVERRIDE";

/// The OAuth client codex logs in as; a refresh must name the same client.
pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// How long before `exp` the host refreshes, at most. Every turn gets a fresh
/// file, so this only has to outlast one turn: a day covers any real turn
/// while refreshing a ten-day token about once every nine days.
const MAX_REFRESH_MARGIN_SECS: i64 = 24 * 3600;

/// codex's fallback when the access token's `exp` can't be read.
const STALE_LAST_REFRESH_SECS: i64 = 8 * 24 * 3600;

const REFRESH_TIMEOUT: Duration = Duration::from_secs(20);

const AUTH_FILE: &str = "auth.json";

/// Where the login a codex launch runs under is stored on the host: the
/// managed account's directory, or the user's own codex home.
pub fn source_home(account_dir: Option<&Path>, home: &Path) -> PathBuf {
    match account_dir {
        Some(dir) => dir.to_path_buf(),
        None => crate::sandbox::policy::codex_home_dir(home),
    }
}

/// The token endpoint's answer, as far as a refresh needs it.
#[derive(Clone, Default, PartialEq, Eq, serde::Deserialize)]
pub(crate) struct TokenResponse {
    pub access_token: Option<String>,
    pub id_token: Option<String>,
    pub refresh_token: Option<String>,
}

impl std::fmt::Debug for TokenResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let has = |t: &Option<String>| t.as_deref().is_some_and(|t| !t.is_empty());
        f.debug_struct("TokenResponse")
            .field("access_token", &has(&self.access_token))
            .field("id_token", &has(&self.id_token))
            .field("refresh_token", &has(&self.refresh_token))
            .finish()
    }
}

/// Exchanges a refresh token for fresh tokens. Production is [`http_refresh`];
/// tests pass a closure.
pub(crate) type Refresher<'a> =
    &'a (dyn Fn(&str) -> std::result::Result<TokenResponse, RefreshFailure> + Sync);

/// A codex login on the engine: the `auth.json` in `source_home`, refreshed
/// through `refresh`, judged at `now` (seconds).
struct CodexProvider<'a> {
    source_home: &'a Path,
    refresh: Refresher<'a>,
    now: i64,
}

impl CodexProvider<'_> {
    fn stored(&self) -> PathBuf {
        self.source_home.join(AUTH_FILE)
    }
}

impl LoginProvider for CodexProvider<'_> {
    type Place = ();
    type Grant = TokenResponse;
    type Launch = Value;
    const PROVIDER: &'static str = "codex";

    fn key(&self) -> String {
        self.source_home.to_string_lossy().into_owned()
    }

    fn stamp(&self) -> Option<String> {
        crate::agent::credential_file::file_stamp(&self.stored())
    }

    fn load(&self) -> Result<Option<(String, ())>, String> {
        Ok(std::fs::read_to_string(self.stored())
            .ok()
            .map(|raw| (raw, ())))
    }

    fn parse(&self, json: &Value) -> Option<Creds> {
        json.is_object().then(|| Creds {
            refresh: refresh_token(json).map(str::to_string),
            expires_at_ms: access_expiry(json).map(|(_, exp)| exp * 1000),
        })
    }

    fn due(&self, _creds: &Creds, json: &Value, now_ms: i64) -> bool {
        needs_refresh(json, now_ms / 1000)
    }

    fn refresh(&self, refresh_token: &str) -> Result<TokenResponse, RefreshFailure> {
        (self.refresh)(refresh_token)
    }

    fn apply(&self, json: &mut Value, grant: &TokenResponse, now_ms: i64) -> bool {
        if !json.is_object() {
            return false;
        }
        apply_refresh(json, grant.clone(), now_ms / 1000);
        true
    }

    fn save(&self, _place: &(), json: &Value) -> Result<(), String> {
        let text = serde_json::to_string_pretty(json).map_err(|e| e.to_string())?;
        crate::agent::credential_file::write_private_file(&self.stored(), text.as_bytes())
            .map_err(|e| e.to_string())
    }

    fn launch(&self, json: &Value, _creds: &Creds) -> Value {
        launch_credential(json)
    }

    /// A refusal is recorded against the refused refresh token, by its hash
    /// only: a new sign-in writes a new one, and the mark lapses.
    fn mark_of(&self, _stamp: Option<&str>, json: &Value) -> Option<String> {
        refresh_token(json).map(|t| super::digest(t.as_bytes()))
    }

    fn current_mark(&self) -> Option<String> {
        let json = read_auth(&self.stored())?;
        self.mark_of(None, &json)
    }

    fn mark_path(&self) -> Option<PathBuf> {
        super::mark_file("codex-signed-out", &self.key())
    }

    fn now_ms(&self) -> i64 {
        self.now * 1000
    }
}

/// Why a codex launch got no credential, with the text the user sees. Never
/// carries a credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CodexLoginError {
    /// The refresh token was refused; only a new sign-in helps.
    Revoked,
    /// The access token has expired and the refresh could not be completed.
    Unavailable(String),
}

impl std::fmt::Display for CodexLoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Revoked => f.write_str(
                "The Codex login has expired or was revoked. Sign in again in Settings › Providers.",
            ),
            Self::Unavailable(reason) => write!(
                f,
                "Couldn't refresh the Codex login ({reason}); its access token has expired."
            ),
        }
    }
}

impl From<CodexLoginError> for crate::error::Error {
    fn from(e: CodexLoginError) -> Self {
        Self::Other(e.to_string())
    }
}

/// The launch credential for a codex run under the login in `source_home`
/// (see [`launch_credential`]): `None` when nothing is stored. The login is
/// refreshed on the host first when its access token is about to expire,
/// single-flight, written back atomically, kept in memory when that write
/// fails; a refused refresh token is recorded so Settings reads the account
/// signed out.
pub(crate) fn launch_file(
    source_home: &Path,
    refresh: Refresher<'_>,
    now: i64,
) -> Result<Option<Value>, CodexLoginError> {
    let login = HostLogin::new(CodexProvider {
        source_home,
        refresh,
        now,
    });
    match login.credential(Demand::Launch) {
        Ok(launch) => Ok(Some(launch)),
        Err(LoginError::SignedOut) => Ok(None),
        Err(LoginError::Revoked) => Err(CodexLoginError::Revoked),
        Err(LoginError::Unavailable(reason)) => Err(CodexLoginError::Unavailable(reason)),
    }
}

/// Whether a refresh of the login in `source_home` was refused and it hasn't
/// changed since. For the sign-in probe.
pub(crate) fn is_revoked(source_home: &Path) -> bool {
    HostLogin::new(CodexProvider {
        source_home,
        refresh: &http_refresh,
        now: chrono::Utc::now().timestamp(),
    })
    .is_revoked()
}

/// The production [`Refresher`]: the codex token endpoint over HTTPS. Blocking,
/// because launches and turns are synchronous; the request runs on its own
/// thread and runtime so a caller on an async worker never nests runtimes.
pub(crate) fn http_refresh(
    refresh_token: &str,
) -> std::result::Result<TokenResponse, RefreshFailure> {
    let url = std::env::var(REFRESH_URL_OVERRIDE_ENV)
        .ok()
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| REFRESH_URL.to_string());
    refresh_at(&url, refresh_token)
}

/// [`http_refresh`] against `url`.
pub(crate) fn refresh_at(
    url: &str,
    refresh_token: &str,
) -> std::result::Result<TokenResponse, RefreshFailure> {
    let url = url.to_string();
    let refresh_token = refresh_token.to_string();
    super::run_request(async move { post_refresh(&url, &refresh_token).await })
}

async fn post_refresh(
    url: &str,
    refresh_token: &str,
) -> std::result::Result<TokenResponse, RefreshFailure> {
    let client = reqwest::Client::builder()
        .user_agent("Fletch")
        .timeout(REFRESH_TIMEOUT)
        .build()
        .map_err(|e| RefreshFailure::Failed(format!("http client: {e}")))?;
    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "client_id": CLIENT_ID,
        "refresh_token": refresh_token,
    });
    // A transport error's text names the URL, never the request body.
    let response = client
        .post(url)
        .json(&body)
        .send()
        .await
        .map_err(|e| RefreshFailure::Failed(format!("sign-in service unreachable: {e}")))?;
    let status = response.status().as_u16();
    let answer = response.json::<Value>().await.ok();
    interpret_refresh(status, answer)
}

/// What the token endpoint's answer means. A 401, or one of the refresh-token
/// error codes codex itself treats as permanent, is a dead login; anything else
/// that failed is worth retrying on the next turn.
pub(crate) fn interpret_refresh(
    status: u16,
    body: Option<Value>,
) -> std::result::Result<TokenResponse, RefreshFailure> {
    if (200..300).contains(&status) {
        let tokens: TokenResponse = body
            .and_then(|b| serde_json::from_value(b).ok())
            .unwrap_or_default();
        return match tokens.access_token.as_deref() {
            Some(t) if !t.is_empty() => Ok(tokens),
            _ => Err(RefreshFailure::Failed(
                "the sign-in service answered without an access token".into(),
            )),
        };
    }
    let code = body.as_ref().and_then(error_code);
    let permanent_code = code.as_deref().is_some_and(|c| {
        matches!(
            c,
            "refresh_token_expired"
                | "refresh_token_reused"
                | "refresh_token_invalidated"
                | "invalid_refresh_token"
                | "invalid_grant"
        )
    });
    if status == 401 || permanent_code {
        return Err(RefreshFailure::Rejected);
    }
    Err(RefreshFailure::Failed(match code {
        Some(code) => format!("the sign-in service answered {status} ({code})"),
        None => format!("the sign-in service answered {status}"),
    }))
}

/// The error code of a token-endpoint failure, in either shape it comes in:
/// OpenAI's `{error: {code}}` or OAuth's `{error: "<code>"}`.
fn error_code(body: &Value) -> Option<String> {
    let error = body.get("error")?;
    error
        .get("code")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .map(str::to_ascii_lowercase)
}

/// The launch copy of a login: everything as stored, unknown fields included,
/// with the refresh token blanked. Blanked, not removed — codex rejects the
/// whole file without the field.
pub(crate) fn launch_credential(auth: &Value) -> Value {
    let mut out = auth.clone();
    if let Some(tokens) = out.get_mut("tokens").and_then(Value::as_object_mut) {
        if tokens.contains_key("refresh_token") {
            tokens.insert("refresh_token".into(), Value::String(String::new()));
        }
    }
    out
}

fn refresh_token(auth: &Value) -> Option<&str> {
    auth.pointer("/tokens/refresh_token")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
}

/// `(iat, exp)` of the access token, from its unverified payload; `iat` is 0
/// when absent.
fn access_expiry(auth: &Value) -> Option<(i64, i64)> {
    let token = auth.pointer("/tokens/access_token")?.as_str()?;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    let exp = claims.get("exp")?.as_i64()?;
    let iat = claims.get("iat").and_then(Value::as_i64).unwrap_or(0);
    Some((iat, exp))
}

/// Whether the host should refresh before this launch: only a ChatGPT login
/// with a refresh token, and only once its access token is inside the margin —
/// a day, or half the token's lifetime if that is shorter, so a short-lived
/// token isn't refreshed on every turn.
pub(crate) fn needs_refresh(auth: &Value, now: i64) -> bool {
    if refresh_token(auth).is_none() {
        return false;
    }
    match access_expiry(auth) {
        Some((iat, exp)) => {
            let lifetime = if iat > 0 { exp - iat } else { i64::MAX };
            let margin = MAX_REFRESH_MARGIN_SECS.min(lifetime / 2);
            exp - now <= margin
        }
        None => auth
            .get("last_refresh")
            .and_then(Value::as_str)
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map_or(true, |at| now - at.timestamp() > STALE_LAST_REFRESH_SECS),
    }
}

/// Fold a refresh into the stored login the way codex's own `persist_tokens`
/// does: replace the tokens that came back, keep the rest, stamp
/// `last_refresh`.
pub(crate) fn apply_refresh(auth: &mut Value, tokens: TokenResponse, now: i64) {
    let Some(obj) = auth.as_object_mut() else {
        return;
    };
    let stored = obj
        .entry("tokens")
        .or_insert_with(|| Value::Object(Default::default()));
    if let Some(stored) = stored.as_object_mut() {
        for (key, value) in [
            ("access_token", tokens.access_token),
            ("id_token", tokens.id_token),
            ("refresh_token", tokens.refresh_token),
        ] {
            if let Some(value) = value.filter(|v| !v.is_empty()) {
                stored.insert(key.into(), Value::String(value));
            }
        }
    }
    let stamp = chrono::DateTime::from_timestamp(now, 0)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
    obj.insert("last_refresh".into(), Value::String(stamp));
}

pub(crate) fn read_auth(path: &Path) -> Option<Value> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice::<Value>(&bytes)
        .ok()
        .filter(Value::is_object)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: i64 = 1_791_448_171;
    const DAY: i64 = 24 * 3600;

    fn jwt(iat: i64, exp: i64) -> String {
        let enc = |v: Value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string().as_bytes())
        };
        format!(
            "{}.{}.sig",
            enc(json!({"alg": "RS256"})),
            enc(json!({"iat": iat, "exp": exp}))
        )
    }

    fn login(access_exp: i64) -> Value {
        json!({
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": "id.jwt.value",
                "access_token": jwt(access_exp - 10 * DAY, access_exp),
                "refresh_token": "rt.original",
                "account_id": "acct-1",
                "future_token_field": 7
            },
            "last_refresh": "2026-10-02T15:37:10.326659Z",
            "future_top_level": {"kept": true}
        })
    }

    fn rotated() -> TokenResponse {
        TokenResponse {
            access_token: Some(jwt(NOW, NOW + 10 * DAY)),
            id_token: Some("id.new".into()),
            refresh_token: Some("rt.rotated".into()),
        }
    }

    #[test]
    fn the_launch_credential_blanks_the_refresh_token_and_keeps_every_other_field() {
        let auth = login(NOW + 5 * DAY);
        let out = launch_credential(&auth);
        assert_eq!(out["tokens"]["refresh_token"], "");
        assert_eq!(
            out["tokens"]["access_token"],
            auth["tokens"]["access_token"]
        );
        assert_eq!(out["tokens"]["future_token_field"], 7);
        assert_eq!(out["future_top_level"], json!({"kept": true}));
        assert_eq!(out["last_refresh"], auth["last_refresh"]);
    }

    #[test]
    fn an_api_key_login_is_passed_through_unchanged() {
        let auth = json!({"auth_mode": "apikey", "OPENAI_API_KEY": "sk-x"});
        assert_eq!(launch_credential(&auth), auth);
        assert!(!needs_refresh(&auth, NOW));
    }

    #[test]
    fn a_token_with_days_left_is_not_refreshed() {
        assert!(!needs_refresh(&login(NOW + 2 * DAY), NOW));
    }

    #[test]
    fn a_token_inside_the_day_margin_is_refreshed() {
        assert!(needs_refresh(&login(NOW + DAY - 1), NOW));
        assert!(needs_refresh(&login(NOW - 1), NOW));
    }

    #[test]
    fn a_short_lived_token_uses_half_its_lifetime_as_the_margin() {
        let mut auth = login(0);
        auth["tokens"]["access_token"] = Value::String(jwt(NOW - 1000, NOW + 2600));
        assert!(!needs_refresh(&auth, NOW));
        auth["tokens"]["access_token"] = Value::String(jwt(NOW - 2000, NOW + 1600));
        assert!(needs_refresh(&auth, NOW));
    }

    #[test]
    fn an_unreadable_token_falls_back_to_codexs_last_refresh_rule() {
        let mut auth = login(0);
        auth["tokens"]["access_token"] = Value::String("opaque".into());
        auth["last_refresh"] = Value::String("2026-10-07T00:00:00Z".into());
        assert!(!needs_refresh(&auth, NOW));
        auth["last_refresh"] = Value::String("2026-09-01T00:00:00Z".into());
        assert!(needs_refresh(&auth, NOW));
    }

    #[test]
    fn a_login_without_a_refresh_token_is_never_refreshed() {
        let mut auth = login(NOW - 1);
        auth["tokens"]["refresh_token"] = Value::String(String::new());
        assert!(!needs_refresh(&auth, NOW));
    }

    #[test]
    fn a_token_response_debug_prints_no_token() {
        let printed = format!("{:?}", rotated());
        assert!(!printed.contains("rt.rotated"), "{printed}");
        assert!(!printed.contains("id.new"), "{printed}");
    }

    #[test]
    fn the_endpoints_refusals_map_to_rejected_or_failed() {
        let openai = |code: &str| Some(json!({"error": {"message": "m", "code": code}}));
        assert_eq!(
            interpret_refresh(401, openai("invalid_refresh_token")),
            Err(RefreshFailure::Rejected)
        );
        assert_eq!(
            interpret_refresh(400, openai("refresh_token_reused")),
            Err(RefreshFailure::Rejected)
        );
        assert_eq!(
            interpret_refresh(400, Some(json!({"error": "invalid_grant"}))),
            Err(RefreshFailure::Rejected)
        );
        assert_eq!(
            interpret_refresh(503, None),
            Err(RefreshFailure::Failed(
                "the sign-in service answered 503".into()
            ))
        );
        assert!(matches!(
            interpret_refresh(200, Some(json!({}))),
            Err(RefreshFailure::Failed(_))
        ));
    }

    /// One canned HTTP exchange on a local port: returns the URL and a handle
    /// yielding the raw request it received.
    fn mock_endpoint(
        status: &str,
        body: &'static str,
    ) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/oauth/token", listener.local_addr().unwrap());
        let status = status.to_string();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = stream.read(&mut buf).unwrap();
                request.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&request);
                if let Some(head_end) = text.find("\r\n\r\n") {
                    let length = text[..head_end]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= head_end + 4 + length {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let reply = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(reply.as_bytes()).unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        (url, handle)
    }

    #[test]
    fn the_refresh_posts_codexs_grant_and_reads_the_rotated_tokens() {
        let (url, server) = mock_endpoint(
            "200 OK",
            r#"{"access_token":"at.new","id_token":"id.new","refresh_token":"rt.next","expires_in":864000}"#,
        );
        let tokens = refresh_at(&url, "rt.current").unwrap();
        let request = server.join().unwrap();

        assert!(request.starts_with("POST /oauth/token"), "{request}");
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(
            body,
            json!({"grant_type": "refresh_token", "client_id": CLIENT_ID, "refresh_token": "rt.current"})
        );
        assert_eq!(
            tokens,
            TokenResponse {
                access_token: Some("at.new".into()),
                id_token: Some("id.new".into()),
                refresh_token: Some("rt.next".into()),
            }
        );
    }

    #[test]
    fn a_refused_refresh_token_reads_as_rejected() {
        let (url, server) = mock_endpoint(
            "401 Unauthorized",
            r#"{"error":{"message":"Invalid refresh token.","type":"invalid_request_error","param":null,"code":"invalid_refresh_token"}}"#,
        );
        assert_eq!(refresh_at(&url, "rt.dead"), Err(RefreshFailure::Rejected));
        server.join().unwrap();
    }
}
