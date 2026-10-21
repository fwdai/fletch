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
async fn an_unknown_op_is_still_refused_by_name() {
    let td = tempfile::tempdir().unwrap();
    let (resp, effects) = dispatcher(td.path())
        .dispatch_inner("u", "pr_rocket", &Value::Null)
        .await;
    assert!(!resp.ok);
    assert_eq!(resp.error.as_deref(), Some("unknown op: pr_rocket"));
    assert!(effects.is_empty());
}

#[tokio::test]
async fn pr_reads_reject_a_number_that_is_not_a_positive_integer() {
    let td = tempfile::tempdir().unwrap();
    let disp = dispatcher(td.path());
    for op in ["pr_checks", "pr_view"] {
        for bad in [
            json!(0),
            json!(-3),
            json!(1.5),
            json!("12"),
            json!(true),
            json!(u64::from(u32::MAX) + 1),
        ] {
            let (resp, effects) = disp
                .dispatch_inner("c", op, &json!({ "number": bad }))
                .await;
            assert!(!resp.ok, "{op} {bad} must be refused");
            let err = resp.error.unwrap();
            assert!(
                err.starts_with(&format!("{op}: `number` must be a positive integer")),
                "{op} {bad}: {err}"
            );
            assert!(effects.is_empty());
        }
    }
}

#[tokio::test]
async fn pr_check_log_needs_a_check_and_sane_numbers() {
    let td = tempfile::tempdir().unwrap();
    let disp = dispatcher(td.path());
    for (args, needle) in [
        (json!({}), "requires `check`"),
        (json!({ "check": "" }), "requires `check`"),
        (json!({ "check": 0 }), "requires `check`"),
        (json!({ "check": true }), "requires `check`"),
        (
            json!({ "check": "test", "lines": 0 }),
            "`lines` must be a positive integer",
        ),
        (
            json!({ "check": "test", "number": -1 }),
            "`number` must be a positive integer",
        ),
    ] {
        let (resp, fx) = disp.dispatch_inner("l", "pr_check_log", &args).await;
        assert!(!resp.ok, "{args}");
        let err = resp.error.unwrap();
        assert!(err.contains(needle), "{args}: {err}");
        assert!(fx.is_empty());
    }
}

#[tokio::test]
async fn pr_list_validates_its_filter_before_any_lookup() {
    let td = tempfile::tempdir().unwrap();
    let disp = dispatcher(td.path());
    for (args, needle) in [
        (json!({ "state": "merged" }), "`state` must be"),
        (json!({ "state": 1 }), "`state` must be"),
        (json!({ "limit": 0 }), "`limit` must be a positive integer"),
        (json!({ "head": "--all" }), "option-like head"),
        (json!({ "base": "-x" }), "option-like base"),
    ] {
        let (resp, fx) = disp.dispatch_inner("l", "pr_list", &args).await;
        assert!(!resp.ok, "{args}");
        let err = resp.error.unwrap();
        assert!(err.contains(needle), "{args}: {err}");
        assert!(fx.is_empty());
    }
}

#[test]
fn pr_list_filter_defaults_and_clamps() {
    use crate::rpc::git::pr_reads::pr_list_filter;
    let f = pr_list_filter(&json!({})).unwrap();
    assert_eq!(
        (f.state.as_str(), f.limit, f.head, f.base),
        ("open", 20, None, None)
    );
    let f = pr_list_filter(
        &json!({ "state": "all", "limit": 500, "head": " feat/x ", "base": "main" }),
    )
    .unwrap();
    assert_eq!(f.state, "all");
    assert_eq!(f.limit, 100);
    assert_eq!(f.head.as_deref(), Some("feat/x"));
    assert_eq!(f.base.as_deref(), Some("main"));
}

#[test]
fn a_check_is_named_by_id_or_name() {
    use crate::rpc::git::pr_reads::CheckRef;
    assert_eq!(
        CheckRef::from_args(&json!({ "check": 42 })),
        Some(CheckRef::Id(42))
    );
    assert_eq!(
        CheckRef::from_args(&json!({ "check": "42" })),
        Some(CheckRef::Id(42))
    );
    assert_eq!(
        CheckRef::from_args(&json!({ "check": " test (ubuntu) " })),
        Some(CheckRef::Name("test (ubuntu)".into()))
    );
    assert_eq!(CheckRef::from_args(&json!({ "check": null })), None);
}

#[test]
fn find_run_matches_exactly_then_loosely_and_names_the_rest() {
    use crate::github::CheckRun;
    use crate::rpc::git::pr_reads::{find_run, CheckRef};
    let run = |id: Option<u64>, name: &str| CheckRun {
        id,
        name: name.into(),
        status: "completed".into(),
        conclusion: Some("failure".into()),
        required: false,
        url: None,
        started_at: None,
        completed_at: None,
    };
    let runs = vec![
        run(Some(1), "test"),
        run(Some(2), "test (ubuntu)"),
        run(None, "ci/legacy"),
    ];
    assert_eq!(
        find_run(&runs, &CheckRef::Id(2)).unwrap().name,
        "test (ubuntu)"
    );
    assert_eq!(
        find_run(&runs, &CheckRef::Name("test".into())).unwrap().id,
        Some(1),
        "an exact name wins over a substring match"
    );
    assert_eq!(
        find_run(&runs, &CheckRef::Name("UBUNTU".into()))
            .unwrap()
            .id,
        Some(2)
    );
    let err = find_run(&runs, &CheckRef::Name("tes".into())).unwrap_err();
    assert!(err.contains("matches several"), "{err}");
    let err = find_run(&runs, &CheckRef::Id(9)).unwrap_err();
    assert!(
        err.contains("test (id 1)") && err.contains("ci/legacy"),
        "{err}"
    );
}

