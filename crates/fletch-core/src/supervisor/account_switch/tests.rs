use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::*;
use crate::agent::Agent;
use crate::host::sink::RecordingSink;
use crate::pty_session::{PtySession, PtySpawn};
use crate::sandbox::KillHandle;
use crate::supervisor::login_refresh::LoginVerdict;
use crate::supervisor::tests::{
    committed_repo, record_in_checkouts, record_with_status, test_supervisor,
};
use crate::workspace::AgentStatus;

const AGENT: &str = "yosemite";

/// Run `test` with the accounts root at a fresh tempdir, on its own runtime:
/// the root is process-wide, so its lock has to be held across the awaits.
fn in_root<F, Fut>(test: F)
where
    F: FnOnce(PathBuf) -> Fut,
    Fut: Future<Output = ()>,
{
    accounts::with_test_root(|root| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(test(root.to_path_buf()));
    });
}

fn far_future_ms() -> i64 {
    crate::agent::host_login::now_ms() + 7 * 24 * 3_600_000
}

/// A managed claude account holding a file login that needs no refresh.
fn signed_in(root: &Path, id: &str) {
    let dir = signed_out(root, id);
    let blob = serde_json::json!({
        "claudeAiOauth": {
            "accessToken": format!("sk-ant-oat01-test-{id}"),
            "refreshToken": format!("sk-ant-ort01-test-{id}"),
            "expiresAt": far_future_ms(),
        }
    });
    std::fs::write(dir.join(".credentials.json"), blob.to_string()).unwrap();
}

fn signed_out(root: &Path, id: &str) -> PathBuf {
    let dir = root.join("claude").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A resting claude agent (no process) stamped with `account`, in a fresh
/// checkout under `dir`. It has no session to resume, so a relaunch of it
/// fails at once rather than launching anything.
async fn agent_on(dir: &Path, account: Option<&str>) -> Arc<Supervisor> {
    let checkout = committed_repo(dir, "repo").await;
    let sup = Arc::new(test_supervisor());
    let mut record = record_in_checkouts(&sup, AGENT, std::slice::from_ref(&checkout));
    record.account = account.map(str::to_string);
    record.session_id = None;
    sup.workspace.add_agent(&mut record).unwrap();
    sup
}

fn live_process(sup: &Supervisor, dir: &Path) {
    let pty = cat_pty(dir);
    sup.agents
        .lock()
        .insert(AGENT.to_string(), Arc::new(Agent::over_pty(pty)));
}

/// A live process launched on a host token, so a 401 result reaches the
/// login retry (`observe_login`).
fn live_process_with_login(sup: &Supervisor, dir: &Path) {
    let pty = cat_pty(dir);
    sup.agents.lock().insert(
        AGENT.to_string(),
        Arc::new(Agent::over_pty_with_login(pty, far_future_ms())),
    );
}

fn rejected_result() -> serde_json::Value {
    serde_json::json!({
        "type": "result",
        "is_error": true,
        "api_error_status": 401,
        "result": "Failed to authenticate. API Error: 401",
    })
}

fn cat_pty(dir: &Path) -> PtySession {
    PtySession::spawn(
        PtySpawn {
            program: Path::new("/bin/cat"),
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
    .unwrap()
}

fn stamp(sup: &Supervisor) -> Option<String> {
    sup.workspace.agent(AGENT).unwrap().account
}

fn event_names(sink: &RecordingSink) -> Vec<String> {
    sink.events().into_iter().map(|(name, _)| name).collect()
}

fn refusal(record: &AgentRecord, requested: &str) -> String {
    target_stamp(record, requested).unwrap_err().to_string()
}

#[test]
fn a_provider_without_accounts_is_refused() {
    in_root(|_| async {
        let mut record = record_with_status(AGENT, AgentStatus::Idle);
        record.provider = "opencode".into();
        assert!(refusal(&record, "work").contains("no accounts"));
    });
}

/// A codex login that needs no refresh: an access token whose `exp` is a week
/// out, distinct per account.
fn codex_signed_in(root: &Path, id: &str) -> PathBuf {
    use base64::Engine as _;
    let dir = root.join("codex").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let enc = |v: serde_json::Value| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string().as_bytes())
    };
    let exp = chrono::Utc::now().timestamp() + 7 * 24 * 3600;
    let access = format!(
        "{}.{}.sig-{id}",
        enc(serde_json::json!({"alg": "RS256"})),
        enc(serde_json::json!({"iat": exp - 10 * 24 * 3600, "exp": exp, "acct": id}))
    );
    let login = serde_json::json!({
        "auth_mode": "chatgpt",
        "OPENAI_API_KEY": null,
        "tokens": {"id_token": "id", "access_token": access,
                   "refresh_token": format!("rt.{id}"), "account_id": id},
        "last_refresh": "2026-10-01T00:00:00Z"
    });
    std::fs::write(dir.join("auth.json"), login.to_string()).unwrap();
    dir
}

/// Codex no longer runs in its account's directory (its sessions live in the
/// agent's own overlay), so the switch takes it like claude.
#[test]
fn a_codex_workspace_is_no_longer_refused() {
    in_root(|root| async move {
        codex_signed_in(&root, "work");
        let mut record = record_with_status(AGENT, AgentStatus::Idle);
        record.provider = "codex".into();
        assert_eq!(
            target_stamp(&record, "work").unwrap().as_deref(),
            Some("work")
        );
    });
}

#[test]
fn a_resting_codex_session_is_restamped_onto_the_signed_in_account() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        codex_signed_in(&root, "work");
        codex_signed_in(&root, "home");
        let checkout = committed_repo(td.path(), "repo").await;
        let sup = Arc::new(test_supervisor());
        let mut record = record_in_checkouts(&sup, AGENT, std::slice::from_ref(&checkout));
        record.provider = "codex".into();
        record.account = Some("work".into());
        record.session_id = None;
        sup.workspace.add_agent(&mut record).unwrap();
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();

        let record = sup.switch_account(&ctx, AGENT, "home").await.unwrap();

        assert_eq!(record.account.as_deref(), Some("home"));
        assert!(sup.agents.lock().is_empty());
    });
}

