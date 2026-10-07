//! Provider accounts: the Settings › Providers list of sign-ins per CLI, and
//! which one new agents use. Thin over `agent::accounts`, which owns the
//! directories; this layer adds the one `settings` row per provider and the
//! change event, like every other host-owned setting.

use std::sync::Arc;

use crate::agent::accounts::{self, ProviderAccount};
use crate::database;
use crate::error::{Error, Result};
use crate::host::EngineCtx;

/// Every account of every account-capable provider, probed. Runs the probes
/// off the async runtime: each is a file read or a Keychain presence check.
pub async fn list_provider_accounts_impl(ctx: &Arc<EngineCtx>) -> Result<Vec<ProviderAccount>> {
    // Read the settings up front so the blocking probe never holds the DB.
    let active: Vec<(String, Option<String>)> = {
        let conn = ctx.db.lock();
        accounts::ACCOUNT_PROVIDERS
            .iter()
            .map(|p| {
                (
                    p.to_string(),
                    database::get_setting(&conn, &accounts::active_setting_key(p)),
                )
            })
            .collect()
    };
    tokio::task::spawn_blocking(move || {
        accounts::list_accounts(|provider| {
            active
                .iter()
                .find(|(p, _)| p == provider)
                .and_then(|(_, id)| id.clone())
                .filter(|id| !accounts::is_default(id))
        })
    })
    .await
    .map_err(|e| Error::Other(format!("account probe failed: {e}")))
}

/// Create a managed account directory. Signing in is a separate step (the
/// login PTY, run with this account's env), so a fresh account lists as
/// signed out until then.
pub fn add_provider_account_impl(provider: &str, id: &str) -> Result<()> {
    if accounts::list_account_ids(provider).iter().any(|known| known == id) {
        return Err(Error::Other(format!("An account named `{id}` already exists.")));
    }
    accounts::ensure_account_dir(provider, id)?;
    Ok(())
}

/// Delete a managed account directory — its login, its transcripts, its links.
/// The active account can't be removed; pick another first.
pub fn remove_provider_account_impl(ctx: &EngineCtx, provider: &str, id: &str) -> Result<()> {
    let active = database::get_setting(&ctx.db.lock(), &accounts::active_setting_key(provider));
    if active.as_deref() == Some(id) {
        return Err(Error::Other(
            "This account is in use. Choose another account before removing it.".into(),
        ));
    }
    accounts::remove_account_dir(provider, id)
}

/// Name the account new agents of `provider` use. `None` or the default id
/// means the CLI's own login. A managed id must exist on disk.
pub fn set_active_provider_account_impl(
    ctx: &EngineCtx,
    provider: &str,
    id: Option<&str>,
) -> Result<()> {
    if !accounts::supports_accounts(provider) {
        return Err(Error::Other(format!("`{provider}` has no account directories.")));
    }
    let id = id.filter(|id| !accounts::is_default(id));
    if let Some(id) = id {
        accounts::validate_account_id(id)?;
        if !accounts::account_dir(provider, id)?.is_dir() {
            return Err(Error::Other(format!("No account named `{id}`.")));
        }
    }
    let key = accounts::active_setting_key(provider);
    let value = id.unwrap_or(accounts::DEFAULT_ACCOUNT);
    database::set_setting(&ctx.db.lock(), &key, value)?;
    super::settings::announce(ctx, &key, Some(value));
    Ok(())
}
