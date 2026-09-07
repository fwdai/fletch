//! Getting the pinned weights onto disk.
//!
//! The download is fire-and-forget: half a gigabyte can't be awaited by the
//! Settings toggle that asks for it, so [`download`] returns the moment the
//! work is underway and progress arrives as `dictation:model_progress`
//! events. That makes the event stream the only live channel, and [`status`]
//! the truth a freshly mounted UI reads.
//!
//! The file appears at its final path only once its digest matched: the
//! download streams to a temp file and is renamed in, which is what lets
//! [`models::installed_path`] treat "right size, right place" as verified.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;
use tauri::AppHandle;

use super::models::{self, WhisperModel};
use crate::download;
use crate::error::{Error, Result};

/// Guards the one download allowed at a time. The Settings toggle and its
/// retry button both reach [`download`], and two streams would share one
/// per-process temp path.
static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// How far along the model install is. `Verifying` is the tail of the
/// download, not a separate pass — the digest is computed as bytes arrive.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum State {
    Downloading,
    Verifying,
    Installed,
    Error,
}

/// Payload of `dictation:model_progress`. `total` is absent only when the
/// server sends no length and the catalog size is unknown; `error` is set for
/// `Error` alone and carries the message shown in Settings.
#[derive(Clone, Serialize)]
struct Progress {
    model_id: &'static str,
    state: State,
    received: u64,
    total: Option<u64>,
    error: Option<String>,
}

/// A snapshot of the default model's install state. `downloading` is this
/// process's in-flight flag, so it is honest across a Settings screen that
/// mounted mid-download and missed the events so far.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Status {
    pub installed: bool,
    pub downloading: bool,
}

pub fn status() -> Status {
    Status {
        installed: models::installed_path(models::default_model()).is_some(),
        downloading: IN_FLIGHT.load(Ordering::Acquire),
    }
}

/// Start fetching the default model, unless it is already installed or a
/// download is running — either way the returned status says what the caller
/// asked about, so an opt-in and a retry are the same call.
pub fn download(app: AppHandle) -> Status {
    let model = models::default_model();
    if models::installed_path(model).is_some() {
        return Status {
            installed: true,
            downloading: false,
        };
    }
    // The claim on the flag is the guard, so it's also the "already running"
    // check — a separate read first would just be a wider race window.
    if IN_FLIGHT
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Status {
            installed: false,
            downloading: true,
        };
    }

    tauri::async_runtime::spawn(async move {
        let result = run(&app, model).await;
        IN_FLIGHT.store(false, Ordering::Release);
        match result {
            Ok(()) => emit(&app, model, State::Installed, model.size, None),
            Err(e) => {
                tracing::warn!(error = %e, model = model.id, "whisper model download failed");
                emit(&app, model, State::Error, 0, Some(e.to_string()));
            }
        }
    });
    Status {
        installed: false,
        downloading: true,
    }
}

/// Delete the installed weights, so the disk cost of an engine the user turned
/// off doesn't linger. Returns the state after the attempt.
pub fn remove() -> Status {
    let model = models::default_model();
    if let Some(path) = models::installed_path(model) {
        if let Err(e) = std::fs::remove_file(&path) {
            tracing::warn!(error = %e, path = %path.display(), "removing whisper model failed");
        }
    }
    status()
}

async fn run(app: &AppHandle, model: &'static WhisperModel) -> Result<()> {
    let dest = models::path(model)
        .ok_or_else(|| Error::Other("whisper models root is not initialized".into()))?;
    let root = dest
        .parent()
        .expect("a model path is a file inside the models root");
    tokio::fs::create_dir_all(root)
        .await
        .map_err(|e| Error::Other(format!("create {}: {e}", root.display())))?;
    clear_stale_tmp(root).await;

    emit(app, model, State::Downloading, 0, None);
    let tmp = download::download_verified(model.url, model.sha256, root, |received, total| {
        // The last call lands after the final byte, when all that's left is
        // finalizing the hash and the rename — so report that as verifying
        // rather than parking the bar at 100% under a "downloading" label.
        let state = if total.is_some_and(|t| received >= t) {
            State::Verifying
        } else {
            State::Downloading
        };
        emit(app, model, state, received, None);
    })
    .await?;

    tokio::fs::rename(&tmp, &dest)
        .await
        .map_err(|e| Error::Other(format!("install {}: {e}", dest.display())))?;
    tracing::info!(model = model.id, path = %dest.display(), "whisper model installed");
    Ok(())
}

/// A temp file untouched for this long is a dead download. A live one is
/// written every network chunk, and a request that receives nothing for
/// [`download::READ_TIMEOUT`] fails and removes its own file — so a file this
/// idle has no request behind it, only a process that died mid-stream.
const STALE_TMP_AFTER: Duration = Duration::from_secs(10 * 60);
// The sweep is only safe while the read timeout fires well before it: a
// stalled-but-live request must be gone before its file looks stale.
const _: () = assert!(STALE_TMP_AFTER.as_secs() >= 5 * download::READ_TIMEOUT.as_secs());

/// Drop temp files from a run that died. The download path removes its own on
/// failure, but a process killed mid-stream leaves a partial half-gigabyte
/// behind under a pid-stamped name that is never reused.
///
/// Staleness is judged by age, not by pid: `IN_FLIGHT` only coordinates one
/// process, and a second app instance sharing this directory has its own
/// in-flight file here that must not be unlinked from under it (its rename
/// would fail after the whole download). Anything modified within the window
/// is presumed live, whoever owns it.
async fn clear_stale_tmp(root: &Path) {
    let Ok(mut dir) = tokio::fs::read_dir(root).await else {
        return;
    };
    while let Ok(Some(entry)) = dir.next_entry().await {
        if !download::is_tmp_name(&entry.file_name().to_string_lossy()) {
            continue;
        }
        let idle = entry
            .metadata()
            .await
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok());
        // An unreadable mtime is left alone: a spurious keep costs disk until
        // the next sweep, a spurious delete costs someone their download.
        if idle.is_some_and(|d| d >= STALE_TMP_AFTER) {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
}

fn emit(
    app: &AppHandle,
    model: &'static WhisperModel,
    state: State,
    received: u64,
    error: Option<String>,
) {
    crate::dictation::emit(
        app,
        "dictation:model_progress",
        Progress {
            model_id: model.id,
            state,
            received,
            // The catalog's size is the pin the digest belongs to, so it beats
            // whatever `Content-Length` the CDN reports.
            total: Some(model.size),
            error,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sweep must clear a killed run's leftovers without touching an
    /// installed model, or a temp file that is still being written — ours or
    /// another instance's.
    #[tokio::test]
    async fn stale_temp_files_are_swept_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stale = root.join("download-424242.tmp");
        let live = root.join("download-424243.tmp");
        let model = root.join("ggml-large-v3-turbo-q5_0.bin");
        for f in [&stale, &live, &model] {
            std::fs::write(f, b"x").unwrap();
        }
        let long_ago = std::time::SystemTime::now() - STALE_TMP_AFTER * 2;
        std::fs::File::open(&stale)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        std::fs::File::open(&model)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();

        clear_stale_tmp(root).await;

        assert!(!stale.exists(), "a dead run's temp file must be swept");
        assert!(
            live.exists(),
            "a temp file still being written must survive"
        );
        assert!(
            model.exists(),
            "installed weights must survive, however old"
        );
    }

    /// Without `whisper::init` there is no models root, so nothing can be
    /// installed — the status must say so rather than panic on the missing path.
    #[test]
    fn status_without_a_models_root_is_not_installed() {
        let s = status();
        assert!(!s.installed);
        assert!(!s.downloading);
    }
}