#[test]
fn the_current_account_is_refused_whichever_way_the_default_is_spelled() {
    in_root(|root| async move {
        signed_in(&root, "work");
        let mut record = record_with_status(AGENT, AgentStatus::Idle);
        for stamp in [None, Some(""), Some("default")] {
            record.account = stamp.map(str::to_string);
            for requested in ["", "default", " default "] {
                assert!(refusal(&record, requested).contains("already runs under the `default`"));
            }
        }
        record.account = Some("work".into());
        assert!(refusal(&record, "work").contains("already runs under the `work`"));
    });
}

#[test]
fn a_managed_id_with_no_directory_is_refused() {
    in_root(|_| async {
        let record = record_with_status(AGENT, AgentStatus::Idle);
        assert!(refusal(&record, "gone").contains("No claude account named `gone`"));
    });
}

#[test]
fn an_id_that_is_not_an_account_name_is_refused() {
    in_root(|_| async {
        let record = record_with_status(AGENT, AgentStatus::Idle);
        assert!(refusal(&record, "../etc").contains("lowercase letters"));
    });
}

#[test]
fn a_signed_out_target_is_refused_and_the_stamp_kept() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_out(&root, "work");
        let sup = agent_on(td.path(), None).await;
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();

        let why = sup
            .switch_account(&ctx, AGENT, "work")
            .await
            .unwrap_err()
            .to_string();

        assert!(why.contains("`work` account isn't signed in"), "{why}");
        assert_eq!(stamp(&sup), None);
    });
}

#[test]
fn a_switch_mid_turn_is_refused_and_the_stamp_kept() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let sup = agent_on(td.path(), Some("work")).await;
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        for busy in [AgentStatus::Running, AgentStatus::Spawning] {
            sup.statuses.lock().insert(AGENT.into(), busy);

            let why = sup
                .switch_account(&ctx, AGENT, "home")
                .await
                .unwrap_err()
                .to_string();

            assert_eq!(why, BUSY_MSG);
            assert_eq!(stamp(&sup).as_deref(), Some("work"));
        }
    });
}

#[test]
fn an_archived_agent_is_refused() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let sup = agent_on(td.path(), None).await;
        sup.workspace.begin_archive(AGENT).unwrap();
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();

        let why = sup.switch_account(&ctx, AGENT, "home").await.unwrap_err();

        assert!(why.to_string().contains("archived"), "{why}");
    });
}

