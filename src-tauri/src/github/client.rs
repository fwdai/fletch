//! HTTP plumbing for the GitHub API: the app-managed token, a thin REST +
//! GraphQL client over it, and the derived auth config for git's own https
//! transport. Endpoint knowledge lives in `github::` (the parent module);
//! everything here is transport.
//!
//! The token comes from the app's own OAuth device flow (`repo` scope) and is
//! stored in the `settings` table — plaintext for now, the same bar as the
//! `gh` CLI's hosts.yml, isolated behind `set_token`/`token` so a move to the
//! OS keychain later touches only the storage call sites. The token must
//! never appear in logs, telemetry, or error strings.

use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{json, Value};

use crate::error::{Error, Result};

/// `settings` key holding the GitHub access token.
pub const TOKEN_SETTING: &str = "github_token";

const API_BASE: &str = "https://api.github.com";
const GRAPHQL_URL: &str = "https://api.github.com/graphql";

/// API-call ceiling. Generous — these move little data, so it only trips on a
/// stalled connection (mirrors `git.rs`'s NET_TIMEOUT rationale).
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// The token, cached in-process. Seeded from the DB at startup and updated on
/// login, so API calls (deep in poll paths, with no DB handle) never touch
/// the DB — the same pattern as `bin_resolve`'s override registry.
fn token_registry() -> &'static RwLock<Option<String>> {
    static TOKEN: OnceLock<RwLock<Option<String>>> = OnceLock::new();
    TOKEN.get_or_init(|| RwLock::new(None))
}

/// Replace the in-process token. Callers that change the *persisted* token
/// (login, disconnect) write the DB and then call this; blank counts as none.
pub fn set_token(token: Option<String>) {
    *token_registry().write().unwrap() = token.filter(|t| !t.trim().is_empty());
}

pub fn token() -> Option<String> {
    token_registry().read().unwrap().clone()
}

// ---------------------------------------------------------------------------
// Rate-limit backoff
//
// GitHub enforces two ceilings the poll paths can trip: the primary hourly
// points budget (5000 for authenticated GraphQL) and a *secondary* limit on
// bursts/concurrency that answers with `403` + `Retry-After`. Rather than keep
// hammering, we honor those signals with a single process-global "don't call
// until" instant. Poll paths (`graphql_opt`, the batch fetchers) check
// [`is_backing_off`] and serve the persisted PR snapshot instead; user-driven
// mutations (create/merge) skip the gate so an explicit action still errors
// loudly instead of silently no-op'ing.
// ---------------------------------------------------------------------------

/// GraphQL points remaining (of ~5000/hr) at or below which we stop polling
/// until the window resets — a cushion so an in-flight batch can't overrun.
const RATE_BUDGET_FLOOR: i64 = 50;
/// Fallback pause when GitHub signals a secondary limit without a `Retry-After`.
const DEFAULT_BACKOFF: Duration = Duration::from_secs(60);

fn backoff_registry() -> &'static RwLock<Option<Instant>> {
    static BACKOFF: OnceLock<RwLock<Option<Instant>>> = OnceLock::new();
    BACKOFF.get_or_init(|| RwLock::new(None))
}

/// Pause API polling for `dur`. Extends an existing pause, never shortens it —
/// two overlapping signals settle on the later deadline.
fn set_backoff(dur: Duration) {
    let until = Instant::now() + dur;
    let mut w = backoff_registry().write().unwrap();
    if w.map_or(true, |cur| until > cur) {
        *w = Some(until);
    }
}

/// True while a rate-limit pause is in effect. Poll paths short-circuit to the
/// last persisted state instead of spending a request that would likely 403.
pub fn is_backing_off() -> bool {
    backoff_registry()
        .read()
        .unwrap()
        .is_some_and(|until| Instant::now() < until)
}

/// Feed the queried GraphQL `rateLimit` budget back into the gate: when the
/// remaining points fall to the floor, pause until the window resets
/// (`reset_at_ms` is GitHub's `resetAt` as ms-epoch).
pub fn note_rate_budget(remaining: i64, reset_at_ms: Option<i64>) {
    if remaining > RATE_BUDGET_FLOOR {
        return;
    }
    let dur = reset_at_ms
        .and_then(|reset| {
            let now = chrono::Utc::now().timestamp_millis();
            (reset > now).then(|| Duration::from_millis((reset - now) as u64))
        })
        .unwrap_or(DEFAULT_BACKOFF);
    set_backoff(dur);
}

