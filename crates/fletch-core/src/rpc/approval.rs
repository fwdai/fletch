//! Human approval for an agent's credentialed publish. Off by default.
//!
//! Every path that does not produce an explicit approval — no window listening,
//! a timeout, a dropped channel — resolves to denied, so the gate cannot fail
//! open. Autopilot and live Git-panel delegations are answered without a prompt
//! by the UI (`store/publishApproval.ts`), which owns the state that decides it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;
use serde_json::json;
use tokio::sync::oneshot;

use crate::host::EventSink;

/// Anything but `"true"` means off.
pub const SETTING: &str = "publish_confirmation";

/// Payload `{ id, agent_id, op, repo, detail }`; `op` and `repo` let the UI
/// match a standing per-checkout authorization.
pub const EVENT_REQUESTED: &str = "publish:approval-requested";

/// `0` waits until answered.
pub const WAIT_SETTING: &str = "publish_approval_wait";

pub const DEFAULT_WAIT_SECS: u64 = 120;

/// Mirror of [`SETTING`] for the dispatcher, which has no DB handle.
static ENABLED: AtomicBool = AtomicBool::new(false);

static WAIT_SECS: AtomicU64 = AtomicU64::new(DEFAULT_WAIT_SECS);

/// A question waiting for an answer: the channel [`answer`] resolves, plus what
/// was asked. The description travels with the sender rather than in a second
/// registry so a request cannot be listed after it has been answered — the two
/// are added and removed together.
struct Request {
    answer: oneshot::Sender<bool>,
    info: PendingApproval,
}

/// One unanswered publish approval, as a client that was not connected when the
/// event fired sees it.
///
/// The field names mirror the [`EVENT_REQUESTED`] payload (hence `agent_id`, not
/// `agentId`), so anything that already renders the event's card renders these
/// with the same code. `requested_at` is the only addition: a list has no
/// arrival order without it.
#[derive(Debug, Clone, Serialize)]
pub struct PendingApproval {
    pub id: String,
    pub agent_id: String,
    pub op: String,
    pub repo: Option<String>,
    pub detail: String,
    pub requested_at: String,
}

static PENDING: Mutex<Option<HashMap<String, Request>>> = Mutex::new(None);

pub fn parse_enabled(raw: Option<&str>) -> bool {
    raw == Some("true")
}

pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn parse_wait_secs(raw: Option<&str>) -> u64 {
    raw.and_then(|s| s.trim().parse().ok())
        .unwrap_or(DEFAULT_WAIT_SECS)
}

pub fn set_wait_secs(secs: u64) {
    WAIT_SECS.store(secs, Ordering::Relaxed);
}

pub fn wait_secs() -> u64 {
    WAIT_SECS.load(Ordering::Relaxed)
}

/// `detail` is shown verbatim so the user approves a specific act, not a category.
pub async fn refuse_unless_approved(
    sink: &dyn EventSink,
    agent_id: &str,
    op: &str,
    repo: Option<&str>,
    detail: &str,
) -> Option<String> {
    if !enabled() {
        return None;
    }
    let (id, answer) = register(agent_id, op, repo, detail);
    // The one emit in the engine whose failure matters: an unasked question is
    // a denial, so this goes through the sink directly rather than through the
    // logging `host::emit`.
    if sink
        .emit_value(
            EVENT_REQUESTED,
            json!({
                "id": id,
                "agent_id": agent_id,
                "op": op,
                "repo": repo,
                "detail": detail,
            }),
        )
        .is_err()
    {
        forget(&id);
        return Some(refusal(detail, "no window is available to approve it"));
    }
    let wait = wait_secs();
    let answered = if wait == 0 {
        Ok(answer.await)
    } else {
        tokio::time::timeout(Duration::from_secs(wait), answer).await
    };
    match answered {
        Ok(Ok(true)) => None,
        Ok(Ok(false)) => Some(refusal(detail, "you declined it")),
        // Dropped sender or expired wait: both are "nobody approved".
        Ok(Err(_)) => {
            forget(&id);
            Some(refusal(detail, "the approval prompt was dismissed"))
        }
        Err(_) => {
            forget(&id);
            Some(refusal(detail, &format!("nobody answered within {wait}s")))
        }
    }
}

/// Unknown ids are ignored: a late answer must not publish anything.
pub fn answer(id: &str, approved: bool) {
    if let Some(request) = take(id) {
        let _ = request.answer.send(approved);
    }
}

/// Every question still waiting, oldest first.
///
/// The desktop never needs this — its window was listening when the event fired
/// — but a host serves clients that connect *after* an agent asked, and
/// `fletch-host approvals list` is how the answer gets in from a terminal. Same
/// registry `answer` consumes, so a request that resolves while this is being
/// rendered simply stops being listed.
pub fn pending() -> Vec<PendingApproval> {
    let guard = PENDING.lock();
    let Some(map) = guard.as_ref() else {
        return Vec::new();
    };
    let mut out: Vec<PendingApproval> = map.values().map(|r| r.info.clone()).collect();
    out.sort_by(|a, b| a.requested_at.cmp(&b.requested_at));
    out
}

fn refusal(detail: &str, why: &str) -> String {
    format!("not publishing ({detail}): {why}")
}

fn register(
    agent_id: &str,
    op: &str,
    repo: Option<&str>,
    detail: &str,
) -> (String, oneshot::Receiver<bool>) {
    let id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel();
    let info = PendingApproval {
        id: id.clone(),
        agent_id: agent_id.to_string(),
        op: op.to_string(),
        repo: repo.map(str::to_string),
        detail: detail.to_string(),
        requested_at: chrono::Utc::now().to_rfc3339(),
    };
    PENDING
        .lock()
        .get_or_insert_with(HashMap::new)
        .insert(id.clone(), Request { answer: tx, info });
    (id, rx)
}

fn take(id: &str) -> Option<Request> {
    PENDING.lock().as_mut()?.remove(id)
}

fn forget(id: &str) {
    let _ = take(id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_is_opt_in() {
        assert!(parse_enabled(Some("true")));
        for off in [None, Some(""), Some("false"), Some("1"), Some("yes")] {
            assert!(!parse_enabled(off), "{off:?} must not enable the prompt");
        }
    }

    #[tokio::test]
    async fn an_answer_resolves_its_request_once() {
        let (id, rx) = register("fuji", "push", Some("repo"), "push 1 commit");
        answer(&id, true);
        assert_eq!(rx.await, Ok(true));
        answer(&id, true);
        assert!(take(&id).is_none());
    }

    #[tokio::test]
    async fn answers_do_not_cross_requests() {
        let (first, rx_first) = register("fuji", "push", None, "push 1 commit");
        let (second, rx_second) = register("etna", "push", None, "push 2 commits");
        answer(&second, true);
        answer(&first, false);
        assert_eq!(rx_first.await, Ok(false));
        assert_eq!(rx_second.await, Ok(true));
    }

    // No test toggles `set_enabled`: it is process-global, and a parallel
    // git-dispatcher test would see the fail-closed refusal.

    #[test]
    fn answering_an_unknown_request_is_a_no_op() {
        answer("never-registered", true);
    }
}
