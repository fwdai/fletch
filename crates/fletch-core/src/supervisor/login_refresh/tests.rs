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
    let expiry = crate::agent::host_login::now_ms() + 10 * 60 * 1000;
    live_with_login(&sup, td.path(), expiry);
    assert_eq!(sup.login_due("a1"), Some(expiry));
}

#[tokio::test]
async fn a_busy_process_is_never_due() {
    let td = tempfile::tempdir().unwrap();
    let sup = test_supervisor();
    live_with_login(&sup, td.path(), crate::agent::host_login::now_ms() + 60_000);
    sup.statuses
        .lock()
        .insert("a1".into(), AgentStatus::Running);
    assert_eq!(sup.login_due("a1"), None);
}

#[tokio::test]
async fn a_process_with_runway_left_is_not_due() {
    let td = tempfile::tempdir().unwrap();
    let sup = test_supervisor();
    live_with_login(
        &sup,
        td.path(),
        crate::agent::host_login::now_ms() + 5 * HOUR_MS,
    );
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
            at_ms: crate::agent::host_login::now_ms(),
            account: None,
            token: Ok(Some(AccessToken::for_test("kept", 7))),
        },
    );
    let token = sup.launch_login("a1", &record).await.unwrap().unwrap();
    assert_eq!(token.secret(), "kept");
    assert!(!sup.logins.lock().prefetched.contains_key("a1"));
}

/// A token kept for one account never signs in a launch stamped with
/// another: the switch resolves its target's token before it restamps, and a
/// revive racing it must resolve its own.
#[test]
fn a_launch_under_another_stamp_ignores_the_kept_token() {
    crate::agent::accounts::with_test_root(|_| {
        let sup = test_supervisor();
        let mut record = crate::supervisor::tests::record_with_status("a1", AgentStatus::Idle);
        record.account = Some("gone".into());
        sup.logins.lock().prefetched.insert(
            "a1".into(),
            Prefetched {
                at_ms: crate::agent::host_login::now_ms(),
                account: None,
                token: Ok(Some(AccessToken::for_test("kept", 7))),
            },
        );
        let launch = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(sup.launch_login("a1", &record));
        assert!(launch.is_err(), "the `gone` stamp resolves its own login");
    });
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
            at_ms: crate::agent::host_login::now_ms(),
            account: None,
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
    sup.logins.lock().limited.insert("a1".into());
    sup.mark_moved("a1");
    sup.forget_logins("a1");
    let logins = sup.logins.lock();
    assert!(logins.rejected.is_empty() && logins.retrying.is_empty());
    assert!(logins.last_turn.is_empty() && logins.prefetched.is_empty());
    assert!(logins.limited.is_empty() && logins.moved.is_empty());
}

// --- a spent quota: the turn is resent once under the active account ---

/// Claude's result once the five-hour window is spent, as seen in a real
/// transcript; no `api_error_status` comes with it.
fn session_limit_result() -> Value {
    json!({
        "type": "result",
        "is_error": true,
        "result": "You've hit your session limit · resets 1pm (Asia/Bangkok)",
    })
}

fn codex_limit_failure() -> Value {
    json!({
        "type": "turn.failed",
        "error": {"message": "You've hit your usage limit. Try again at 3pm."},
    })
}

#[test]
fn a_429_result_is_a_limit_rejection() {
    assert!(is_limit_rejection(
        &json!({"type": "result", "is_error": true, "api_error_status": 429, "result": "x"}),
        false
    ));
}

#[test]
fn a_result_naming_a_spent_limit_is_a_limit_rejection() {
    assert!(is_limit_rejection(&session_limit_result(), false));
    for text in [
        "Claude AI usage limit reached|1760000000",
        "5-hour limit reached ∙ resets 3pm",
        "You've hit your usage limit. Try again at 5pm.",
        "API Error: 429 rate limit exceeded",
    ] {
        assert!(
            is_limit_rejection(
                &json!({"type": "result", "is_error": true, "result": text}),
                false
            ),
            "{text}"
        );
    }
}

#[test]
fn a_codex_turn_that_failed_on_a_usage_limit_is_a_limit_rejection() {
    assert!(is_limit_rejection(&codex_limit_failure(), false));
    assert!(!is_limit_rejection(
        &json!({"type": "turn.failed", "error": {"message": "stream disconnected"}}),
        false
    ));
}

