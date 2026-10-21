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
        resp.error
            .as_deref()
            .unwrap_or_default()
            .contains("no origin remote"),
        "error should explain there is nothing to fetch from, got: {:?}",
        resp.error
    );
    assert!(!has_action_done(&effects, "git_fetch"));
}

/// The checkout's `origin` is the agent's to rewrite. A `git_fetch` needs no
/// approval and runs with the user's token, so it must fetch only from the
/// remote the user's repository names — never from wherever the checkout's
/// origin now points — and refuse, with the change named, when they differ.
#[tokio::test]
async fn git_fetch_refuses_a_checkout_whose_origin_was_repointed() {
    let td = tempfile::tempdir().unwrap();
    let origin = td.path().join("origin.git");
    run_git(
        td.path(),
        &["init", "-q", "--bare", origin.to_str().unwrap()],
    );
    let source = td.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    run_git(&source, &["init", "-q", "-b", "main"]);
    run_git(&source, &["config", "user.email", "t@example.com"]);
    run_git(&source, &["config", "user.name", "Tester"]);
    run_git(
        &source,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    std::fs::write(source.join("a.txt"), b"x").unwrap();
    run_git(&source, &["add", "-A"]);
    run_git(&source, &["commit", "-q", "-m", "init"]);
    run_git(&source, &["push", "-q", "origin", "main"]);
    let checkout = td.path().join("checkout");
    run_git(
        td.path(),
        &[
            "clone",
            "-q",
            source.to_str().unwrap(),
            checkout.to_str().unwrap(),
        ],
    );
    run_git(
        &checkout,
        &["remote", "set-url", "origin", origin.to_str().unwrap()],
    );

    // A repository the user can read and the agent must not: one secret commit.
    let private = td.path().join("private");
    std::fs::create_dir_all(&private).unwrap();
    run_git(&private, &["init", "-q", "-b", "main"]);
    run_git(&private, &["config", "user.email", "t@example.com"]);
    run_git(&private, &["config", "user.name", "Tester"]);
    std::fs::write(private.join("secret.txt"), b"s3cret").unwrap();
    run_git(&private, &["add", "-A"]);
    run_git(&private, &["commit", "-q", "-m", "secret"]);
    let secret = crate::git::testing::git(&private, &["rev-parse", "HEAD"]);

    // The agent repoints its origin and asks the host to fetch.
    run_git(
        &checkout,
        &["remote", "set-url", "origin", private.to_str().unwrap()],
    );
    let disp = crate::rpc::git::GitDispatcher::new(
        checkout.clone(),
        source.clone(),
        "main".to_string(),
        crate::rpc::caps::AgentCaps::interactive(),
    );
    let (resp, effects) = disp
        .dispatch_inner("f3", "git_fetch", &json!({"ref": "main"}))
        .await;

    assert!(!resp.ok, "a repointed origin must be refused: {resp:?}");
    let err = resp.error.as_deref().unwrap_or_default();
    assert!(err.contains("refusing"), "{err}");
    assert!(
        err.contains("private"),
        "names where origin now points: {err}"
    );
    assert!(!has_action_done(&effects, "git_fetch"));
    let have = crate::git::testing::git_output(&checkout, &["cat-file", "-e", &secret]);
    assert!(
        !have.status.success(),
        "nothing was fetched from the repointed origin"
    );

    // Restored, the same request fetches from the user's remote.
    run_git(
        &checkout,
        &["remote", "set-url", "origin", origin.to_str().unwrap()],
    );
    let (resp, _) = disp
        .dispatch_inner("f4", "git_fetch", &json!({"ref": "main"}))
        .await;
    assert!(resp.ok && resp.exit_code == Some(0), "{resp:?}");
    assert_eq!(
        crate::git::testing::git(&checkout, &["rev-parse", "refs/remotes/origin/main"]),
        crate::git::testing::git(&source, &["rev-parse", "main"]),
        "the fetch lands under origin/<branch> as before"
    );
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
