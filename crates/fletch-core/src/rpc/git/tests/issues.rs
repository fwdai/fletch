use crate::rpc::git::test_support::*;

use serde_json::{json, Value};

fn repo_without_origin(root: &std::path::Path) -> std::path::PathBuf {
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "feat/x"]);
    repo
}

#[tokio::test]
async fn issue_view_needs_a_number_and_a_sane_comment_cap() {
    let td = tempfile::tempdir().unwrap();
    let disp = dispatcher(td.path());
    for (args, needle) in [
        (json!({}), "requires `number`"),
        (
            json!({ "number": 0 }),
            "`number` must be a positive integer",
        ),
        (
            json!({ "number": "9" }),
            "`number` must be a positive integer",
        ),
        (
            json!({ "number": 9, "comments": -1 }),
            "`comments` must be a non-negative integer",
        ),
    ] {
        let (resp, fx) = disp.dispatch_inner("i", "issue_view", &args).await;
        assert!(!resp.ok, "{args}");
        let err = resp.error.unwrap();
        assert!(err.contains(needle), "{args}: {err}");
        assert!(fx.is_empty());
    }
}

#[tokio::test]
async fn issue_list_validates_state_labels_and_limit() {
    let td = tempfile::tempdir().unwrap();
    let disp = dispatcher(td.path());
    for (args, needle) in [
        (json!({ "state": "merged" }), "`state` must be"),
        (
            json!({ "labels": 7 }),
            "`labels` must be a string or a list",
        ),
        (json!({ "limit": 0 }), "`limit` must be a positive integer"),
    ] {
        let (resp, fx) = disp.dispatch_inner("i", "issue_list", &args).await;
        assert!(!resp.ok, "{args}");
        let err = resp.error.unwrap();
        assert!(err.contains(needle), "{args}: {err}");
        assert!(fx.is_empty());
    }
}

/// Both reads go through the PR reads' preflight: a checkout with no GitHub
/// origin is told so, with nothing emitted.
#[tokio::test]
async fn issue_reads_name_a_non_github_origin() {
    let td = tempfile::tempdir().unwrap();
    let repo = repo_without_origin(td.path());
    let disp = dispatcher(&repo);
    for (op, args) in [
        ("issue_view", json!({ "number": 914 })),
        ("issue_list", Value::Null),
    ] {
        let (resp, fx) = disp.dispatch_inner("i", op, &args).await;
        assert_eq!(
            resp.error.as_deref(),
            Some(
                format!("{op}: origin is not a GitHub repository (or there is no origin)").as_str()
            )
        );
        assert!(fx.is_empty(), "{op}: {fx:?}");
    }
}
