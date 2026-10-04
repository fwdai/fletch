// The driver's wiring, against a scripted world: what a pass enrolls and drops,
// what it reads (and how rarely), what it hands the agent, how a verification
// lands on the cycle it was run for, and what it records. The decisions
// themselves are `step_tests.rs`; these are the parts that only exist here —
// ported from the desktop driver's tests (`src/store/autopilotSync.test.ts`).

use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::Mutex;

use super::*;
use crate::autopilot::readiness::tests::{checks, git, pr};
use crate::autopilot::tests::tracked_repo;
use crate::autopilot::GiveUpReason;
use crate::github::{MergeState, PrState};
use crate::host::sink::RecordingSink;
use crate::verify::{CheckOutcome, CheckResult};
use crate::workspace::{new_agent_record, AgentView};

const NOW: i64 = 10_000_000;

/// One `dispatch` the world was asked for: the checkout, the playbook, its
/// params.
type Dispatched = (Key, String, BTreeMap<String, String>);

#[derive(Default)]
struct Fake {
    agents: Mutex<Vec<AgentRecord>>,
    busy: Mutex<HashSet<String>>,
    in_flight: Mutex<HashSet<Key>>,
    git: Mutex<HashMap<Key, GitState>>,
    pr: Mutex<HashMap<Key, PrLive>>,
    threads: Mutex<HashMap<Key, PrComments>>,
    reads: AtomicUsize,
    thread_reads: AtomicUsize,
    dispatched: Mutex<Vec<Dispatched>>,
    verified: Mutex<Vec<Key>>,
    report: Mutex<Option<VerificationReport>>,
}

impl World for Fake {
    fn agents(&self) -> Vec<AgentRecord> {
        self.agents.lock().clone()
    }

    fn busy(&self, agent_id: &str) -> bool {
        self.busy.lock().contains(agent_id)
    }

    fn delegation_live(&self, key: &Key) -> bool {
        self.in_flight.lock().contains(key)
    }

    async fn git(&self, key: &Key) -> Option<GitState> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.git.lock().get(key).cloned()
    }

    async fn pr(&self, key: &Key) -> Option<PrLive> {
        self.pr.lock().get(key).cloned()
    }

    async fn threads(&self, key: &Key) -> Option<PrComments> {
        self.thread_reads.fetch_add(1, Ordering::SeqCst);
        self.threads.lock().get(key).cloned()
    }

    async fn dispatch(
        &self,
        key: &Key,
        action: &str,
        params: &BTreeMap<String, String>,
    ) -> Result<()> {
        self.dispatched
            .lock()
            .push((key.clone(), action.to_string(), params.clone()));
        Ok(())
    }

    async fn verify(&self, key: &Key) -> Option<VerificationReport> {
        self.verified.lock().push(key.clone());
        self.report.lock().clone()
    }
}

fn agent(id: &str, project: &str, subdirs: &[&str]) -> AgentRecord {
    let path = std::path::PathBuf::from(format!("/r/{id}"));
    let mut record = new_agent_record(
        id.to_string(),
        id.to_string(),
        "claude".to_string(),
        tracked_repo(&path, subdirs[0]),
        String::new(),
        AgentView::Custom,
    );
    record.project_id = project.to_string();
    for subdir in &subdirs[1..] {
        record.repos.push(tracked_repo(&path.join(subdir), subdir));
    }
    record
}

fn key(agent: &str, subdir: Option<&str>) -> Key {
    (agent.to_string(), subdir.map(str::to_string))
}

/// A checkout whose open PR is failing `test` — the world autopilot acts on.
fn failing(world: &Fake, k: &Key, sha: &str) {
    world.git.lock().insert(
        k.clone(),
        GitState {
            head_sha: Some(sha.to_string()),
            ..git()
        },
    );
    world.pr.lock().insert(
        k.clone(),
        PrLive {
            state: pr(),
            checks: Some(checks(MergeState::Blocked, &["test"])),
        },
    );
}