#[test]
fn a_resting_session_is_restamped_without_a_relaunch() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let sup = agent_on(td.path(), Some("work")).await;
        let (ctx, sink, _dir) = crate::host::ctx::test_ctx();

        let record = sup.switch_account(&ctx, AGENT, "home").await.unwrap();

        assert_eq!(record.account.as_deref(), Some("home"));
        assert_eq!(stamp(&sup).as_deref(), Some("home"));
        assert!(sup.agents.lock().is_empty());
        assert_eq!(event_names(&sink), vec!["workspace:changed".to_string()]);
    });
}

#[test]
fn a_switch_to_the_default_targets_no_stamp() {
    in_root(|_| async {
        let mut record = record_with_status(AGENT, AgentStatus::Idle);
        record.account = Some("work".into());
        for requested in ["default", ""] {
            assert_eq!(target_stamp(&record, requested).unwrap(), None);
        }
    });
}

#[test]
fn an_errored_session_is_switchable() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let sup = agent_on(td.path(), Some("work")).await;
        sup.statuses.lock().insert(AGENT.into(), AgentStatus::Error);
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();

        let record = sup.switch_account(&ctx, AGENT, "home").await.unwrap();

        assert_eq!(record.account.as_deref(), Some("home"));
    });
}

/// The error belonged to the old account; the switched session rests idle
/// until its next launch says otherwise.
#[test]
fn an_errored_session_switched_at_rest_drops_the_old_error() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let sup = agent_on(td.path(), Some("work")).await;
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        sup.set_status(
            &ctx,
            AGENT,
            AgentStatus::Error,
            Some("sign in again".into()),
        );

        let record = sup.switch_account(&ctx, AGENT, "home").await.unwrap();

        assert_eq!(record.status, AgentStatus::Idle);
        assert!(
            !sup.statuses.lock().contains_key(AGENT),
            "no live status for a resting agent"
        );
        assert_ne!(
            sup.workspace.agent(AGENT).unwrap().status,
            AgentStatus::Error
        );
    });
}

/// A `default` spelled out on the record is the default: a switch off it
/// and the way back both resolve like an absent stamp.
#[test]
fn a_spelled_out_default_stamp_switches_to_a_managed_account_and_back() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let sup = agent_on(td.path(), Some(accounts::DEFAULT_ACCOUNT)).await;
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();

        let record = sup.switch_account(&ctx, AGENT, "home").await.unwrap();

        assert_eq!(record.account.as_deref(), Some("home"));
        // The way back would probe this machine's own login, so only the
        // stamp it writes is checked.
        assert_eq!(
            target_stamp(&record, accounts::DEFAULT_ACCOUNT).unwrap(),
            None
        );
    });
}

#[test]
fn a_relaunch_that_fails_to_start_has_already_stopped_the_old_process() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let sup = agent_on(td.path(), Some("work")).await;
        live_process(&sup, td.path());
        let (ctx, sink, _dir) = crate::host::ctx::test_ctx();

        // The agent has no session to resume, so the start fails; the
        // teardown and `Spawning` before it are what is under test.
        let _ = sup.switch_account(&ctx, AGENT, "home").await;

        assert!(!sup.agents.lock().contains_key(AGENT));
        assert!(sink
            .events()
            .iter()
            .any(|(name, payload)| name == "agent:status" && payload["status"] == "spawning"));
    });
}

#[test]
fn a_failed_relaunch_puts_the_old_stamp_back_and_says_why() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let sup = agent_on(td.path(), Some("work")).await;
        live_process(&sup, td.path());
        let (ctx, sink, _dir) = crate::host::ctx::test_ctx();

        let why = sup
            .switch_account(&ctx, AGENT, "home")
            .await
            .unwrap_err()
            .to_string();

        assert!(
            why.contains("under the `home` account, so it stays on `work`"),
            "{why}"
        );
        assert_eq!(stamp(&sup).as_deref(), Some("work"));
        assert_eq!(sup.status_of(AGENT), Some(AgentStatus::Error));
        assert!(!sup.agents.lock().contains_key(AGENT));
        assert!(event_names(&sink).contains(&"workspace:changed".to_string()));
    });
}

