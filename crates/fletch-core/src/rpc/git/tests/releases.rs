use crate::rpc::git::test_support::*;

use serde_json::{json, Value};

fn repo_without_origin(root: &std::path::Path) -> std::path::PathBuf {
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "feat/x"]);
    repo
}

#[tokio::test]
async fn release_reads_validate_before_any_lookup() {
    let td = tempfile::tempdir().unwrap();
    let disp = dispatcher(td.path());
    for (op, args, needle) in [
        (
            "release_list",
            json!({ "limit": 0 }),
            "`limit` must be a positive integer",
        ),
        (
            "release_view",
            json!({ "tag": 7 }),
            "`tag` must be a string",
        ),
        (
            "release_view",
            json!({ "tag": "" }),
            "`tag` must be a tag name",
        ),
        (
            "release_view",
            json!({ "tag": "-v1" }),
            "`tag` must be a tag name",
        ),
        (
            "release_view",
            json!({ "tag": "v1 beta" }),
            "`tag` must be a tag name",
        ),
        (
            "release_view",
            json!({ "tag": "../latest" }),
            "`tag` must be a tag name",
        ),
    ] {
        let (resp, fx) = disp.dispatch_inner("r", op, &args).await;
        assert!(!resp.ok, "{op} {args}");
        let err = resp.error.unwrap();
        assert!(err.contains(needle), "{op} {args}: {err}");
        assert!(fx.is_empty());
    }
}

#[test]
fn a_tag_is_trimmed_and_may_carry_git_tag_characters() {
    use crate::rpc::git::releases::tag_arg;
    assert_eq!(tag_arg(&json!({})), Ok(None));
    assert_eq!(tag_arg(&json!({ "tag": null })), Ok(None));
    assert_eq!(
        tag_arg(&json!({ "tag": " v1.2.0-rc.1+build/7 " })),
        Ok(Some("v1.2.0-rc.1+build/7".into()))
    );
    assert_eq!(
        tag_arg(&json!({ "tag": "image-v2026.10.21" })),
        Ok(Some("image-v2026.10.21".into()))
    );
    assert_eq!(
        tag_arg(&json!({ "tag": "v1.0.0@rc1" })),
        Ok(Some("v1.0.0@rc1".into())),
        "`@` is legal in a tag name"
    );
}

/// The tag rules are git's own: what `git check-ref-format --allow-onelevel`
/// accepts passes, what it refuses is refused.
#[test]
fn tag_validation_follows_git_check_ref_format() {
    use crate::rpc::git::releases::is_well_formed_tag;
    for ok in [
        "v1",
        "v1.0.0@rc1",
        "releases/2026.10",
        "a+b",
        "ünïcode-tag",
        "with{brace}",
        "v1.0.0-rc.1+build.7",
    ] {
        assert!(is_well_formed_tag(ok), "{ok:?} is a valid tag");
    }
    for bad in [
        "", "@", "-v1", "/v1", "v1/", "v1.", "a..b", "a@{b", "a//b", "v 1", "v~1", "v^1", "v:1",
        "v?1", "v*1", "v[1", "v\\1", "v\t1", ".hidden", "a/.b", "v1.lock", "a/b.lock",
    ] {
        assert!(!is_well_formed_tag(bad), "{bad:?} is not a valid tag");
    }
}

/// Both reads go through the PR reads' preflight: a checkout with no GitHub
/// origin is told so, with nothing emitted.
#[tokio::test]
async fn release_reads_name_a_non_github_origin() {
    let td = tempfile::tempdir().unwrap();
    let repo = repo_without_origin(td.path());
    let disp = dispatcher(&repo);
    for (op, args) in [
        ("release_list", Value::Null),
        ("release_view", json!({ "tag": "v1" })),
        ("release_view", Value::Null),
    ] {
        let (resp, fx) = disp.dispatch_inner("r", op, &args).await;
        assert_eq!(
            resp.error.as_deref(),
            Some(
                format!("{op}: origin is not a GitHub repository (or there is no origin)").as_str()
            )
        );
        assert!(fx.is_empty(), "{op}: {fx:?}");
    }
}
