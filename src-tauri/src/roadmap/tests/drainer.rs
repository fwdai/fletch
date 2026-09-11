
use super::*;
use crate::roadmap::types::{Horizon, ItemSource};

fn item(code: &str, rank: f64) -> RoadmapItem {
    RoadmapItem {
        id: format!("id-{code}"),
        project_id: "p1".into(),
        code: code.into(),
        title: format!("do {code}"),
        why: String::new(),
        horizon: Horizon::Next,
        status: ItemStatus::Queued,
        rank,
        area: None,
        source: ItemSource::User,
        accept: Vec::new(),
        deps: Vec::new(),
        agent_id: None,
        workflow_def_id: None,
        run_id: None,
        pr_url: None,
        pr_number: None,
        hold_reason: None,
        held_by: None,
        held_at: None,
        close_reason: None,
        issue_url: None,
        created_at: 0,
        updated_at: 0,
    }
}

fn codes(list: &[&str]) -> HashSet<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

fn pr(number: Option<i64>) -> FinalizedPr {
    FinalizedPr {
        url: "https://github.com/o/r/pull/42".into(),
        number,
    }
}

#[test]
fn the_queue_follows_rank() {
    let mut first = item("FLT-101", 1.0);
    first.horizon = Horizon::Later;
    let mut second = item("FLT-100", 2.0);
    second.horizon = Horizon::Now;
    let queue = vec![first, second];

    assert_eq!(
        pick_next(&queue, 0, 1, &codes(&[]), &codes(&["FLT-100", "FLT-101"])),
        Decision::Dispatch(0)
    );
}

#[test]
fn an_empty_queue_decides_nothing() {
    assert_eq!(
        pick_next(&[], 0, 1, &codes(&[]), &codes(&[])),
        Decision::Empty
    );
}

/// can't quietly drop one and leave the queue dispatching work it must not.
#[test]
fn a_held_a_handed_off_and_an_unqueued_row_are_not_in_the_queue() {
    let ready = item("FLT-100", 1.0);

    let mut held = item("FLT-101", 2.0);
    held.hold_reason = Some("direction unclear".into());
    held.held_by = Some(EventActor::Pm);
    held.held_at = Some(1);
    assert!(held.is_held());

    let mut handed = item("FLT-102", 3.0);
    handed.agent_id = Some("w1".into());

    let mut open = item("FLT-103", 4.0);
    open.status = ItemStatus::Open;

    let board = vec![ready.clone(), held, handed, open];
    assert_eq!(
        dispatchable(&board)
            .iter()
            .map(|i| i.code.clone())
            .collect::<Vec<_>>(),
        vec![ready.code],
        "only the plain queued row is claimable"
    );
}

#[test]
fn a_held_head_is_skipped_and_the_next_item_still_dispatches() {
    let mut held = item("FLT-100", 1.0);
    held.hold_reason = Some("wrong scope".into());
    let ready = item("FLT-101", 2.0);

    let queue = dispatchable(&[held, ready]);
    assert_eq!(queue.len(), 1);
    assert_eq!(
        pick_next(&queue, 0, 1, &codes(&[]), &codes(&["FLT-100", "FLT-101"])),
        Decision::Dispatch(0),
    );
    assert_eq!(queue[0].code, "FLT-101");
}

#[test]
fn a_held_done_item_is_not_a_landed_dependency() {
    let mut shipped = item("FLT-100", 1.0);
    shipped.status = ItemStatus::Done;
    let mut shipped_but_held = item("FLT-101", 2.0);
    shipped_but_held.status = ItemStatus::Done;
    shipped_but_held.hold_reason = Some("we agreed something else".into());
    // Not done at all, held or otherwise.
    let mut open = item("FLT-102", 3.0);
    open.status = ItemStatus::Open;
    let mut held_open = item("FLT-103", 4.0);
    held_open.status = ItemStatus::Open;
    held_open.hold_reason = Some("direction".into());

    assert_eq!(
        done_codes(&[shipped, shipped_but_held, open, held_open]),
        codes(&["FLT-100"]),
        "a held item satisfies nobody's dependency, however it got to done"
    );
}

/// item because its PR merged (which is reality, and correct), and the dependant
/// behind it must *still* be blocked. Before `done_codes` this dispatched.
#[test]
fn a_dependant_of_a_held_done_item_stays_blocked() {
    let mut dep = item("FLT-100", 1.0);
    dep.status = ItemStatus::Done;
    dep.hold_reason = Some("we agreed something else".into());
    dep.held_by = Some(EventActor::Pm);
    let mut dependant = item("FLT-101", 2.0);
    dependant.deps = vec!["FLT-100".into()];

    let board = vec![dep.clone(), dependant.clone()];
    let known = codes(&["FLT-100", "FLT-101"]);
    let queue = dispatchable(&board);
    assert_eq!(queue.len(), 1, "only the dependant is queued");
    assert_eq!(
        pick_next(&queue, 0, 1, &done_codes(&board), &known),
        Decision::Blocked {
            item_id: "id-FLT-101".into(),
            waiting_on: vec!["FLT-100".into()],
        },
        "the hold survived onto the done row, so the work behind it waits"
    );

    dep.hold_reason = None;
    dep.held_by = None;
    let released = vec![dep, dependant];
    assert_eq!(
        pick_next(
            &dispatchable(&released),
            0,
            1,
            &done_codes(&released),
            &known
        ),
        Decision::Dispatch(0)
    );
}

