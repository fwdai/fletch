use std::sync::Arc;

use crate::agent::accounts;
use crate::agent::AuthStatus;
use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::workspace::{AgentRecord, AgentStatus};

use super::events::emit_workspace_changed;
use super::Supervisor;

const BUSY_MSG: &str = "Wait for the turn to finish before switching accounts.";

fn label(stamp: Option<&str>) -> &str {
    stamp.unwrap_or(accounts::DEFAULT_ACCOUNT)
}

/// The stamp a switch of `record` to `requested` would write (`None` for the
/// default), or why the switch is refused. Reads the record and the accounts
/// root only; the login probe and the busy check come after.
fn target_stamp(record: &AgentRecord, requested: &str) -> Result<Option<String>> {
    if record.archive.is_some() {
        return Err(Error::Other("agent is archived".into()));
    }
    let provider = record.provider.as_str();
    if !accounts::supports_accounts(provider) {
        return Err(Error::Other(format!(
            "`{provider}` has no accounts to switch between."
        )));
    }
    // Such a provider keeps its sessions in the account's own directory, so
    // the other account's launch would find nothing to resume.
    if accounts::launches_in_account_dir(provider) {
        return Err(Error::Other(format!(
            "Switching accounts mid-conversation isn't available for {provider} yet: \
             its sessions live in each account's own directory, so the other account \
             can't resume this one."
        )));
    }
    let requested = requested.trim();
    let target = (!accounts::is_default(requested)).then(|| requested.to_string());
    let current = record
        .account
        .as_deref()
        .filter(|id| !accounts::is_default(id));
    if target.as_deref() == current {
        return Err(Error::Other(format!(
            "This agent already runs under the `{}` account.",
            label(current)
        )));
    }
    if let Some(id) = target.as_deref() {
        if !accounts::account_dir(provider, id)?.is_dir() {
            return Err(Error::Other(format!("No {provider} account named `{id}`.")));
        }
    }
    Ok(target)
}

/// Refuse a target whose login the probe reads as signed out. `Unknown` is let
/// through: the launch is the final word, and a wrong refusal would send the
/// user chasing a login they already have. Blocking: a Keychain presence check
/// or a file read.
fn ensure_signed_in(provider: &str, target: Option<&str>) -> Result<()> {
    let (status, detail) =
        accounts::probe_account(provider, target.unwrap_or(accounts::DEFAULT_ACCOUNT))?;
    if status != AuthStatus::SignedOut {
        return Ok(());
    }
    let why = detail.map(|d| format!(" ({d})")).unwrap_or_default();
    Err(Error::Other(format!(
        "The `{}` account isn't signed in{why}. Sign in under Settings → Providers, then switch.",
        label(target)
    )))
}

impl Supervisor {
    /// Move `agent_id`'s workspace onto another account of its provider (an
    /// id, or `default`), keeping the workspace and the conversation. Refused
    /// mid-turn; otherwise the stamp is persisted and a live agent is
    /// relaunched on its own session under it, so the next turn runs on the
    /// new account. That includes a per-turn agent between turns: its handle
    /// froze the account into its launch spec. A session with no process
    /// needs nothing more: whatever launches it next reads the new stamp. A
    /// relaunch that fails puts the old stamp back, so a refusal never leaves
    /// the agent on an account it couldn't start under.
    pub async fn switch_account(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
        agent_id: &str,
        account: &str,
    ) -> Result<AgentRecord> {
        // Rewind's prologue. The existence check comes first, so an unknown id
        // never creates a delivery lock. The delivery lock is a send's, so a
        // send that arrives meanwhile waits and then routes to the process
        // launched under the new stamp. The route keeps an archive from
        // tearing the checkout down under the relaunch.
        self.workspace.agent(agent_id)?;
        let _delivering = self.lock_delivery(agent_id).await;
        let _route = self.open_route(agent_id)?;

        // The probe and the target's token can take a Keychain read or a
        // refresh, so they run before the lifecycle lock every agent shares.
        let record = self.workspace.agent(agent_id)?;
        let target = target_stamp(&record, account)?;
        let provider = record.provider.clone();
        let probed = target.clone();
        tokio::task::spawn_blocking(move || ensure_signed_in(&provider, probed.as_deref()))
            .await
            .map_err(|e| Error::Other(format!("account probe failed: {e}")))??;
        if self.agents.lock().contains_key(agent_id) {
            if let Err(e) = self
                .prefetch_login_as(agent_id, &record.provider, target.as_deref(), None)
                .await
            {
                self.discard_prefetch(agent_id);
                return Err(Error::Other(format!(
                    "Couldn't sign in to the `{}` account: {e}",
                    label(target.as_deref())
                )));
            }
        }

        let lifecycle_guard = self.agent_lifecycle.lock().await;
        let record = self.workspace.agent(agent_id)?;
        let checked = target_stamp(&record, account).and_then(|target| {
            if self.is_busy(agent_id) {
                Err(Error::Other(BUSY_MSG.into()))
            } else {
                Ok(target)
            }
        });
        let target = match checked {
            Ok(target) => target,
            Err(e) => {
                self.discard_prefetch(agent_id);
                return Err(e);
            }
        };

        self.workspace
            .update_agent_account(agent_id, target.as_deref())?;
        // A token the API rejected belonged to the old account; replacing it
        // would force a refresh of the new one for nothing.
        let rejected = self.logins.lock().rejected.remove(agent_id);
        let live = self.agents.lock().contains_key(agent_id);
        if live {
            if let Err(e) = self.relaunch_locked(ctx, agent_id).await {
                self.discard_prefetch(agent_id);
                if let Err(undo) = self
                    .workspace
                    .update_agent_account(agent_id, record.account.as_deref())
                {
                    tracing::warn!(agent_id, error = %undo, "restoring the account stamp failed");
                }
                if let Some(expiry) = rejected {
                    self.logins
                        .lock()
                        .rejected
                        .insert(agent_id.to_string(), expiry);
                }
                emit_workspace_changed(ctx.sink.as_ref());
                if e.to_string() == super::login_refresh::BUSY_MSG {
                    return Err(Error::Other(BUSY_MSG.into()));
                }
                return Err(Error::Other(format!(
                    "Couldn't start the agent under the `{}` account, so it stays on `{}`: {e}",
                    label(target.as_deref()),
                    label(record.account.as_deref()),
                )));
            }
        } else {
            self.discard_prefetch(agent_id);
            // The failure was the old account's; the next launch is the new
            // one's to judge.
            if self.effective_status(agent_id, &record) == AgentStatus::Error {
                if let Err(e) = self.workspace.clear_agent_error(agent_id) {
                    tracing::warn!(agent_id, error = %e, "clearing the old account's error failed");
                }
                self.set_status(ctx, agent_id, AgentStatus::Idle, None);
            }
        }
        drop(lifecycle_guard);
        tracing::info!(
            agent_id,
            from = label(record.account.as_deref()),
            to = label(target.as_deref()),
            relaunched = live,
            "switched the agent's provider account"
        );
        emit_workspace_changed(ctx.sink.as_ref());
        self.agent_record(agent_id)
            .ok_or_else(|| Error::AgentNotFound(agent_id.to_string()))
    }
}

#[cfg(test)]
mod tests;
