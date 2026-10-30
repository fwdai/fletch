//! Getting a host ready to boot boxes, and waiting for it: the pinned image's
//! install (one at a time, in the background, `box:image` events) and the
//! tools disks built after it. Turning the flag on starts the install
//! (`commands::set_box_setting_impl`); a launch or provision that arrives
//! before it is done waits here instead of failing
//! (`BoxRegistry::ensure_running`), reporting what it waits on as the box's
//! `Preparing` phase.

use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use fletch_box::InstalledImage;
use serde::Serialize;
use tokio::sync::watch;

use crate::error::{Error, Result};

use super::{registry, settings, Ready, EVENT_IMAGE};

/// How long a launch waits for the image download before giving up: 850 MB
/// on a slow connection.
pub const IMAGE_WAIT: Duration = Duration::from_secs(30 * 60);
/// How long a launch waits for a tools disk build: an npm install through the
/// egress proxy in a fresh box.
pub const TOOLS_WAIT: Duration = Duration::from_secs(10 * 60);
/// How long a tools disk build waits for codegraph's guest bundle (a 55 MB
/// download, seconds as a rule) before building the disk without it.
pub const CODEGRAPH_BUNDLE_WAIT: Duration = Duration::from_secs(3 * 60);

/// How long a launch waits for a tools disk build to finish, the codegraph
/// bundle it fetches first included when indexing is on.
pub fn tools_wait() -> Duration {
    if crate::codegraph::enabled() {
        TOOLS_WAIT + CODEGRAPH_BUNDLE_WAIT
    } else {
        TOOLS_WAIT
    }
}

/// `box_image_status` and the `box:image` event.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BoxImageStatus {
    /// `not_installed` | `downloading` | `verifying` | `installed` | `damaged` | `unavailable`
    pub phase: &'static str,
    /// Percent, while downloading.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<u8>,
    /// The installed version, or the one being installed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The version this build pins and where it downloads from.
    pub pinned_version: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The one install at a time: its cancel flag and its latest status, which
/// waiters follow.
struct Install {
    cancel: Arc<AtomicBool>,
    status: watch::Receiver<BoxImageStatus>,
}

fn install_slot() -> &'static Mutex<Option<Install>> {
    static SLOT: OnceLock<Mutex<Option<Install>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

fn lock_slot() -> std::sync::MutexGuard<'static, Option<Install>> {
    install_slot().lock().unwrap_or_else(|p| p.into_inner())
}

/// The image's state: the running install's latest word, else the store's.
pub fn image_status() -> BoxImageStatus {
    if let Some(install) = lock_slot().as_ref() {
        return install.status.borrow().clone();
    }
    let pinned = super::pinned_image();
    let mut status = BoxImageStatus {
        phase: "not_installed",
        progress: None,
        version: None,
        pinned_version: pinned.version.clone(),
        url: pinned.url(),
        error: None,
    };
    // Installed means this build's version is present (`installed_image`);
    // another version is neither bootable by this build nor an error.
    if let Ok(Some(image)) = super::installed_image() {
        status.phase = "installed";
        status.version = Some(image.version);
    }
    status
}

/// Start downloading (or copying, for a `file://` URL) this build's pinned
/// image in the background; progress and the outcome arrive as `box:image`
/// events, and the tools disks are built once it is in. A no-op while an
/// install runs, and quick when the image is installed already (the store
/// skips the download), so the toggle and a Retry can both call it blindly.
pub fn start_install() -> Result<()> {
    if !settings::enabled() {
        return Err(Error::Other(
            "box sandbox is off; enable it in Developer settings".into(),
        ));
    }
    let pinned = super::pinned_image();
    let mut status = BoxImageStatus {
        phase: "downloading",
        progress: Some(0),
        version: Some(pinned.version.clone()),
        pinned_version: pinned.version.clone(),
        url: pinned.url(),
        error: None,
    };
    let (tx, rx) = watch::channel(status.clone());
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut slot = lock_slot();
        if slot.is_some() {
            return Ok(());
        }
        *slot = Some(Install {
            cancel: cancel.clone(),
            status: rx,
        });
    }
    crate::host::spawn(async move {
        let publish = |status: &BoxImageStatus| {
            tx.send_replace(status.clone());
            super::emit(EVENT_IMAGE, status);
        };
        publish(&status);
        let mut last = 0u8;
        let result = super::image_store()
            .install(&pinned, |p| {
                if cancel.load(Ordering::Relaxed) {
                    return ControlFlow::Break(());
                }
                let pct = (p.done * 100).checked_div(p.total).unwrap_or(0) as u8;
                if pct != last {
                    last = pct;
                    status.progress = Some(pct);
                    publish(&status);
                }
                ControlFlow::Continue(())
            })
            .await;
        // Cleared before the final word so `image_status` reads the store for
        // a cancelled install, and waiters that re-read after the channel
        // closes see the installed image.
        *lock_slot() = None;
        match result {
            Ok(image) => {
                status.phase = "installed";
                status.progress = None;
                status.version = Some(image.version);
                publish(&status);
                prebuild_tools();
            }
            Err(fletch_box::Error::Cancelled) => publish(&image_status()),
            Err(e) => {
                tracing::warn!(error = %e, "box image install failed");
                status.phase = "unavailable";
                status.progress = None;
                status.error = Some(e.to_string());
                publish(&status);
            }
        }
    });
    Ok(())
}