/// Ruled off the board is not shipped: a dependant must never fork on work
/// still exists), so the dependant blocks — it does not sail through the
#[test]
fn a_rejected_item_is_never_a_landed_dependency() {
    let mut dep = item("FLT-100", 1.0);
    dep.status = ItemStatus::Rejected;
    dep.close_reason = Some("out of scope".into());
    let mut dependant = item("FLT-101", 2.0);
    dependant.deps = vec!["FLT-100".into()];

    let board = vec![dep, dependant];
    let known = codes(&["FLT-100", "FLT-101"]);
    assert!(
        done_codes(&board).is_empty(),
        "a rejected item satisfies nobody's dependency"
    );
    assert_eq!(
        pick_next(&dispatchable(&board), 0, 1, &done_codes(&board), &known),
        Decision::Blocked {
            item_id: "id-FLT-101".into(),
            waiting_on: vec!["FLT-100".into()],
        }
    );
}

#[test]
fn a_done_dependency_lets_an_item_through() {
    let mut it = item("FLT-101", 10.0);
    it.deps = vec!["FLT-100".into()];

    assert_eq!(
        pick_next(
            &[it],
            0,
            1,
            &codes(&["FLT-100"]),
            &codes(&["FLT-100", "FLT-101"])
        ),
        Decision::Dispatch(0)
    );
}

#[test]
fn an_in_review_dependency_still_blocks() {
    let mut it = item("FLT-101", 10.0);
    it.deps = vec!["FLT-100".into()];

    assert_eq!(
        pick_next(
            &[it],
            0,
            1,
            &codes(&[]),
            &codes(&["FLT-100", "FLT-101"])
        ),
        Decision::Blocked {
            item_id: "id-FLT-101".into(),
            waiting_on: vec!["FLT-100".into()],
        }
    );
}

#[test]
fn a_dependency_that_no_longer_exists_counts_as_satisfied() {
    // never ships — waiting for it would block this one forever.
    let mut it = item("FLT-101", 10.0);
    it.deps = vec!["FLT-100".into()];

    assert_eq!(
        pick_next(&[it], 0, 1, &codes(&[]), &codes(&["FLT-101"])),
        Decision::Dispatch(0)
    );
}

#[test]
fn a_blocked_head_does_not_block_the_rest_of_the_queue() {
    // Skipped, never failed: FLT-100's turn comes when its dep lands.
    let mut blocked = item("FLT-100", 10.0);
    blocked.deps = vec!["FLT-099".into()];
    let ready = item("FLT-101", 20.0);

    assert_eq!(
        pick_next(
            &[blocked, ready],
            0,
            1,
            &codes(&[]),
            &codes(&["FLT-099", "FLT-100", "FLT-101"])
        ),
        Decision::Dispatch(1)
    );
}

#[test]
fn an_all_blocked_queue_reports_the_head_and_what_it_waits_on() {
    let mut head = item("FLT-100", 10.0);
    head.deps = vec!["FLT-098".into(), "FLT-099".into()];
    let mut tail = item("FLT-101", 20.0);
    tail.deps = vec!["FLT-100".into()];

    let known = codes(&["FLT-098", "FLT-099", "FLT-100", "FLT-101"]);
    assert_eq!(
        pick_next(&[head, tail], 0, 1, &codes(&["FLT-098"]), &known),
        Decision::Blocked {
            item_id: "id-FLT-100".into(),
            waiting_on: vec!["FLT-099".into()],
        }
    );
}

#[test]
fn unsatisfied_deps_reports_only_the_live_unlanded_ones() {
    let deps = vec![
        "FLT-100".to_string(), // done
        "FLT-101".to_string(), // exists, not done
        "FLT-102".to_string(), // deleted
    ];
    assert_eq!(
        unsatisfied_deps(&deps, &codes(&["FLT-100"]), &codes(&["FLT-100", "FLT-101"])),
        vec!["FLT-101".to_string()]
    );
}

#[test]
fn the_cap_holds_the_queue_even_with_a_ready_item() {
    let ready = item("FLT-100", 10.0);
    assert_eq!(
        pick_next(
            &[ready],
            MAX_CONCURRENT_ROADMAP_RUNS,
            MAX_CONCURRENT_ROADMAP_RUNS,
            &codes(&[]),
            &codes(&["FLT-100"])
        ),
        Decision::AtCapacity
    );
}

#[test]
fn capacity_is_checked_before_dependencies() {
    // An at-capacity project says so rather than reporting a dep block the user
    // can't act on — the item may well be unblocked by the time a slot frees.
    let mut blocked = item("FLT-100", 10.0);
    blocked.deps = vec!["FLT-099".into()];
    assert_eq!(
        pick_next(
            &[blocked],
            MAX_CONCURRENT_ROADMAP_RUNS,
            MAX_CONCURRENT_ROADMAP_RUNS,
            &codes(&[]),
            &codes(&["FLT-099", "FLT-100"])
        ),
        Decision::AtCapacity
    );
}

#[test]
fn a_raised_cap_dispatches_a_second_independent_item() {
    let queue = vec![item("FLT-100", 1.0), item("FLT-101", 2.0)];
    let known = codes(&["FLT-100", "FLT-101"]);

    assert_eq!(
        pick_next(&queue, 0, 2, &codes(&[]), &known),
        Decision::Dispatch(0)
    );
    assert_eq!(
        pick_next(&queue[1..], 1, 2, &codes(&[]), &known),
        Decision::Dispatch(0)
    );
    assert_eq!(
        pick_next(&queue[1..], 2, 2, &codes(&[]), &known),
        Decision::AtCapacity
    );
}

