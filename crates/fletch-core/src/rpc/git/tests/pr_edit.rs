use crate::rpc::git::test_support::*;

use serde_json::{json, Value};

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
async fn pr_edit_needs_something_to_edit() {
    let td = tempfile::tempdir().unwrap();
    let disp = dispatcher(td.path());
    for args in [json!({}), json!({ "number": 3 }), json!({ "title": null })] {
        let (resp, fx) = disp.dispatch_inner("e", "pr_edit", &args).await;
        assert!(!resp.ok, "{args}");
        let err = resp.error.unwrap();
        assert!(err.contains("at least one of"), "{args}: {err}");
        assert!(fx.is_empty());
    }
}

#[tokio::test]
async fn pr_edit_refuses_blank_or_mistyped_fields_before_any_lookup() {
    let td = tempfile::tempdir().unwrap();
    let disp = dispatcher(td.path());
    for (args, needle) in [
        (json!({ "title": "  " }), "`title` must not be empty"),
        (json!({ "body": "" }), "`body` must not be empty"),
        (json!({ "title": 7 }), "`title` must be a string"),
        (json!({ "draft": "yes" }), "`draft` must be true or false"),
        (
            json!({ "title": "t", "number": 0 }),
            "`number` must be a positive integer",
        ),
        (json!({ "base": "--upload-pack=evil" }), "option-like base"),
    ] {
        let (resp, fx) = disp.dispatch_inner("e", "pr_edit", &args).await;
        assert!(!resp.ok, "{args}");
        let err = resp.error.unwrap();
        assert!(err.contains(needle), "{args}: {err}");
        assert!(fx.is_empty(), "{args}: {fx:?}");
    }
}

/// The preflight that names why a read can answer nothing applies to the edit
/// too: a checkout with no GitHub origin is told so, and nothing is emitted.
#[tokio::test]
async fn pr_edit_names_a_non_github_origin_and_emits_nothing() {
    let td = tempfile::tempdir().unwrap();
    let repo = repo_without_origin(td.path());
    let (resp, fx) = dispatcher(&repo)
        .dispatch_inner("e", "pr_edit", &json!({ "title": "better title" }))
        .await;
    assert_eq!(
        resp.error.as_deref(),
        Some("pr_edit: origin is not a GitHub repository (or there is no origin)")
    );
    assert!(fx.is_empty());
}

/// A successful edit or comment is a mutation the panel attributes to the
/// agent's turn.
#[test]
fn pr_writes_are_mutating_ops() {
    for op in ["pr_edit", "pr_comment", "issue_comment"] {
        assert!(crate::rpc::git::is_mutating_op(op), "{op}");
    }
}

#[tokio::test]
async fn comments_need_a_body_and_issues_need_a_number() {
    let td = tempfile::tempdir().unwrap();
    let disp = dispatcher(td.path());
    for (op, args, needle) in [
        ("pr_comment", json!({}), "requires a non-empty `body`"),
        (
            "pr_comment",
            json!({ "body": "  " }),
            "requires a non-empty `body`",
        ),
        (
            "pr_comment",
            json!({ "body": "x", "number": 0 }),
            "`number` must be a positive integer",
        ),
        ("issue_comment", json!({ "body": "x" }), "requires `number`"),
    ] {
        let (resp, fx) = disp.dispatch_inner("c", op, &args).await;
        assert!(!resp.ok, "{op} {args}");
        let err = resp.error.unwrap();
        assert!(err.contains(needle), "{op} {args}: {err}");
        assert!(fx.is_empty());
    }
}

/// Merging lands work on the base: a grant that may not publish is refused
/// before anything is looked up, with the same reason as for a push.
#[tokio::test]
async fn pr_merge_is_refused_for_grants_that_may_not_publish() {
    use crate::rpc::caps::AgentCaps;
    use crate::rpc::git::GitDispatcher;
    let td = tempfile::tempdir().unwrap();
    let repo = repo_without_origin(td.path());
    for (caps, needle) in [
        (AgentCaps::run_owned(), "finalizes"),
        (AgentCaps::advisory(), "roadmap item"),
    ] {
        let disp = GitDispatcher::new(repo.clone(), repo.clone(), "main".to_string(), caps);
        let (resp, fx) = disp
            .dispatch_inner("m", "pr_merge", &json!({ "number": 3 }))
            .await;
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains(needle), "{caps:?}");
        assert!(fx.is_empty());
        // Closing publishes nothing and stays reachable (here: its preflight).
        let (resp, _fx) = disp
            .dispatch_inner("c", "pr_close", &json!({ "number": 3 }))
            .await;
        assert!(
            resp.error
                .unwrap()
                .contains("origin is not a GitHub repository"),
            "{caps:?}"
        );
    }
}

#[tokio::test]
async fn pr_close_and_pr_merge_validate_and_name_a_non_github_origin() {
    let td = tempfile::tempdir().unwrap();
    let disp = dispatcher(td.path());
    for (op, args, needle) in [
        (
            "pr_close",
            json!({ "number": 0 }),
            "`number` must be a positive integer",
        ),
        (
            "pr_close",
            json!({ "comment": 5 }),
            "`comment` must be a string",
        ),
        (
            "pr_merge",
            json!({ "number": "7" }),
            "`number` must be a positive integer",
        ),
    ] {
        let (resp, fx) = disp.dispatch_inner("x", op, &args).await;
        assert!(!resp.ok, "{op} {args}");
        assert!(resp.error.unwrap().contains(needle), "{op} {args}");
        assert!(fx.is_empty());
    }
    let repo = repo_without_origin(td.path());
    let disp = dispatcher(&repo);
    for op in ["pr_close", "pr_merge"] {
        let (resp, fx) = disp.dispatch_inner("x", op, &Value::Null).await;
        assert_eq!(
            resp.error.as_deref(),
            Some(
                format!("{op}: origin is not a GitHub repository (or there is no origin)").as_str()
            )
        );
        assert!(fx.is_empty(), "{op}: {fx:?}");
    }
    for op in ["pr_close", "pr_merge"] {
        assert!(crate::rpc::git::is_mutating_op(op), "{op}");
    }
}

#[tokio::test]
async fn comments_name_a_non_github_origin_and_emit_nothing() {
    let td = tempfile::tempdir().unwrap();
    let repo = repo_without_origin(td.path());
    let disp = dispatcher(&repo);
    for (op, args) in [
        ("pr_comment", json!({ "body": "done" })),
        ("issue_comment", json!({ "body": "done", "number": 3 })),
    ] {
        let (resp, fx) = disp.dispatch_inner("c", op, &args).await;
        assert_eq!(
            resp.error.as_deref(),
            Some(
                format!("{op}: origin is not a GitHub repository (or there is no origin)").as_str()
            )
        );
        assert!(fx.is_empty(), "{op}: {fx:?}");
    }
}

#[test]
fn edited_fields_name_what_the_request_set() {
    use super::{edited_fields, parse_edit};
    let edit = parse_edit(&json!({ "title": " t ", "body": "b\n\nmore", "draft": false })).unwrap();
    assert_eq!(edit.title.as_deref(), Some("t"));
    assert_eq!(
        edit.body.as_deref(),
        Some("b\n\nmore"),
        "body keeps its line breaks"
    );
    assert_eq!(edit.draft, Some(false));
    assert_eq!(edited_fields(&edit), vec!["title", "body", "draft"]);
}
