// Every case `src/autopilot.test.ts` pinned, plus the cycle bookkeeping the
// desktop store applied (`src/store/autopilot.test.ts`, "cycle bookkeeping").
// Autopilot spends agent turns and CI runs without being asked, so most of
// these prove it STOPS — on a spent budget, a world it failed to change, a rung
// it isn't allowed to take, anything the user is doing themselves — or stays
// QUIET where nothing is its business.

use super::*;
use crate::autopilot::readiness::tests::{checks, file, git, pr, pr_in, thread};
use crate::autopilot::readiness::CommitMode;
use crate::git_state::FileStatus;
use crate::github::{MergeState, PrComment, PrComments, PrStatus};
use crate::verify::{CheckOutcome, CheckResult};

const NOW: i64 = 1_000_000;

fn git_at(sha: &str) -> GitState {
    GitState {
        head_sha: Some(sha.to_string()),
        ..git()
    }
}

/// A PR whose required checks are failing — the one world autopilot acts on.
fn failing() -> ReadinessInput {
    ReadinessInput {
        git: Some(git_at("sha1")),
        pr: Some(pr()),
        checks: Some(checks(MergeState::Blocked, &["test"])),
        comments: Some(PrComments {
            unresolved: Vec::new(),
        }),
    }
}

/// The same PR, fixed.
fn green() -> ReadinessInput {
    ReadinessInput {
        git: Some(git_at("sha2")),
        pr: Some(pr()),
        checks: Some(checks(MergeState::Clean, &[])),
        comments: Some(PrComments {
            unresolved: Vec::new(),
        }),
    }
}

fn ladder() -> LadderContext {
    LadderContext {
        base: "main".to_string(),
        commit_mode: CommitMode::CommitPr,
    }
}

/// The situation `failing()` is.
fn situation() -> String {
    blocker_fingerprint(&detect_blockers(&failing()))
}

fn report(outcomes: &[(&str, CheckOutcome)]) -> VerificationReport {
    VerificationReport {
        checks: outcomes
            .iter()
            .map(|(name, outcome)| CheckResult {
                name: name.to_string(),
                command: format!("run {name}"),
                outcome: *outcome,
                duration_ms: 1,
                tail: Vec::new(),
            })
            .collect(),
    }
}

fn cycle(rung: DelegationKind, phase: CyclePhase) -> Cycle {
    Cycle {
        rung,
        attempt: 1,
        signature: state_signature(&failing()),
        phase,
        phase_since: NOW,
    }
}

fn with_cycle(c: Cycle) -> AutopilotState {
    AutopilotState {
        cycle: Some(c),
        ..AutopilotState::enrolled()
    }
}

/// Attempts spent on `failing()`'s situation — the only kind that counts.
fn spent(n: u32) -> AutopilotState {
    AutopilotState {
        attempts: HashMap::from([(DelegationKind::FixChecks, n)]),
        situation: situation(),
        ..AutopilotState::enrolled()
    }
}

/// One step with the defaults overridden by `f`.
struct Case {
    state: Option<AutopilotState>,
    readiness: ReadinessInput,
    agent_busy: bool,
    delegation_in_flight: bool,
    verification: Option<VerificationReport>,
    now: i64,
}

impl Case {
    fn new() -> Self {
        Self {
            state: Some(AutopilotState::enrolled()),
            readiness: failing(),
            agent_busy: false,
            delegation_in_flight: false,
            verification: None,
            now: NOW,
        }
    }

    fn state(mut self, s: AutopilotState) -> Self {
        self.state = Some(s);
        self
    }

    fn readiness(mut self, r: ReadinessInput) -> Self {
        self.readiness = r;
        self
    }

    fn step(&self) -> Effect {
        autopilot_step(&AutopilotInput {
            state: self.state.as_ref(),
            readiness: &self.readiness,
            ladder: &ladder(),
            agent_busy: self.agent_busy,
            delegation_in_flight: self.delegation_in_flight,
            verification: self.verification.as_ref(),
            now: self.now,
        })
    }
}

fn waits(why: WaitReason) -> Effect {
    Effect::Wait { why }
}

