// Every case `src/readiness.test.ts` pins, so the host's ladder and the one the
// desktop still renders cannot drift. The TypeScript file's "portability to
// Rust" block has no counterpart: this is the port it was guarding.

use super::*;
use crate::git_state::FileStatus;
use crate::github::PrComment;
use crate::supervisor::delegation::app_action_message;

pub(crate) fn git() -> GitState {
    GitState {
        branch: "feat".to_string(),
        parent_branch: "main".to_string(),
        ahead: 1,
        behind: 0,
        unpushed: 0,
        files: Vec::new(),
        additions: 0,
        deletions: 0,
        remote_url: None,
        has_origin: true,
        head_sha: None,
        blocked_config: Vec::new(),
        worktrees: Vec::new(),
    }
}

pub(crate) fn file(kind: StatusKind, path: &str) -> FileStatus {
    FileStatus {
        path: path.to_string(),
        kind,
        staged: false,
        additions: 1,
        deletions: 0,
    }
}

pub(crate) fn pr() -> PrState {
    PrState {
        number: 7,
        url: "https://x".to_string(),
        state: PrStatus::Open,
        title: "t".to_string(),
        mergeable: MergeableState::Mergeable,
        opened_at: None,
        merged_at: None,
        branch: None,
    }
}

pub(crate) fn pr_in(state: PrStatus) -> PrState {
    PrState { state, ..pr() }
}

pub(crate) fn checks(merge_state: MergeState, failing: &[&str]) -> PrChecks {
    PrChecks {
        merge_state,
        rollup: "none".to_string(),
        total: 0,
        passed: 0,
        failed: 0,
        pending: 0,
        required_failing: failing.iter().map(|s| s.to_string()).collect(),
        runs: Vec::new(),
    }
}

pub(crate) fn thread(id: &str, we_replied_last: bool) -> PrComment {
    PrComment {
        id: id.to_string(),
        author: "bot".to_string(),
        is_bot: true,
        body: format!("c {id}"),
        path: None,
        line: None,
        url: "https://x".to_string(),
        replies: 0,
        we_replied_last,
    }
}

fn comments(n: usize, we_replied_last: bool) -> PrComments {
    PrComments {
        unresolved: (0..n)
            .map(|i| thread(&format!("t{i}"), we_replied_last))
            .collect(),
    }
}

fn input() -> ReadinessInput {
    ReadinessInput {
        git: Some(git()),
        pr: None,
        checks: None,
        comments: None,
    }
}

fn with_git(f: impl FnOnce(&mut GitState)) -> ReadinessInput {
    let mut g = git();
    f(&mut g);
    ReadinessInput {
        git: Some(g),
        ..input()
    }
}

fn ctx() -> LadderContext {
    LadderContext {
        base: "main".to_string(),
        commit_mode: CommitMode::CommitPr,
    }
}

fn kinds(i: &ReadinessInput) -> Vec<&'static str> {
    detect_blockers(i).iter().map(Blocker::kind).collect()
}

fn rung(i: &ReadinessInput) -> Rung {
    next_rung(i, &ctx())
}

/// The `kind` and `params` of a delegate rung, or a panic naming what it was.
fn delegated(r: &Rung) -> (DelegationKind, &'static str, BTreeMap<String, String>) {
    match r {
        Rung::Delegate {
            kind,
            action,
            params,
            ..
        } => (*kind, *action, params.clone()),
        other => panic!("expected a delegate rung, got {other:?}"),
    }
}

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

// ── detectBlockers ─────────────────────────────────────────────────────────

#[test]
fn reports_nothing_for_a_clean_pushed_gate_clean_proposal() {
    let i = ReadinessInput {
        pr: Some(pr()),
        checks: Some(checks(MergeState::Clean, &[])),
        comments: Some(comments(0, false)),
        ..input()
    };
    assert!(kinds(&i).is_empty());
}

#[test]
fn reports_unknown_state_as_no_blockers_and_the_ladder_waits() {
    let i = ReadinessInput {
        git: None,
        ..input()
    };
    assert!(detect_blockers(&i).is_empty());
    assert_eq!(
        rung(&i),
        Rung::Wait {
            why: WaitWhy::UnknownState
        }
    );
}

