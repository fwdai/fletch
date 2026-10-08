//! Keeping a live claude on a valid login. A claude process reads its
//! `CLAUDE_CODE_OAUTH_TOKEN` once at start and can't refresh it, so the only
//! way onto a new token is a relaunch that resumes the session. Two triggers:
//! a turn about to start on a token inside the refresh margin, and a turn
//! that ended on a 401 anyway (it outran the margin, or the token was revoked
//! server-side), which gets exactly one refresh, relaunch and re-delivery.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::agent::claude_oauth;
use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::message_queue::PendingMsg;
use crate::workspace::AgentStatus;

use super::lifecycle::arm_spawn_timeout;
use super::Supervisor;

const GAVE_UP_MSG: &str = "Claude rejected this account's login again after a refresh. \
     Sign in again under Settings → Providers, then resend.";

#[derive(Default)]
pub(super) struct Logins {
    /// The expiry of a token the API rejected, per agent: the agent's next
    /// launch replaces that token even though it looks unexpired.
    pub(super) rejected: HashMap<String, i64>,
    /// Agents whose current turn is already the one retry.
    retrying: HashSet<String>,
    /// The last turn Fletch delivered to each agent, for the retry to resend.
    last_turn: HashMap<String, PendingMsg>,
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

/// Whether a stream-json event is the `result` of a turn the API refused to
/// authenticate. claude 2.1.x reports the status as `api_error_status`; older
/// builds only put it in the result text.
fn is_auth_rejection(event: &Value) -> bool {
    if event.get("type").and_then(Value::as_str) != Some("result") {
        return false;
    }
    event.get("api_error_status").and_then(Value::as_u64) == Some(401)
        || event
            .get("result")
            .and_then(Value::as_str)
            .is_some_and(|text| text.contains("API Error: 401"))
}

impl Supervisor {
    /// Stop `agent_id`'s process and start it again on its own session,
    /// re-reading the record — so the launch resolves a fresh login token, and
    /// whatever else the record now says. `switch_view`'s teardown without the
    /// view change, under the same lifecycle lock. The caller decides the agent
    /// is safe to stop: this does not wait for a running turn.
    pub async fn relaunch_with_resume(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
        agent_id: &str,
    ) -> Result<()> {
        let _lifecycle_guard = self.agent_lifecycle.lock().await;
        let record = self.workspace.agent(agent_id)?;
        if self.deleting_projects.lock().contains(&record.project_id) {
            return Err(Error::Other("project deletion is in progress".into()));
        }
        // Retire the old generation first, so its exit can't land on the new
        // process's status while the old one winds down.
        self.bump_generation(agent_id);
        let taken = self.agents.lock().remove(agent_id);
        if let Some(agent) = taken {
            let _ = agent.shutdown();
        }
        self.activities.lock().remove(agent_id);
        self.native_inputs.lock().remove(agent_id);

        self.set_status(ctx, agent_id, AgentStatus::Spawning, None);
        arm_spawn_timeout(self.clone(), ctx.clone(), agent_id.to_string());
        tokio::time::sleep(Duration::from_millis(150)).await;

        if let Err(e) = self.start_process(ctx, agent_id).await {
            let err = e.to_string();
            self.set_status(ctx, agent_id, AgentStatus::Error, Some(err));
            return Err(e);
        }
        Ok(())
    }

