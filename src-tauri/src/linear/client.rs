//! HTTP plumbing for the Linear API: the app-managed API key and a thin
//! GraphQL client over it. Endpoint knowledge lives in the parent module;
//! everything here is transport — the same split as `github::client`.
//!
//! The key is a Linear personal API key the user pastes in Project Settings,
//! persisted via `crate::secrets` (keychain on release macOS, `settings`
//! table in dev) and mirrored in-process behind `set_token`/`token` so API
//! calls never touch the DB. The key must never appear in logs, telemetry,
//! or error strings.

use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use serde_json::{json, Value};

use crate::error::{Error, Result};

/// `crate::secrets` key holding the Linear API key (keychain account name on
/// release macOS; `settings` key in the dev fallback).
pub const TOKEN_SETTING: &str = "linear_token";

const GRAPHQL_URL: &str = "https://api.linear.app/graphql";

/// API-call ceiling — mirrors `github::client::HTTP_TIMEOUT`'s rationale.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// The key, cached in-process. Seeded from the store at startup and updated
/// on connect/disconnect — the same pattern as `github::client`.
fn token_registry() -> &'static RwLock<Option<String>> {
    static TOKEN: OnceLock<RwLock<Option<String>>> = OnceLock::new();
    TOKEN.get_or_init(|| RwLock::new(None))
}

/// Replace the in-process key. Callers that change the *persisted* key
/// (connect, disconnect) write the store and then call this; blank counts
/// as none.
pub fn set_token(token: Option<String>) {
    *token_registry().write().unwrap() = token.filter(|t| !t.trim().is_empty());
}

pub fn token() -> Option<String> {
    token_registry().read().unwrap().clone()
}

/// A GraphQL client bound to a Linear API key. `new` binds the stored key
/// (failing with a clear "connect Linear" error when there is none);
/// `with_key` binds an explicit key, used to validate a pasted key *before*
/// it is persisted.
pub struct Client {
    http: reqwest::Client,
    token: String,
}

impl Client {
    pub fn new() -> Result<Self> {
        let token = token().ok_or_else(|| {
            Error::Other("not connected to Linear — add an API key in Project Settings".into())
        })?;
        Self::with_key(token)
    }

    pub fn with_key(token: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent("Fletch")
            .timeout(HTTP_TIMEOUT)
            .build()
            .map_err(|e| Error::Other(format!("http client: {e}")))?;
        Ok(Self { http, token })
    }

    /// GraphQL call returning the `data` object. Linear reports failures as
    /// 200 + `errors[]`; those surface as errors with the server's messages.
    /// A 401 gets a clear "key no longer valid" message. Transport errors are
    /// mapped without ever embedding headers, so the key can't leak.
    pub async fn graphql(&self, query: &str, variables: Value) -> Result<Value> {
        let resp = self
            .http
            .post(GRAPHQL_URL)
            // A personal API key is sent verbatim (no `Bearer` prefix) — the
            // documented header form for Linear API keys.
            .header("Authorization", &self.token)
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await
            .map_err(|e| Error::Other(format!("linear request failed: {e}")))?;
        let status = resp.status();
        let body = resp.json::<Value>().await.unwrap_or(Value::Null);
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Error::Other(
                "Linear API key is no longer valid — reconnect Linear in Project Settings".into(),
            ));
        }
        if !status.is_success() {
            return Err(Error::Other(format!("linear request failed ({status})")));
        }
        if let Some(errors) = body.get("errors").and_then(Value::as_array) {
            if !errors.is_empty() {
                let msgs: Vec<&str> = errors
                    .iter()
                    .filter_map(|e| e.get("message").and_then(Value::as_str))
                    .collect();
                return Err(Error::Other(format!("linear: {}", msgs.join("; "))));
            }
        }
        Ok(body.get("data").cloned().unwrap_or(Value::Null))
    }
}

/// Serializes tests that mutate the process-global token registry, so
/// parallel test threads can't clobber each other's state (mirrors
/// `github::client::test_token_lock`).
#[cfg(test)]
pub(crate) fn test_token_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Blank keys (a cleared setting) must count as disconnected.
    #[test]
    fn blank_token_is_none() {
        let _guard = test_token_lock();
        set_token(Some("  ".into()));
        assert_eq!(token(), None);
        set_token(None);
    }
}
