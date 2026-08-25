use super::*;

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::process::Command as Sh;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::broadcast;

use crate::workflow::attempt::Deadlines;
use crate::workflow::driver::{BoxFuture, SpawnedAgent, TurnUsage};
use crate::workflow::spec::{AgentSpec, Finalize, Integrate, Join, Parallel};

fn sh(dir: &Path, args: &[&str]) {
    let out = Sh::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Sh::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Whether `git <args>` exits zero — for the predicate commands whose answer is
/// the exit status, not stdout.
fn git_ok(dir: &Path, args: &[&str]) -> bool {
    Sh::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git")
        .status
        .success()
}

/// How the stub "agent" behaves for one run. Everything the kernel's phases can
/// hinge on is expressible here, so each test configures rather than defines a
/// driver.
#[derive(Default)]
struct Behavior {
    /// The agent commits work in the adopted workspace during its turn.
    commit: bool,
    fail_spawn: bool,
    /// The turn never ends — models a wedged agent (the wall-timeout path).
    hang: bool,
    /// `verdict.json` body the nth agent writes into its step's blackboard dir.
    verdicts: Vec<Option<String>>,
    /// Raised when the prompt lands, then the turn hangs — models a cancel
    /// arriving mid-step.
    cancel_on_prompt: Option<Arc<AtomicBool>>,
}

/// A real-git stub driver for the kernel: it never clones. Every spawn adopts
/// `SpawnReq::existing_workspace` (the supervisor-side contract this runner is
/// written against) and the "agent" works directly in that tree.
struct Stub {
    behavior: Behavior,
    blackboard: PathBuf,
    /// Step ids in run order, so the nth agent knows which blackboard dir is its
    /// own.
    steps: Vec<String>,
    tx: broadcast::Sender<StatusEvent>,
    state: Mutex<StubState>,
}

#[derive(Default)]
struct StubState {
    statuses: HashMap<String, AgentStatus>,
    /// agent id → (spawn ordinal, adopted workspace).
    agents: HashMap<String, (usize, PathBuf)>,
    /// Adopted workspaces in spawn order — the proof that steps share one tree.
    adopted: Vec<PathBuf>,
    stops: usize,
    archives: usize,
}

impl Stub {
    fn new(behavior: Behavior, blackboard: PathBuf, steps: Vec<String>) -> Arc<Self> {
        Arc::new(Self {
            behavior,
            blackboard,
            steps,
            tx: broadcast::channel(256).0,
            state: Mutex::new(StubState::default()),
        })
    }
    fn set(&self, id: &str, s: AgentStatus) {
        self.state.lock().statuses.insert(id.to_string(), s.clone());
        let _ = self.tx.send(StatusEvent {
            agent_id: id.to_string(),
            status: s,
        });
    }
    fn adopted(&self) -> Vec<PathBuf> {
        self.state.lock().adopted.clone()
    }
    fn stops(&self) -> usize {
        self.state.lock().stops
    }
    fn archives(&self) -> usize {
        self.state.lock().archives
    }
}

impl AgentDriver for Stub {
    fn spawn(&self, req: SpawnReq) -> BoxFuture<'_, Result<SpawnedAgent>> {
        Box::pin(async move {
            if self.behavior.fail_spawn {
                return Err(Error::Other("no agent binary".into()));
            }
            let workspace = req
                .existing_workspace
                .clone()
                .expect("kernel spawns must adopt the run workspace");
            let id = {
                let mut st = self.state.lock();
                st.adopted.push(workspace.clone());
                let ordinal = st.adopted.len() - 1;
                let id = format!("stub-{}", ordinal + 1);
                st.agents.insert(id.clone(), (ordinal, workspace.clone()));
                id
            };
            self.set(&id, AgentStatus::Idle);
            Ok(SpawnedAgent {
                agent_id: id,
                worktree: workspace,
            })
        })
    }

    fn status(&self, id: &str) -> Option<AgentStatus> {
        self.state.lock().statuses.get(id).cloned()
    }

    fn subscribe(&self) -> broadcast::Receiver<StatusEvent> {
        self.tx.subscribe()
    }

    fn send_message<'a>(&'a self, id: &'a str, _text: String) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let (ordinal, workspace) = self.state.lock().agents.get(id).cloned().unwrap();
            if let Some(flag) = &self.behavior.cancel_on_prompt {
                flag.store(true, Ordering::SeqCst);
                return Ok(());
            }
            if self.behavior.hang {
                return Ok(());
            }
            self.set(id, AgentStatus::Running);
            if self.behavior.commit {
                std::fs::write(workspace.join(format!("{id}.txt")), "work").unwrap();
                sh(&workspace, &["add", "-A"]);
                sh(&workspace, &["commit", "-qm", "agent work"]);
            }
            if let Some(Some(body)) = self.behavior.verdicts.get(ordinal) {
                let dir = self.blackboard.join(&self.steps[ordinal]);
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("verdict.json"), body).unwrap();
            }
            self.set(id, AgentStatus::Idle);
            Ok(())
        })
    }

    fn stop<'a>(&'a self, _id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.state.lock().stops += 1;
            Ok(())
        })
    }

    fn archive<'a>(&'a self, _id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.state.lock().archives += 1;
            Ok(())
        })
    }

    fn last_activity(&self, _id: &str) -> Option<i64> {
        None
    }

    fn turn_usage(&self, _id: &str) -> Option<TurnUsage> {
        None
    }
}

