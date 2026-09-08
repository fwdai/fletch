use tauri::{command, plugin::PermissionState, AppHandle, Runtime};

use crate::{PushExt, Result};

/// Ask iOS for alert permission. Called once, after the first pairing — never
/// on launch (docs/remote-protocol.md, "Push notifications" → "Phone").
#[command]
pub(crate) fn request_permission<R: Runtime>(app: AppHandle<R>) -> Result<PermissionState> {
    app.push().request_permission()
}

/// Register with APNs. The token does not come back from here: it arrives
/// whenever iOS has one, as the `push://token` event.
#[command]
pub(crate) fn register<R: Runtime>(app: AppHandle<R>) -> Result<()> {
    app.push().register()
}

#[command]
pub(crate) fn unregister<R: Runtime>(app: AppHandle<R>) -> Result<()> {
    app.push().unregister()
}
