//! OAuth via the Device Authorization Grant (RFC 8628). GitHub and Google
//! are both supported.
//!
//! Google is identity-only: its token reads the profile once, then drops.
//! GitHub requests the `repo` scope and its token is persisted (the app's
//! secret store + the in-process registry) — it powers the GitHub API client
//! and git's https auth, replacing the `gh` CLI dependency.
//!
//! A device flow has no loopback listener and no browser dependency: the user
//! is shown a code and a URL and types it in on whatever device they like.
//! That is what makes it the one sign-in a headless host can complete, so the
//! flow lives here rather than in the desktop shell. What the shell keeps is
//! the two things only it can do: the OAuth client id (baked into the app
//! binary at compile time) and telling the user about the code — the desktop
//! emits an event to its window, `fletch-host` prints it and its
//! `github_login_status` op hands it to a client.

use std::time::{Duration, Instant};

use serde::Serialize;

use crate::DbState;

#[derive(Debug, Clone, Serialize)]
pub struct OAuthProfile {
    pub provider: String,
    pub provider_user_id: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub avatar_url: Option<String>,
}

/// What the provider wants the user to do, handed to the caller's `on_code`
/// the moment the provider answers. `expires_in` is how long the code stays
/// good, so a client that polls can tell "still waiting" from "too late".
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCode {
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
}

pub enum PollOutcome {
    Token(String),
    Pending,
    SlowDown,
    Failed(String),
}

/// The OAuth client this host signs in as. Baked into each binary at compile
/// time (`option_env!`) rather than resolved here: a macOS .app launched from
/// Finder gets a minimal environment, so the desktop cannot read it at runtime,
/// and a headless host has no business carrying the desktop's id by default.
pub struct Credentials {
    pub client_id: String,
    /// Google only — required by its device endpoint but, per Google's docs,
    /// not treated as confidential for "limited-input device" clients.
    pub client_secret: Option<String>,
}

struct ProviderCfg {
    device_code_url: &'static str,
    token_url: &'static str,
    scope: &'static str,
    creds: Credentials,
}

fn provider_cfg(provider: &str, creds: Credentials) -> Result<ProviderCfg, String> {
    match provider {
        "github" => Ok(ProviderCfg {
            device_code_url: "https://github.com/login/device/code",
            token_url: "https://github.com/login/oauth/access_token",
            // `repo` powers PR/clone/push via the API + git https auth; the
            // read scopes cover the profile (name, primary email).
            scope: "repo read:user user:email",
            creds,
        }),
        "google" => Ok(ProviderCfg {
            device_code_url: "https://oauth2.googleapis.com/device/code",
            token_url: "https://oauth2.googleapis.com/token",
            scope: "openid email profile",
            creds,
        }),
        other => Err(format!("unknown provider: {other}")),
    }
}

fn str_field(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(|s| s.to_string())
}

pub fn github_profile(user: &serde_json::Value, emails: &serde_json::Value) -> OAuthProfile {
    // /user/emails carries the authoritative primary address (the /user email
    // is often null for accounts that keep it private).
    let email = emails
        .as_array()
        .and_then(|arr| {
            arr.iter()
                .find(|e| e.get("primary").and_then(|p| p.as_bool()).unwrap_or(false))
                .or_else(|| arr.first())
        })
        .and_then(|e| str_field(e, "email"))
        .or_else(|| str_field(user, "email"));
    OAuthProfile {
        provider: "github".into(),
        provider_user_id: user
            .get("id")
            .map(|v| v.to_string())
            .unwrap_or_default()
            .trim_matches('"')
            .to_string(),
        name: str_field(user, "name").or_else(|| str_field(user, "login")),
        email,
        avatar_url: str_field(user, "avatar_url"),
    }
}

pub fn google_profile(info: &serde_json::Value) -> OAuthProfile {
    OAuthProfile {
        provider: "google".into(),
        provider_user_id: str_field(info, "sub").unwrap_or_default(),
        name: str_field(info, "name"),
        email: str_field(info, "email"),
        avatar_url: str_field(info, "picture"),
    }
}

