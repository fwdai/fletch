use crate::rpc::git::test_support::*;

use serde_json::{json, Value};

use crate::rpc::caps::AgentCaps;
use crate::rpc::git::{GitDispatcher, EVENT_BRANCH_CREATED};
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
    assert!(err.contains("no origin remote"), "got: {err}");
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
async fn git_push_refuses_commits_made_as_someone_else() {
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
    run_git(&repo, &["checkout", "-q", "-b", "fix/identity"]);
    std::fs::write(repo.join("b.txt"), b"y").unwrap();
    run_git(&repo, &["add", "-A"]);
    // The mistake under test: an overridden committer email.
    run_git(
        &repo,
        &[
            "-c",
            "user.email=agent@example.com",
            "commit",
            "-q",
            "-m",
            "as the agent",
        ],
    );

    let rpc_dir = td.path().join("rpc");
    ensure_mailbox(&rpc_dir).unwrap();
    let requests = rpc_dir.join("requests");
    let disp = dispatcher(&repo);

    write_request(&requests, "p1.json", r#"{"id":"p1","op":"git_push"}"#);
    process_pending(&rpc_dir, &disp).await;
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(rpc_dir.join("responses/p1.json")).unwrap())
            .unwrap();
    assert_eq!(v["ok"], false, "a foreign committer must be refused: {v}");
    let err = v["error"].as_str().unwrap();
    assert!(
        err.contains("agent@example.com"),
        "names the stray email: {err}"
    );
    assert!(
        err.contains("t@example.com"),
        "names the configured one: {err}"
    );
    let refs = std::process::Command::new("git")
        .args(["--git-dir", remote.to_str().unwrap(), "for-each-ref"])
        .output()
        .expect("git");
    assert!(
        String::from_utf8_lossy(&refs.stdout).trim().is_empty(),
        "nothing may reach the remote"
    );

    // Rewritten as the checkout's identity, the push goes through.
    run_git(&repo, &["commit", "-q", "--amend", "--no-edit"]);
    write_request(&requests, "p2.json", r#"{"id":"p2","op":"git_push"}"#);
    process_pending(&rpc_dir, &disp).await;
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(rpc_dir.join("responses/p2.json")).unwrap())
            .unwrap();
    assert_eq!(v["ok"], true, "the rewritten branch must push: {v}");
}

#[tokio::test]
async fn push_commit_publishes_the_checked_commit_not_the_moved_branch() {
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
    run_git(&repo, &["checkout", "-q", "-b", "fix/pinned"]);
    let rev = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(&repo)
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    // What a check read: the tip now. Then the branch moves, as it could while
    // a publish prompt is open — here as someone else would, with another
    // committer email.
    let checked = rev(&["rev-parse", "HEAD"]);
    std::fs::write(repo.join("b.txt"), b"y").unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(
        &repo,
        &[
            "-c",
            "user.email=late@example.com",
            "commit",
            "-q",
            "-m",
            "late",
        ],
    );

    unsigned(&repo);
    let summary = crate::git::publish(
        &crate::git::Publication {
            checkout: &repo,
            source_repo: &repo,
            branch: "fix/pinned",
            force: false,
        },
        crate::git::Tip::Commit(&checked),
    )
    .await
    .expect("push")
    .summary;
    assert_eq!(summary, "pushed");
    let remote_tip = std::process::Command::new("git")
        .args([
            "--git-dir",
            remote.to_str().unwrap(),
            "rev-parse",
            "refs/heads/fix/pinned",
        ])
        .output()
        .expect("git");
    assert_eq!(
        String::from_utf8_lossy(&remote_tip.stdout).trim(),
        checked,
        "origin must hold the checked commit, not the late one"
    );
    // The tracking ref is set, as `push -u` would have set it.
    assert_eq!(
        rev(&["rev-parse", "--abbrev-ref", "fix/pinned@{upstream}"]),
        "origin/fix/pinned"
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
    // No remote, so the push failed: the branch exists locally but origin
    // never got it, and a branch origin doesn't have is nobody's to force.
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, RpcEvent::Named { name, .. } if name == EVENT_BRANCH_CREATED)),
        "a failed push must not record the branch as the workspace's own: {effects:?}"
    );
}

