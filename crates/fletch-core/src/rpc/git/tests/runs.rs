use crate::rpc::git::test_support::*;

use serde_json::{json, Value};

fn repo_without_origin(root: &std::path::Path) -> std::path::PathBuf {
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "feat/x"]);
    repo
}

#[tokio::test]
async fn run_list_validates_its_filter_before_any_lookup() {
    let td = tempfile::tempdir().unwrap();
    let disp = dispatcher(td.path());
    for (args, needle) in [
        (json!({ "status": "green" }), "`status` must be one of"),
        (
            json!({ "event": "Push!" }),
            "`event` must be a trigger name",
        ),
        (json!({ "limit": 0 }), "`limit` must be a positive integer"),
        (json!({ "branch": "--all" }), "option-like branch"),
        (
            json!({ "workflow": "../secrets" }),
            "`workflow` must be a workflow file name",
        ),
        (
            json!({ "workflow": "a/b.yml" }),
            "`workflow` must be a workflow file name",
        ),
        (
            json!({ "workflow": true }),
            "`workflow` must be a file name",
        ),
        (
            json!({ "workflow": 0 }),
            "`workflow` must be a file name or a positive id",
        ),
    ] {
        let (resp, fx) = disp.dispatch_inner("l", "run_list", &args).await;
        assert!(!resp.ok, "{args}");
        let err = resp.error.unwrap();
        assert!(err.contains(needle), "{args}: {err}");
        assert!(fx.is_empty());
    }
}

#[test]
fn run_list_filter_defaults_clamps_and_spells_the_workflow() {
    use crate::rpc::git::runs::run_list_filter;
    let f = run_list_filter(&json!({})).unwrap();
    assert_eq!(f.limit, 20);
    assert!(f.branch.is_none() && f.workflow.is_none() && f.status.is_none() && f.event.is_none());

    let f = run_list_filter(&json!({
        "branch": " main ", "workflow": ".github/workflows/ci.yml", "status": "failure",
        "event": "workflow_dispatch", "limit": 500
    }))
    .unwrap();
    assert_eq!(f.branch.as_deref(), Some("main"));
    assert_eq!(
        f.workflow.as_deref(),
        Some("ci.yml"),
        "the repository's spelling of a workflow path is accepted"
    );
    assert_eq!(f.status.as_deref(), Some("failure"));
    assert_eq!(f.event.as_deref(), Some("workflow_dispatch"));
    assert_eq!(f.limit, 100);

    let f = run_list_filter(&json!({ "workflow": 4242 })).unwrap();
    assert_eq!(
        f.workflow.as_deref(),
        Some("4242"),
        "a numeric id is a workflow too"
    );
}

#[tokio::test]
async fn run_log_needs_a_run_and_sane_arguments() {
    let td = tempfile::tempdir().unwrap();
    let disp = dispatcher(td.path());
    for (args, needle) in [
        (json!({}), "requires `run`"),
        (json!({ "run": 0 }), "`run` must be a positive integer"),
        (json!({ "run": "9001" }), "`run` must be a positive integer"),
        (
            json!({ "run": 9001, "job": "" }),
            "`job` must be a job's `id` or `name`",
        ),
        (
            json!({ "run": 9001, "job": true }),
            "`job` must be a job's `id` or `name`",
        ),
        (
            json!({ "run": 9001, "lines": 0 }),
            "`lines` must be a positive integer",
        ),
    ] {
        let (resp, fx) = disp.dispatch_inner("l", "run_log", &args).await;
        assert!(!resp.ok, "{args}");
        let err = resp.error.unwrap();
        assert!(err.contains(needle), "{args}: {err}");
        assert!(fx.is_empty());
    }
}

