//! A claude workspace launched for real (under sandbox-exec) on a scripted
//! `claude` that logs how each launch started and a digest of the token it
//! was handed. Ignored: they set process-wide roots and the claude binary
//! override, so run each alone:
//!
//!   cargo test --lib scripted_relaunch -- --ignored --test-threads=1

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};

use super::*;
use crate::supervisor::tests::{committed_repo, record_in_checkouts};

const AGENT: &str = "matterhorn";

fn digest(secret: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(secret.as_bytes())
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect()
}

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
  [ "$prev" = "--resume" ] && mode="resume"
  [ "$prev" = "--session-id" ] && mode="fresh"
  prev="$a"
done
echo "launch $tok $mode" >> "$LOG"
while IFS= read -r line; do :; done
"#,
        log = log.display()
    );
    std::fs::write(&script, body).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

fn launches(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

async fn wait_for_launches(log: &Path, count: usize) -> Vec<String> {
    for _ in 0..200 {
        let lines = launches(log);
        if lines.len() >= count {
            return lines;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no {count} launches in {:?}", launches(log));
}

/// A login with no refresh token, so resolving it never reaches the network.
fn write_login(dir: &Path, access: &str, expires_at_ms: i64) {
    let blob = json!({"claudeAiOauth": {"accessToken": access, "expiresAt": expires_at_ms}});
    std::fs::write(dir.join(".credentials.json"), blob.to_string()).unwrap();
}

struct Launched {
    sup: Arc<Supervisor>,
    ctx: Arc<EngineCtx>,
    log: PathBuf,
    account: PathBuf,
    _keep: Vec<tempfile::TempDir>,
}

/// A managed-account agent launched once, on a token `expires_in_ms` away,
/// with a transcript so the next launch resumes.
async fn launched(root: &Path, expires_in_ms: i64) -> Launched {
    crate::host::runtime::init(tokio::runtime::Handle::current());
    let scratch = tempfile::tempdir().unwrap();
    std::env::set_var(
        crate::workspace::WORKSPACES_ROOT_ENV,
        scratch.path().join("ws"),
    );
    std::env::set_var(crate::rpc::RPC_ROOT_ENV, scratch.path().join("rpc"));
    let account = root.join("claude").join("work");
    std::fs::create_dir_all(&account).unwrap();
    write_login(
        &account,
        "sk-ant-oat01-first",
        claude_oauth::now_ms() + expires_in_ms,
    );

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
    record.account = Some("work".into());
    sup.workspace.add_agent(&mut record).unwrap();
    let session = sup.workspace.agent(AGENT).unwrap().session_id.unwrap();
    let (ctx, _sink, dir) = crate::host::ctx::test_ctx();
    ctx.set_supervisor(sup.clone());

    sup.start_process(&ctx, AGENT).await.expect("first launch");
    wait_for_launches(&log, 1).await;
    let projects = parent
        .join(crate::transcripts::DOCKER_CLAUDE_PROJECTS_DIRNAME)
        .join(crate::transcripts::claude_project_dirname(&checkout).unwrap());
    std::fs::create_dir_all(&projects).unwrap();
    std::fs::write(
        projects.join(format!("{session}.jsonl")),
        "{\"type\":\"user\",\"uuid\":\"u-1\"}\n",
    )
    .unwrap();
    Launched {
        sup,
        ctx,
        log,
        account,
        _keep: vec![scratch, dir],
    }
}

fn in_root<F, Fut>(test: F)
where
    F: FnOnce(PathBuf) -> Fut,
    Fut: Future<Output = ()>,
{
    crate::agent::accounts::with_test_root(|root| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(test(root.to_path_buf()));
    });
    crate::bin_resolve::set_agent_overrides(HashMap::new());
}

#[test]
#[ignore]
fn scripted_relaunch_with_resume_resumes_on_the_current_login() {
    in_root(|root| async move {
        let run = launched(&root, 5 * HOUR_MS).await;
        write_login(
            &run.account,
            "sk-ant-oat01-second",
            claude_oauth::now_ms() + 6 * HOUR_MS,
        );
        run.sup
            .relaunch_with_resume(&run.ctx, AGENT)
            .await
            .expect("relaunch");
        let lines = wait_for_launches(&run.log, 2).await;
        println!("{lines:?}");
        assert_eq!(
            lines[1],
            format!("launch {} resume", &digest("sk-ant-oat01-second")[..12])
        );
        run.sup.shutdown();
    });
}

#[test]
#[ignore]
fn scripted_relaunch_if_login_due_keeps_the_process_without_a_fresher_token() {
    in_root(|root| async move {
        let run = launched(&root, 10 * 60 * 1000).await;
        run.sup.relaunch_if_login_due(&run.ctx, AGENT).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(launches(&run.log).len(), 1, "{:?}", launches(&run.log));
        run.sup.shutdown();
    });
}

#[test]
#[ignore]
fn scripted_relaunch_if_login_due_moves_onto_a_fresher_token() {
    in_root(|root| async move {
        let run = launched(&root, 10 * 60 * 1000).await;
        write_login(
            &run.account,
            "sk-ant-oat01-second",
            claude_oauth::now_ms() + 6 * HOUR_MS,
        );
        run.sup.relaunch_if_login_due(&run.ctx, AGENT).await;
        let lines = wait_for_launches(&run.log, 2).await;
        println!("{lines:?}");
        assert_eq!(
            lines[1],
            format!("launch {} resume", &digest("sk-ant-oat01-second")[..12])
        );
        run.sup.shutdown();
    });
}