#[test]
fn pr_comments_limit_defaults_and_clamps() {
    use crate::rpc::git::pr_reads::comment_limit;
    assert_eq!(comment_limit(&Value::Null), Ok(50));
    assert_eq!(comment_limit(&json!({})), Ok(50));
    assert_eq!(comment_limit(&json!({ "limit": null })), Ok(50));
    assert_eq!(comment_limit(&json!({ "limit": 7 })), Ok(7));
    assert_eq!(comment_limit(&json!({ "limit": 100 })), Ok(100));
    assert_eq!(comment_limit(&json!({ "limit": 5000 })), Ok(100));
    for bad in [json!(0), json!(-1), json!("10"), json!(2.5)] {
        assert!(
            comment_limit(&json!({ "limit": bad })).is_err(),
            "{bad} must be refused"
        );
    }
}

#[tokio::test]
async fn pr_comments_rejects_a_bad_limit_before_any_lookup() {
    let td = tempfile::tempdir().unwrap();
    let (resp, _fx) = dispatcher(td.path())
        .dispatch_inner("c", "pr_comments", &json!({ "limit": 0 }))
        .await;
    assert!(!resp.ok);
    assert!(
        resp.error
            .as_deref()
            .unwrap()
            .starts_with("pr_comments: `limit` must be a positive integer"),
        "{:?}",
        resp.error
    );
}

/// A checkout with no `origin` resolves no GitHub repo, which the reads report
/// as such — before the token or backoff checks, so the answer is the same
/// whatever the process-global token registry holds — without touching the
/// network.
fn repo_without_origin(root: &std::path::Path) -> std::path::PathBuf {
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "feat/x"]);
    run_git(&repo, &["config", "user.email", "t@example.com"]);
    run_git(&repo, &["config", "user.name", "Tester"]);
    std::fs::write(repo.join("a.txt"), b"x").unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "init"]);
    repo
}

#[tokio::test]
async fn pr_reads_name_a_non_github_origin_and_emit_nothing() {
    let td = tempfile::tempdir().unwrap();
    let repo = repo_without_origin(td.path());
    let disp = dispatcher(&repo);
    for op in [
        "pr_view",
        "pr_checks",
        "pr_comments",
        "pr_check_log",
        "pr_list",
        "repo_view",
    ] {
        let args = if op == "pr_check_log" {
            json!({ "check": "test" })
        } else {
            Value::Null
        };
        let (resp, effects) = disp.dispatch_inner("r", op, &args).await;
        assert!(!resp.ok, "{op}");
        assert_eq!(
            resp.error.as_deref(),
            Some(
                format!("{op}: origin is not a GitHub repository (or there is no origin)").as_str()
            )
        );
        assert!(effects.is_empty(), "{op} is a read: {effects:?}");
    }
}

/// An explicit `args.number` still goes through the preflight: a checkout
/// that can't reach GitHub is told so rather than "no checks".
#[tokio::test]
async fn pr_checks_by_number_still_needs_a_github_origin() {
    let td = tempfile::tempdir().unwrap();
    let repo = repo_without_origin(td.path());
    let (resp, _fx) = dispatcher(&repo)
        .dispatch_inner("c", "pr_checks", &json!({ "number": 7 }))
        .await;
    assert_eq!(
        resp.error.as_deref(),
        Some("pr_checks: origin is not a GitHub repository (or there is no origin)")
    );
}

/// Reads are not publishing: a run-owned step agent and the advisory PM chat
/// reach the op (here: its "not a GitHub origin" answer) instead of the caps
/// refusal.
#[tokio::test]
async fn pr_reads_are_not_refused_by_denied_caps() {
    use crate::rpc::caps::AgentCaps;
    use crate::rpc::git::GitDispatcher;
    let td = tempfile::tempdir().unwrap();
    let repo = repo_without_origin(td.path());
    for caps in [AgentCaps::run_owned(), AgentCaps::advisory()] {
        let disp = GitDispatcher::new(repo.clone(), repo.clone(), "main".to_string(), caps);
        for op in [
            "pr_view",
            "pr_checks",
            "pr_comments",
            "pr_list",
            "repo_view",
        ] {
            let (resp, _fx) = disp.dispatch_inner("r", op, &Value::Null).await;
            let err = resp.error.unwrap_or_default();
            assert!(
                err.contains("origin is not a GitHub repository"),
                "{op} under {caps:?} must reach the op, got: {err}"
            );
        }
    }
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
