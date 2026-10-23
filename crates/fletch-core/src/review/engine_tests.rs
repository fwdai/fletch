use super::*;

fn reviewer() -> ReviewerView {
    ReviewerView {
        custom_agent_id: "ca-1".into(),
        name: "Reviewer".into(),
        provider: "codex".into(),
        model: None,
        agent_id: Some("rev-1".into()),
    }
}

/// The code read at cheap signature `cheap` with `digest`.
fn read(cheap: &str, digest: &str) -> Sighting {
    Sighting::Read {
        cheap: cheap.into(),
        digest: ContentDigest::Verified {
            digest: digest.into(),
        },
    }
}

fn skipped(cheap: &str) -> Sighting {
    Sighting::Read {
        cheap: cheap.into(),
        digest: ContentDigest::Skipped,
    }
}

fn unverified(cheap: &str) -> Sighting {
    Sighting::Read {
        cheap: cheap.into(),
        digest: ContentDigest::Unverified {
            reason: "git diff: Permission denied".into(),
        },
    }
}

fn unreadable() -> Sighting {
    Sighting::Unreadable {
        reason: "the checkout is not ready".into(),
    }
}

/// A verified code state.
fn state(cheap: &str, digest: &str) -> CodeState {
    CodeState {
        cheap: cheap.into(),
        content: Content::Verified {
            digest: digest.into(),
        },
    }
}

/// An unreadable code state.
fn unreadable_state(cheap: &str) -> CodeState {
    CodeState {
        cheap: cheap.into(),
        content: Content::Unverified {
            reason: "git diff: Permission denied".into(),
        },
    }
}

/// The read a sighting records.
fn read_of(sighting: &Sighting) -> CodeRead {
    match sighting {
        Sighting::Read { cheap, digest } => CodeRead {
            cheap: cheap.clone(),
            digest: digest.clone(),
        },
        other => panic!("not a read: {other:?}"),
    }
}

/// A round begun on the code `c1` / `d1`, as a start reads it first.
fn begun(trigger: Trigger, pr: Option<u32>) -> Tracked {
    let mut t = Tracked::default();
    t.record_sighting(&read("c1", "d1"));
    t.begin(reviewer(), trigger, state("c1", "d1"), pr, 1_000)
        .unwrap();
    t
}

/// A review that landed on `c1#d1` and whose turn ended.
fn reviewed() -> Tracked {
    let mut t = begun(Trigger::Manual, Some(7));
    t.submitted().unwrap();
    t.turn_ended();
    t
}

// ── the derived phase ──────────────────────────────────────────────────────

#[test]
fn an_untouched_checkout_is_idle_and_a_flight_is_reviewing() {
    assert_eq!(Tracked::default().derive(0), Phase::Idle);
    let t = begun(Trigger::Manual, None);
    assert_eq!(t.derive(0), Phase::Reviewing);
    assert_eq!(t.round, 1);
    assert_eq!(t.trigger, Some(Trigger::Manual));
}

#[test]
fn a_report_makes_the_code_reviewed_then_the_threads_and_a_fresh_read_decide() {
    let mut t = reviewed();
    assert_eq!(t.reviewed, Some(state("c1", "d1")));
    assert_eq!(t.derive(0), Phase::Clean);
    assert_eq!(t.derive(2), Phase::Findings);
    // A read of other code: the status moved, or only its content.
    t.record_sighting(&skipped("c2"));
    assert_eq!(t.derive(2), Phase::Stale);
    t.record_sighting(&read("c1", "d2"));
    assert_eq!(t.derive(0), Phase::Stale);
    // Back to the reviewed code.
    t.record_sighting(&read("c1", "d1"));
    assert_eq!(t.derive(0), Phase::Clean);
}