#[test]
fn a_failed_relaunch_restores_the_old_accounts_rejected_token() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let sup = agent_on(td.path(), Some("work")).await;
        live_process(&sup, td.path());
        sup.logins.lock().rejected.insert(AGENT.into(), 42);
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();

        assert!(sup.switch_account(&ctx, AGENT, "home").await.is_err());

        assert_eq!(sup.logins.lock().rejected.get(AGENT), Some(&42));
    });
}

/// The old account had spent its one retry; the agent stays on it after a
/// failed switch, so the next 401 still gives up.
#[test]
fn a_failed_relaunch_keeps_the_old_accounts_spent_retry() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let sup = agent_on(td.path(), Some("work")).await;
        live_process_with_login(&sup, td.path());
        assert_eq!(
            sup.observe_login(AGENT, &rejected_result()),
            LoginVerdict::Retrying
        );
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();

        assert!(sup.switch_account(&ctx, AGENT, "home").await.is_err());

        live_process_with_login(&sup, td.path());
        assert_eq!(
            sup.observe_login(AGENT, &rejected_result()),
            LoginVerdict::GaveUp
        );
    });
}

/// A 401 spent the old account's one retry; the new account's first 401
/// gets a retry of its own.
#[test]
fn a_switch_gives_the_new_account_its_own_login_retry() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let sup = agent_on(td.path(), Some("work")).await;
        live_process_with_login(&sup, td.path());
        assert_eq!(
            sup.observe_login(AGENT, &rejected_result()),
            LoginVerdict::Retrying
        );
        // The process went away after the turn, so the switch is at rest.
        sup.agents.lock().remove(AGENT);
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();

        sup.switch_account(&ctx, AGENT, "home").await.unwrap();

        live_process_with_login(&sup, td.path());
        assert_eq!(
            sup.observe_login(AGENT, &rejected_result()),
            LoginVerdict::Retrying
        );
    });
}

/// The target's token is resolved before the lifecycle lock; a refusal under
/// it must not leave that token for the old account's next launch.
#[test]
fn a_switch_refused_under_the_lock_drops_the_target_token() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let sup = agent_on(td.path(), Some("work")).await;
        live_process(&sup, td.path());
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();

        let held = sup.agent_lifecycle.lock().await;
        let switching = {
            let (sup, ctx) = (sup.clone(), ctx.clone());
            tokio::spawn(async move { sup.switch_account(&ctx, AGENT, "home").await })
        };
        while !sup.logins.lock().prefetched.contains_key(AGENT) {
            tokio::task::yield_now().await;
        }
        sup.statuses
            .lock()
            .insert(AGENT.into(), AgentStatus::Running);
        drop(held);

        let why = switching.await.unwrap().unwrap_err().to_string();

        assert_eq!(why, BUSY_MSG);
        assert!(!sup.logins.lock().prefetched.contains_key(AGENT));
        assert_eq!(stamp(&sup).as_deref(), Some("work"));
        assert!(sup.agents.lock().contains_key(AGENT));
    });
}

#[test]
fn a_switch_forgets_the_old_accounts_rejected_token() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let sup = agent_on(td.path(), Some("work")).await;
        sup.logins.lock().rejected.insert(AGENT.into(), 42);
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();

        sup.switch_account(&ctx, AGENT, "home").await.unwrap();

        assert!(!sup.logins.lock().rejected.contains_key(AGENT));
    });
}

/// A send that arrives while the switch holds the agent waits for it, so it
/// routes against the new stamp: the restamp's `workspace:changed` comes
/// before the send's `turn:sent`.
#[test]
fn a_send_during_a_switch_routes_after_the_restamp() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let sup = agent_on(td.path(), Some("work")).await;
        let (ctx, sink, _dir) = crate::host::ctx::test_ctx();

        let held = sup.agent_lifecycle.lock().await;
        let switching = {
            let (sup, ctx) = (sup.clone(), ctx.clone());
            tokio::spawn(async move { sup.switch_account(&ctx, AGENT, "home").await })
        };
        let switch_holds_delivery = || {
            sup.delivery_locks
                .lock()
                .get(AGENT)
                .is_some_and(|lock| lock.try_lock().is_err())
        };
        while !switch_holds_delivery() {
            tokio::task::yield_now().await;
        }
        let sending = {
            let (sup, ctx) = (sup.clone(), ctx.clone());
            tokio::spawn(async move {
                sup.send_user_message(&ctx, AGENT, "t-1", "go on", &[])
                    .await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            event_names(&sink).is_empty(),
            "the send ran ahead of the switch"
        );
        drop(held);

        switching.await.unwrap().unwrap();
        let _ = sending.await.unwrap();

        let names = event_names(&sink);
        let restamped = names.iter().position(|n| n == "workspace:changed").unwrap();
        let sent = names.iter().position(|n| n == "turn:sent").unwrap();
        assert!(restamped < sent, "{names:?}");
    });
}