fn step(id: &str, gate: Gate) -> Step {
    Step {
        id: id.to_string(),
        agent: "coder".to_string(),
        goal: format!("do {id}"),
        gate,
        budgets: None,
        comms: vec![],
    }
}

fn spec_of(blocks: Vec<Block>, finalize: Option<Finalize>) -> Spec {
    let mut agents = BTreeMap::new();
    agents.insert(
        "coder".to_string(),
        AgentSpec {
            base: "codex".to_string(),
            model: None,
            effort: None,
            instructions: None,
            skills: vec![],
            mcp_servers: vec![],
            custom_agent: None,
        },
    );
    Spec {
        version: 1,
        name: "demo".to_string(),
        description: None,
        budgets: None,
        agents,
        workflow: blocks,
        finalize,
    }
}

/// A source repo with a bare `origin`, so a finalize push has somewhere to land.
/// Returns `(source, bare, base_sha)`.
fn fixture_repo(tmp: &Path) -> (PathBuf, PathBuf, String) {
    let bare = tmp.join("origin.git");
    std::fs::create_dir_all(&bare).unwrap();
    sh(&bare, &["init", "-q", "--bare", "-b", "main"]);
    let source = tmp.join("source");
    std::fs::create_dir_all(&source).unwrap();
    sh(&source, &["init", "-q", "-b", "main"]);
    sh(&source, &["config", "user.email", "t@t.t"]);
    sh(&source, &["config", "user.name", "t"]);
    std::fs::write(source.join("README"), "base").unwrap();
    sh(&source, &["add", "-A"]);
    sh(&source, &["commit", "-qm", "base"]);
    sh(
        &source,
        &["remote", "add", "origin", bare.to_str().unwrap()],
    );
    let base_sha = git_out(&source, &["rev-parse", "HEAD"]);
    (source, bare, base_sha)
}

struct Fixture {
    db: super::super::Db,
    run_dir: PathBuf,
    bare: PathBuf,
    cancel: Arc<AtomicBool>,
}

fn scaffold(tmp: &Path, run_id: &str, branch: &str, spec: &Spec) -> Fixture {
    let (source, bare, base_sha) = fixture_repo(tmp);
    let run_dir = tmp.join("rundir");
    std::fs::create_dir_all(blackboard::blackboard_dir(&run_dir)).unwrap();
    let db = crate::database::init(tmp).unwrap();
    db.lock()
        .execute(
            "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                    base_sha,status,budgets_json,spent_json,created_at,updated_at)
             VALUES (?1,'demo',?2,'the task','p',?3,?4,?5,?6,'pending','{}','{}',0,0)",
            rusqlite::params![
                run_id,
                serde_json::to_string(spec).unwrap(),
                source.to_string_lossy(),
                run_dir.to_string_lossy(),
                branch,
                base_sha,
            ],
        )
        .unwrap();
    Fixture {
        db,
        run_dir,
        bare,
        cancel: Arc::new(AtomicBool::new(false)),
    }
}