/// The same checkout, green at a new commit.
fn green(world: &Fake, k: &Key) {
    world.git.lock().insert(
        k.clone(),
        GitState {
            head_sha: Some("fixed".to_string()),
            ..git()
        },
    );
    world.pr.lock().insert(
        k.clone(),
        PrLive {
            state: pr(),
            checks: Some(checks(MergeState::Clean, &[])),
        },
    );
}

struct Harness {
    table: &'static Table,
    ctx: Arc<EngineCtx>,
    sink: Arc<RecordingSink>,
    world: Arc<Fake>,
    _dir: tempfile::TempDir,
}

impl Harness {
    fn new(agents: Vec<AgentRecord>) -> Self {
        let (ctx, sink, dir) = crate::host::ctx::test_ctx();
        let world = Arc::new(Fake::default());
        *world.agents.lock() = agents;
        Self {
            table: Box::leak(Box::new(Table::default())),
            ctx,
            sink,
            world,
            _dir: dir,
        }
    }

    async fn pass(&self, now: i64) {
        run_pass(self.table, &self.ctx, &self.world, now).await;
    }

    fn events(&self, name: &str) -> Vec<serde_json::Value> {
        self.sink
            .events()
            .into_iter()
            .filter(|(n, _)| n == name)
            .map(|(_, p)| p)
            .collect()
    }

    fn log(&self) -> Vec<LogEntry> {
        store::read_log(&self.ctx.db.lock(), store::LogScope::All).unwrap()
    }

    fn dispatched(&self) -> Vec<Dispatched> {
        self.world.dispatched.lock().clone()
    }

    fn cycle(&self, k: &Key) -> Option<super::super::step::Cycle> {
        self.table.get(k).and_then(|t| t.state.cycle)
    }

    /// Wait out a verification spawned by the last pass.
    async fn verified(&self, k: &Key) {
        for _ in 0..200 {
            if !self.table.get(k).is_some_and(|t| t.verifying) {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("verification never landed");
    }

    /// A project row, so a `project_settings` row can reference it.
    fn project(&self, id: &str) {
        self.ctx
            .db
            .lock()
            .execute(
                "INSERT INTO projects (id, name, created_at) VALUES (?1, ?1, 0)",
                [id],
            )
            .unwrap();
    }
}

// ── enrollment ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn enrolls_an_unseen_checkout_and_acts_on_it_in_the_same_pass() {
    let h = Harness::new(vec![agent("aare", "p1", &["repo"])]);
    let k = key("aare", None);
    failing(&h.world, &k, "sha1");

    h.pass(NOW).await;

    assert_eq!(
        h.dispatched(),
        [(
            k.clone(),
            "fix-checks".to_string(),
            BTreeMap::from([("failing".to_string(), "test".to_string())])
        )]
    );
    let cycle = h.cycle(&k).expect("a cycle is open");
    assert_eq!(cycle.rung, DelegationKind::FixChecks);
    assert_eq!(cycle.attempt, 1);
    // Enrolled, then the cycle opened: two rows, the last one mid-cycle.
    let rows = h.events(super::super::EVENT_STATE);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["enrolled"], true);
    assert_eq!(rows[0]["cycle"], serde_json::Value::Null);
    assert_eq!(rows[1]["cycle"]["rung"], "fix-checks");
    assert_eq!(rows[1]["cycle"]["phase"], "working");
    // The dispatch is logged with the attempt of the cycle it opened.
    let log = h.log();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].outcome, Outcome::Dispatch);
    assert_eq!(log[0].attempt, 1);
    assert_eq!(h.events(super::super::EVENT_LOG).len(), 1);
}

#[tokio::test]
async fn does_nothing_for_a_switched_off_project_and_forgets_its_state() {
    let h = Harness::new(vec![agent("aare", "p1", &["repo"])]);
    let k = key("aare", None);
    failing(&h.world, &k, "sha1");
    h.table.enroll(&k);
    h.project("p1");
    store::set_project(&h.ctx.db.lock(), "p1", false).unwrap();

    h.pass(NOW).await;

    assert!(h.dispatched().is_empty());
    assert!(!h.table.contains(&k));
    let rows = h.events(super::super::EVENT_STATE);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["enrolled"], false);
    assert_eq!(rows[0]["project_enabled"], false);
    assert_eq!(h.world.reads.load(Ordering::SeqCst), 0, "nothing was read");
}