/// The evidence rule: only a `Read` sighting moves the code half of the
/// phase. For every phase a checkout can be in, a look that read nothing or
/// couldn't read the checkout derives the same phase as before it.
#[test]
fn a_look_without_a_read_never_changes_the_phase() {
    let mut cases: Vec<(&str, Tracked, u32)> = Vec::new();
    cases.push(("idle", Tracked::default(), 0));
    cases.push(("clean", reviewed(), 0));
    cases.push(("findings", reviewed(), 3));
    let mut stale = reviewed();
    stale.record_sighting(&skipped("c2"));
    cases.push(("stale", stale, 1));
    let mut moved = reviewed();
    moved.record_sighting(&read("c1", "d9"));
    cases.push(("stale by content", moved, 0));
    let mut unverifiable = reviewed();
    unverifiable.record_sighting(&unverified("c1"));
    cases.push(("stale, unverified", unverifiable, 0));
    let mut failed = reviewed();
    failed.record_failure(FailureOrigin::Round, "boom", None);
    cases.push(("failed", failed, 0));
    let mut queued = reviewed();
    queued.queue(Trigger::Manual, None);
    cases.push(("queued", queued, 0));
    cases.push(("reviewing", begun(Trigger::Manual, None), 0));

    for (name, before, open) in cases {
        let phase = before.derive(open);
        for sighting in [Sighting::NotScanned, unreadable()] {
            let mut after = before.clone();
            after.record_sighting(&sighting);
            assert_eq!(after.derive(open), phase, "{name} after {sighting:?}");
            assert_eq!(after.last_read, before.last_read, "{name}: the facts stay");
            assert_eq!(after.failure, before.failure, "{name}: so does the failure");
        }
    }
}

#[test]
fn a_turn_that_ends_without_a_report_is_a_round_failure_until_the_next_start() {
    let mut t = begun(Trigger::PrOpen, Some(7));
    t.turn_ended();
    assert_eq!(t.derive(0), Phase::Failed);
    let failure = t.failure.clone().unwrap();
    assert_eq!(failure.origin, FailureOrigin::Round);
    assert!(failure.reason.contains("without submitting"));
    assert_eq!(
        failure.code,
        Some(state("c1", "d1")),
        "the code the round read"
    );
    assert_eq!(t.reviewed, None, "nothing was reviewed");
    // A round that begins clears it, and the note.
    t.note = Some("note".into());
    t.begin(
        reviewer(),
        Trigger::Manual,
        state("c1", "d1"),
        Some(7),
        2_000,
    )
    .unwrap();
    assert_eq!(t.failure, None);
    assert_eq!(t.note, None);
    assert_eq!(t.round, 2);
}

// ── one failure record ─────────────────────────────────────────────────────

/// Every origin persists and comes back deriving its documented phase: a scan
/// that couldn't verify reads stale, every other origin failed. The wire's
/// error is the failure's reason, ahead of any note.
#[test]
fn every_failure_origin_round_trips_and_derives_its_phase() {
    let c = conn();
    let key = ("arabia".to_string(), None);
    for (origin, phase) in [
        (FailureOrigin::Scan, Phase::Stale),
        (FailureOrigin::Start, Phase::Failed),
        (FailureOrigin::Round, Phase::Failed),
        (FailureOrigin::Queued, Phase::Failed),
        (FailureOrigin::Restart, Phase::Failed),
    ] {
        let mut t = reviewed();
        t.note = Some("a note".into());
        t.record_failure(
            origin,
            format!("{origin:?} went wrong"),
            Some(state("c2", "d2")),
        );
        assert_eq!(t.derive(0), phase, "{origin:?}");
        persist::save(&c, &key, &t).unwrap();
        let (_, back) = persist::load_all(&c).unwrap().pop().unwrap();
        let back = back.loaded();
        assert_eq!(back.failure, t.failure, "{origin:?} persisted");
        assert_eq!(back.derive(0), phase, "{origin:?} after a restart");
        assert_eq!(
            back.error().as_deref(),
            Some(format!("{origin:?} went wrong").as_str())
        );
    }
}

/// A restart turns a round that was in flight into a restart failure.
#[test]
fn a_round_in_flight_at_shutdown_loads_as_a_restart_failure() {
    let c = conn();
    let key = ("arabia".to_string(), None);
    let mut flying = begun(Trigger::PrOpen, Some(7));
    flying.phase = flying.derive(0);
    assert_eq!(flying.phase, Phase::Reviewing);
    persist::save(&c, &key, &flying).unwrap();
    let (_, back) = persist::load_all(&c).unwrap().pop().unwrap();
    assert_eq!(back.flight, None, "a flight never persists");
    let back = back.loaded();
    let failure = back.failure.clone().unwrap();
    assert_eq!(failure.origin, FailureOrigin::Restart);
    assert_eq!(failure.reason, "interrupted by restart");
    assert_eq!(back.derive(0), Phase::Failed);
}

// ── restarts ───────────────────────────────────────────────────────────────

