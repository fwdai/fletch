//! Drainer decision tests.
//!
//! Everything the drainer *decides* is a pure function over a snapshot
//! ([`pick_next`], [`unsatisfied_deps`], [`resolve_workflow`], [`settle`],
//! [`build_brief`]), so the rules are tested here without a tokio runtime, a
//! clock, or a database. The tick itself is the thin part: read a snapshot,
//! call these, write the answer back.

use super::*;
use crate::roadmap::types::{Horizon, ItemSource};

/// A queued item, `n` milliseconds into the board's life. `created_at` is what
/// orders the queue, so the tests set it explicitly rather than relying on
/// insertion order.
fn item(code: &str, created_at: i64) -> RoadmapItem {
    RoadmapItem {
        id: format!("id-{code}"),
        project_id: "p1".into(),
        code: code.into(),
        parent_id: None,
        title: format!("do {code}"),
        why: String::new(),
        horizon: Horizon::Next,
        status: ItemStatus::Queued,
        area: None,
        source: ItemSource::User,
        accept: Vec::new(),
        deps: Vec::new(),
        agent_id: None,
        workflow_def_id: None,
        run_id: None,
        pr_url: None,
        pr_number: None,
        created_at,
        updated_at: created_at,
    }
}

fn codes(list: &[&str]) -> HashSet<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

/// The PR a finished run recorded on its row.
fn pr(number: Option<i64>) -> FinalizedPr {
    FinalizedPr {
        url: "https://github.com/o/r/pull/42".into(),
        number,
    }
}

// ───────────────────────────── ordering ─────────────────────────────────

#[test]
fn the_queue_is_fifo() {
    // The user queued these in this order; the drainer honours it. Horizon is
    // deliberately not consulted — a `later` item the user queued outranks a
    // `now` item they didn't.
    let mut first = item("FLT-100", 10);
    first.horizon = Horizon::Later;
    let mut second = item("FLT-101", 20);
    second.horizon = Horizon::Now;
    let queue = vec![first, second];

    assert_eq!(
        pick_next(&queue, 0, &codes(&[]), &codes(&["FLT-100", "FLT-101"])),
        Decision::Dispatch(0)
    );
}

#[test]
fn an_empty_queue_decides_nothing() {
    assert_eq!(pick_next(&[], 0, &codes(&[]), &codes(&[])), Decision::Empty);
}

// ───────────────────────────── dependencies ─────────────────────────────

#[test]
fn a_done_dependency_lets_an_item_through() {
    let mut it = item("FLT-101", 10);
    it.deps = vec!["FLT-100".into()];

    assert_eq!(
        pick_next(
            &[it],
            0,
            &codes(&["FLT-100"]),
            &codes(&["FLT-100", "FLT-101"])
        ),
        Decision::Dispatch(0)
    );
}

