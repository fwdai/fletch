//! Claude limits on demand: Claude's OAuth usage endpoint, asked with the
//! account's own access token when — and only when — the user presses Refresh.
//!
//! The endpoint is undocumented and rate-limits eagerly, so this never polls:
//! the command layer gates every call behind the refresh floor and a persisted
//! 429 back-off (`super::refresh_allowed`). The token is the host's login
//! (`agent::claude_oauth`), refreshed first when it is due, so a lapsed token
//! no longer needs an agent run to come back.
//!
//! The token is sent in one header and dropped. It is never logged, never
//! part of an error and never returned.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use super::{from_oauth_usage, now_secs, RefreshOutcome};
use crate::agent::claude_oauth::{self, AccessToken, LoginError};
use crate::error::{Error, Result};

pub const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

/// The beta flag the endpoint requires of OAuth callers.
const OAUTH_BETA: &str = "oauth-2025-04-20";

const TIMEOUT: Duration = Duration::from_secs(15);

/// Read the limits of the claude account whose config dir is `account_dir`
/// (`None` = the default account). A 401 on a token that looked unexpired
/// (revoked server-side) gets one forced refresh and a second ask. No login,
/// or a revoked one, reads as signed out.
pub async fn read_limits(account_dir: Option<PathBuf>) -> Result<RefreshOutcome> {
    let dir = account_dir.as_deref();
    let Some(token) = signed_in(claude_oauth::access_token_for_launch(dir).await)? else {
        return Ok(RefreshOutcome::SignedOut);
    };
    let client = reqwest::Client::builder()
        .user_agent("Fletch")
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| Error::Other(format!("http client: {e}")))?;
    let (status, body) = get_usage(&client, &token).await?;
    if status != 401 {
        return interpret(status, body.as_ref(), now_secs());
    }
    let replaced = claude_oauth::replace_rejected_token(dir, token.expires_at_ms()).await;
    let Some(token) = signed_in(replaced)? else {
        return Ok(RefreshOutcome::SignedOut);
    };
    let (status, body) = get_usage(&client, &token).await?;
    interpret(status, body.as_ref(), now_secs())
}

fn signed_in(token: std::result::Result<AccessToken, LoginError>) -> Result<Option<AccessToken>> {
    match token {
        Ok(token) => Ok(Some(token)),
        Err(LoginError::SignedOut | LoginError::Revoked) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

async fn get_usage(client: &reqwest::Client, token: &AccessToken) -> Result<(u16, Option<Value>)> {
    // A transport error's text names the URL, never the request's headers.
    let response = client
        .get(USAGE_URL)
        .bearer_auth(token.secret())
        .header("anthropic-beta", OAUTH_BETA)
        .send()
        .await
        .map_err(|e| Error::Other(format!("Claude's usage endpoint is unreachable: {e}")))?;
    let status = response.status().as_u16();
    let body = if response.status().is_success() {
        response.json::<Value>().await.ok()
    } else {
        None
    };
    Ok((status, body))
}

/// What an answer means. 401 even after a forced refresh: stale, not signed
/// out, since the login itself is still there. 429: back off. Any other
/// failure is an error the user sees; it carries the status only, never the
/// body.
pub fn interpret(status: u16, body: Option<&Value>, now: i64) -> Result<RefreshOutcome> {
    match status {
        200..=299 => body
            .and_then(|b| from_oauth_usage(b, now))
            .map(RefreshOutcome::Limits)
            .ok_or_else(|| Error::Other("Claude's usage endpoint answered without limits.".into())),
        401 => Ok(RefreshOutcome::Stale),
        429 => Ok(RefreshOutcome::RateLimited),
        403 => Err(Error::Other(
            "This Claude login isn't allowed to read its usage (403).".into(),
        )),
        other => Err(Error::Other(format!(
            "Claude's usage endpoint answered {other}."
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        next_refresh_state, refresh_allowed, LimitSource, LimitWindow, RefreshStatus,
    };
    use super::*;
    use serde_json::json;

    fn usage_fixture() -> Value {
        json!({
            "five_hour": { "utilization": 37.0, "resets_at": "2026-10-07T14:30:00+00:00" },
            "seven_day": { "utilization": 12.0, "resets_at": "2026-10-10T08:00:00+00:00" },
            "seven_day_opus": { "utilization": 0.0, "resets_at": null },
            "seven_day_sonnet": null,
            "extra_usage": { "is_enabled": false, "monthly_limit": null },
        })
    }

    #[test]
    fn a_200_becomes_an_oauth_usage_reading() {
        let RefreshOutcome::Limits(limits) = interpret(200, Some(&usage_fixture()), 9).unwrap()
        else {
            panic!("expected a reading");
        };
        assert_eq!(limits.source, LimitSource::OauthUsage);
        assert_eq!(limits.as_of, 9);
        assert_eq!(
            limits.five_hour,
            Some(LimitWindow {
                percent: 37.0,
                resets_at: Some(1_791_383_400)
            })
        );
        assert_eq!(
            limits.seven_day.map(|w| w.resets_at),
            Some(Some(1_791_619_200))
        );
    }

    #[test]
    fn a_200_without_windows_is_an_error() {
        assert!(interpret(200, Some(&json!({})), 0).is_err());
        assert!(interpret(200, None, 0).is_err());
    }

    #[test]
    fn a_401_is_a_stale_token_and_retryable_at_once() {
        let outcome = interpret(401, None, 0).unwrap();
        assert_eq!(outcome, RefreshOutcome::Stale);
        let state = next_refresh_state(None, &outcome, 100);
        assert_eq!(state.status, RefreshStatus::Stale);
        assert!(refresh_allowed(Some(&state), 101));
    }

    #[test]
    fn a_429_backs_off_and_holds_the_next_refresh() {
        let outcome = interpret(429, None, 0).unwrap();
        assert_eq!(outcome, RefreshOutcome::RateLimited);
        let state = next_refresh_state(None, &outcome, 100);
        assert_eq!(state.status, RefreshStatus::RateLimited);
        assert!(!refresh_allowed(Some(&state), 101));
        let next = state.next_allowed_at.expect("a back-off");
        assert!(refresh_allowed(Some(&state), next));
    }

    #[test]
    fn other_failures_name_the_status_only() {
        let err = interpret(500, Some(&json!({"error": "secret-ish body"})), 0)
            .unwrap_err()
            .to_string();
        assert!(err.contains("500"), "{err}");
        assert!(!err.contains("secret-ish"), "{err}");
        assert!(interpret(403, None, 0).is_err());
    }
}
