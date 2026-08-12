//! Merge-sweep decision tests.
//!
//! Same shape as the drainer's: everything the sweep *decides* is a pure
//! function over a snapshot ([`pollable`], [`verdict`], [`patch_for`]), so the
//! rules are tested with no network, no runtime, and no database. The task is
//! the thin part — read the watch list, call these, write the answer back.

use super::*;
use crate::roadmap::types::{Horizon, ItemSource};

/// An item under review with an open PR against it.
fn in_review(code: &str, number: Option<i64>) -> RoadmapItem {
    RoadmapItem {
        id: format!("id-{code}"),
        project_id: "p1".into(),
        code: code.into(),
        title: format!("do {code}"),
        why: String::new(),
        horizon: Horizon::Now,
        status: ItemStatus::InReview,
        rank: 1.0,
        area: None,
        source: ItemSource::User,
        accept: Vec::new(),
        deps: Vec::new(),
        agent_id: None,
        workflow_def_id: None,
        run_id: Some("run-1".into()),
        pr_url: number.map(|n| format!("https://github.com/o/r/pull/{n}")),
        pr_number: number,
        hold_reason: None,
        held_by: None,
        held_at: None,
        close_reason: None,
        issue_url: None,
        created_at: 10,
        updated_at: 10,
    }
}

// ───────────────────────────── the watch list ───────────────────────────

#[test]
fn an_empty_board_gives_the_sweep_nothing_to_do() {
    // The idle case, and the common one: with nothing in review the task sleeps
    // on its nudge instead of ticking, so an install whose board is all `done`
    // makes no requests at all.
    assert!(pollable(&[]).is_empty());
}

#[test]
fn only_in_review_items_with_a_pr_number_are_polled() {
    let mut queued = in_review("FLT-100", Some(1));
    queued.status = ItemStatus::Queued;
    let mut active = in_review("FLT-101", Some(2));
    active.status = ItemStatus::Active;
    let mut done = in_review("FLT-102", Some(3));
    done.status = ItemStatus::Done;
    let watched = in_review("FLT-103", Some(4));

    let items = vec![queued, active, done, watched];
    let picked = pollable(&items);

    assert_eq!(picked.len(), 1);
    assert_eq!(picked[0].code, "FLT-103");
}

#[test]
fn an_item_in_review_without_a_number_is_left_to_the_user() {
    // Nothing to poll with, and a number guessed off the URL could be wrong —
    // a wrong number is a wrong verdict written to the board. The card still
    // links the PR.
    assert!(pollable(&[in_review("FLT-100", None)]).is_empty());
}

// ───────────────────────────── verdicts ─────────────────────────────────

#[test]
fn a_merged_pr_ships_its_item() {
    assert_eq!(verdict(Some(PrStatus::Merged)), Verdict::Landed);
    let patch = patch_for(&Verdict::Landed).expect("a merge is a write");
    assert_eq!(patch.status, Some(ItemStatus::Done));
    // The PR is how the item shipped — the link stays on the row.
    assert_eq!(patch.pr_url, None);
    assert_eq!(patch.pr_number, None);
    // And the run stays attached: it is the record of the work.
    assert_eq!(patch.run_id, None);
}

#[test]
fn a_pr_closed_without_merging_puts_its_item_back_on_the_board() {
    assert_eq!(verdict(Some(PrStatus::Closed)), Verdict::Abandoned);
    let patch = patch_for(&Verdict::Abandoned).expect("a close is a write");
    assert_eq!(patch.status, Some(ItemStatus::Open));
    // Not `done` (nothing shipped) and not `queued` (re-running work someone
    // just rejected is the last thing they asked for). Back to the user.
    //
    // The run link is dropped so a re-queue dispatches a fresh run instead of
    // settling instantly against the old terminal one…
    assert_eq!(patch.run_id, Some(None));
    // …while the PR columns are left alone, so the card keeps its history.
    assert_eq!(patch.pr_url, None);
    assert_eq!(patch.pr_number, None);
}

#[test]
fn an_open_pr_changes_nothing() {
    assert_eq!(verdict(Some(PrStatus::Open)), Verdict::Waiting);
    assert!(patch_for(&Verdict::Waiting).is_none());
}

#[test]
fn a_failed_read_changes_nothing_either() {
    // Never blank state on a fetch failure: no token, a rate-limit pause, or an
    // unresolvable remote all mean "we learned nothing", not "the PR is gone".
    // Same policy `supervisor::resolve_pr_state` follows.
    assert_eq!(verdict(None), Verdict::Waiting);
    assert!(patch_for(&verdict(None)).is_none());
}

// ───────────────────────────── the history event ────────────────────────

#[test]
fn a_verdict_writes_and_records_together_or_not_at_all() {
    // `event_for` pairs `patch_for` verdict by verdict: whatever writes the
    // board also writes the durable record, and a verdict that touches nothing
    // records nothing. The `shipped` event's timestamp is the item's `done_at`.
    assert!(event_for(&Verdict::Waiting, false).is_none());
    assert_eq!(
        event_for(&Verdict::Landed, false),
        Some((EventKind::Shipped, None))
    );
    // The fact is what happened to the *pull request*: the item is back on the
    // board and nobody abandoned it. `abandoned` was the flattened kind.
    assert_eq!(
        event_for(&Verdict::Abandoned, false),
        Some((
            EventKind::PrClosed,
            Some("nothing merged — the item is back on the board".to_string())
        ))
    );
}

