use super::*;
use crate::rpc::git::test_support::*;

use serde_json::json;

use crate::rpc::caps::AgentCaps;
use crate::rpc::git::GitDispatcher;

#[tokio::test]
async fn open_pr_refuses_option_like_base() {
    let td = tempfile::tempdir().unwrap();
    let repo = td.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "main"]);

    let disp = dispatcher(&repo);
    let (resp, fx) = disp
        .dispatch_inner(
            "b1",
            "open_pr",
            &json!({"title": "t", "base": "--upload-pack=evil"}),
        )
        .await;
    assert!(!resp.ok, "an option-named base must be refused: {resp:?}");
    assert!(
        resp.error
            .as_deref()
            .unwrap_or_default()
            .contains("option-like base"),
        "error must name the refused base, got: {:?}",
        resp.error
    );
    assert!(fx.is_empty(), "a refused PR must emit nothing: {fx:?}");
}

#[tokio::test]
async fn open_pr_refuses_a_base_missing_from_origin() {
    let td = tempfile::tempdir().unwrap();
    let origin = td.path().join("origin.git");
    std::fs::create_dir_all(&origin).unwrap();
    run_git(&origin, &["init", "-q", "--bare", "-b", "main"]);

    let repo = td.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "main"]);
    run_git(&repo, &["config", "user.email", "t@example.com"]);
    run_git(&repo, &["config", "user.name", "Tester"]);
    run_git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    std::fs::write(repo.join("a.txt"), b"x").unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "init"]);
    run_git(&repo, &["push", "-q", "origin", "main"]);

    let disp = dispatcher(&repo);
    let (resp, fx) = disp
        .dispatch_inner(
            "b3",
            "open_pr",
            &json!({"title": "t", "branch": "fix/typo-base", "base": "mainn"}),
        )
        .await;
    assert!(
        !resp.ok,
        "a base origin does not have must be refused: {resp:?}"
    );
    assert!(
        resp.error
            .as_deref()
            .unwrap_or_default()
            .contains("does not exist on origin"),
        "error must name the missing base, got: {:?}",
        resp.error
    );
    assert!(
        fx.is_empty(),
        "nothing may be created before the base is known good: {fx:?}"
    );
    let branches = std::process::Command::new("git")
        .current_dir(&repo)
        .args(["branch", "--list", "fix/typo-base"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&branches.stdout).trim().is_empty(),
        "the head branch must not be materialized for a bad base"
    );
}

/// `ls-remote`'s ref argument is a pattern: a glob base matches a real branch
/// and exits 0, but GitHub rejects it as a literal base.
#[tokio::test]
async fn open_pr_refuses_a_glob_base_that_matches_a_real_branch() {
    let td = tempfile::tempdir().unwrap();
    let origin = td.path().join("origin.git");
    std::fs::create_dir_all(&origin).unwrap();
    run_git(&origin, &["init", "-q", "--bare", "-b", "main"]);

    let repo = td.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "main"]);
    run_git(&repo, &["config", "user.email", "t@example.com"]);
    run_git(&repo, &["config", "user.name", "Tester"]);
    run_git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    std::fs::write(repo.join("a.txt"), b"x").unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "init"]);
    run_git(&repo, &["push", "-q", "origin", "main"]);
    run_git(&repo, &["push", "-q", "origin", "main:refs/heads/feat/x"]);

    let disp = dispatcher(&repo);
    let (resp, fx) = disp
        .dispatch_inner(
            "b5",
            "open_pr",
            &json!({"title": "t", "branch": "fix/glob-base", "base": "feat/*"}),
        )
        .await;
    assert!(
        !resp.ok,
        "a glob base must be refused even though ls-remote matches it: {resp:?}"
    );
    assert!(
        resp.error
            .as_deref()
            .unwrap_or_default()
            .contains("does not exist on origin"),
        "error must name the missing base, got: {:?}",
        resp.error
    );
    assert!(fx.is_empty(), "nothing may be created: {fx:?}");
}

#[tokio::test]
async fn open_pr_does_not_refuse_a_base_it_could_not_verify() {
    let td = tempfile::tempdir().unwrap();
    let repo = td.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "main"]);
    run_git(&repo, &["config", "user.email", "t@example.com"]);
    run_git(&repo, &["config", "user.name", "Tester"]);
    std::fs::write(repo.join("a.txt"), b"x").unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "init"]);
    run_git(&repo, &["checkout", "-q", "-b", "fix/unverifiable"]);

    let disp = dispatcher(&repo);
    let (resp, _fx) = disp
        .dispatch_inner("b4", "open_pr", &json!({"title": "t", "base": "feat/x"}))
        .await;
    assert!(!resp.ok);
    let err = resp.error.as_deref().unwrap_or_default();
    assert!(
        !err.contains("does not exist on origin"),
        "an unanswerable probe must not be reported as a missing base: {err}"
    );
    assert!(err.contains("push failed"), "got: {err}");
}

#[tokio::test]
async fn open_pr_base_override_does_not_unlock_the_recorded_base() {
    let td = tempfile::tempdir().unwrap();
    let repo = td.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "release/2.0"]);

    let disp = GitDispatcher::new(
        repo.clone(),
        "release/2.0".to_string(),
        AgentCaps::interactive(),
    );
    let (resp, fx) = disp
        .dispatch_inner("b2", "open_pr", &json!({"title": "t", "base": "feat/x"}))
        .await;
    assert!(
        !resp.ok,
        "publishing the review base must stay refused whatever base the PR names: {resp:?}"
    );
    assert!(
        resp.error
            .as_deref()
            .unwrap_or_default()
            .contains("reviewed against"),
        "error must be the review-base refusal, got: {:?}",
        resp.error
    );
    assert!(fx.is_empty(), "a refused PR must emit nothing: {fx:?}");
}

#[test]
fn fallback_branch_slugifies_title() {
    assert_eq!(
        fallback_branch("Fix the Login Crash!"),
        "chore/fix-the-login-crash"
    );
    assert_eq!(
        fallback_branch("  Add   CSV export  "),
        "chore/add-csv-export"
    );
    assert_eq!(fallback_branch(""), "chore/update");
    assert_eq!(fallback_branch("!!!"), "chore/update");
}
