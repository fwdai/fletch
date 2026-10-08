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
use sha2::{Digest, Sha256};

use super::accounts;
use super::credential_file::{self, Entry, Flights, PrivateDir, RefreshFailure};
use crate::error::{Error, Result};

pub const REFRESH_URL: &str = "https://auth.openai.com/oauth/token";

/// codex's own override of [`REFRESH_URL`], honoured the same way so a test
/// rig that points codex elsewhere points the host refresh there too.
const REFRESH_URL_OVERRIDE_ENV: &str = "CODEX_REFRESH_TOKEN_URL_OVERRIDE";

/// The OAuth client codex logs in as; a refresh must name the same client.
pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// The per-agent `CODEX_HOME`, under the agent's own checkouts dir, the way
/// the container claude path keeps a per-agent `projects` dir
/// (`transcripts::DOCKER_CLAUDE_PROJECTS_DIRNAME`).
pub(crate) const OVERLAY_DIRNAME: &str = ".fletch-codex-home";

const AUTH_FILE: &str = "auth.json";

/// How long before `exp` the host refreshes, at most. Every turn gets a fresh
/// file, so this only has to outlast one turn: a day covers any real turn
/// while refreshing a ten-day token about once every nine days.
const MAX_REFRESH_MARGIN_SECS: i64 = 24 * 3600;

/// codex's fallback when the access token's `exp` can't be read.
const STALE_LAST_REFRESH_SECS: i64 = 8 * 24 * 3600;

const REFRESH_TIMEOUT: Duration = Duration::from_secs(20);

/// The overlay inside an agent's checkouts dir.
pub(crate) fn overlay_in(agent_dir: &Path) -> PathBuf {
    agent_dir.join(OVERLAY_DIRNAME)
}

/// The `CODEX_HOME` agent `agent_id` runs in. The one answer the launch, the
/// fork/rewind writer and the transcript locator all use, so a thread is
/// always where the next `codex exec resume` looks, whatever the agent's
/// checkout layout.
pub fn overlay_for_agent(agent_id: &str) -> Result<PathBuf> {
    Ok(overlay_in(&crate::workspace::agent_parent_dir(agent_id)?))
}

/// Where the login a codex launch runs under is stored on the host: the
/// managed account's directory, or the user's own codex home.
pub fn source_home(account_dir: Option<&Path>, home: &Path) -> PathBuf {
    match account_dir {
        Some(dir) => dir.to_path_buf(),
        None => crate::sandbox::policy::codex_home_dir(home),
    }
}

/// Open `overlay` for a host write: its parent (the agent's checkouts dir,
/// which the agent can write into but not replace) by handle, and the overlay
/// beneath it without following a link, recreated as a real directory when
/// the agent swapped it for anything else. Every host write into an overlay
/// goes through the handle this returns, so nothing the agent plants
/// redirects it.
pub(crate) fn open_overlay(overlay: &Path) -> Result<PrivateDir> {
    let (parent, name) = match (
        overlay.parent(),
        overlay.file_name().and_then(|n| n.to_str()),
    ) {
        (Some(parent), Some(name)) => (parent, name),
        _ => return Err(Error::Other("the codex overlay has no parent".into())),
    };
    std::fs::create_dir_all(parent)?;
    Ok(PrivateDir::open(parent)?.subdir(name)?)
}