/// Pixel size we request when caching an avatar. Large enough to stay crisp on
/// retina displays, small enough to keep the base64 blob tiny.
const AVATAR_SIZE: u32 = 256;
/// Refuse to cache anything larger than this — guards the DB against a rogue
/// or mis-typed response. A 256px avatar is well under 100 KB.
const AVATAR_MAX_BYTES: usize = 2 * 1024 * 1024;
/// Hard cap on the avatar download. It runs *after* the user has approved the
/// login, and is purely best-effort, so a stalled CDN must degrade to initials
/// rather than hang `device_login` forever.
const AVATAR_TIMEOUT: Duration = Duration::from_secs(10);

/// Rewrite a provider avatar URL to request our preferred pixel size, so the
/// bytes we download (and cache) are small. Unknown providers pass through.
fn sized_avatar_url(provider: &str, url: &str) -> String {
    match provider {
        // GitHub honours an `s=` query param.
        "github" => {
            let sep = if url.contains('?') { '&' } else { '?' };
            format!("{url}{sep}s={AVATAR_SIZE}")
        }
        // Google photo URLs carry a trailing size directive like `=s96-c`.
        "google" => match url.rfind("=s") {
            Some(i) => format!("{}=s{AVATAR_SIZE}-c", &url[..i]),
            None => format!("{url}=s{AVATAR_SIZE}-c"),
        },
        _ => url.to_string(),
    }
}

/// Resolve a response `Content-Type` to an image MIME, or `None` if it is not
/// an image. This guards against a CDN answering an avatar URL with text/html
/// or JSON (an error page, a redirect-to-login): without it we would base64
/// non-image bytes straight into the database. A blank type on an otherwise-OK
/// response is assumed to be PNG.
fn image_mime(content_type: &str) -> Option<String> {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if mime.is_empty() {
        return Some("image/png".to_string());
    }
    mime.starts_with("image/").then_some(mime)
}

/// Assemble a self-contained `data:` URI from image bytes so the avatar can be
/// stored in SQLite and rendered without a network round-trip (or CSP concerns).
fn to_data_uri(mime: &str, bytes: &[u8]) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine};
    format!("data:{mime};base64,{}", STANDARD.encode(bytes))
}

/// Download an avatar and return it as a `data:` URI. Best-effort: any failure
/// (network, non-success status, empty/oversized body) yields `None`, and the
/// caller falls back to initials.
async fn fetch_avatar_data_uri(
    http: &reqwest::Client,
    provider: &str,
    url: &str,
) -> Option<String> {
    let resp = http
        .get(sized_avatar_url(provider, url))
        .timeout(AVATAR_TIMEOUT)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    // Reject non-image responses before downloading the body, so a CDN error
    // page is never base64-encoded into the database.
    let mime = image_mime(
        resp.headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or(""),
    )?;
    let bytes = resp.bytes().await.ok()?;
    if bytes.is_empty() || bytes.len() > AVATAR_MAX_BYTES {
        return None;
    }
    Some(to_data_uri(&mime, &bytes))
}

pub fn classify_token_response(v: &serde_json::Value) -> PollOutcome {
    if let Some(tok) = str_field(v, "access_token") {
        return PollOutcome::Token(tok);
    }
    match v.get("error").and_then(|e| e.as_str()) {
        Some("authorization_pending") => PollOutcome::Pending,
        Some("slow_down") => PollOutcome::SlowDown,
        Some(other) => PollOutcome::Failed(other.to_string()),
        None => PollOutcome::Failed("malformed token response".into()),
    }
}

