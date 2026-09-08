use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use tauri::{
    ipc::{Channel, InvokeResponseBody},
    plugin::{PermissionState, PluginApi, PluginHandle},
    AppHandle, Emitter, Runtime,
};

use crate::{Result, OPENED_EVENT, TOKEN_EVENT};

tauri::ios_plugin_binding!(init_plugin_push);

/// The Swift plugin's event name → the Tauri event it is re-emitted as. Swift
/// can only reach the Rust side through a channel, so each kind gets one whose
/// only job is `emit`; the webview then `listen()`s for these the same way it
/// listens for the pairing deep link.
const EVENTS: &[(&str, &str)] = &[("token", TOKEN_EVENT), ("opened", OPENED_EVENT)];

#[derive(Serialize)]
struct RegisterListener {
    event: String,
    handler: Channel<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PermissionResponse {
    permission_state: PermissionState,
}

pub struct Push<R: Runtime>(PluginHandle<R>);

pub(crate) fn init<R: Runtime, C: DeserializeOwned>(
    app: &AppHandle<R>,
    api: PluginApi<R, C>,
) -> Result<Push<R>> {
    let handle = api.register_ios_plugin(init_plugin_push)?;
    for (swift_event, tauri_event) in EVENTS {
        let app = app.clone();
        let tauri_event = *tauri_event;
        let handler: Channel<Value> = Channel::new(move |body| {
            if let InvokeResponseBody::Json(json) = body {
                if let Ok(payload) = serde_json::from_str::<Value>(&json) {
                    let _ = app.emit(tauri_event, payload);
                }
            }
            Ok(())
        });
        handle.run_mobile_plugin::<()>(
            "registerListener",
            RegisterListener {
                event: (*swift_event).to_string(),
                handler,
            },
        )?;
    }
    Ok(Push(handle))
}

impl<R: Runtime> Push<R> {
    pub fn request_permission(&self) -> Result<PermissionState> {
        self.0
            .run_mobile_plugin::<PermissionResponse>("requestPermission", ())
            .map(|response| response.permission_state)
            .map_err(Into::into)
    }

    pub fn register(&self) -> Result<()> {
        self.0
            .run_mobile_plugin::<()>("register", ())
            .map_err(Into::into)
    }

    pub fn unregister(&self) -> Result<()> {
        self.0
            .run_mobile_plugin::<()>("unregister", ())
            .map_err(Into::into)
    }
}
