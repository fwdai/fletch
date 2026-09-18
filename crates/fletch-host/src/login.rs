//! GitHub sign-in on a machine with no browser.
//!
//! The flow is the engine's (`fletch_core::oauth::device_login`, the same one
//! the desktop runs). What this adds is the shape a socket needs: the flow
//! outlives a single request, so `github_login_start` returns the code and a
//! background task keeps polling, and `github_login_status` reports what that
//! task has learned. The token is stored by the engine, exactly as it is for the
//! desktop; the `accounts` row — which the desktop's frontend writes after the
//! command returns, and which git falls back to for a commit identity — is
//! written here for the same reason.

use std::sync::Arc;

use fletch_core::oauth::{self, Credentials, DeviceCode, OAuthProfile};
use fletch_core::{database, DbState};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::oneshot;

/// Where a sign-in has got to.
enum Phase {
    /// Waiting for the user to enter the code.
    Waiting(DeviceCode),
    Complete(OAuthProfile),
    Failed(String),
}

/// The one sign-in a host can have in flight. Not a map: a single operator
/// signing a single machine in does not need one, and a second `start` while
/// the first is still good returns the first code rather than asking the user
/// which of two codes to type.
pub struct Login {
    phase: Mutex<Option<Phase>>,
}

impl Login {
    pub fn new() -> Self {
        Self {
            phase: Mutex::new(None),
        }
    }

    /// Start the flow (or re-report the one already running) and return
    /// `{ userCode, verificationUrl, expiresIn }`.
    pub async fn start(self: &Arc<Self>, db: DbState) -> Result<Value, String> {
        if let Some(Phase::Waiting(code)) = self.phase.lock().as_ref() {
            return Ok(waiting(code));
        }
        let creds = Credentials {
            client_id: github_client_id()?,
            client_secret: None,
        };

        // The code arrives inside the flow, before the first poll, so the task
        // hands it back through a channel and this call answers with it. If the
        // flow fails before the provider issues one, the sender drops and the
        // stored failure is what the caller gets.
        let (tx, rx) = oneshot::channel();
        let login = self.clone();
        tokio::spawn(async move {
            let outcome = oauth::device_login(&db, "github", creds, |code| {
                *login.phase.lock() = Some(Phase::Waiting(code.clone()));
                let _ = tx.send(code.clone());
            })
            .await;
            let phase = match outcome {
                Ok(profile) => {
                    // Best-effort: the token (which the engine has already
                    // stored) is what makes the API and git work; the account
                    // row only supplies a fallback commit identity.
                    if let Err(e) = link_account(&db, &profile) {
                        tracing::warn!(error = %e, "github login: could not save the account row");
                    }
                    tracing::info!("github login complete");
                    Phase::Complete(profile)
                }
                Err(e) => {
                    tracing::warn!(error = %e, "github login failed");
                    Phase::Failed(e)
                }
            };
            *login.phase.lock() = Some(phase);
        });

        match rx.await {
            Ok(code) => Ok(waiting(&code)),
            Err(_) => Err(match self.phase.lock().as_ref() {
                Some(Phase::Failed(e)) => e.clone(),
                _ => "the GitHub sign-in ended before it issued a code".to_string(),
            }),
        }
    }

    /// What the flow is doing now. `idle` means nothing has been started since
    /// this host came up — including a login that completed in a previous run,
    /// whose token is nonetheless in place (`status` reports the connection).
    pub fn status(&self) -> Value {
        match self.phase.lock().as_ref() {
            None => json!({ "state": "idle" }),
            Some(Phase::Waiting(code)) => {
                let mut value = waiting(code);
                value["state"] = json!("waiting");
                value
            }
            Some(Phase::Complete(profile)) => json!({
                "state": "complete",
                "name": profile.name,
                "email": profile.email,
            }),
            Some(Phase::Failed(error)) => json!({ "state": "failed", "error": error }),
        }
    }
}

impl Default for Login {
    fn default() -> Self {
        Self::new()
    }
}

fn waiting(code: &DeviceCode) -> Value {
    json!({
        "userCode": code.user_code,
        "verificationUrl": code.verification_uri,
        "expiresIn": code.expires_in,
    })
}

/// The OAuth client this host signs in as.
///
/// The desktop bakes its client id in at compile time because a macOS .app
/// launched from Finder has no environment to read. A host is started by a shell
/// or an init system, both of which do have one, so the environment wins here
/// and a build-time value is only the fallback — one binary can then serve
/// whichever GitHub OAuth app the operator registered.
fn github_client_id() -> Result<String, String> {
    std::env::var("QUORUM_GITHUB_CLIENT_ID")
        .ok()
        .or_else(|| option_env!("QUORUM_GITHUB_CLIENT_ID").map(str::to_string))
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            "GitHub sign-in is not configured: start fletch-host with QUORUM_GITHUB_CLIENT_ID \
             set to your GitHub OAuth app's client id (the app must have the device flow \
             enabled)"
                .to_string()
        })
}

/// Write the signed-in identity onto the single `accounts` row, mirroring what
/// the desktop's `linkOAuthAccount` does (`src/storage/accounts.ts`) — same
/// table, same columns, same "first word is the first name" split.
fn link_account(db: &DbState, profile: &OAuthProfile) -> fletch_core::error::Result<()> {
    let conn = db.lock();
    let id = match database::db_select(&conn, "accounts", json!({}))?
        .into_iter()
        .next()
        .and_then(|row| row.get("id").and_then(Value::as_str).map(str::to_string))
    {
        Some(id) => id,
        None => database::db_insert(&conn, "accounts", json!({ "name": "" }))?,
    };
    let full = profile.name.clone().unwrap_or_default().trim().to_string();
    let (first, last) = match full.split_once(' ') {
        Some((first, last)) => (first.to_string(), last.to_string()),
        None => (full.clone(), String::new()),
    };
    database::db_update(
        &conn,
        "accounts",
        json!({ "id": id }),
        json!({
            "oauth_provider": profile.provider,
            "oauth_id": profile.provider_user_id,
            "name": full,
            "first_name": first,
            "last_name": last,
            "email": profile.email,
            "avatar_url": profile.avatar_url,
        }),
    )?;
    Ok(())
}
