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
fn the_signature_is_head_plus_the_dirty_paths_in_order() {
    use crate::git_state::{FileStatus, GitState, StatusKind};
    let file = |p: &str| FileStatus {
        path: p.into(),
        kind: StatusKind::Modified,
        staged: false,
        additions: 0,
        deletions: 0,
    };
    let git = GitState {
        branch: "feat".into(),
        parent_branch: "main".into(),
        ahead: 1,
        behind: 0,
        unpushed: 0,
        files: vec![file("b.rs"), file("a.rs")],
        additions: 0,
        deletions: 0,
        remote_url: None,
        has_origin: false,
        head_sha: Some("abc".into()),
        blocked_config: Vec::new(),
        worktrees: Vec::new(),
    };
    assert_eq!(signature(&git), "abc|a.rs:Modified:+0-0,b.rs:Modified:+0-0");
    // More lines in an already-dirty file is a change too.
    let mut edited = git.clone();
    edited.files[1].additions = 3;
    assert_ne!(signature(&edited), signature(&git));
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
