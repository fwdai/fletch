//! Cloud providers in Settings: which provider accounts this machine is
//! connected to, and connecting or disconnecting one. The token is kept in this
//! machine's secret store, so these are this machine's commands only — none of
//! them is on the remote wire (`remote::dispatch`).

use std::sync::Arc;

use tauri::State;

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
/// destroyed: while one cannot be, the provider stays connected and the
/// disconnect is refused with why (`fletch_core::commands::cloud_disconnect_impl`).
/// The machines of workspaces at the provider are not touched: they stay
/// there, running or stopped, out of this machine's reach until it connects
/// again: a workspace whose box is there does not start meanwhile, and the
/// box of one discarded meanwhile is destroyed at the provider at the first
/// start after it does (`sandbox::boxes::cloud`).
#[tauri::command]
pub async fn cloud_disconnect(db: State<'_, Db>, provider: String) -> Result<()> {
    fletch_core::commands::cloud_disconnect_impl(db.inner(), provider_id(&provider)?).await
}