#[tokio::test]
async fn does_nothing_for_a_paused_agent_the_kill_switch() {
    let h = Harness::new(vec![agent("aare", "p1", &["repo"])]);
    let k = key("aare", None);
    failing(&h.world, &k, "sha1");
    h.table.enroll(&k);
    store::set_agent_paused(&h.ctx.db.lock(), "aare", true, |_| true).unwrap();

    h.pass(NOW).await;

    assert!(h.dispatched().is_empty());
    assert!(!h.table.contains(&k));
    assert_eq!(h.events(super::super::EVENT_STATE)[0]["paused"], true);

    // Un-paused: picked back up fresh on the next pass.
    store::set_agent_paused(&h.ctx.db.lock(), "aare", false, |_| true).unwrap();
    h.pass(NOW + 10_000).await;
    assert_eq!(h.dispatched().len(), 1);
}

#[tokio::test]
async fn drops_an_enrollment_whose_agent_is_gone() {
    let h = Harness::new(Vec::new());
    let k = key("vanished", None);
    h.table.enroll(&k);

    h.pass(NOW).await;

    assert!(!h.table.contains(&k));
    assert!(
        h.events(super::super::EVENT_STATE).is_empty(),
        "nothing to say about an agent no client lists"
    );
}

// ── one dispatch per agent per pass ────────────────────────────────────────

#[tokio::test]
async fn hands_a_rung_to_only_one_checkout_of_a_multi_repo_agent() {
    let h = Harness::new(vec![agent("aare", "p1", &["repo", "web"])]);
    let (primary, web) = (key("aare", None), key("aare", Some("web")));
    failing(&h.world, &primary, "sha1");
    failing(&h.world, &web, "sha1");

    h.pass(NOW).await;
    assert_eq!(
        h.dispatched().len(),
        1,
        "two triggers would coalesce into one turn"
    );
    assert_eq!(h.dispatched()[0].0, primary);

    // The sibling's turn is running: the loser keeps waiting.
    h.world.busy.lock().insert("aare".to_string());
    h.pass(NOW + 10_000).await;
    assert_eq!(h.dispatched().len(), 1);
}

#[tokio::test]
async fn still_dispatches_for_different_agents_in_one_pass() {
    let h = Harness::new(vec![
        agent("aare", "p1", &["repo"]),
        agent("rhone", "p1", &["repo"]),
    ]);
    failing(&h.world, &key("aare", None), "sha1");
    failing(&h.world, &key("rhone", None), "sha1");
    h.pass(NOW).await;
    assert_eq!(h.dispatched().len(), 2);
}

#[tokio::test]
async fn never_dispatches_over_a_live_delegation_or_a_running_turn() {
    let h = Harness::new(vec![
        agent("aare", "p1", &["repo"]),
        agent("rhone", "p1", &["repo"]),
    ]);
    failing(&h.world, &key("aare", None), "sha1");
    failing(&h.world, &key("rhone", None), "sha1");
    h.world.in_flight.lock().insert(key("aare", None));
    h.world.busy.lock().insert("rhone".to_string());
    h.pass(NOW).await;
    assert!(h.dispatched().is_empty());
}

// ── what a pass reads ──────────────────────────────────────────────────────

#[tokio::test]
async fn a_working_cycle_is_judged_without_reading_anything() {
    let h = Harness::new(vec![agent("aare", "p1", &["repo"])]);
    let k = key("aare", None);
    failing(&h.world, &k, "sha1");
    h.pass(NOW).await;
    let reads = h.world.reads.load(Ordering::SeqCst);

    h.world.busy.lock().insert("aare".to_string());
    h.pass(NOW + 10_000).await;

    assert_eq!(
        h.world.reads.load(Ordering::SeqCst),
        reads,
        "the agent is mid-turn"
    );
    assert_eq!(h.cycle(&k).unwrap().phase, CyclePhase::Working);
}