pub fn cancel_install() -> Result<()> {
    match lock_slot().as_ref() {
        Some(install) => {
            install.cancel.store(true, Ordering::Relaxed);
            Ok(())
        }
        None => Err(Error::Other("no box image install is running".into())),
    }
}

/// Everything a boot needs, now: the flag, the runtime and an installed
/// image, with no waiting.
pub fn ready_now() -> Result<Ready> {
    let paths = super::availability()?;
    let image = super::installed_image()?
        .ok_or_else(|| Error::SandboxUnavailable("the box image is not installed".into()))?;
    Ok(Ready { paths, image })
}

/// Everything a boot needs, waiting for the image when it is still on its
/// way (starting the download when nothing is): `on_progress` hears each
/// change of what is being waited on, for the box's `Preparing` phase. Fails
/// only on the runtime probe, a download error, a cancel, or [`IMAGE_WAIT`].
pub async fn ready(mut on_progress: impl FnMut(String)) -> Result<Ready> {
    let paths = super::availability()?;
    let image = match super::installed_image()? {
        Some(image) => image,
        None => wait_for_image(&mut on_progress).await?,
    };
    Ok(Ready { paths, image })
}

async fn wait_for_image(on_progress: &mut impl FnMut(String)) -> Result<InstalledImage> {
    let deadline = Instant::now() + IMAGE_WAIT;
    let mut reported: Option<String> = None;
    loop {
        if let Some(image) = super::installed_image()? {
            return Ok(image);
        }
        let Some(mut rx) = lock_slot().as_ref().map(|i| i.status.clone()) else {
            // No install running and no image: this launch starts the one the
            // toggle would have (a cancelled download, say).
            start_install()?;
            continue;
        };
        loop {
            let status = rx.borrow_and_update().clone();
            match status.phase {
                "installed" => break,
                "downloading" | "verifying" => {
                    let detail = match status.progress {
                        Some(pct) => format!("downloading image {pct}%"),
                        None => "verifying image".to_string(),
                    };
                    if reported.as_deref() != Some(&detail) {
                        on_progress(detail.clone());
                        reported = Some(detail);
                    }
                }
                _ => {
                    return Err(Error::SandboxUnavailable(match status.error {
                        Some(e) => format!("the box image download failed: {e}"),
                        None => "the box image download was cancelled".into(),
                    }))
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, rx.changed()).await {
                Ok(Ok(())) => {}
                // The install ended and dropped its sender: the store decides.
                Ok(Err(_)) => break,
                Err(_) => {
                    return Err(Error::SandboxUnavailable(format!(
                        "the box image download did not finish within {} minutes",
                        IMAGE_WAIT.as_secs() / 60
                    )))
                }
            }
        }
    }
}

/// Build the tools disk of every supported provider whose CLI this host has,
/// in the background (`box:tools` events), so the first launch finds its disk
/// ready. Skips what is built already, unless indexing is on and the disk lacks
/// codegraph (`registry::tools_disk_settled`). Off the caller's thread: the
/// probe and the CLI version lookups run processes.
pub fn prebuild_tools() {
    crate::host::spawn(async {
        let planned = tokio::task::spawn_blocking(|| {
            let ready = ready_now()?;
            let providers: Vec<String> = fletch_box::tools::PROVIDERS
                .iter()
                .filter(|p| super::supports_provider(p))
                .filter(|p| crate::agent::cached_provider_version(p).is_some())
                .map(|p| p.to_string())
                .collect();
            Ok::<_, Error>((ready, providers))
        })
        .await;
        match planned {
            Ok(Ok((ready, providers))) => {
                for provider in providers {
                    if !registry::tools_disk_settled(&provider) {
                        registry::BoxRegistry::global().start_tools_build(&ready, &provider);
                    }
                }
            }
            Ok(Err(e)) => {
                tracing::info!(error = %e, "no tools prebuild: the box runtime is not ready")
            }
            Err(e) => tracing::warn!(error = %e, "tools prebuild task failed"),
        }
    });
}

/// How much longer than its usual budget a spawn of a box agent of `provider`
/// may take right now: the image download and the tools disk build it will
/// wait for in `ensure_running`, nothing once both are in place. For the
/// spawn watchdog.
pub fn setup_allowance(provider: &str) -> Duration {
    if !settings::enabled() {
        return Duration::ZERO;
    }
    let mut extra = Duration::ZERO;
    if !matches!(super::installed_image(), Ok(Some(_))) {
        extra += IMAGE_WAIT;
    }
    // A disk without codegraph is booted as is; only a missing one is waited on.
    if registry::built_tools_disk(provider).is_none() {
        extra += tools_wait();
    }
    extra
}