/// A stale verdict persisted with its facts still reads stale after a restart
/// when the first answer reads no git (a client connecting, a quiet tick) —
/// and so does one a scan couldn't verify, with its note.
#[test]
fn a_persisted_stale_checkout_reads_stale_before_any_scan() {
    let c = conn();
    let mut stale = reviewed();
    stale.record_sighting(&skipped("c2"));
    stale.phase = stale.derive(0);
    let mut unverifiable = reviewed();
    unverifiable.record_sighting(&unverified("c1"));
    unverifiable.phase = unverifiable.derive(0);
    persist::save(&c, &("stale".into(), None), &stale).unwrap();
    persist::save(&c, &("unverified".into(), None), &unverifiable).unwrap();

    for (key, back) in persist::load_all(&c).unwrap() {
        let mut back = back.loaded();
        back.record_sighting(&Sighting::NotScanned);
        assert_eq!(back.derive(0), Phase::Stale, "{key:?}");
        if key.0 == "unverified" {
            assert!(
                back.error()
                    .unwrap()
                    .starts_with("couldn't verify the review is current"),
                "the note survives too"
            );
        }
    }
}

/// Old facts are not trusted forever: every loaded checkout is read on the
/// first tick, whatever its PR or activity, and only a look that read (or
/// tried to read) git settles that.
#[test]
fn the_first_tick_after_a_restart_reads_every_loaded_checkout() {
    let c = conn();
    let quiet = reviewed();
    for agent in ["a", "b", "c"] {
        persist::save(&c, &(agent.into(), None), &quiet).unwrap();
    }
    let now = 10_000_000;
    let loaded: Vec<Tracked> = persist::load_all(&c)
        .unwrap()
        .into_iter()
        .map(|(_, t)| t.loaded())
        .collect();
    assert_eq!(loaded.len(), 3);
    for mut t in loaded {
        assert!(t.wants_scan(false, now), "no PR, not active: read anyway");
        t.record_sighting(&Sighting::NotScanned);
        assert!(t.wants_scan(false, now), "a cheap look settles nothing");
        t.record_sighting(&read("c1", "d1"));
        assert!(!t.wants_scan(false, now), "read once; quiet again");
        let mut u = reviewed().loaded();
        u.record_sighting(&unreadable());
        assert!(!u.wants_scan(false, now), "a read attempt settles it too");
    }
}

// ── one automatic attempt per code state ───────────────────────────────────

/// The one comparison, every case of it.
#[test]
fn identity_is_decided_by_the_cheap_half_and_the_content() {
    use Identity::*;
    let cases = [
        // the cheap halves differ, whatever the content
        (read("c2", "d1"), state("c1", "d1"), Different),
        (skipped("c2"), state("c1", "d1"), Different),
        (unverified("c2"), unreadable_state("c1"), Different),
        // both verified: the digest decides
        (read("c1", "d1"), state("c1", "d1"), Same),
        (read("c1", "d2"), state("c1", "d1"), Different),
        // the same unreadable state: nothing new to read
        (unverified("c1"), unreadable_state("c1"), Same),
        // readability changed: a new state either way
        (read("c1", "d1"), unreadable_state("c1"), Different),
        (unverified("c1"), state("c1", "d1"), Different),
        // a skipped digest on the same cheap half can't say
        (skipped("c1"), state("c1", "d1"), Unknown),
        (skipped("c1"), unreadable_state("c1"), Unknown),
    ];
    for (sighting, stored, expected) in cases {
        assert_eq!(
            identity(&read_of(&sighting), &stored),
            expected,
            "{sighting:?} vs {stored:?}"
        );
    }
    // A skipped read is never a stored state.
    assert_eq!(read_of(&skipped("c1")).state(), None);
    assert_eq!(read_of(&read("c1", "d1")).state(), Some(state("c1", "d1")));
}

/// A digest is skipped only when the cheap half differs from the reviewed
/// one, so a skipped read is `Different` from the reviewed state — staleness
/// never meets `Unknown`.
#[test]
fn a_skipped_read_is_always_different_from_the_reviewed_state() {
    let reviewed = state("c1", "d1");
    for cheap in ["c1", "c2", ""] {
        if !digest_needed(cheap, false, Some(&reviewed)) {
            assert_eq!(
                identity(&read_of(&skipped(cheap)), &reviewed),
                Identity::Different,
                "{cheap}"
            );
        }
    }
    assert!(
        digest_needed("c1", false, Some(&reviewed)),
        "same cheap half"
    );
    assert!(
        digest_needed("c2", true, Some(&reviewed)),
        "a state to record"
    );
    assert!(!digest_needed("c2", false, None), "nothing to compare to");
}

