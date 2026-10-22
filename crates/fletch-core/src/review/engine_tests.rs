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

fn begun(trigger: Trigger, pr: Option<u32>) -> Tracked {
    let mut t = Tracked::default();
    t.begin(reviewer(), trigger, "sha1|".into(), pr, 1_000)
        .unwrap();
    t
}

// ── the derived phase ──────────────────────────────────────────────────────

#[test]
fn an_untouched_checkout_is_idle_and_a_flight_is_reviewing() {
    let t = Tracked::default();
    assert_eq!(t.derive(Some("sha1|"), 0), Phase::Idle);
    let t = begun(Trigger::Manual, None);
    assert_eq!(t.derive(Some("sha1|"), 0), Phase::Reviewing);
    assert_eq!(t.round, 1);
    assert_eq!(t.trigger, Some(Trigger::Manual));
}

#[test]
fn a_report_makes_the_code_reviewed_then_the_threads_and_the_code_decide() {
    let mut t = begun(Trigger::Manual, None);
    t.submitted().unwrap();
    t.turn_ended();
    assert_eq!(t.reviewed_signature.as_deref(), Some("sha1|"));
    assert_eq!(t.derive(Some("sha1|"), 0), Phase::Clean);
    assert_eq!(t.derive(Some("sha1|"), 2), Phase::Findings);
    // The code moved: stale whatever the threads say. An unreadable checkout
    // is not a moved one.
    assert_eq!(t.derive(Some("sha2|a.rs"), 2), Phase::Stale);
    assert_eq!(t.derive(Some("sha2|"), 0), Phase::Stale);
    assert_eq!(t.derive(None, 0), Phase::Clean);
}

#[test]
fn a_turn_that_ends_without_a_report_fails_loudly_until_the_next_start() {
    let mut t = begun(Trigger::PrOpen, Some(7));
    t.turn_ended();
    assert_eq!(t.derive(Some("sha1|"), 0), Phase::Failed);
    assert!(t.failed.as_deref().unwrap().contains("without submitting"));
    assert_eq!(t.reviewed_signature, None, "nothing was reviewed");
    // The next start clears the failure and the note.
    t.error = Some("note".into());
    t.begin(reviewer(), Trigger::Manual, "sha1|".into(), Some(7), 2_000)
        .unwrap();
    assert_eq!(t.failed, None);
    assert_eq!(t.error, None);
    assert_eq!(t.round, 2);
}

// ── one at a time, one report per turn ─────────────────────────────────────

