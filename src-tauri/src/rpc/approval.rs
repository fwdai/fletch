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
use serde_json::json;
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

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

static PENDING: Mutex<Option<HashMap<String, oneshot::Sender<bool>>>> = Mutex::new(None);

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
    app: &AppHandle,
    agent_id: &str,
    op: &str,
    repo: Option<&str>,
    detail: &str,
) -> Option<String> {
    if !enabled() {
        return None;
    }
    let (id, answer) = register();
    if app
        .emit(
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
    if let Some(tx) = take(id) {
        let _ = tx.send(approved);
    }
}

fn refusal(detail: &str, why: &str) -> String {
    format!("not publishing ({detail}): {why}")
}

fn register() -> (String, oneshot::Receiver<bool>) {
    let id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel();
    PENDING
        .lock()
        .get_or_insert_with(HashMap::new)
        .insert(id.clone(), tx);
    (id, rx)
}

fn take(id: &str) -> Option<oneshot::Sender<bool>> {
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
        let (id, rx) = register();
        answer(&id, true);
        assert_eq!(rx.await, Ok(true));
        answer(&id, true);
        assert!(take(&id).is_none());
    }

    #[tokio::test]
    async fn answers_do_not_cross_requests() {
        let (first, rx_first) = register();
        let (second, rx_second) = register();
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