#[test]
fn treats_a_conflict_as_its_own_blocker_not_also_as_uncommitted_work() {
    let i = with_git(|g| {
        g.ahead = 0;
        g.files = vec![
            file(StatusKind::Conflicted, "a.ts"),
            file(StatusKind::Modified, "a.ts"),
        ];
    });
    assert_eq!(
        detect_blockers(&i),
        [Blocker::Conflicted {
            paths: vec!["a.ts".to_string()]
        }]
    );
}

#[test]
fn counts_uncommitted_files_when_nothing_is_conflicted() {
    let i = with_git(|g| {
        g.ahead = 0;
        g.files = vec![
            file(StatusKind::Modified, "a.ts"),
            file(StatusKind::Added, "b.ts"),
        ];
    });
    assert_eq!(detect_blockers(&i), [Blocker::Uncommitted { files: 2 }]);
}

#[test]
fn escalates_rather_than_ignoring_a_proposal_closed_unlanded() {
    let i = ReadinessInput {
        pr: Some(pr_in(PrStatus::Closed)),
        ..input()
    };
    assert_eq!(kinds(&i), ["proposal-closed"]);
}

#[test]
fn reports_unpushed_and_unsubmitted_only_when_no_proposal_exists() {
    let unpushed = |n| with_git(|g| g.unpushed = n);
    assert_eq!(kinds(&unpushed(2)), ["unpushed", "unsubmitted"]);
    // A proposal exists → the work is submitted.
    let i = ReadinessInput {
        pr: Some(pr()),
        checks: Some(checks(MergeState::Clean, &[])),
        ..unpushed(2)
    };
    assert_eq!(kinds(&i), ["unpushed"]);
    // A merged proposal covers the work that landed, `ahead` and all.
    let i = ReadinessInput {
        pr: Some(pr_in(PrStatus::Merged)),
        ..with_git(|g| g.ahead = 3)
    };
    assert!(kinds(&i).is_empty());
    let i = ReadinessInput {
        pr: Some(pr_in(PrStatus::Closed)),
        ..unpushed(2)
    };
    assert!(!kinds(&i).contains(&"unsubmitted"));
    // Commits made after the merge need a follow-up proposal.
    let i = ReadinessInput {
        pr: Some(pr_in(PrStatus::Merged)),
        ..unpushed(2)
    };
    assert_eq!(kinds(&i), ["unpushed", "unsubmitted"]);
}

#[test]
fn maps_the_merge_gate_onto_diverged_checks_review_draft() {
    let gate = |state, failing: &[&str]| {
        kinds(&ReadinessInput {
            pr: Some(pr()),
            checks: Some(checks(state, failing)),
            comments: Some(comments(0, false)),
            ..input()
        })
    };
    assert_eq!(gate(MergeState::Behind, &[]), ["diverged"]);
    assert_eq!(gate(MergeState::Dirty, &[]), ["diverged"]);
    assert_eq!(gate(MergeState::Blocked, &["test"]), ["checks-failing"]);
    // Blocked with nothing failing is a pure review gate.
    assert_eq!(gate(MergeState::Blocked, &[]), ["review-required"]);
    assert_eq!(gate(MergeState::Draft, &[]), ["draft"]);
    // `unstable` with nothing failing is just "checks still running".
    assert!(gate(MergeState::Unstable, &[]).is_empty());
    // But a failing check blocks whether or not it shuts the gate.
    assert_eq!(
        gate(MergeState::Unstable, &["rust-test"]),
        ["checks-failing"]
    );
}

#[test]
fn never_invents_a_conflict_from_a_not_yet_computed_mergeable_verdict() {
    let with = |mergeable| ReadinessInput {
        pr: Some(PrState { mergeable, ..pr() }),
        ..input()
    };
    assert!(kinds(&with(MergeableState::Unknown)).is_empty());
    assert_eq!(kinds(&with(MergeableState::Conflicting)), ["diverged"]);
}

