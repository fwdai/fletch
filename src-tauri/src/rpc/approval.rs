//! Human approval for an agent's credentialed publish.
//!
//! [`crate::rpc::caps`] constrains *where* an agent may publish. This is the
//! other half: whether the act of publishing is something the user approved. It
//! is the last thing keeping the `HostHeldCredentials` guarantee at `Partial` —
//! credentials never enter the sandbox and the destination is constrained, but
//! until now an agent pushed under the user's identity without asking.
//!
//! **Off by default**, because prompting conflicts with unattended operation:
//! autopilot exists to work while nobody is watching, and a prompt would hang it
//! until the timeout and then deny. Turning it on is a deliberate trade of
//! unattended publishing for a gate — see [`SETTING`].
//!
//! Shape: the dispatcher [`request`]s approval and awaits a one-shot; the UI
//! answers through the `answer_publish_approval` command. Every path that does
//! not produce an explicit approval — no window listening, a timeout, a dropped
//! channel — resolves to **denied**, so the gate cannot fail open.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::json;
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

/// Settings key enabling the prompt. Absent or anything but `"true"` means off,
/// so an install that has never seen this setting keeps today's behaviour.
pub const SETTING: &str = "publish_confirmation";

/// The event the UI listens for. Payload: `{ id, agent_id, op, detail }`.
pub const EVENT_REQUESTED: &str = "publish:approval-requested";

/// How long an unanswered request waits before being denied. Generous enough for
/// a user to read the prompt and decide, short enough that an agent whose user
/// has walked away fails with a clear refusal instead of hanging its turn.
const DECISION_TIMEOUT: Duration = Duration::from_secs(120);

/// In-memory mirror of [`SETTING`], so the dispatcher (no DB handle) can read it.
/// Seeded at startup and updated by the set-command — the same idiom as
/// `sandbox::set_selected_engine_kind` and `codegraph::set_enabled`.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Requests awaiting a human answer, by request id.
static PENDING: Mutex<Option<HashMap<String, oneshot::Sender<bool>>>> = Mutex::new(None);

/// Interpret a raw [`SETTING`] value as opt-*in*: only an explicit `"true"` enables.
pub fn parse_enabled(raw: Option<&str>) -> bool {
    raw == Some("true")
}

/// Update the in-memory mirror (startup seed + the toggle command).
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// Whether publishing currently requires the user's approval.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Ask the user to approve one publish, and wait for the answer.
///
/// `detail` is the human-readable thing being approved (`"push fix/login"`,
/// `"open a pull request from fix/login"`), shown verbatim in the prompt so the
/// user approves a specific act rather than a category.
///
/// Returns the refusal reason when not approved, mirroring
/// [`crate::rpc::caps::AgentCaps::refuses_branch`] so both gates read the same
/// way at the call site.
pub async fn refuse_unless_approved(
    app: &AppHandle,
    agent_id: &str,
    detail: &str,
) -> Option<String> {
    if !enabled() {
        return None;
    }
    let (id, answer) = register();
    if app
        .emit(
            EVENT_REQUESTED,
            json!({ "id": id, "agent_id": agent_id, "detail": detail }),
        )
        .is_err()
    {
        // No window to ask: deny rather than publish unasked.
        forget(&id);
        return Some(refusal(detail, "no window is available to approve it"));
    }
    match tokio::time::timeout(DECISION_TIMEOUT, answer).await {
        Ok(Ok(true)) => None,
        Ok(Ok(false)) => Some(refusal(detail, "you declined it")),
        // Sender dropped (window closed mid-prompt) or the wait expired. Both are
        // "nobody approved", which is a refusal.
        Ok(Err(_)) => {
            forget(&id);
            Some(refusal(detail, "the approval prompt was dismissed"))
        }
        Err(_) => {
            forget(&id);
            Some(refusal(
                detail,
                &format!("nobody answered within {}s", DECISION_TIMEOUT.as_secs()),
            ))
        }
    }
}

/// Record the user's decision for `id`. Unknown ids are ignored — the request
/// may already have timed out, and a late answer must not publish anything.
pub fn answer(id: &str, approved: bool) {
    if let Some(tx) = take(id) {
        let _ = tx.send(approved);
    }
}

fn refusal(detail: &str, why: &str) -> String {
    format!("not publishing ({detail}): {why}")
}

/// A fresh request id paired with the receiver its answer will arrive on.
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

/// Drop a request nobody will answer, so a dismissed or expired prompt doesn't
/// leak its slot for the life of the process.
fn forget(id: &str) {
    let _ = take(id);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opt-in, so an install that has never seen the setting keeps publishing
    /// unattended — and a blank or malformed value can't silently enable a gate
    /// that would hang autopilot.
    #[test]
    fn the_setting_is_opt_in() {
        assert!(parse_enabled(Some("true")));
        for off in [None, Some(""), Some("false"), Some("1"), Some("yes")] {
            assert!(!parse_enabled(off), "{off:?} must not enable the prompt");
        }
    }

    /// An answer resolves exactly one request, and only once: a replayed or
    /// duplicated answer must not approve a later publish.
    #[tokio::test]
    async fn an_answer_resolves_its_request_once() {
        let (id, rx) = register();
        answer(&id, true);
        assert_eq!(rx.await, Ok(true));
        // The id is spent — answering again finds nothing to resolve.
        answer(&id, true);
        assert!(take(&id).is_none());
    }

    /// Answers are routed per request, so two agents awaiting approval can't
    /// receive each other's decision.
    #[tokio::test]
    async fn answers_do_not_cross_requests() {
        let (first, rx_first) = register();
        let (second, rx_second) = register();
        answer(&second, true);
        answer(&first, false);
        assert_eq!(rx_first.await, Ok(false));
        assert_eq!(rx_second.await, Ok(true));
    }

    /// An unknown id is ignored rather than panicking — a late answer for an
    /// already-timed-out request is expected, not exceptional.
    #[test]
    fn answering_an_unknown_request_is_a_no_op() {
        answer("never-registered", true);
    }

    /// The in-memory mirror is what the dispatcher reads (it has no DB handle),
    /// so a write that doesn't land there would leave the gate off however the
    /// setting is stored. Restores the default, since the mirror is process-wide.
    #[test]
    fn the_mirror_round_trips() {
        set_enabled(true);
        assert!(enabled());
        set_enabled(false);
        assert!(!enabled());
    }
}
