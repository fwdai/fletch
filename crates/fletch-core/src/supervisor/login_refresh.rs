//! Keeping a live claude on a valid login. A claude process reads its
//! `CLAUDE_CODE_OAUTH_TOKEN` once at start and can't refresh it, so the only
//! way onto a new token is a relaunch that resumes the session. Two triggers:
//! a turn about to start on a token inside the refresh margin, and a turn
//! that ended on a 401 anyway (it outran the margin, or the token was revoked
//! server-side), which gets one refresh, relaunch and re-delivery.
//!
//! Every launch's token is resolved ahead of the launch where the caller can
//! (`prefetch_login`): outside the lifecycle lock every agent shares, and
//! before the spawn watchdog starts counting, so a slow sign-in server or a
//! Keychain prompt costs neither.
//!
//! A turn that ended on a spent quota rides the same relaunch-and-resend
//! path (`observe_limit`): when Settings names another account as the active
//! one, the agent is restamped onto it and the turn is resent once from
//! there. Both account providers' terminal events are read for it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::agent::accounts;
use crate::agent::host_login::claude::{self as claude_login, AccessToken};
use crate::agent::Agent;
use crate::database;
use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::message_queue::PendingMsg;
use crate::workspace::{AgentRecord, AgentStatus};

use super::account_switch::{ensure_signed_in, managed};
use super::events::emit_workspace_changed;
use super::lifecycle::arm_spawn_timeout;
use super::Supervisor;

const GAVE_UP_MSG: &str = "Claude rejected this account's login again after a refresh. \
     Sign in again under Settings → Providers, then resend.";

const BUSY_MSG: &str = "a turn is in progress";

/// How long a token resolved ahead of a launch stays good for that launch. A
/// fresh spawn provisions its checkout in between, which takes a while; the
/// token itself has at least the refresh margin left when it is resolved.
const PREFETCH_TTL_MS: i64 = 5 * 60 * 1000;

#[derive(Default)]
pub(super) struct Logins {
    /// The expiry of a token the API rejected, per agent: the agent's next
    /// launch replaces that token even though it looks unexpired. Cleared
    /// only once a launch has succeeded on the replacement.
    pub(super) rejected: HashMap<String, i64>,
    /// Agents whose current turn is already the one retry.
    retrying: HashSet<String>,
    /// The last turn Fletch delivered to each agent, for the retry to resend.
    last_turn: HashMap<String, PendingMsg>,
    /// Tokens resolved ahead of a launch (`prefetch_login`).
    pub(super) prefetched: HashMap<String, Prefetched>,
    /// Agents whose current turn the stream already called a spent quota — a
    /// `rate_limit_event` marked rejected, or claude's own `<synthetic>`
    /// message saying so — ahead of the result that closes it. That result
    /// is then a limit whatever it says: claude shrinks its text once the
    /// agent has spoken, and need not flag it as an error at all.
    limited: HashSet<String>,
    /// Agents restamped by the active-account fan-out while their process
    /// still ran on the old account's login (`follow_active_account`): the
    /// turn in flight ends on that login, and a limit it hits is the old
    /// account's, so it is resent once from the relaunch the fan-out already
    /// flagged. Cleared by the relaunch (`restart_taken`).
    moved: HashSet<String>,
}

pub(super) struct Prefetched {
    at_ms: i64,
    /// The stamp it was resolved for: a launch under any other stamp (an
    /// account switch, or a revive racing one) resolves its own.
    account: Option<String>,
    token: std::result::Result<Option<AccessToken>, String>,
}

impl Prefetched {
    fn fits(&self, record: &AgentRecord, now_ms: i64) -> bool {
        fn managed(a: Option<&str>) -> Option<&str> {
            a.filter(|id| !crate::agent::accounts::is_default(id))
        }
        now_ms - self.at_ms < PREFETCH_TTL_MS
            && managed(self.account.as_deref()) == managed(record.account.as_deref())
    }
}

