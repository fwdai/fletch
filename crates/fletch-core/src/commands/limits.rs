//! Provider limits (Settings › Providers): the last known five-hour and weekly
//! readings per account, and the manual refresh that asks the vendor again.
//! Thin over `agent::limits`, which owns the shapes, the normalisation and the
//! settings rows; this layer resolves the account and picks the source.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::agent::accounts;
use crate::agent::limits::{self, AccountLimits};
use crate::error::{Error, Result};
use crate::host::EngineCtx;

/// Every account of `provider` with its stored limits, keyed by account id
/// (`default` first in id order, then the managed ones). A read of the
/// settings rows only — nothing is asked of the vendor.
pub fn get_provider_limits_impl(
    ctx: &EngineCtx,
    provider: &str,
) -> Result<BTreeMap<String, AccountLimits>> {
    require_account_provider(provider)?;
    Ok(limits::load_provider(&ctx.db.lock(), provider))
}

/// Ask the vendor for `account`'s limits now and store the answer. Refused
/// quietly — the stored row comes back unchanged — inside the refresh floor or
/// a 429 back-off, so an impatient click costs nothing. Signed-out, stale and
/// rate-limited answers are stored states, not errors; an error is something
/// the user can't read off the row (no binary, no network, a malformed reply).
pub async fn refresh_provider_limits_impl(
    ctx: &Arc<EngineCtx>,
    provider: &str,
    account: &str,
) -> Result<AccountLimits> {
    require_account_provider(provider)?;
    let account = Some(account).filter(|a| !accounts::is_default(a));
    let dir: Option<PathBuf> = accounts::existing_account_dir(provider, account)?;

    let now = limits::now_secs();
    let stored = limits::load(&ctx.db.lock(), provider, account);
    if !limits::refresh_allowed(stored.refresh.as_ref(), now) {
        return Ok(stored);
    }

    let outcome = match provider {
        "codex" => limits::app_server::read_limits(dir.as_deref()).await?,
        _ => {
            return Err(Error::Other(format!(
                "`{provider}` limits can't be refreshed on demand."
            )))
        }
    };
    limits::record_refresh(ctx, provider, account, outcome, limits::now_secs())
}

fn require_account_provider(provider: &str) -> Result<()> {
    if accounts::supports_accounts(provider) {
        Ok(())
    } else {
        Err(Error::Other(format!(
            "`{provider}` has no account directories."
        )))
    }
}
