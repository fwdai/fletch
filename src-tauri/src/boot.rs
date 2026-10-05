use parking_lot::Mutex;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

use crate::host::{BootPhase, BootStep};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum BootSnapshot {
    Booting {
        step: BootPhase,
        /// Percent through `step`, for the one that reports it.
        #[serde(skip_serializing_if = "Option::is_none")]
        progress: Option<u8>,
    },
    Ready,
    Failed {
        message: String,
    },
}

impl From<BootStep> for BootSnapshot {
    fn from(step: BootStep) -> Self {
        Self::Booting {
            step: step.phase,
            progress: step.progress,
        }
    }
}

/// Where startup is, as the webview sees it. Managed before anything else in
/// `setup`, so `boot_state` answers from the webview's first frame whatever the
/// boot thread is doing. Every change is also emitted as `boot:state`; the
/// frontend subscribes to that first and then asks, so a change that lands
/// between the two is seen either way.
#[derive(Clone)]
pub struct BootStatus(Arc<Mutex<BootSnapshot>>);

impl Default for BootStatus {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(
            BootStep::from(BootPhase::OpeningDatabase).into(),
        )))
    }
}

impl BootStatus {
    pub fn set(&self, app: &AppHandle, snapshot: BootSnapshot) {
        // Timestamped, so a slow start in the field says which step was slow.
        tracing::info!(state = ?snapshot, "boot");
        *self.0.lock() = snapshot.clone();
        if let Err(e) = app.emit("boot:state", snapshot) {
            tracing::warn!(error = %e, "boot:state emit failed");
        }
    }

    pub fn fail(&self, app: &AppHandle, message: String) {
        tracing::error!(error = %message, "engine boot failed");
        self.set(app, BootSnapshot::Failed { message });
    }
}

#[tauri::command]
pub fn boot_state(status: State<'_, BootStatus>) -> BootSnapshot {
    let snapshot = status.0.lock().clone();
    tracing::debug!(state = ?snapshot, "boot_state asked");
    snapshot
}
