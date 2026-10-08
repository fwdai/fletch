// Off iOS there is no browser, and nothing to fail about: the Pair screen
// simply lists nobody and offers the typed address. This half exists so the
// crate compiles for a Mac host, which the dev loop's `cargo check` needs.

use std::marker::PhantomData;

use serde::de::DeserializeOwned;
use tauri::{plugin::PluginApi, AppHandle, Runtime};

use crate::{BrowseResult, Result};

/// `PhantomData<fn() -> R>`: `Send + Sync` whatever the runtime is, as Tauri's
/// state requires.
pub struct Nearby<R: Runtime>(PhantomData<fn() -> R>);

pub(crate) fn init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    _api: PluginApi<R, C>,
) -> Result<Nearby<R>> {
    Ok(Nearby(PhantomData))
}

impl<R: Runtime> Nearby<R> {
    pub fn browse(&self, _duration_ms: u64) -> Result<BrowseResult> {
        Ok(BrowseResult::default())
    }
}