/// The finding: an automatic round fails on code A; the author edits the
/// same dirty file — same paths and kinds, other bytes. The next tick's read
/// skips the digest (the cheap half differs from the reviewed one), so the
/// gate asks for a full read, and on it allows exactly one attempt. Editing
/// back to the bytes the round failed on stays blocked.
#[test]
fn a_content_edit_after_a_failed_automatic_round_allows_one_attempt() {
    let mut t = reviewed();
    t.record_sighting(&skipped("c2"));
    assert_eq!(
        t.automatic_review(),
        AutoReview::Allow,
        "moved code: one attempt"
    );

    // The attempt reads A in full and its round fails on it.
    t.record_sighting(&read("c2", "a"));
    t.begin(reviewer(), Trigger::ReReview, state("c2", "a"), Some(7), 0)
        .unwrap();
    t.round_failed("the reviewer's turn failed");
    assert_eq!(t.failure.as_ref().unwrap().code, Some(state("c2", "a")));

    for _ in 0..10 {
        // A tick: the status is unchanged, so the digest is skipped…
        t.record_sighting(&skipped("c2"));
        // …and the gate won't decide on the cheap half.
        assert_eq!(t.automatic_review(), AutoReview::NeedsFullRead);
        // The full read finds A again: blocked.
        t.record_sighting(&read("c2", "a"));
        assert_eq!(t.automatic_review(), AutoReview::Block);
    }

    // The author edits the same dirty file: B, same paths and kinds.
    t.record_sighting(&skipped("c2"));
    assert_eq!(t.automatic_review(), AutoReview::NeedsFullRead);
    t.record_sighting(&read("c2", "b"));
    assert_eq!(
        t.automatic_review(),
        AutoReview::Allow,
        "a new state: one attempt"
    );

    // …and back to A's exact bytes before any attempt: the failed state.
    t.record_sighting(&read("c2", "a"));
    assert_eq!(t.automatic_review(), AutoReview::Block, "A stays blocked");

    // B again; its one attempt runs and fails: B is blocked too.
    t.record_sighting(&read("c2", "b"));
    assert_eq!(t.automatic_review(), AutoReview::Allow);
    t.begin(reviewer(), Trigger::ReReview, state("c2", "b"), Some(7), 0)
        .unwrap();
    t.round_failed("boom");
    t.record_sighting(&read("c2", "b"));
    assert_eq!(t.automatic_review(), AutoReview::Block, "and only one");
}

/// Every gate decision is made on a read that has its digest: a read that
/// skipped it, against a failure on the same cheap half, only ever asks.
#[test]
fn the_gate_never_decides_on_a_skipped_read() {
    for failed_on in [state("c2", "a"), unreadable_state("c2")] {
        let mut t = reviewed();
        t.record_failure(FailureOrigin::Start, "boom", Some(failed_on.clone()));
        t.record_sighting(&skipped("c2"));
        assert_eq!(
            t.automatic_review(),
            AutoReview::NeedsFullRead,
            "{failed_on:?}"
        );
    }
    // A skipped read on another cheap half is Different from the failure:
    // decided without a digest because the cheap halves alone settle it.
    let mut t = reviewed();
    t.record_failure(FailureOrigin::Start, "boom", Some(state("c2", "a")));
    t.record_sighting(&skipped("c3"));
    assert_eq!(t.automatic_review(), AutoReview::Allow);
}

/// An unreadable state is a state of its own: retried on the same unreadable
/// state never, and once it becomes readable, once.
#[test]
fn an_unreadable_state_is_blocked_until_it_becomes_readable_again() {
    let mut t = reviewed();
    t.record_sighting(&unverified("c1"));
    assert_eq!(t.derive(0), Phase::Stale);
    let failure = t.failure.clone().unwrap();
    assert_eq!(failure.origin, FailureOrigin::Scan);
    assert_eq!(failure.code, Some(unreadable_state("c1")));
    assert_eq!(
        t.automatic_review(),
        AutoReview::Block,
        "a round would fail the same way"
    );
    assert!(t.auto_budget_left(7), "and none of the budget was spent");

    // The file becomes readable again, with new content: a new state.
    t.record_sighting(&read("c1", "d2"));
    assert_eq!(
        t.failure, None,
        "a read of another state clears the scan failure"
    );
    assert_eq!(t.derive(0), Phase::Stale);
    assert_eq!(t.automatic_review(), AutoReview::Allow);

    // Readable again with the reviewed bytes: current, nothing to review.
    let mut t = reviewed();
    t.record_sighting(&unverified("c1"));
    t.record_sighting(&read("c1", "d1"));
    assert_eq!(t.derive(0), Phase::Clean);
    assert_eq!(t.automatic_review(), AutoReview::Block);
}

