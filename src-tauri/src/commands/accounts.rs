//! Provider accounts (Settings › Providers): list, add, remove, and choose the
//! one new agents use. Thin wrappers over `commands::accounts` in the engine.
//! Local to this Mac, like the sign-in PTY: the directories live on this disk.

use std::sync::Arc;
use tauri::State;

use crate::agent::accounts::ProviderAccount;
use crate::error::Result;
use crate::host::EngineCtx;
use fletch_core::commands as engine;

/// Every account of every account-capable provider, each probed for its
/// login, with the active one flagged.
#[tauri::command]
pub async fn list_provider_accounts(
    ctx: State<'_, Arc<EngineCtx>>,
) -> Result<Vec<ProviderAccount>> {
    engine::list_provider_accounts_impl(&ctx).await
}

/// Create a managed account directory for `provider`, named `id`.
#[tauri::command]
pub fn add_provider_account(provider: String, id: String) -> Result<()> {
    engine::add_provider_account_impl(&provider, &id)
}

/// Delete a managed account directory. Refused while it is the active one.
#[tauri::command]
pub fn remove_provider_account(
    ctx: State<'_, Arc<EngineCtx>>,
    provider: String,
    id: String,
) -> Result<()> {
    engine::remove_provider_account_impl(&ctx, &provider, &id)
}

/// Name the account new agents of `provider` use; `None` means the CLI's own.
#[tauri::command]
pub fn set_active_provider_account(
    ctx: State<'_, Arc<EngineCtx>>,
    provider: String,
    id: Option<String>,
) -> Result<()> {
    engine::set_active_provider_account_impl(&ctx, &provider, id.as_deref())
}
