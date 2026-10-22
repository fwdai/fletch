use fletch_core::oauth::{self, Credentials, OAuthProfile};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// The `oauth:device-code` payload, unchanged: the frontend reads these three
/// keys (`src/util/useGithubConnect.ts`) and opens the browser itself.
#[derive(Debug, Clone, Serialize)]
struct DeviceCodePayload {
    provider: String,
    user_code: String,
    verification_uri: String,
}

/// A build-time config value, baked into the binary via `option_env!`.
/// Returns `None` when the variable was unset *or empty* at compile time, so a
/// missing CI secret reads as "not configured" rather than a confusing
/// provider error. (Runtime env is not used: a macOS .app launched from Finder
/// gets a minimal environment, so the keys must be embedded at build time.)
macro_rules! config_value {
    ($key:literal) => {
        option_env!($key)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
}

/// This app's OAuth client for `provider`. The id is a per-application
/// secret, which is why it is the shell's to supply and not the engine's.
fn credentials(provider: &str) -> Result<Credentials, String> {
    match provider {
        "github" => Ok(Credentials {
            client_id: config_value!("QUORUM_GITHUB_CLIENT_ID")
                .ok_or_else(|| "GitHub sign-in is not configured".to_string())?,
        }),
        other => Err(format!("unknown provider: {other}")),
    }
}

/// Drive the full device flow and return a normalized profile, emitting the
/// user code to the window as soon as the provider issues it.
#[tauri::command]
pub async fn oauth_device_login(
    app: AppHandle,
    db: tauri::State<'_, std::sync::Arc<parking_lot::Mutex<rusqlite::Connection>>>,
    provider: String,
) -> Result<OAuthProfile, String> {
    let creds = credentials(&provider)?;
    let db = db.inner().clone();
    oauth::device_login(&db, &provider, creds, |code| {
        let _ = app.emit(
            "oauth:device-code",
            DeviceCodePayload {
                provider: provider.clone(),
                user_code: code.user_code.clone(),
                verification_uri: code.verification_uri.clone(),
            },
        );
    })
    .await
}
