//! Cloud providers in Settings: which provider accounts this machine is
//! connected to, and connecting or disconnecting one; and this install's
//! runner at a provider (`fletch_core::cloud_runner`), created, inspected,
//! started, stopped and deleted from there. The token is kept in this
//! machine's secret store, and the runner is this install's, so these are this
//! machine's commands only — none of them is on the remote wire
//! (`remote::dispatch`).

use std::sync::Arc;

use fletch_core::cloud_runner::{
    self, CreateInputs, Deleted, Progress, RunnerPairing, RunnerRecord, RunnerStatus,
};
use tauri::{AppHandle, Emitter, State};

use crate::cloud::{self, CloudProviderStatus, ProviderId};
use crate::error::{Error, Result};

type Db = Arc<parking_lot::Mutex<rusqlite::Connection>>;

fn provider_id(provider: &str) -> Result<ProviderId> {
    ProviderId::parse(provider)
        .ok_or_else(|| Error::Other(format!("unknown cloud provider: {provider:?}")))
}

/// Every provider, connected or not. Read from what is stored — never the
/// network — so Settings renders at once.
#[tauri::command]
pub fn cloud_providers(db: State<'_, Db>) -> Result<Vec<CloudProviderStatus>> {
    cloud::statuses(&db.lock())
}

/// Connect a provider with a pasted token, acting in `account` (absent or
/// blank: the provider's default, `CloudProviderStatus::account_placeholder`).
/// The token is validated against the provider before anything is stored, so
/// a bad paste fails loudly and changes nothing; once connected, the
/// provider's spare machines are made in the background
/// (`fletch_core::commands::cloud_connect_impl`).
#[tauri::command]
pub async fn cloud_connect(
    db: State<'_, Db>,
    provider: String,
    token: String,
    account: Option<String>,
) -> Result<CloudProviderStatus> {
    fletch_core::commands::cloud_connect_impl(
        db.inner(),
        provider_id(&provider)?,
        &token,
        account.as_deref(),
    )
    .await
}

/// Forget a provider's token and account, once its spare machines are
/// destroyed: while one cannot be, or while this install has a runner at the
/// provider, it stays connected and the disconnect is refused with why
/// (`fletch_core::commands::cloud_disconnect_impl`).
/// The machines of workspaces at the provider are not touched: they stay
/// there, running or stopped, out of this machine's reach until it connects
/// again: a workspace whose box is there does not start meanwhile, and the
/// box of one discarded meanwhile is destroyed at the provider at the first
/// start after it does (`sandbox::boxes::cloud`).
#[tauri::command]
pub async fn cloud_disconnect(db: State<'_, Db>, provider: String) -> Result<()> {
    fletch_core::commands::cloud_disconnect_impl(db.inner(), provider_id(&provider)?).await
}

/// Where the runner is: none, being set up, or its machine's state as its
/// provider reports it, with its record. Asks the provider when there is a
/// runner.
#[tauri::command]
pub async fn cloud_runner_status(db: State<'_, Db>) -> Result<RunnerStatus> {
    cloud_runner::status(db.inner()).await
}

/// Create the runner at `provider` and set it up, authorising the calling
/// device's key on it; answers its record. Each step is emitted as
/// `cloud-runner:progress` as it starts (`cloud_runner::Progress`). The Claude
/// token in `inputs` goes to the runner's disk and nowhere else.
#[tauri::command]
pub async fn cloud_runner_create(
    app: AppHandle,
    db: State<'_, Db>,
    provider: String,
    inputs: CreateInputs,
) -> Result<RunnerRecord> {
    let progress = move |step: Progress| {
        let _ = app.emit("cloud-runner:progress", step);
    };
    cloud_runner::create(db.inner(), provider_id(&provider)?, inputs, &progress).await
}

/// Start the runner's machine and wait until it runs; a lost machine is made
/// again on its disk, a lost disk is refused.
#[tauri::command]
pub async fn cloud_runner_start(db: State<'_, Db>) -> Result<()> {
    cloud_runner::start(db.inner()).await
}

/// Stop the runner's machine and wait until it has stopped.
#[tauri::command]
pub async fn cloud_runner_stop(db: State<'_, Db>) -> Result<()> {
    cloud_runner::stop(db.inner()).await
}

/// Delete the runner, its workspaces' machines first; one that cannot be
/// reached is refused unless `force`.
#[tauri::command]
pub async fn cloud_runner_delete(db: State<'_, Db>, force: bool) -> Result<Deleted> {
    cloud_runner::delete(db.inner(), force).await
}

/// A pairing code for another device, minted by the running runner with the
/// access `preset` names (`full` when absent).
#[tauri::command]
pub async fn cloud_runner_pairing_link(
    db: State<'_, Db>,
    preset: Option<String>,
) -> Result<RunnerPairing> {
    cloud_runner::pairing_link(db.inner(), preset.as_deref()).await
}