/// How a relaunch under the caller's lifecycle lock went, short of an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Relaunch {
    Restarted,
    /// Mid-turn; nothing was stopped.
    Busy,
}

/// What an account switch takes of the old account's rejected login, to put
/// back if the switch fails.
pub(super) struct Rejection {
    expiry: Option<i64>,
    retrying: bool,
}

/// What a turn's terminal event says about the agent's login.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LoginVerdict {
    Fine,
    /// Rejected; a refresh, relaunch and resend are queued for the turn end.
    Retrying,
    /// Rejected on the retry too; only the user can fix it.
    GaveUp,
}

/// What an atomic idle check found of an agent's live process.
pub(super) enum Taken {
    /// Idle, and now out of the `agents` map: the caller owns its teardown.
    Agent(Arc<Agent>),
    /// Spawning or mid-turn; left running.
    Busy,
    /// No live process.
    Gone,
}

/// Whether a stream-json event is the `result` of a turn the API refused to
/// authenticate. claude 2.1.x reports the status as `api_error_status`; only
/// an error result without one falls back to the text, so an answer that
/// merely quotes "API Error: 401" is never taken for a rejection.
fn is_auth_rejection(event: &Value) -> bool {
    if event.get("type").and_then(Value::as_str) != Some("result")
        || event.get("is_error").and_then(Value::as_bool) != Some(true)
    {
        return false;
    }
    match event.get("api_error_status") {
        Some(status) => status.as_u64() == Some(401),
        None => event
            .get("result")
            .and_then(Value::as_str)
            .is_some_and(|text| text.contains("API Error: 401")),
    }
}

/// Whether a turn's terminal event says the account's quota is spent: a
/// claude `result` closing a turn the stream already called a limit
/// (`limited`, see `Logins::limited`), or an error result carrying a 429 or
/// naming a limit in its text; codex's `turn.failed` whose message does.
/// Short of the stream's own word, only an error result counts, so an answer
/// that merely discusses limits never does.
fn is_limit_rejection(event: &Value, limited: bool) -> bool {
    match event.get("type").and_then(Value::as_str) {
        Some("result") => {
            if limited {
                return true;
            }
            if event.get("is_error").and_then(Value::as_bool) != Some(true) {
                return false;
            }
            event.get("api_error_status").and_then(Value::as_u64) == Some(429)
                || event
                    .get("result")
                    .and_then(Value::as_str)
                    .is_some_and(names_a_limit)
        }
        Some("turn.failed") => event
            .pointer("/error/message")
            .and_then(Value::as_str)
            .is_some_and(names_a_limit),
        _ => false,
    }
}

/// The vendors' words for a spent quota: claude's "You've hit your session
/// limit · resets 1pm" and "usage limit reached", codex's "You've hit your
/// usage limit", a plain rate limit. A device or budget limit is not the
/// account's quota, and another account would not lift it.
fn names_a_limit(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    (text.contains("hit your") && text.contains("limit"))
        || text.contains("usage limit")
        || text.contains("rate limit")
        || (text.contains("limit reached")
            && !text.contains("device limit")
            && !text.contains("budget limit"))
}

/// Whether a stream event, ahead of the result, already says the turn hit a
/// limit: a claude `rate_limit_event` reporting the request refused, or the
/// `assistant` message claude writes itself (model `<synthetic>`, the CLI
/// talking to itself) whose text names one — "You've hit your session limit ·
/// resets 1pm" is such a message, and the result after it may say nothing.
fn is_limit_rejected_event(event: &Value) -> bool {
    match event.get("type").and_then(Value::as_str) {
        Some("rate_limit_event") => {
            event
                .pointer("/rate_limit_info/status")
                .and_then(Value::as_str)
                == Some("rejected")
        }
        Some("assistant") => {
            event.pointer("/message/model").and_then(Value::as_str) == Some("<synthetic>")
                && names_a_limit(&message_text(event))
        }
        _ => false,
    }
}