/// stays blocked with a free slot going spare — deps serialize dependants, which
#[test]
fn a_dependant_stays_blocked_however_high_the_cap() {
    let mut dependant = item("FLT-101", 2.0);
    dependant.deps = vec!["FLT-100".into()];
    let known = codes(&["FLT-100", "FLT-101"]);

    assert_eq!(
        pick_next(
            &[dependant.clone()],
            1,
            MAX_CONCURRENT_ROADMAP_CEILING,
            &codes(&[]),
            &known
        ),
        Decision::Blocked {
            item_id: "id-FLT-101".into(),
            waiting_on: vec!["FLT-100".into()],
        }
    );
    assert_eq!(
        pick_next(
            &[dependant],
            1,
            MAX_CONCURRENT_ROADMAP_CEILING,
            &codes(&["FLT-100"]),
            &known
        ),
        Decision::Dispatch(0)
    );
}

/// dispatch, then capacity — never two, however much is ready.
#[test]
fn the_default_cap_still_dispatches_one_at_a_time() {
    let queue = vec![item("FLT-100", 1.0), item("FLT-101", 2.0)];
    let known = codes(&["FLT-100", "FLT-101"]);
    assert_eq!(MAX_CONCURRENT_ROADMAP_RUNS, 1);
    assert_eq!(
        pick_next(&queue, 0, MAX_CONCURRENT_ROADMAP_RUNS, &codes(&[]), &known),
        Decision::Dispatch(0)
    );
    assert_eq!(
        pick_next(
            &queue[1..],
            1,
            MAX_CONCURRENT_ROADMAP_RUNS,
            &codes(&[]),
            &known
        ),
        Decision::AtCapacity
    );
}

#[test]
fn the_cap_setting_is_parsed_clamped_and_defaulted() {
    assert_eq!(parse_cap(None), MAX_CONCURRENT_ROADMAP_RUNS);
    assert_eq!(parse_cap(Some("1")), 1);
    assert_eq!(parse_cap(Some("4")), 4);
    assert_eq!(parse_cap(Some("12")), MAX_CONCURRENT_ROADMAP_CEILING);
    for bad in ["0", "-1", "two", "3.5", "", "1e3"] {
        assert_eq!(
            parse_cap(Some(bad)),
            MAX_CONCURRENT_ROADMAP_RUNS,
            "{bad:?} should read as the default"
        );
    }
}

#[test]
fn the_boolean_dials_read_both_answers_and_fall_back() {
    for on in ["1", "true", "on", "yes", "TRUE", "On"] {
        assert!(parse_flag(Some(on), false), "{on} should read as on");
    }
    for off in ["0", "false", "off", "no", "FALSE", "Off"] {
        assert!(!parse_flag(Some(off), true), "{off} should read as off");
    }
    assert!(!parse_flag(None, false));
    assert!(parse_flag(None, true));
    assert!(!parse_flag(Some("maybe"), false));
    assert!(parse_flag(Some("maybe"), true));
}

#[test]
fn every_dial_is_declared_on_both_sides_of_the_wire() {
    const TS: &str = include_str!("../../../../src/components/ProjectScreen/Roadmap/autonomy.ts");
    for expected in [
        format!("export const AUTOQUEUE_KEY = {AUTOQUEUE_KEY:?};"),
        format!("export const MAX_CONCURRENT_KEY = {MAX_CONCURRENT_KEY:?};"),
        format!(
            "export const SETTLE_REVIEW_KEY = {:?};",
            crate::roadmap::review::SETTLE_REVIEW_KEY
        ),
        format!("export const DEFAULT_MAX_CONCURRENT = {MAX_CONCURRENT_ROADMAP_RUNS};"),
        format!("export const MAX_CONCURRENT_CEILING = {MAX_CONCURRENT_ROADMAP_CEILING};"),
    ] {
        assert!(
            TS.contains(&expected),
            "autonomy.ts must declare `{expected}` — the host reads what it writes"
        );
    }
}

