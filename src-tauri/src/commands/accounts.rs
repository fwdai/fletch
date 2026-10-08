//! Provider accounts (Settings › Providers): list, add, remove, and choose the
//! one new agents use. Thin wrappers over `commands::accounts` in the engine.
//! Local to this Mac, like the sign-in PTY: the directories live on this disk.

use std::sync::Arc;
use tauri::State;

use crate::agent::accounts::ProviderAccount;
use crate::error::Result;
use crate::host::EngineCtx;
use crate::supervisor::Supervisor;
use crate::workspace::AgentRecord;
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

/// Delete a managed account — its directory and its login. Refused while it is
/// the active one or any live agent runs under it. A sign-in still running for
/// it is killed first — once the removal is known to go ahead, so a refusal
/// leaves the sign-in alone — so nothing writes the account back after the
/// directory is gone.
#[tauri::command]
pub fn remove_provider_account(
    ctx: State<'_, Arc<EngineCtx>>,
    logins: State<'_, crate::provider_login::ProviderLoginSessions>,
    provider: String,
    id: String,
) -> Result<()> {
    engine::ensure_account_removable(&ctx, &provider, &id)?;
    // Dropping the session kills its PTY (see `ProviderLoginSessions`).
    logins
        .lock()
        .remove(&super::provider_login::session_key(&provider, Some(&id)));
    engine::remove_provider_account_impl(&ctx, &provider, &id)
}

/// Sign an account out with the CLI's own logout (`id` a managed id or
/// `default`). A sign-in still running for it is killed first, so it can't
/// write the login straight back.
#[tauri::command]
pub async fn sign_out_provider_account(
    logins: State<'_, crate::provider_login::ProviderLoginSessions>,
    provider: String,
    id: String,
) -> Result<()> {
    // Dropping the session kills its PTY (see `ProviderLoginSessions`).
    logins
        .lock()
        .remove(&super::provider_login::session_key(&provider, Some(&id)));
    engine::sign_out_provider_account_impl(&provider, &id).await
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

/// Move an agent onto another account of its provider (`account` is an id or
/// `default`) from its next turn. Resolves to the restamped record.
#[tauri::command]
pub async fn switch_agent_account(
    supervisor: State<'_, Arc<Supervisor>>,
    ctx: State<'_, Arc<EngineCtx>>,
    agent_id: String,
    account: String,
) -> Result<AgentRecord> {
    engine::switch_agent_account_impl(supervisor.inner(), ctx.inner(), &agent_id, &account).await
}