/// The removal gate reads the stamp, so the account an agent was switched off
/// is free to remove and the one it was switched onto is held.
#[test]
fn a_switch_moves_the_removal_gate_with_the_agent() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "work");
        signed_in(&root, "home");
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        let sup = Arc::new(Supervisor::new(Arc::new(
            crate::workspace::WorkspaceManager::new(ctx.db.clone()),
        )));
        let checkout = committed_repo(td.path(), "repo").await;
        let mut record = record_in_checkouts(&sup, AGENT, std::slice::from_ref(&checkout));
        record.account = Some("work".into());
        sup.workspace.add_agent(&mut record).unwrap();
        let removable =
            |id: &str| crate::commands::ensure_account_removable(&ctx, "claude", id).is_ok();
        assert!(!removable("work"));

        sup.switch_account(&ctx, AGENT, "home").await.unwrap();

        assert!(removable("work"));
        assert!(!removable("home"));
    });
}

/// A supervisor over the ctx's own database, so the accounts commands see the
/// agents the switch restamps; `AGENT` is stamped `work`.
async fn shared_db_agent(dir: &Path) -> (Arc<Supervisor>, Arc<EngineCtx>, tempfile::TempDir) {
    let (ctx, _sink, db_dir) = crate::host::ctx::test_ctx();
    let sup = Arc::new(Supervisor::new(Arc::new(
        crate::workspace::WorkspaceManager::new(ctx.db.clone()),
    )));
    let checkout = committed_repo(dir, "repo").await;
    let mut record = record_in_checkouts(&sup, AGENT, std::slice::from_ref(&checkout));
    record.account = Some("work".into());
    sup.workspace.add_agent(&mut record).unwrap();
    (sup, ctx, db_dir)
}

/// Start a switch to `to` and return once it holds the provider's account
/// lock, parked on the lifecycle lock `held` keeps.
async fn parked_switch(
    sup: &Arc<Supervisor>,
    ctx: &Arc<EngineCtx>,
    to: &'static str,
) -> tokio::task::JoinHandle<Result<AgentRecord>> {
    let switching = {
        let (sup, ctx) = (sup.clone(), ctx.clone());
        tokio::spawn(async move { sup.switch_account(&ctx, AGENT, to).await })
    };
    while !sup
        .delivery_locks
        .lock()
        .get(AGENT)
        .is_some_and(|lock| lock.try_lock().is_err())
    {
        tokio::task::yield_now().await;
    }
    assert!(ctx.account_locks.is_held("claude"));
    switching
}

/// The target is checked before it is stamped; a removal can't slip between
/// the two. It waits for the switch, then sees the new stamp and refuses.
#[test]
fn a_removal_during_a_switch_waits_and_then_refuses_the_new_stamp() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let (sup, ctx, _db) = shared_db_agent(td.path()).await;

        let held = sup.agent_lifecycle.lock().await;
        let switching = parked_switch(&sup, &ctx, "home").await;
        let removing = {
            let ctx = ctx.clone();
            tokio::spawn(async move {
                crate::commands::remove_provider_account_impl(&ctx, "claude", "home").await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!removing.is_finished(), "the removal ran inside the switch");
        drop(held);

        switching.await.unwrap().unwrap();
        let why = removing.await.unwrap().unwrap_err().to_string();

        assert!(why.contains("still run under this account"), "{why}");
        assert!(root.join("claude").join("home").is_dir());
        assert_eq!(stamp(&sup).as_deref(), Some("home"));
    });
}

