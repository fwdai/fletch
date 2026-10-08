use tauri::{command, AppHandle, Runtime};

use crate::{BrowseResult, Error, NearbyExt, Result};

/// Browse for `duration_ms` and answer with every host seen by the end of it.
///
/// The bridge call blocks until Swift answers, so it runs on a blocking thread:
/// an async command would otherwise hold a runtime worker for the whole browse.
#[command]
pub(crate) async fn browse<R: Runtime>(
    app: AppHandle<R>,
    duration_ms: Option<u64>,
) -> Result<BrowseResult> {
    let duration_ms = duration_ms.unwrap_or(2000);
    tauri::async_runtime::spawn_blocking(move || app.nearby().browse(duration_ms))
        .await
        .map_err(|e| Error::new(e.to_string()))?
}