#[test]
fn carries_the_failing_check_names_not_a_count() {
    let i = ReadinessInput {
        pr: Some(pr()),
        checks: Some(checks(MergeState::Blocked, &["build", "test (18)"])),
        ..input()
    };
    assert_eq!(
        detect_blockers(&i),
        [Blocker::ChecksFailing {
            checks: vec!["build".to_string(), "test (18)".to_string()]
        }]
    );
}

#[test]
fn reads_the_failing_names_never_the_raw_failed_count() {
    let mut c = checks(MergeState::Unstable, &[]);
    c.failed = 3;
    let i = ReadinessInput {
        pr: Some(pr()),
        checks: Some(c),
        comments: Some(comments(0, false)),
        ..input()
    };
    assert!(kinds(&i).is_empty());
}

#[test]
fn reports_unresolved_review_threads_only_on_an_open_proposal() {
    let i = ReadinessInput {
        pr: Some(pr()),
        checks: Some(checks(MergeState::Clean, &[])),
        comments: Some(comments(3, false)),
        ..input()
    };
    assert_eq!(kinds(&i), ["review-unaddressed"]);
    let i = ReadinessInput {
        pr: Some(pr_in(PrStatus::Merged)),
        comments: Some(comments(3, false)),
        ..input()
    };
    assert!(kinds(&i).is_empty());
}

// ── nextRung ordering ──────────────────────────────────────────────────────

#[test]
fn reconciles_the_working_copy_before_anything_else() {
    let i = ReadinessInput {
        git: Some(GitState {
            files: vec![file(StatusKind::Conflicted, "a.ts")],
            unpushed: 1,
            ..git()
        }),
        pr: Some(pr()),
        checks: Some(checks(MergeState::Dirty, &["test"])),
        comments: Some(comments(2, false)),
    };
    let (kind, action, _) = delegated(&rung(&i));
    assert_eq!(kind, DelegationKind::Resolve);
    assert_eq!(action, "resolve-conflicts");
}

#[test]
fn commits_in_the_sticky_mode_degrading_commit_pr_to_push_when_a_pr_is_open() {
    let dirty = || with_git(|g| g.files = vec![file(StatusKind::Modified, "a.ts")]);
    let (kind, _, params) = delegated(&rung(&dirty()));
    assert_eq!(kind, DelegationKind::CommitPr);
    assert_eq!(params, map(&[("base", "main")]));
    // A PR already exists — pushing is what updates it.
    let i = ReadinessInput {
        pr: Some(pr()),
        checks: Some(checks(MergeState::Clean, &[])),
        ..dirty()
    };
    assert_eq!(delegated(&rung(&i)).0, DelegationKind::CommitPush);
    // Plain local commit mode carries no base param.
    let commit = LadderContext {
        base: "main".to_string(),
        commit_mode: CommitMode::Commit,
    };
    let (kind, action, params) = delegated(&next_rung(&dirty(), &commit));
    assert_eq!(kind, DelegationKind::Commit);
    assert_eq!(action, "commit");
    assert!(params.is_empty());
}

#[test]
fn proposes_the_work_before_asking_the_forge_about_it() {
    let (kind, _, params) = delegated(&rung(&with_git(|g| g.unpushed = 2)));
    assert_eq!(kind, DelegationKind::OpenPr);
    assert_eq!(params, map(&[("base", "main")]));
    // Already proposed, just behind on pushes → plain push.
    let i = ReadinessInput {
        pr: Some(pr()),
        checks: Some(checks(MergeState::Clean, &[])),
        ..with_git(|g| g.unpushed = 2)
    };
    assert_eq!(delegated(&rung(&i)).0, DelegationKind::Push);
}

#[test]
fn syncs_with_mainline_before_trusting_a_check_result() {
    let i = ReadinessInput {
        pr: Some(pr()),
        checks: Some(checks(MergeState::Behind, &["test"])),
        ..input()
    };
    let (kind, _, params) = delegated(&rung(&i));
    assert_eq!(kind, DelegationKind::UpdateBranch);
    assert_eq!(params, map(&[("base", "main")]));
}