fn ctx_with(fx: &Fixture, driver: Arc<dyn AgentDriver>) -> RunCtx {
    RunCtx {
        db: fx.db.clone(),
        driver,
        app: None,
        cancel: fx.cancel.clone(),
        pending_ask: Arc::new(AtomicBool::new(false)),
        deadlines: Deadlines::default(),
        runs: None,
    }
}

fn run_status(db: &super::super::Db, run_id: &str) -> (String, Option<String>) {
    db.lock()
        .query_row(
            "SELECT status, error FROM wf_run WHERE id = ?1",
            [run_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
}

fn exec_rows(db: &super::super::Db, run_id: &str) -> Vec<(String, String, Option<String>)> {
    let conn = db.lock();
    let mut stmt = conn
        .prepare("SELECT id, status, agent_id FROM wf_step_exec WHERE run_id = ?1 ORDER BY rowid")
        .unwrap();
    let rows = stmt
        .query_map([run_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap();
    rows.collect::<rusqlite::Result<Vec<_>>>().unwrap()
}

fn event_types(db: &super::super::Db, run_id: &str) -> Vec<String> {
    let conn = db.lock();
    let mut stmt = conn
        .prepare("SELECT type FROM wf_event WHERE run_id = ?1 ORDER BY seq")
        .unwrap();
    let rows = stmt.query_map([run_id], |r| r.get::<_, String>(0)).unwrap();
    rows.collect::<rusqlite::Result<Vec<_>>>().unwrap()
}

// ─────────────────────────────── the happy path ──────────────────────────────

#[tokio::test]
async fn two_steps_share_one_workspace_and_land_on_one_line() {
    let tmp = tempfile::tempdir().unwrap();
    let branch = "wf/demo-abcdef12";
    let spec = spec_of(
        vec![
            Block::Step(step("plan", Gate::Commit)),
            Block::Step(step("build", Gate::Commit)),
        ],
        Some(Finalize {
            push: true,
            open_pr: false,
            pr_base: Some("main".to_string()),
        }),
    );
    let fx = scaffold(tmp.path(), "run-kernel", branch, &spec);
    let driver = Stub::new(
        Behavior {
            commit: true,
            ..Default::default()
        },
        blackboard::blackboard_dir(&fx.run_dir),
        vec!["plan".into(), "build".into()],
    );
    let ctx = ctx_with(&fx, driver.clone());

    run_kernel(&ctx, "run-kernel").await;

    assert_eq!(run_status(&fx.db, "run-kernel").0, "done");

    // Both steps ran in the *same* checkout — the whole point of the kernel.
    let workspace = gitops::run_repo_path(&fx.run_dir);
    assert_eq!(driver.adopted(), vec![workspace.clone(), workspace.clone()]);

    // Two done execs, each with its agent stamped and its ref pinned in the
    // shared workspace.
    let execs = exec_rows(&fx.db, "run-kernel");
    assert_eq!(execs.len(), 2);
    for (exec_id, status, agent_id) in &execs {
        assert_eq!(status, "done");
        assert!(agent_id.is_some(), "agent stamped on {exec_id}");
        let pinned = git_out(&workspace, &["rev-parse", &gitops::step_ref(exec_id)]);
        assert!(!pinned.is_empty(), "refs/wf/steps/{exec_id} pinned");
    }
    // One line: the second step's ref is a descendant of the first's, and the
    // whole run is base + two commits.
    let (first, second) = (&execs[0].0, &execs[1].0);
    assert!(
        git_ok(
            &workspace,
            &[
                "merge-base",
                "--is-ancestor",
                &gitops::step_ref(first),
                &gitops::step_ref(second),
            ]
        ),
        "step 2 builds on step 1"
    );
    assert_eq!(git_out(&workspace, &["rev-list", "--count", "HEAD"]), "3");

    // Pushed, and every phase is on the timeline.
    assert_eq!(
        git_out(
            &fx.bare,
            &["rev-list", "--count", &format!("refs/heads/{branch}")]
        ),
        "3"
    );
    let events = event_types(&fx.db, "run-kernel");
    for expected in [
        event_type::RUN_LAUNCHED,
        event_type::ATTEMPT_SPAWNED,
        event_type::ATTEMPT_READY,
        event_type::PROMPT_SENT,
        event_type::TURN_ENDED,
        event_type::GATE_EVALUATED,
        event_type::BOUNDARY_COMMIT,
        event_type::FINALIZE_PUSHED,
        event_type::RUN_DONE,
    ] {
        assert!(
            events.iter().any(|e| e == expected),
            "{expected} journaled: {events:?}"
        );
    }
    // Both chats stay replayable.
    assert_eq!(driver.archives(), 2);
}

// ──────────────────────────────── verdict gate ───────────────────────────────

#[tokio::test]
async fn verdict_done_advances_the_run() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = spec_of(
        vec![
            Block::Step(step("plan", Gate::Verdict)),
            Block::Step(step("build", Gate::Verdict)),
        ],
        None,
    );
    let fx = scaffold(tmp.path(), "run-v", "wf/demo-1", &spec);
    let driver = Stub::new(
        Behavior {
            commit: true,
            verdicts: vec![
                Some(r#"{"result":"done","summary":"planned"}"#.into()),
                Some(r#"{"result":"done","summary":"built"}"#.into()),
            ],
            ..Default::default()
        },
        blackboard::blackboard_dir(&fx.run_dir),
        vec!["plan".into(), "build".into()],
    );
    let ctx = ctx_with(&fx, driver.clone());

    run_kernel(&ctx, "run-v").await;

    assert_eq!(run_status(&fx.db, "run-v").0, "done");
    assert_eq!(exec_rows(&fx.db, "run-v").len(), 2);
}

#[tokio::test]
async fn verdict_revise_fails_the_run_with_the_reason() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = spec_of(
        vec![
            Block::Step(step("plan", Gate::Verdict)),
            Block::Step(step("build", Gate::Verdict)),
        ],
        None,
    );
    let fx = scaffold(tmp.path(), "run-v2", "wf/demo-2", &spec);
    let driver = Stub::new(
        Behavior {
            commit: true,
            verdicts: vec![Some(
                r#"{"result":"revise","summary":"needs another pass"}"#.into(),
            )],
            ..Default::default()
        },
        blackboard::blackboard_dir(&fx.run_dir),
        vec!["plan".into(), "build".into()],
    );
    let ctx = ctx_with(&fx, driver.clone());

    run_kernel(&ctx, "run-v2").await;

    let (status, error) = run_status(&fx.db, "run-v2");
    assert_eq!(status, "failed");
    let error = error.unwrap();
    assert!(error.contains("revise"), "{error}");
    assert!(error.contains("needs another pass"), "{error}");

    // The first step is `blocked` and the second never spawned — no silent
    // advance past an unmet gate.
    let execs = exec_rows(&fx.db, "run-v2");
    assert_eq!(execs.len(), 1);
    assert_eq!(execs[0].1, "blocked");
    assert_eq!(driver.adopted().len(), 1);
    // Pausing/failing stops processes.
    assert_eq!(driver.stops(), 1);
}

#[tokio::test]
async fn a_missing_verdict_fails_the_run() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = spec_of(vec![Block::Step(step("plan", Gate::Verdict))], None);
    let fx = scaffold(tmp.path(), "run-v3", "wf/demo-3", &spec);
    let driver = Stub::new(
        Behavior {
            commit: true,
            ..Default::default()
        },
        blackboard::blackboard_dir(&fx.run_dir),
        vec!["plan".into()],
    );
    let ctx = ctx_with(&fx, driver.clone());

    run_kernel(&ctx, "run-v3").await;

    let (status, error) = run_status(&fx.db, "run-v3");
    assert_eq!(status, "failed");
    assert!(error.unwrap().contains("no verdict.json"));
}

// ─────────────────────────── errors, timeout, cancel ─────────────────────────

#[tokio::test]
async fn a_spawn_failure_fails_the_run() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = spec_of(vec![Block::Step(step("plan", Gate::Commit))], None);
    let fx = scaffold(tmp.path(), "run-e", "wf/demo-e", &spec);
    let driver = Stub::new(
        Behavior {
            fail_spawn: true,
            ..Default::default()
        },
        blackboard::blackboard_dir(&fx.run_dir),
        vec!["plan".into()],
    );
    let ctx = ctx_with(&fx, driver.clone());

    run_kernel(&ctx, "run-e").await;

    let (status, error) = run_status(&fx.db, "run-e");
    assert_eq!(status, "failed");
    assert!(error.unwrap().contains("spawn failed"));
    let execs = exec_rows(&fx.db, "run-e");
    assert_eq!(execs[0].1, "error");
    assert!(
        event_types(&fx.db, "run-e")
            .iter()
            .any(|e| e == event_type::ATTEMPT_ERROR),
        "the cause is on the timeline"
    );
}

#[tokio::test]
async fn a_wedged_step_hits_the_wall_timeout_and_fails_the_run() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = spec_of(vec![Block::Step(step("plan", Gate::Commit))], None);
    let fx = scaffold(tmp.path(), "run-t", "wf/demo-t", &spec);
    let driver = Stub::new(
        Behavior {
            hang: true,
            ..Default::default()
        },
        blackboard::blackboard_dir(&fx.run_dir),
        vec!["plan".into()],
    );
    let ctx = ctx_with(&fx, driver.clone());

    run_kernel(&ctx, "run-t").await;

    let (status, error) = run_status(&fx.db, "run-t");
    assert_eq!(status, "failed");
    assert!(error.unwrap().contains("timed out"));
    // The wedged agent is not left running.
    assert_eq!(driver.stops(), 1);
    assert_eq!(exec_rows(&fx.db, "run-t")[0].1, "error");
}

#[tokio::test]
async fn a_cancel_mid_step_stops_the_agent_and_marks_the_run_canceled() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = spec_of(
        vec![
            Block::Step(step("plan", Gate::Commit)),
            Block::Step(step("build", Gate::Commit)),
        ],
        None,
    );
    let fx = scaffold(tmp.path(), "run-c", "wf/demo-c", &spec);
    let driver = Stub::new(
        Behavior {
            cancel_on_prompt: Some(fx.cancel.clone()),
            ..Default::default()
        },
        blackboard::blackboard_dir(&fx.run_dir),
        vec!["plan".into(), "build".into()],
    );
    let ctx = ctx_with(&fx, driver.clone());

    run_kernel(&ctx, "run-c").await;

    assert_eq!(run_status(&fx.db, "run-c").0, "canceled");
    assert_eq!(driver.stops(), 1, "the live agent is stopped");
    assert_eq!(driver.archives(), 1, "its chat stays replayable");
    let execs = exec_rows(&fx.db, "run-c");
    assert_eq!(execs.len(), 1, "the second step never spawned");
    assert_eq!(execs[0].1, "abandoned");
    assert!(event_types(&fx.db, "run-c")
        .iter()
        .any(|e| e == event_type::RUN_CANCELED));
}

