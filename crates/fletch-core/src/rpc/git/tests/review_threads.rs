//! The thread ops over the local reviewer's threads: an engine behind the
//! dispatcher, an agent whose checkout the threads are about, no GitHub.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::host::sink::RecordingSink;
use crate::host::EngineCtx;
use crate::review::{self, Finding, ReplyAuthor};
use crate::rpc::caps::AgentCaps;
use crate::rpc::git::test_support::*;
use crate::rpc::git::GitDispatcher;
use crate::supervisor::Supervisor;
use crate::workspace::{new_agent_record, AgentView, TrackedRepo, WorkspaceManager};

const AGENT: &str = "arabia";

fn tracked_repo(path: &std::path::Path, subdir: &str) -> TrackedRepo {
    TrackedRepo {
        repo_path: path.to_path_buf(),
        subdir: subdir.to_string(),
        branch: None,
        parent_branch: None,
        base_sha: None,
        pr_number: None,
        pr_url: None,
        pr_title: None,
        pr_state: None,
        label: None,
        adopted_checkout: None,
    }
}

/// A host with one agent, `arabia`, whose primary checkout is a repo with no
/// origin (so nothing here can reach GitHub), and a dispatcher for it.
fn host() -> (
    Arc<EngineCtx>,
    Arc<RecordingSink>,
    GitDispatcher,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::database::init(dir.path()).unwrap();
    let sink = Arc::new(RecordingSink::new());
    let ctx = Arc::new(EngineCtx::new(sink.clone(), db.clone(), Box::new(|| false)));
    let sup = Arc::new(Supervisor::new(Arc::new(WorkspaceManager::new(db))));
    ctx.set_supervisor(sup.clone());

    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "feat/x"]);
    let mut record = new_agent_record(
        AGENT.to_string(),
        AGENT.to_string(),
        "claude".to_string(),
        tracked_repo(&repo, "repo"),
        String::new(),
        AgentView::Custom,
    );
    sup.workspace.add_agent(&mut record).unwrap();

    let disp = GitDispatcher::new(
        repo.clone(),
        repo,
        "main".to_string(),
        AgentCaps::interactive(),
    )
    .with_review(ctx.clone(), AGENT);
    (ctx, sink, disp, dir)
}

fn seed(ctx: &EngineCtx, subdir: Option<&str>, titles: &[&str]) -> Vec<String> {
    let findings: Vec<Finding> = titles
        .iter()
        .map(|t| Finding {
            path: Some("src/a.rs".into()),
            line: Some(3),
            severity: None,
            title: t.to_string(),
            body: String::new(),
        })
        .collect();
    let conn = ctx.db.lock();
    review::store::insert_findings(
        &conn,
        &(AGENT.to_string(), subdir.map(str::to_string)),
        1,
        "Reviewer",
        &findings,
    )
    .unwrap()
}

fn threads_events(sink: &RecordingSink) -> Vec<Value> {
    sink.events()
        .into_iter()
        .filter(|(name, _)| name == "pr:threads_changed")
        .map(|(_, payload)| payload)
        .collect()
}

/// A checkout with no origin and no PR still lists its local threads, in the
/// shape a GitHub thread has, instead of the "not a GitHub origin" refusal.
#[tokio::test]
async fn pr_threads_lists_local_threads_without_a_pr() {
    let (ctx, _sink, disp, _dir) = host();
    let ids = seed(&ctx, None, &["first", "second"]);

    let (resp, fx) = disp.dispatch_inner("t", "pr_threads", &Value::Null).await;
    assert!(resp.ok, "{:?}", resp.error);
    let listed: Vec<Value> = serde_json::from_str(resp.stdout.as_deref().unwrap()).unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0]["id"], ids[0]);
    assert_eq!(listed[0]["source"], "local");
    assert_eq!(listed[0]["author"], "Reviewer");
    assert_eq!(listed[0]["is_bot"], true);
    assert_eq!(listed[0]["body"], "first");
    assert_eq!(listed[0]["path"], "src/a.rs");
    assert_eq!(listed[0]["url"], Value::Null);
    assert_eq!(listed[0]["we_replied_last"], false);
    assert!(fx.is_empty(), "a read emits nothing");

    // With no local threads the read's own reason comes back, as before.
    for id in &ids {
        let conn = ctx.db.lock();
        review::store::resolve(&conn, id, ReplyAuthor::User).unwrap();
    }
    let (resp, _) = disp.dispatch_inner("t", "pr_threads", &Value::Null).await;
    assert_eq!(
        resp.error.as_deref(),
        Some("pr_threads: origin is not a GitHub repository (or there is no origin)")
    );
}

/// A reply leaves the thread open as a push-back; a resolve closes it. Each
/// tells the clients, with the merged set, and is attributed like a GitHub
/// mutation.
#[tokio::test]
async fn reply_and_resolve_answer_local_threads_and_announce_them() {
    let (ctx, sink, disp, _dir) = host();
    let ids = seed(&ctx, None, &["only"]);
    let id = ids[0].as_str();

    let (resp, fx) = disp
        .dispatch_inner(
            "r",
            "reply_thread",
            &json!({ "thread": id, "body": "by design" }),
        )
        .await;
    assert!(resp.ok, "{:?}", resp.error);
    assert_eq!(resp.stdout.as_deref(), Some("replied"));
    assert!(has_action_done(&fx, "reply_thread"));
    let events = threads_events(&sink);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["agent_id"], AGENT);
    assert_eq!(events[0]["subdir"], Value::Null);
    let unresolved = events[0]["comments"]["unresolved"].as_array().unwrap();
    assert_eq!(unresolved.len(), 1);
    assert_eq!(unresolved[0]["we_replied_last"], true);
    assert_eq!(unresolved[0]["replies"], 1);
    assert_eq!(events[0]["new_thread_ids"], json!([]));

    let (resp, fx) = disp
        .dispatch_inner("s", "resolve_thread", &json!({ "id": id }))
        .await;
    assert!(resp.ok, "{:?}", resp.error);
    assert_eq!(resp.stdout.as_deref(), Some("resolved"));
    assert!(has_action_done(&fx, "resolve_thread"));
    let events = threads_events(&sink);
    assert_eq!(events.len(), 2);
    assert_eq!(events[1]["comments"]["unresolved"], json!([]));

    // Gone: answered by its reason, not by a GitHub call.
    let (resp, fx) = disp
        .dispatch_inner("s", "resolve_thread", &json!({ "thread": id }))
        .await;
    assert!(resp.ok, "resolving twice keeps the first resolution");
    let (resp, fx2) = disp
        .dispatch_inner("s", "resolve_thread", &json!({ "thread": "local:nope" }))
        .await;
    assert_eq!(
        resp.error.as_deref(),
        Some("resolve_thread: no local review thread \"local:nope\" on this checkout")
    );
    assert!(!has_action_done(&fx2, "resolve_thread"));
    let _ = fx;
}

/// A thread about another checkout — a sibling repo, or another agent's —
/// is not this dispatcher's to answer.
#[tokio::test]
async fn a_thread_of_another_checkout_is_refused() {
    let (ctx, _sink, disp, _dir) = host();
    let other = seed(&ctx, Some("api"), &["elsewhere"]);
    let (resp, _) = disp
        .dispatch_inner(
            "r",
            "reply_thread",
            &json!({ "thread": other[0], "body": "x" }),
        )
        .await;
    assert!(resp
        .error
        .as_deref()
        .unwrap()
        .starts_with("reply_thread: no local review thread"));
}
