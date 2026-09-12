use crate::rpc::git::test_support::*;

use serde_json::{json, Value};

use crate::rpc::git::EVENT_BRANCH_CREATED;
use crate::rpc::{ensure_mailbox, process_pending, RpcEvent};

#[tokio::test]
async fn git_push_refuses_option_named_head_branch() {
    let td = tempfile::tempdir().unwrap();
    let repo = td.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "main"]);
    run_git(&repo, &["config", "user.email", "t@example.com"]);
    run_git(&repo, &["config", "user.name", "Tester"]);
    std::fs::write(repo.join("a.txt"), b"x").unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "init"]);

    // Repoint HEAD at an option-named branch, as an agent controlling its own
    // `.git` could.
    let head = std::process::Command::new("git")
        .current_dir(&repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let sha = String::from_utf8_lossy(&head.stdout).trim().to_string();
    std::fs::write(repo.join(".git/refs/heads/--mirror"), format!("{sha}\n")).unwrap();
    std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/--mirror\n").unwrap();

    let disp = dispatcher(&repo);
    let (resp, fx) = disp.dispatch_inner("p", "git_push", &Value::Null).await;
    assert!(
        !resp.ok,
        "an option-named HEAD must be refused before any push: {resp:?}"
    );
    assert!(
        resp.error
            .as_deref()
            .unwrap_or_default()
            .contains("option-like"),
        "error must name the refusal, got: {:?}",
        resp.error
    );
    assert!(fx.is_empty(), "a refused push must emit nothing: {fx:?}");
}

#[tokio::test]
async fn git_push_without_remote_reports_error() {
    let td = tempfile::tempdir().unwrap();
    let repo = td.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "main"]);
    run_git(&repo, &["config", "user.email", "t@example.com"]);
    run_git(&repo, &["config", "user.name", "Tester"]);
    std::fs::write(repo.join("a.txt"), b"x").unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "init"]);
    // Own branch: pushing the review base is refused outright, which would mask
    // the missing-remote error under test.
    run_git(&repo, &["checkout", "-q", "-b", "fix/no-remote"]);

    let rpc_dir = td.path().join("rpc");
    ensure_mailbox(&rpc_dir).unwrap();
    write_request(
        &rpc_dir.join("requests"),
        "p1.json",
        r#"{"id":"p1","op":"git_push"}"#,
    );

    let dispatcher = dispatcher(&repo);
    process_pending(&rpc_dir, &dispatcher).await;

    let body = std::fs::read_to_string(rpc_dir.join("responses/p1.json")).unwrap();
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["ok"], false);
    let err = v["error"].as_str().unwrap();
    assert!(!err.contains("unknown op"), "got: {err}");
    assert!(err.contains("push failed"), "got: {err}");
}

#[tokio::test]
async fn git_push_refuses_the_review_base() {
    let td = tempfile::tempdir().unwrap();
    let remote = td.path().join("remote.git");
    run_git(
        td.path(),
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    let repo = td.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "main"]);
    run_git(&repo, &["config", "user.email", "t@example.com"]);
    run_git(&repo, &["config", "user.name", "Tester"]);
    run_git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    std::fs::write(repo.join("a.txt"), b"x").unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "init"]);

    let rpc_dir = td.path().join("rpc");
    ensure_mailbox(&rpc_dir).unwrap();
    write_request(
        &rpc_dir.join("requests"),
        "p1.json",
        r#"{"id":"p1","op":"git_push","args":{"force":true}}"#,
    );
    process_pending(&rpc_dir, &dispatcher(&repo)).await;

    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(rpc_dir.join("responses/p1.json")).unwrap())
            .unwrap();
    assert_eq!(
        v["ok"], false,
        "pushing the review base must be refused: {v}"
    );
    assert!(v["error"].as_str().unwrap().contains("refusing to publish"));
    let refs = std::process::Command::new("git")
        .args(["--git-dir", remote.to_str().unwrap(), "for-each-ref"])
        .output()
        .expect("git");
    assert!(
        String::from_utf8_lossy(&refs.stdout).trim().is_empty(),
        "the remote must have no refs"
    );
}