fn dispatched(
    e: &Effect,
) -> (
    DelegationKind,
    &'static str,
    BTreeMap<String, String>,
    String,
) {
    match e {
        Effect::Dispatch {
            rung,
            action,
            params,
            situation,
            ..
        } => (*rung, *action, params.clone(), situation.clone()),
        other => panic!("expected a dispatch, got {other:?}"),
    }
}

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn modified(path: &str, staged: bool) -> FileStatus {
    FileStatus {
        staged,
        ..file(StatusKind::Modified, path)
    }
}

// ── autopilot refuses to act ───────────────────────────────────────────────

#[test]
fn does_nothing_at_all_unless_the_checkout_was_enrolled() {
    let mut c = Case::new();
    c.state = None;
    assert_eq!(c.step(), waits(WaitReason::NotEnrolled));
    let c = Case::new().state(AutopilotState::default());
    assert_eq!(c.step(), waits(WaitReason::NotEnrolled));
}

#[test]
fn never_interleaves_with_a_turn_it_didnt_start() {
    let mut c = Case::new();
    c.agent_busy = true;
    assert_eq!(c.step(), waits(WaitReason::AgentBusy));
    let mut c = Case::new();
    c.delegation_in_flight = true;
    assert_eq!(c.step(), waits(WaitReason::DelegationInFlight));
}

#[test]
fn waits_out_an_unsettled_world_rather_than_inventing_work() {
    let c = Case::new().readiness(ReadinessInput {
        checks: Some(checks(MergeState::Unknown, &[])),
        ..failing()
    });
    assert_eq!(c.step(), waits(WaitReason::GateSettling));
}

#[test]
fn does_nothing_once_there_is_nothing_left_that_it_handles() {
    // A mergeable PR is not autopilot's to merge.
    assert_eq!(
        Case::new().readiness(green()).step(),
        waits(WaitReason::NothingToDo)
    );
}

// ── its job starts when a PR is open, and only then ────────────────────────

#[test]
fn is_a_no_op_with_no_pr_at_all_whatever_the_tree_looks_like() {
    let c = Case::new().readiness(ReadinessInput {
        pr: None,
        git: None,
        ..failing()
    });
    assert_eq!(c.step(), waits(WaitReason::NoPr));
    // Pushed, not proposed: opening the PR is the user's move.
    let c = Case::new().readiness(ReadinessInput {
        pr: None,
        ..failing()
    });
    assert_eq!(c.step(), waits(WaitReason::NoPr));
    // Uncommitted work: so is committing it.
    let c = Case::new().readiness(ReadinessInput {
        pr: None,
        git: Some(GitState {
            files: vec![modified("a.ts", false)],
            ..git_at("sha1")
        }),
        ..failing()
    });
    assert_eq!(c.step(), waits(WaitReason::NoPr));
}

#[test]
fn is_a_no_op_on_a_closed_or_merged_pr() {
    let c = Case::new().readiness(ReadinessInput {
        pr: Some(pr_in(PrStatus::Closed)),
        ..failing()
    });
    assert_eq!(c.step(), waits(WaitReason::NoPr));
    let c = Case::new().readiness(ReadinessInput {
        pr: Some(pr_in(PrStatus::Merged)),
        ..green()
    });
    assert_eq!(c.step(), waits(WaitReason::NoPr));
}

#[test]
fn leaves_a_rung_it_never_drives_to_the_user_which_keeps_it_off_a_dirty_tree() {
    let c = Case::new().readiness(ReadinessInput {
        git: Some(GitState {
            files: vec![modified("a.ts", false)],
            ..git_at("sha1")
        }),
        ..failing()
    });
    assert_eq!(c.step(), waits(WaitReason::NotMine));
}

#[test]
fn is_a_no_op_on_a_human_owned_gate() {
    let review_gate = Case::new().readiness(ReadinessInput {
        checks: Some(checks(MergeState::Blocked, &[])),
        ..failing()
    });
    assert_eq!(review_gate.step(), waits(WaitReason::NothingToDo));
    let draft = Case::new().readiness(ReadinessInput {
        checks: Some(checks(MergeState::Draft, &[])),
        ..green()
    });
    assert_eq!(draft.step(), waits(WaitReason::NothingToDo));
}