/// The host records a branch as the workspace's own only when its push
/// created it on origin: a name origin already had is not claimed even when
/// the push lands on it, and a rejected push claims nothing. Without that, a
/// failed first push would authorize force against whatever later appears
/// under the name.
#[tokio::test]
async fn git_push_records_a_branch_only_when_its_push_created_it_on_origin() {
    let td = tempfile::tempdir().unwrap();
    let remote = td.path().join("origin.git");
    // `-b main`, so the clone below checks `main` out rather than an unborn
    // default branch.
    run_git(
        td.path(),
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            remote.to_str().unwrap(),
        ],
    );
    // Someone else's clone seeds origin: `main`, a branch `fix/taken` at the
    // same commit (which the agent's push below fast-forwards), and a branch
    // `fix/diverged` with its own history (which the agent's push cannot).
    let other = td.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    run_git(&other, &["init", "-q", "-b", "main"]);
    run_git(&other, &["config", "user.email", "o@example.com"]);
    run_git(&other, &["config", "user.name", "Other"]);
    run_git(
        &other,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    std::fs::write(other.join("a.txt"), b"x").unwrap();
    run_git(&other, &["add", "-A"]);
    run_git(&other, &["commit", "-q", "-m", "init"]);
    run_git(&other, &["push", "-q", "origin", "main", "main:fix/taken"]);
    run_git(&other, &["checkout", "-q", "-b", "fix/diverged"]);
    std::fs::write(other.join("theirs.txt"), b"t").unwrap();
    run_git(&other, &["add", "-A"]);
    run_git(&other, &["commit", "-q", "-m", "theirs"]);
    run_git(&other, &["push", "-q", "origin", "fix/diverged"]);

    let repo = td.path().join("repo");
    run_git(
        td.path(),
        &[
            "clone",
            "-q",
            remote.to_str().unwrap(),
            repo.to_str().unwrap(),
        ],
    );
    run_git(&repo, &["config", "user.email", "t@example.com"]);
    run_git(&repo, &["config", "user.name", "Tester"]);
    std::fs::write(repo.join("b.txt"), b"y").unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "mine"]);
    // The agent names its branches itself, with plain `git checkout -b`
    // (`args.branch` would pick a collision-free name instead of colliding).
    let disp = dispatcher(&repo);
    let born = |fx: &[RpcEvent]| {
        fx.iter()
            .any(|e| matches!(e, RpcEvent::Named { name, .. } if name == EVENT_BRANCH_CREATED))
    };

    // Rejected: origin's fix/diverged is not an ancestor of the agent's tip.
    run_git(&repo, &["checkout", "-q", "-b", "fix/diverged"]);
    let (resp, fx) = disp.dispatch_inner("p1", "git_push", &Value::Null).await;
    assert!(!resp.ok, "a non-fast-forward push is rejected: {resp:?}");
    assert!(!born(&fx), "a rejected push records nothing: {fx:?}");

    // Accepted, but onto a name origin already had: not this workspace's.
    run_git(&repo, &["checkout", "-q", "-b", "fix/taken"]);
    let (resp, fx) = disp.dispatch_inner("p2", "git_push", &Value::Null).await;
    assert!(
        resp.ok,
        "a fast-forward onto an existing branch lands: {resp:?}"
    );
    assert!(
        !born(&fx),
        "a branch origin already had is not claimed: {fx:?}"
    );

    // A name origin did not have, created by this push: the workspace's own,
    // and the branch the checkout is on.
    run_git(&repo, &["checkout", "-q", "-b", "fix/fresh"]);
    let (resp, fx) = disp.dispatch_inner("p3", "git_push", &Value::Null).await;
    assert!(resp.ok, "{resp:?}");
    assert!(
        fx.iter().any(|e| matches!(
            e,
            RpcEvent::Named { name, payload }
                if name == EVENT_BRANCH_CREATED
                    && payload["branch"] == "fix/fresh"
                    && payload["current"] == true
        )),
        "the push that created the branch records it as own and current: {fx:?}"
    );
}