/// A sign-out of the target waits for a switch that already checked its
/// login. Aborted before it runs, so no CLI logout is started here.
#[test]
fn a_sign_out_during_a_switch_waits_for_it() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let (sup, ctx, _db) = shared_db_agent(td.path()).await;

        let held = sup.agent_lifecycle.lock().await;
        let switching = parked_switch(&sup, &ctx, "home").await;
        let signing_out = {
            let ctx = ctx.clone();
            tokio::spawn(async move {
                crate::commands::sign_out_provider_account_impl(&ctx, "claude", "home").await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert!(
            !signing_out.is_finished(),
            "the sign-out ran inside the switch"
        );
        signing_out.abort();
        drop(held);
        switching.await.unwrap().unwrap();
        assert_eq!(stamp(&sup).as_deref(), Some("home"));
    });
}

/// Defence behind the lock: the re-check under the lifecycle lock still
/// refuses a target whose directory is gone.
#[test]
fn a_target_removed_before_the_restamp_is_refused() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let (sup, ctx, _db) = shared_db_agent(td.path()).await;
        // Live, so the switch fetches the target's token after its probe:
        // that token being kept is the sign the probe has passed.
        live_process(&sup, td.path());

        let held = sup.agent_lifecycle.lock().await;
        let switching = parked_switch(&sup, &ctx, "home").await;
        while !sup.logins.lock().prefetched.contains_key(AGENT) {
            tokio::task::yield_now().await;
        }
        std::fs::remove_dir_all(root.join("claude").join("home")).unwrap();
        drop(held);

        let why = switching.await.unwrap().unwrap_err().to_string();

        assert!(why.contains("No claude account named `home`"), "{why}");
        assert_eq!(stamp(&sup).as_deref(), Some("work"));
    });
}

// --- the active account moves the whole fleet ---

/// A second resting agent `id` of `provider` stamped `account`, in its own
/// checkout under `dir`, in the shared-db supervisor.
async fn add_agent(sup: &Supervisor, dir: &Path, id: &str, provider: &str, account: Option<&str>) {
    let checkout = committed_repo(dir, id).await;
    let mut record = record_in_checkouts(sup, id, std::slice::from_ref(&checkout));
    record.provider = provider.into();
    record.account = account.map(str::to_string);
    sup.workspace.add_agent(&mut record).unwrap();
}

fn stamp_of(sup: &Supervisor, id: &str) -> Option<String> {
    sup.workspace.agent(id).unwrap().account
}

#[test]
fn the_active_account_restamps_every_resting_agent_of_the_provider() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let (sup, ctx, _db) = shared_db_agent(td.path()).await;
        add_agent(&sup, td.path(), "denali", "claude", None).await;
        add_agent(&sup, td.path(), "rainier", "claude", Some("home")).await;
        add_agent(&sup, td.path(), "shasta", "codex", Some("work")).await;

        let moved = sup
            .follow_active_account(&ctx, "claude", Some("home"))
            .await
            .unwrap();

        assert_eq!(moved, 2);
        assert_eq!(stamp(&sup).as_deref(), Some("home"));
        assert_eq!(stamp_of(&sup, "denali").as_deref(), Some("home"));
        assert_eq!(stamp_of(&sup, "rainier").as_deref(), Some("home"));
        assert_eq!(
            stamp_of(&sup, "shasta").as_deref(),
            Some("work"),
            "another provider's agents stay"
        );
        assert!(
            sup.agents.lock().is_empty(),
            "resting agents launch nothing"
        );
    });
}

#[test]
fn the_active_account_emits_one_workspace_change_and_none_when_nothing_moves() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let (ctx, sink, _db) = crate::host::ctx::test_ctx();
        let sup = Arc::new(Supervisor::new(Arc::new(
            crate::workspace::WorkspaceManager::new(ctx.db.clone()),
        )));
        add_agent(&sup, td.path(), AGENT, "claude", Some("work")).await;

        sup.follow_active_account(&ctx, "claude", Some("home"))
            .await
            .unwrap();
        assert_eq!(event_names(&sink), vec!["workspace:changed".to_string()]);

        let moved = sup
            .follow_active_account(&ctx, "claude", Some("home"))
            .await
            .unwrap();
        assert_eq!(moved, 0);
        assert_eq!(event_names(&sink).len(), 1);
    });
}