/// The text of a stream-json `assistant` event's message: a plain string, or
/// its text blocks joined.
fn message_text(event: &Value) -> String {
    match event.pointer("/message/content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Whether relaunching onto `fresh` buys the live process anything: only a
/// token that lapses later than the one it runs on does. Offline, or with the
/// sign-in server down, the resolved token is the same one, and a relaunch
/// would only restart the process onto the same expiry, every message.
fn improves(fresh: &Result<Option<AccessToken>>, live_expires_at_ms: i64) -> bool {
    matches!(fresh, Ok(Some(token)) if token.expires_at_ms() > live_expires_at_ms)
}

impl Supervisor {
    /// Resolve the login token `agent_id`'s next launch will use and keep it
    /// for that launch. Callers run it before the lifecycle lock and the spawn
    /// watchdog; a launch that finds nothing kept resolves inline. Also returns
    /// it, for a caller that wants to compare. `Ok(None)` for providers that
    /// take no host token.
    pub(super) async fn prefetch_login(&self, agent_id: &str) -> Result<Option<AccessToken>> {
        let record = self.workspace.agent(agent_id)?;
        let rejected = self.logins.lock().rejected.get(agent_id).copied();
        self.prefetch_login_as(
            agent_id,
            &record.provider,
            record.account.as_deref(),
            rejected,
        )
        .await
    }

    /// [`Self::prefetch_login`] for a launch under `account` rather than the
    /// record's current stamp: the account switch resolves its target's token
    /// before it restamps.
    pub(super) async fn prefetch_login_as(
        &self,
        agent_id: &str,
        provider: &str,
        account: Option<&str>,
        rejected: Option<i64>,
    ) -> Result<Option<AccessToken>> {
        if provider != "claude" {
            return Ok(None);
        }
        let token = claude_login::launch_token(account, rejected).await;
        let kept = match &token {
            Ok(token) => Ok(token.clone()),
            Err(e) => Err(e.to_string()),
        };
        self.logins.lock().prefetched.insert(
            agent_id.to_string(),
            Prefetched {
                at_ms: crate::agent::host_login::now_ms(),
                account: account.map(str::to_string),
                token: kept,
            },
        );
        token
    }

    /// Drop a token resolved for a launch that is no longer coming.
    pub(super) fn discard_prefetch(&self, agent_id: &str) {
        self.logins.lock().prefetched.remove(agent_id);
    }

    fn has_prefetched(&self, agent_id: &str, record: &AgentRecord) -> bool {
        let now = crate::agent::host_login::now_ms();
        self.logins
            .lock()
            .prefetched
            .get(agent_id)
            .is_some_and(|p| p.fits(record, now))
    }

    /// The token this launch of `record` signs in with: the one kept by
    /// `prefetch_login` while it is recent, else resolved now.
    pub(super) async fn launch_login(
        &self,
        agent_id: &str,
        record: &AgentRecord,
    ) -> Result<Option<AccessToken>> {
        let kept = self.logins.lock().prefetched.remove(agent_id);
        if let Some(kept) = kept.filter(|p| p.fits(record, crate::agent::host_login::now_ms())) {
            return kept.token.map_err(Error::Other);
        }
        let rejected = self.logins.lock().rejected.get(agent_id).copied();
        claude_login::launch_token(record.account.as_deref(), rejected).await
    }

    /// The launch replaced a rejected token, so the mark has done its job.
    pub(super) fn launch_succeeded(&self, agent_id: &str) {
        self.logins.lock().rejected.remove(agent_id);
    }

    /// Take the old account's rejected-token mark and spent retry out, on an
    /// account switch: the new account's token was never rejected, and its
    /// first 401 deserves its own retry.
    pub(super) fn take_rejection(&self, agent_id: &str) -> Rejection {
        let mut logins = self.logins.lock();
        Rejection {
            expiry: logins.rejected.remove(agent_id),
            retrying: logins.retrying.remove(agent_id),
        }
    }

    /// Put back what [`Self::take_rejection`] took, for a switch that failed.
    pub(super) fn restore_rejection(&self, agent_id: &str, taken: Rejection) {
        let mut logins = self.logins.lock();
        if let Some(expiry) = taken.expiry {
            logins.rejected.insert(agent_id.to_string(), expiry);
        }
        if taken.retrying {
            logins.retrying.insert(agent_id.to_string());
        }
    }

    /// Drop everything kept about `agent_id`'s login, on teardown.
    pub(super) fn forget_logins(&self, agent_id: &str) {
        let mut logins = self.logins.lock();
        logins.rejected.remove(agent_id);
        logins.retrying.remove(agent_id);
        logins.last_turn.remove(agent_id);
        logins.prefetched.remove(agent_id);
        logins.limited.remove(agent_id);
        logins.moved.remove(agent_id);
    }

    /// Note that the active-account fan-out restamped `agent_id` under a
    /// running process (see `Logins::moved`).
    pub(super) fn mark_moved(&self, agent_id: &str) {
        self.logins.lock().moved.insert(agent_id.to_string());
    }

    /// Take `agent_id`'s process out of the `agents` map if it is idle. The
    /// status check and the removal happen under one `agents` lock: a send can
    /// flip an agent Idle→Running on another thread (`transition_active`
    /// touches only `statuses`), and a keystroke into a native TUI can start a
    /// turn at any time, so a separate check-then-remove could tear down an
    /// in-flight turn.
    pub(super) fn take_idle(&self, agent_id: &str, record: &AgentRecord) -> Taken {
        let mut agents = self.agents.lock();
        if !agents.contains_key(agent_id) {
            return Taken::Gone;
        }
        if matches!(
            self.effective_status(agent_id, record),
            AgentStatus::Spawning | AgentStatus::Running
        ) {
            return Taken::Busy;
        }
        match agents.remove(agent_id) {
            Some(agent) => Taken::Agent(agent),
            None => Taken::Gone,
        }
    }

    /// Stop a taken process and start the agent again on its own session.
    /// Under the caller's lifecycle lock. A failed start leaves the agent in
    /// `Error` with the reason.
    pub(super) async fn restart_taken(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
        agent_id: &str,
        agent: Option<Arc<Agent>>,
    ) -> Result<()> {
        // Retire the old generation first, so its exit can't land on the new
        // process's status while the old one winds down.
        self.bump_generation(agent_id);
        // The launch below reads the record, so it runs under the current
        // stamp: nothing is left over from a restamp under the old process.
        self.logins.lock().moved.remove(agent_id);
        if let Some(agent) = agent {
            let _ = agent.shutdown();
        }
        self.activities.lock().remove(agent_id);
        self.native_inputs.lock().remove(agent_id);

        self.set_status(ctx, agent_id, AgentStatus::Spawning, None);
        arm_spawn_timeout(self.clone(), ctx.clone(), agent_id.to_string());
        // Let the old process fully release its session before resuming it.
        tokio::time::sleep(Duration::from_millis(150)).await;

        if let Err(e) = self.start_process(ctx, agent_id).await {
            let err = e.to_string();
            self.set_status(ctx, agent_id, AgentStatus::Error, Some(err));
            return Err(e);
        }
        Ok(())
    }

    /// Stop `agent_id`'s process and start it again on its own session,
    /// re-reading the record — so the launch resolves a fresh login token, and
    /// whatever else the record now says. Refused while a turn runs. Unlike
    /// `respawn_agent_preserving_session` it neither defers nor flushes the
    /// queue, so a caller holding the agent's delivery lock may use it.
    pub async fn relaunch_with_resume(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
        agent_id: &str,
    ) -> Result<()> {
        // A failure here is kept and surfaces from the launch, under the lock.
        let _ = self.prefetch_login(agent_id).await;
        let _lifecycle_guard = self.agent_lifecycle.lock().await;
        self.relaunch_locked(ctx, agent_id).await
    }

    /// [`Self::relaunch_with_resume`] for a caller already holding
    /// `agent_lifecycle`, which is not reentrant.
    pub(super) async fn relaunch_locked(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
        agent_id: &str,
    ) -> Result<()> {
        match self.try_relaunch_locked(ctx, agent_id).await? {
            Relaunch::Restarted => Ok(()),
            Relaunch::Busy => Err(Error::Other(BUSY_MSG.into())),
        }
    }

    /// [`Self::relaunch_locked`] with a busy agent reported as such, for a
    /// caller with its own words for it.
    pub(super) async fn try_relaunch_locked(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
        agent_id: &str,
    ) -> Result<Relaunch> {
        let record = self.workspace.agent(agent_id)?;
        if self.deleting_projects.lock().contains(&record.project_id) {
            return Err(Error::Other("project deletion is in progress".into()));
        }
        // A launch that can't sign in fails before the running process is
        // stopped, so it keeps working on the token it has.
        if !self.has_prefetched(agent_id, &record) {
            self.prefetch_login(agent_id).await?;
        } else if let Some(Prefetched { token: Err(e), .. }) =
            self.logins.lock().prefetched.get(agent_id)
        {
            return Err(Error::Other(e.clone()));
        }
        let agent = match self.take_idle(agent_id, &record) {
            Taken::Agent(agent) => Some(agent),
            Taken::Busy => return Ok(Relaunch::Busy),
            Taken::Gone => None,
        };
        self.restart_taken(ctx, agent_id, agent).await?;
        Ok(Relaunch::Restarted)
    }

    /// Before a turn is handed to an idle claude process, relaunch it if the
    /// token it runs on is inside the refresh margin and a later-lapsing one
    /// can be had: the turn would otherwise start on a token that lapses under
    /// it. The token is resolved first, outside the lifecycle lock; when it is
    /// no better (offline, sign-in server down), the turn goes to the current
    /// process. A failed relaunch leaves the agent in `Error` with the reason
    /// and the caller's delivery holds the message.
    pub(super) async fn relaunch_if_login_due(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
        agent_id: &str,
    ) {
        let Some(live) = self.login_due(agent_id) else {
            return;
        };
        let fresh = self.prefetch_login(agent_id).await;
        if !improves(&fresh, live) {
            self.logins.lock().prefetched.remove(agent_id);
            tracing::info!(
                agent_id,
                "claude login near expiry but no fresher token to be had; keeping the process"
            );
            return;
        }
        tracing::info!(
            agent_id,
            "claude login near expiry; relaunching onto a fresh token"
        );
        let _lifecycle_guard = self.agent_lifecycle.lock().await;
        if let Err(e) = self.relaunch_locked(ctx, agent_id).await {
            tracing::warn!(agent_id, error = %e, "relaunch onto a fresh claude login failed");
        }
    }

    /// The expiry of the token `agent_id`'s idle process runs on, when it is
    /// inside the refresh margin.
    fn login_due(&self, agent_id: &str) -> Option<i64> {
        let expiry = self
            .agents
            .lock()
            .get(agent_id)
            .and_then(|agent| agent.login_expires_at_ms())?;
        (claude_login::needs_refresh(expiry, crate::agent::host_login::now_ms())
            && !self.is_busy(agent_id))
        .then_some(expiry)
    }

    /// Record the turn just delivered, for a login retry to resend.
    pub(super) fn remember_turn(&self, agent_id: &str, msg: &PendingMsg) {
        self.logins
            .lock()
            .last_turn
            .insert(agent_id.to_string(), msg.clone());
    }

    /// Read a managed claude event for a login rejection. Must run *before*
    /// the event reaches the turn-closing handler: the first rejection flags
    /// a session-preserving respawn, which the turn-end transition then runs
    /// instead of the ordinary queue drain (`drain_pending_respawn`), and the
    /// rejected turn is put back at the head of the queue for the respawn's
    /// flush to resend. The respawn's launch replaces the rejected token
    /// (`host_login::claude::launch_token`). A rejection of the retry gives up.
    ///
    /// The budget is one retry per attempt, not per agent: giving up clears
    /// it, so the user's next send after signing in again gets its own
    /// refresh and relaunch — which is how the new sign-in reaches a process
    /// still running on the dead token.
    pub(super) fn observe_login(&self, agent_id: &str, event: &Value) -> LoginVerdict {
        if event.get("type").and_then(Value::as_str) != Some("result") {
            return LoginVerdict::Fine;
        }
        if !is_auth_rejection(event) {
            self.logins.lock().retrying.remove(agent_id);
            return LoginVerdict::Fine;
        }
        // A process launched without a host token signed in some other way
        // (an API key, the stored setup-token); nothing here can renew it.
        let Some(expiry) = self
            .agents
            .lock()
            .get(agent_id)
            .and_then(|agent| agent.login_expires_at_ms())
        else {
            return LoginVerdict::Fine;
        };
        let retry = {
            let mut logins = self.logins.lock();
            if !logins.retrying.insert(agent_id.to_string()) {
                logins.retrying.remove(agent_id);
                return LoginVerdict::GaveUp;
            }
            logins.rejected.insert(agent_id.to_string(), expiry);
            logins.last_turn.get(agent_id).cloned()
        };
        tracing::warn!(
            agent_id,
            "claude login rejected mid-session; refreshing and resending once"
        );
        if let Some(msg) = retry {
            self.message_queue.lock().requeue_front(agent_id, msg);
        }
        self.respawn_pending.lock().insert(agent_id.to_string());
        LoginVerdict::Retrying
    }

    /// Read a turn's events for a spent quota, for either account provider.
    /// Must run *before* the event reaches the turn-closing handler, like
    /// `observe_login`, and before it: a limit result is not a login
    /// rejection, and the other reader would clear the retry mark this sets.
    ///
    /// When Settings names another account as the active one, the agent is
    /// restamped onto it, the turn is put back at the head of the queue, and
    /// a session-preserving respawn is flagged for the turn end: its launch
    /// signs in as the new account (claude) or copies its login into the
    /// overlay (codex), and its flush resends the turn. One retry per
    /// attempt; a limit under the active account itself — including on that
    /// retry — leaves the vendor's error standing in the chat, which is the
    /// user's cue that every account they chose from is spent. The active
    /// account must exist and probe as signed in, else nothing moves. No other
    /// account is ever picked: the selection is the user's.
    pub(super) fn observe_limit(
        &self,
        ctx: &Arc<EngineCtx>,
        agent_id: &str,
        event: &Value,
    ) -> LoginVerdict {
        if is_limit_rejected_event(event) {
            self.logins.lock().limited.insert(agent_id.to_string());
            return LoginVerdict::Fine;
        }
        let terminal = matches!(
            event.get("type").and_then(Value::as_str),
            Some("result") | Some("turn.failed")
        );
        if !terminal {
            return LoginVerdict::Fine;
        }
        let limited = self.logins.lock().limited.remove(agent_id);
        if !is_limit_rejection(event, limited) {
            return LoginVerdict::Fine;
        }
        self.retry_on_active_account(ctx, agent_id)
    }

    fn retry_on_active_account(&self, ctx: &Arc<EngineCtx>, agent_id: &str) -> LoginVerdict {
        let Ok(record) = self.workspace.agent(agent_id) else {
            return LoginVerdict::Fine;
        };
        let provider = record.provider.as_str();
        if !accounts::supports_accounts(provider) {
            return LoginVerdict::Fine;
        }
        let active = database::get_setting(&ctx.db.lock(), &accounts::active_setting_key(provider));
        let active = managed(active.as_deref()).map(str::to_string);
        let current = managed(record.account.as_deref());
        let label = |stamp: Option<&str>| stamp.unwrap_or(accounts::DEFAULT_ACCOUNT).to_string();
        if active.as_deref() == current {
            let (was_retry, moved) = {
                let mut logins = self.logins.lock();
                (
                    logins.retrying.remove(agent_id),
                    logins.moved.contains(agent_id),
                )
            };
            if was_retry {
                tracing::warn!(
                    agent_id,
                    account = label(current),
                    "the turn hit a usage limit under the active account too; the error stands"
                );
                return LoginVerdict::Fine;
            }
            if !moved {
                tracing::info!(
                    agent_id,
                    account = label(current),
                    "the turn hit a usage limit under the active account; nothing to retry on"
                );
                return LoginVerdict::Fine;
            }
            // The stamp says the active account, but the process that ran this
            // turn was launched before the fan-out restamped it; the limit is
            // the old account's. The relaunch the fan-out flagged brings the
            // new login; the turn rides its flush.
            let retry = {
                let mut logins = self.logins.lock();
                logins.retrying.insert(agent_id.to_string());
                logins.last_turn.get(agent_id).cloned()
            };
            tracing::warn!(
                agent_id,
                to = label(current),
                "the turn hit a usage limit on the login it started on; resending it once under the active account"
            );
            if let Some(msg) = retry {
                self.message_queue.lock().requeue_front(agent_id, msg);
            }
            self.respawn_pending.lock().insert(agent_id.to_string());
            return LoginVerdict::Retrying;
        }
        if !self.logins.lock().retrying.insert(agent_id.to_string()) {
            self.logins.lock().retrying.remove(agent_id);
            tracing::warn!(
                agent_id,
                "the retried turn hit a usage limit again; the error stands"
            );
            return LoginVerdict::Fine;
        }
        let target_exists = match active.as_deref() {
            Some(id) => accounts::account_dir(provider, id).is_ok_and(|dir| dir.is_dir()),
            None => true,
        };
        if !target_exists {
            self.logins.lock().retrying.remove(agent_id);
            tracing::warn!(
                agent_id,
                account = label(active.as_deref()),
                "the active account has no directory; the usage limit stands"
            );
            return LoginVerdict::Fine;
        }
        if let Err(e) = ensure_signed_in(provider, active.as_deref()) {
            self.logins.lock().retrying.remove(agent_id);
            tracing::warn!(agent_id, error = %e, "the active account can't take the turn; the usage limit stands");
            return LoginVerdict::Fine;
        }
        if let Err(e) = self
            .workspace
            .update_agent_account(agent_id, active.as_deref())
        {
            self.logins.lock().retrying.remove(agent_id);
            tracing::warn!(agent_id, error = %e, "restamping onto the active account failed; the usage limit stands");
            return LoginVerdict::Fine;
        }
        let retry = {
            let mut logins = self.logins.lock();
            // The old account's rejected token and prefetched login are its
            // own; the launch under the new stamp resolves its own.
            logins.rejected.remove(agent_id);
            logins.prefetched.remove(agent_id);
            logins.last_turn.get(agent_id).cloned()
        };
        tracing::warn!(
            agent_id,
            from = label(current),
            to = label(active.as_deref()),
            "the turn hit a usage limit; resending it once under the active account"
        );
        if let Some(msg) = retry {
            self.message_queue.lock().requeue_front(agent_id, msg);
        }
        self.respawn_pending.lock().insert(agent_id.to_string());
        emit_workspace_changed(ctx.sink.as_ref());
        LoginVerdict::Retrying
    }

    /// Surface a login that failed its retry, once the turn has closed.
    pub(super) fn report_login_failure(&self, ctx: &Arc<EngineCtx>, agent_id: &str) {
        self.set_status(
            ctx,
            agent_id,
            AgentStatus::Error,
            Some(GAVE_UP_MSG.to_string()),
        );
    }
}

#[cfg(test)]
mod tests;