/// Inspect a response for rate-limit signals and arm the backoff gate:
/// a secondary-limit `403`/`429` (honoring `Retry-After`), a GraphQL
/// `RATE_LIMITED` error, or an exhausted `x-ratelimit-remaining` header.
fn observe_rate_limit(status: reqwest::StatusCode, headers: &reqwest::header::HeaderMap, body: &Value) {
    let header_secs = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<i64>().ok())
    };

    // Secondary (burst/concurrency) limit: back off for Retry-After, or a
    // conservative default when the header is absent.
    if matches!(status.as_u16(), 403 | 429) {
        let secs = header_secs("retry-after").filter(|s| *s > 0);
        set_backoff(secs.map_or(DEFAULT_BACKOFF, |s| Duration::from_secs(s as u64)));
        return;
    }

    // GraphQL reports its secondary limit as 200 + a RATE_LIMITED error.
    let rate_limited_error = body
        .get("errors")
        .and_then(Value::as_array)
        .is_some_and(|errs| {
            errs.iter().any(|e| {
                e.get("type").and_then(Value::as_str) == Some("RATE_LIMITED")
                    || e.get("message")
                        .and_then(Value::as_str)
                        .is_some_and(|m| m.to_lowercase().contains("rate limit"))
            })
        });
    if rate_limited_error {
        set_backoff(DEFAULT_BACKOFF);
        return;
    }

    // Primary budget exhausted: pause until the reset the header names.
    if header_secs("x-ratelimit-remaining") == Some(0) {
        let dur = header_secs("x-ratelimit-reset")
            .map(|reset| Duration::from_secs((reset - chrono::Utc::now().timestamp()).max(1) as u64))
            .unwrap_or(DEFAULT_BACKOFF);
        set_backoff(dur);
    }
}

/// Git-config env authenticating git's https transport to github.com with the
/// app token — how clone/push/fetch work on a machine with no credential
/// helper. Empty when no token (or for SSH remotes, where the config key
/// simply never matches). Env (`GIT_CONFIG_*`), not `-c` argv, so the token
/// doesn't show up in `ps`.
pub fn git_auth_env() -> Vec<(String, String)> {
    let Some(token) = token() else {
        return Vec::new();
    };
    let basic = base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"));
    vec![
        ("GIT_CONFIG_COUNT".to_string(), "1".to_string()),
        (
            "GIT_CONFIG_KEY_0".to_string(),
            "http.https://github.com/.extraheader".to_string(),
        ),
        (
            "GIT_CONFIG_VALUE_0".to_string(),
            format!("AUTHORIZATION: basic {basic}"),
        ),
    ]
}

/// A REST/GraphQL client bound to the current token. Construction fails with
/// a clear "connect GitHub" error when no token exists, so every endpoint
/// gets that check for free.
pub struct Client {
    http: reqwest::Client,
    token: String,
}

impl Client {
    pub fn new() -> Result<Self> {
        let token = token().ok_or_else(|| {
            Error::Gh("not connected to GitHub — sign in with GitHub to enable this".into())
        })?;
        let http = reqwest::Client::builder()
            .user_agent("Fletch")
            .timeout(HTTP_TIMEOUT)
            .build()
            .map_err(|e| Error::Gh(format!("http client: {e}")))?;
        Ok(Self { http, token })
    }

    fn request(&self, method: reqwest::Method, url: String) -> reqwest::RequestBuilder {
        self.http
            .request(method, url)
            .bearer_auth(&self.token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
    }

    /// REST call returning `(status, body)`. Transport failures error; HTTP
    /// error statuses do NOT — endpoints decide what a 404/422 means (missing
    /// PR vs duplicate vs real failure), so status mapping stays with the
    /// endpoint, not the transport.
    pub async fn rest(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(reqwest::StatusCode, Value)> {
        let mut req = self.request(method, format!("{API_BASE}{path}"));
        if let Some(body) = body {
            req = req.json(body);
        }
        let resp = req.send().await.map_err(request_error)?;
        let status = resp.status();
        // Some successes (e.g. 204 from PUT) have no body; treat as null.
        let body = resp.json::<Value>().await.unwrap_or(Value::Null);
        Ok((status, body))
    }

    /// GraphQL call returning the `data` object. GraphQL reports failures as
    /// 200 + `errors[]`, so those are surfaced as `Error::Gh` with the
    /// server's messages — callers match on the text for expected cases
    /// (e.g. auto-merge's "clean status").
    pub async fn graphql(&self, query: &str, variables: Value) -> Result<Value> {
        self.graphql_inner(query, variables, false).await
    }

    /// Like [`graphql`](Self::graphql) but tolerant of a partial `errors[]`:
    /// returns whatever `data` came back even when some fields errored. For
    /// *batched* queries, one inaccessible repo/PR must null just its own alias
    /// (`data.aN == null`), not fail the whole round-trip and blank every other
    /// agent's state.
    pub async fn graphql_partial(&self, query: &str, variables: Value) -> Result<Value> {
        self.graphql_inner(query, variables, true).await
    }

    async fn graphql_inner(&self, query: &str, variables: Value, allow_partial: bool) -> Result<Value> {
        let resp = self
            .request(reqwest::Method::POST, GRAPHQL_URL.to_string())
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await
            .map_err(request_error)?;
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = resp.json::<Value>().await.unwrap_or(Value::Null);
        observe_rate_limit(status, &headers, &body);
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Error::Gh(
                "GitHub token is no longer valid — sign in with GitHub again".into(),
            ));
        }
        if !status.is_success() {
            return Err(Error::Gh(format!(
                "GraphQL request failed ({status}): {}",
                rest_error_message(&body),
            )));
        }
        if let Some(errors) = body.get("errors").and_then(Value::as_array) {
            if !errors.is_empty() {
                let msgs: Vec<&str> = errors
                    .iter()
                    .filter_map(|e| e.get("message").and_then(Value::as_str))
                    .collect();
                // Partial mode keeps the good aliases; note the rest at debug
                // (never at a level that could leak a token-bearing message).
                if allow_partial && body.get("data").is_some_and(|d| !d.is_null()) {
                    tracing::debug!(errors = %msgs.join("; "), "graphql partial errors");
                } else {
                    return Err(Error::Gh(msgs.join("; ")));
                }
            }
        }
        Ok(body.get("data").cloned().unwrap_or(Value::Null))
    }
}

