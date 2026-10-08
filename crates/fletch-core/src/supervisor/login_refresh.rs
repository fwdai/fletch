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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::agent::claude_oauth::{self, AccessToken};
use crate::agent::Agent;
use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::message_queue::PendingMsg;
use crate::workspace::{AgentRecord, AgentStatus};

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
        let token = claude_oauth::launch_token(account, rejected).await;
        let kept = match &token {
            Ok(token) => Ok(token.clone()),
            Err(e) => Err(e.to_string()),
        };
        self.logins.lock().prefetched.insert(
            agent_id.to_string(),
            Prefetched {
                at_ms: claude_oauth::now_ms(),
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
        let now = claude_oauth::now_ms();
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
        if let Some(kept) = kept.filter(|p| p.fits(record, claude_oauth::now_ms())) {
            return kept.token.map_err(Error::Other);
        }
        let rejected = self.logins.lock().rejected.get(agent_id).copied();
        claude_oauth::launch_token(record.account.as_deref(), rejected).await
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
        (claude_oauth::needs_refresh(expiry, claude_oauth::now_ms()) && !self.is_busy(agent_id))
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
    /// (`claude_oauth::launch_token`). A rejection of the retry gives up.
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