#[test]
fn still_judges_a_cycle_in_flight_when_the_pr_closes_under_it() {
    let c = Case::new()
        .state(with_cycle(cycle(
            DelegationKind::FixChecks,
            CyclePhase::AwaitingEvidence,
        )))
        .readiness(ReadinessInput {
            pr: Some(pr_in(PrStatus::Merged)),
            ..green()
        });
    assert_eq!(
        c.step(),
        Effect::Settle {
            rung: DelegationKind::FixChecks
        }
    );
}

// ── opening a cycle ────────────────────────────────────────────────────────

#[test]
fn dispatches_fix_checks_with_the_failing_names_the_world_and_the_situation() {
    assert_eq!(
        Case::new().step(),
        Effect::Dispatch {
            rung: DelegationKind::FixChecks,
            action: "fix-checks",
            params: map(&[("failing", "test")]),
            signature: state_signature(&failing()),
            situation: situation(),
        }
    );
}

#[test]
fn fixes_a_failing_check_on_a_repo_with_no_required_checks() {
    let mut soft = checks(MergeState::Unstable, &["rust-test"]);
    soft.failed = 1;
    let c = Case::new().readiness(ReadinessInput {
        checks: Some(soft),
        ..failing()
    });
    let (rung, _, params, _) = dispatched(&c.step());
    assert_eq!(rung, DelegationKind::FixChecks);
    assert_eq!(params, map(&[("failing", "rust-test")]));
}

#[test]
fn refuses_to_re_enter_a_world_it_already_failed_to_change() {
    let c = Case::new().state(AutopilotState {
        barren: vec![state_signature(&failing())],
        ..AutopilotState::enrolled()
    });
    assert_eq!(c.step(), waits(WaitReason::NoProgress));
}

#[test]
fn waits_quietly_once_the_rungs_budget_for_this_situation_is_spent() {
    let c = Case::new().state(spent(rung_budget(DelegationKind::FixChecks)));
    assert_eq!(c.step(), waits(WaitReason::BudgetSpent));
}

// ── the budget belongs to a situation ──────────────────────────────────────

#[test]
fn starts_the_count_over_when_the_failing_check_is_a_different_one() {
    let other = ReadinessInput {
        checks: Some(checks(MergeState::Blocked, &["lint"])),
        ..failing()
    };
    let expected = blocker_fingerprint(&detect_blockers(&other));
    let c = Case::new().state(spent(3)).readiness(other);
    let (rung, _, _, situation) = dispatched(&c.step());
    assert_eq!(rung, DelegationKind::FixChecks);
    assert_eq!(situation, expected);
}

#[test]
fn does_not_count_attempts_spent_on_some_other_situation() {
    let c = Case::new().state(AutopilotState {
        attempts: HashMap::from([(DelegationKind::FixChecks, 3)]),
        situation: "checks-failing:lint".to_string(),
        ..AutopilotState::enrolled()
    });
    assert_eq!(dispatched(&c.step()).3, situation());
}

#[test]
fn stays_off_a_world_it_already_proved_barren_whatever_the_situation_says() {
    let c = Case::new().state(AutopilotState {
        barren: vec![state_signature(&failing())],
        situation: "checks-failing:lint".to_string(),
        ..AutopilotState::enrolled()
    });
    assert_eq!(c.step(), waits(WaitReason::NoProgress));
}

#[test]
fn stamps_the_dispatch_with_the_situation() {
    assert_eq!(dispatched(&Case::new().step()).3, situation());
}

// ── judging a cycle in flight ──────────────────────────────────────────────

fn in_flight(phase: CyclePhase) -> Case {
    Case::new().state(with_cycle(cycle(DelegationKind::FixChecks, phase)))
}

#[test]
fn holds_while_the_agent_works_then_asks_for_a_local_verdict() {
    let mut c = in_flight(CyclePhase::Working);
    c.agent_busy = true;
    assert_eq!(c.step(), waits(WaitReason::AwaitingEvidence));
    assert_eq!(in_flight(CyclePhase::Working).step(), Effect::Verify);
}

#[test]
fn settles_when_the_checks_it_was_fixing_are_gone_and_the_world_moved() {
    let c = in_flight(CyclePhase::AwaitingEvidence).readiness(green());
    assert_eq!(
        c.step(),
        Effect::Settle {
            rung: DelegationKind::FixChecks
        }
    );
}