#[test]
fn the_cap_is_read_per_project() {
    let conn = test_conn();
    conn.execute(
        "INSERT INTO projects (id, name, created_at) VALUES ('p1', 'fletch', 0), ('p2', 'o', 0)",
        [],
    )
    .unwrap();
    assert_eq!(concurrency_cap(&conn, "p1"), MAX_CONCURRENT_ROADMAP_RUNS);
    assert!(!autoqueue(&conn, "p1"));

    for (project, value) in [("p1", "3"), ("p2", "nonsense")] {
        conn.execute(
            "INSERT INTO project_settings (project_id, key, value) VALUES (?1, ?2, ?3)",
            rusqlite::params![project, MAX_CONCURRENT_KEY, value],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO project_settings (project_id, key, value) VALUES ('p1', ?1, '1')",
        rusqlite::params![AUTOQUEUE_KEY],
    )
    .unwrap();

    assert_eq!(concurrency_cap(&conn, "p1"), 3);
    assert_eq!(concurrency_cap(&conn, "p2"), MAX_CONCURRENT_ROADMAP_RUNS);
    assert!(autoqueue(&conn, "p1"));
    assert!(!autoqueue(&conn, "p2"));
}

#[test]
fn an_items_own_workflow_wins_over_the_project_default() {
    let mut it = item("FLT-100", 10.0);
    it.workflow_def_id = Some("wf-item".into());
    assert_eq!(
        resolve_workflow(&it, Some("wf-default")),
        Some("wf-item".to_string())
    );
}

#[test]
fn an_item_without_one_falls_back_to_the_project_default() {
    let it = item("FLT-100", 10.0);
    assert_eq!(
        resolve_workflow(&it, Some("wf-default")),
        Some("wf-default".to_string())
    );
}

#[test]
fn no_workflow_anywhere_resolves_to_nothing() {
    assert_eq!(resolve_workflow(&item("FLT-100", 10.0), None), None);
}

#[test]
fn a_live_run_leaves_its_item_alone() {
    for status in [RunStatus::Pending, RunStatus::Running, RunStatus::Paused] {
        assert_eq!(settle(Some(status), None), Settlement::Running);
    }
}

#[test]
fn a_finished_run_with_a_pr_lands_in_review() {
    assert_eq!(
        settle(Some(RunStatus::Done), Some(&pr(Some(42)))),
        Settlement::InReview
    );
}

#[test]
fn a_pr_whose_number_never_landed_still_reaches_review() {
    assert_eq!(
        settle(Some(RunStatus::Done), Some(&pr(None))),
        Settlement::InReview
    );
}

#[test]
fn a_finished_run_with_no_pr_is_simply_done() {
    assert_eq!(settle(Some(RunStatus::Done), None), Settlement::Done);
}

#[test]
fn a_lost_run_releases_its_item_back_to_the_board() {
    // Never back to `queued`: an auto-retry loop on a failing workflow burns
    assert_eq!(
        settle(Some(RunStatus::Failed), None),
        Settlement::Released(RUN_FAILED)
    );
    assert_eq!(
        settle(Some(RunStatus::Canceled), None),
        Settlement::Released(RUN_CANCELED)
    );
    assert_eq!(settle(None, None), Settlement::Released(RUN_DELETED));
}

fn test_conn() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    crate::database::get_migrations()
        .to_latest(&mut conn)
        .unwrap();
    conn
}

fn run(conn: &Connection, id: &str, item_id: &str, status: &str, created_at: i64) {
    conn.execute(
        "INSERT INTO wf_run (id, name, spec_json, task, project_id, repo_path, run_dir, branch,
                             base_sha, status, budgets_json, spent_json, created_at, updated_at,
                             roadmap_item_id)
         VALUES (?1, 'n', '{}', 't', 'p1', '/r', '/d', 'wf/x', 'sha', ?2, '{}', '{}', ?3, ?3, ?4)",
        rusqlite::params![id, status, created_at, item_id],
    )
    .unwrap();
}

#[test]
fn recovery_adopts_only_a_live_run() {
    let conn = test_conn();
    run(&conn, "old-done", "id-FLT-100", "done", 10);
    run(&conn, "live", "id-FLT-100", "running", 20);
    assert_eq!(
        dispatched_run_id(&conn, "id-FLT-100"),
        Some("live".to_string())
    );

    run(&conn, "newest-failed", "id-FLT-101", "failed", 30);
    run(&conn, "older-canceled", "id-FLT-101", "canceled", 20);
    assert_eq!(dispatched_run_id(&conn, "id-FLT-101"), None);

    assert_eq!(dispatched_run_id(&conn, "id-FLT-999"), None);
}