/// The human-facing `message` from a REST error body, falling back to the raw
/// JSON so an unexpected shape still says *something* actionable.
pub fn rest_error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| body.to_string())
}

/// Map a reqwest transport error without ever embedding the URL's query or
/// headers (reqwest Display doesn't include headers, so the token is safe).
fn request_error(e: reqwest::Error) -> Error {
    Error::Gh(format!("request failed: {e}"))
}

/// Serializes tests that mutate the process-global token registry (the unit
/// tests here and the live test in the parent module), so parallel test
/// threads can't clobber each other's token state.
#[cfg(test)]
pub(crate) fn test_token_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Clear the backoff gate. Tests only — keeps a rate-limit signal from one
/// test leaking into the next (the registry is process-global).
#[cfg(test)]
pub(crate) fn clear_backoff() {
    *backoff_registry().write().unwrap() = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                reqwest::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    /// Serializes the tests that mutate the process-global backoff registry, so
    /// one test's arming can't race another's `clear_backoff()` → `assert!`
    /// window under `cargo test`'s parallel threads (mirrors `test_token_lock`).
    fn test_backoff_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn retry_after_arms_backoff() {
        let _guard = test_backoff_lock();
        clear_backoff();
        assert!(!is_backing_off());
        observe_rate_limit(
            reqwest::StatusCode::FORBIDDEN,
            &headers(&[("retry-after", "30")]),
            &Value::Null,
        );
        assert!(is_backing_off(), "403 + Retry-After must pause polling");
        clear_backoff();
    }

    #[test]
    fn graphql_rate_limited_error_arms_backoff() {
        let _guard = test_backoff_lock();
        clear_backoff();
        let body = json!({ "errors": [{ "type": "RATE_LIMITED", "message": "API rate limit exceeded" }] });
        observe_rate_limit(reqwest::StatusCode::OK, &headers(&[]), &body);
        assert!(is_backing_off(), "GraphQL RATE_LIMITED must pause polling");
        clear_backoff();
    }

    #[test]
    fn healthy_response_does_not_arm_backoff() {
        let _guard = test_backoff_lock();
        clear_backoff();
        observe_rate_limit(
            reqwest::StatusCode::OK,
            &headers(&[("x-ratelimit-remaining", "4999")]),
            &json!({ "data": {} }),
        );
        assert!(!is_backing_off(), "a normal response must not pause polling");
    }

    #[test]
    fn budget_floor_arms_backoff_only_when_low() {
        let _guard = test_backoff_lock();
        clear_backoff();
        note_rate_budget(500, None);
        assert!(!is_backing_off(), "ample budget must not pause");
        note_rate_budget(1, None);
        assert!(is_backing_off(), "budget at the floor must pause");
        clear_backoff();
    }

    /// The git auth header is the documented actions/checkout form: basic
    /// base64("x-access-token:<token>"), scoped to github.com https only.
    #[test]
    fn git_auth_env_encodes_token_and_scopes_to_github() {
        let _guard = test_token_lock();
        set_token(Some("tok123".into()));
        let env = git_auth_env();
        assert_eq!(env[0], ("GIT_CONFIG_COUNT".into(), "1".into()));
        assert_eq!(
            env[1].1, "http.https://github.com/.extraheader",
            "auth must be scoped to github.com, not sent to every host",
        );
        let expected = base64::engine::general_purpose::STANDARD.encode("x-access-token:tok123");
        assert_eq!(env[2].1, format!("AUTHORIZATION: basic {expected}"));

        set_token(None);
        assert!(git_auth_env().is_empty(), "no token → no auth env");
    }

    /// Blank tokens (a cleared setting) must count as signed out.
    #[test]
    fn blank_token_is_none() {
        let _guard = test_token_lock();
        set_token(Some("  ".into()));
        assert_eq!(token(), None);
        set_token(None);
    }
}