/// Create (or repair) the overlay: its `sessions` dir, and the shared config
/// linked in from the user's codex home. The overlay belongs to Fletch, so
/// anything the agent left where a shared link belongs is replaced by the
/// link again.
pub fn prepare_overlay(overlay: &Path, home: &Path) -> Result<()> {
    let dir = open_overlay(overlay)?;
    dir.subdir("sessions")?;
    let source = accounts::shared_source_dir("codex", home);
    for item in accounts::shared_items("codex") {
        let target = source.join(item);
        match dir.entry(item)? {
            Entry::Link(at) if at == target => continue,
            Entry::Missing => {}
            _ => dir.remove(item)?,
        }
        if target.exists() {
            dir.symlink(&target, item)?;
        }
    }
    Ok(())
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

/// Write the launch credential for a codex run into `overlay`, refreshing the
/// host's login in `source_home` first when its access token is about to
/// expire. Called before every launch and every turn.
///
/// Single-flight per login: concurrent launches under one account serialise
/// here, and the file is re-read under the lock, so a refresh another launch
/// (or the user's own codex) just made is used rather than repeated with a
/// refresh token that has since rotated. A refusal is checked the same way:
/// a refresh token that changed on disk since it was read is tried once
/// before the login counts as dead.
///
/// A rotated login whose save failed is kept in memory ([`KEPT`]): the
/// refresh token on disk is already spent, so dropping it would sign the
/// account out. Launches use the kept login, and each retries the save while
/// the file is as the failed save left it; a file that changed meanwhile (a
/// new sign-in) wins and the kept login is dropped.
pub(crate) fn write_launch_credential(
    source_home: &Path,
    overlay: &Path,
    refresh: Refresher<'_>,
    now: i64,
) -> Result<()> {
    static FLIGHTS: Flights<parking_lot::Mutex<()>> = Flights::new();
    let lock = FLIGHTS.get(&source_home.to_string_lossy());
    let _flight = lock.lock();
    let dir = open_overlay(overlay)?;
    let stored = source_home.join(AUTH_FILE);
    let key = source_home.to_string_lossy();
    let kept = KEPT.current(&key, credential_file::file_stamp(&stored).as_deref());
    let found = match kept {
        Some(kept) => {
            save_rotated(&key, &stored, &kept);
            Some(kept)
        }
        None => read_auth(&stored),
    };
    let Some(mut auth) = found else {
        dir.remove(AUTH_FILE)?;
        return Ok(());
    };
    let mut retried = false;
    while needs_refresh(&auth, now) {
        let sent = refresh_token(&auth).unwrap_or_default().to_string();
        match refresh(&sent) {
            Ok(tokens) => {
                apply_refresh(&mut auth, tokens, now);
                save_rotated(&key, &stored, &auth);
                break;
            }
            Err(RefreshFailure::Rejected) => {
                let rotated = read_auth(&stored)
                    .filter(|fresh| refresh_token(fresh).is_some_and(|t| t != sent));
                match rotated {
                    Some(fresh) if !retried => {
                        retried = true;
                        auth = fresh;
                    }
                    _ => {
                        mark_signed_out(source_home, &sent);
                        dir.remove(AUTH_FILE)?;
                        return Err(Error::Other(SIGNED_OUT_MSG.into()));
                    }
                }
            }
            Err(RefreshFailure::Failed(reason)) => {
                if access_expiry(&auth).is_some_and(|(_, exp)| exp <= now) {
                    return Err(Error::Other(format!(
                        "Couldn't refresh the Codex login ({reason}); its access token has expired."
                    )));
                }
                tracing::warn!(%reason, "codex login refresh failed; launching with the current access token");
                break;
            }
        }
    }
    let launch = serde_json::to_string_pretty(&launch_credential(&auth))?;
    dir.write_file(AUTH_FILE, launch.as_bytes(), 0o600)?;
    Ok(())
}

/// Rotated logins whose save failed, by source home (see
/// [`write_launch_credential`]).
static KEPT: credential_file::Kept<Value> = credential_file::Kept::new();

/// Save a rotated login to `stored`, or keep it in memory with the file's
/// stamp when the save fails, so the next launch can use it and retry.
fn save_rotated(key: &str, stored: &Path, auth: &Value) {
    let saved = serde_json::to_string_pretty(auth)
        .map_err(std::io::Error::other)
        .and_then(|json| credential_file::write_private_file(stored, json.as_bytes()));
    match saved {
        Ok(()) => KEPT.forget(key),
        Err(e) => {
            tracing::warn!(error = %e, "could not save the refreshed codex login; keeping it in memory");
            KEPT.keep(key, auth.clone(), credential_file::file_stamp(stored));
        }
    }
}

pub(crate) const SIGNED_OUT_MSG: &str =
    "The Codex login has expired or was revoked. Sign in again in Settings › Providers.";

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
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| RefreshFailure::Failed(format!("no runtime: {e}")))?;
        runtime.block_on(post_refresh(&url, &refresh_token))
    })
    .join()
    .unwrap_or_else(|_| Err(RefreshFailure::Failed("refresh thread panicked".into())))
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