#[tokio::test]
async fn a_pre_canceled_run_spawns_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = spec_of(vec![Block::Step(step("plan", Gate::Commit))], None);
    let fx = scaffold(tmp.path(), "run-c0", "wf/demo-c0", &spec);
    fx.cancel.store(true, Ordering::SeqCst);
    let driver = Stub::new(
        Behavior::default(),
        blackboard::blackboard_dir(&fx.run_dir),
        vec!["plan".into()],
    );
    let ctx = ctx_with(&fx, driver.clone());

    run_kernel(&ctx, "run-c0").await;

    assert_eq!(run_status(&fx.db, "run-c0").0, "canceled");
    assert!(driver.adopted().is_empty());
}

// ────────────────────────────── resume & routing ─────────────────────────────

#[tokio::test]
async fn a_non_terminal_kernel_run_is_failed_rather_than_resumed() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = spec_of(vec![Block::Step(step("plan", Gate::Commit))], None);
    let fx = scaffold(tmp.path(), "run-r", "wf/demo-r", &spec);
    // The state a crash leaves behind: a `running` run with a live-looking exec.
    {
        let conn = fx.db.lock();
        conn.execute(
            "UPDATE wf_run SET status = 'running' WHERE id = 'run-r'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wf_step_exec (id, run_id, step_id, attempt, iteration, status,
                    gate_mode, agent_id)
             VALUES ('exec-old','run-r','plan',1,0,'running','commit','stub-old')",
            [],
        )
        .unwrap();
    }
    let driver = Stub::new(
        Behavior::default(),
        blackboard::blackboard_dir(&fx.run_dir),
        vec!["plan".into()],
    );
    let ctx = ctx_with(&fx, driver.clone());

    run_kernel(&ctx, "run-r").await;

    let (status, error) = run_status(&fx.db, "run-r");
    assert_eq!(status, "failed");
    assert!(error.unwrap().contains("do not resume yet"));
    // Nothing was re-run, and the stale exec no longer claims to be live.
    assert!(driver.adopted().is_empty());
    assert_eq!(exec_rows(&fx.db, "run-r")[0].1, "abandoned");
    assert_eq!(driver.stops(), 1);
}