#[test]
fn an_in_review_dependency_still_blocks() {
    // `in_review` means the PR is open, not merged: a dependant forked now
    // would build on a tree that doesn't contain the work it depends on.
    let mut it = item("FLT-101", 10);
    it.deps = vec!["FLT-100".into()];

    assert_eq!(
        pick_next(
            &[it],
            0,
            // FLT-100 exists but is not done.
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
    // The item it pointed at was deleted off the board, and a deleted item
    // never ships — waiting for it would block this one forever.
    let mut it = item("FLT-101", 10);
    it.deps = vec!["FLT-100".into()];

    assert_eq!(
        pick_next(&[it], 0, &codes(&[]), &codes(&["FLT-101"])),
        Decision::Dispatch(0)
    );
}

#[test]
fn a_blocked_head_does_not_block_the_rest_of_the_queue() {
    // Skipped, never failed: FLT-100's turn comes when its dep lands.
    let mut blocked = item("FLT-100", 10);
    blocked.deps = vec!["FLT-099".into()];
    let ready = item("FLT-101", 20);

    assert_eq!(
        pick_next(
            &[blocked, ready],
            0,
            &codes(&[]),
            &codes(&["FLT-099", "FLT-100", "FLT-101"])
        ),
        Decision::Dispatch(1)
    );
}

#[test]
fn an_all_blocked_queue_reports_the_head_and_what_it_waits_on() {
    let mut head = item("FLT-100", 10);
    head.deps = vec!["FLT-098".into(), "FLT-099".into()];
    let mut tail = item("FLT-101", 20);
    tail.deps = vec!["FLT-100".into()];

    let known = codes(&["FLT-098", "FLT-099", "FLT-100", "FLT-101"]);
    // FLT-098 landed; FLT-099 hasn't.
    assert_eq!(
        pick_next(&[head, tail], 0, &codes(&["FLT-098"]), &known),
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

// ───────────────────────────── concurrency ──────────────────────────────

#[test]
fn the_cap_holds_the_queue_even_with_a_ready_item() {
    let ready = item("FLT-100", 10);
    assert_eq!(
        pick_next(
            &[ready],
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
    let mut blocked = item("FLT-100", 10);
    blocked.deps = vec!["FLT-099".into()];
    assert_eq!(
        pick_next(
            &[blocked],
            MAX_CONCURRENT_ROADMAP_RUNS,
            &codes(&[]),
            &codes(&["FLT-099", "FLT-100"])
        ),
        Decision::AtCapacity
    );
}

// ───────────────────────────── workflow choice ──────────────────────────

#[test]
fn an_items_own_workflow_wins_over_the_project_default() {
    let mut it = item("FLT-100", 10);
    it.workflow_def_id = Some("wf-item".into());
    assert_eq!(
        resolve_workflow(&it, Some("wf-default")),
        Some("wf-item".to_string())
    );
}

#[test]
fn an_item_without_one_falls_back_to_the_project_default() {
    let it = item("FLT-100", 10);
    assert_eq!(
        resolve_workflow(&it, Some("wf-default")),
        Some("wf-default".to_string())
    );
}

#[test]
fn no_workflow_anywhere_resolves_to_nothing() {
    // Explicitly not a hardcoded fallback spec: inventing a workflow for work
    // the user queued is worse than asking them to pick one.
    assert_eq!(resolve_workflow(&item("FLT-100", 10), None), None);
}

// ───────────────────────────── settlement ───────────────────────────────

#[test]
fn a_live_run_leaves_its_item_alone() {
    for status in [RunStatus::Pending, RunStatus::Running, RunStatus::Paused] {
        assert_eq!(settle(Some(status), None), Settlement::Running);
    }
}

#[test]
fn a_finished_run_with_a_pr_lands_in_review() {
    // `merge_sweep` is what takes it from here to `done`, once GitHub says the
    // PR merged.
    assert_eq!(
        settle(Some(RunStatus::Done), Some(&pr(Some(42)))),
        Settlement::InReview
    );
}

#[test]
fn a_pr_whose_number_never_landed_still_reaches_review() {
    // The URL is what makes it reviewable; the number is what makes it
    // *pollable*. Without one the item sits in review until a human moves it,
    // which is better than settling work that has an open PR straight to done.
    assert_eq!(
        settle(Some(RunStatus::Done), Some(&pr(None))),
        Settlement::InReview
    );
}

#[test]
fn a_finished_run_with_no_pr_is_simply_done() {
    // A workflow with `open_pr: false` finished the work; nothing else is
    // coming, so there is nothing to review.
    assert_eq!(settle(Some(RunStatus::Done), None), Settlement::Done);
}

#[test]
fn a_lost_run_releases_its_item_back_to_the_board() {
    // Never back to `queued`: an auto-retry loop on a failing workflow burns
    // tokens all night. Re-queueing is the user's call, once they know why.
    assert_eq!(
        settle(Some(RunStatus::Failed), None),
        Settlement::Released("its run failed")
    );
    assert_eq!(
        settle(Some(RunStatus::Canceled), None),
        Settlement::Released("its run was canceled")
    );
    assert_eq!(
        settle(None, None),
        Settlement::Released("its run was deleted")
    );
}

// ───────────────────────────── crash recovery ───────────────────────────

/// A migrated in-memory DB, as the app opens the real file.
fn test_conn() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    crate::database::get_migrations()
        .to_latest(&mut conn)
        .unwrap();
    conn
}

/// A roadmap-dispatched run for `item_id`, in `status`, created at `created_at`.
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
    // The item is `active` with `run_id` NULL — a claim whose launch never wrote
    // back. The newest back-linked run is a *terminal* one from a previous
    // cycle: adopting it would settle this claim against last cycle's outcome
    // (resurrecting a merged-or-dead PR as `in_review`). Only the live run may
    // be adopted.
    let conn = test_conn();
    run(&conn, "old-done", "id-FLT-100", "done", 10);
    run(&conn, "live", "id-FLT-100", "running", 20);
    assert_eq!(
        dispatched_run_id(&conn, "id-FLT-100"),
        Some("live".to_string())
    );

    // Newest is terminal and nothing live remains: the claim's run never
    // started, which is the release path — not an adoption.
    run(&conn, "newest-failed", "id-FLT-101", "failed", 30);
    run(&conn, "older-canceled", "id-FLT-101", "canceled", 20);
    assert_eq!(dispatched_run_id(&conn, "id-FLT-101"), None);

    // No back-link at all is the same answer.
    assert_eq!(dispatched_run_id(&conn, "id-FLT-999"), None);
}

// ───────────────────────────── note dedup ───────────────────────────────

#[test]
fn a_note_repeats_when_the_row_moved_and_stays_quiet_when_it_didnt() {
    let mut said: HashMap<String, SaidNote> = HashMap::new();
    let blocked = item("FLT-100", 10);
    let note = "Waiting on FLT-099";

    // First time it's said, and then it's silent — a permanently blocked item
    // must not re-emit the same string every tick.
    assert!(record_note(&mut said, &blocked, note));
    assert!(!record_note(&mut said, &blocked, note));

    // Unqueue + re-queue bumps `updated_at` (every write does), and the note is
    // recomputed to the same string. It has to be said again: the user is
    // looking at a card that has no explanation on it any more.
    let requeued = RoadmapItem {
        updated_at: blocked.updated_at + 1,
        ..blocked.clone()
    };
    assert!(record_note(&mut said, &requeued, note));
    assert!(!record_note(&mut said, &requeued, note));

    // A different reason is always news, version or no version.
    assert!(record_note(&mut said, &requeued, "Waiting on FLT-098"));
}

// ───────────────────────────── the brief ────────────────────────────────

#[test]
fn the_brief_carries_the_item_its_criteria_and_its_ancestry() {
    let mut it = item("FLT-142", 10);
    it.title = "Persist worktree state across restarts".into();
    it.why = "A hard quit loses every checkout binding.".into();
    it.accept = vec!["survives a quit".into(), "orphans are offered".into()];
    it.deps = vec!["FLT-140".into()];

    let mut dep = item("FLT-140", 5);
    dep.title = "Worktree registry".into();
    dep.status = ItemStatus::Done;

    let brief = build_brief(&it, &[&dep]);

    assert!(brief.starts_with("FLT-142: Persist worktree state across restarts"));
    assert!(brief.contains("A hard quit loses every checkout binding."));
    // Acceptance criteria arrive as a checklist the run can tick through.
    assert!(brief.contains("Done when:"));
    assert!(brief.contains("- [ ] survives a quit"));
    assert!(brief.contains("- [ ] orphans are offered"));
    // What already landed underneath it — context a human in a chat would have
    // and a non-interactive run wouldn't.
    assert!(brief.contains("- FLT-140: Worktree registry (done)"));
    // And the string that lets the board find its way back to the work.
    assert!(brief.contains("[FLT-142]"));
    assert!(brief.contains("pull request title"));
}

#[test]
fn a_bare_item_still_produces_a_usable_brief() {
    // No why, no criteria, no deps: the run gets the title and the tracking
    // instruction, and no empty headings pretending there's more.
    let brief = build_brief(&item("FLT-100", 10), &[]);
    assert!(brief.starts_with("FLT-100: do FLT-100"));
    assert!(!brief.contains("Done when:"));
    assert!(!brief.contains("Builds on"));
    assert!(brief.contains("[FLT-100]"));
}