#[test]
fn fixes_checks_before_reading_review_threads_written_against_them() {
    let i = ReadinessInput {
        pr: Some(pr()),
        checks: Some(checks(MergeState::Blocked, &["build", "test"])),
        comments: Some(comments(4, false)),
        ..input()
    };
    let (kind, _, params) = delegated(&rung(&i));
    assert_eq!(kind, DelegationKind::FixChecks);
    assert_eq!(params, map(&[("failing", "build, test")]));
}

#[test]
fn escalates_what_no_agent_can_clear() {
    let with = |state| ReadinessInput {
        pr: Some(pr()),
        checks: Some(checks(state, &[])),
        ..input()
    };
    assert_eq!(
        rung(&with(MergeState::Blocked)),
        Rung::Escalate {
            blocker: Blocker::ReviewRequired
        }
    );
    assert_eq!(
        rung(&with(MergeState::Draft)),
        Rung::Escalate {
            blocker: Blocker::Draft
        }
    );
}

#[test]
fn hands_unresolved_review_threads_to_the_agent_with_the_count() {
    let i = ReadinessInput {
        pr: Some(pr()),
        checks: Some(checks(MergeState::Clean, &[])),
        comments: Some(comments(2, false)),
        ..input()
    };
    match rung(&i) {
        Rung::Delegate {
            kind,
            params,
            blocker,
            ..
        } => {
            assert_eq!(kind, DelegationKind::ResolveComments);
            assert_eq!(params, map(&[("count", "2")]));
            assert_eq!(blocker, Blocker::ReviewUnaddressed { count: 2 });
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn escalates_a_thread_it_already_pushed_back_on_instead_of_re_arguing_it() {
    let i = ReadinessInput {
        pr: Some(pr()),
        checks: Some(checks(MergeState::Clean, &[])),
        comments: Some(comments(1, true)),
        ..input()
    };
    assert_eq!(
        rung(&i),
        Rung::Escalate {
            blocker: Blocker::ReviewDisputed { count: 1 }
        }
    );
}

#[test]
fn works_the_actionable_threads_first_when_some_are_disputed() {
    let mixed = PrComments {
        unresolved: vec![thread("t0", true), thread("t1", false)],
    };
    let i = ReadinessInput {
        pr: Some(pr()),
        checks: Some(checks(MergeState::Clean, &[])),
        comments: Some(mixed),
        ..input()
    };
    let (kind, _, params) = delegated(&rung(&i));
    assert_eq!(kind, DelegationKind::ResolveComments);
    assert_eq!(params, map(&[("count", "1")]));
}

#[test]
fn merges_only_on_an_open_gate_and_waits_while_the_gate_is_computing() {
    let with_checks = |state| ReadinessInput {
        pr: Some(pr()),
        checks: Some(checks(state, &[])),
        ..input()
    };
    assert_eq!(rung(&with_checks(MergeState::Clean)), Rung::Merge);
    for state in [MergeState::Unknown, MergeState::HasHooks] {
        assert_eq!(
            rung(&with_checks(state)),
            Rung::Wait {
                why: WaitWhy::GateComputing
            }
        );
    }
    // No checks read, mergeability not computed → computing.
    let no_checks = |mergeable| ReadinessInput {
        pr: Some(PrState { mergeable, ..pr() }),
        ..input()
    };
    assert_eq!(
        rung(&no_checks(MergeableState::Unknown)),
        Rung::Wait {
            why: WaitWhy::GateComputing
        }
    );
    // No checks read, but no conflict: never auto-merge off zero CI knowledge.
    assert_eq!(rung(&no_checks(MergeableState::Mergeable)), Rung::Ready);
}

#[test]
fn reports_a_landed_proposal_and_a_clean_tree_with_nothing_to_do() {
    let i = ReadinessInput {
        pr: Some(pr_in(PrStatus::Merged)),
        ..input()
    };
    assert_eq!(rung(&i), Rung::Landed);
    assert_eq!(rung(&with_git(|g| g.ahead = 0)), Rung::Ready);
}

#[test]
fn keeps_climbing_after_a_merge_landed_is_not_the_end_of_the_workspace() {
    let merged = |f: fn(&mut GitState)| ReadinessInput {
        pr: Some(pr_in(PrStatus::Merged)),
        ..with_git(f)
    };
    let (kind, _, params) = delegated(&rung(&merged(|g| {
        g.files = vec![file(StatusKind::Modified, "a.ts")]
    })));
    assert_eq!(kind, DelegationKind::CommitPr);
    assert_eq!(params, map(&[("base", "main")]));
    assert_eq!(
        delegated(&rung(&merged(|g| g.unpushed = 1))).0,
        DelegationKind::OpenPr
    );
}

#[test]
fn is_total_every_combination_yields_a_rung() {
    let states = [
        Some(PrStatus::Open),
        Some(PrStatus::Merged),
        Some(PrStatus::Closed),
        None,
    ];
    let gates = [
        MergeState::Clean,
        MergeState::Blocked,
        MergeState::Unstable,
        MergeState::Behind,
        MergeState::Dirty,
        MergeState::Draft,
        MergeState::HasHooks,
        MergeState::Unknown,
    ];
    for state in states {
        for gate in gates {
            for files in [
                vec![],
                vec![file(StatusKind::Modified, "a.ts")],
                vec![file(StatusKind::Conflicted, "a.ts")],
            ] {
                for unpushed in [0, 1] {
                    let i = ReadinessInput {
                        git: Some(GitState {
                            files: files.clone(),
                            unpushed,
                            ..git()
                        }),
                        pr: state.map(pr_in),
                        checks: Some(checks(gate, &[])),
                        comments: Some(comments(0, false)),
                    };
                    // Total: returns without panicking, whatever the input.
                    let _ = rung(&i);
                }
            }
        }
    }
}

// ── the trigger each rung produces ─────────────────────────────────────────

/// The shared fixture: `src/readiness.test.ts` pins the desktop's ladder plus
/// `appActionMessage` against these same strings, so the trigger the host's
/// autopilot sends for a situation is the one the Git panel's button sends.
#[test]
fn each_rung_composes_the_typescript_trigger() {
    let ctx = LadderContext {
        base: "trunk".to_string(),
        commit_mode: CommitMode::CommitPr,
    };
    let open = |checks_: PrChecks, threads: usize| ReadinessInput {
        pr: Some(pr()),
        checks: Some(checks_),
        comments: Some(comments(threads, false)),
        ..input()
    };
    let cases: Vec<(ReadinessInput, &str)> = vec![
        (
            with_git(|g| g.files = vec![file(StatusKind::Conflicted, "a.ts")]),
            "[app-action] resolve-conflicts",
        ),
        (
            with_git(|g| g.files = vec![file(StatusKind::Modified, "a.ts")]),
            r#"[app-action] commit-pr base="trunk""#,
        ),
        (
            ReadinessInput {
                pr: Some(pr()),
                ..with_git(|g| g.files = vec![file(StatusKind::Modified, "a.ts")])
            },
            "[app-action] commit-push",
        ),
        (
            with_git(|g| g.unpushed = 2),
            r#"[app-action] open-pr base="trunk""#,
        ),
        (
            ReadinessInput {
                pr: Some(pr()),
                checks: Some(checks(MergeState::Clean, &[])),
                ..with_git(|g| g.unpushed = 2)
            },
            "[app-action] push",
        ),
        (
            open(checks(MergeState::Behind, &[]), 0),
            r#"[app-action] update-branch base="trunk""#,
        ),
        (
            open(checks(MergeState::Blocked, &["build", "test"]), 0),
            r#"[app-action] fix-checks failing="build, test""#,
        ),
        (
            open(checks(MergeState::Clean, &[]), 2),
            r#"[app-action] resolve-comments count="2""#,
        ),
    ];
    for (i, expected) in cases {
        let (_, action, params) = delegated(&next_rung(&i, &ctx));
        let pairs: Vec<(&str, &str)> = params
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(app_action_message(action, &pairs), expected);
    }
}