/// Claude's own message once the window is spent, as it reaches the stream:
/// the CLI talking to itself under the `<synthetic>` model.
fn synthetic_limit_message() -> Value {
    json!({
        "type": "assistant",
        "message": {
            "model": "<synthetic>",
            "role": "assistant",
            "content": [{"type": "text",
                         "text": "You've hit your session limit · resets 1pm (Asia/Bangkok)"}],
        },
    })
}

/// Claude shrinks the result to "Turn failed" once the agent has spoken, and
/// need not flag it as an error: what the stream said earlier — a rejected
/// `rate_limit_event`, or its own limit message — settles the result.
#[test]
fn a_turn_the_stream_already_called_a_limit_ends_as_one_whatever_the_result_says() {
    let bare = json!({"type": "result", "is_error": true, "result": "Turn failed"});
    assert!(!is_limit_rejection(&bare, false));
    assert!(is_limit_rejection(&bare, true));
    assert!(is_limit_rejection(&ok_result(), true));
    assert!(is_limit_rejected_event(&json!({
        "type": "rate_limit_event",
        "rate_limit_info": {"status": "rejected", "rateLimitType": "five_hour"},
    })));
    assert!(!is_limit_rejected_event(&json!({
        "type": "rate_limit_event",
        "rate_limit_info": {"status": "allowed"},
    })));
    assert!(is_limit_rejected_event(&synthetic_limit_message()));
    assert!(is_limit_rejected_event(&json!({
        "type": "assistant",
        "message": {"model": "<synthetic>", "content": "Claude AI usage limit reached|176"},
    })));
}

#[test]
fn an_agents_own_words_about_limits_are_not_a_limit() {
    assert!(!is_limit_rejected_event(&json!({
        "type": "assistant",
        "message": {"model": "claude-opus-5-5",
                    "content": [{"type": "text", "text": "You've hit your usage limit, it says."}]},
    })));
    assert!(!is_limit_rejected_event(&json!({
        "type": "assistant",
        "message": {"model": "<synthetic>",
                    "content": [{"type": "text", "text": "Request interrupted by user"}]},
    })));
}

#[test]
fn a_successful_turn_or_another_limit_is_not_a_limit_rejection() {
    assert!(!is_limit_rejection(
        &json!({"type": "result", "is_error": false, "result": "usage limit reached"}),
        false
    ));
    for text in [
        "account device limit reached",
        "Budget limit reached ($5)",
        "You've hit your device limit",
        "You've hit your budget limit for this session",
    ] {
        assert!(
            !is_limit_rejection(
                &json!({"type": "result", "is_error": true, "result": text}),
                false
            ),
            "{text}"
        );
    }
    assert!(!is_limit_rejection(&rejected_result(), false));
}

/// Run `test` with the accounts root at a fresh tempdir, on its own runtime:
/// the root is process-wide, so its lock has to be held across the awaits.
fn in_root<F, Fut>(test: F)
where
    F: FnOnce(std::path::PathBuf) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    crate::agent::accounts::with_test_root(|root| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(test(root.to_path_buf()));
    });
}

/// A managed claude account holding a file login that needs no refresh.
fn signed_in(root: &std::path::Path, id: &str) {
    let dir = root.join("claude").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let blob = json!({
        "claudeAiOauth": {
            "accessToken": format!("sk-ant-oat01-test-{id}"),
            "refreshToken": format!("sk-ant-ort01-test-{id}"),
            "expiresAt": crate::agent::host_login::now_ms() + 7 * 24 * HOUR_MS,
        }
    });
    std::fs::write(dir.join(".credentials.json"), blob.to_string()).unwrap();
}

/// A claude agent `a1` stamped `work`, with the active account set to
/// `active`, its last turn remembered, in a supervisor over the ctx's db.
async fn agent_on_work(
    dir: &std::path::Path,
    active: &str,
) -> (Arc<Supervisor>, Arc<EngineCtx>, tempfile::TempDir) {
    agent_on_work_for(dir, "claude", active).await
}

