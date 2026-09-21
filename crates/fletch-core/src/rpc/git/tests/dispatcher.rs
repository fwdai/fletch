use crate::rpc::git::test_support::*;

use serde_json::{json, Value};

use crate::rpc::{ensure_mailbox, process_pending};

#[tokio::test]
async fn echo_round_trips_free_text() {
    let td = tempfile::tempdir().unwrap();
    let rpc_dir = td.path().join(".fletch-rpc");
    ensure_mailbox(&rpc_dir).unwrap();
    write_request(
        &rpc_dir.join("requests"),
        "req-echo.json",
        r#"{"id":"req-echo","op":"echo","args":{"message":"hello from the agent"}}"#,
    );

    let dispatcher = dispatcher(td.path());
    process_pending(&rpc_dir, &dispatcher).await;

    let body = std::fs::read_to_string(rpc_dir.join("responses/req-echo.json")).unwrap();
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["id"], "req-echo");
    assert_eq!(v["ok"], true);
    assert_eq!(v["stdout"], "hello from the agent");
}

/// `set_title` never touches the database itself: it validates, collapses
/// whitespace, and hands the title to the supervisor's watcher as an event.
#[tokio::test]
async fn set_title_relays_a_normalized_title_event() {
    let td = tempfile::tempdir().unwrap();
    let rpc_dir = td.path().join(".fletch-rpc");
    ensure_mailbox(&rpc_dir).unwrap();
    write_request(
        &rpc_dir.join("requests"),
        "t1.json",
        r#"{"id":"t1","op":"set_title","args":{"title":"  Swap sidebar\n title and prompt "}}"#,
    );

    let dispatcher = dispatcher(td.path());
    let effects = process_pending(&rpc_dir, &dispatcher).await;

    let body = std::fs::read_to_string(rpc_dir.join("responses/t1.json")).unwrap();
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["stdout"], "Swap sidebar title and prompt");

    let titles: Vec<&Value> = effects
        .iter()
        .filter_map(|e| {
            let crate::rpc::RpcEvent::Named { name, payload } = e;
            (name == crate::rpc::git::EVENT_TITLE_SET).then_some(payload)
        })
        .collect();
    assert_eq!(titles.len(), 1);
    assert_eq!(titles[0]["title"], "Swap sidebar title and prompt");
}

#[tokio::test]
async fn set_title_rejects_empty_and_oversized_titles() {
    let td = tempfile::tempdir().unwrap();
    let rpc_dir = td.path().join(".fletch-rpc");
    ensure_mailbox(&rpc_dir).unwrap();
    write_request(
        &rpc_dir.join("requests"),
        "empty.json",
        r#"{"id":"empty","op":"set_title","args":{"title":"   "}}"#,
    );
    let long = "x".repeat(81);
    write_request(
        &rpc_dir.join("requests"),
        "long.json",
        &format!(r#"{{"id":"long","op":"set_title","args":{{"title":"{long}"}}}}"#),
    );

    let dispatcher = dispatcher(td.path());
    let effects = process_pending(&rpc_dir, &dispatcher).await;

    for id in ["empty", "long"] {
        let body = std::fs::read_to_string(rpc_dir.join(format!("responses/{id}.json"))).unwrap();
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], false, "{id} should be refused");
    }
    assert!(effects.is_empty(), "a refused title must not emit an event");
}

#[tokio::test]
async fn signal_git_action_emits_action_done_for_known_actions() {
    let td = tempfile::tempdir().unwrap();
    let repo = td.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q"]);

    let rpc_dir = td.path().join("rpc");
    ensure_mailbox(&rpc_dir).unwrap();
    write_request(
        &rpc_dir.join("requests"),
        "s1.json",
        r#"{"id":"s1","op":"signal_git_action","args":{"action":"git_commit"}}"#,
    );

    let dispatcher = dispatcher(&repo);
    let effects = process_pending(&rpc_dir, &dispatcher).await;

    let body = std::fs::read_to_string(rpc_dir.join("responses/s1.json")).unwrap();
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["ok"], true, "response: {body}");
    assert!(
        has_action_done(&effects, "git_commit"),
        "a post-commit hook signal must relay an action-done, got: {effects:?}"
    );
}

#[tokio::test]
async fn signal_git_action_rejects_unknown_action() {
    let td = tempfile::tempdir().unwrap();
    let repo = td.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q"]);

    let disp = dispatcher(&repo);
    let (resp, effects) = disp
        .dispatch_inner("s", "signal_git_action", &json!({"action": "rm -rf"}))
        .await;
    assert!(!resp.ok, "an unrecognized action must be rejected");
    assert!(
        effects.is_empty(),
        "a rejected signal must emit nothing, got: {effects:?}"
    );
}

#[tokio::test]
async fn read_only_and_failed_ops_emit_no_action_done() {
    let td = tempfile::tempdir().unwrap();
    let repo = td.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "main"]);
    run_git(&repo, &["config", "user.email", "t@example.com"]);
    run_git(&repo, &["config", "user.name", "Tester"]);
    std::fs::write(repo.join("a.txt"), b"x").unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "init"]);
    let disp = dispatcher(&repo);

    let (_r, status_fx) = disp.dispatch_inner("s", "git_status", &Value::Null).await;
    assert!(
        !has_action_done(&status_fx, "git_status"),
        "git_status is read-only and must not signal an action"
    );

    let (resp, push_fx) = disp.dispatch_inner("p", "git_push", &Value::Null).await;
    assert!(!resp.ok);
    assert!(
        !has_action_done(&push_fx, "git_push"),
        "a failed git_push must not signal an action"
    );
}
