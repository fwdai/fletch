// Ported from the desktop watcher's tests (`src/store/delegationSync.test.ts`,
// `src/store/publishApproval.test.ts`), which pinned the bugs that shaped the
// rules: a delegation dispatched at an agent nobody was looking at never
// advanced (and a running agent's held trigger was never delivered), and a
// multi-repo agent could only hold one delegation. Plus the host-only parts:
// the ops, the driver's pass, and the approval pre-authorization.

use super::*;
use crate::git_state::FileStatus;
use crate::host::sink::RecordingSink;
use crate::supervisor::tests::{
    committed_repo, record_in_checkouts, record_with_status, test_supervisor,
};

const NOW: i64 = 100_000;
const LATE: i64 = NOW + GIVE_UP_GRACE_MS + 1;

fn delegation(kind: DelegationKind) -> Delegation {
    Delegation {
        kind,
        prompt: "[app-action] commit".to_string(),
        started_at: NOW,
        saw_running: false,
        saw_git_op: false,
        queued: false,
    }
}

fn git(files: Vec<FileStatus>, unpushed: u32) -> GitState {
    GitState {
        branch: "feat".to_string(),
        parent_branch: "main".to_string(),
        ahead: 1,
        behind: 0,
        unpushed,
        files,
        additions: 0,
        deletions: 0,
        remote_url: None,
        has_origin: true,
        head_sha: None,
        blocked_config: Vec::new(),
    }
}

fn file(kind: StatusKind) -> FileStatus {
    FileStatus {
        path: "a.ts".to_string(),
        kind,
        staged: false,
        additions: 1,
        deletions: 0,
    }
}

fn clean() -> GitState {
    git(Vec::new(), 0)
}

fn dirty() -> GitState {
    git(vec![file(StatusKind::Modified)], 0)
}

fn pr(state: PrStatus, mergeable: MergeableState) -> PrState {
    PrState {
        number: 7,
        url: "https://github.com/o/r/pull/7".to_string(),
        state,
        title: "t".to_string(),
        mergeable,
        opened_at: None,
        merged_at: None,
    }
}

fn checks(merge_state: MergeState) -> PrChecks {
    PrChecks {
        merge_state,
        rollup: "passing".to_string(),
        total: 1,
        passed: 1,
        failed: 0,
        pending: 0,
        required_failing: Vec::new(),
        runs: Vec::new(),
    }
}

fn key(agent: &str, subdir: Option<&str>) -> Key {
    (agent.to_string(), subdir.map(str::to_string))
}

fn statuses(pairs: &[(&str, AgentStatus)]) -> HashMap<String, AgentStatus> {
    pairs
        .iter()
        .map(|(a, s)| (a.to_string(), s.clone()))
        .collect()
}

fn with_git(pairs: Vec<(Key, GitState)>) -> HashMap<Key, Observed> {
    pairs
        .into_iter()
        .map(|(k, g)| {
            (
                k,
                Observed {
                    git: Some(g),
                    ..Observed::default()
                },
            )
        })
        .collect()
}

fn finish(k: Key, phase: Phase, notice: &'static str) -> Effect {
    Effect::Finish {
        key: k,
        phase,
        notice,
    }
}

// ── the trigger string ─────────────────────────────────────────────────────

