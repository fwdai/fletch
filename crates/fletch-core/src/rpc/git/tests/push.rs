use crate::rpc::git::test_support::*;

use serde_json::{json, Value};

use crate::rpc::git::{EVENT_BRANCH_CREATED, EVENT_BRANCH_OWNED};
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

/// Stacking a PR on a branch this workspace created records that branch as the
/// base. Follow-up commits for the lower PR still go onto it while that PR is
/// open — as a plain push only, never a force, which would rewrite the stacked
/// PR's base. A base the workspace did not create (a teammate's branch whose
/// PR was adopted, say) stays refused, and so does an owned base with no open
/// PR to review the push (never opened, merged or closed).
#[tokio::test]
async fn git_push_updates_an_owned_base_under_review_without_force_only() {
    let td = tempfile::tempdir().unwrap();
    let crate::git::testing::Repos {
        origin,
        source,
        checkout,
        ..
    } = crate::git::testing::repos(td.path());
    run_git(&checkout, &["checkout", "-q", "-b", "feat/backend"]);
    crate::git::testing::commit_file(&checkout, "backend.txt");

    let stacked = |owned: &[&str], open: &[&str]| {
        crate::rpc::git::GitDispatcher::new(
            checkout.clone(),
            source.clone(),
            "feat/backend".to_string(),
            crate::rpc::caps::AgentCaps::interactive(),
        )
        .with_owned_branches(owned)
        .with_open_pr_heads(open)
    };
    let on_origin = || {
        crate::git::testing::git_output(
            &origin,
            &["rev-parse", "--verify", "-q", "refs/heads/feat/backend"],
        )
    };

    for (owned, open, why) in [
        (
            &["feat/ui"][..],
            &["feat/backend"][..],
            "a base this workspace did not create stays refused",
        ),
        (
            &["feat/backend"][..],
            &[][..],
            "an owned base with no open PR stays refused",
        ),
    ] {
        let (resp, fx) = stacked(owned, open)
            .dispatch_inner("p1", "git_push", &Value::Null)
            .await;
        assert!(
            resp.error
                .as_deref()
                .unwrap_or_default()
                .contains("refusing to publish"),
            "{why}: {resp:?}"
        );
        assert!(fx.is_empty(), "{why}: {fx:?}");
        assert!(
            !on_origin().status.success(),
            "{why}: nothing may reach origin"
        );
    }

    let (resp, _fx) = stacked(&["feat/backend"], &["feat/backend"])
        .dispatch_inner("p2", "git_push", &Value::Null)
        .await;
    assert!(
        resp.ok,
        "the lower PR's branch must take the push: {resp:?}"
    );
    let pushed = crate::git::testing::git(&checkout, &["rev-parse", "HEAD"]);
    assert_eq!(String::from_utf8_lossy(&on_origin().stdout).trim(), pushed);

    // The owned base itself is the anchor a force used to pass on.
    run_git(&checkout, &["commit", "-q", "--amend", "-m", "rewritten"]);
    let (resp, fx) = stacked(&["feat/backend"], &["feat/backend"])
        .dispatch_inner("p3", "git_push", &json!({ "force": true }))
        .await;
    assert!(!resp.ok, "force on a stacked PR's base must be refused");
    assert!(
        resp.error
            .as_deref()
            .unwrap_or_default()
            .contains("without --force"),
        "{resp:?}"
    );
    assert!(fx.is_empty(), "{fx:?}");
    assert_eq!(
        String::from_utf8_lossy(&on_origin().stdout).trim(),
        pushed,
        "origin keeps the commit the stacked PR is based on"
    );
}