#[test]
fn believes_a_failing_local_verification_over_cis_silence() {
    let mut c = in_flight(CyclePhase::AwaitingEvidence).readiness(green());
    c.verification = Some(report(&[("test", CheckOutcome::Failed)]));
    assert_eq!(
        c.step(),
        Effect::Retry {
            rung: DelegationKind::FixChecks,
            barren: None
        }
    );
}

#[test]
fn records_a_barren_signature_when_a_cycle_changed_nothing() {
    assert_eq!(
        in_flight(CyclePhase::AwaitingEvidence).step(),
        Effect::Retry {
            rung: DelegationKind::FixChecks,
            barren: Some(state_signature(&failing()))
        }
    );
}

#[test]
fn gives_up_the_second_time_the_same_world_produces_nothing() {
    let c = Case::new().state(AutopilotState {
        barren: vec![state_signature(&failing())],
        ..with_cycle(cycle(
            DelegationKind::FixChecks,
            CyclePhase::AwaitingEvidence,
        ))
    });
    assert_eq!(
        c.step(),
        Effect::GiveUp {
            rung: DelegationKind::FixChecks,
            reason: GiveUpReason::NoProgress,
            barren: Some(state_signature(&failing()))
        }
    );
}

#[test]
fn treats_a_changed_commit_as_progress_even_when_the_failure_is_identical() {
    let c = in_flight(CyclePhase::AwaitingEvidence).readiness(ReadinessInput {
        git: Some(git_at("sha9")),
        ..failing()
    });
    assert_eq!(
        c.step(),
        Effect::Retry {
            rung: DelegationKind::FixChecks,
            barren: None
        }
    );
}

fn last_attempt() -> Case {
    Case::new().state(with_cycle(Cycle {
        attempt: rung_budget(DelegationKind::FixChecks),
        ..cycle(DelegationKind::FixChecks, CyclePhase::AwaitingEvidence)
    }))
}

#[test]
fn gives_up_when_the_failing_cycle_was_the_last_one_in_the_budget() {
    assert!(matches!(
        last_attempt().step(),
        Effect::GiveUp {
            rung: DelegationKind::FixChecks,
            reason: GiveUpReason::BudgetSpent,
            ..
        }
    ));
}

#[test]
fn waits_for_ci_but_calls_the_cycle_inconclusive_if_it_never_speaks() {
    let computing = ReadinessInput {
        checks: Some(checks(MergeState::Unknown, &[])),
        ..failing()
    };
    let c = in_flight(CyclePhase::AwaitingEvidence).readiness(computing.clone());
    assert_eq!(c.step(), waits(WaitReason::AwaitingEvidence));
    let c = Case::new()
        .state(with_cycle(Cycle {
            phase_since: NOW - EVIDENCE_TIMEOUT_MS - 1,
            ..cycle(DelegationKind::FixChecks, CyclePhase::AwaitingEvidence)
        }))
        .readiness(computing);
    assert!(matches!(
        c.step(),
        Effect::GiveUp {
            rung: DelegationKind::FixChecks,
            reason: GiveUpReason::NoEvidence,
            ..
        }
    ));
}

#[test]
fn a_give_up_carries_the_same_barren_verdict_a_retry_would() {
    match last_attempt().step() {
        Effect::GiveUp { barren, .. } => assert_eq!(barren, Some(state_signature(&failing()))),
        other => panic!("{other:?}"),
    }
}

// ── the reconcile rungs ────────────────────────────────────────────────────

/// A mid-merge tree: the merge's own content staged, the unresolved file
/// conflicted.
fn mid_merge(files: Vec<FileStatus>) -> ReadinessInput {
    ReadinessInput {
        git: Some(GitState {
            files,
            ..git_at("sha1")
        }),
        ..failing()
    }
}

fn merge_files() -> Vec<FileStatus> {
    vec![file(StatusKind::Conflicted, "a.ts"), modified("b.ts", true)]
}

#[test]
fn resolves_conflicts_ahead_of_everything_else_thats_wrong() {
    let c = Case::new().readiness(mid_merge(merge_files()));
    let (rung, action, _, _) = dispatched(&c.step());
    assert_eq!(rung, DelegationKind::Resolve);
    assert_eq!(action, "resolve-conflicts");
}