#[tokio::test]
async fn a_terminal_run_is_not_redriven() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = spec_of(vec![Block::Step(step("plan", Gate::Commit))], None);
    let fx = scaffold(tmp.path(), "run-d", "wf/demo-d", &spec);
    fx.db
        .lock()
        .execute("UPDATE wf_run SET status = 'done' WHERE id = 'run-d'", [])
        .unwrap();
    let driver = Stub::new(
        Behavior::default(),
        blackboard::blackboard_dir(&fx.run_dir),
        vec!["plan".into()],
    );
    let ctx = ctx_with(&fx, driver.clone());

    run_kernel(&ctx, "run-d").await;

    assert_eq!(run_status(&fx.db, "run-d").0, "done");
    assert!(driver.adopted().is_empty());
    assert!(event_types(&fx.db, "run-d").is_empty());
}

#[test]
fn eligibility_admits_plain_steps_with_commit_or_verdict_gates_only() {
    let commit_and_verdict = spec_of(
        vec![
            Block::Step(step("a", Gate::Commit)),
            Block::Step(step("b", Gate::Verdict)),
        ],
        None,
    );
    assert!(kernel_eligible(&commit_and_verdict));

    // A gate the kernel can't decide by itself.
    for gate in [
        Gate::Artifact {
            path: "PLAN.md".into(),
        },
        Gate::Tests,
        Gate::Approval {
            require: vec![],
            artifact: None,
        },
    ] {
        let spec = spec_of(vec![Block::Step(step("a", gate))], None);
        assert!(!kernel_eligible(&spec));
    }

    // A block kind the kernel doesn't execute.
    let parallel = spec_of(
        vec![Block::Parallel(Parallel {
            join: Join::All,
            integrate: Integrate::None,
            max_concurrent: None,
            steps: vec![step("a", Gate::Commit)],
        })],
        None,
    );
    assert!(!kernel_eligible(&parallel));

    // Nothing to run is not the kernel's business either.
    assert!(!kernel_eligible(&spec_of(vec![], None)));
}