/// [`agent_on_work`] for an agent of `provider`.
async fn agent_on_work_for(
    dir: &std::path::Path,
    provider: &str,
    active: &str,
) -> (Arc<Supervisor>, Arc<EngineCtx>, tempfile::TempDir) {
    let (ctx, _sink, db_dir) = crate::host::ctx::test_ctx();
    let sup = Arc::new(Supervisor::new(Arc::new(
        crate::workspace::WorkspaceManager::new(ctx.db.clone()),
    )));
    let checkout = crate::supervisor::tests::committed_repo(dir, "repo").await;
    let mut record =
        crate::supervisor::tests::record_in_checkouts(&sup, "a1", std::slice::from_ref(&checkout));
    record.provider = provider.into();
    record.account = Some("work".into());
    sup.workspace.add_agent(&mut record).unwrap();
    set_active(&ctx, provider, active);
    sup.remember_turn("a1", &msg());
    (sup, ctx, db_dir)
}

fn stamp(sup: &Supervisor) -> Option<String> {
    sup.workspace.agent("a1").unwrap().account
}

/// `observe_limit` for `a1`, off the runtime's workers: in the app the
/// stream is read on a plain thread, and the limit path blocks on the
/// provider's account lock there.
fn observe(sup: &Arc<Supervisor>, ctx: &Arc<EngineCtx>, event: &Value) -> LoginVerdict {
    tokio::task::block_in_place(|| sup.observe_limit(ctx, "a1", event))
}

#[test]
fn a_limit_under_another_account_resends_the_turn_under_the_active_one() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let (sup, ctx, _db) = agent_on_work(td.path(), "home").await;
        sup.logins.lock().rejected.insert("a1".into(), 42);

        assert_eq!(
            observe(&sup, &ctx, &session_limit_result()),
            LoginVerdict::Retrying
        );

        assert_eq!(stamp(&sup).as_deref(), Some("home"));
        assert!(sup.respawn_pending.lock().contains("a1"));
        assert_eq!(
            sup.message_queue
                .lock()
                .drain_coalesced("a1")
                .map(|m| m.text),
            Some("fix the bug".to_string())
        );
        // The old account's rejected token is its own.
        assert!(!sup.logins.lock().rejected.contains_key("a1"));
    });
}

#[test]
fn a_codex_turn_failed_on_a_limit_takes_the_same_path() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let (sup, ctx, _db) = agent_on_work(td.path(), "home").await;

        assert_eq!(
            observe(&sup, &ctx, &codex_limit_failure()),
            LoginVerdict::Retrying
        );
        assert_eq!(stamp(&sup).as_deref(), Some("home"));
    });
}

/// A codex login that needs no refresh: an access token whose `exp` is a week
/// out, distinct per account.
fn codex_signed_in(root: &std::path::Path, id: &str) {
    use base64::Engine as _;
    let dir = root.join("codex").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let enc = |v: Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
    let exp = chrono::Utc::now().timestamp() + 7 * 24 * 3600;
    let access = format!(
        "{}.{}.sig-{id}",
        enc(json!({"alg": "RS256"})),
        enc(json!({"iat": exp - 10 * 24 * 3600, "exp": exp, "acct": id}))
    );
    let login = json!({
        "auth_mode": "chatgpt",
        "OPENAI_API_KEY": null,
        "tokens": {"id_token": "id", "access_token": access,
                   "refresh_token": format!("rt.{id}"), "account_id": id},
        "last_refresh": "2026-10-01T00:00:00Z"
    });
    std::fs::write(dir.join("auth.json"), login.to_string()).unwrap();
}

fn set_active(ctx: &EngineCtx, provider: &str, id: &str) {
    crate::database::set_setting(
        &ctx.db.lock(),
        &crate::agent::accounts::active_setting_key(provider),
        id,
    )
    .unwrap();
}