/// Drive the full device flow and return a normalized profile. A GitHub
/// token is persisted for the API client (see module docs); Google's is read
/// once and dropped.
///
/// `on_code` is called once, as soon as the provider has issued a code and
/// before the first poll. It is the host's only job in this flow: show the code
/// to whoever is signing in.
pub async fn device_login(
    db: &DbState,
    provider: &str,
    creds: Credentials,
    on_code: impl FnOnce(&DeviceCode),
) -> Result<OAuthProfile, String> {
    let cfg = provider_cfg(provider, creds)?;
    let http = reqwest::Client::builder()
        .user_agent("Fletch")
        .build()
        .map_err(|e| e.to_string())?;

    // 1. Request a device + user code.
    let dc: serde_json::Value = http
        .post(cfg.device_code_url)
        .header("Accept", "application/json")
        .form(&[
            ("client_id", cfg.creds.client_id.as_str()),
            ("scope", cfg.scope),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let device_code = match str_field(&dc, "device_code") {
        Some(c) => c,
        None => {
            // No device_code means the provider rejected the request — surface
            // its error/description (and the raw body) instead of a blind miss.
            let err = str_field(&dc, "error").unwrap_or_default();
            let desc = str_field(&dc, "error_description").unwrap_or_default();
            return Err(format!(
                "device code request rejected: {err} {desc} (raw: {dc})"
            ));
        }
    };
    let user_code = str_field(&dc, "user_code").ok_or("no user_code in response")?;
    // GitHub uses `verification_uri`; Google uses `verification_url`.
    let verification = str_field(&dc, "verification_uri")
        .or_else(|| str_field(&dc, "verification_url"))
        .ok_or("no verification uri in response")?;
    let mut interval = dc.get("interval").and_then(|v| v.as_u64()).unwrap_or(5);
    // Don't poll past the device code's lifetime (GitHub ~900s, Google ~1800s).
    let expires_in = dc.get("expires_in").and_then(|v| v.as_u64()).unwrap_or(900);
    let deadline = Instant::now() + Duration::from_secs(expires_in);

    // 2. Tell whoever is signing in to enter the code (and open the browser, if
    //    this host has one).
    on_code(&DeviceCode {
        user_code,
        verification_uri: verification,
        expires_in,
    });

    // 3. Poll the token endpoint until the user approves (or it fails).
    let access_token = loop {
        if Instant::now() >= deadline {
            return Err("device code expired before authorization".into());
        }
        tokio::time::sleep(Duration::from_secs(interval)).await;
        let mut poll: Vec<(&str, &str)> = vec![
            ("client_id", cfg.creds.client_id.as_str()),
            ("device_code", device_code.as_str()),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ];
        if let Some(secret) = &cfg.creds.client_secret {
            poll.push(("client_secret", secret.as_str()));
        }
        let resp: serde_json::Value = http
            .post(cfg.token_url)
            .header("Accept", "application/json")
            .form(&poll)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        match classify_token_response(&resp) {
            PollOutcome::Token(t) => break t,
            PollOutcome::Pending => {}
            PollOutcome::SlowDown => interval += 5,
            PollOutcome::Failed(e) => return Err(format!("authorization failed: {e}")),
        }
    };

    // Persist the GitHub token — it authenticates the API client and git's
    // https transport from now on. A failed write fails the sign-in: the
    // store is the Keychain on release macOS, which can refuse writes (e.g.
    // locked keychain), and proceeding would look signed in until quit, then
    // silently sign out on next launch. Same posture as
    // `store_container_token`; the mirror is only set once storage succeeded.
    if provider == "github" {
        crate::secrets::set(&db.lock(), crate::github::TOKEN_SETTING, &access_token)
            .map_err(|e| format!("couldn't save your GitHub login: {e}"))?;
        crate::github::set_token(Some(access_token.clone()));
    }

    // 4. Read the profile (Google's token drops out of scope after this).
    let profile = match provider {
        "github" => {
            let user: serde_json::Value = http
                .get("https://api.github.com/user")
                .bearer_auth(&access_token)
                .header("Accept", "application/vnd.github+json")
                .send()
                .await
                .map_err(|e| e.to_string())?
                .json()
                .await
                .map_err(|e| e.to_string())?;
            let emails: serde_json::Value = http
                .get("https://api.github.com/user/emails")
                .bearer_auth(&access_token)
                .header("Accept", "application/vnd.github+json")
                .send()
                .await
                .map_err(|e| e.to_string())?
                .json()
                .await
                .unwrap_or_else(|_| serde_json::json!([]));
            github_profile(&user, &emails)
        }
        "google" => {
            let info: serde_json::Value = http
                .get("https://openidconnect.googleapis.com/v1/userinfo")
                .bearer_auth(&access_token)
                .send()
                .await
                .map_err(|e| e.to_string())?
                .json()
                .await
                .map_err(|e| e.to_string())?;
            google_profile(&info)
        }
        _ => return Err("unknown provider".into()),
    };

    // 5. Cache the avatar as a self-contained data URI so it can be stored in
    //    SQLite and rendered offline. On any failure we drop it (initials show).
    let mut profile = profile;
    if let Some(url) = profile.avatar_url.take() {
        profile.avatar_url = fetch_avatar_data_uri(&http, provider, &url).await;
    }
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_user_into_profile() {
        let body = serde_json::json!({
            "id": 4242, "login": "octocat",
            "name": "Octo Cat", "avatar_url": "https://x/y.png"
        });
        let emails = serde_json::json!([
            {"email":"a@b.com","primary":false,"verified":true},
            {"email":"octo@github.com","primary":true,"verified":true}
        ]);
        let p = github_profile(&body, &emails);
        assert_eq!(p.provider, "github");
        assert_eq!(p.provider_user_id, "4242");
        assert_eq!(p.name.as_deref(), Some("Octo Cat"));
        assert_eq!(p.email.as_deref(), Some("octo@github.com")); // primary wins
        assert_eq!(p.avatar_url.as_deref(), Some("https://x/y.png"));
    }

    #[test]
    fn github_profile_falls_back_to_login_when_name_missing() {
        let body = serde_json::json!({ "id": 7, "login": "ghost" });
        let p = github_profile(&body, &serde_json::json!([]));
        assert_eq!(p.name.as_deref(), Some("ghost"));
        assert_eq!(p.email, None);
    }

    #[test]
    fn parses_google_userinfo_into_profile() {
        let body = serde_json::json!({
            "sub": "11822", "name": "Jane Doe",
            "email": "jane@gmail.com", "picture": "https://g/p.png"
        });
        let p = google_profile(&body);
        assert_eq!(p.provider, "google");
        assert_eq!(p.provider_user_id, "11822");
        assert_eq!(p.email.as_deref(), Some("jane@gmail.com"));
    }

    #[test]
    fn sizes_github_avatar_url() {
        // No existing query → add one.
        assert_eq!(
            sized_avatar_url("github", "https://avatars.githubusercontent.com/u/42"),
            "https://avatars.githubusercontent.com/u/42?s=256"
        );
        // Existing query (e.g. ?v=4) → append.
        assert_eq!(
            sized_avatar_url("github", "https://avatars.githubusercontent.com/u/42?v=4"),
            "https://avatars.githubusercontent.com/u/42?v=4&s=256"
        );
    }

    #[test]
    fn sizes_google_avatar_url() {
        // Replace an existing size directive.
        assert_eq!(
            sized_avatar_url("google", "https://lh3.googleusercontent.com/a/ABC=s96-c"),
            "https://lh3.googleusercontent.com/a/ABC=s256-c"
        );
        // Append when there is none.
        assert_eq!(
            sized_avatar_url("google", "https://lh3.googleusercontent.com/a/ABC"),
            "https://lh3.googleusercontent.com/a/ABC=s256-c"
        );
    }

    #[test]
    fn unknown_provider_url_passes_through() {
        assert_eq!(
            sized_avatar_url("other", "https://x/y.png"),
            "https://x/y.png"
        );
    }

    #[test]
    fn image_mime_accepts_images_and_rejects_others() {
        assert_eq!(image_mime("image/png").as_deref(), Some("image/png"));
        // Strips charset params and lowercases.
        assert_eq!(
            image_mime("IMAGE/JPEG; charset=binary").as_deref(),
            Some("image/jpeg")
        );
        // Blank type on an OK response is assumed to be an image (PNG).
        assert_eq!(image_mime("").as_deref(), Some("image/png"));
        // Non-image types are rejected — they must not reach the database.
        assert_eq!(image_mime("text/html"), None);
        assert_eq!(image_mime("application/json"), None);
    }

    #[test]
    fn builds_data_uri_from_bytes() {
        // "hi" → base64 "aGk=".
        assert_eq!(
            to_data_uri("image/png", b"hi"),
            "data:image/png;base64,aGk="
        );
        assert_eq!(
            to_data_uri("image/jpeg", b"hi"),
            "data:image/jpeg;base64,aGk="
        );
    }

    #[test]
    fn classifies_pending_and_terminal_poll_states() {
        assert!(matches!(
            classify_token_response(&serde_json::json!({"error":"authorization_pending"})),
            PollOutcome::Pending
        ));
        assert!(matches!(
            classify_token_response(&serde_json::json!({"error":"slow_down"})),
            PollOutcome::SlowDown
        ));
        assert!(matches!(
            classify_token_response(&serde_json::json!({"error":"access_denied"})),
            PollOutcome::Failed(_)
        ));
        match classify_token_response(&serde_json::json!({"access_token":"tok_123"})) {
            PollOutcome::Token(t) => assert_eq!(t, "tok_123"),
            _ => panic!("expected token"),
        }
    }
}
