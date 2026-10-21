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
    let crate::git::testing::Repos {
        origin,
        source,
        checkout,
        ..
    } = crate::git::testing::repos(td.path());

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

/// One fixture for the remote-ref ops: a bare origin with `main` and a
/// `feat/one` branch, and a clone that has fetched neither since.
fn origin_with_two_branches(root: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let origin = root.join("origin.git");
    std::fs::create_dir_all(&origin).unwrap();
    run_git(&origin, &["init", "-q", "--bare", "-b", "main"]);
    let seed = root.join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    run_git(&seed, &["init", "-q", "-b", "main"]);
    run_git(&seed, &["config", "user.email", "t@example.com"]);
    run_git(&seed, &["config", "user.name", "Tester"]);
    run_git(
        &seed,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    std::fs::write(seed.join("a.txt"), b"x").unwrap();
    run_git(&seed, &["add", "-A"]);
    run_git(&seed, &["commit", "-q", "-m", "init"]);
    run_git(&seed, &["push", "-q", "origin", "main"]);
    run_git(&seed, &["checkout", "-q", "-b", "feat/one"]);
    std::fs::write(seed.join("b.txt"), b"y").unwrap();
    run_git(&seed, &["add", "-A"]);
    run_git(&seed, &["commit", "-q", "-m", "one"]);
    run_git(&seed, &["push", "-q", "origin", "feat/one"]);
    // A pull-request-style ref, as GitHub publishes them.
    run_git(
        &seed,
        &["push", "-q", "origin", "feat/one:refs/pull/7/head"],
    );

    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "main"]);
    run_git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    (origin, repo)
}

fn rev_parse(repo: &std::path::Path, rev: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .current_dir(repo)
        .args(["rev-parse", "--verify", "--quiet", rev])
        .output()
        .unwrap();
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[tokio::test]
async fn git_fetch_takes_several_refs_and_a_pull_request_head() {
    let td = tempfile::tempdir().unwrap();
    let (_origin, repo) = origin_with_two_branches(td.path());
    let disp = dispatcher(&repo);
    let (resp, fx) = disp
        .dispatch_inner(
            "f",
            "git_fetch",
            &json!({ "refs": ["main", "feat/one", "pull/7/head"] }),
        )
        .await;
    assert!(resp.ok, "{resp:?}");
    assert_eq!(resp.exit_code, Some(0));
    assert!(fx.is_empty(), "a fetch is not an action: {fx:?}");
    assert!(rev_parse(&repo, "origin/main").is_some());
    let one = rev_parse(&repo, "origin/feat/one").expect("feat/one fetched");
    assert_eq!(
        rev_parse(&repo, "origin/pr/7").as_deref(),
        Some(one.as_str()),
        "the PR head lands under origin/pr/<n>"
    );
}

#[tokio::test]
async fn git_fetch_names_the_refs_that_failed() {
    let td = tempfile::tempdir().unwrap();
    let (_origin, repo) = origin_with_two_branches(td.path());
    let (resp, _fx) = dispatcher(&repo)
        .dispatch_inner("f", "git_fetch", &json!({ "refs": ["main", "feat/nope"] }))
        .await;
    assert!(!resp.ok);
    let err = resp.error.unwrap();
    assert!(err.contains("fetch origin main feat/nope failed"), "{err}");
}

#[tokio::test]
async fn git_fetch_rejects_a_refs_value_that_is_not_a_list_of_names() {
    let td = tempfile::tempdir().unwrap();
    let disp = dispatcher(td.path());
    for args in [json!({ "refs": "main" }), json!({ "refs": [1, 2] })] {
        let (resp, _fx) = disp.dispatch_inner("f", "git_fetch", &args).await;
        assert!(!resp.ok, "{args}");
        assert!(
            resp.error.unwrap().contains("`refs` must be an array"),
            "{args}"
        );
    }
}

#[test]
fn requested_refs_name_branches_or_pull_request_heads() {
    use crate::git::Branches;
    use crate::rpc::git::commands::requested_branch;
    assert_eq!(requested_branch("main"), Branches::One("main"));
    assert_eq!(requested_branch("feat/x"), Branches::One("feat/x"));
    assert_eq!(requested_branch("pull/12/head"), Branches::PullRequest(12));
    assert_eq!(
        requested_branch("refs/pull/12/head"),
        Branches::PullRequest(12)
    );
    assert_eq!(
        requested_branch("pull/abc/head"),
        Branches::One("pull/abc/head"),
        "only a numeric PR maps"
    );
}

#[test]
fn requested_refs_merge_ref_and_refs_and_drop_blanks() {
    use crate::rpc::git::commands::requested_refs;
    assert_eq!(requested_refs(&json!({})).unwrap(), Vec::<String>::new());
    assert_eq!(
        requested_refs(&json!({ "ref": " main ", "refs": ["", "feat/x", "feat/x"] })).unwrap(),
        vec!["main".to_string(), "feat/x".to_string()]
    );
}

#[tokio::test]
async fn git_ls_remote_lists_origin_heads_under_a_prefix() {
    let td = tempfile::tempdir().unwrap();
    let (_origin, repo) = origin_with_two_branches(td.path());
    let disp = dispatcher(&repo);
    let (resp, fx) = disp
        .dispatch_inner("l", "git_ls_remote", &Value::Null)
        .await;
    assert!(resp.ok, "{resp:?}");
    assert!(fx.is_empty());
    let heads: Vec<Value> = serde_json::from_str(resp.stdout.as_deref().unwrap()).unwrap();
    let names: Vec<&str> = heads
        .iter()
        .map(|h| h["branch"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["feat/one", "main"]);
    assert_eq!(heads[0]["sha"].as_str().unwrap().len(), 40);

    let (resp, _fx) = disp
        .dispatch_inner("l", "git_ls_remote", &json!({ "prefix": "feat/" }))
        .await;
    let heads: Vec<Value> = serde_json::from_str(resp.stdout.as_deref().unwrap()).unwrap();
    assert_eq!(heads.len(), 1);
    assert_eq!(heads[0]["branch"], "feat/one");
}

#[tokio::test]
async fn git_ls_remote_without_remote_reports_error() {
    let td = tempfile::tempdir().unwrap();
    let repo = td.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "main"]);
    let (resp, _fx) = dispatcher(&repo)
        .dispatch_inner("l", "git_ls_remote", &Value::Null)
        .await;
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("no origin remote"));
}

