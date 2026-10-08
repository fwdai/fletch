use serde::{de::DeserializeOwned, Serialize};
use tauri::{
    plugin::{PluginApi, PluginHandle},
    AppHandle, Runtime,
};

use crate::{BrowseResult, Result};

tauri::ios_plugin_binding!(init_plugin_nearby);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowseArgs {
    duration_ms: u64,
}

pub struct Nearby<R: Runtime>(PluginHandle<R>);

pub(crate) fn init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    api: PluginApi<R, C>,
) -> Result<Nearby<R>> {
    Ok(Nearby(api.register_ios_plugin(init_plugin_nearby)?))
}

impl<R: Runtime> Nearby<R> {
    pub fn browse(&self, duration_ms: u64) -> Result<BrowseResult> {
        self.0
            .run_mobile_plugin::<BrowseResult>("browse", BrowseArgs { duration_ms })
            .map_err(Into::into)
    }
}