#[test]
fn reviewed_code_and_never_attempted_checkouts_want_no_automatic_review() {
    assert_eq!(
        Tracked::default().automatic_review(),
        AutoReview::Block,
        "nothing read"
    );
    let mut never = Tracked::default();
    never.record_sighting(&skipped("c1"));
    assert_eq!(
        never.automatic_review(),
        AutoReview::Block,
        "never reviewed, never attempted"
    );
    // A first review that failed on a read state: blocked on that state, one
    // attempt once the code moves.
    never.record_failure(FailureOrigin::Start, "boom", Some(state("c1", "a")));
    never.record_sighting(&read("c1", "a"));
    assert_eq!(never.automatic_review(), AutoReview::Block);
    never.record_sighting(&skipped("c2"));
    assert_eq!(never.automatic_review(), AutoReview::Allow);
    // A failure with no complete state blocks nothing.
    let mut none = Tracked::default();
    none.record_failure(FailureOrigin::Start, "no reviewer", None);
    none.record_sighting(&skipped("c1"));
    assert_eq!(none.automatic_review(), AutoReview::Allow);
}

/// Stored states persist whole, and records from earlier builds migrate: a
/// review's `cheap#digest` string becomes that verified state, a failure's
/// cheap-only signature becomes no state (which blocks nothing).
#[test]
fn code_states_persist_whole_and_old_string_forms_migrate() {
    let c = conn();
    let key = ("arabia".to_string(), None);
    let mut t = reviewed();
    t.record_failure(FailureOrigin::Scan, "x", Some(unreadable_state("c1")));
    persist::save(&c, &key, &t).unwrap();
    let (_, back) = persist::load_all(&c).unwrap().pop().unwrap();
    assert_eq!(back.reviewed, t.reviewed);
    assert_eq!(back.round_code, t.round_code);
    assert_eq!(back.failure, t.failure);

    let old = serde_json::json!({
        "round": 2,
        "reviewed_signature": "c1#d1",
        "failure": {
            "reason": "boom",
            "origin": "round",
            "cheap_signature": "c1",
            "at_ms": 5
        },
        "phase": "failed"
    });
    let migrated: Tracked = serde_json::from_value(old).unwrap();
    assert_eq!(migrated.reviewed, Some(state("c1", "d1")));
    let failure = migrated.failure.clone().unwrap();
    assert_eq!(failure.code, None, "a cheap-only signature names no state");
    assert_eq!(failure.origin, FailureOrigin::Round);
    let mut migrated = migrated.loaded();
    migrated.record_sighting(&read("c1", "d9"));
    assert_eq!(
        migrated.automatic_review(),
        AutoReview::Allow,
        "one retry allowed"
    );

    let unverified_old: Tracked =
        serde_json::from_value(serde_json::json!({ "reviewed_signature": "c1#unverified" }))
            .unwrap();
    assert!(matches!(
        unverified_old.reviewed.unwrap().content,
        Content::Unverified { .. }
    ));
    let no_digest: Tracked =
        serde_json::from_value(serde_json::json!({ "reviewed_signature": "c1" })).unwrap();
    let mut no_digest = no_digest.loaded();
    no_digest.record_sighting(&read("c1", "d1"));
    assert_eq!(
        no_digest.derive(0),
        Phase::Stale,
        "never shown the same: stale"
    );
}

// ── one at a time, one report per turn ─────────────────────────────────────

#[test]
fn a_second_start_is_refused_while_one_is_in_flight() {
    let mut t = begun(Trigger::Manual, None);
    assert_eq!(
        t.begin(reviewer(), Trigger::Manual, state("c1", "d1"), None, 1_001),
        Err("a review is already running on this checkout")
    );
    assert_eq!(t.round, 1);
}

#[test]
fn a_second_report_in_one_turn_is_refused_and_none_without_a_flight() {
    let mut t = Tracked::default();
    assert_eq!(
        t.submitted(),
        Err("no review is in flight for this reviewer")
    );
    let mut t = begun(Trigger::Manual, None);
    t.submitted().unwrap();
    assert_eq!(t.submitted(), Err("already submitted this turn"));
}

#[test]
fn a_stop_is_a_note_not_a_failure() {
    let mut t = begun(Trigger::Manual, None);
    t.stop();
    assert_eq!(t.derive(0), Phase::Idle);
    assert_eq!(t.failure, None);
    assert_eq!(t.error().as_deref(), Some("review stopped"));
}