fn db_item(conn: &Connection, status: ItemStatus) -> RoadmapItem {
    conn.execute(
        "INSERT OR IGNORE INTO projects (id, name, created_at) VALUES ('p1', 'fletch', 0)",
        [],
    )
    .unwrap();
    store::create(
        conn,
        "p1",
        &crate::roadmap::types::NewItem {
            title: "it".into(),
            status: Some(status),
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn each_settlement_names_its_event() {
    let with_pr = pr(Some(42));
    assert_eq!(settlement_event(&Settlement::Running, None), None);
    assert_eq!(
        settlement_event(&Settlement::InReview, Some(&with_pr)),
        Some((EventKind::PrOpened, Some(with_pr.url.clone())))
    );
    assert_eq!(
        settlement_event(&Settlement::Done, None),
        Some((EventKind::Shipped, None))
    );
    assert_eq!(
        settlement_event(&Settlement::Released(RUN_FAILED), None),
        Some((EventKind::RunFailed, Some(RUN_FAILED.to_string())))
    );
}

/// somebody deleted. All three used to land as `run_failed`, so three deliberate
#[test]
fn each_way_a_run_ends_names_its_own_fact() {
    assert_eq!(release_kind(RUN_FAILED), EventKind::RunFailed);
    assert_eq!(release_kind(RUN_CANCELED), EventKind::RunCanceled);
    assert_eq!(release_kind(RUN_DELETED), EventKind::RunDeleted);
    // Nothing ran, and not because anyone decided so: these are failures of the
    assert_eq!(release_kind(RUN_NEVER_STARTED), EventKind::RunFailed);
    assert_eq!(release_kind(RUN_UNLAUNCHABLE), EventKind::RunFailed);
    assert_eq!(release_kind("something new"), EventKind::RunFailed);

    for (status, kind) in [
        (RunStatus::Failed, EventKind::RunFailed),
        (RunStatus::Canceled, EventKind::RunCanceled),
    ] {
        let (got, _) = settlement_event(&settle(Some(status), None), None).expect("an ending");
        assert_eq!(got, kind, "{status:?}");
    }
    let (deleted, detail) = settlement_event(&settle(None, None), None).expect("an ending");
    assert_eq!(deleted, EventKind::RunDeleted);
    assert_eq!(detail.as_deref(), Some(RUN_DELETED));
}

#[test]
fn each_settlement_names_its_patch() {
    let with_pr = pr(Some(42));

    let review = settlement_patch(&Settlement::InReview, Some(&with_pr));
    assert_eq!(review.status, Some(ItemStatus::InReview));
    // Copied onto the item so the sweep never has to join back to the run.
    assert_eq!(review.pr_url, Some(Some(with_pr.url.clone())));
    assert_eq!(review.pr_number, Some(Some(42)));

    assert_eq!(
        settlement_patch(&Settlement::Done, None).status,
        Some(ItemStatus::Done)
    );

    // Back to the board, never to `queued` — and the run link is dropped so a
    let released = settlement_patch(&Settlement::Released(RUN_CANCELED), None);
    assert_eq!(released.status, Some(ItemStatus::Open));
    assert_eq!(released.run_id, Some(None));

    let running = settlement_patch(&Settlement::Running, None);
    assert_eq!(running.status, None);
    assert_eq!(running.run_id, None);
}

#[test]
fn a_launch_that_never_started_is_an_ending_like_any_other() {
    let ending = Settlement::Released(RUN_UNLAUNCHABLE);

    let patch = settlement_patch(&ending, None);
    assert_eq!(patch.status, Some(ItemStatus::Open));
    assert_eq!(patch.run_id, Some(None));

    let (kind, detail) = settlement_event(&ending, None).expect("an ending records");
    assert_eq!(kind, EventKind::RunFailed);
    assert_eq!(detail.as_deref(), Some(RUN_UNLAUNCHABLE));

    assert_eq!(
        review::outcome_for(&ending, None),
        Some(review::Outcome::Failed(RUN_UNLAUNCHABLE.to_string())),
        "every way out of `active` reaches the PM"
    );
}

#[test]
fn a_claim_records_one_dispatched_event_naming_the_workflow() {
    let conn = test_conn();
    let it = db_item(&conn, ItemStatus::Queued);

    let (claimed, event) = claim_item(&conn, &it.id, "wf-1", Some("Build & review"))
        .unwrap()
        .expect("claims");
    assert_eq!(claimed.status, ItemStatus::Active);
    assert_eq!(claimed.workflow_def_id.as_deref(), Some("wf-1"));
    assert_eq!(event.kind, EventKind::Dispatched);
    assert_eq!(event.actor, EventActor::Drainer);
    assert_eq!(event.detail.as_deref(), Some("Build & review"));
    assert_eq!(events::list_for_item(&conn, &it.id).unwrap(), vec![event]);

    assert!(claim_item(&conn, &it.id, "wf-1", Some("Build & review"))
        .unwrap()
        .is_none());
    assert_eq!(events::list_for_item(&conn, &it.id).unwrap().len(), 1);
}

#[test]
fn a_claim_falls_back_to_the_definition_id_when_the_name_is_gone() {
    let conn = test_conn();
    let it = db_item(&conn, ItemStatus::Queued);
    let (_, event) = claim_item(&conn, &it.id, "wf-orphan", None)
        .unwrap()
        .expect("claims");
    assert_eq!(event.detail.as_deref(), Some("wf-orphan"));
}

#[test]
fn a_definition_name_is_read_off_the_row_or_reported_missing() {
    let conn = test_conn();
    conn.execute(
        "INSERT INTO wf_definition (id, name, spec_json, created_at, updated_at)
         VALUES ('wf-1', 'Build & review', '{}', 0, 0), ('wf-blank', '  ', '{}', 0, 0)",
        [],
    )
    .unwrap();
    assert_eq!(
        definition_name(&conn, "wf-1"),
        Some("Build & review".to_string())
    );
    assert_eq!(definition_name(&conn, "wf-blank"), None);
    assert_eq!(definition_name(&conn, "wf-gone"), None);
}

#[test]
fn a_release_persists_its_reason_where_the_note_never_lands() {
    let conn = test_conn();
    let it = db_item(&conn, ItemStatus::Active);

    let (row, event) = apply_and_record(
        &conn,
        &it.id,
        None,
        &ItemPatch {
            status: Some(ItemStatus::Open),
            run_id: Some(None),
            ..Default::default()
        },
        EventActor::Drainer,
        EventKind::RunFailed,
        Some("its run failed".to_string()),
    )
    .unwrap()
    .expect("the item is there to release");
    assert_eq!(row.status, ItemStatus::Open);

    let listed = events::list_for_item(&conn, &it.id).unwrap();
    assert_eq!(listed, vec![event]);
    assert_eq!(listed[0].kind, EventKind::RunFailed);
    assert_eq!(listed[0].detail.as_deref(), Some("its run failed"));

    let by_kind = |kind: &str| -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM roadmap_item_events WHERE item_id = ?1 AND kind = ?2",
            rusqlite::params![it.id, kind],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(by_kind("run_failed"), 1);
    assert_eq!(by_kind("note"), 0);
}

#[test]
fn a_conditional_verdict_that_misses_records_nothing() {
    let conn = test_conn();
    let it = db_item(&conn, ItemStatus::Queued);

    let outcome = apply_and_record(
        &conn,
        &it.id,
        Some(ItemStatus::InReview),
        &ItemPatch {
            status: Some(ItemStatus::Done),
            ..Default::default()
        },
        EventActor::Sweep,
        EventKind::Shipped,
        None,
    )
    .unwrap();
    assert!(outcome.is_none());
    assert!(events::list_for_item(&conn, &it.id).unwrap().is_empty());
}

/// hand-written JSON so it cannot quietly drift out of validity — `definition_spec`
fn minimal_spec() -> String {
    use crate::workflow::spec::{AgentSpec, Block, Gate, Step};
    let mut agents = std::collections::BTreeMap::new();
    agents.insert(
        "coder".to_string(),
        AgentSpec {
            base: "claude".into(),
            model: None,
            effort: None,
            instructions: None,
            skills: vec![],
            mcp_servers: vec![],
            custom_agent: None,
        },
    );
    serde_json::to_string(&Spec {
        version: 1,
        name: "t".into(),
        description: None,
        budgets: None,
        agents,
        workflow: vec![Block::Step(Step {
            id: "build".into(),
            agent: "coder".into(),
            goal: "do it".into(),
            gate: Gate::Verdict,
            budgets: None,
            comms: vec![],
        })],
        finalize: None,
    })
    .unwrap()
}

fn set_deps(conn: &Connection, item: &RoadmapItem, codes: &[&str]) {
    store::update(
        conn,
        &item.id,
        &ItemPatch {
            deps: Some(codes.iter().map(|c| (*c).to_string()).collect()),
            ..Default::default()
        },
    )
    .unwrap();
}

#[test]
fn a_wedged_queue_head_records_one_blocked_event_not_one_per_tick() {
    // was the only trace. That is the durable line `EventKind::Blocked` exists
    // for — and it must land once, not once every fifteen seconds.
    let conn = test_conn();
    let a = db_item(&conn, ItemStatus::Queued);
    let b = db_item(&conn, ItemStatus::Queued);
    set_deps(&conn, &a, &[&b.code]);
    set_deps(&conn, &b, &[&a.code]);

    let Claim::Note {
        item,
        text,
        recorded,
    } = plan_and_claim(&conn, "p1", 1)
    else {
        panic!("expected a note about the wedged head");
    };
    assert_eq!(item.id, a.id, "the head of the queue is what's wedged");
    assert!(
        text.contains(&format!("{} → {} → {}", a.code, b.code, a.code)),
        "the loop is named, not just the wait: {text}"
    );
    let event = recorded.expect("a blockage that never resolves is durable");
    assert_eq!(event.kind, EventKind::Blocked);
    assert_eq!(event.actor, EventActor::Drainer);
    assert_eq!(event.detail.as_deref(), Some(text.as_str()));

    let Claim::Note { recorded, .. } = plan_and_claim(&conn, "p1", 1) else {
        panic!("expected a note");
    };
    assert!(
        recorded.is_none(),
        "a durable line must not repeat per tick"
    );
    assert_eq!(events::list_for_item(&conn, &a.id).unwrap().len(), 1);
}

#[test]
fn a_dependant_of_a_rejected_item_wedges_durably() {
    let conn = test_conn();
    let dep = db_item(&conn, ItemStatus::Active);
    let dependant = db_item(&conn, ItemStatus::Queued);
    set_deps(&conn, &dependant, &[&dep.code]);

    let Claim::Note { text, recorded, .. } = plan_and_claim(&conn, "p1", 1) else {
        panic!("expected a waiting note");
    };
    assert!(text.contains(&dep.code), "{text}");
    assert!(recorded.is_none(), "a dep still being built is transient");

    store::reject(&conn, &dep.id, "not doing this").unwrap();
    let Claim::Note {
        item,
        text,
        recorded,
    } = plan_and_claim(&conn, "p1", 1)
    else {
        panic!("expected a note about the wedged dependant");
    };
    assert_eq!(item.id, dependant.id);
    assert!(
        text.contains(&dep.code) && text.contains("rejected"),
        "the wedge names the dep and the decision: {text}"
    );
    assert!(text.contains("remove or replace"), "{text}");
    let event = recorded.expect("a dep nobody will build is a durable blockage");
    assert_eq!(event.kind, EventKind::Blocked);
    assert_eq!(event.actor, EventActor::Drainer);
    assert_eq!(event.detail.as_deref(), Some(text.as_str()));

    let Claim::Note { recorded, .. } = plan_and_claim(&conn, "p1", 1) else {
        panic!("expected a note");
    };
    assert!(
        recorded.is_none(),
        "a durable line must not repeat per tick"
    );
    assert_eq!(
        events::list_for_item(&conn, &dependant.id).unwrap().len(),
        1
    );

    store::reopen(&conn, &dep.id).unwrap();
    let Claim::Note { text, recorded, .. } = plan_and_claim(&conn, "p1", 1) else {
        panic!("expected a waiting note");
    };
    assert!(!text.contains("rejected"), "{text}");
    assert!(
        recorded.is_none(),
        "waiting on live work is transient again"
    );
}

#[test]
fn every_standing_blockage_is_durable_not_just_the_dependency_loop() {
    let conn = test_conn();
    let it = db_item(&conn, ItemStatus::Queued);

    let Claim::Note { text, recorded, .. } = plan_and_claim(&conn, "p1", 1) else {
        panic!("expected a note about the wedged item");
    };
    assert!(text.contains("No workflow"), "{text}");
    let event = recorded.expect("a blockage that never resolves is durable");
    assert_eq!(event.kind, EventKind::Blocked);
    assert_eq!(event.actor, EventActor::Drainer);
    assert_eq!(
        event.detail.as_deref(),
        Some(text.as_str()),
        "one reason string for both channels"
    );

    let Claim::Note { recorded, .. } = plan_and_claim(&conn, "p1", 1) else {
        panic!("expected a note");
    };
    assert!(
        recorded.is_none(),
        "a durable line must not repeat per tick"
    );
    assert_eq!(events::list_for_item(&conn, &it.id).unwrap().len(), 1);

    conn.execute(
        "INSERT INTO wf_definition (id, name, spec_json, created_at, updated_at)
         VALUES ('wf-1', 'Build', ?1, 0, 0)",
        rusqlite::params![minimal_spec()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO project_settings (project_id, key, value) VALUES ('p1', ?1, 'wf-1')",
        rusqlite::params![DEFAULT_WORKFLOW_KEY],
    )
    .unwrap();

    let Claim::Note { text, recorded, .. } = plan_and_claim(&conn, "p1", 1) else {
        panic!("expected a note about the missing repo");
    };
    assert!(text.contains("no repo"), "{text}");
    let event = recorded.expect("a project with no repo is not going to grow one by itself");
    assert_eq!(event.detail.as_deref(), Some(text.as_str()));

    let trail = events::list_for_item(&conn, &it.id).unwrap();
    assert_eq!(trail.len(), 2);
    assert!(trail[0].detail.as_deref().unwrap().contains("no repo"));
    assert!(trail[1].detail.as_deref().unwrap().contains("No workflow"));
}

/// refuse it, so the queue can never get past it on its own.
#[test]
fn an_invalid_workflow_spec_wedges_durably() {
    let conn = test_conn();
    let it = db_item(&conn, ItemStatus::Queued);
    conn.execute(
        "INSERT INTO wf_definition (id, name, spec_json, created_at, updated_at)
         VALUES ('wf-bad', 'Broken', '{\"nope\":1}', 0, 0)",
        [],
    )
    .unwrap();
    store::update(
        &conn,
        &it.id,
        &ItemPatch {
            workflow_def_id: Some(Some("wf-bad".into())),
            ..Default::default()
        },
    )
    .unwrap();

    let Claim::Note { text, recorded, .. } = plan_and_claim(&conn, "p1", 1) else {
        panic!("expected a note about the invalid spec");
    };
    assert!(text.contains("missing or no longer valid"), "{text}");
    let event = recorded.expect("an invalid spec does not fix itself");
    assert_eq!(event.kind, EventKind::Blocked);
}

#[test]
fn ordinary_dep_waiting_stays_transient() {
    let conn = test_conn();
    let dep = db_item(&conn, ItemStatus::Open);
    let waiting = db_item(&conn, ItemStatus::Queued);
    set_deps(&conn, &waiting, &[&dep.code]);

    let Claim::Note { text, recorded, .. } = plan_and_claim(&conn, "p1", 1) else {
        panic!("expected a note");
    };
    assert_eq!(text, format!("Waiting on {}", dep.code));
    assert!(recorded.is_none());
    assert!(events::list_for_item(&conn, &waiting.id)
        .unwrap()
        .is_empty());
}

fn reached(conn: &Connection, project_id: &str) -> &'static str {
    match plan_and_claim(conn, project_id, 1) {
        Claim::Nothing => "nothing",
        Claim::Note { .. } => "in the queue",
        Claim::Claimed(..) => "claimed",
    }
}

#[test]
fn a_held_project_dispatches_nothing_and_says_nothing_on_the_cards() {
    let conn = test_conn();
    let ready = db_item(&conn, ItemStatus::Queued);
    assert_eq!(
        reached(&conn, "p1"),
        "in the queue",
        "unheld, it is dispatchable"
    );
    // standing blockage, so it wrote one `blocked` line) is the baseline: what the
    // *hold* must add to is nothing.
    let before = events::list_for_item(&conn, &ready.id).unwrap();

    brakes::hold_project(&conn, "p1", "re-planning the quarter", EventActor::Pm).unwrap();
    assert_eq!(
        reached(&conn, "p1"),
        "nothing",
        "a held project never reaches its queue at all"
    );
    assert_eq!(
        store::get(&conn, &ready.id).unwrap().unwrap().status,
        ItemStatus::Queued,
        "the item keeps its place in the queue — a hold is not an unqueue"
    );
    assert_eq!(
        events::list_for_item(&conn, &ready.id).unwrap(),
        before,
        "a board-wide stop is not history about any one item"
    );

    assert!(brakes::release_project(&conn, "p1").unwrap());
    assert_eq!(reached(&conn, "p1"), "in the queue");
}

#[test]
fn the_whole_decision_keeps_a_held_done_items_dependants_waiting() {
    let conn = test_conn();
    let dep = db_item(&conn, ItemStatus::Done);
    let dependant = db_item(&conn, ItemStatus::Queued);
    set_deps(&conn, &dependant, &[&dep.code]);

    let Claim::Note { text, .. } = plan_and_claim(&conn, "p1", 1) else {
        panic!("expected the dependant to be picked");
    };
    assert!(
        text.contains("No workflow"),
        "a done dependency lets it through: {text}"
    );

    brakes::hold_item(&conn, &dep.id, "we agreed something else", EventActor::Pm).unwrap();
    let Claim::Note {
        item,
        text,
        recorded,
    } = plan_and_claim(&conn, "p1", 1)
    else {
        panic!("expected a note about the waiting dependant");
    };
    assert_eq!(item.id, dependant.id);
    assert_eq!(
        text,
        format!("Waiting on {}", dep.code),
        "the dependency is done, and held, so it does not count as landed"
    );
    assert!(
        recorded.is_none(),
        "waiting on a hold resolves when the user lifts it — nothing durable"
    );
}

#[test]
fn a_project_hold_blocks_the_dispatch_a_merge_would_have_triggered() {
    let conn = test_conn();
    let dep = db_item(&conn, ItemStatus::Done);
    let dependant = db_item(&conn, ItemStatus::Queued);
    set_deps(&conn, &dependant, &[&dep.code]);
    assert_eq!(reached(&conn, "p1"), "in the queue");

    brakes::hold_project(&conn, "p1", "re-planning the quarter", EventActor::User).unwrap();
    assert_eq!(
        reached(&conn, "p1"),
        "nothing",
        "the dependency landed, and the board is stopped anyway"
    );
    assert_eq!(
        store::get(&conn, &dependant.id).unwrap().unwrap().status,
        ItemStatus::Queued
    );
}

#[test]
fn a_sweep_that_ships_a_held_item_leaves_its_dependants_waiting() {
    use crate::roadmap::merge_sweep::{self, Verdict};

    let conn = test_conn();
    let dep = db_item(&conn, ItemStatus::InReview);
    let dependant = db_item(&conn, ItemStatus::Queued);
    set_deps(&conn, &dependant, &[&dep.code]);
    brakes::hold_item(&conn, &dep.id, "we agreed something else", EventActor::Pm).unwrap();

    let (kind, detail) = merge_sweep::event_for(&Verdict::Landed, true).expect("a merge records");
    let (row, event) = apply_and_record(
        &conn,
        &dep.id,
        Some(ItemStatus::InReview),
        &merge_sweep::patch_for(&Verdict::Landed).expect("a merge writes"),
        EventActor::Sweep,
        kind,
        detail,
    )
    .unwrap()
    .expect("a merged PR is a fact the board reflects");
    assert_eq!(row.status, ItemStatus::Done, "the board reflects reality");
    assert!(row.is_held(), "and the hold survives onto the done row");
    assert_eq!(
        event.detail.as_deref(),
        Some(merge_sweep::SHIPPED_WHILE_HELD),
        "the trail says why the items behind it are still queued"
    );

    let Claim::Note { item, text, .. } = plan_and_claim(&conn, "p1", 1) else {
        panic!("expected the dependant to still be waiting");
    };
    assert_eq!(item.id, dependant.id);
    assert_eq!(text, format!("Waiting on {}", dep.code));

    // Released by the user, the same board dispatches what the merge unblocked.
    brakes::release_item(&conn, &dep.id).unwrap();
    let Claim::Note { item, text, .. } = plan_and_claim(&conn, "p1", 1) else {
        panic!("expected the dependant to be picked");
    };
    assert_eq!(item.id, dependant.id);
    assert!(text.contains("No workflow"), "{text}");
}

#[test]
fn a_held_item_is_never_claimed_through_the_whole_decision() {
    let conn = test_conn();
    let it = db_item(&conn, ItemStatus::Queued);
    assert_eq!(reached(&conn, "p1"), "in the queue");

    brakes::hold_item(&conn, &it.id, "confirm the scope first", EventActor::Pm).unwrap();
    assert_eq!(
        reached(&conn, "p1"),
        "nothing",
        "a held item is not a blocked queue — it simply isn't in the queue"
    );
    assert_eq!(
        store::get(&conn, &it.id).unwrap().unwrap().status,
        ItemStatus::Queued
    );
}

#[test]
fn a_note_repeats_when_the_row_moved_and_stays_quiet_when_it_didnt() {
    let mut said: HashMap<String, SaidNote> = HashMap::new();
    let blocked = item("FLT-100", 10.0);
    let note = "Waiting on FLT-099";

    // First time it's said, and then it's silent — a permanently blocked item
    // must not re-emit the same string every tick.
    assert!(record_note(&mut said, &blocked, note));
    assert!(!record_note(&mut said, &blocked, note));

    let requeued = RoadmapItem {
        updated_at: blocked.updated_at + 1,
        ..blocked.clone()
    };
    assert!(record_note(&mut said, &requeued, note));
    assert!(!record_note(&mut said, &requeued, note));

    assert!(record_note(&mut said, &requeued, "Waiting on FLT-098"));
}

#[test]
fn the_brief_carries_the_item_its_criteria_and_its_ancestry() {
    let mut it = item("FLT-142", 10.0);
    it.title = "Persist worktree state across restarts".into();
    it.why = "A hard quit loses every checkout binding.".into();
    it.accept = vec!["survives a quit".into(), "orphans are offered".into()];
    it.deps = vec!["FLT-140".into()];

    let mut dep = item("FLT-140", 5.0);
    dep.title = "Worktree registry".into();
    dep.status = ItemStatus::Done;

    let brief = build_brief(&it, &[&dep]);

    assert!(brief.starts_with("FLT-142: Persist worktree state across restarts"));
    assert!(brief.contains("A hard quit loses every checkout binding."));
    assert!(brief.contains("Done when:"));
    assert!(brief.contains("- [ ] survives a quit"));
    assert!(brief.contains("- [ ] orphans are offered"));
    assert!(brief.contains("- FLT-140: Worktree registry (done)"));
    assert!(brief.contains("[FLT-142]"));
    assert!(brief.contains("pull request title"));
}

#[test]
fn a_bare_item_still_produces_a_usable_brief() {
    let brief = build_brief(&item("FLT-100", 10.0), &[]);
    assert!(brief.starts_with("FLT-100: do FLT-100"));
    assert!(!brief.contains("Done when:"));
    assert!(!brief.contains("Builds on"));
    assert!(brief.contains("[FLT-100]"));
}
