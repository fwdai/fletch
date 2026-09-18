use super::*;
use crate::rpc::git::test_support::*;

use serde_json::{json, Value};

use crate::rpc::caps::AgentCaps;
use crate::rpc::git::{GitDispatcher, EVENT_BRANCH_CREATED};
use crate::rpc::RpcEvent;

#[test]
fn the_primary_checkout_is_reported_as_no_repo() {
    assert_eq!(approval_repo(Some("app"), Some("app")), None);
    assert_eq!(approval_repo(Some("web"), Some("app")), Some("web"));
    assert_eq!(approval_repo(None, None), None);
}

#[tokio::test]
async fn repo_arg_targets_sibling_checkout() {
    let td = tempfile::tempdir().unwrap();
    let a = td.path().join("a");
    let b = td.path().join("b");
    for repo in [&a, &b] {
        std::fs::create_dir_all(repo).unwrap();
        run_git(repo, &["init", "-q", "-b", "main"]);
    }
    std::fs::write(b.join("x.txt"), b"x").unwrap();

    let disp =
        GitDispatcher::new(a.clone(), "main".into(), AgentCaps::interactive()).with_repos(vec![
            ("a".into(), a.clone(), "main".into(), None),
            ("b".into(), b.clone(), "main".into(), None),
        ]);

    let (resp, _fx) = disp
        .dispatch_inner("s1", "git_status", &json!({"repo": "b"}))
        .await;
    assert_eq!(resp.exit_code, Some(0), "status in b: {resp:?}");
    assert!(
        resp.stdout.as_deref().unwrap_or_default().contains("x.txt"),
        "targeting `b` must see its dirty file: {resp:?}"
    );

    let (resp, _fx) = disp.dispatch_inner("s2", "git_status", &Value::Null).await;
    assert!(
        !resp.stdout.as_deref().unwrap_or_default().contains("x.txt"),
        "the default target must remain the primary checkout: {resp:?}"
    );
}

#[tokio::test]
async fn branch_events_carry_the_targeted_repo() {
    let td = tempfile::tempdir().unwrap();
    let a = td.path().join("a");
    let b = td.path().join("b");
    for repo in [&a, &b] {
        std::fs::create_dir_all(repo).unwrap();
        run_git(repo, &["init", "-q", "-b", "main"]);
        run_git(repo, &["config", "user.email", "t@example.com"]);
        run_git(repo, &["config", "user.name", "Tester"]);
        std::fs::write(repo.join("f.txt"), b"x").unwrap();
        run_git(repo, &["add", "-A"]);
        run_git(repo, &["commit", "-q", "-m", "init"]);
        run_git(repo, &["checkout", "-q", "--detach"]);
    }

    let disp =
        GitDispatcher::new(a.clone(), "main".into(), AgentCaps::interactive()).with_repos(vec![
            ("a".into(), a.clone(), "main".into(), None),
            ("b".into(), b.clone(), "main".into(), None),
        ]);

    let (_resp, fx) = disp
        .dispatch_inner(
            "p1",
            "git_push",
            &json!({"repo": "b", "branch": "feat/backend"}),
        )
        .await;
    assert!(
        fx.iter().any(|e| matches!(
            e,
            RpcEvent::Named { name, payload }
                if name == EVENT_BRANCH_CREATED
                    && payload["branch"] == "feat/backend"
                    && payload["repo"] == "b"
        )),
        "branch event must carry the targeted repo, got: {fx:?}"
    );

    let (_resp, fx) = disp
        .dispatch_inner("p2", "git_push", &json!({"branch": "feat/front"}))
        .await;
    assert!(
        fx.iter().any(|e| matches!(
            e,
            RpcEvent::Named { name, payload }
                if name == EVENT_BRANCH_CREATED && payload["repo"] == "a"
        )),
        "defaulted op must attribute to the primary subdir, got: {fx:?}"
    );
}

#[tokio::test]
async fn unknown_repo_arg_is_rejected_with_tracked_names() {
    let td = tempfile::tempdir().unwrap();
    let a = td.path().join("a");
    std::fs::create_dir_all(&a).unwrap();
    run_git(&a, &["init", "-q", "-b", "main"]);

    let disp = GitDispatcher::new(a.clone(), "main".into(), AgentCaps::interactive())
        .with_repos(vec![("a".into(), a.clone(), "main".into(), None)]);
    let (resp, fx) = disp
        .dispatch_inner("p", "git_push", &json!({"repo": "nope"}))
        .await;
    assert!(!resp.ok, "an unknown repo must be rejected: {resp:?}");
    let err = resp.error.as_deref().unwrap_or_default();
    assert!(err.contains("unknown repo"), "got: {err}");
    assert!(err.contains('a'), "should list tracked checkouts: {err}");
    assert!(fx.is_empty(), "a rejected op must emit nothing: {fx:?}");
}