#[test]
fn a_start_claims_the_checkout_until_it_begins_or_lets_go() {
    let mut t = Tracked::default();
    t.claim().unwrap();
    assert_eq!(
        t.claim(),
        Err("a review is already running on this checkout")
    );
    t.release();
    t.claim().unwrap();
    t.begin(reviewer(), Trigger::Manual, state("s", "d"), None, 0)
        .unwrap();
    assert!(!t.starting, "begun: the claim became the flight");
    assert_eq!(
        t.claim(),
        Err("a review is already running on this checkout")
    );
    t.record_failure(FailureOrigin::Round, "boom", None);
    assert!(!t.starting);
    t.claim().unwrap();
}

#[test]
fn a_pr_that_had_a_round_is_not_reviewed_on_sight_again() {
    let mut t = Tracked::default();
    assert!(!t.reviewed_pr(7));
    t.begin(reviewer(), Trigger::PrOpen, state("s", "d"), Some(7), 0)
        .unwrap();
    t.turn_ended();
    assert!(t.reviewed_pr(7));
    assert!(!t.reviewed_pr(8));
}

// ── the automatic budget ───────────────────────────────────────────────────

#[test]
fn automatic_rounds_are_counted_per_pr_and_reset_when_the_pr_changes() {
    let mut t = Tracked::default();
    for _ in 0..MAX_ROUNDS_PER_PR {
        assert!(t.auto_budget_left(7));
        t.begin(reviewer(), Trigger::ReReview, state("s", "d"), Some(7), 0)
            .unwrap();
        t.submitted().unwrap();
        t.turn_ended();
    }
    assert!(!t.auto_budget_left(7), "three automatic rounds on #7");
    assert!(t.auto_budget_left(8), "another PR starts over");
    t.begin(reviewer(), Trigger::Manual, state("s", "d"), Some(7), 0)
        .unwrap();
    t.turn_ended();
    assert_eq!(t.pr_rounds, MAX_ROUNDS_PER_PR, "a manual round is free");
    t.begin(reviewer(), Trigger::ReReview, state("s", "d"), Some(8), 0)
        .unwrap();
    assert_eq!(t.pr_rounds, 1, "the count started over with #8");
}

// ── the queue ──────────────────────────────────────────────────────────────

#[test]
fn a_start_queued_behind_a_busy_coder_runs_once_it_is_free() {
    let mut t = Tracked::default();
    t.queue(Trigger::PrOpen, None);
    assert_eq!(t.derive(0), Phase::Queued);
    t.queue(Trigger::Manual, Some("ca-2".into()));
    assert_eq!(
        t.pending.as_ref().unwrap().trigger,
        Trigger::PrOpen,
        "the first stays"
    );
    assert_eq!(t.take_pending(false), None, "the coder is still busy");
    let pending = t.take_pending(true).expect("free: the start runs");
    assert_eq!(pending.trigger, Trigger::PrOpen);
    assert_ne!(t.derive(0), Phase::Queued);

    let mut t = begun(Trigger::Manual, None);
    t.queue(Trigger::ReReview, None);
    assert_eq!(t.derive(0), Phase::Reviewing, "a flight outranks the queue");
    assert_eq!(
        t.take_pending(true),
        None,
        "never while a round is in flight"
    );
    let mut failed = reviewed();
    failed.record_failure(FailureOrigin::Round, "boom", None);
    failed.queue(Trigger::Manual, None);
    assert_eq!(
        failed.derive(0),
        Phase::Queued,
        "the queue outranks a failure"
    );
}

#[test]
fn stopping_a_queued_start_cancels_it() {
    let mut t = Tracked::default();
    t.queue(Trigger::PrOpen, None);
    t.stop();
    assert_eq!(t.pending, None);
    assert_eq!(t.derive(0), Phase::Idle);
    assert_eq!(t.take_pending(true), None);
}

/// A queued start that can't begin is recorded as a queued failure; one
/// beaten by a round already on the checkout waits its turn again.
#[test]
fn a_queued_start_that_cannot_begin_reads_failed_or_waits_again() {
    let mut t = Tracked::default();
    t.queue(Trigger::PrOpen, None);
    let _ = t.take_pending(true).unwrap();
    t.claim().unwrap();
    t.record_failure(
        FailureOrigin::Queued,
        "queued review not started: No reviewer is set up — pick one in Settings › Review",
        None,
    );
    assert_eq!(t.derive(0), Phase::Failed);
    assert_eq!(
        t.error().as_deref(),
        Some("queued review not started: No reviewer is set up — pick one in Settings › Review")
    );

    let mut t = Tracked::default();
    t.queue(Trigger::Manual, Some("ca-1".into()));
    let pending = t.take_pending(true).unwrap();
    t.requeue(pending.clone());
    assert_eq!(t.pending, Some(pending), "put back, not failed");
    assert_eq!(t.failure, None);
    assert_eq!(t.derive(0), Phase::Queued);
}

