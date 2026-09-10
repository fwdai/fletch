//! Human approval for an agent's credentialed publish.
//!
//! [`crate::rpc::caps`] constrains *where* an agent may publish. This is the
//! other half: whether the act of publishing is something the user approved. It
//! is the last thing keeping the `HostHeldCredentials` guarantee at `Partial` —
//! credentials never enter the sandbox and the destination is constrained, but
//! until now an agent pushed under the user's identity without asking.
//!
//! **Off by default** — a product choice, not a technical limit. When it is on,
//! publishing the user already authorized is answered without a prompt: the UI
//! recognises an autopilot-driven push (enrollment is standing consent) and a live
//! Git-panel delegation (the click is the consent), so neither an unattended run
//! nor a button press waits on a dialog. See `store/publishApproval.ts`; that
//! policy lives in the UI because the state it reads — autopilot enrollment,
//! delegation liveness — is frontend-owned.
//!
//! The gate is nonetheless a secondary control: an agent has unrestricted network
//! access, so it does not need `git_push` to exfiltrate. What this protects is
//! *attribution* — a branch or pull request appearing to come from the user.
//!
//! Shape: the dispatcher [`request`]s approval and awaits a one-shot; the UI
//! answers through the `answer_publish_approval` command. Every path that does
//! not produce an explicit approval — no window listening, a timeout, a dropped
//! channel — resolves to **denied**, so the gate cannot fail open.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::json;
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

/// Settings key enabling the prompt. Absent or anything but `"true"` means off,
/// so an install that has never seen this setting keeps today's behaviour.
pub const SETTING: &str = "publish_confirmation";

/// The event the UI listens for. Payload:
/// `{ id, agent_id, op, repo, detail }`.
///
/// `op` and `repo` are what let the UI decide whether the user already
/// authorized this publish: authorization is per-checkout (autopilot enrollment,
/// a Git-panel delegation), and autopilot only ever needs `git_push`. Without
/// both, the UI would have to infer them from `detail`'s prose.
pub const EVENT_REQUESTED: &str = "publish:approval-requested";

/// Settings key: how many seconds an unanswered request waits before being
/// denied. `0` waits until answered (a closed window or dismissed prompt still
/// denies). Absent or unparsable falls back to [`DEFAULT_WAIT_SECS`].
pub const WAIT_SETTING: &str = "publish_approval_wait";

/// Default wait: generous enough for a user to read the prompt and decide,
/// short enough that an agent whose user has walked away fails with a clear
/// refusal instead of hanging its turn.
pub const DEFAULT_WAIT_SECS: u64 = 120;

/// In-memory mirror of [`SETTING`], so the dispatcher (no DB handle) can read it.
/// Seeded at startup and updated by the set-command — the same idiom as
/// `sandbox::set_selected_engine_kind` and `codegraph::set_enabled`.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// In-memory mirror of [`WAIT_SETTING`], same idiom.
static WAIT_SECS: AtomicU64 = AtomicU64::new(DEFAULT_WAIT_SECS);

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

/// Interpret a raw [`WAIT_SETTING`] value; anything unparsable is the default.
pub fn parse_wait_secs(raw: Option<&str>) -> u64 {
    raw.and_then(|s| s.trim().parse().ok())
        .unwrap_or(DEFAULT_WAIT_SECS)
}

/// Update the in-memory wait mirror (startup seed + the set-command).
pub fn set_wait_secs(secs: u64) {
    WAIT_SECS.store(secs, Ordering::Relaxed);
}

/// How long a prompt waits for an answer; `0` means until answered.
pub fn wait_secs() -> u64 {
    WAIT_SECS.load(Ordering::Relaxed)
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
        // No window to ask: deny rather than publish unasked.
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
        // Sender dropped (window closed mid-prompt) or the wait expired. Both are
        // "nobody approved", which is a refusal.
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

    // No test toggles `set_enabled` here on purpose: it is process-global, and a
    // parallel git-dispatcher test observing it as ON would see the fail-closed
    // refusal (no approval channel in tests) and fail. The mirror is a plain
    // AtomicBool; asserting its round-trip is not worth that hazard.

    /// An unknown id is ignored rather than panicking — a late answer for an
    /// already-timed-out request is expected, not exceptional.
    #[test]
    fn answering_an_unknown_request_is_a_no_op() {
        answer("never-registered", true);
    }
}