fn read_auth(path: &Path) -> Option<Value> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice::<Value>(&bytes)
        .ok()
        .filter(Value::is_object)
}

/// Where the signed-out mark for the login in `source_home` lives: under the
/// accounts root, never in the user's own codex home, named for the home's
/// path.
fn signed_out_marker(source_home: &Path) -> Option<PathBuf> {
    let root = accounts::accounts_root().ok()?;
    Some(
        root.join(".state")
            .join("codex-signed-out")
            .join(&hex_digest(source_home.to_string_lossy().as_bytes())[..16]),
    )
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Remember that the refresh token in `source_home` was refused, by its hash
/// only. A new sign-in writes a new refresh token, which the mark no longer
/// matches, so the mark lapses on its own. Best-effort: a lost mark costs the
/// Settings badge, not the launch, which fails anyway.
fn mark_signed_out(source_home: &Path, refresh_token: &str) {
    let Some(marker) = signed_out_marker(source_home) else {
        return;
    };
    let write = || -> std::io::Result<()> {
        if let Some(dir) = marker.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&marker, hex_digest(refresh_token.as_bytes()))
    };
    if let Err(e) = write() {
        tracing::warn!(error = %e, "could not record the codex login as signed out");
    }
}

/// Whether the login stored as `auth_json` in `source_home` is one a refresh
/// already found dead. For the sign-in probe: the file still has tokens, but
/// they no longer work.
pub(crate) fn marked_signed_out(source_home: &Path, auth_json: Option<&[u8]>) -> bool {
    let Some(marker) = signed_out_marker(source_home) else {
        return false;
    };
    let Ok(mark) = std::fs::read_to_string(&marker) else {
        return false;
    };
    let Some(auth) = auth_json.and_then(|b| serde_json::from_slice::<Value>(b).ok()) else {
        return false;
    };
    refresh_token(&auth).is_some_and(|t| hex_digest(t.as_bytes()) == mark.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    fn write_login(dir: &Path, auth: &Value) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(AUTH_FILE), auth.to_string()).unwrap();
    }

    fn read(path: &Path) -> Value {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    fn never(_: &str) -> std::result::Result<TokenResponse, RefreshFailure> {
        panic!("no refresh expected")
    }

    fn rotated() -> TokenResponse {
        TokenResponse {
            access_token: Some(jwt(NOW, NOW + 10 * DAY)),
            id_token: Some("id.new".into()),
            refresh_token: Some("rt.rotated".into()),
        }
    }

    /// A source home and an overlay inside an agent dir, in a fresh tempdir.
    fn dirs() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let td = tempfile::tempdir().unwrap();
        let source = td.path().join("src");
        let overlay = overlay_in(&td.path().join("agent"));
        (td, source, overlay)
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
    fn a_fresh_login_is_copied_into_the_overlay_without_its_refresh_token() {
        let (_td, source, overlay) = dirs();
        let auth = login(NOW + 5 * DAY);
        write_login(&source, &auth);

        write_launch_credential(&source, &overlay, &never, NOW).unwrap();

        let written = read(&overlay.join(AUTH_FILE));
        assert_eq!(written, launch_credential(&auth));
        assert_eq!(read(&source.join(AUTH_FILE)), auth);
    }

    #[test]
    fn a_near_expiry_login_is_refreshed_on_the_host_and_the_rotation_persisted() {
        let (_td, source, overlay) = dirs();
        write_login(&source, &login(NOW + 3600));
        let seen = Mutex::new(String::new());
        let refresh = |rt: &str| {
            *seen.lock() = rt.to_string();
            Ok(rotated())
        };

        write_launch_credential(&source, &overlay, &refresh, NOW).unwrap();

        assert_eq!(*seen.lock(), "rt.original");
        let host = read(&source.join(AUTH_FILE));
        assert_eq!(host["tokens"]["refresh_token"], "rt.rotated");
        assert_eq!(host["tokens"]["id_token"], "id.new");
        assert_eq!(host["tokens"]["future_token_field"], 7);
        assert_eq!(host["future_top_level"], json!({"kept": true}));
        assert_eq!(host["last_refresh"], "2026-10-08T08:29:31.000000Z");
        let launch = read(&overlay.join(AUTH_FILE));
        assert_eq!(launch["tokens"]["refresh_token"], "");
        assert_eq!(
            launch["tokens"]["access_token"],
            host["tokens"]["access_token"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn credential_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let (_td, source, overlay) = dirs();
        write_login(&source, &login(NOW + 3600));
        write_launch_credential(&source, &overlay, &|_| Ok(rotated()), NOW).unwrap();
        for path in [source.join(AUTH_FILE), overlay.join(AUTH_FILE)] {
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{}", path.display());
        }
    }

    #[test]
    fn concurrent_launches_under_one_login_refresh_once() {
        let td = tempfile::tempdir().unwrap();
        let source = td.path().join("src");
        write_login(&source, &login(NOW + 3600));
        let calls = AtomicUsize::new(0);
        let refresh = |_: &str| {
            calls.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(50));
            Ok(rotated())
        };
        std::thread::scope(|s| {
            for i in 0..4 {
                let overlay = overlay_in(&td.path().join(format!("agent{i}")));
                let (source, refresh) = (&source, &refresh);
                s.spawn(move || write_launch_credential(source, &overlay, refresh, NOW).unwrap());
            }
        });
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_revoked_refresh_fails_the_launch_and_withdraws_the_overlay_credential() {
        accounts::with_test_root(|root| {
            let source = root.join("codex/work");
            let overlay = overlay_in(&root.join("agent"));
            write_login(&source, &login(NOW - 1));
            open_overlay(&overlay).unwrap();
            std::fs::write(overlay.join(AUTH_FILE), "stale").unwrap();

            let err =
                write_launch_credential(&source, &overlay, &|_| Err(RefreshFailure::Rejected), NOW)
                    .unwrap_err();

            assert!(err.to_string().contains("Sign in again"), "{err}");
            assert!(!overlay.join(AUTH_FILE).exists());
        });
    }

    /// The probe reads a revoked login as signed out until a new sign-in
    /// replaces its refresh token.
    #[test]
    fn a_revoked_login_reads_signed_out_until_a_new_sign_in() {
        accounts::with_test_root(|root| {
            let source = root.join("codex/work");
            let overlay = overlay_in(&root.join("agent"));
            write_login(&source, &login(NOW - 1));
            let probe = || super::super::auth_probe::probe_dir("codex", &source).status;
            assert_eq!(probe(), super::super::AuthStatus::SignedIn);

            let _ =
                write_launch_credential(&source, &overlay, &|_| Err(RefreshFailure::Rejected), NOW);
            assert_eq!(probe(), super::super::AuthStatus::SignedOut);

            let mut relogged = login(NOW + 5 * DAY);
            relogged["tokens"]["refresh_token"] = Value::String("rt.new-login".into());
            write_login(&source, &relogged);
            assert_eq!(probe(), super::super::AuthStatus::SignedIn);
        });
    }

    /// Another process rotated the login between this read and the POST: the
    /// refusal of the spent token is not the login's death, and the rotated
    /// token is used instead.
    #[test]
    fn a_refusal_after_a_concurrent_rotation_retries_with_the_new_token() {
        accounts::with_test_root(|root| {
            let source = root.join("codex/work");
            let overlay = overlay_in(&root.join("agent"));
            write_login(&source, &login(NOW + 3600));
            let sent = Mutex::new(Vec::new());
            let refresh = |rt: &str| {
                sent.lock().push(rt.to_string());
                if rt == "rt.original" {
                    let mut moved = login(NOW + 3600);
                    moved["tokens"]["refresh_token"] = Value::String("rt.elsewhere".into());
                    write_login(&source, &moved);
                    Err(RefreshFailure::Rejected)
                } else {
                    Ok(rotated())
                }
            };

            write_launch_credential(&source, &overlay, &refresh, NOW).unwrap();

            assert_eq!(*sent.lock(), vec!["rt.original", "rt.elsewhere"]);
            assert_eq!(
                read(&source.join(AUTH_FILE))["tokens"]["refresh_token"],
                "rt.rotated"
            );
            let stored = std::fs::read(source.join(AUTH_FILE)).unwrap();
            assert!(!marked_signed_out(&source, Some(&stored)));
        });
    }

    /// Makes the source home unwritable (so a save fails) until dropped.
    #[cfg(unix)]
    struct ReadOnly<'a>(&'a Path);

    #[cfg(unix)]
    impl<'a> ReadOnly<'a> {
        fn set(dir: &'a Path) -> Self {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
            Self(dir)
        }
    }

    #[cfg(unix)]
    impl Drop for ReadOnly<'_> {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o755));
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_rotation_that_cannot_be_saved_is_kept_and_used_by_the_next_launch() {
        let (_td, source, overlay) = dirs();
        write_login(&source, &login(NOW + 3600));
        {
            let _ro = ReadOnly::set(&source);
            write_launch_credential(&source, &overlay, &|_| Ok(rotated()), NOW).unwrap();
            assert_eq!(
                read(&source.join(AUTH_FILE))["tokens"]["refresh_token"],
                "rt.original"
            );

            write_launch_credential(&source, &overlay, &never, NOW).unwrap();
        }

        let launch = read(&overlay.join(AUTH_FILE));
        assert_eq!(
            launch["tokens"]["access_token"],
            json!(jwt(NOW, NOW + 10 * DAY))
        );
        assert_eq!(launch["tokens"]["refresh_token"], "");
    }

    #[cfg(unix)]
    #[test]
    fn a_kept_rotation_is_saved_once_the_store_takes_it() {
        let (_td, source, overlay) = dirs();
        write_login(&source, &login(NOW + 3600));
        {
            let _ro = ReadOnly::set(&source);
            write_launch_credential(&source, &overlay, &|_| Ok(rotated()), NOW).unwrap();
        }

        write_launch_credential(&source, &overlay, &never, NOW).unwrap();

        assert_eq!(
            read(&source.join(AUTH_FILE))["tokens"]["refresh_token"],
            "rt.rotated"
        );
        write_launch_credential(&source, &overlay, &never, NOW).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_sign_in_after_a_failed_save_wins_over_the_kept_rotation() {
        let (_td, source, overlay) = dirs();
        write_login(&source, &login(NOW + 3600));
        {
            let _ro = ReadOnly::set(&source);
            write_launch_credential(&source, &overlay, &|_| Ok(rotated()), NOW).unwrap();
        }
        let mut signed_in = login(NOW + 9 * DAY);
        signed_in["tokens"]["refresh_token"] = Value::String("rt.new-sign-in".into());
        signed_in["tokens"]["account_id"] = Value::String("acct-after-sign-in".into());
        write_login(&source, &signed_in);

        write_launch_credential(&source, &overlay, &never, NOW).unwrap();

        assert_eq!(read(&source.join(AUTH_FILE)), signed_in);
        assert_eq!(
            read(&overlay.join(AUTH_FILE))["tokens"]["account_id"],
            "acct-after-sign-in"
        );
    }

    #[test]
    fn a_transient_failure_still_launches_while_the_token_is_valid() {
        let (_td, source, overlay) = dirs();
        write_login(&source, &login(NOW + 3600));
        let offline = |_: &str| Err(RefreshFailure::Failed("offline".into()));

        write_launch_credential(&source, &overlay, &offline, NOW).unwrap();

        assert_eq!(
            read(&overlay.join(AUTH_FILE))["tokens"]["refresh_token"],
            ""
        );
    }

    #[test]
    fn a_transient_failure_with_an_expired_token_fails_the_turn() {
        let (_td, source, overlay) = dirs();
        write_login(&source, &login(NOW - 1));
        let offline = |_: &str| Err(RefreshFailure::Failed("offline".into()));

        let err = write_launch_credential(&source, &overlay, &offline, NOW).unwrap_err();

        assert!(err.to_string().contains("offline"), "{err}");
    }

    #[test]
    fn no_host_login_leaves_no_credential_in_the_overlay() {
        let (_td, source, overlay) = dirs();
        open_overlay(&overlay).unwrap();
        std::fs::write(overlay.join(AUTH_FILE), "{}").unwrap();
        write_launch_credential(&source, &overlay, &never, NOW).unwrap();
        assert!(!overlay.join(AUTH_FILE).exists());
    }

    /// Between turns the agent swaps its overlay for a link to the host's
    /// codex home. The next turn's write lands in a real overlay again and the
    /// host login it would have hit is untouched.
    #[cfg(unix)]
    #[test]
    fn a_turn_after_the_overlay_was_swapped_for_a_link_never_writes_through_it() {
        let (td, source, overlay) = dirs();
        let auth = login(NOW + 5 * DAY);
        write_login(&source, &auth);
        write_launch_credential(&source, &overlay, &never, NOW).unwrap();
        std::fs::rename(&overlay, td.path().join("moved")).unwrap();
        std::os::unix::fs::symlink(&source, &overlay).unwrap();

        write_launch_credential(&source, &overlay, &never, NOW).unwrap();

        assert_eq!(read(&source.join(AUTH_FILE)), auth);
        assert!(overlay.symlink_metadata().unwrap().is_dir());
        assert_eq!(
            read(&overlay.join(AUTH_FILE))["tokens"]["refresh_token"],
            ""
        );
    }

    /// The same with the credential file itself replaced by a link.
    #[cfg(unix)]
    #[test]
    fn a_turn_replaces_a_planted_credential_link_instead_of_writing_through_it() {
        let (_td, source, overlay) = dirs();
        let auth = login(NOW + 5 * DAY);
        write_login(&source, &auth);
        open_overlay(&overlay).unwrap();
        std::os::unix::fs::symlink(source.join(AUTH_FILE), overlay.join(AUTH_FILE)).unwrap();

        write_launch_credential(&source, &overlay, &never, NOW).unwrap();

        assert_eq!(read(&source.join(AUTH_FILE)), auth);
        assert!(!overlay
            .join(AUTH_FILE)
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
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

    #[test]
    fn the_overlay_relinks_shared_config_and_replaces_what_the_agent_put_there() {
        let td = tempfile::tempdir().unwrap();
        let home = td.path().join("home");
        let shared = home.join(".codex");
        std::fs::create_dir_all(shared.join("skills")).unwrap();
        std::fs::write(shared.join("config.toml"), "model = \"x\"").unwrap();
        let overlay = overlay_in(&td.path().join("root"));
        std::fs::create_dir_all(&overlay).unwrap();
        std::fs::write(overlay.join("config.toml"), "[mcp_servers.evil]").unwrap();

        prepare_overlay(&overlay, &home).unwrap();

        assert!(overlay.join("sessions").is_dir());
        for item in ["config.toml", "skills"] {
            assert_eq!(
                std::fs::read_link(overlay.join(item)).unwrap(),
                shared.join(item)
            );
        }
        assert!(overlay.join("AGENTS.md").symlink_metadata().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn an_overlay_swapped_for_a_symlink_is_recreated_as_a_directory() {
        let td = tempfile::tempdir().unwrap();
        let home = td.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let elsewhere = td.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let overlay = overlay_in(td.path());
        std::os::unix::fs::symlink(&elsewhere, &overlay).unwrap();

        prepare_overlay(&overlay, &home).unwrap();

        assert!(overlay.symlink_metadata().unwrap().file_type().is_dir());
        assert!(!elsewhere.join("sessions").exists());
    }
}
