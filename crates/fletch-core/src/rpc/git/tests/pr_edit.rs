use crate::rpc::git::test_support::*;

use serde_json::json;

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

/// A successful edit is a mutation the panel attributes to the agent's turn.
#[test]
fn pr_edit_is_a_mutating_op() {
    assert!(crate::rpc::git::is_mutating_op("pr_edit"));
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
