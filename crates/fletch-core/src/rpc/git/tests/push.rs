use crate::rpc::git::test_support::*;

use serde_json::{json, Value};

use crate::rpc::git::ledger::{BranchLedger, MemoryLedger, StaticPrIndex};
use crate::rpc::git::EVENT_BRANCH_CREATED;
use crate::rpc::{ensure_mailbox, process_pending, RpcEvent};
use crate::workspace::OwnedBranch;

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

/// A dispatcher over the fixture's checkout with `base` recorded, sharing
/// `ledger` (so what one push records, the next reads) and answering PR
/// questions from `index`.
fn stacked(
    fx: &crate::git::testing::Repos,
    base: &str,
    ledger: &std::sync::Arc<MemoryLedger>,
    index: StaticPrIndex,
) -> crate::rpc::git::GitDispatcher {
    crate::rpc::git::GitDispatcher::new(
        fx.checkout.clone(),
        fx.source.clone(),
        base.to_string(),
        crate::rpc::caps::AgentCaps::interactive(),
    )
    .with_ledger(ledger.clone())
    .with_pr_index(index)
}

fn on_origin(fx: &crate::git::testing::Repos, branch: &str) -> Option<String> {
    let out = crate::git::testing::git_output(
        &fx.origin,
        &[
            "rev-parse",
            "--verify",
            "-q",
            &format!("refs/heads/{branch}"),
        ],
    );
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Create `branch` on origin from the checkout through `git_push`, so the
/// ledger records it as created by this workspace; its tip.
async fn create_owned(
    fx: &crate::git::testing::Repos,
    branch: &str,
    ledger: &std::sync::Arc<MemoryLedger>,
) -> String {
    run_git(&fx.checkout, &["checkout", "-q", "-B", branch]);
    crate::git::testing::commit_file(&fx.checkout, &format!("{}.txt", branch.replace('/', "-")));
    let (resp, _fx) = stacked(fx, "main", ledger, no_open_prs())
        .dispatch_inner("create", "git_push", &Value::Null)
        .await;
    assert!(resp.ok, "{resp:?}");
    let tip = on_origin(fx, branch).expect("created");
    assert!(
        ledger.owned(None).contains(&OwnedBranch {
            name: branch.to_string(),
            tip: Some(tip.clone()),
        }),
        "the push that created it records it with its tip: {:?}",
        ledger.owned(None)
    );
    tip
}

/// The lower branch of the workspace's own stack takes follow-up commits only
/// while every fact holds, each read at the push: the workspace created it, an
/// open PR has it as head (asked of GitHub now, so a merge earlier in the
/// session counts), and only as a plain push.
#[tokio::test]
async fn a_stacked_base_takes_a_plain_push_only_while_everything_holds() {
    let td = tempfile::tempdir().unwrap();
    let fx = crate::git::testing::repos(td.path());
    let ledger = std::sync::Arc::new(MemoryLedger::default());
    let created = create_owned(&fx, "feat/backend", &ledger).await;
    crate::git::testing::commit_file(&fx.checkout, "follow-up.txt");
    let follow_up = crate::git::testing::git(&fx.checkout, &["rev-parse", "HEAD"]);
    let under_review = || StaticPrIndex {
        heads: vec!["feat/backend".into()],
        ..no_open_prs()
    };

    for (disp, why, expect) in [
        (
            stacked(
                &fx,
                "feat/backend",
                &std::sync::Arc::new(MemoryLedger::default()),
                under_review(),
            ),
            "a base this workspace did not create",
            "reviewed against",
        ),
        (
            stacked(&fx, "feat/backend", &ledger, no_open_prs()),
            "a base whose PR merged or closed (no open PR has it as head)",
            "no open pull request",
        ),
        (
            stacked(
                &fx,
                "feat/backend",
                &ledger,
                StaticPrIndex {
                    reachable: false,
                    ..under_review()
                },
            ),
            "GitHub could not answer",
            "could not confirm",
        ),
    ] {
        let (resp, effects) = disp.dispatch_inner("p", "git_push", &Value::Null).await;
        let err = resp.error.as_deref().unwrap_or_default();
        assert!(err.contains(expect), "{why}: {resp:?}");
        assert!(effects.is_empty(), "{why}: {effects:?}");
        assert_eq!(
            on_origin(&fx, "feat/backend").as_deref(),
            Some(created.as_str()),
            "{why}: nothing may reach origin"
        );
    }

    let disp = stacked(&fx, "feat/backend", &ledger, under_review());
    let (resp, _fx) = disp.dispatch_inner("p2", "git_push", &Value::Null).await;
    assert!(
        resp.ok,
        "the lower PR's branch must take the push: {resp:?}"
    );
    assert_eq!(on_origin(&fx, "feat/backend"), Some(follow_up.clone()));
    assert_eq!(
        ledger.owned(None)[0].tip.as_deref(),
        Some(follow_up.as_str()),
        "the push moves the recorded tip"
    );

    run_git(
        &fx.checkout,
        &["commit", "-q", "--amend", "-m", "rewritten"],
    );
    let (resp, effects) = disp
        .dispatch_inner("p3", "git_push", &json!({ "force": true }))
        .await;
    let err = resp.error.as_deref().unwrap_or_default();
    assert!(err.contains("without --force"), "{resp:?}");
    assert!(effects.is_empty(), "{effects:?}");
    assert_eq!(
        on_origin(&fx, "feat/backend"),
        Some(follow_up),
        "origin keeps the commit the stacked PR is based on"
    );
}

/// In a stack A <- B <- C the recorded base is B, yet A is still B's base.
/// Force asks GitHub which open PRs a branch is the base of, so A stays
/// unforceable while B is based on it, whatever the recorded base says.
#[tokio::test]
async fn force_is_refused_on_any_branch_an_open_pr_is_based_on() {
    let td = tempfile::tempdir().unwrap();
    let fx = crate::git::testing::repos(td.path());
    let ledger = std::sync::Arc::new(MemoryLedger::default());
    let created = create_owned(&fx, "feat/a", &ledger).await;
    run_git(&fx.checkout, &["commit", "-q", "--amend", "-m", "rebased"]);

    let b_on_a = StaticPrIndex {
        bases: vec!["feat/a".into()],
        ..no_open_prs()
    };
    let (resp, effects) = stacked(&fx, "feat/b", &ledger, b_on_a)
        .dispatch_inner("p1", "git_push", &json!({ "force": true }))
        .await;
    let err = resp.error.as_deref().unwrap_or_default();
    assert!(err.contains("based on it"), "{resp:?}");
    assert!(effects.is_empty(), "{effects:?}");
    assert_eq!(on_origin(&fx, "feat/a"), Some(created));

    // Nothing based on it any more: its own rewrite goes through.
    let (resp, _fx) = stacked(&fx, "feat/b", &ledger, no_open_prs())
        .dispatch_inner("p2", "git_push", &json!({ "force": true }))
        .await;
    assert!(resp.ok, "{resp:?}");
    let rewritten = crate::git::testing::git(&fx.checkout, &["rev-parse", "HEAD"]);
    assert_eq!(on_origin(&fx, "feat/a"), Some(rewritten.clone()));
    assert_eq!(
        ledger.owned(None)[0].tip.as_deref(),
        Some(rewritten.as_str())
    );
}

/// Ownership is of the branch this workspace made, not of its name: deleted
/// and recreated by someone else, it is refused a force even after the agent
/// fetched and integrated it — and a plain push on top of the recreated branch
/// (allowed, as to any branch) does not adopt it.
#[tokio::test]
async fn a_branch_recreated_by_someone_else_is_not_this_workspaces() {
    let td = tempfile::tempdir().unwrap();
    let fx = crate::git::testing::repos(td.path());
    let ledger = std::sync::Arc::new(MemoryLedger::default());
    let created = create_owned(&fx, "fix/x", &ledger).await;

    // Someone else's fix/x, from main, replaces the one this workspace made.
    run_git(&fx.source, &["checkout", "-q", "-b", "theirs", "main"]);
    crate::git::testing::commit_file(&fx.source, "theirs.txt");
    run_git(&fx.source, &["push", "-q", "-f", "origin", "theirs:fix/x"]);
    let theirs = on_origin(&fx, "fix/x").unwrap();
    assert_ne!(theirs, created);

    // The agent takes theirs and builds on it: integrated, so a lease alone
    // would pass.
    run_git(&fx.checkout, &["fetch", "-q", "origin", "fix/x"]);
    run_git(&fx.checkout, &["reset", "-q", "--hard", "FETCH_HEAD"]);
    crate::git::testing::commit_file(&fx.checkout, "on-theirs.txt");
    let disp = stacked(&fx, "main", &ledger, no_open_prs());
    let (resp, _fx) = disp.dispatch_inner("p1", "git_push", &Value::Null).await;
    assert!(
        resp.ok,
        "a plain push to a non-base branch is allowed: {resp:?}"
    );
    assert_eq!(
        ledger.owned(None)[0].tip.as_deref(),
        Some(created.as_str()),
        "a push that doesn't continue this workspace's branch must not adopt it"
    );
    let pushed = on_origin(&fx, "fix/x").unwrap();

    run_git(
        &fx.checkout,
        &["commit", "-q", "--amend", "-m", "rewritten"],
    );
    let (resp, effects) = disp
        .dispatch_inner("p2", "git_push", &json!({ "force": true }))
        .await;
    let err = resp.error.as_deref().unwrap_or_default();
    assert!(err.contains("recreated"), "{resp:?}");
    assert!(effects.is_empty(), "{effects:?}");
    assert_eq!(on_origin(&fx, "fix/x"), Some(pushed), "origin is untouched");
}

/// Two forces in one session: the second is checked against the tip the
/// first published, read from the ledger at the push, not a copy taken at
/// spawn.
#[tokio::test]
async fn a_second_force_in_one_session_is_checked_against_the_first() {
    let td = tempfile::tempdir().unwrap();
    let fx = crate::git::testing::repos(td.path());
    let ledger = std::sync::Arc::new(MemoryLedger::default());
    create_owned(&fx, "fix/twice", &ledger).await;
    let disp = stacked(&fx, "main", &ledger, no_open_prs());
    for round in ["first", "second"] {
        run_git(&fx.checkout, &["commit", "-q", "--amend", "-m", round]);
        let (resp, _fx) = disp
            .dispatch_inner(round, "git_push", &json!({ "force": true }))
            .await;
        assert!(resp.ok, "{round}: {resp:?}");
        let head = crate::git::testing::git(&fx.checkout, &["rev-parse", "HEAD"]);
        assert_eq!(on_origin(&fx, "fix/twice"), Some(head.clone()), "{round}");
        assert_eq!(ledger.owned(None)[0].tip, Some(head), "{round}");
    }
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
        disp.owned_now().is_empty(),
        "a branch origin already held must not be claimed"
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
            expect: None,
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
    let born = |fx: &[RpcEvent]| {
        fx.iter().any(|e| {
            matches!(
                e,
                RpcEvent::Named { name, payload }
                    if name == EVENT_BRANCH_CREATED && payload["branch"] == "fix/local"
            )
        })
    };
    let head = || crate::git::testing::git(&repo, &["rev-parse", "HEAD"]);
    assert!(
        born(&fx),
        "the push that creates the branch on origin must record it: {fx:?}"
    );
    assert_eq!(
        disp.owned_now(),
        [OwnedBranch {
            name: "fix/local".into(),
            tip: Some(head())
        }],
        "and own it, with the tip it published"
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
    assert_eq!(
        disp.owned_now()[0].tip,
        Some(head()),
        "a push continuing it moves its tip"
    );
}

/// A branch already owned is not probed or re-recorded as the checkout's; one
/// recorded before tips were gets its tip from this push.
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

    let disp = dispatcher(&repo).with_owned_branches(&[("fix/known", None)]);
    let (resp, fx) = disp.dispatch_inner("p", "git_push", &Value::Null).await;
    assert!(resp.ok, "{resp:?}");
    assert!(
        !fx.iter()
            .any(|e| matches!(e, RpcEvent::Named { name, .. } if name == EVENT_BRANCH_CREATED)),
        "{fx:?}"
    );
    assert_eq!(
        disp.owned_now()[0].tip,
        Some(crate::git::testing::git(&repo, &["rev-parse", "HEAD"]))
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
    let dispatcher = dispatcher(&repo)
        .with_owned_branches(&[("fix/diverged", None)])
        .with_pr_index(no_open_prs());

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

    let disp = dispatcher(&repo).with_owned_branches(&[("fix/mine", None)]);
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
/// the user's remote records it as the agent's own, with the tip it
/// published, and a force after a rewrite is allowed against that record.
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
    )
    .with_pr_index(no_open_prs());
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
    let pushed = crate::git::testing::git(&checkout, &["rev-parse", "HEAD"]);
    assert_eq!(
        crate::git::testing::git(&origin, &["rev-parse", "refs/heads/fix/mine"]),
        pushed
    );
    assert_eq!(
        disp.owned_now(),
        [OwnedBranch {
            name: "fix/mine".into(),
            tip: Some(pushed)
        }]
    );

    // Recorded at the push and read at the next one: a rewritten branch can
    // be forced in the same session, with no rebuild in between.
    run_git(&checkout, &["commit", "-q", "--amend", "-m", "rewritten"]);
    let (resp, _fx) = disp
        .dispatch_inner("p2", "git_push", &json!({ "force": true }))
        .await;
    assert!(resp.ok, "{resp:?}");
    assert_eq!(
        crate::git::testing::git(&origin, &["rev-parse", "refs/heads/fix/mine"]),
        crate::git::testing::git(&checkout, &["rev-parse", "HEAD"])
    );
}
