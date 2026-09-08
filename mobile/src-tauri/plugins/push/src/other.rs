// There is no APNs off iOS. This half exists so the crate still compiles for a
// Mac host — the dev loop that runs `cargo check` and the unit tests — and so a
// call that would only work on a phone fails with a sentence instead of a
// missing command.

use std::marker::PhantomData;

use serde::de::DeserializeOwned;
use tauri::{
    plugin::{PermissionState, PluginApi},
    AppHandle, Runtime,
};

use crate::{Error, Result};

/// `PhantomData<fn() -> R>` rather than `PhantomData<R>`: the type has to be
/// `Send + Sync` to live in Tauri's state, and a fn pointer is regardless of
/// what the runtime is.
pub struct Push<R: Runtime>(PhantomData<fn() -> R>);

pub(crate) fn init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    _api: PluginApi<R, C>,
) -> Result<Push<R>> {
    Ok(Push(PhantomData))
}

fn unsupported() -> Error {
    Error::new("push is iOS-only")
}

impl<R: Runtime> Push<R> {
    pub fn request_permission(&self) -> Result<PermissionState> {
        Err(unsupported())
    }

    pub fn register(&self) -> Result<()> {
        Err(unsupported())
    }

    pub fn unregister(&self) -> Result<()> {
        Err(unsupported())
    }
}

#[cfg(test)]
mod tests {
    use super::unsupported;

    #[test]
    fn says_which_platform_it_needs() {
        assert_eq!(unsupported().to_string(), "push is iOS-only");
    }
}