#[tokio::test]
async fn git_push_detached_without_branch_arg_errors() {
    let td = tempfile::tempdir().unwrap();
    let repo = td.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "main"]);
    run_git(&repo, &["config", "user.email", "t@example.com"]);
    run_git(&repo, &["config", "user.name", "Tester"]);
    std::fs::write(repo.join("a.txt"), b"x").unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "init"]);
    run_git(&repo, &["checkout", "-q", "--detach"]);

    let rpc_dir = td.path().join("rpc");
    ensure_mailbox(&rpc_dir).unwrap();
    write_request(
        &rpc_dir.join("requests"),
        "p1.json",
        r#"{"id":"p1","op":"git_push"}"#,
    );

    let dispatcher = dispatcher(&repo);
    process_pending(&rpc_dir, &dispatcher).await;

    let body = std::fs::read_to_string(rpc_dir.join("responses/p1.json")).unwrap();
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["ok"], false);
    assert!(
        v["error"].as_str().unwrap().contains("args.branch"),
        "got: {body}"
    );
}

#[tokio::test]
async fn git_push_detached_materializes_named_branch() {
    let td = tempfile::tempdir().unwrap();
    let repo = td.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "main"]);
    run_git(&repo, &["config", "user.email", "t@example.com"]);
    run_git(&repo, &["config", "user.name", "Tester"]);
    std::fs::write(repo.join("a.txt"), b"x").unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "init"]);
    run_git(&repo, &["checkout", "-q", "--detach"]);

    let rpc_dir = td.path().join("rpc");
    ensure_mailbox(&rpc_dir).unwrap();
    write_request(
        &rpc_dir.join("requests"),
        "p1.json",
        r#"{"id":"p1","op":"git_push","args":{"branch":"fix/thing"}}"#,
    );

    let dispatcher = dispatcher(&repo);
    let effects = process_pending(&rpc_dir, &dispatcher).await;

    let head = std::process::Command::new("git")
        .current_dir(&repo)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "fix/thing");
    assert!(
        effects.iter().any(|e| matches!(
            e,
            RpcEvent::Named { name, payload }
                if name == EVENT_BRANCH_CREATED && payload["branch"] == "fix/thing"
        )),
        "expected a branch-created event, got: {effects:?}"
    );
}

#[tokio::test]
async fn git_push_force_rewrites_diverged_remote() {
    let td = tempfile::tempdir().unwrap();
    let remote = td.path().join("origin.git");
    run_git(
        td.path(),
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );

    let repo = td.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "main"]);
    run_git(&repo, &["config", "user.email", "t@example.com"]);
    run_git(&repo, &["config", "user.name", "Tester"]);
    run_git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    std::fs::write(repo.join("a.txt"), b"x").unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "init"]);
    // Own branch: force-push is only reachable off the review base.
    run_git(&repo, &["checkout", "-q", "-b", "fix/diverged"]);

    let rpc_dir = td.path().join("rpc");
    ensure_mailbox(&rpc_dir).unwrap();
    let requests = rpc_dir.join("requests");
    let dispatcher = dispatcher(&repo).with_own_branch(Some("fix/diverged".to_string()));

    write_request(&requests, "p1.json", r#"{"id":"p1","op":"git_push"}"#);
    process_pending(&rpc_dir, &dispatcher).await;
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(rpc_dir.join("responses/p1.json")).unwrap())
            .unwrap();
    assert_eq!(v["ok"], true, "seed push should succeed: {v}");

    run_git(&repo, &["commit", "-q", "--amend", "-m", "rewritten"]);

    write_request(&requests, "p2.json", r#"{"id":"p2","op":"git_push"}"#);
    process_pending(&rpc_dir, &dispatcher).await;
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(rpc_dir.join("responses/p2.json")).unwrap())
            .unwrap();
    assert_eq!(v["ok"], false, "diverged push must be rejected: {v}");

    write_request(
        &requests,
        "p3.json",
        r#"{"id":"p3","op":"git_push","args":{"force":true}}"#,
    );
    process_pending(&rpc_dir, &dispatcher).await;
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(rpc_dir.join("responses/p3.json")).unwrap())
            .unwrap();
    assert_eq!(v["ok"], true, "force push should succeed: {v}");

    let local = std::process::Command::new("git")
        .current_dir(&repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let remote_head = std::process::Command::new("git")
        .current_dir(&remote)
        .args(["rev-parse", "refs/heads/fix/diverged"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&local.stdout).trim(),
        String::from_utf8_lossy(&remote_head.stdout).trim(),
        "the remote branch should match the rewritten local HEAD"
    );
}