#[tokio::test]
async fn reads_review_threads_at_most_once_a_minute() {
    let h = Harness::new(vec![agent("aare", "p1", &["repo"])]);
    let k = key("aare", None);
    green(&h.world, &k);

    h.pass(NOW).await;
    h.pass(NOW + 10_000).await;
    h.pass(NOW + 20_000).await;
    assert_eq!(h.world.thread_reads.load(Ordering::SeqCst), 1);

    h.pass(NOW + THREADS_EVERY_MS).await;
    assert_eq!(h.world.thread_reads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn reads_no_threads_without_an_open_pr() {
    let h = Harness::new(vec![agent("aare", "p1", &["repo"])]);
    let k = key("aare", None);
    h.world.git.lock().insert(k.clone(), git());
    h.pass(NOW).await;
    assert_eq!(h.world.thread_reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_comment_round_is_judged_on_threads_read_after_it_ran() {
    let h = Harness::new(vec![agent("aare", "p1", &["repo"])]);
    let k = key("aare", None);
    green(&h.world, &k);
    h.world.threads.lock().insert(
        k.clone(),
        PrComments {
            unresolved: vec![crate::autopilot::readiness::tests::thread("t1", false)],
        },
    );
    h.pass(NOW).await;
    assert_eq!(h.dispatched()[0].1, "resolve-comments");
    // Turn over: the cycle starts awaiting evidence...
    h.pass(NOW + 10_000).await;
    assert_eq!(h.cycle(&k).unwrap().phase, CyclePhase::AwaitingEvidence);
    // ...and the agent resolved the thread. The next pass, well inside the
    // minute, reads the threads again rather than judging the stale copy.
    h.world.threads.lock().insert(
        k.clone(),
        PrComments {
            unresolved: Vec::new(),
        },
    );
    h.pass(NOW + 20_000).await;
    assert_eq!(h.world.thread_reads.load(Ordering::SeqCst), 2);
    assert!(h.cycle(&k).is_none());
    assert_eq!(h.log()[0].outcome, Outcome::Settle);
}

// ── verification ───────────────────────────────────────────────────────────

fn failed_tests() -> VerificationReport {
    VerificationReport {
        checks: vec![CheckResult {
            name: "test".to_string(),
            command: "cargo test".to_string(),
            outcome: CheckOutcome::Failed,
            duration_ms: 1,
            tail: Vec::new(),
        }],
    }
}

#[tokio::test]
async fn a_fix_is_verified_once_and_judged_on_that_verdict() {
    let h = Harness::new(vec![agent("aare", "p1", &["repo"])]);
    let k = key("aare", None);
    failing(&h.world, &k, "sha1");
    *h.world.report.lock() = Some(failed_tests());
    h.pass(NOW).await;

    // The turn ended: awaiting evidence, and the project's own checks run.
    h.pass(NOW + 10_000).await;
    assert_eq!(h.cycle(&k).unwrap().phase, CyclePhase::AwaitingEvidence);
    h.verified(&k).await;
    assert_eq!(h.world.verified.lock().len(), 1);

    // CI looks green, but the local tests failed: not fixed.
    green(&h.world, &k);
    h.pass(NOW + 20_000).await;
    assert!(h.cycle(&k).is_none());
    assert_eq!(h.log()[0].outcome, Outcome::Retry);
    assert_eq!(h.log()[0].attempt, 1);
    assert_eq!(
        h.world.verified.lock().len(),
        1,
        "one verification per cycle"
    );
}

#[tokio::test]
async fn a_verification_that_could_not_run_is_judged_on_ci_alone() {
    let h = Harness::new(vec![agent("aare", "p1", &["repo"])]);
    let k = key("aare", None);
    failing(&h.world, &k, "sha1");
    h.pass(NOW).await;
    h.pass(NOW + 10_000).await;
    h.verified(&k).await;
    assert!(h.table.get(&k).unwrap().verdict.is_none());

    green(&h.world, &k);
    h.pass(NOW + 20_000).await;
    assert_eq!(h.log()[0].outcome, Outcome::Settle);
}

#[tokio::test]
async fn a_checkout_is_held_while_its_verification_runs() {
    let h = Harness::new(vec![agent("aare", "p1", &["repo"])]);
    let k = key("aare", None);
    failing(&h.world, &k, "sha1");
    h.pass(NOW).await;
    h.table.update(&k, |t| {
        t.state.advance(CyclePhase::AwaitingEvidence, NOW);
        t.verifying = true;
    });
    let reads = h.world.reads.load(Ordering::SeqCst);
    h.pass(NOW + 10_000).await;
    assert_eq!(h.world.reads.load(Ordering::SeqCst), reads);
    assert!(h.cycle(&k).is_some());
}

// ── what it records ────────────────────────────────────────────────────────

#[tokio::test]
async fn carries_the_attempt_forward_so_a_retry_reads_as_the_try_it_was() {
    let h = Harness::new(vec![agent("aare", "p1", &["repo"])]);
    let k = key("aare", None);
    failing(&h.world, &k, "sha1");
    h.pass(NOW).await; // dispatch, try 1
    h.pass(NOW + 10_000).await; // verify (no report)
    h.verified(&k).await;
    failing(&h.world, &k, "sha2"); // the agent committed; still red
    h.pass(NOW + 20_000).await; // retry
    h.pass(NOW + 30_000).await; // dispatch, try 2
    let attempts: Vec<(Outcome, u32)> = h
        .log()
        .iter()
        .rev()
        .map(|e| (e.outcome, e.attempt))
        .collect();
    assert_eq!(
        attempts,
        [
            (Outcome::Dispatch, 1),
            (Outcome::Retry, 1),
            (Outcome::Dispatch, 2)
        ]
    );
}

#[tokio::test]
async fn logs_a_give_up_with_the_reason_then_goes_quiet() {
    let h = Harness::new(vec![agent("aare", "p1", &["repo"])]);
    let k = key("aare", None);
    failing(&h.world, &k, "sha1");
    h.table.enroll(&k);
    let sit = blocker_fingerprint_of(&h, &k);
    h.table.update(&k, |t| {
        // Two tries already spent on this very situation.
        t.state.situation = sit.clone();
        t.state.attempts.insert(DelegationKind::FixChecks, 2);
        t.state
            .open_cycle(DelegationKind::FixChecks, "elsewhere".into(), sit, NOW);
        t.state.advance(CyclePhase::AwaitingEvidence, NOW);
    });

    h.pass(NOW + 10_000).await;

    let log = h.log();
    assert_eq!(log[0].outcome, Outcome::GiveUp);
    assert_eq!(log[0].attempt, 3);
    assert_eq!(log[0].reason, Some(GiveUpReason::BudgetSpent));
    let events = h.events(super::super::EVENT_LOG);
    assert_eq!(events.last().unwrap()["outcome"], "give-up");
    assert_eq!(events.last().unwrap()["reason"], "budget-spent");

    // The budget for this situation is spent: nothing more, and no news.
    h.pass(NOW + 20_000).await;
    h.pass(NOW + 30_000).await;
    assert!(h.dispatched().is_empty());
    assert_eq!(h.log().len(), 1);
}

#[tokio::test]
async fn stays_silent_on_a_tick_with_nothing_to_do() {
    let h = Harness::new(vec![agent("aare", "p1", &["repo"])]);
    green(&h.world, &key("aare", None));
    h.pass(NOW).await;
    let before = h.sink.events().len();
    h.pass(NOW + 10_000).await;
    assert_eq!(h.sink.events().len(), before);
    assert!(h.log().is_empty());
}

/// The situation the checkout's current world is, as the step would stamp it.
fn blocker_fingerprint_of(h: &Harness, k: &Key) -> String {
    let live = h.world.pr.lock().get(k).cloned().unwrap();
    let input = ReadinessInput {
        git: h.world.git.lock().get(k).cloned(),
        pr: Some::<PrState>(live.state),
        checks: live.checks,
        comments: None,
    };
    super::super::step::blocker_fingerprint(&super::super::readiness::detect_blockers(&input))
}