#[test]
fn routing_reads_the_runs_frozen_spec() {
    let tmp = tempfile::tempdir().unwrap();
    let eligible = spec_of(vec![Block::Step(step("a", Gate::Commit))], None);
    let ineligible = spec_of(vec![Block::Step(step("a", Gate::Tests))], None);
    let db = crate::database::init(tmp.path()).unwrap();
    let insert = |conn: &rusqlite::Connection, id: &str, spec: &Spec| {
        conn.execute(
            "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                    base_sha,status,budgets_json,spent_json,created_at,updated_at)
             VALUES (?1,'demo',?2,'t','p','/tmp/r','/tmp/d','wf/x','sha','pending','{}','{}',0,0)",
            rusqlite::params![id, serde_json::to_string(spec).unwrap()],
        )
        .unwrap();
    };
    {
        let conn = db.lock();
        insert(&conn, "kernel", &eligible);
        insert(&conn, "legacy", &ineligible);
        conn.execute(
            "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                    base_sha,status,budgets_json,spent_json,created_at,updated_at)
             VALUES ('broken','demo','not json','t','p','/tmp/r','/tmp/d','wf/x','sha',
                     'pending','{}','{}',0,0)",
            [],
        )
        .unwrap();
    }

    assert!(routes_to_kernel(&db, "kernel"));
    assert!(!routes_to_kernel(&db, "legacy"));
    // An unreadable spec and a missing run both belong to the old engine, which
    // owns reporting that failure.
    assert!(!routes_to_kernel(&db, "broken"));
    assert!(!routes_to_kernel(&db, "nope"));
}