#[test]
fn remote_heads_keep_those_under_the_prefix() {
    use crate::rpc::git::commands::{remote_heads, RemoteHead};
    let heads = vec![
        ("main".to_string(), "aaaa".to_string()),
        ("feat/x".to_string(), "bbbb".to_string()),
    ];
    assert_eq!(remote_heads(heads.clone(), None).len(), 2);
    assert_eq!(
        remote_heads(heads, Some("feat/")),
        vec![RemoteHead {
            branch: "feat/x".into(),
            sha: "bbbb".into()
        }]
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

/// The mode #934 is about: the checkout and the user's repository are two
/// repositories, and the transport runs in the user's over the checkout's
/// objects. A multi-ref fetch with a pull request's head lands each under
/// its tracking name in the checkout, and the listing reads the user's
/// remote — never the checkout's origin.
#[tokio::test]
async fn fetch_and_ls_remote_work_across_a_real_checkout_split() {
    let td = tempfile::tempdir().unwrap();
    let crate::git::testing::Repos {
        origin,
        source,
        checkout,
        ..
    } = crate::git::testing::repos(td.path());
    // A second branch and a PR head, published from the user's repository.
    run_git(&source, &["checkout", "-q", "-b", "feat/one"]);
    let tip = crate::git::testing::commit_file(&source, "one.txt");
    run_git(&source, &["push", "-q", "origin", "feat/one"]);
    run_git(
        &source,
        &["push", "-q", "origin", "feat/one:refs/pull/7/head"],
    );
    run_git(&source, &["checkout", "-q", "main"]);

    let disp = crate::rpc::git::GitDispatcher::new(
        checkout.clone(),
        source.clone(),
        "main".to_string(),
        crate::rpc::caps::AgentCaps::interactive(),
    );
    let (resp, fx) = disp
        .dispatch_inner(
            "f",
            "git_fetch",
            &json!({ "refs": ["main", "feat/one", "pull/7/head"] }),
        )
        .await;
    assert!(resp.ok, "{resp:?}");
    assert!(fx.is_empty());
    let at = |rev: &str| crate::git::testing::git(&checkout, &["rev-parse", "--verify", rev]);
    assert_eq!(at("refs/remotes/origin/feat/one"), tip);
    assert_eq!(
        at("refs/remotes/origin/pr/7"),
        tip,
        "the PR head lands as origin/pr/7"
    );
    assert_eq!(
        at("refs/remotes/origin/main"),
        crate::git::testing::git(&source, &["rev-parse", "main"])
    );
    // Fetched again, once the tracking refs exist: a slashed branch's
    // ancestor name must not be deleted blindly.
    let (resp, _fx) = disp
        .dispatch_inner(
            "f2",
            "git_fetch",
            &json!({ "refs": ["feat/one", "pull/7/head"] }),
        )
        .await;
    assert!(resp.ok, "second fetch: {resp:?}");

    let (resp, _fx) = disp
        .dispatch_inner("l", "git_ls_remote", &json!({ "prefix": "feat/" }))
        .await;
    assert!(resp.ok, "{resp:?}");
    let heads: Vec<Value> = serde_json::from_str(resp.stdout.as_deref().unwrap()).unwrap();
    assert_eq!(heads.len(), 1);
    assert_eq!(heads[0]["branch"], "feat/one");
    assert_eq!(heads[0]["sha"], tip);

    // A repointed checkout origin is refused by both, as by the single fetch.
    run_git(
        &checkout,
        &[
            "remote",
            "set-url",
            "origin",
            td.path().join("elsewhere").to_str().unwrap(),
        ],
    );
    for (op, args) in [
        ("git_fetch", json!({ "refs": ["main"] })),
        ("git_ls_remote", Value::Null),
    ] {
        let (resp, _fx) = disp.dispatch_inner("r", op, &args).await;
        assert!(!resp.ok, "{op}");
        assert!(resp.error.unwrap().contains("refusing"), "{op}");
    }
    let _ = origin;
}
