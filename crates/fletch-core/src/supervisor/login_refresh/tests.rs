use super::*;
use crate::pty_session::{PtySession, PtySpawn};
use crate::sandbox::KillHandle;
use crate::supervisor::tests::test_supervisor;
use serde_json::json;

const HOUR_MS: i64 = 3_600_000;

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
fn a_401_result_is_an_auth_rejection() {
    assert!(is_auth_rejection(&rejected_result()));
}

#[test]
fn a_successful_turn_quoting_a_401_is_not_a_rejection() {
    assert!(!is_auth_rejection(&json!({
        "type": "result",
        "is_error": false,
        "result": "The log said: API Error: 401 OAuth access token is invalid.",
    })));
}

#[test]
fn the_text_counts_only_on_an_error_result_without_a_status() {
    let text = "Failed to authenticate. API Error: 401 OAuth access token is invalid.";
    assert!(is_auth_rejection(
        &json!({"type": "result", "is_error": true, "result": text})
    ));
    assert!(!is_auth_rejection(&json!({
        "type": "result", "is_error": true, "api_error_status": 529, "result": text,
    })));
}

#[test]
fn only_a_result_event_can_be_a_rejection() {
    assert!(!is_auth_rejection(&ok_result()));
    assert!(!is_auth_rejection(
        &json!({"type": "assistant", "is_error": true, "api_error_status": 401})
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
async fn a_rejected_retry_gives_up() {
    let td = tempfile::tempdir().unwrap();
    let sup = test_supervisor();
    live_with_login(&sup, td.path(), 42);
    sup.observe_login("a1", &rejected_result());
    assert_eq!(
        sup.observe_login("a1", &rejected_result()),
        LoginVerdict::GaveUp
    );
}

#[tokio::test]
async fn a_good_turn_resets_the_retry_budget() {
    let td = tempfile::tempdir().unwrap();
    let sup = test_supervisor();
    live_with_login(&sup, td.path(), 42);
    sup.observe_login("a1", &rejected_result());
    assert_eq!(sup.observe_login("a1", &ok_result()), LoginVerdict::Fine);
    assert_eq!(
        sup.observe_login("a1", &rejected_result()),
        LoginVerdict::Retrying
    );
}

/// The next attempt after giving up — the user's resend once they have signed
/// in again — gets its own refresh and relaunch, or the new sign-in would
/// never reach a process still running on the dead token.
#[tokio::test]
async fn the_attempt_after_giving_up_gets_its_own_retry() {
    let td = tempfile::tempdir().unwrap();
    let sup = test_supervisor();
    live_with_login(&sup, td.path(), 42);
    sup.observe_login("a1", &rejected_result());
    sup.observe_login("a1", &rejected_result());
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

#[tokio::test]
async fn an_idle_process_inside_the_margin_is_due() {
    let td = tempfile::tempdir().unwrap();
    let sup = test_supervisor();
    let expiry = claude_oauth::now_ms() + 10 * 60 * 1000;
    live_with_login(&sup, td.path(), expiry);
    assert_eq!(sup.login_due("a1"), Some(expiry));
}

#[tokio::test]
async fn a_busy_process_is_never_due() {
    let td = tempfile::tempdir().unwrap();
    let sup = test_supervisor();
    live_with_login(&sup, td.path(), claude_oauth::now_ms() + 60_000);
    sup.statuses
        .lock()
        .insert("a1".into(), AgentStatus::Running);
    assert_eq!(sup.login_due("a1"), None);
}

#[tokio::test]
async fn a_process_with_runway_left_is_not_due() {
    let td = tempfile::tempdir().unwrap();
    let sup = test_supervisor();
    live_with_login(&sup, td.path(), claude_oauth::now_ms() + 5 * HOUR_MS);
    assert_eq!(sup.login_due("a1"), None);
}

#[test]
fn a_token_no_later_than_the_live_one_does_not_improve_it() {
    let live = 1_000;
    assert!(!improves(&Ok(Some(AccessToken::for_test("t", live))), live));
    assert!(!improves(&Ok(None), live));
    assert!(!improves(&Err(Error::Other("offline".into())), live));
}

#[test]
fn a_later_lapsing_token_improves_the_live_one() {
    assert!(improves(
        &Ok(Some(AccessToken::for_test("t", 2_000))),
        1_000
    ));
}

#[tokio::test]
async fn an_idle_process_is_taken_and_a_busy_one_left() {
    let td = tempfile::tempdir().unwrap();
    let sup = test_supervisor();
    let record = crate::supervisor::tests::record_with_status("a1", AgentStatus::Idle);
    assert!(matches!(sup.take_idle("a1", &record), Taken::Gone));
    live_with_login(&sup, td.path(), 42);
    sup.statuses
        .lock()
        .insert("a1".into(), AgentStatus::Running);
    assert!(matches!(sup.take_idle("a1", &record), Taken::Busy));
    assert!(sup.agents.lock().contains_key("a1"));
    sup.statuses.lock().insert("a1".into(), AgentStatus::Idle);
    let Taken::Agent(agent) = sup.take_idle("a1", &record) else {
        panic!("an idle process is taken");
    };
    assert!(!sup.agents.lock().contains_key("a1"));
    let _ = agent.shutdown();
}

#[tokio::test]
async fn a_launch_takes_the_token_resolved_ahead_of_it() {
    let sup = test_supervisor();
    let record = crate::supervisor::tests::record_with_status("a1", AgentStatus::Idle);
    sup.logins.lock().prefetched.insert(
        "a1".into(),
        Prefetched {
            at_ms: claude_oauth::now_ms(),
            token: Ok(Some(AccessToken::for_test("kept", 7))),
        },
    );
    let token = sup.launch_login("a1", &record).await.unwrap().unwrap();
    assert_eq!(token.secret(), "kept");
    assert!(!sup.logins.lock().prefetched.contains_key("a1"));
}

/// A launch that fails keeps the rejected-token mark, so the next launch
/// still replaces the token and the next 401 still gets its retry.
#[tokio::test]
async fn a_failed_launch_keeps_the_rejected_mark() {
    let sup = test_supervisor();
    let record = crate::supervisor::tests::record_with_status("a1", AgentStatus::Idle);
    sup.logins.lock().rejected.insert("a1".into(), 42);
    sup.logins.lock().prefetched.insert(
        "a1".into(),
        Prefetched {
            at_ms: claude_oauth::now_ms(),
            token: Err("offline".into()),
        },
    );
    assert!(sup.launch_login("a1", &record).await.is_err());
    assert_eq!(sup.logins.lock().rejected.get("a1"), Some(&42));
    sup.launch_succeeded("a1");
    assert!(!sup.logins.lock().rejected.contains_key("a1"));
}

#[test]
fn teardown_forgets_everything_kept_about_the_login() {
    let sup = test_supervisor();
    sup.logins.lock().rejected.insert("a1".into(), 42);
    sup.logins.lock().retrying.insert("a1".into());
    sup.remember_turn("a1", &msg());
    sup.forget_logins("a1");
    let logins = sup.logins.lock();
    assert!(logins.rejected.is_empty() && logins.retrying.is_empty());
    assert!(logins.last_turn.is_empty() && logins.prefetched.is_empty());
}

#[cfg(target_os = "macos")]
mod launched;
