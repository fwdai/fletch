//! A claude workspace launched for real (under sandbox-exec) on a scripted
//! `claude` that logs, per launch, how it was started and a digest of the
//! `CLAUDE_CODE_OAUTH_TOKEN` it was handed, and answers every turn. Ignored:
//! they set process-wide roots and the claude binary override, so run each
//! one alone:
//!
//!   cargo test --lib scripted_switch -- --ignored --nocapture
//!   cargo test --lib scripted_codex_switch -- --ignored --nocapture
//!   FLETCH_LIVE_SWITCH=ai-eve:tttr-1 cargo test --lib live_switch -- --ignored --nocapture

use std::collections::HashMap;
use std::time::Duration;

use super::*;
use crate::agent::host_login::claude as claude_login;

fn digest(secret: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(secret.as_bytes())
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Answers `--version`, then one `result` per user message. The token is
/// hashed inside the sandbox; only the digest reaches the log.
fn fake_claude(dir: &Path, log: &Path) -> PathBuf {
    let script = dir.join("fake-claude");
    let body = format!(
        r#"#!/bin/sh
LOG='{log}'
if [ "$1" = "--version" ]; then echo "2.1.287 (Claude Code)"; exit 0; fi
tok=$(printf %s "$CLAUDE_CODE_OAUTH_TOKEN" | /usr/bin/shasum -a 256 | /usr/bin/cut -c1-12)
mode=none
prev=
for a in "$@"; do
  [ "$prev" = "--resume" ] && mode="resume:$a"
  [ "$prev" = "--session-id" ] && mode="fresh:$a"
  prev="$a"
done
echo "launch $tok $mode" >> "$LOG"
while IFS= read -r line; do
  case "$line" in
    *'"type":"user"'*)
      echo "turn $tok" >> "$LOG"
      echo '{{"type":"result","subtype":"success","is_error":false,"result":"ok"}}'
      ;;
  esac
done
"#,
        log = log.display()
    );
    std::fs::write(&script, body).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

fn log_lines(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

async fn wait_for(log: &Path, count: usize, prefix: &str) -> Vec<String> {
    for _ in 0..200 {
        let lines: Vec<String> = log_lines(log)
            .into_iter()
            .filter(|l| l.starts_with(prefix))
            .collect();
        if lines.len() >= count {
            return lines;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no {count} `{prefix}` lines in {:?}", log_lines(log));
}

async fn wait_idle(sup: &Supervisor) {
    for _ in 0..200 {
        if sup.status_of(AGENT) == Some(AgentStatus::Idle) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("agent never went idle: {:?}", sup.status_of(AGENT));
}

/// The token digest and expiry each account's launch resolves to.
async fn token_of(id: &str) -> (String, i64) {
    let dir = accounts::account_dir("claude", id).unwrap();
    let token = claude_login::access_token_for_launch(Some(&dir))
        .await
        .expect("a usable login");
    (digest(token.secret()), token.expires_at_ms())
}

/// One turn on `from`, a switch to `to` at the turn boundary with a send
/// racing it, and the turn after: what each launch was handed, as digests.
/// Points a process-wide root at the scratch dir for the test's length.
struct EnvRoot {
    var: &'static str,
    was: Option<std::ffi::OsString>,
}

impl EnvRoot {
    fn set(var: &'static str, to: PathBuf) -> Self {
        let was = std::env::var_os(var);
        std::env::set_var(var, to);
        Self { var, was }
    }
}

impl Drop for EnvRoot {
    fn drop(&mut self) {
        match self.was.take() {
            Some(was) => std::env::set_var(self.var, was),
            None => std::env::remove_var(self.var),
        }
    }
}

async fn switch_between(scratch: &Path, from: &str, to: &str) {
    // The reaper and reader threads spawn onto it once a process exits.
    crate::host::runtime::init(tokio::runtime::Handle::current());
    let _workspaces = EnvRoot::set(crate::workspace::WORKSPACES_ROOT_ENV, scratch.join("ws"));
    let _rpc = EnvRoot::set(crate::rpc::RPC_ROOT_ENV, scratch.join("rpc"));
    let parent = crate::workspace::agent_parent_dir(AGENT).unwrap();
    std::fs::create_dir_all(&parent).unwrap();
    let log = parent.join("launches.log");
    let script = fake_claude(&parent, &log);
    crate::bin_resolve::set_agent_overrides(HashMap::from([(
        "claude".to_string(),
        script.display().to_string(),
    )]));

    let checkout = committed_repo(&parent, "repo").await;
    let sup = Arc::new(test_supervisor());
    let mut record = record_in_checkouts(&sup, AGENT, std::slice::from_ref(&checkout));
    record.account = Some(from.to_string());
    sup.workspace.add_agent(&mut record).unwrap();
    let session = sup.workspace.agent(AGENT).unwrap().session_id.unwrap();
    let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
    ctx.set_supervisor(sup.clone());

    let (from_digest, from_expiry) = token_of(from).await;
    let (to_digest, to_expiry) = token_of(to).await;
    println!(
        "{from}: token {from_digest} expires_in_min {}; {to}: token {to_digest} expires_in_min {}",
        (from_expiry - crate::agent::host_login::now_ms()) / 60_000,
        (to_expiry - crate::agent::host_login::now_ms()) / 60_000,
    );
    assert_ne!(from_digest, to_digest, "the two accounts share a token");

    sup.start_process(&ctx, AGENT).await.expect("first launch");
    wait_idle(&sup).await;
    sup.clone()
        .send_user_message(&ctx, AGENT, "t-1", "first", &[])
        .await
        .unwrap();
    wait_for(&log, 1, "turn").await;
    wait_idle(&sup).await;

    // What claude leaves behind after a turn, so the next launch resumes.
    let projects = parent
        .join(crate::transcripts::DOCKER_CLAUDE_PROJECTS_DIRNAME)
        .join(crate::transcripts::claude_project_dirname(&checkout).unwrap());
    std::fs::create_dir_all(&projects).unwrap();
    std::fs::write(
        projects.join(format!("{session}.jsonl")),
        "{\"type\":\"user\",\"uuid\":\"u-1\"}\n",
    )
    .unwrap();

    // Park the switch holding the agent's delivery lock, so the send is
    // known to arrive during it.
    let held = sup.agent_lifecycle.lock().await;
    let switching = {
        let (sup, ctx, to) = (sup.clone(), ctx.clone(), to.to_string());
        tokio::spawn(async move { sup.switch_account(&ctx, AGENT, &to).await })
    };
    while !sup
        .delivery_locks
        .lock()
        .get(AGENT)
        .is_some_and(|lock| lock.try_lock().is_err())
    {
        tokio::task::yield_now().await;
    }
    let sending = {
        let (sup, ctx) = (sup.clone(), ctx.clone());
        tokio::spawn(async move {
            sup.send_user_message(&ctx, AGENT, "t-2", "second", &[])
                .await
        })
    };
    drop(held);
    let switched = switching.await.unwrap().expect("switch");
    let queued = sending.await.unwrap().unwrap();
    assert_eq!(switched.account.as_deref(), Some(to));

    let launches = wait_for(&log, 2, "launch").await;
    let turns = wait_for(&log, 2, "turn").await;
    println!("send during the switch held: {queued}");
    for line in log_lines(&log) {
        println!("{line}");
    }
    let from12 = &from_digest[..12];
    let to12 = &to_digest[..12];
    assert_eq!(launches[0], format!("launch {from12} fresh:{session}"));
    assert_eq!(launches[1], format!("launch {to12} resume:{session}"));
    assert_eq!(
        turns,
        vec![format!("turn {from12}"), format!("turn {to12}")]
    );
    assert_eq!(
        sup.workspace.agent(AGENT).unwrap().account.as_deref(),
        Some(to)
    );

    sup.shutdown();
    crate::bin_resolve::set_agent_overrides(HashMap::new());
}

#[test]
#[ignore]
fn scripted_switch_moves_a_resumed_session_onto_the_other_accounts_token() {
    in_root(|root| async move {
        signed_in(&root, "work");
        signed_in(&root, "home");
        let scratch = tempfile::tempdir().unwrap();
        switch_between(scratch.path(), "work", "home").await;
    });
}

/// Against this Mac's real claude accounts (`FLETCH_LIVE_SWITCH=<from>:<to>`,
/// managed ids). Reads their logins as a launch does; nothing reaches the API.
#[test]
#[ignore]
fn live_switch_moves_a_resumed_session_onto_the_other_accounts_token() {
    let pair = std::env::var("FLETCH_LIVE_SWITCH")
        .expect("set FLETCH_LIVE_SWITCH=<from>:<to>, two managed claude account ids");
    let (from, to) = pair.split_once(':').expect("<from>:<to>");
    let scratch = tempfile::tempdir().unwrap();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(switch_between(scratch.path(), from, to));
}

/// A per-turn `codex`: every turn is one process, which logs the digest of the
/// access token in its `CODEX_HOME`'s `auth.json`, whether that file still
/// carries a refresh token, and the thread it resumed, then answers.
fn fake_codex(dir: &Path, log: &Path) -> PathBuf {
    let script = dir.join("fake-codex");
    let body = format!(
        r#"#!/bin/sh
LOG='{log}'
if [ "$1" = "--version" ]; then echo "codex-cli 0.154.0"; exit 0; fi
auth="$CODEX_HOME/auth.json"
tok=$(/usr/bin/grep -o '"access_token": *"[^"]*"' "$auth" | /usr/bin/shasum -a 256 | /usr/bin/cut -c1-12)
if /usr/bin/grep -q '"refresh_token": *""' "$auth"; then rt=blank; else rt=present; fi
mode=fresh
prev=
for a in "$@"; do
  [ "$prev" = "resume" ] && mode="resume:$a"
  prev="$a"
done
echo "turn $tok $rt $mode" >> "$LOG"
echo '{{"type":"thread.started","thread_id":"thread-1"}}'
echo '{{"type":"turn.started"}}'
echo '{{"type":"item.completed","item":{{"id":"item_0","type":"agent_message","text":"ok"}}}}'
echo '{{"type":"turn.completed","usage":{{}}}}'
"#,
        log = log.display()
    );
    std::fs::write(&script, body).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

/// The digest `fake_codex` logs for an account's login.
fn codex_token_digest(root: &Path, id: &str) -> String {
    let login: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join("codex").join(id).join("auth.json")).unwrap(),
    )
    .unwrap();
    let pretty = serde_json::to_string_pretty(&login).unwrap();
    let line = pretty
        .lines()
        .find(|l| l.contains("\"access_token\""))
        .unwrap()
        .trim()
        .trim_end_matches(',')
        .to_string();
    use sha2::{Digest, Sha256};
    Sha256::digest(format!("{line}\n").as_bytes())
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A per-turn codex workspace: a turn on `work`, a switch to `home` between
/// turns, and the turn after. Each turn's process reads the overlay
/// credential the host wrote for the stamp it runs under, never sees a
/// refresh token, and the second turn resumes the first turn's thread.
#[test]
#[ignore]
fn scripted_codex_switch_moves_the_next_turn_onto_the_other_login() {
    in_root(|root| async move {
        codex_signed_in(&root, "work");
        codex_signed_in(&root, "home");
        let scratch = tempfile::tempdir().unwrap();
        crate::host::runtime::init(tokio::runtime::Handle::current());
        std::env::set_var(
            crate::workspace::WORKSPACES_ROOT_ENV,
            scratch.path().join("ws"),
        );
        std::env::set_var(crate::rpc::RPC_ROOT_ENV, scratch.path().join("rpc"));
        let parent = crate::workspace::agent_parent_dir(AGENT).unwrap();
        std::fs::create_dir_all(&parent).unwrap();
        let log = parent.join("turns.log");
        let script = fake_codex(&parent, &log);
        crate::bin_resolve::set_agent_overrides(HashMap::from([(
            "codex".to_string(),
            script.display().to_string(),
        )]));

        let checkout = committed_repo(&parent, "repo").await;
        let sup = Arc::new(test_supervisor());
        let mut record = record_in_checkouts(&sup, AGENT, std::slice::from_ref(&checkout));
        record.provider = "codex".into();
        record.account = Some("work".into());
        record.session_id = None;
        sup.workspace.add_agent(&mut record).unwrap();
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        ctx.set_supervisor(sup.clone());

        sup.start_process(&ctx, AGENT).await.expect("first launch");
        sup.clone()
            .send_user_message(&ctx, AGENT, "t-1", "first", &[])
            .await
            .unwrap();
        wait_for(&log, 1, "turn").await;
        wait_idle(&sup).await;

        let switched = sup
            .switch_account(&ctx, AGENT, "home")
            .await
            .expect("switch");
        assert_eq!(switched.account.as_deref(), Some("home"));
        sup.clone()
            .send_user_message(&ctx, AGENT, "t-2", "second", &[])
            .await
            .unwrap();
        let turns = wait_for(&log, 2, "turn").await;
        for line in log_lines(&log) {
            println!("{line}");
        }

        let work = codex_token_digest(&root, "work");
        let home = codex_token_digest(&root, "home");
        assert_ne!(work, home);
        assert_eq!(turns[0], format!("turn {work} blank fresh"));
        assert_eq!(turns[1], format!("turn {home} blank resume:thread-1"));
        let overlay = parent.join(crate::agent::codex_home::OVERLAY_DIRNAME);
        assert!(overlay.join("sessions").is_dir());

        sup.shutdown();
        crate::bin_resolve::set_agent_overrides(HashMap::new());
    });
}