/// The hold rule: a held item whose PR merged still ships (the board reflects
/// reality), and the *line* is what says the hold outlived the merge. Nothing
/// waiting on the item may move — that half is the drainer's dep gate
/// (`done_codes`), pinned in its own tests.
#[test]
fn a_held_item_still_ships_and_the_line_says_the_hold_stood() {
    // The write takes no view of the hold at all — `patch_for` has nowhere to put
    // one. Skipping it would leave a card claiming "in review" about a pull
    // request that landed, and the hold columns are untouched, so the reason
    // survives onto the `done` row (which is what gates the dependants).
    let patch = patch_for(&Verdict::Landed).expect("a merge is a write, held or not");
    assert_eq!(patch.status, Some(ItemStatus::Done));
    let (kind, detail) = event_for(&Verdict::Landed, true).expect("a merge still records");
    assert_eq!(kind, EventKind::Shipped);
    assert_eq!(detail.as_deref(), Some(SHIPPED_WHILE_HELD));
    assert!(
        SHIPPED_WHILE_HELD.contains("hold stands"),
        "the reader has to learn why the queue behind it is stuck"
    );
    // A hold has nothing to say about the other two verdicts: a closed PR is
    // closed, and a verdict that writes nothing records nothing either way.
    assert_eq!(
        event_for(&Verdict::Abandoned, true),
        event_for(&Verdict::Abandoned, false)
    );
    assert!(event_for(&Verdict::Waiting, true).is_none());
}

/// A held item is still *polled*. The sweep's job is to learn what GitHub did,
/// and a hold is not a reason to stop looking — it is what the answer means.
#[test]
fn a_held_item_stays_on_the_watch_list() {
    let mut held = in_review("FLT-100", Some(7));
    held.hold_reason = Some("we agreed something else".into());
    held.held_by = Some(crate::roadmap::events::EventActor::Pm);
    held.held_at = Some(1);
    assert!(held.is_held());
    assert_eq!(pollable(&[held]).len(), 1);
}

// ───────────────────────── a PR that stops answering ────────────────────

/// The give-up rule. "No answer" is right to retry for a while and wrong to retry
/// forever: a deleted PR, a revoked token and a non-GitHub remote all look like a
/// rate-limit pause here, and the difference is only visible in how long it lasts.
#[test]
fn the_sweep_gives_up_on_a_pr_that_never_answers_and_says_so_once() {
    let mut seen = HashMap::new();
    assert!(
        still_watching(&seen, "i1", 10),
        "an item nobody has asked about yet is watched"
    );

    // Silences short of the limit change nothing: the sweep keeps asking, and says
    // nothing, because a pause is not a wedge.
    for _ in 1..UNANSWERED_LIMIT {
        assert!(!record_miss(&mut seen, "i1", 10));
        assert!(still_watching(&seen, "i1", 10));
    }
    // The crossing is the news, and it is news exactly once.
    assert!(
        record_miss(&mut seen, "i1", 10),
        "this is the pass that gives up"
    );
    assert!(
        !still_watching(&seen, "i1", 10),
        "and the item leaves the watch list, so neither poller pays again"
    );
    assert!(
        !record_miss(&mut seen, "i1", 10),
        "a durable line must not repeat once it has been said"
    );
}

/// Any write to the row is the question being asked again from scratch — the same
/// key the drainer's note dedup uses, for the same reason: somebody did something
/// about it, and the old count describes a state that no longer exists.
#[test]
fn a_row_that_changed_is_watched_again() {
    let mut seen = HashMap::new();
    for _ in 0..UNANSWERED_LIMIT {
        record_miss(&mut seen, "i1", 10);
    }
    assert!(!still_watching(&seen, "i1", 10));
    assert!(
        still_watching(&seen, "i1", 11),
        "the user re-queued, re-linked or edited the item: ask again"
    );
    // And the count restarts rather than resuming where it left off.
    assert!(!record_miss(&mut seen, "i1", 11));
    assert!(still_watching(&seen, "i1", 11));
}

/// An answer clears the memory, so a rate-limit pause followed by a normal read
/// leaves nothing behind to accumulate towards a false give-up.
#[test]
fn an_answer_forgets_the_silence_before_it() {
    let mut seen = HashMap::new();
    for _ in 0..UNANSWERED_LIMIT - 1 {
        record_miss(&mut seen, "i1", 10);
    }
    answered(&mut seen, "i1");
    assert!(seen.is_empty());
    assert!(still_watching(&seen, "i1", 10));
    // The next silence starts its own count from one.
    assert!(!record_miss(&mut seen, "i1", 10));
}

/// One item's silence is not another's: the memory is per item, so a deleted PR on
/// one card cannot mute the watch on the next.
#[test]
fn the_count_is_per_item() {
    let mut seen = HashMap::new();
    for _ in 0..UNANSWERED_LIMIT {
        record_miss(&mut seen, "i1", 10);
    }
    assert!(!still_watching(&seen, "i1", 10));
    assert!(still_watching(&seen, "i2", 10));
}

#[test]
fn the_unreachable_note_names_the_pr_and_what_to_do() {
    let note = unreachable_note(142);
    assert!(note.contains("#142"), "{note}");
    // It cannot tell the three causes apart, so it names them rather than guessing.
    assert!(note.contains("deleted"), "{note}");
    assert!(note.contains("token"), "{note}");
    // And it says what the user can actually do about it, since only they can.
    assert!(note.contains("mark this done"), "{note}");
}

// ───────────────────────────── the note ─────────────────────────────────

#[test]
fn the_closed_note_names_the_pr() {
    let note = abandoned_note(142);
    assert!(note.contains("#142"));
    assert!(note.contains("without merging"));
}