#[test]
fn refuses_to_finish_a_merge_that_would_swallow_the_users_uncommitted_work() {
    let c = Case::new().readiness(mid_merge(vec![
        file(StatusKind::Conflicted, "a.ts"),
        modified("mine.ts", false),
    ]));
    assert_eq!(c.step(), waits(WaitReason::DirtyTree));
}

#[test]
fn leaves_local_conflicts_alone_when_no_pr_is_open() {
    let c = Case::new().readiness(ReadinessInput {
        pr: None,
        ..mid_merge(merge_files())
    });
    assert_eq!(c.step(), waits(WaitReason::NoPr));
}

#[test]
fn updates_a_branch_that_has_fallen_behind_its_base() {
    let c = Case::new().readiness(ReadinessInput {
        checks: Some(checks(MergeState::Behind, &["test"])),
        ..failing()
    });
    let (rung, _, params, _) = dispatched(&c.step());
    assert_eq!(rung, DelegationKind::UpdateBranch);
    assert_eq!(params, map(&[("base", "main")]));
}

#[test]
fn judges_a_reconcile_on_the_world_not_by_running_the_tests() {
    for rung in [DelegationKind::Resolve, DelegationKind::UpdateBranch] {
        let c = Case::new().state(with_cycle(cycle(rung, CyclePhase::Working)));
        assert_eq!(c.step(), Effect::AwaitEvidence);
    }
    // And a failing local report must not condemn one.
    let mut c = Case::new()
        .state(with_cycle(cycle(
            DelegationKind::UpdateBranch,
            CyclePhase::AwaitingEvidence,
        )))
        .readiness(green());
    c.verification = Some(report(&[("test", CheckOutcome::Failed)]));
    assert_eq!(
        c.step(),
        Effect::Settle {
            rung: DelegationKind::UpdateBranch
        }
    );
}

#[test]
fn still_runs_the_tests_for_a_code_fix() {
    assert_eq!(in_flight(CyclePhase::Working).step(), Effect::Verify);
}

#[test]
fn gives_a_reconcile_two_attempts_not_three() {
    for rung in [DelegationKind::Resolve, DelegationKind::UpdateBranch] {
        assert_eq!(rung_budget(rung), 2);
    }
}

// ── the review-comments rung ───────────────────────────────────────────────

fn greptile(id: &str) -> PrComment {
    PrComment {
        author: "greptileai".to_string(),
        path: Some("a.ts".to_string()),
        line: Some(1),
        ..thread(id, false)
    }
}

fn with_threads(threads: Vec<PrComment>) -> ReadinessInput {
    ReadinessInput {
        comments: Some(PrComments {
            unresolved: threads,
        }),
        ..green()
    }
}

#[test]
fn works_threads_that_are_waiting_on_us_with_the_count() {
    let c = Case::new().readiness(with_threads(vec![greptile("t1"), greptile("t2")]));
    let (rung, _, params, _) = dispatched(&c.step());
    assert_eq!(rung, DelegationKind::ResolveComments);
    assert_eq!(params, map(&[("count", "2")]));
}

#[test]
fn never_re_argues_a_thread_it_already_pushed_back_on() {
    let pushed_back = PrComment {
        we_replied_last: true,
        ..greptile("t1")
    };
    let c = Case::new().readiness(with_threads(vec![pushed_back]));
    assert_eq!(c.step(), waits(WaitReason::NothingToDo));
}

#[test]
fn engages_again_once_the_human_answers() {
    let answered = PrComment {
        replies: 2,
        ..greptile("t1")
    };
    let c = Case::new().readiness(with_threads(vec![answered]));
    assert_eq!(dispatched(&c.step()).0, DelegationKind::ResolveComments);
}

#[test]
fn is_judged_by_the_threads_not_by_the_tests() {
    let c = Case::new().state(with_cycle(cycle(
        DelegationKind::ResolveComments,
        CyclePhase::Working,
    )));
    assert_eq!(c.step(), Effect::AwaitEvidence);
    let mut c = Case::new()
        .state(with_cycle(cycle(
            DelegationKind::ResolveComments,
            CyclePhase::AwaitingEvidence,
        )))
        .readiness(green());
    c.verification = Some(report(&[("test", CheckOutcome::Failed)]));
    assert_eq!(
        c.step(),
        Effect::Settle {
            rung: DelegationKind::ResolveComments
        }
    );
}