#[test]
fn code_that_moved_during_the_review_keeps_the_findings_and_reads_stale() {
    let mut t = begun(Trigger::Manual, None);
    t.submitted().unwrap();
    t.record_sighting(&read("c1", "d1"));
    t.check_code_after_report();
    assert_eq!(t.note, None, "unchanged code is no news");

    for after in [read("c1", "d2"), skipped("c2")] {
        let mut t = begun(Trigger::Manual, None);
        t.submitted().unwrap();
        t.record_sighting(&after);
        t.check_code_after_report();
        t.turn_ended();
        assert_eq!(t.note.as_deref(), Some(CODE_MOVED), "{after:?}");
        assert_eq!(t.reviewed, Some(state("c1", "d1")), "the code it read");
        assert_eq!(t.derive(3), Phase::Stale);
    }

    // A read that couldn't verify is a scan failure, not "moved".
    let mut t = begun(Trigger::Manual, None);
    t.submitted().unwrap();
    t.record_sighting(&unverified("c1"));
    t.check_code_after_report();
    t.turn_ended();
    assert_eq!(t.note, None);
    assert_eq!(t.failure.as_ref().unwrap().origin, FailureOrigin::Scan);
    assert_eq!(t.derive(0), Phase::Stale);
}

// ── the signature ──────────────────────────────────────────────────────────

#[test]
fn the_cheap_signature_is_head_plus_the_dirty_paths_and_kinds_in_order() {
    use crate::git_state::{FileStatus, GitState, StatusKind};
    let file = |p: &str, additions| FileStatus {
        path: p.into(),
        kind: StatusKind::Modified,
        staged: false,
        additions,
        deletions: 0,
    };
    let git = GitState {
        branch: "feat".into(),
        parent_branch: "main".into(),
        ahead: 1,
        behind: 0,
        unpushed: 0,
        files: vec![file("b.rs", 1), file("a.rs", 0)],
        additions: 0,
        deletions: 0,
        remote_url: None,
        has_origin: false,
        head_sha: Some("abc".into()),
        blocked_config: Vec::new(),
        worktrees: Vec::new(),
    };
    let cheap = cheap_signature(&git);
    assert_eq!(cheap, "abc|a.rs:Modified,b.rs:Modified");
    // Line counts are the content digest's to judge, not the status's.
    let mut more = git.clone();
    more.files[0].additions = 9;
    assert_eq!(cheap_signature(&more), cheap);
    // The persisted string form, and back.
    let stored = state(&cheap, "d1");
    assert_eq!(stored.signature(), format!("{cheap}#d1"));
    assert_eq!(CodeState::from_signature(&stored.signature()), stored);
}

/// The content digest moves with what a status read can't see: one dirty
/// line replaced by another, an untracked file's content. Identical trees
/// digest identically.
#[tokio::test]
async fn the_content_digest_follows_the_uncommitted_content() {
    use crate::git::testing::{commit_file, init_repo};
    use crate::git_state::content_digest;
    let td = tempfile::tempdir().unwrap();
    let repo = td.path().join("repo");
    init_repo(&repo);
    commit_file(&repo, "a.txt");

    let clean = content_digest(&repo).await.unwrap();
    assert_eq!(content_digest(&repo).await.unwrap(), clean, "stable");

    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    let one = content_digest(&repo).await.unwrap();
    assert_ne!(one, clean);
    // Same file, same kind, same +1-1: only the content differs.
    std::fs::write(repo.join("a.txt"), "two\n").unwrap();
    let two = content_digest(&repo).await.unwrap();
    assert_ne!(
        two, one,
        "replacing one dirty line with another must move it"
    );
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    assert_eq!(
        content_digest(&repo).await.unwrap(),
        one,
        "same tree, same digest"
    );

    std::fs::write(repo.join("new.txt"), "first").unwrap();
    let untracked = content_digest(&repo).await.unwrap();
    assert_ne!(untracked, one);
    std::fs::write(repo.join("new.txt"), "second").unwrap();
    assert_ne!(
        content_digest(&repo).await.unwrap(),
        untracked,
        "an untracked file's content edit must move it"
    );
}