/// The shared fixture: `src/delegation.test.ts` pins the TypeScript
/// `appActionMessage` against these same strings, so the host composes exactly
/// what a client used to.
#[test]
fn the_trigger_matches_the_typescript_fixture() {
    type Case<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a str);
    let cases: [Case; 5] = [
        ("commit", &[], "[app-action] commit"),
        (
            "commit-pr",
            &[("base", "main")],
            r#"[app-action] commit-pr base="main""#,
        ),
        (
            "fix-checks",
            &[("failing", "unit, lint"), ("repo", "web")],
            r#"[app-action] fix-checks failing="unit, lint" repo="web""#,
        ),
        // Empty values are dropped, not sent as `key=""`.
        ("open-pr", &[("base", "")], "[app-action] open-pr"),
        (
            "fix-checks",
            &[("failing", r#"say "hi""#)],
            r#"[app-action] fix-checks failing="say \"hi\"""#,
        ),
    ];
    for (action, params, expected) in cases {
        assert_eq!(app_action_message(action, params), expected);
    }
}

#[test]
fn every_kind_round_trips_through_its_playbook_name() {
    for kind in DelegationKind::ALL {
        assert_eq!(DelegationKind::from_action(kind.action()), Some(kind));
    }
    assert_eq!(
        DelegationKind::from_action("resolve-conflicts"),
        Some(DelegationKind::Resolve)
    );
    // The kind's own spelling is not a playbook.
    assert_eq!(DelegationKind::from_action("resolve"), None);
    assert_eq!(DelegationKind::from_action("rm -rf"), None);
    assert_eq!(
        serde_json::to_value(DelegationKind::ResolveComments).unwrap(),
        "resolve-comments"
    );
}

// ── the pure decisions ─────────────────────────────────────────────────────

#[test]
fn only_fix_checks_and_resolve_comments_settle_on_idle() {
    let settling: Vec<DelegationKind> = DelegationKind::ALL
        .into_iter()
        .filter(|k| settles_on_idle(*k))
        .collect();
    assert_eq!(
        settling,
        [DelegationKind::FixChecks, DelegationKind::ResolveComments]
    );
}

#[test]
fn an_op_proves_only_its_own_playbook() {
    use DelegationKind::*;
    let proves = [
        (Commit, "git_commit", true),
        (Commit, "git_push", false),
        (Resolve, "git_commit", true),
        (Resolve, "git_update_branch", true),
        (CommitPush, "git_push", true),
        (CommitPush, "open_pr", false),
        (FixChecks, "git_commit", true),
        (CommitPr, "open_pr", true),
        (CommitPr, "git_push", false),
        (OpenPr, "open_pr", true),
        (OpenPr, "git_commit", false),
        (Push, "git_push", true),
        (ResolveComments, "reply_thread", true),
        (ResolveComments, "resolve_thread", true),
        (ResolveComments, "git_commit", false),
        (UpdateBranch, "git_update_branch", true),
        (UpdateBranch, "git_commit", false),
    ];
    for (kind, op, expected) in proves {
        assert_eq!(action_proves_kind(kind, op), expected, "{kind:?} / {op}");
    }
}

#[test]
fn each_kind_resolves_on_its_own_target() {
    use DelegationKind::*;
    let unpushed = git(Vec::new(), 2);
    let conflicted = git(vec![file(StatusKind::Conflicted)], 0);
    let open = pr(PrStatus::Open, MergeableState::Unknown);

    assert!(delegation_resolved(Commit, Some(&clean()), None, None));
    assert!(!delegation_resolved(Commit, Some(&dirty()), None, None));
    assert!(!delegation_resolved(Commit, None, None, None));

    assert!(delegation_resolved(CommitPush, Some(&clean()), None, None));
    assert!(!delegation_resolved(
        CommitPush,
        Some(&unpushed),
        None,
        None
    ));

    // Clean AND open: a PR already open is not evidence the commit landed.
    assert!(delegation_resolved(
        CommitPr,
        Some(&clean()),
        Some(&open),
        None
    ));
    assert!(!delegation_resolved(
        CommitPr,
        Some(&dirty()),
        Some(&open),
        None
    ));
    assert!(!delegation_resolved(CommitPr, Some(&clean()), None, None));

    assert!(delegation_resolved(OpenPr, None, Some(&open), None));
    assert!(!delegation_resolved(
        OpenPr,
        None,
        Some(&pr(PrStatus::Closed, MergeableState::Unknown)),
        None
    ));

    assert!(delegation_resolved(Push, Some(&dirty()), None, None));
    assert!(!delegation_resolved(Push, Some(&unpushed), None, None));

    assert!(delegation_resolved(Resolve, Some(&dirty()), None, None));
    assert!(!delegation_resolved(Resolve, Some(&conflicted), None, None));

    // The rich merge state wins when present; `unknown` keeps waiting.
    for (state, expected) in [
        (MergeState::Clean, true),
        (MergeState::Blocked, true),
        (MergeState::Behind, false),
        (MergeState::Dirty, false),
        (MergeState::Unknown, false),
    ] {
        assert_eq!(
            delegation_resolved(UpdateBranch, None, Some(&open), Some(&checks(state))),
            expected,
            "{state:?}"
        );
    }
    // Without it, the coarse `mergeable` verdict.
    assert!(delegation_resolved(
        UpdateBranch,
        None,
        Some(&pr(PrStatus::Open, MergeableState::Mergeable)),
        None
    ));
    assert!(!delegation_resolved(UpdateBranch, None, Some(&open), None));

    // Never observable from state.
    for kind in [FixChecks, ResolveComments] {
        assert!(!delegation_resolved(
            kind,
            Some(&clean()),
            Some(&open),
            Some(&checks(MergeState::Clean))
        ));
    }
}

#[test]
fn a_step_needs_the_target_and_the_op_and_a_delivered_trigger() {
    let mut d = delegation(DelegationKind::Commit);
    // Resolved alone is not ours.
    assert_eq!(
        delegation_step(&d, &AgentStatus::Idle, true, NOW),
        Step::Wait
    );
    d.saw_git_op = true;
    assert_eq!(
        delegation_step(&d, &AgentStatus::Running, true, NOW),
        Step::Resolve
    );
    // Still queued: never resolves, whatever the snapshot says.
    d.queued = true;
    assert_eq!(
        delegation_step(&d, &AgentStatus::Idle, true, NOW),
        Step::Dequeue
    );
    assert_eq!(
        delegation_step(&d, &AgentStatus::Spawning, true, NOW),
        Step::Wait
    );
}

#[test]
fn a_step_arms_the_clock_on_our_turn_or_after_the_grace_window() {
    let mut d = delegation(DelegationKind::Commit);
    assert_eq!(
        delegation_step(&d, &AgentStatus::Running, false, NOW),
        Step::MarkRunning
    );
    assert_eq!(
        delegation_step(&d, &AgentStatus::Idle, false, NOW),
        Step::Wait
    );
    assert_eq!(
        delegation_step(&d, &AgentStatus::Idle, false, LATE),
        Step::GiveUp
    );
    d.saw_running = true;
    assert_eq!(
        delegation_step(&d, &AgentStatus::Running, false, NOW),
        Step::Wait
    );
    assert_eq!(
        delegation_step(&d, &AgentStatus::Idle, false, NOW),
        Step::GiveUp
    );
    assert_eq!(
        delegation_step(&d, &AgentStatus::Stopped, false, NOW),
        Step::GiveUp
    );
}

// ── the pass (planDelegationPass) ──────────────────────────────────────────

#[test]
fn resolves_a_delegation_on_an_agent_nobody_is_looking_at() {
    let mut d = delegation(DelegationKind::Commit);
    d.saw_git_op = true;
    d.saw_running = true;
    let effects = plan_pass(
        &BTreeMap::from([(key("a1", None), d)]),
        &statuses(&[("a1", AgentStatus::Idle)]),
        &with_git(vec![(key("a1", None), clean())]),
        NOW,
    );
    assert_eq!(
        effects,
        [finish(
            key("a1", None),
            Phase::Done,
            "Agent committed your changes"
        )]
    );
}

#[test]
fn dequeues_a_held_trigger_once_the_turn_it_waits_behind_settles() {
    let mut d = delegation(DelegationKind::Commit);
    d.queued = true;
    let table = BTreeMap::from([(key("a1", None), d)]);
    let none = HashMap::new();
    assert!(plan_pass(
        &table,
        &statuses(&[("a1", AgentStatus::Running)]),
        &none,
        NOW
    )
    .is_empty());
    assert_eq!(
        plan_pass(&table, &statuses(&[("a1", AgentStatus::Idle)]), &none, NOW),
        [Effect::Dequeue {
            key: key("a1", None)
        }]
    );
}

#[test]
fn gives_up_on_a_settled_agent_that_never_reached_the_target() {
    let mut d = delegation(DelegationKind::Commit);
    d.saw_running = true;
    assert_eq!(
        plan_pass(
            &BTreeMap::from([(key("a1", None), d)]),
            &statuses(&[("a1", AgentStatus::Idle)]),
            &with_git(vec![(key("a1", None), dirty())]),
            NOW,
        ),
        [finish(key("a1", None), Phase::Abandoned, ABANDONED_NOTICE)]
    );
}

#[test]
fn a_settled_unobservable_kind_is_done_not_abandoned() {
    for kind in DelegationKind::ALL
        .into_iter()
        .filter(|k| settles_on_idle(*k))
    {
        let mut d = delegation(kind);
        d.saw_running = true;
        assert_eq!(
            plan_pass(
                &BTreeMap::from([(key("a1", None), d)]),
                &statuses(&[("a1", AgentStatus::Idle)]),
                &HashMap::new(),
                NOW,
            ),
            [finish(key("a1", None), Phase::Done, delegation_done(kind))],
            "{kind:?}"
        );
    }
    assert_eq!(
        delegation_done(DelegationKind::FixChecks),
        "Agent finished — checks are re-running"
    );
}

#[test]
fn drops_a_delegation_whose_agent_is_gone() {
    assert_eq!(
        plan_pass(
            &BTreeMap::from([(key("a1", None), delegation(DelegationKind::Commit))]),
            &HashMap::new(),
            &HashMap::new(),
            NOW,
        ),
        [Effect::DropOrphan {
            key: key("a1", None)
        }]
    );
}

#[test]
fn waits_out_the_grace_window_before_giving_up_unarmed() {
    let table = BTreeMap::from([(key("a1", None), delegation(DelegationKind::Commit))]);
    let idle = statuses(&[("a1", AgentStatus::Idle)]);
    assert!(plan_pass(&table, &idle, &HashMap::new(), NOW).is_empty());
    assert_eq!(
        plan_pass(&table, &idle, &HashMap::new(), LATE),
        [finish(key("a1", None), Phase::Abandoned, ABANDONED_NOTICE)]
    );
}

#[test]
fn judges_each_checkout_against_its_own_state() {
    let mut d = delegation(DelegationKind::Commit);
    d.saw_git_op = true;
    d.saw_running = true;
    let (primary, web) = (key("a1", None), key("a1", Some("web")));
    let effects = plan_pass(
        &BTreeMap::from([(primary.clone(), d.clone()), (web.clone(), d)]),
        &statuses(&[("a1", AgentStatus::Idle)]),
        &with_git(vec![(primary.clone(), clean()), (web.clone(), dirty())]),
        NOW,
    );
    assert_eq!(
        effects,
        [
            finish(primary, Phase::Done, "Agent committed your changes"),
            finish(web, Phase::Abandoned, ABANDONED_NOTICE),
        ]
    );
}

#[test]
fn dequeues_at_most_one_delegation_per_agent_per_pass() {
    let mut d = delegation(DelegationKind::Commit);
    d.queued = true;
    let effects = plan_pass(
        &BTreeMap::from([(key("a1", None), d.clone()), (key("a1", Some("web")), d)]),
        &statuses(&[("a1", AgentStatus::Idle)]),
        &HashMap::new(),
        NOW,
    );
    assert_eq!(
        effects,
        [Effect::Dequeue {
            key: key("a1", None)
        }]
    );
}

#[test]
fn still_dequeues_concurrently_for_different_agents() {
    let mut d = delegation(DelegationKind::Commit);
    d.queued = true;
    let effects = plan_pass(
        &BTreeMap::from([(key("a1", None), d.clone()), (key("a2", None), d)]),
        &statuses(&[("a1", AgentStatus::Idle), ("a2", AgentStatus::Idle)]),
        &HashMap::new(),
        NOW,
    );
    assert_eq!(
        effects,
        [
            Effect::Dequeue {
                key: key("a1", None)
            },
            Effect::Dequeue {
                key: key("a2", None)
            },
        ]
    );
}

// ── the table ──────────────────────────────────────────────────────────────

fn record(table: &Delegations, k: Key, kind: DelegationKind, queued: bool) -> u64 {
    table
        .record(
            k,
            kind,
            format!("[app-action] {}", kind.action()),
            queued,
            NOW,
        )
        .0
}

fn entry(table: &Delegations, k: &Key) -> Delegation {
    table.snapshot()[k].1.clone()
}

#[test]
fn a_git_action_acks_every_checkout_of_that_agent_whose_playbook_it_is() {
    let table = Delegations::default();
    record(&table, key("a1", None), DelegationKind::Commit, false);
    record(
        &table,
        key("a1", Some("web")),
        DelegationKind::Commit,
        false,
    );
    record(&table, key("a2", None), DelegationKind::Commit, false);

    assert!(table.note_git_action("a1", "git_commit"));

    assert!(entry(&table, &key("a1", None)).saw_git_op);
    assert!(entry(&table, &key("a1", Some("web"))).saw_git_op);
    assert!(!entry(&table, &key("a2", None)).saw_git_op);
}

#[test]
fn a_git_action_from_another_playbook_or_while_queued_is_ignored() {
    let table = Delegations::default();
    record(&table, key("a1", None), DelegationKind::Commit, false);
    record(&table, key("a1", Some("web")), DelegationKind::Commit, true);

    assert!(!table.note_git_action("a1", "git_push"));
    assert!(!entry(&table, &key("a1", None)).saw_git_op);

    table.note_git_action("a1", "git_commit");
    assert!(entry(&table, &key("a1", None)).saw_git_op);
    assert!(!entry(&table, &key("a1", Some("web"))).saw_git_op);
}

#[test]
fn a_dequeue_hands_over_one_held_trigger_exactly_once() {
    let table = Delegations::default();
    let id = record(&table, key("a1", Some("web")), DelegationKind::Commit, true);
    record(&table, key("a1", None), DelegationKind::Commit, true);

    let (prompt, shown) = table
        .take_dequeue(&key("a1", Some("web")), id, NOW + 5)
        .expect("held");
    assert_eq!(prompt, "[app-action] commit");
    assert_eq!(shown.phase, Phase::Started);
    assert_eq!(shown.started_at, NOW + 5, "the clock restarts at delivery");
    // Idempotent, so a repeated pass can't double-deliver.
    assert!(table
        .take_dequeue(&key("a1", Some("web")), id, NOW + 6)
        .is_none());
    // The sibling is left alone.
    assert!(entry(&table, &key("a1", None)).queued);
}

#[test]
fn a_pass_cannot_finish_a_delegation_recorded_after_it_decided() {
    let table = Delegations::default();
    let old = record(&table, key("a1", None), DelegationKind::Commit, false);
    record(&table, key("a1", None), DelegationKind::Push, false);
    assert!(table
        .finish(&key("a1", None), old, Phase::Done, None)
        .is_none());
    assert_eq!(entry(&table, &key("a1", None)).kind, DelegationKind::Push);
}

#[test]
fn the_view_names_the_live_phase() {
    let table = Delegations::default();
    record(&table, key("a1", None), DelegationKind::Commit, true);
    record(
        &table,
        key("a2", Some("web")),
        DelegationKind::FixChecks,
        false,
    );
    assert_eq!(
        table.mark_agent_running("a1"),
        [],
        "a queued one's turn is not ours"
    );
    assert_eq!(table.mark_agent_running("a2").len(), 1);
    let phases: Vec<(String, Option<String>, Phase)> = table
        .views()
        .into_iter()
        .map(|v| (v.agent_id, v.subdir, v.phase))
        .collect();
    assert_eq!(
        phases,
        [
            ("a1".to_string(), None, Phase::Queued),
            ("a2".to_string(), Some("web".to_string()), Phase::Running),
        ]
    );
    let wire = serde_json::to_value(&table.views()[1]).unwrap();
    assert_eq!(
        wire,
        serde_json::json!({
            "agent_id": "a2",
            "subdir": "web",
            "kind": "fix-checks",
            "phase": "running",
            "started_at": NOW,
        })
    );
}

// ── publish pre-authorization (publishPreAuthorized's delegation half) ─────

#[test]
fn a_delegation_authorizes_the_publishes_its_own_playbook_performs() {
    use DelegationKind::*;
    for (kind, op) in [
        (Push, "git_push"),
        (CommitPush, "git_push"),
        (FixChecks, "git_push"),
        (OpenPr, "open_pr"),
        (CommitPr, "open_pr"),
    ] {
        let table = Delegations::default();
        record(&table, key("a1", None), kind, false);
        assert!(table.pre_authorizes("a1", None, op), "{kind:?} → {op}");
    }
}

#[test]
fn a_delegation_cannot_launder_a_publish_its_playbook_never_performs() {
    let table = Delegations::default();
    record(&table, key("a1", None), DelegationKind::Commit, false);
    assert!(!table.pre_authorizes("a1", None, "git_push"));
    assert!(!table.pre_authorizes("a1", None, "open_pr"));

    let push_only = Delegations::default();
    record(&push_only, key("a1", None), DelegationKind::Push, false);
    assert!(!push_only.pre_authorizes("a1", None, "open_pr"));
}

#[test]
fn pre_authorization_is_scoped_to_its_checkout_and_needs_a_delivered_trigger() {
    let table = Delegations::default();
    record(&table, key("a1", None), DelegationKind::Push, false);
    assert!(!table.pre_authorizes("a1", Some("web"), "git_push"));
    assert!(!table.pre_authorizes("a2", None, "git_push"));

    let held = Delegations::default();
    record(&held, key("a1", None), DelegationKind::Push, true);
    assert!(
        !held.pre_authorizes("a1", None, "git_push"),
        "the push belongs to the turn it waits behind"
    );
}

#[test]
fn with_no_delegation_nothing_is_authorized() {
    let table = Delegations::default();
    for op in ["git_push", "open_pr"] {
        assert!(!table.pre_authorizes("a1", None, op));
    }
}

// ── the op and the driver's pass, against a supervisor ─────────────────────

fn changes(sink: &RecordingSink) -> Vec<serde_json::Value> {
    sink.events()
        .into_iter()
        .filter(|(name, _)| name == EVENT_CHANGED)
        .map(|(_, payload)| payload)
        .collect()
}

fn turns_sent(sink: &RecordingSink) -> Vec<String> {
    sink.events()
        .into_iter()
        .filter(|(name, _)| name == "turn:sent")
        .filter_map(|(_, payload)| payload["text"].as_str().map(str::to_string))
        .collect()
}

/// An idle agent with two checkouts (`repo-0` primary, `repo-1` secondary) and
/// no process: a send gets as far as `turn:sent` and is then held.
async fn idle_agent(dir: &std::path::Path, id: &str) -> Arc<Supervisor> {
    let primary = committed_repo(dir, "primary").await;
    let secondary = committed_repo(dir, "secondary").await;
    let sup = Arc::new(test_supervisor());
    let mut record = record_in_checkouts(&sup, id, &[primary, secondary]);
    record.session_id = None;
    sup.workspace.add_agent(&mut record).unwrap();
    sup
}

#[tokio::test]
async fn delegating_to_an_idle_agent_sends_the_trigger_as_a_turn() {
    let td = tempfile::tempdir().unwrap();
    let sup = idle_agent(td.path(), "yukon").await;
    let (ctx, sink, _dir) = crate::host::ctx::test_ctx();
    let table = Delegations::default();

    let shown = delegate(
        &table,
        &sup,
        &ctx,
        "yukon",
        Some("repo-1"),
        "fix-checks",
        &BTreeMap::from([
            ("failing".to_string(), "unit".to_string()),
            // A caller cannot aim the playbook at another checkout.
            ("repo".to_string(), "elsewhere".to_string()),
        ]),
    )
    .await
    .expect("delegated");

    assert_eq!(shown.kind, DelegationKind::FixChecks);
    assert_eq!(shown.phase, Phase::Started);
    assert_eq!(
        turns_sent(&sink),
        [r#"[app-action] fix-checks failing="unit" repo="repo-1""#]
    );
    let events = changes(&sink);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["phase"], "started");
    assert_eq!(events[0]["subdir"], "repo-1");
    assert!(!entry(&table, &key("yukon", Some("repo-1"))).queued);
}

#[tokio::test]
async fn delegating_to_a_running_agent_holds_the_trigger() {
    let sup = Arc::new(test_supervisor());
    let mut rec = record_with_status("yangtze", AgentStatus::Running);
    sup.workspace.add_agent(&mut rec).unwrap();
    sup.statuses
        .lock()
        .insert("yangtze".to_string(), AgentStatus::Running);
    let (ctx, sink, _dir) = crate::host::ctx::test_ctx();
    let table = Delegations::default();

    // The primary named by its own subdir is still the primary.
    let shown = delegate(
        &table,
        &sup,
        &ctx,
        "yangtze",
        Some("repo"),
        "commit",
        &BTreeMap::new(),
    )
    .await
    .expect("delegated");

    assert_eq!(shown.phase, Phase::Queued);
    assert_eq!(shown.subdir, None);
    assert!(
        turns_sent(&sink).is_empty(),
        "nothing is written into the running turn"
    );
    assert_eq!(changes(&sink)[0]["phase"], "queued");
    assert_eq!(
        entry(&table, &key("yangtze", None)).prompt,
        "[app-action] commit"
    );
}

#[tokio::test]
async fn delegate_refuses_an_unknown_action_or_repo() {
    let sup = Arc::new(test_supervisor());
    let mut rec = record_with_status("yarra", AgentStatus::Idle);
    sup.workspace.add_agent(&mut rec).unwrap();
    let (ctx, sink, _dir) = crate::host::ctx::test_ctx();
    let table = Delegations::default();

    let err = delegate(
        &table,
        &sup,
        &ctx,
        "yarra",
        None,
        "rebase-everything",
        &BTreeMap::new(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("unknown git action"), "{err}");
    let err = delegate(
        &table,
        &sup,
        &ctx,
        "yarra",
        Some("nope"),
        "commit",
        &BTreeMap::new(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("unknown repo"), "{err}");
    let err = delegate(
        &table,
        &sup,
        &ctx,
        "ghost",
        None,
        "commit",
        &BTreeMap::new(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, Error::AgentNotFound(_)), "{err}");

    assert!(table.is_empty());
    assert!(sink.events().is_empty());
}

/// The rule the desktop guarded and the phone did not: a trigger held behind a
/// running turn is delivered as its own turn once the agent settles.
#[tokio::test]
async fn a_pass_delivers_the_held_trigger_once_the_agent_settles() {
    let td = tempfile::tempdir().unwrap();
    let sup = idle_agent(td.path(), "yenisei").await;
    let (ctx, sink, _dir) = crate::host::ctx::test_ctx();
    let table = Delegations::default();
    sup.statuses
        .lock()
        .insert("yenisei".to_string(), AgentStatus::Running);
    delegate(
        &table,
        &sup,
        &ctx,
        "yenisei",
        None,
        "commit",
        &BTreeMap::new(),
    )
    .await
    .unwrap();

    run_pass(&table, &ctx, &sup).await;
    assert!(turns_sent(&sink).is_empty(), "still running: still held");

    sup.statuses
        .lock()
        .insert("yenisei".to_string(), AgentStatus::Idle);
    run_pass(&table, &ctx, &sup).await;
    assert_eq!(turns_sent(&sink), ["[app-action] commit"]);
    let phases: Vec<String> = changes(&sink)
        .iter()
        .map(|e| e["phase"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(phases, ["queued", "started"]);

    // Delivered once: the next pass waits for the turn instead of resending.
    run_pass(&table, &ctx, &sup).await;
    assert_eq!(turns_sent(&sink).len(), 1);
}

#[tokio::test]
async fn a_pass_drops_the_delegations_of_an_agent_that_is_gone() {
    let sup = Arc::new(test_supervisor());
    let (ctx, sink, _dir) = crate::host::ctx::test_ctx();
    let table = Delegations::default();
    record(&table, key("vanished", None), DelegationKind::Commit, false);

    run_pass(&table, &ctx, &sup).await;

    assert!(table.is_empty());
    let events = changes(&sink);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["phase"], "abandoned");
    assert!(
        events[0].get("notice").is_none(),
        "nothing to tell about a gone agent"
    );
}
