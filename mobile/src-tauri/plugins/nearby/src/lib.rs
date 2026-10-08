// The Fletch hosts announcing themselves on this network
// (docs/remote-protocol.md, "Discovery"), for the Pair screen's "Macs nearby"
// list. One command, and only iOS can answer it: browsing goes through
// NWBrowser, which needs no multicast entitlement, where a browser in Rust
// would.

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

pub use platform::Nearby;

/// One announcing host, as its TXT record describes it. None of it is
/// trusted: the handshake that follows is what authenticates.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NearbyHost {
    pub name: String,
    /// The host key it claims, base64url.
    pub host_key: String,
    pub port: u16,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct BrowseResult {
    pub hosts: Vec<NearbyHost>,
}

pub trait NearbyExt<R: Runtime> {
    fn nearby(&self) -> &Nearby<R>;
}

impl<R: Runtime, T: Manager<R>> NearbyExt<R> for T {
    fn nearby(&self) -> &Nearby<R> {
        self.state::<Nearby<R>>().inner()
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("nearby")
        .invoke_handler(tauri::generate_handler![commands::browse])
        .setup(|app, api| {
            app.manage(platform::init(app, api)?);
            Ok(())
        })
        .build()
}