/// An incomplete scan is an error, never a digest: a modified file git can't
/// read would otherwise drop out of the digest and compare equal to the
/// reviewed tree.
#[tokio::test]
async fn an_unreadable_modified_file_makes_the_digest_an_error() {
    use crate::git::testing::{commit_file, init_repo};
    use crate::git_state::content_digest;
    use std::os::unix::fs::PermissionsExt;
    let td = tempfile::tempdir().unwrap();
    let repo = td.path().join("repo");
    init_repo(&repo);
    commit_file(&repo, "a.txt");
    std::fs::write(repo.join("a.txt"), "changed\n").unwrap();
    let file = repo.join("a.txt");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Root reads anything; the check only means something for a real user.
    let readable_anyway = std::fs::read(&file).is_ok();
    let digest = content_digest(&repo).await;
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    if readable_anyway {
        return;
    }
    let err = digest.expect_err("an unreadable file must fail the digest");
    assert!(err.to_string().starts_with("git diff:"), "{err}");
}

/// A name with a newline can't go through `--stdin-paths`; it is hashed on
/// its own, so its content still counts.
#[tokio::test]
async fn a_newline_named_untracked_file_is_hashed_by_content() {
    use crate::git::testing::{commit_file, init_repo};
    use crate::git_state::content_digest;
    let td = tempfile::tempdir().unwrap();
    let repo = td.path().join("repo");
    init_repo(&repo);
    commit_file(&repo, "a.txt");
    let odd = repo.join("odd\nname.txt");
    std::fs::write(&odd, "first").unwrap();
    let first = content_digest(&repo).await.unwrap();
    std::fs::write(&odd, "second").unwrap();
    let second = content_digest(&repo).await.unwrap();
    assert_ne!(first, second, "the content edit must move the digest");
    std::fs::write(&odd, "first").unwrap();
    assert_eq!(content_digest(&repo).await.unwrap(), first);
}

// ── the reviewer resolution ────────────────────────────────────────────────

fn conn() -> rusqlite::Connection {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::database::get_migrations()
        .to_latest(&mut conn)
        .unwrap();
    conn
}

fn custom_agent(conn: &rusqlite::Connection, id: &str, name: &str, base: &str, updated: i64) {
    conn.execute(
        "INSERT INTO custom_agents (id, name, description, color, base, model, effort, \
         instructions, skill_ids, mcp_server_ids, created_at, updated_at) \
         VALUES (?1, ?2, '', 25, ?3, 'gpt-5', 'high', 'look for bugs', '[]', '[]', 0, ?4)",
        rusqlite::params![id, name, base, updated],
    )
    .unwrap();
}

#[test]
fn the_reviewer_is_the_request_then_the_setting_then_the_newest_named_reviewer() {
    let c = conn();
    assert_eq!(resolve_reviewer(&c, None), None, "nothing to pick from");

    custom_agent(&c, "ca-old", "reviewer", "claude", 1);
    custom_agent(&c, "ca-new", "Reviewer", "codex", 2);
    custom_agent(&c, "ca-other", "Architect", "claude", 3);
    let by_name = resolve_reviewer(&c, None).unwrap();
    assert_eq!(
        by_name.custom_agent_id, "ca-new",
        "newest, case-insensitive"
    );
    assert_eq!(by_name.base, "codex");
    assert_eq!(by_name.model.as_deref(), Some("gpt-5"));
    assert_eq!(by_name.effort.as_deref(), Some("high"));

    crate::database::set_setting(&c, SETTING_AGENT, "ca-old").unwrap();
    assert_eq!(
        resolve_reviewer(&c, None).unwrap().custom_agent_id,
        "ca-old"
    );
    // A setting that no longer resolves falls through to the name.
    crate::database::set_setting(&c, SETTING_AGENT, "ca-gone").unwrap();
    assert_eq!(
        resolve_reviewer(&c, None).unwrap().custom_agent_id,
        "ca-new"
    );

    assert_eq!(
        resolve_reviewer(&c, Some("ca-other"))
            .unwrap()
            .custom_agent_id,
        "ca-other",
        "an explicit request wins"
    );
    assert_eq!(
        resolve_reviewer(&c, Some("ca-gone"))
            .unwrap()
            .custom_agent_id,
        "ca-new",
        "a dangling request falls through"
    );
}

#[test]
fn review_on_pr_open_is_on_unless_switched_off() {
    let c = conn();
    assert!(on_pr_open(&c));
    crate::database::set_setting(&c, SETTING_ON_PR_OPEN, "false").unwrap();
    assert!(!on_pr_open(&c));
    crate::database::set_setting(&c, SETTING_ON_PR_OPEN, "true").unwrap();
    assert!(on_pr_open(&c));
}