/// A branch the agent made with plain `git checkout -b` becomes its own the
/// moment its push puts it on origin: the event the host records the branch
/// from is emitted then, and not again on the next push.
#[tokio::test]
async fn git_push_of_a_locally_created_branch_records_it_as_own_once() {
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
    run_git(&repo, &["checkout", "-q", "-b", "fix/local"]);

    let disp = dispatcher(&repo);
    let (resp, fx) = disp.dispatch_inner("p1", "git_push", &Value::Null).await;
    assert!(resp.ok, "{resp:?}");
    let born = |fx: &[RpcEvent]| {
        fx.iter().any(|e| {
            matches!(
                e,
                RpcEvent::Named { name, payload }
                    if name == EVENT_BRANCH_CREATED && payload["branch"] == "fix/local"
            )
        })
    };
    assert!(
        born(&fx),
        "the push that creates the branch on origin must record it: {fx:?}"
    );

    std::fs::write(repo.join("b.txt"), b"y").unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "more"]);
    let (resp, fx) = disp.dispatch_inner("p2", "git_push", &Value::Null).await;
    assert!(resp.ok, "{resp:?}");
    assert!(
        !born(&fx),
        "a branch already on origin is not born again: {fx:?}"
    );
}

/// A branch the host already recorded is not probed or re-recorded.
#[tokio::test]
async fn git_push_of_the_recorded_own_branch_emits_no_branch_event() {
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
    run_git(&repo, &["checkout", "-q", "-b", "fix/known"]);

    let disp = dispatcher(&repo).with_owned(&["fix/known"]);
    let (resp, fx) = disp.dispatch_inner("p", "git_push", &Value::Null).await;
    assert!(resp.ok, "{resp:?}");
    assert!(
        !fx.iter()
            .any(|e| matches!(e, RpcEvent::Named { name, .. } if name == EVENT_BRANCH_CREATED)),
        "{fx:?}"
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
    let dispatcher = dispatcher(&repo).with_owned(&["fix/diverged"]);

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

/// A bare origin with `main` pushed, and the checkout on `branch`, pushed too.
/// The stack scenario's starting point: the branch exists on origin and is
/// recorded as the workspace's own.
fn stacked_fixture(td: &std::path::Path, branch: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let remote = td.join("origin.git");
    run_git(td, &["init", "-q", "--bare", remote.to_str().unwrap()]);
    let repo = td.join("repo");
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
    run_git(&repo, &["push", "-q", "origin", "main"]);
    run_git(&repo, &["checkout", "-q", "-b", branch]);
    std::fs::write(repo.join("b.txt"), b"y").unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "work"]);
    run_git(&repo, &["push", "-q", "origin", branch]);
    (repo, remote)
}