#[test]
fn counts_a_push_back_as_progress_not_as_a_barren_cycle() {
    let before = with_threads(vec![greptile("t1")]);
    let after = with_threads(vec![PrComment {
        we_replied_last: true,
        ..greptile("t1")
    }]);
    assert_ne!(state_signature(&before), state_signature(&after));
}

#[test]
fn counts_a_partial_round_as_progress() {
    let before = with_threads(vec![greptile("t1"), greptile("t2"), greptile("t3")]);
    let after = with_threads(vec![greptile("t3")]);
    assert_ne!(state_signature(&before), state_signature(&after));
}

#[test]
fn gets_two_attempts_each_one_posts_into_a_real_conversation() {
    assert_eq!(rung_budget(DelegationKind::ResolveComments), 2);
}

// ── unstagedEdits ──────────────────────────────────────────────────────────

#[test]
fn counts_only_the_users_in_flight_work_not_the_merges_own_content() {
    assert_eq!(unstaged_edits(None), 0);
    let merge = GitState {
        files: merge_files(),
        ..git()
    };
    assert_eq!(unstaged_edits(Some(&merge)), 0);
    let mine = GitState {
        files: vec![modified("mine.ts", false)],
        ..git()
    };
    assert_eq!(unstaged_edits(Some(&mine)), 1);
}

// ── stateSignature ─────────────────────────────────────────────────────────

#[test]
fn signature_changes_with_the_commit_and_with_the_set_of_failures() {
    assert_ne!(state_signature(&failing()), state_signature(&green()));
    let other = ReadinessInput {
        git: Some(git_at("other")),
        ..failing()
    };
    assert_ne!(state_signature(&failing()), state_signature(&other));
}

#[test]
fn signature_ignores_the_order_ci_reports_its_checks_in() {
    let a = ReadinessInput {
        checks: Some(checks(MergeState::Clean, &["lint", "test"])),
        ..failing()
    };
    let b = ReadinessInput {
        checks: Some(checks(MergeState::Clean, &["test", "lint"])),
        ..failing()
    };
    assert_eq!(state_signature(&a), state_signature(&b));
}

#[test]
fn signature_is_stable_when_nothing_observable_changed() {
    assert_eq!(state_signature(&failing()), state_signature(&failing()));
}

#[test]
fn signature_tracks_the_conflict_set_so_a_partial_resolution_is_progress() {
    let conflicts = |paths: &[&str]| ReadinessInput {
        git: Some(GitState {
            files: paths
                .iter()
                .map(|p| file(StatusKind::Conflicted, p))
                .collect(),
            ..git_at("sha1")
        }),
        ..failing()
    };
    assert_ne!(
        state_signature(&conflicts(&["a.ts", "b.ts"])),
        state_signature(&conflicts(&["a.ts"]))
    );
    assert_eq!(
        state_signature(&conflicts(&["a.ts", "b.ts"])),
        state_signature(&conflicts(&["b.ts", "a.ts"]))
    );
}

/// The exact spelling, so the desktop's copy and the host's agree on what a
/// world is (and a log or a test reading one is not reading a guess).
#[test]
fn signature_is_sha_failures_conflicts_and_threads() {
    let i = ReadinessInput {
        git: Some(GitState {
            files: vec![
                file(StatusKind::Conflicted, "b.ts"),
                file(StatusKind::Conflicted, "a.ts"),
            ],
            ..git_at("abc")
        }),
        pr: Some(pr()),
        checks: Some(checks(MergeState::Blocked, &["test", "build"])),
        comments: Some(PrComments {
            unresolved: vec![thread("t2", true), thread("t1", false)],
        }),
    };
    assert_eq!(state_signature(&i), "abc|build,test|a.ts,b.ts|t1,t2!");
    assert_eq!(state_signature(&ReadinessInput::default()), "no-head|||");
}

// ── blockerFingerprint ─────────────────────────────────────────────────────

