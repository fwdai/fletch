use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::*;
use crate::agent::Agent;
use crate::host::sink::RecordingSink;
use crate::pty_session::{PtySession, PtySpawn};
use crate::sandbox::KillHandle;
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
    crate::agent::claude_oauth::now_ms() + 7 * 24 * 3_600_000
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
    let pty = PtySession::spawn(
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
    .unwrap();
    sup.agents
        .lock()
        .insert(AGENT.to_string(), Arc::new(Agent::over_pty(pty)));
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

#[test]
fn a_provider_whose_sessions_live_in_the_account_dir_is_refused_by_name() {
    in_root(|root| async move {
        std::fs::create_dir_all(root.join("codex").join("work")).unwrap();
        let mut record = record_with_status(AGENT, AgentStatus::Idle);
        record.provider = "codex".into();
        let why = refusal(&record, "work");
        assert!(why.contains("isn't available for codex"), "{why}");
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

#[cfg(target_os = "macos")]
mod launched;