    /// Before a turn is handed to an idle claude process, relaunch it if the
    /// token it runs on is inside the refresh margin: the turn would otherwise
    /// start on a token that lapses under it. A failed relaunch leaves the
    /// agent in `Error` with the reason (a revoked login reads "sign in
    /// again"), and the caller's delivery then holds the message.
    pub(super) async fn relaunch_if_login_due(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
        agent_id: &str,
    ) {
        let expiry = self
            .agents
            .lock()
            .get(agent_id)
            .and_then(|agent| agent.login_expires_at_ms());
        let Some(expiry) = expiry else {
            return;
        };
        if !claude_oauth::needs_refresh(expiry, claude_oauth::now_ms()) || self.is_busy(agent_id) {
            return;
        }
        tracing::info!(
            agent_id,
            "claude login near expiry; relaunching onto a fresh token"
        );
        if let Err(e) = self.relaunch_with_resume(ctx, agent_id).await {
            tracing::warn!(agent_id, error = %e, "relaunch onto a fresh claude login failed");
        }
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
    /// (`claude_oauth::launch_token`). A rejection of the retry is final.
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
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::pty_session::{PtySession, PtySpawn};
    use crate::sandbox::KillHandle;
    use crate::supervisor::tests::test_supervisor;
    use serde_json::json;

    fn rejected_result() -> Value {
        json!({
            "type": "result",
            "is_error": true,
            "api_error_status": 401,
            "result": "Failed to authenticate. API Error: 401 OAuth access token has expired.",
        })
    }

    fn ok_result() -> Value {
        json!({"type": "result", "is_error": false, "result": "done"})
    }

    fn msg() -> PendingMsg {
        PendingMsg {
            turn_id: "t1".into(),
            text: "fix the bug".into(),
            attachments: Vec::new(),
        }
    }

    fn live_with_login(sup: &Supervisor, dir: &std::path::Path, expires_at_ms: i64) {
        let pty = PtySession::spawn(
            PtySpawn {
                program: std::path::Path::new("/bin/cat"),
                args: &[],
                cwd: dir,
                env: &[],
                cols: 80,
                rows: 24,
                env_remove: &[],
                kill_plan: KillHandle::ProcessGroup,
            },
            |_| {},
            |_| {},
        )
        .unwrap();
        sup.agents.lock().insert(
            "a1".into(),
            Arc::new(Agent::over_pty_with_login(pty, expires_at_ms)),
        );
    }

    #[test]
    fn a_401_result_is_an_auth_rejection_and_other_results_are_not() {
        assert!(is_auth_rejection(&rejected_result()));
        assert!(is_auth_rejection(&json!({
            "type": "result",
            "result": "Failed to authenticate. API Error: 401 OAuth access token is invalid.",
        })));
        assert!(!is_auth_rejection(&ok_result()));
        assert!(!is_auth_rejection(
            &json!({"type": "result", "api_error_status": 529})
        ));
        assert!(!is_auth_rejection(
            &json!({"type": "assistant", "api_error_status": 401})
        ));
    }

    #[tokio::test]
    async fn a_first_rejection_queues_the_turn_and_a_respawn_onto_a_new_token() {
        let td = tempfile::tempdir().unwrap();
        let sup = test_supervisor();
        live_with_login(&sup, td.path(), 42);
        sup.remember_turn("a1", &msg());

        assert_eq!(
            sup.observe_login("a1", &rejected_result()),
            LoginVerdict::Retrying
        );
        assert!(sup.respawn_pending.lock().contains("a1"));
        assert_eq!(sup.logins.lock().rejected.get("a1"), Some(&42));
        assert_eq!(
            sup.message_queue
                .lock()
                .drain_coalesced("a1")
                .map(|m| m.text),
            Some("fix the bug".to_string())
        );
    }

    #[tokio::test]
    async fn a_rejected_retry_gives_up_and_a_good_turn_resets_the_budget() {
        let td = tempfile::tempdir().unwrap();
        let sup = test_supervisor();
        live_with_login(&sup, td.path(), 42);

        assert_eq!(
            sup.observe_login("a1", &rejected_result()),
            LoginVerdict::Retrying
        );
        assert_eq!(
            sup.observe_login("a1", &rejected_result()),
            LoginVerdict::GaveUp
        );
        // The budget is per failure, not per agent lifetime.
        assert_eq!(
            sup.observe_login("a1", &rejected_result()),
            LoginVerdict::Retrying
        );
        assert_eq!(sup.observe_login("a1", &ok_result()), LoginVerdict::Fine);
        assert_eq!(
            sup.observe_login("a1", &rejected_result()),
            LoginVerdict::Retrying
        );
    }

    #[tokio::test]
    async fn a_rejection_without_a_host_token_is_left_alone() {
        let sup = test_supervisor();
        assert_eq!(
            sup.observe_login("a1", &rejected_result()),
            LoginVerdict::Fine
        );
        assert!(!sup.respawn_pending.lock().contains("a1"));
    }
}
