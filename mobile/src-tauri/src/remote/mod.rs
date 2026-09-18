// The phone's four secure-channel commands. Everything they do is
// `fletch_proto::client::Dialer` — the socket, the Noise state, the connection
// map and the device key file — so this module is only the Tauri end of it:
// managed state, command signatures, and the three events on the webview's bus.
//
// The same wrapper exists on the desktop (`src-tauri/src/client/mod.rs`), which
// is what makes one dialer serve both apps. Keep the two in step: the command
// names, their argument keys and the event payloads are the contract the shared
// TS transport (`src/remote/ws.ts`) is written against.

use std::sync::Arc;

use fletch_proto::client::{ClientEvent, ConnectResult, ConnectionId, Dialer, Target};
use tauri::{AppHandle, Emitter, Manager, State};

/// The dialer this app's commands work on. The device key stays where it has
/// always been — `<app_data_dir>/device_key` — so existing pairings survive.
pub fn dialer(app: &AppHandle) -> tauri::Result<Arc<Dialer>> {
    let dir = app.path().app_data_dir()?;
    let app = app.clone();
    Ok(Dialer::new(dir, Box::new(move |event| emit(&app, event))))
}

/// Every event carries its connection id, so the webview can run more than one
/// connection and tell them apart.
fn emit(app: &AppHandle, event: ClientEvent) {
    let name = event.name();
    let _ = match event {
        ClientEvent::Message(payload) => app.emit(name, payload),
        ClientEvent::Close(payload) => app.emit(name, payload),
        ClientEvent::Error(payload) => app.emit(name, payload),
    };
}

/// Open a socket to `url`, run the Noise handshake, and report the host's
/// identity and the new connection's id. `timeout_ms` bounds the dial and the
/// handshake together.
#[tauri::command]
pub async fn remote_connect(
    state: State<'_, Arc<Dialer>>,
    url: String,
    host_key: Option<String>,
    timeout_ms: Option<u64>,
) -> Result<ConnectResult, String> {
    state
        .connect(Target {
            url,
            host_key,
            timeout_ms,
        })
        .await
}

/// Encrypt one JSON document and send it on `connection_id`.
#[tauri::command]
pub async fn remote_send(
    state: State<'_, Arc<Dialer>>,
    connection_id: ConnectionId,
    text: String,
) -> Result<(), String> {
    state.send(connection_id, &text).await
}

/// Close `connection_id` with 1000; a no-op if it is already gone.
#[tauri::command]
pub async fn remote_close(
    state: State<'_, Arc<Dialer>>,
    connection_id: ConnectionId,
) -> Result<(), String> {
    state.close(connection_id).await
}

/// This device's public key, base64url — what the host records when pairing.
#[tauri::command]
pub fn remote_device_public_key(state: State<'_, Arc<Dialer>>) -> Result<String, String> {
    state.device_public_key()
}