#[test]
fn the_active_account_skips_archived_agents() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let (sup, ctx, _db) = shared_db_agent(td.path()).await;
        sup.workspace.begin_archive(AGENT).unwrap();

        let moved = sup
            .follow_active_account(&ctx, "claude", Some("home"))
            .await
            .unwrap();

        assert_eq!(moved, 0);
        assert_eq!(stamp(&sup).as_deref(), Some("work"));
    });
}

/// The turn in flight finishes on the token it has; the respawn the turn
/// end drains relaunches the agent under the new stamp.
#[test]
fn a_busy_agent_is_restamped_and_flagged_for_the_turn_end_respawn() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let (sup, ctx, _db) = shared_db_agent(td.path()).await;
        live_process(&sup, td.path());
        sup.statuses
            .lock()
            .insert(AGENT.into(), AgentStatus::Running);
        sup.logins.lock().rejected.insert(AGENT.into(), 42);

        sup.follow_active_account(&ctx, "claude", Some("home"))
            .await
            .unwrap();
        // The deferred respawn re-flags itself once it finds the agent busy.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        assert_eq!(stamp(&sup).as_deref(), Some("home"));
        assert!(sup.respawn_pending.lock().contains(AGENT));
        assert!(
            sup.agents.lock().contains_key(AGENT),
            "mid-turn, nothing stopped"
        );
        assert!(!sup.logins.lock().rejected.contains_key(AGENT));
    });
}

#[test]
fn an_errored_resting_agent_is_rested_on_the_active_account() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let (sup, ctx, _db) = shared_db_agent(td.path()).await;
        sup.set_status(&ctx, AGENT, AgentStatus::Error, Some("limit".into()));

        sup.follow_active_account(&ctx, "claude", Some("home"))
            .await
            .unwrap();

        assert_eq!(stamp(&sup).as_deref(), Some("home"));
        assert_ne!(sup.status_of(AGENT), Some(AgentStatus::Error));
        assert_ne!(
            sup.workspace.agent(AGENT).unwrap().status,
            AgentStatus::Error
        );
    });
}

#[test]
fn selecting_an_account_in_settings_moves_the_fleet_onto_it() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_in(&root, "home");
        let (sup, ctx, _db) = shared_db_agent(td.path()).await;

        crate::commands::set_active_provider_account_impl(&sup, &ctx, "claude", Some("home"))
            .await
            .unwrap();

        assert_eq!(
            crate::database::get_setting(&ctx.db.lock(), &accounts::active_setting_key("claude"))
                .as_deref(),
            Some("home")
        );
        assert_eq!(stamp(&sup).as_deref(), Some("home"));
    });
}

/// The whole fleet would launch on a login the account doesn't have.
#[test]
fn selecting_a_signed_out_account_in_settings_is_refused() {
    in_root(|root| async move {
        let td = tempfile::tempdir().unwrap();
        signed_out(&root, "home");
        let (sup, ctx, _db) = shared_db_agent(td.path()).await;

        let why =
            crate::commands::set_active_provider_account_impl(&sup, &ctx, "claude", Some("home"))
                .await
                .unwrap_err()
                .to_string();

        assert!(why.contains("`home` account isn't signed in"), "{why}");
        assert_eq!(
            crate::database::get_setting(&ctx.db.lock(), &accounts::active_setting_key("claude")),
            None
        );
        assert_eq!(stamp(&sup).as_deref(), Some("work"));
    });
}

/// A spawn that picks a signed-in account is stamped with it. (The default's
/// probe reads this machine's own login, so it isn't asserted here.)
#[test]
fn a_spawn_picks_a_signed_in_account() {
    in_root(|root| async move {
        signed_in(&root, "home");
        assert_eq!(
            chosen_account("claude", " home ").await.unwrap().as_deref(),
            Some("home")
        );
    });
}

#[test]
fn a_spawn_picking_a_missing_or_signed_out_account_is_refused() {
    in_root(|root| async move {
        signed_out(&root, "home");
        let missing = chosen_account("claude", "work").await.unwrap_err().to_string();
        assert!(missing.contains("No claude account named `work`"), "{missing}");
        let out = chosen_account("claude", "home").await.unwrap_err().to_string();
        assert!(out.contains("`home` account isn't signed in"), "{out}");
        let none = chosen_account("cursor", "home").await.unwrap_err().to_string();
        assert!(none.contains("no accounts"), "{none}");
    });
}

#[cfg(target_os = "macos")]
mod launched;
