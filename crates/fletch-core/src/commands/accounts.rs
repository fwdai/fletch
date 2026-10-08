//! Provider accounts: the Settings › Providers list of sign-ins per CLI, and
//! which one new agents use. Thin over `agent::accounts`, which owns the
//! directories; this layer adds the one `settings` row per provider and the
//! change event, like every other host-owned setting.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

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
    if accounts::list_account_ids(provider)
        .iter()
        .any(|known| known == id)
    {
        return Err(Error::Other(format!(
            "An account named `{id}` already exists."
        )));
    }
    accounts::ensure_account_dir(provider, id)?;
    Ok(())
}

/// Whether a managed account may be removed right now: not while it is the
/// active one, and not while any live (non-archived) agent is stamped with it
/// — that agent's login and transcripts live in the directory, and a running
/// one would write it straight back. The error names the fix.
pub fn ensure_account_removable(ctx: &EngineCtx, provider: &str, id: &str) -> Result<()> {
    let conn = ctx.db.lock();
    let active = database::get_setting(&conn, &accounts::active_setting_key(provider));
    if active.as_deref() == Some(id) {
        return Err(Error::Other(
            "This account is in use. Choose another account before removing it.".into(),
        ));
    }
    let agents = crate::workspace::live_agents_on_account(&conn, provider, id)?;
    if agents > 0 {
        let noun = if agents == 1 { "agent" } else { "agents" };
        return Err(Error::Other(format!(
            "{agents} {noun} still run under this account. Archive them before removing it."
        )));
    }
    Ok(())
}

/// Delete a managed account directory — its login, its transcripts, its links.
/// Refused under the same conditions as [`ensure_account_removable`].
pub fn remove_provider_account_impl(ctx: &EngineCtx, provider: &str, id: &str) -> Result<()> {
    ensure_account_removable(ctx, provider, id)?;
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
        return Err(Error::Other(format!(
            "`{provider}` has no account directories."
        )));
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

/// How long a CLI's logout may run before it is taken as hung. It only clears
/// a local credential (and claude revokes its token), so this is generous.
const LOGOUT_TIMEOUT: Duration = Duration::from_secs(30);

/// Sign account `id` of `provider` out with the CLI's own logout, run against
/// the account's directory — for the default, the CLI's own, which the user's
/// terminal shares. The CLI clears its credential the way it stored it
/// (Keychain item or credentials file), so nothing here touches either. The
/// account itself stays, its sessions with it, and lists as signed out.
pub async fn sign_out_provider_account_impl(provider: &str, id: &str) -> Result<()> {
    let home =
        dirs::home_dir().ok_or_else(|| Error::Other("HOME directory not available".into()))?;
    let (bin, label) = crate::agent::provider_bin_label(provider)
        .ok_or_else(|| Error::Other(format!("unknown provider `{provider}`")))?;
    let program = crate::agent::resolve_agent_bin(provider, bin, label, &home)?;
    let mut cmd = logout_command(&program, provider, id, &home)?;

    let output = tokio::time::timeout(LOGOUT_TIMEOUT, cmd.output())
        .await
        .map_err(|_| {
            Error::Other(format!(
                "{label} logout did not finish within {}s.",
                LOGOUT_TIMEOUT.as_secs()
            ))
        })??;
    if output.status.success() {
        return Ok(());
    }
    // The CLI's own last word on why, if it gave one; logout prints no secret.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reason: String = stderr
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim().chars().take(200).collect())
        .unwrap_or_else(|| output.status.to_string());
    Err(Error::Other(format!("{label} logout failed: {reason}")))
}

/// The logout run for one account: the pinned argv, the user's login-shell
/// env, and for a managed account its config-dir var with the default's
/// ambient keys removed — the same layering a sign-in gets, so the logout
/// lands on exactly the login that sign-in wrote.
fn logout_command(
    program: &str,
    provider: &str,
    id: &str,
    home: &std::path::Path,
) -> Result<tokio::process::Command> {
    let args = crate::agent::logout_command(provider)
        .filter(|_| accounts::supports_accounts(provider))
        .ok_or_else(|| Error::Other(format!("`{provider}` has no in-app sign-out.")))?;
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .current_dir(home)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    crate::bin_resolve::apply_login_shell_env(cmd.as_std_mut());
    if !accounts::is_default(id) {
        if !accounts::account_dir(provider, id)?.is_dir() {
            return Err(Error::Other(format!("No account named `{id}`.")));
        }
        // After the login-shell layer, so its config-dir var can't stand in
        // for the account's own.
        cmd.envs(accounts::account_env(provider, id)?);
        for var in accounts::ambient_credential_vars(provider) {
            cmd.env_remove(var);
        }
    }
    Ok(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn env_of<'a>(cmd: &'a tokio::process::Command, var: &str) -> Option<Option<&'a OsStr>> {
        cmd.as_std()
            .get_envs()
            .find(|(k, _)| *k == OsStr::new(var))
            .map(|(_, v)| v)
    }

    #[test]
    fn a_managed_accounts_logout_runs_against_its_own_dir() {
        accounts::with_test_root(|root| {
            accounts::ensure_account_dir("claude", "work").unwrap();
            let cmd = logout_command("claude", "claude", "work", root).unwrap();
            let args: Vec<_> = cmd.as_std().get_args().collect();
            assert_eq!(args, ["auth", "logout"]);
            let dir = root.join("claude").join("work");
            assert_eq!(
                env_of(&cmd, "CLAUDE_CONFIG_DIR"),
                Some(Some(dir.as_os_str()))
            );
            // Removed, so an API key in the shell can't be what gets logged out.
            assert_eq!(env_of(&cmd, "ANTHROPIC_API_KEY"), Some(None));
        });
    }

    #[test]
    fn the_default_accounts_logout_runs_against_the_clis_own_dir() {
        accounts::with_test_root(|root| {
            let cmd = logout_command("codex", "codex", accounts::DEFAULT_ACCOUNT, root).unwrap();
            let args: Vec<_> = cmd.as_std().get_args().collect();
            assert_eq!(args, ["logout"]);
            // Nothing of Fletch's own: whatever the user's shell says, if anything.
            let shell = crate::bin_resolve::login_shell_env()
                .and_then(|env| env.get("CODEX_HOME"))
                .map(OsStr::new);
            assert_eq!(env_of(&cmd, "CODEX_HOME").flatten(), shell);
        });
    }

    #[test]
    fn logout_is_refused_for_a_missing_account_or_a_provider_without_one() {
        accounts::with_test_root(|root| {
            assert!(logout_command("claude", "claude", "nope", root).is_err());
            assert!(logout_command("cursor", "cursor", accounts::DEFAULT_ACCOUNT, root).is_err());
        });
    }
}