fn remote_tip(remote: &std::path::Path, branch: &str) -> String {
    let out = std::process::Command::new("git")
        .args([
            "--git-dir",
            remote.to_str().unwrap(),
            "rev-parse",
            &format!("refs/heads/{branch}"),
        ])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn local_head(repo: &std::path::Path) -> String {
    let out = std::process::Command::new("git")
        .current_dir(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The workspace opened PR A→main from `feat/a`, then PR B→A from `feat/b`:
/// the record's base is now `feat/a`, a branch the workspace created. A
/// follow-up to PR A goes to `feat/a` with a plain push, and a rewrite of it
/// with a force — the branch is the workspace's own whatever is stacked on it.
#[tokio::test]
async fn git_push_to_the_lower_branch_of_the_workspaces_own_stack_is_allowed() {
    let td = tempfile::tempdir().unwrap();
    let (repo, remote) = stacked_fixture(td.path(), "feat/a");
    let disp = dispatcher(&repo)
        .with_base("feat/a")
        .with_owned(&["feat/a", "feat/b"]);

    std::fs::write(repo.join("c.txt"), b"z").unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "follow-up"]);
    let (resp, _) = disp.dispatch_inner("p1", "git_push", &Value::Null).await;
    assert!(resp.ok, "a follow-up to the lower PR must land: {resp:?}");
    assert_eq!(remote_tip(&remote, "feat/a"), local_head(&repo));

    run_git(
        &repo,
        &["commit", "-q", "--amend", "-m", "follow-up, rewritten"],
    );
    let (resp, _) = disp
        .dispatch_inner("p2", "git_push", &json!({ "force": true }))
        .await;
    assert!(
        resp.ok,
        "the workspace may rewrite a branch it created: {resp:?}"
    );
    assert_eq!(remote_tip(&remote, "feat/a"), local_head(&repo));
}

/// The same stack, in a workspace spawned from a branch it did not create: the
/// spawn base stays off limits even though the record's base has moved onto
/// the stack, and so does a stacked base the workspace did not create.
#[tokio::test]
async fn git_push_refuses_the_bases_the_workspace_did_not_create() {
    let td = tempfile::tempdir().unwrap();
    let (repo, remote) = stacked_fixture(td.path(), "release/24");
    let before = remote_tip(&remote, "release/24");
    std::fs::write(repo.join("c.txt"), b"z").unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "sneaky"]);

    // Spawned from release/24; its own PR A→release/24 from feat/a, PR B→A.
    let disp = GitDispatcher::new(
        repo.clone(),
        repo.clone(),
        "release/24".to_string(),
        AgentCaps::interactive(),
    )
    .with_base("feat/a")
    .with_owned(&["feat/a", "feat/b"]);
    let (resp, fx) = disp.dispatch_inner("p1", "git_push", &Value::Null).await;
    assert!(!resp.ok, "the spawn base is still protected: {resp:?}");
    assert!(resp
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("reviewed against"));
    assert!(fx.is_empty());
    assert_eq!(remote_tip(&remote, "release/24"), before);

    // A stacked base the workspace did not create (the user pointed its PR at
    // a teammate's branch) is protected too.
    run_git(&repo, &["checkout", "-q", "-b", "teammate/wip"]);
    let disp = GitDispatcher::new(
        repo.clone(),
        repo.clone(),
        "main".to_string(),
        AgentCaps::interactive(),
    )
    .with_base("teammate/wip")
    .with_owned(&["feat/a"]);
    let (resp, _) = disp.dispatch_inner("p2", "git_push", &Value::Null).await;
    assert!(!resp.ok, "{resp:?}");
    assert!(resp
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("reviewed against"));
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

    let disp = dispatcher(&repo).with_owned(&["fix/mine"]);
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
        err.contains("no origin remote") && !err.contains("force is limited"),
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

/// Across a real checkout split, the push that puts an agent-made branch on
/// the user's remote records it as the agent's own, and a force after a
/// rewrite is then allowed once the dispatcher carries that record.
#[tokio::test]
async fn an_agent_made_branch_becomes_its_own_across_a_real_checkout_split() {
    let td = tempfile::tempdir().unwrap();
    let crate::git::testing::Repos {
        origin,
        source,
        checkout,
        ..
    } = crate::git::testing::repos(td.path());
    run_git(&checkout, &["checkout", "-q", "-b", "fix/mine"]);
    crate::git::testing::commit_file(&checkout, "mine.txt");

    let disp = crate::rpc::git::GitDispatcher::new(
        checkout.clone(),
        source.clone(),
        "main".to_string(),
        crate::rpc::caps::AgentCaps::interactive(),
    );
    let (resp, fx) = disp.dispatch_inner("p1", "git_push", &Value::Null).await;
    assert!(resp.ok, "{resp:?}");
    assert!(
        fx.iter().any(|e| matches!(
            e,
            RpcEvent::Named { name, payload }
                if name == EVENT_BRANCH_CREATED && payload["branch"] == "fix/mine"
        )),
        "{fx:?}"
    );
    assert_eq!(
        crate::git::testing::git(&origin, &["rev-parse", "refs/heads/fix/mine"]),
        crate::git::testing::git(&checkout, &["rev-parse", "HEAD"])
    );

    // The host recorded the branch; the next dispatcher carries it, and a
    // rewritten branch can be forced.
    run_git(&checkout, &["commit", "-q", "--amend", "-m", "rewritten"]);
    let disp = disp.with_owned(&["fix/mine"]);
    let (resp, _fx) = disp
        .dispatch_inner("p2", "git_push", &json!({ "force": true }))
        .await;
    assert!(resp.ok, "{resp:?}");
    assert_eq!(
        crate::git::testing::git(&origin, &["rev-parse", "refs/heads/fix/mine"]),
        crate::git::testing::git(&checkout, &["rev-parse", "HEAD"])
    );
}