/// Codex has no login reader to reset the budget on a clean turn, and
/// reports one as `turn.completed`: a successful retry, then a later limit
/// under another selection, is a fresh attempt and gets its own retry.
#[test]
fn a_codex_turn_completed_after_the_retry_resets_the_budget_for_a_later_limit() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        codex_signed_in(&root, "home");
        codex_signed_in(&root, "work");
        let (sup, ctx, _db) = agent_on_work_for(td.path(), "codex", "home").await;

        assert_eq!(
            observe(&sup, &ctx, &codex_limit_failure()),
            LoginVerdict::Retrying
        );
        assert_eq!(stamp(&sup).as_deref(), Some("home"));
        // The relaunch resent the turn and it went through.
        sup.respawn_pending.lock().remove("a1");
        sup.message_queue.lock().drain_coalesced("a1");
        assert_eq!(
            observe(&sup, &ctx, &json!({"type": "turn.completed", "usage": {}})),
            LoginVerdict::Fine
        );
        assert!(!sup.logins.lock().retrying.contains("a1"));

        set_active(&ctx, "codex", "work");
        assert_eq!(
            observe(&sup, &ctx, &codex_limit_failure()),
            LoginVerdict::Retrying
        );
        assert_eq!(stamp(&sup).as_deref(), Some("work"));
    });
}

/// The retry's read, probe and restamp run under the provider's account
/// lock, after whoever holds it: a removal or a Settings change in flight
/// can't slip between the check and the restamp.
#[test]
fn a_limit_retry_waits_for_the_provider_account_lock() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let (sup, ctx, _db) = agent_on_work(td.path(), "home").await;

        let held = ctx.account_locks.lock("claude").await;
        let observing = {
            let (sup, ctx) = (sup.clone(), ctx.clone());
            tokio::task::spawn_blocking(move || {
                sup.observe_limit(&ctx, "a1", &session_limit_result())
            })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!observing.is_finished(), "the retry ran inside the lock");
        assert_eq!(stamp(&sup).as_deref(), Some("work"));
        drop(held);

        assert_eq!(observing.await.unwrap(), LoginVerdict::Retrying);
        assert_eq!(stamp(&sup).as_deref(), Some("home"));
    });
}

/// The selection is read under the lock, so one that changed while the
/// retry waited is the one it follows: back to the agent's own account
/// here, which leaves nothing to retry on.
#[test]
fn a_selection_changed_while_the_retry_waited_is_the_one_it_follows() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        signed_in(&root, "work");
        let (sup, ctx, _db) = agent_on_work(td.path(), "home").await;

        let held = ctx.account_locks.lock("claude").await;
        let observing = {
            let (sup, ctx) = (sup.clone(), ctx.clone());
            tokio::task::spawn_blocking(move || {
                sup.observe_limit(&ctx, "a1", &session_limit_result())
            })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        set_active(&ctx, "claude", "work");
        drop(held);

        assert_eq!(observing.await.unwrap(), LoginVerdict::Fine);
        assert_eq!(stamp(&sup).as_deref(), Some("work"));
        assert!(!sup.respawn_pending.lock().contains("a1"));
    });
}

/// The error stands in the chat: there is nothing else the user chose.
#[test]
fn a_limit_under_the_active_account_leaves_the_error_standing() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "work");
        let (sup, ctx, _db) = agent_on_work(td.path(), "work").await;

        assert_eq!(
            observe(&sup, &ctx, &session_limit_result()),
            LoginVerdict::Fine
        );

        assert_eq!(stamp(&sup).as_deref(), Some("work"));
        assert!(!sup.respawn_pending.lock().contains("a1"));
        assert!(sup.message_queue.lock().is_empty("a1"));
    });
}

/// The retry ran under the active account and hit a limit there too: both
/// accounts are spent, and the second error is the user's to see.
#[test]
fn a_second_limit_on_the_retry_leaves_the_error_standing_and_the_new_stamp() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let (sup, ctx, _db) = agent_on_work(td.path(), "home").await;
        observe(&sup, &ctx, &session_limit_result());
        sup.respawn_pending.lock().remove("a1");
        sup.message_queue.lock().drain_coalesced("a1");

        assert_eq!(
            observe(&sup, &ctx, &session_limit_result()),
            LoginVerdict::Fine
        );

        assert_eq!(stamp(&sup).as_deref(), Some("home"));
        assert!(!sup.respawn_pending.lock().contains("a1"));
        assert!(!sup.logins.lock().retrying.contains("a1"));
        // A later attempt gets its own retry, like the login one does.
        set_active(&ctx, "claude", "work");
        signed_in(&root, "work");
        assert_eq!(
            observe(&sup, &ctx, &session_limit_result()),
            LoginVerdict::Retrying
        );
    });
}