#[tokio::test]
async fn git_push_force_refuses_a_branch_the_agent_does_not_own() {
    let td = tempfile::tempdir().unwrap();
    let repo = td.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "main"]);
    run_git(&repo, &["config", "user.email", "t@example.com"]);
    run_git(&repo, &["config", "user.name", "Tester"]);
    std::fs::write(repo.join("a.txt"), b"x").unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "init"]);
    run_git(&repo, &["checkout", "-q", "-b", "develop"]);

    let disp = dispatcher(&repo).with_own_branch(Some("fix/mine".to_string()));
    let (resp, fx) = disp
        .dispatch_inner("p", "git_push", &json!({ "force": true }))
        .await;
    assert!(
        !resp.ok,
        "force-pushing a non-own branch must be refused before any push: {resp:?}"
    );
    assert!(
        resp.error.as_deref().unwrap_or_default().contains("force"),
        "the refusal must name the force constraint, got: {:?}",
        resp.error
    );
    assert!(
        fx.is_empty(),
        "a refused force push must emit nothing: {fx:?}"
    );

    let (resp, _fx) = disp.dispatch_inner("p2", "git_push", &Value::Null).await;
    let err = resp.error.as_deref().unwrap_or_default();
    assert!(
        err.contains("push failed") && !err.contains("force is limited"),
        "a non-force push must reach the transport, not the force gate: {err:?}"
    );
}

#[tokio::test]
async fn git_push_force_is_refused_until_the_own_branch_is_recorded() {
    let td = tempfile::tempdir().unwrap();
    let repo = td.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "main"]);
    run_git(&repo, &["config", "user.email", "t@example.com"]);
    run_git(&repo, &["config", "user.name", "Tester"]);
    std::fs::write(repo.join("a.txt"), b"x").unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "init"]);
    run_git(&repo, &["checkout", "-q", "-b", "fix/fresh"]);

    let disp = dispatcher(&repo);
    let (resp, fx) = disp
        .dispatch_inner("p", "git_push", &json!({ "force": true }))
        .await;
    assert!(
        !resp.ok,
        "force must fail closed while no own branch is recorded: {resp:?}"
    );
    assert!(
        resp.error.as_deref().unwrap_or_default().contains("force"),
        "the refusal must name the force constraint, got: {:?}",
        resp.error
    );
    assert!(
        fx.is_empty(),
        "a refused force push must emit nothing: {fx:?}"
    );
}

#[tokio::test]
async fn git_push_force_refuses_when_remote_advanced_unseen() {
    let td = tempfile::tempdir().unwrap();
    let remote = td.path().join("origin.git");
    run_git(
        td.path(),
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );

    let a = td.path().join("a");
    std::fs::create_dir_all(&a).unwrap();
    run_git(&a, &["init", "-q", "-b", "main"]);
    run_git(&a, &["config", "user.email", "a@example.com"]);
    run_git(&a, &["config", "user.name", "A"]);
    run_git(&a, &["remote", "add", "origin", remote.to_str().unwrap()]);
    std::fs::write(a.join("f.txt"), b"1").unwrap();
    run_git(&a, &["add", "-A"]);
    run_git(&a, &["commit", "-q", "-m", "c1"]);
    run_git(&a, &["push", "-u", "-q", "origin", "main"]);

    // Explicit `-b main`: the bare repo's default HEAD follows the host's
    // `init.defaultBranch`.
    let b = td.path().join("b");
    run_git(
        td.path(),
        &[
            "clone",
            "-q",
            "-b",
            "main",
            remote.to_str().unwrap(),
            b.to_str().unwrap(),
        ],
    );
    run_git(&b, &["config", "user.email", "b@example.com"]);
    run_git(&b, &["config", "user.name", "B"]);
    std::fs::write(b.join("g.txt"), b"2").unwrap();
    run_git(&b, &["add", "-A"]);
    run_git(&b, &["commit", "-q", "-m", "c2"]);
    run_git(&b, &["push", "-q", "origin", "main"]);

    run_git(&a, &["commit", "-q", "--amend", "-m", "c1-rewritten"]);

    let rpc_dir = td.path().join("rpc");
    ensure_mailbox(&rpc_dir).unwrap();
    write_request(
        &rpc_dir.join("requests"),
        "p.json",
        r#"{"id":"p","op":"git_push","args":{"force":true}}"#,
    );
    let dispatcher = dispatcher(&a);
    process_pending(&rpc_dir, &dispatcher).await;
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(rpc_dir.join("responses/p.json")).unwrap())
            .unwrap();
    assert_eq!(
        v["ok"], false,
        "force push must be refused when the remote advanced with a commit we never integrated: {v}"
    );

    let remote_head = std::process::Command::new("git")
        .current_dir(&remote)
        .args(["rev-parse", "refs/heads/main"])
        .output()
        .unwrap();
    let b_head = std::process::Command::new("git")
        .current_dir(&b)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&remote_head.stdout).trim(),
        String::from_utf8_lossy(&b_head.stdout).trim(),
        "remote main must be untouched after the refused force push"
    );
}