/// A branch is owned only if this workspace's push created it on origin. A
/// name the host materializes is checked against the checkout's
/// remote-tracking refs, which the agent can delete; origin is asked anyway,
/// and a branch it already held is pushed (here, fast-forwarded) but never
/// claimed.
#[tokio::test]
async fn a_branch_that_already_existed_on_origin_is_never_claimed() {
    let td = tempfile::tempdir().unwrap();
    let crate::git::testing::Repos {
        origin,
        source,
        checkout,
        ..
    } = crate::git::testing::repos(td.path());
    run_git(&source, &["push", "-q", "origin", "main:teammate/wip"]);
    run_git(&checkout, &["checkout", "-q", "--detach"]);
    crate::git::testing::commit_file(&checkout, "mine.txt");

    let disp = crate::rpc::git::GitDispatcher::new(
        checkout.clone(),
        source.clone(),
        "main".to_string(),
        crate::rpc::caps::AgentCaps::interactive(),
    );
    let (resp, fx) = disp
        .dispatch_inner("p1", "git_push", &json!({ "branch": "teammate/wip" }))
        .await;
    assert!(resp.ok, "{resp:?}");
    assert_eq!(
        crate::git::testing::git(&origin, &["rev-parse", "refs/heads/teammate/wip"]),
        crate::git::testing::git(&checkout, &["rev-parse", "HEAD"])
    );
    assert!(
        !fx.iter()
            .any(|e| matches!(e, RpcEvent::Named { name, .. } if name == EVENT_BRANCH_OWNED)),
        "a branch origin already held must not be claimed: {fx:?}"
    );
    assert!(
        fx.iter().any(|e| matches!(
            e,
            RpcEvent::Named { name, payload }
                if name == EVENT_BRANCH_CREATED && payload["branch"] == "teammate/wip"
        )),
        "the checkout is still on it: {fx:?}"
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
    assert!(
        effects.iter().any(|e| matches!(
            e,
            RpcEvent::Named { name, payload }
                if name == EVENT_BRANCH_CREATED && payload["branch"] == "fix/thing"
        )),
        "expected a branch-created event, got: {effects:?}"
    );
}

/// A branch the agent made with plain `git checkout -b` becomes its own the
/// moment its push puts it on origin: the events the host records the branch
/// and its ownership from are emitted then, and not again on the next push.
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
    let emitted = |fx: &[RpcEvent], event: &str| {
        fx.iter().any(|e| {
            matches!(
                e,
                RpcEvent::Named { name, payload }
                    if name == event && payload["branch"] == "fix/local"
            )
        })
    };
    let born =
        |fx: &[RpcEvent]| emitted(fx, EVENT_BRANCH_CREATED) || emitted(fx, EVENT_BRANCH_OWNED);
    assert!(
        emitted(&fx, EVENT_BRANCH_CREATED) && emitted(&fx, EVENT_BRANCH_OWNED),
        "the push that creates the branch on origin must record it and its ownership: {fx:?}"
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

/// A branch already owned is not probed or re-recorded.
#[tokio::test]
async fn git_push_of_an_owned_branch_emits_no_branch_event() {
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

    let disp = dispatcher(&repo).with_owned_branches(&["fix/known"]);
    let (resp, fx) = disp.dispatch_inner("p", "git_push", &Value::Null).await;
    assert!(resp.ok, "{resp:?}");
    assert!(
        !fx.iter().any(|e| matches!(
            e,
            RpcEvent::Named { name, .. }
                if name == EVENT_BRANCH_CREATED || name == EVENT_BRANCH_OWNED
        )),
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
    let dispatcher = dispatcher(&repo).with_owned_branches(&["fix/diverged"]);

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

    let disp = dispatcher(&repo).with_owned_branches(&["fix/mine"]);
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
    for event in [EVENT_BRANCH_CREATED, EVENT_BRANCH_OWNED] {
        assert!(
            fx.iter().any(|e| matches!(
                e,
                RpcEvent::Named { name, payload }
                    if name == event && payload["branch"] == "fix/mine"
            )),
            "{event}: {fx:?}"
        );
    }
    assert_eq!(
        crate::git::testing::git(&origin, &["rev-parse", "refs/heads/fix/mine"]),
        crate::git::testing::git(&checkout, &["rev-parse", "HEAD"])
    );

    // The host recorded the branch as owned; the next dispatcher carries it,
    // and a rewritten branch can be forced.
    run_git(&checkout, &["commit", "-q", "--amend", "-m", "rewritten"]);
    let disp = disp.with_owned_branches(&["fix/mine"]);
    let (resp, _fx) = disp
        .dispatch_inner("p2", "git_push", &json!({ "force": true }))
        .await;
    assert!(resp.ok, "{resp:?}");
    assert_eq!(
        crate::git::testing::git(&origin, &["rev-parse", "refs/heads/fix/mine"]),
        crate::git::testing::git(&checkout, &["rev-parse", "HEAD"])
    );
}