#[test]
fn a_second_start_is_refused_while_one_is_in_flight() {
    let mut t = begun(Trigger::Manual, None);
    assert_eq!(
        t.begin(reviewer(), Trigger::Manual, "sha1|".into(), None, 1_001),
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
fn a_stop_is_not_a_failure() {
    let mut t = begun(Trigger::Manual, None);
    t.stop();
    assert_eq!(t.derive(Some("sha1|"), 0), Phase::Idle);
    assert_eq!(t.error.as_deref(), Some("review stopped"));
}

// ── the automatic budget ───────────────────────────────────────────────────

#[test]
fn automatic_rounds_are_counted_per_pr_and_reset_when_the_pr_changes() {
    let mut t = Tracked::default();
    for _ in 0..MAX_ROUNDS_PER_PR {
        assert!(t.auto_budget_left(7));
        t.begin(reviewer(), Trigger::ReReview, "s".into(), Some(7), 0)
            .unwrap();
        t.submitted().unwrap();
        t.turn_ended();
    }
    assert!(!t.auto_budget_left(7), "three automatic rounds on #7");
    assert!(t.auto_budget_left(8), "another PR starts over");
    // A manual round never spends the budget.
    t.begin(reviewer(), Trigger::Manual, "s".into(), Some(7), 0)
        .unwrap();
    t.turn_ended();
    assert_eq!(t.pr_rounds, MAX_ROUNDS_PER_PR);
    t.begin(reviewer(), Trigger::ReReview, "s".into(), Some(8), 0)
        .unwrap();
    assert_eq!(t.pr_rounds, 1, "the count started over with #8");
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
    let full = full_signature(&cheap, "d1");
    assert_eq!(cheap_part(&full), cheap);
    assert_ne!(full, full_signature(&cheap, "d2"));
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

// ── the queue ──────────────────────────────────────────────────────────────

#[test]
fn a_start_queued_behind_a_busy_coder_runs_once_it_is_free() {
    let mut t = Tracked::default();
    t.queue(Trigger::PrOpen, None);
    assert_eq!(t.derive(Some("s"), 0), Phase::Queued);
    // A second request while queued keeps the first.
    t.queue(Trigger::Manual, Some("ca-2".into()));
    assert_eq!(t.pending.as_ref().unwrap().trigger, Trigger::PrOpen);
    assert_eq!(t.pending.as_ref().unwrap().custom_agent_id, None);

    assert_eq!(t.take_pending(false), None, "the coder is still busy");
    assert_eq!(t.derive(Some("s"), 0), Phase::Queued);
    let pending = t.take_pending(true).expect("free: the start runs");
    assert_eq!(pending.trigger, Trigger::PrOpen);
    assert_ne!(t.derive(Some("s"), 0), Phase::Queued);
    t.begin(reviewer(), pending.trigger, "s".into(), Some(7), 0)
        .unwrap();
    assert_eq!(t.derive(Some("s"), 0), Phase::Reviewing);

    // Queued above everything but a review in flight.
    let mut t = begun(Trigger::Manual, None);
    t.queue(Trigger::ReReview, None);
    assert_eq!(t.derive(Some("sha1|"), 0), Phase::Reviewing);
    assert_eq!(
        t.take_pending(true),
        None,
        "never while a round is in flight"
    );
    let mut failed = Tracked {
        failed: Some("boom".into()),
        ..Tracked::default()
    };
    failed.queue(Trigger::Manual, None);
    assert_eq!(failed.derive(None, 0), Phase::Queued);
}

#[test]
fn stopping_a_queued_start_cancels_it() {
    let mut t = Tracked::default();
    t.queue(Trigger::PrOpen, None);
    t.stop();
    assert_eq!(t.pending, None);
    assert_eq!(t.derive(Some("s"), 0), Phase::Idle);
    assert_eq!(t.take_pending(true), None);
}

#[test]
fn code_that_moved_during_the_review_keeps_the_findings_and_reads_stale() {
    let mut t = begun(Trigger::Manual, None);
    t.submitted().unwrap();
    t.check_code_after_report(Some("sha1|"));
    assert_eq!(t.error, None, "unchanged code is no news");

    let mut t = begun(Trigger::Manual, None);
    t.submitted().unwrap();
    t.check_code_after_report(Some("sha2|a.rs:Modified#d"));
    t.turn_ended();
    assert_eq!(t.error.as_deref(), Some(CODE_MOVED));
    assert_eq!(
        t.reviewed_signature.as_deref(),
        Some("sha1|"),
        "the code it read"
    );
    assert_eq!(t.derive(Some("sha2|a.rs:Modified#d"), 3), Phase::Stale);
    // An unreadable checkout says nothing either way.
    let mut t = begun(Trigger::Manual, None);
    t.submitted().unwrap();
    t.check_code_after_report(None);
    assert_eq!(t.error, None);
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
    t.begin(reviewer(), Trigger::Manual, "s".into(), None, 0)
        .unwrap();
    assert!(!t.starting, "begun: the claim became the flight");
    assert_eq!(
        t.claim(),
        Err("a review is already running on this checkout")
    );
    t.fail("boom");
    assert!(!t.starting);
    t.claim().unwrap();
}

#[test]
fn a_pr_that_had_a_round_is_not_reviewed_on_sight_again() {
    let mut t = Tracked::default();
    assert!(!t.reviewed_pr(7));
    t.begin(reviewer(), Trigger::PrOpen, "s".into(), Some(7), 0)
        .unwrap();
    t.turn_ended();
    assert!(t.reviewed_pr(7));
    assert!(!t.reviewed_pr(8));
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

// ── persistence ────────────────────────────────────────────────────────────

#[test]
fn a_review_in_flight_at_shutdown_loads_as_failed_and_the_rest_as_it_was() {
    let c = conn();
    let key = |a: &str, s: Option<&str>| (a.to_string(), s.map(str::to_string));
    let mut flying = begun(Trigger::PrOpen, Some(7));
    flying.phase = Phase::Reviewing;
    persist::save(&c, &key("arabia", None), &flying).unwrap();
    let mut clean = begun(Trigger::Manual, None);
    clean.submitted().unwrap();
    clean.turn_ended();
    clean.phase = Phase::Clean;
    persist::save(&c, &key("arabia", Some("api")), &clean).unwrap();

    let rows = persist::load_all(&c).unwrap();
    assert_eq!(rows.len(), 2);
    let (k, t) = rows.iter().find(|(k, _)| k.1.is_none()).unwrap();
    assert_eq!(k, &key("arabia", None));
    assert_eq!(
        t.phase,
        Phase::Reviewing,
        "stored as it was; `load` judges it"
    );
    assert_eq!(t.flight, None, "a flight never persists");
    let (_, t) = rows.iter().find(|(k, _)| k.1.is_some()).unwrap();
    assert_eq!(t.reviewed_signature.as_deref(), Some("sha1|"));

    persist::delete_for_agent(&c, "arabia").unwrap();
    assert!(persist::load_all(&c).unwrap().is_empty());
}