#[test]
fn fingerprint_distinguishes_which_instance_of_a_blocker_not_just_its_kind() {
    let fp = |names: &[&str]| {
        blocker_fingerprint(&detect_blockers(&ReadinessInput {
            checks: Some(checks(MergeState::Blocked, names)),
            ..failing()
        }))
    };
    assert_ne!(fp(&["test"]), fp(&["lint"]));
    assert_eq!(fp(&["test", "lint"]), fp(&["lint", "test"]));
}

#[test]
fn fingerprint_is_empty_when_nothing_is_blocking() {
    assert_eq!(blocker_fingerprint(&detect_blockers(&green())), "");
    assert_ne!(blocker_fingerprint(&detect_blockers(&failing())), "");
}

// ── verification verdicts ──────────────────────────────────────────────────

#[test]
fn counts_skipped_as_passing_nothing_to_run_is_not_a_failure() {
    assert!(report(&[
        ("test", CheckOutcome::Passed),
        ("lint", CheckOutcome::Skipped)
    ])
    .passed());
    assert!(report(&[]).passed());
}

#[test]
fn counts_every_non_pass_as_a_failure_including_a_blocked_setup() {
    for outcome in [
        CheckOutcome::Failed,
        CheckOutcome::TimedOut,
        CheckOutcome::SetupFailed,
    ] {
        assert!(!report(&[("test", outcome)]).passed());
    }
}

// ── cycle bookkeeping (the desktop store's transitions) ────────────────────

#[test]
fn enrolls_clean_with_no_spent_budget_and_nothing_in_flight() {
    let s = AutopilotState::enrolled();
    assert!(s.enrolled);
    assert!(s.cycle.is_none());
    assert!(s.attempts.is_empty());
    assert!(s.situation.is_empty());
    assert!(s.barren.is_empty());
}

#[test]
fn numbers_attempts_from_the_rungs_spent_budget() {
    let mut s = spent(2);
    s.open_cycle(DelegationKind::FixChecks, "sig".into(), situation(), NOW);
    let c = s.cycle.as_ref().unwrap();
    assert_eq!(c.attempt, 3);
    assert_eq!(c.phase, CyclePhase::Working);
    assert_eq!(c.signature, "sig");
}

#[test]
fn starts_the_budget_over_for_a_new_situation_but_remembers_what_it_failed_at() {
    let mut s = AutopilotState {
        barren: vec!["old-world".to_string()],
        ..spent(3)
    };
    s.open_cycle(
        DelegationKind::FixChecks,
        "sig".into(),
        "checks-failing:lint".into(),
        NOW,
    );
    assert_eq!(s.cycle.as_ref().unwrap().attempt, 1);
    assert_eq!(s.situation, "checks-failing:lint");
    assert_eq!(s.barren, ["old-world"]);
}

#[test]
fn records_a_barren_signature_once_and_only_when_given_one() {
    let mut s = AutopilotState::enrolled();
    s.retry(DelegationKind::FixChecks, Some("w1"));
    s.retry(DelegationKind::FixChecks, Some("w1"));
    s.retry(DelegationKind::FixChecks, None);
    assert_eq!(s.barren, ["w1"]);
    assert_eq!(s.attempts[&DelegationKind::FixChecks], 3);
    assert!(s.cycle.is_none());
}

#[test]
fn gives_the_budget_back_on_success() {
    let mut s = spent(2);
    s.open_cycle(DelegationKind::FixChecks, "sig".into(), situation(), NOW);
    s.settle(DelegationKind::FixChecks);
    assert!(s.cycle.is_none());
    assert_eq!(s.attempts[&DelegationKind::FixChecks], 0);
}

#[test]
fn stamps_the_phase_clock_when_evidence_starts_being_awaited() {
    let mut s = AutopilotState::enrolled();
    s.open_cycle(DelegationKind::FixChecks, "sig".into(), situation(), NOW);
    s.advance(CyclePhase::AwaitingEvidence, NOW + 5);
    let c = s.cycle.as_ref().unwrap();
    assert_eq!(c.phase, CyclePhase::AwaitingEvidence);
    assert_eq!(c.phase_since, NOW + 5);
    // No cycle: nothing to advance.
    let mut idle = AutopilotState::enrolled();
    idle.advance(CyclePhase::AwaitingEvidence, NOW);
    assert!(idle.cycle.is_none());
}
