use crate::rpc::git::test_support::*;

use serde_json::{json, Value};

use crate::rpc::{ensure_mailbox, process_pending};

#[tokio::test]
async fn git_status_runs_a_real_command() {
    let td = tempfile::tempdir().unwrap();
    let rpc_dir = td.path().join(".fletch-rpc");
    ensure_mailbox(&rpc_dir).unwrap();
    write_request(
        &rpc_dir.join("requests"),
        "req-4.json",
        r#"{"id":"req-4","op":"git_status"}"#,
    );

    let dispatcher = dispatcher(td.path());
    process_pending(&rpc_dir, &dispatcher).await;

    let body = std::fs::read_to_string(rpc_dir.join("responses/req-4.json")).unwrap();
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["id"], "req-4");
    assert_eq!(v["ok"], true);
    assert!(v["exit_code"].is_number());
}

#[tokio::test]
async fn git_fetch_without_remote_reports_error() {
    let td = tempfile::tempdir().unwrap();
    let repo = td.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "main"]);

    let disp = dispatcher(&repo);
    let (resp, effects) = disp
        .dispatch_inner("f1", "git_fetch", &json!({"ref": "main"}))
        .await;
    assert!(
        !resp.ok,
        "a failed fetch must be an error response, got: {resp:?}"
    );
    assert!(
        resp.error.as_deref().unwrap_or_default().contains("failed"),
        "error should explain the fetch failed, got: {:?}",
        resp.error
    );
    assert!(!has_action_done(&effects, "git_fetch"));
}

#[tokio::test]
async fn git_fetch_refuses_option_like_ref() {
    let td = tempfile::tempdir().unwrap();
    let repo = td.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "main"]);

    let disp = dispatcher(&repo);
    let (resp, _fx) = disp
        .dispatch_inner("f2", "git_fetch", &json!({"ref": "--upload-pack=evil"}))
        .await;
    assert!(!resp.ok);
    assert!(resp
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("option-like"));
}