#[test]
fn a_limit_with_the_active_account_signed_out_leaves_the_error_standing() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.join("claude").join("home")).unwrap();
        let (sup, ctx, _db) = agent_on_work(td.path(), "home").await;

        assert_eq!(
            observe(&sup, &ctx, &session_limit_result()),
            LoginVerdict::Fine
        );

        assert_eq!(stamp(&sup).as_deref(), Some("work"));
        assert!(!sup.logins.lock().retrying.contains("a1"));
    });
}

#[test]
fn claudes_own_limit_message_settles_the_result_that_follows() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let (sup, ctx, _db) = agent_on_work(td.path(), "home").await;

        assert_eq!(
            observe(&sup, &ctx, &synthetic_limit_message()),
            LoginVerdict::Fine
        );
        assert_eq!(
            observe(
                &sup,
                &ctx,
                &json!({"type": "result", "is_error": false, "result": ""})
            ),
            LoginVerdict::Retrying
        );

        assert_eq!(stamp(&sup).as_deref(), Some("home"));
        assert!(sup.respawn_pending.lock().contains("a1"));
    });
}

/// The radio moved while a turn ran: the stamp already says the new account,
/// but the process is still on the old login, and the limit is the old
/// account's. The relaunch the fan-out flagged carries the resent turn.
#[test]
fn a_limit_on_the_turn_running_through_the_fan_out_is_resent_under_the_new_stamp() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let (sup, ctx, _db) = agent_on_work(td.path(), "home").await;
        live_with_login(&sup, td.path(), far_future());
        sup.statuses
            .lock()
            .insert("a1".into(), AgentStatus::Running);
        sup.follow_active_account(&ctx, "claude", Some("home"))
            .await
            .unwrap();
        // The deferred respawn re-flags itself once it finds the agent busy.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(stamp(&sup).as_deref(), Some("home"));

        assert_eq!(
            observe(&sup, &ctx, &session_limit_result()),
            LoginVerdict::Retrying
        );

        assert!(sup.respawn_pending.lock().contains("a1"));
        assert_eq!(
            sup.message_queue
                .lock()
                .drain_coalesced("a1")
                .map(|m| m.text),
            Some("fix the bug".to_string())
        );
        // One retry: a limit under the new login too is the user's to see.
        assert_eq!(
            observe(&sup, &ctx, &session_limit_result()),
            LoginVerdict::Fine
        );
    });
}

/// Once the relaunch has read the new stamp, a limit is the new account's.
#[tokio::test]
async fn a_relaunch_clears_the_moved_mark() {
    let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
    let sup = Arc::new(test_supervisor());
    sup.mark_moved("a1");
    // No session to resume, so the start fails; the mark is cleared first.
    let _ = sup.restart_taken(&ctx, "a1", None).await;
    assert!(!sup.logins.lock().moved.contains("a1"));
}

fn far_future() -> i64 {
    crate::agent::host_login::now_ms() + 7 * 24 * HOUR_MS
}

/// The rejected `rate_limit_event` settles the result that follows, whatever
/// it says, and is spent by it: a later turn's result stands on its own.
#[test]
fn a_rejected_rate_limit_event_is_spent_by_its_turns_result() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let (sup, ctx, _db) = agent_on_work(td.path(), "home").await;
        let bare = json!({"type": "result", "is_error": true, "result": "Turn failed"});
        assert_eq!(observe(&sup, &ctx, &bare), LoginVerdict::Fine);
        assert_eq!(stamp(&sup).as_deref(), Some("work"));

        observe(
            &sup,
            &ctx,
            &json!({"type": "rate_limit_event", "rate_limit_info": {"status": "rejected"}}),
        );
        assert_eq!(observe(&sup, &ctx, &ok_result()), LoginVerdict::Retrying);
        assert_eq!(stamp(&sup).as_deref(), Some("home"));
        assert!(!sup.logins.lock().limited.contains("a1"));

        // The retry's clean result is not a limit: the mark was spent, and
        // the budget with it.
        assert_eq!(observe(&sup, &ctx, &ok_result()), LoginVerdict::Fine);
        assert_eq!(stamp(&sup).as_deref(), Some("home"));
        assert!(!sup.logins.lock().retrying.contains("a1"));
    });
}

#[cfg(target_os = "macos")]
mod launched;
