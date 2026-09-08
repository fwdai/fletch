// APNs registration for the Fletch mobile companion. Tauri has no push plugin
// of its own, and the only things the phone needs from APNs are the device
// token (which the host stores and the relay sends to) and the `fletch` object
// of a tapped alert — docs/remote-protocol.md, "Push notifications". So this is
// deliberately three commands and two events, and iOS is the only platform
// that can do any of it.

use tauri::{
    plugin::{Builder, TauriPlugin},
    Manager, Runtime,
};

mod commands;
mod error;
#[cfg(target_os = "ios")]
mod ios;
#[cfg(not(target_os = "ios"))]
mod other;

pub use error::{Error, Result};

#[cfg(target_os = "ios")]
use ios as platform;
#[cfg(not(target_os = "ios"))]
use other as platform;

pub use platform::Push;

/// The event the webview listens for to learn the device token:
/// `{ token, environment }`.
pub const TOKEN_EVENT: &str = "push://token";
/// The event a tapped alert raises: the notification's `fletch` object.
pub const OPENED_EVENT: &str = "push://opened";

pub trait PushExt<R: Runtime> {
    fn push(&self) -> &Push<R>;
}

impl<R: Runtime, T: Manager<R>> PushExt<R> for T {
    fn push(&self) -> &Push<R> {
        self.state::<Push<R>>().inner()
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("push")
        .invoke_handler(tauri::generate_handler![
            commands::request_permission,
            commands::register,
            commands::unregister
        ])
        .setup(|app, api| {
            app.manage(platform::init(app, api)?);
            Ok(())
        })
        .build()
}
