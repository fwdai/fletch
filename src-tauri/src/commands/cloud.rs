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

/// Connect a provider with a pasted token, acting in `org` (absent or blank:
/// `personal`). The token is validated against the provider before anything
/// is stored, so a bad paste fails loudly and changes nothing.
#[tauri::command]
pub async fn cloud_connect(
    db: State<'_, Db>,
    provider: String,
    token: String,
    org: Option<String>,
) -> Result<CloudProviderStatus> {
    cloud::connect(db.inner(), provider_id(&provider)?, &token, org.as_deref()).await
}

/// Forget a provider's token and org. Machines that exist at the provider are
/// not touched: they stay there, running or stopped, out of this machine's
/// reach until it connects again, and what becomes of their boxes is the box
/// registry's decision.
#[tauri::command]
pub fn cloud_disconnect(db: State<'_, Db>, provider: String) -> Result<()> {
    cloud::disconnect(&db.lock(), provider_id(&provider)?)
}