#[test]
fn a_failing_job_is_picked_by_default_and_a_tie_lists_the_jobs() {
    use crate::github::CheckRun;
    use crate::rpc::git::pr_reads::CheckRef;
    use crate::rpc::git::runs::pick_job;
    let job = |id: u64, name: &str, conclusion: &str| CheckRun {
        id: Some(id),
        name: name.into(),
        status: "completed".into(),
        conclusion: Some(conclusion.into()),
        required: false,
        url: None,
        started_at: None,
        completed_at: None,
    };
    let mixed = vec![
        job(1, "lint", "success"),
        job(2, "test (ubuntu)", "failure"),
        job(3, "test (macos)", "timed_out"),
    ];
    assert_eq!(
        pick_job(&mixed, None).unwrap().id,
        Some(2),
        "the first failing job is the default"
    );
    assert_eq!(
        pick_job(&mixed, Some(&CheckRef::Name("macos".into())))
            .unwrap()
            .id,
        Some(3)
    );
    assert_eq!(
        pick_job(&mixed, Some(&CheckRef::Id(1))).unwrap().name,
        "lint"
    );

    let green = vec![job(1, "lint", "success"), job(2, "test", "success")];
    let err = pick_job(&green, None).unwrap_err();
    assert!(
        err.contains("no job of the run failed") && err.contains("test (id 2)"),
        "{err}"
    );
    let only = vec![job(5, "build", "success")];
    assert_eq!(
        pick_job(&only, None).unwrap().id,
        Some(5),
        "the only job needs no name"
    );
    assert!(pick_job(&[], None).unwrap_err().contains("no jobs yet"));
}

#[tokio::test]
async fn workflow_dispatch_validates_before_any_lookup() {
    let td = tempfile::tempdir().unwrap();
    let disp = dispatcher(td.path());
    for (args, needle) in [
        (json!({}), "requires `workflow`"),
        (json!({ "workflow": "" }), "requires `workflow`"),
        (
            json!({ "workflow": "../x.yml" }),
            "`workflow` must be a workflow file name",
        ),
        (
            json!({ "workflow": "deploy.yml", "ref": "--force" }),
            "option-like ref",
        ),
        (
            json!({ "workflow": "deploy.yml", "ref": 7 }),
            "`ref` must be a branch or tag name",
        ),
        (
            json!({ "workflow": "deploy.yml", "inputs": ["a"] }),
            "`inputs` must be an object",
        ),
        (
            json!({ "workflow": "deploy.yml", "inputs": { "env": { "x": 1 } } }),
            "`inputs.env` must be a string, number or boolean",
        ),
    ] {
        let (resp, fx) = disp.dispatch_inner("d", "workflow_dispatch", &args).await;
        assert!(!resp.ok, "{args}");
        let err = resp.error.unwrap();
        assert!(err.contains(needle), "{args}: {err}");
        assert!(fx.is_empty(), "{args}: {fx:?}");
    }
}

#[test]
fn dispatch_inputs_are_spelled_as_strings_and_capped() {
    use crate::rpc::git::runs::dispatch_inputs;
    assert!(dispatch_inputs(&json!({})).unwrap().is_empty());
    let got = dispatch_inputs(&json!({ "inputs": {
        "environment": "prod", "replicas": 3, "dry_run": false
    }}))
    .unwrap();
    assert_eq!(got["environment"], json!("prod"));
    assert_eq!(got["replicas"], json!("3"), "a number is sent as text");
    assert_eq!(got["dry_run"], json!("false"), "a boolean is sent as text");

    let many: serde_json::Map<String, Value> =
        (0..11).map(|i| (format!("k{i}"), json!("v"))).collect();
    let err = dispatch_inputs(&json!({ "inputs": many })).unwrap_err();
    assert!(err.contains("at most 10"), "{err}");
}

/// Dispatching runs the repository's secrets on a ref: a grant that may not
/// publish is refused before anything is looked up, like a push.
#[tokio::test]
async fn workflow_dispatch_is_refused_for_grants_that_may_not_publish() {
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
            .dispatch_inner(
                "d",
                "workflow_dispatch",
                &json!({ "workflow": "deploy.yml" }),
            )
            .await;
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains(needle), "{caps:?}");
        assert!(fx.is_empty());
    }
    assert!(crate::rpc::git::is_mutating_op("workflow_dispatch"));
}

/// Every Actions op goes through the PR reads' preflight: a checkout with no
/// GitHub origin is told so, with nothing emitted.
#[tokio::test]
async fn run_reads_name_a_non_github_origin() {
    let td = tempfile::tempdir().unwrap();
    let repo = repo_without_origin(td.path());
    let disp = dispatcher(&repo);
    for (op, args) in [
        ("run_list", Value::Null),
        ("run_log", json!({ "run": 9001 })),
        ("workflow_dispatch", json!({ "workflow": "deploy.yml" })),
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
