//! Provider limits (Settings › Providers): each account's five-hour and
//! weekly readings, and the Refresh that asks the vendor again. Thin wrappers
//! over `commands::limits` in the engine. Local to this Mac, like the accounts
//! themselves: the logins the refresh reads live on this disk.

use std::collections::BTreeMap;
use std::sync::Arc;
use tauri::State;

use crate::agent::limits::AccountLimits;
use crate::error::Result;
use crate::host::EngineCtx;
use fletch_core::commands as engine;

/// Every account of `provider` with its stored limits, keyed by account id.
/// Reads the settings rows only.
#[tauri::command]
pub fn get_provider_limits(
    ctx: State<'_, Arc<EngineCtx>>,
    provider: String,
) -> Result<BTreeMap<String, AccountLimits>> {
    engine::get_provider_limits_impl(&ctx, &provider)
}

/// Ask the vendor for one account's limits now (`account` is a managed id or
/// `default`) and return the row as stored. Inside the refresh floor or a 429
/// back-off nothing is asked and the stored row comes back.
#[tauri::command]
pub async fn refresh_provider_limits(
    ctx: State<'_, Arc<EngineCtx>>,
    provider: String,
    account: String,
) -> Result<AccountLimits> {
    engine::refresh_provider_limits_impl(&ctx, &provider, &account).await
}
