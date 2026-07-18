use super::*;
use crate::supervisor::StatusEvent;
use crate::workspace::AgentStatus;
use std::collections::BTreeMap;
use std::process::Command as Sh;
use tokio::sync::broadcast;

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

/// A real-git stub driver: on spawn it provisions a real `--shared` clone
/// forking from the run repo (exercising `provision_forking_run_repo`), and
/// on each prompt the "agent" makes a commit in that workspace. This drives
/// the full scheduler + gitops + journal path with a mocked agent lifecycle
/// — the §16 stub-agent integration test, deterministic and process-free.
struct StubDriver {
    root: PathBuf,
    /// Whether the "agent" commits during its turn. `false` models an agent
    /// that does nothing, so a `commit` gate stays unmet (blocked-gate test).
    commit: bool,
    tx: broadcast::Sender<StatusEvent>,
    state: parking_lot::Mutex<StubState>,
}
#[derive(Default)]
struct StubState {
    statuses: HashMap<String, AgentStatus>,
    worktrees: HashMap<String, PathBuf>,
    count: usize,
    /// How many times `stop` was called — lets a test prove that entering
    /// `paused` stops the live step agent (§6.5).
    stops: usize,
}
impl StubDriver {
    fn new(root: PathBuf, commit: bool) -> Arc<Self> {
        Arc::new(Self {
            root,
            commit,
            tx: broadcast::channel(256).0,
            state: parking_lot::Mutex::new(StubState::default()),
        })
    }
    fn set(&self, id: &str, s: AgentStatus) {
        self.state.lock().statuses.insert(id.to_string(), s.clone());
        let _ = self.tx.send(StatusEvent {
            agent_id: id.to_string(),
            status: s,
        });
    }
    fn stop_count(&self) -> usize {
        self.state.lock().stops
    }
}
impl AgentDriver for StubDriver {
    fn spawn(
        &self,
        req: SpawnReq,
    ) -> super::super::driver::BoxFuture<'_, Result<super::super::driver::SpawnedAgent>> {
        Box::pin(async move {
            let id = {
                let mut st = self.state.lock();
                st.count += 1;
                format!("stub-{}", st.count)
            };
            let dest = self.root.join(&id);
            let base_ref = req.fork_base.clone().unwrap();
            let spec = crate::sandbox::provision::CheckoutSpec {
                source_repo: &req.repo_path,
                base_ref: &base_ref,
                dest: &dest,
            };
            crate::sandbox::provision::provision_forking_run_repo(
                &spec,
                req.run_repo.as_ref().unwrap(),
            )
            .await?;
            sh(&dest, &["config", "user.email", "t@t.t"]);
            sh(&dest, &["config", "user.name", "t"]);
            self.state.lock().worktrees.insert(id.clone(), dest.clone());
            self.set(&id, AgentStatus::Idle);
            Ok(super::super::driver::SpawnedAgent {
                agent_id: id,
                worktree: dest,
            })
        })
    }
    fn status(&self, id: &str) -> Option<AgentStatus> {
        self.state.lock().statuses.get(id).cloned()
    }
    fn subscribe(&self) -> broadcast::Receiver<StatusEvent> {
        self.tx.subscribe()
    }
    fn send_message<'a>(
        &'a self,
        id: &'a str,
        _text: String,
    ) -> super::super::driver::BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let wt = self.state.lock().worktrees.get(id).cloned().unwrap();
            self.set(id, AgentStatus::Running);
            if self.commit {
                std::fs::write(wt.join(format!("{id}.txt")), "work").unwrap();
                sh(&wt, &["add", "-A"]);
                sh(&wt, &["commit", "-qm", "agent work"]);
            }
            self.set(id, AgentStatus::Idle);
            Ok(())
        })
    }
    fn stop<'a>(&'a self, _id: &'a str) -> super::super::driver::BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.state.lock().stops += 1;
            Ok(())
        })
    }
    fn archive<'a>(&'a self, _id: &'a str) -> super::super::driver::BoxFuture<'a, Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn last_activity(&self, _id: &str) -> Option<i64> {
        None
    }
    fn turn_usage(&self, _id: &str) -> Option<super::super::driver::TurnUsage> {
        None
    }
}

fn step(id: &str) -> Step {
    Step {
        id: id.to_string(),
        agent: "coder".to_string(),
        goal: format!("do {id}"),
        gate: Gate::Commit,
        budgets: None,
        comms: vec![],
    }
}

/// A library DB with one custom agent carrying a model/effort/instructions,
/// for the `build_spawn_req` precedence checks below.
fn spawn_req_conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE skills (id TEXT PRIMARY KEY, name TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '', body TEXT NOT NULL DEFAULT '');
             CREATE TABLE mcp_servers (id TEXT PRIMARY KEY, name TEXT NOT NULL,
                transport TEXT NOT NULL DEFAULT 'stdio', command TEXT NOT NULL DEFAULT '',
                env TEXT NOT NULL DEFAULT '', url TEXT NOT NULL DEFAULT '',
                headers TEXT NOT NULL DEFAULT '');
             CREATE TABLE custom_agents (id TEXT PRIMARY KEY,
                skill_ids TEXT NOT NULL DEFAULT '[]', mcp_server_ids TEXT NOT NULL DEFAULT '[]',
                effort TEXT, model TEXT, instructions TEXT NOT NULL DEFAULT '');
             INSERT INTO custom_agents (id, effort, model, instructions)
                VALUES ('ca1', 'high', 'opus', 'Be thorough.');",
    )
    .unwrap();
    conn
}

fn agent_spec(custom_agent: Option<&str>) -> super::super::spec::AgentSpec {
    super::super::spec::AgentSpec {
        base: "claude".into(),
        model: None,
        effort: None,
        instructions: None,
        skills: vec![],
        mcp_servers: vec![],
        custom_agent: custom_agent.map(str::to_string),
    }
}

#[test]
fn build_spawn_req_inherits_custom_agent_model_effort_instructions() {
    // The coherency bar: a live custom-agent-backed alias must spawn with the
    // same model/effort/instructions the export+import path inlines onto the
    // AgentSpec — sourced here from the custom_agents row as a fallback.
    let conn = spawn_req_conn();
    let dummy = Path::new("/tmp/repo");
    let req = build_spawn_req(
        &conn,
        None,
        &agent_spec(Some("ca1")),
        "base",
        dummy,
        dummy,
        "r",
        None,
    );
    assert_eq!(req.model.as_deref(), Some("opus"));
    assert_eq!(req.effort.as_deref(), Some("high"));
    assert_eq!(req.instructions.as_deref(), Some("Be thorough."));
}

#[test]
fn build_spawn_req_explicit_alias_values_win_over_custom_agent() {
    let conn = spawn_req_conn();
    let mut spec = agent_spec(Some("ca1"));
    spec.model = Some("sonnet".into());
    spec.effort = Some("low".into());
    spec.instructions = Some("Override brief.".into());
    let dummy = Path::new("/tmp/repo");
    let req = build_spawn_req(&conn, None, &spec, "base", dummy, dummy, "r", None);
    assert_eq!(req.model.as_deref(), Some("sonnet"));
    assert_eq!(req.effort.as_deref(), Some("low"));
    assert_eq!(req.instructions.as_deref(), Some("Override brief."));
}

#[test]
fn build_spawn_req_blank_explicit_values_fall_back_to_custom_agent() {
    // A blank explicit override — empty *or* whitespace-only (both reachable
    // from hand-authored/imported YAML) — must not block the custom agent's
    // value or reach spawn as an empty argument; it's treated as unset.
    let conn = spawn_req_conn();
    let dummy = Path::new("/tmp/repo");
    for blank in ["", "   ", "\t\n"] {
        let mut spec = agent_spec(Some("ca1"));
        spec.model = Some(blank.to_string());
        spec.effort = Some(blank.to_string());
        spec.instructions = Some(blank.to_string());
        let req = build_spawn_req(&conn, None, &spec, "base", dummy, dummy, "r", None);
        assert_eq!(req.model.as_deref(), Some("opus"), "blank {blank:?}");
        assert_eq!(req.effort.as_deref(), Some("high"), "blank {blank:?}");
        assert_eq!(
            req.instructions.as_deref(),
            Some("Be thorough."),
            "blank {blank:?}"
        );
    }
}

#[test]
fn build_spawn_req_dangling_custom_agent_inherits_nothing() {
    // A deleted custom agent leaves the alias's own (here unset) values —
    // unchanged behavior, never a resolution error.
    let conn = spawn_req_conn();
    let dummy = Path::new("/tmp/repo");
    let req = build_spawn_req(
        &conn,
        None,
        &agent_spec(Some("gone")),
        "base",
        dummy,
        dummy,
        "r",
        None,
    );
    assert!(req.model.is_none());
    assert!(req.effort.is_none());
    assert!(req.instructions.is_none());
}

#[tokio::test]
async fn linear_two_step_run_reaches_done_and_pushes() {
    let tmp = tempfile::tempdir().unwrap();

    // Bare "remote" + a source repo that points origin at it.
    let bare = tmp.path().join("origin.git");
    std::fs::create_dir_all(&bare).unwrap();
    sh(&bare, &["init", "-q", "--bare", "-b", "main"]);
    let source = tmp.path().join("source");
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
    let base_sha = {
        let out = Sh::new("git")
            .current_dir(&source)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };

    let run_dir = tmp.path().join("rundir");
    std::fs::create_dir_all(blackboard::blackboard_dir(&run_dir)).unwrap();

    // Spec: two commit-gated steps + a finalize push (no PR — no GitHub).
    let mut agents = BTreeMap::new();
    agents.insert(
        "coder".to_string(),
        super::super::spec::AgentSpec {
            base: "codex".to_string(),
            model: None,
            effort: None,
            instructions: None,
            skills: vec![],
            mcp_servers: vec![],
            custom_agent: None,
        },
    );
    let spec = Spec {
        version: 1,
        name: "demo".to_string(),
        description: None,
        budgets: None,
        agents,
        workflow: vec![Block::Step(step("plan")), Block::Step(step("build"))],
        finalize: Some(super::super::spec::Finalize {
            push: true,
            open_pr: false,
            pr_base: Some("main".to_string()),
        }),
    };
    let spec_json = serde_json::to_string(&spec).unwrap();

    let db = crate::database::init(tmp.path()).unwrap();
    let run_id = "run-demo";
    let branch = "wf/demo-abcdef12";
    {
        let conn = db.lock();
        conn.execute(
            "INSERT INTO wf_run (id, name, spec_json, task, project_id, repo_path, run_dir,
                    branch, base_sha, status, budgets_json, spent_json, created_at, updated_at)
                 VALUES (?1,'demo',?2,'the task','p',?3,?4,?5,?6,'pending','{}','{}',0,0)",
            rusqlite::params![
                run_id,
                spec_json,
                source.to_string_lossy(),
                run_dir.to_string_lossy(),
                branch,
                base_sha,
            ],
        )
        .unwrap();
    }

    let driver = StubDriver::new(tmp.path().join("workspaces"), true);
    let ctx = RunCtx {
        db: db.clone(),
        driver,
        app: None,
        cancel: Arc::new(AtomicBool::new(false)),
        pending_ask: Arc::new(AtomicBool::new(false)),
        deadlines: Deadlines::default(),
        runs: None,
    };
    drive_run(&ctx, run_id).await;

    // Run reached done.
    let status: String = db
        .lock()
        .query_row("SELECT status FROM wf_run WHERE id=?1", [run_id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(status, "done", "run should be done");

    // Two step attempts, both done.
    let done_count: i64 = db
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM wf_step_exec WHERE run_id=?1 AND status='done'",
            [run_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(done_count, 2, "both steps done");

    // The branch was pushed to the bare remote, two commits above base
    // (step 2 building on step 1).
    let pushed = Sh::new("git")
        .current_dir(&bare)
        .args(["rev-parse", &format!("refs/heads/{branch}")])
        .output()
        .unwrap();
    assert!(pushed.status.success(), "branch pushed");
    let count = Sh::new("git")
        .current_dir(&bare)
        .args(["rev-list", "--count", &format!("refs/heads/{branch}")])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&count.stdout).trim(),
        "3",
        "base + 2 step commits"
    );

    // A finalize_pushed event was journaled.
    let fin: i64 = db
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM wf_event WHERE run_id=?1 AND type=?2",
            rusqlite::params![run_id, event_type::FINALIZE_PUSHED],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fin, 1, "finalize_pushed journaled");
}

/// A single commit-gated step, no finalize — for the cancel / blocked tests.
fn scaffold_one_step(tmp: &Path, run_id: &str, branch: &str, gate: Gate) -> (Db, PathBuf) {
    let source = tmp.join("source");
    std::fs::create_dir_all(&source).unwrap();
    sh(&source, &["init", "-q", "-b", "main"]);
    sh(&source, &["config", "user.email", "t@t.t"]);
    sh(&source, &["config", "user.name", "t"]);
    std::fs::write(source.join("README"), "base").unwrap();
    sh(&source, &["add", "-A"]);
    sh(&source, &["commit", "-qm", "base"]);
    let base_sha = {
        let o = Sh::new("git")
            .current_dir(&source)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    let run_dir = tmp.join("rundir");
    std::fs::create_dir_all(blackboard::blackboard_dir(&run_dir)).unwrap();
    let mut agents = BTreeMap::new();
    agents.insert(
        "coder".to_string(),
        super::super::spec::AgentSpec {
            base: "codex".to_string(),
            model: None,
            effort: None,
            instructions: None,
            skills: vec![],
            mcp_servers: vec![],
            custom_agent: None,
        },
    );
    let spec = Spec {
        version: 1,
        name: "demo".to_string(),
        description: None,
        budgets: None,
        agents,
        workflow: vec![Block::Step(Step {
            id: "only".to_string(),
            agent: "coder".to_string(),
            goal: "do only".to_string(),
            gate,
            budgets: None,
            comms: vec![],
        })],
        finalize: None,
    };
    let spec_json = serde_json::to_string(&spec).unwrap();
    let db = crate::database::init(tmp).unwrap();
    db.lock()
        .execute(
            "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                    base_sha,status,budgets_json,spent_json,created_at,updated_at)
                 VALUES (?1,'demo',?2,'t','p',?3,?4,?5,?6,'pending','{}','{}',0,0)",
            rusqlite::params![
                run_id,
                spec_json,
                source.to_string_lossy(),
                run_dir.to_string_lossy(),
                branch,
                base_sha,
            ],
        )
        .unwrap();
    (db, tmp.join("ws"))
}

#[tokio::test]
async fn cancel_marks_run_canceled_and_runs_no_step() {
    let tmp = tempfile::tempdir().unwrap();
    let (db, ws) = scaffold_one_step(tmp.path(), "run-cancel", "wf/c-1", Gate::Commit);
    let ctx = RunCtx {
        db: db.clone(),
        driver: StubDriver::new(ws, true),
        app: None,
        cancel: Arc::new(AtomicBool::new(true)), // pre-canceled
        pending_ask: Arc::new(AtomicBool::new(false)),
        deadlines: Deadlines::default(),
        runs: None,
    };
    drive_run(&ctx, "run-cancel").await;
    let status: String = db
        .lock()
        .query_row("SELECT status FROM wf_run WHERE id='run-cancel'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(status, "canceled");
}

#[tokio::test]
async fn terminal_run_is_not_redriven() {
    let tmp = tempfile::tempdir().unwrap();
    let (db, ws) = scaffold_one_step(tmp.path(), "run-term", "wf/t-1", Gate::Commit);
    db.lock()
        .execute("UPDATE wf_run SET status='failed' WHERE id='run-term'", [])
        .unwrap();
    let ctx = RunCtx {
        db: db.clone(),
        driver: StubDriver::new(ws, true),
        app: None,
        cancel: Arc::new(AtomicBool::new(false)),
        pending_ask: Arc::new(AtomicBool::new(false)),
        deadlines: Deadlines::default(),
        runs: None,
    };
    drive_run(&ctx, "run-term").await;
    let (status, execs): (String, i64) = db
        .lock()
        .query_row(
            "SELECT status, (SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-term')
                 FROM wf_run WHERE id='run-term'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "failed", "terminal status must be preserved");
    assert_eq!(execs, 0, "no step may run on a terminal run");
}

#[tokio::test]
async fn unmet_commit_gate_pauses_blocked() {
    let tmp = tempfile::tempdir().unwrap();
    let (db, ws) = scaffold_one_step(tmp.path(), "run-blocked", "wf/b-1", Gate::Commit);
    // commit=false → the agent makes no commit → the commit gate stays unmet
    // through the attempt's one re-prompt, so the run pauses `blocked_gate`.
    let ctx = RunCtx {
        db: db.clone(),
        driver: StubDriver::new(ws, false),
        app: None,
        cancel: Arc::new(AtomicBool::new(false)),
        pending_ask: Arc::new(AtomicBool::new(false)),
        deadlines: Deadlines::default(),
        runs: None,
    };
    drive_run(&ctx, "run-blocked").await;
    let (status, reason): (String, Option<String>) = db
        .lock()
        .query_row(
            "SELECT status, paused_reason FROM wf_run WHERE id='run-blocked'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "paused");
    assert_eq!(reason.as_deref(), Some("blocked_gate"));
}

/// Drive a single approval-gated step to its pause and return the db + run id.
/// commit=true so the step ferries real work; the gate then awaits a human.
async fn drive_to_approval(tmp: &Path, run_id: &str, branch: &str) -> Db {
    let (db, ws) = scaffold_one_step(tmp, run_id, branch, Gate::Approval { require: vec![] });
    let ctx = RunCtx {
        db: db.clone(),
        driver: StubDriver::new(ws, true),
        app: None,
        cancel: Arc::new(AtomicBool::new(false)),
        pending_ask: Arc::new(AtomicBool::new(false)),
        deadlines: Deadlines::default(),
        runs: None,
    };
    drive_run(&ctx, run_id).await;
    db
}

#[tokio::test]
async fn approval_pause_journals_review_evidence() {
    // §9: an approval pause must carry review evidence (verification + diff +
    // budget + verdict) on its own `gate_evidence` event, keyed to the awaiting
    // exec, and leave the step `awaiting_approval`.
    let tmp = tempfile::tempdir().unwrap();
    let db = drive_to_approval(tmp.path(), "run-appr", "wf/a-1").await;
    assert_eq!(run_status_str(&db, "run-appr"), "paused");
    let (reason, awaiting): (Option<String>, i64) = db
        .lock()
        .query_row(
            "SELECT paused_reason,
                    (SELECT COUNT(*) FROM wf_step_exec
                     WHERE run_id='run-appr' AND status='awaiting_approval')
                 FROM wf_run WHERE id='run-appr'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(reason.as_deref(), Some("approval"));
    assert_eq!(awaiting, 1, "the step waits for approval");
    assert_eq!(
        count_events(&db, "run-appr", event_type::GATE_EVIDENCE),
        1,
        "one gate_evidence event journaled at the pause"
    );
    // The evidence payload carries the diff/budget/verification keys.
    let payload: String = db
        .lock()
        .query_row(
            "SELECT payload_json FROM wf_event
                 WHERE run_id='run-appr' AND type=?1 LIMIT 1",
            [event_type::GATE_EVIDENCE],
            |r| r.get(0),
        )
        .unwrap();
    let v: Value = serde_json::from_str(&payload).unwrap();
    assert!(v.get("diff").is_some(), "evidence has a diff: {v}");
    assert!(v.get("budget").is_some(), "evidence has a budget: {v}");
    assert!(v.get("verification").is_some(), "evidence has verification");
}

#[tokio::test]
async fn reject_with_budget_re_prompts_the_step() {
    // §9: rejecting with budget left journals the decision, abandons the
    // rejected attempt, and queues the note as a delivery for the fresh attempt.
    let tmp = tempfile::tempdir().unwrap();
    let db = drive_to_approval(tmp.path(), "run-rej", "wf/r-1").await;

    let re_drive = {
        let conn = db.lock();
        reject_apply(&conn, None, "run-rej", "  please add a regression test  ").unwrap()
    };
    assert!(re_drive, "budget available → re-drive");

    // Decision journaled with the trimmed note.
    let note: String = db
        .lock()
        .query_row(
            "SELECT json_extract(payload_json,'$.note') FROM wf_event
                 WHERE run_id='run-rej' AND type='decision' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(note, "please add a regression test");
    // The rejected attempt is abandoned (no awaiting_approval lingers).
    let awaiting: i64 = db
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM wf_step_exec
                 WHERE run_id='run-rej' AND status='awaiting_approval'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(awaiting, 0, "rejected attempt abandoned");
    // A notify delivery carrying the note is queued for the step's next attempt.
    let queued: i64 = db
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM wf_message
                 WHERE run_id='run-rej' AND kind='notify' AND status='queued'
                   AND body_json LIKE '%regression test%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(queued, 1, "the note is queued as a delivery");
}

#[tokio::test]
async fn reject_without_budget_pauses_blocked_gate_with_the_note() {
    // §9 / §6.5: with the run budget spent there is no attempt to give — the
    // reject pauses `blocked_gate` carrying the note as detail, no re-drive.
    let tmp = tempfile::tempdir().unwrap();
    let db = drive_to_approval(tmp.path(), "run-rej2", "wf/r-2").await;
    // Spend the whole default turn budget (100) so a fresh attempt can't run.
    db.lock()
        .execute(
            "UPDATE wf_run SET spent_json='{\"turns\":100}' WHERE id='run-rej2'",
            [],
        )
        .unwrap();

    let re_drive = {
        let conn = db.lock();
        reject_apply(&conn, None, "run-rej2", "out of scope").unwrap()
    };
    assert!(!re_drive, "budget spent → no re-drive");

    let (status, reason, detail): (String, Option<String>, Option<String>) = db
        .lock()
        .query_row(
            "SELECT r.status, r.paused_reason,
                    (SELECT json_extract(payload_json,'$.detail') FROM wf_event
                     WHERE run_id='run-rej2' AND type='run_paused'
                       AND json_extract(payload_json,'$.reason')='blocked_gate'
                     ORDER BY seq DESC LIMIT 1)
                 FROM wf_run r WHERE r.id='run-rej2'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, "paused");
    assert_eq!(reason.as_deref(), Some("blocked_gate"));
    assert_eq!(detail.as_deref(), Some("out of scope"));
}

#[tokio::test]
async fn pause_stops_the_live_step_agent() {
    // §6.5: entering `paused` stops live step agents — a pause can last days
    // and idle CLI processes must not accumulate. commit=false keeps the
    // commit gate unmet, so the attempt exhausts its re-prompt and the run
    // pauses `blocked_gate`; the engine must have called `driver.stop`.
    let tmp = tempfile::tempdir().unwrap();
    let (db, ws) = scaffold_one_step(tmp.path(), "run-pausestop", "wf/ps-1", Gate::Commit);
    let driver = StubDriver::new(ws, false);
    let ctx = RunCtx {
        db: db.clone(),
        driver: driver.clone(),
        app: None,
        cancel: Arc::new(AtomicBool::new(false)),
        pending_ask: Arc::new(AtomicBool::new(false)),
        deadlines: Deadlines::default(),
        runs: None,
    };
    drive_run(&ctx, "run-pausestop").await;
    assert_eq!(run_status_str(&db, "run-pausestop"), "paused");
    assert!(
        driver.stop_count() >= 1,
        "pausing must stop the live step agent"
    );
}

/// A driver whose `spawn` always errors — drives a run onto the terminal
/// failure path (attempt error → retry → `fail_run`).
struct SpawnFailDriver {
    tx: broadcast::Sender<StatusEvent>,
}
impl SpawnFailDriver {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            tx: broadcast::channel(8).0,
        })
    }
}
impl AgentDriver for SpawnFailDriver {
    fn spawn(
        &self,
        _req: SpawnReq,
    ) -> super::super::driver::BoxFuture<'_, Result<super::super::driver::SpawnedAgent>> {
        Box::pin(async { Err(Error::Other("boom".into())) })
    }
    fn status(&self, _id: &str) -> Option<AgentStatus> {
        None
    }
    fn subscribe(&self) -> broadcast::Receiver<StatusEvent> {
        self.tx.subscribe()
    }
    fn send_message<'a>(
        &'a self,
        _id: &'a str,
        _text: String,
    ) -> super::super::driver::BoxFuture<'a, Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn stop<'a>(&'a self, _id: &'a str) -> super::super::driver::BoxFuture<'a, Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn archive<'a>(&'a self, _id: &'a str) -> super::super::driver::BoxFuture<'a, Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn last_activity(&self, _id: &str) -> Option<i64> {
        None
    }
    fn turn_usage(&self, _id: &str) -> Option<super::super::driver::TurnUsage> {
        None
    }
}

#[tokio::test]
async fn failed_run_emits_a_run_failed_event() {
    // §6.1/§7.1: a run that fails must leave an append-only `run_failed`
    // journal event, not only a materialized-row status — the observability
    // goal requires every terminal outcome to be a timeline event.
    let tmp = tempfile::tempdir().unwrap();
    let (db, _ws) = scaffold_one_step(tmp.path(), "run-fail", "wf/f-1", Gate::Commit);
    let ctx = RunCtx {
        db: db.clone(),
        driver: SpawnFailDriver::new(),
        app: None,
        cancel: Arc::new(AtomicBool::new(false)),
        pending_ask: Arc::new(AtomicBool::new(false)),
        deadlines: Deadlines::default(),
        runs: None,
    };
    drive_run(&ctx, "run-fail").await;

    assert_eq!(run_status_str(&db, "run-fail"), "failed");
    // Exactly one run_failed event, carrying the human-readable cause.
    let (n, err): (i64, Option<String>) = db
        .lock()
        .query_row(
            "SELECT COUNT(*), MAX(json_extract(payload_json,'$.error'))
                 FROM wf_event WHERE run_id='run-fail' AND type=?1",
            [event_type::RUN_FAILED],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(n, 1, "one run_failed event");
    assert!(
        err.as_deref().is_some_and(|e| e.contains("spawn failed")),
        "run_failed payload carries the cause, got {err:?}"
    );
}

#[tokio::test]
async fn stale_verdict_archival_failure_does_not_gate_on_the_stale_verdict() {
    // §8.3: if a leftover verdict cannot be moved aside, the gate must not
    // read it — the loop/retry staleness bug class. Block archival (make
    // `<step-dir>/history` a file so `create_dir_all` fails) and plant a
    // `done` verdict. The attempt must error rather than pass the gate, so
    // the run ends `failed`, never `done`.
    let tmp = tempfile::tempdir().unwrap();
    let (db, ws) = scaffold_one_step(tmp.path(), "run-stale", "wf/st-1", Gate::Verdict);
    // The step dir lives under the run's blackboard; the run_dir is
    // `<tmp>/rundir` (see `scaffold_one_step`).
    let step_dir = blackboard::blackboard_dir(&tmp.path().join("rundir")).join("only");
    std::fs::create_dir_all(&step_dir).unwrap();
    std::fs::write(
        step_dir.join("verdict.json"),
        r#"{"result":"done","summary":"stale"}"#,
    )
    .unwrap();
    // A regular file where the history dir must be created → archival errors.
    std::fs::write(step_dir.join("history"), "not a dir").unwrap();

    let ctx = RunCtx {
        db: db.clone(),
        driver: StubDriver::new(ws, true),
        app: None,
        cancel: Arc::new(AtomicBool::new(false)),
        pending_ask: Arc::new(AtomicBool::new(false)),
        deadlines: Deadlines::default(),
        runs: None,
    };
    drive_run(&ctx, "run-stale").await;

    assert_eq!(
        run_status_str(&db, "run-stale"),
        "failed",
        "a blocked archival must fail the attempt, not gate on the stale verdict"
    );
    let done = count(
        &db,
        "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-stale' AND status='done'",
    );
    assert_eq!(
        done, 0,
        "the stale `done` verdict must not satisfy the gate"
    );
}

/// A stub whose "agent" raises the run's pending-ask flag on its very first
/// turn (standing in for a `wf_ask` routed to the human) and commits on every
/// later turn. It shares the run's `pending_ask` Arc and records every prompt
/// it is sent, so the test can prove the deferral, the pause, and the
/// answer-fold on resume.
struct AskStub {
    root: PathBuf,
    db: Db,
    run_id: String,
    pending_ask: Arc<AtomicBool>,
    /// Whether turn 1 also raises the in-memory flag (fast path). `false`
    /// exercises the scheduler's DB backstop: the ask is persisted but the
    /// poke is "missed", and the run must still pause `question`.
    set_flag: bool,
    /// When set, the ask isn't persisted during the turn — it only becomes
    /// visible when `settle_rpc` drains the mailbox. Proves the scheduler
    /// drains *before* the backstop check (§10.4).
    persist_in_settle: bool,
    tx: broadcast::Sender<StatusEvent>,
    state: parking_lot::Mutex<AskStubState>,
}
#[derive(Default)]
struct AskStubState {
    statuses: HashMap<String, AgentStatus>,
    worktrees: HashMap<String, PathBuf>,
    spawns: usize,
    turns: usize,
    prompts: Vec<String>,
    ask_persisted: bool,
}
impl AskStub {
    fn new(
        root: PathBuf,
        db: Db,
        run_id: &str,
        pending_ask: Arc<AtomicBool>,
        set_flag: bool,
        persist_in_settle: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            root,
            db,
            run_id: run_id.to_string(),
            pending_ask,
            set_flag,
            persist_in_settle,
            tx: broadcast::channel(256).0,
            state: parking_lot::Mutex::new(AskStubState::default()),
        })
    }
    /// Persist a queued ask against the run's live attempt (agent_id is still
    /// NULL mid-turn, so resolution is by run) — exactly as the router does.
    fn persist_ask(&self) {
        let mut st = self.state.lock();
        if st.ask_persisted {
            return;
        }
        st.ask_persisted = true;
        let conn = self.db.lock();
        let exec: String = conn
            .query_row(
                "SELECT id FROM wf_step_exec WHERE run_id = ?1
                     AND status IN ('spawning','running','gating')
                     ORDER BY rowid DESC LIMIT 1",
                [&self.run_id],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO wf_message (id, run_id, from_step_exec_id, to_step_exec_id,
                    kind, body_json, status, created_at)
                 VALUES ('ask-msg-1', ?1, ?2, NULL, 'ask', '{\"question\":\"which db?\"}',
                    'queued', 0)",
            rusqlite::params![self.run_id, exec],
        )
        .unwrap();
    }
    fn set(&self, id: &str, s: AgentStatus) {
        self.state.lock().statuses.insert(id.to_string(), s.clone());
        let _ = self.tx.send(StatusEvent {
            agent_id: id.to_string(),
            status: s,
        });
    }
}
impl AgentDriver for AskStub {
    fn spawn(
        &self,
        req: SpawnReq,
    ) -> super::super::driver::BoxFuture<'_, Result<super::super::driver::SpawnedAgent>> {
        Box::pin(async move {
            let id = {
                let mut st = self.state.lock();
                st.spawns += 1;
                format!("ask-{}", st.spawns)
            };
            let dest = self.root.join(&id);
            let base_ref = req.fork_base.clone().unwrap();
            let spec = crate::sandbox::provision::CheckoutSpec {
                source_repo: &req.repo_path,
                base_ref: &base_ref,
                dest: &dest,
            };
            crate::sandbox::provision::provision_forking_run_repo(
                &spec,
                req.run_repo.as_ref().unwrap(),
            )
            .await?;
            sh(&dest, &["config", "user.email", "t@t.t"]);
            sh(&dest, &["config", "user.name", "t"]);
            self.state.lock().worktrees.insert(id.clone(), dest.clone());
            self.set(&id, AgentStatus::Idle);
            Ok(super::super::driver::SpawnedAgent {
                agent_id: id,
                worktree: dest,
            })
        })
    }
    fn status(&self, id: &str) -> Option<AgentStatus> {
        self.state.lock().statuses.get(id).cloned()
    }
    fn subscribe(&self) -> broadcast::Receiver<StatusEvent> {
        self.tx.subscribe()
    }
    fn send_message<'a>(
        &'a self,
        id: &'a str,
        text: String,
    ) -> super::super::driver::BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let (turn, wt) = {
                let mut st = self.state.lock();
                st.turns += 1;
                st.prompts.push(text);
                (st.turns, st.worktrees.get(id).cloned().unwrap())
            };
            self.set(id, AgentStatus::Running);
            if turn == 1 {
                // First turn: ask the human (defer the gate) — no commit.
                // Unless the ask is deferred to `settle_rpc` (mailbox-drain
                // test), persist it now and raise the poke only when
                // `set_flag`; otherwise the DB backstop must catch it.
                if !self.persist_in_settle {
                    self.persist_ask();
                    if self.set_flag {
                        self.pending_ask.store(true, Ordering::SeqCst);
                    }
                }
            } else {
                // Later turns: do the work so the commit gate is met.
                std::fs::write(wt.join(format!("{id}.txt")), "work").unwrap();
                sh(&wt, &["add", "-A"]);
                sh(&wt, &["commit", "-qm", "work"]);
            }
            self.set(id, AgentStatus::Idle);
            Ok(())
        })
    }
    fn settle_rpc<'a>(&'a self, _id: &'a str) -> super::super::driver::BoxFuture<'a, ()> {
        Box::pin(async move {
            // Models the real drain: a wf_ask the agent wrote during the turn
            // is only dispatched (persisted) when the mailbox is settled.
            if self.persist_in_settle {
                self.persist_ask();
            }
        })
    }
    fn stop<'a>(&'a self, _id: &'a str) -> super::super::driver::BoxFuture<'a, Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn archive<'a>(&'a self, _id: &'a str) -> super::super::driver::BoxFuture<'a, Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn last_activity(&self, _id: &str) -> Option<i64> {
        None
    }
    fn turn_usage(&self, _id: &str) -> Option<super::super::driver::TurnUsage> {
        None
    }
}

#[tokio::test]
async fn ask_pauses_question_then_answer_resumes_to_done() {
    let tmp = tempfile::tempdir().unwrap();
    let (db, ws) = scaffold_one_step(tmp.path(), "run-ask", "wf/ask-1", Gate::Commit);
    let pending_ask = Arc::new(AtomicBool::new(false));
    let driver = AskStub::new(ws, db.clone(), "run-ask", pending_ask.clone(), true, false);
    let ctx = RunCtx {
        db: db.clone(),
        driver: driver.clone(),
        app: None,
        cancel: Arc::new(AtomicBool::new(false)),
        pending_ask: pending_ask.clone(),
        deadlines: Deadlines::default(),
        runs: None,
    };

    // ── First drive: the step asks; the run pauses `question`. ──
    drive_run(&ctx, "run-ask").await;
    let (status, reason): (String, Option<String>) = db
        .lock()
        .query_row(
            "SELECT status, paused_reason FROM wf_run WHERE id='run-ask'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "paused");
    assert_eq!(reason.as_deref(), Some("question"));

    // The asking attempt was abandoned, and its gate was never evaluated
    // (deferred, §10.4).
    let (exec_id, exec_status): (String, String) = db
        .lock()
        .query_row(
            "SELECT id, status FROM wf_step_exec WHERE run_id='run-ask' ORDER BY rowid LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(exec_status, "abandoned");
    let gates: i64 = db
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM wf_event WHERE run_id='run-ask' AND type='gate_evaluated'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        gates, 0,
        "gate must not be evaluated while an ask is pending"
    );

    // ── The human answers (queued for the asking step) and the flag clears,
    // mimicking the fresh RunHandle a real resume creates. ──
    db.lock()
        .execute(
            "INSERT INTO wf_message (id, run_id, from_step_exec_id, to_step_exec_id, kind,
                    body_json, status, created_at)
                 VALUES ('ans-1','run-ask',NULL,?1,'answer',?2,'queued',0)",
            rusqlite::params![exec_id, r#"{"text":"use Postgres"}"#],
        )
        .unwrap();
    pending_ask.store(false, Ordering::SeqCst);

    // ── Resume: a fresh attempt runs, the answer is folded into its prompt,
    // and the run completes. ──
    drive_run(&ctx, "run-ask").await;
    let status: String = db
        .lock()
        .query_row("SELECT status FROM wf_run WHERE id='run-ask'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(status, "done");

    // The answer reached the agent, coalesced into the resumed attempt's
    // single prompt.
    let prompts = driver.state.lock().prompts.clone();
    assert_eq!(prompts.len(), 2, "one ask turn + one resumed turn");
    assert!(
        prompts[1].contains("use Postgres"),
        "answer folded into resumed prompt: {}",
        prompts[1]
    );
    // The queued answer was marked delivered (not re-folded).
    let undelivered: i64 = db
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM wf_message WHERE id='ans-1' AND status='queued'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(undelivered, 0, "answer should be marked delivered");
}

#[tokio::test]
async fn queued_ask_backstop_pauses_even_when_poke_is_missed() {
    // The in-memory pending-ask poke can be lost (the RPC op races the
    // driver's wind-down). The persisted ask is authoritative: even with the
    // flag never set, the scheduler must pause `question` rather than act on
    // the gate outcome (§10.4).
    let tmp = tempfile::tempdir().unwrap();
    let (db, ws) = scaffold_one_step(tmp.path(), "run-ask2", "wf/ask2-1", Gate::Commit);
    let pending_ask = Arc::new(AtomicBool::new(false));
    // set_flag = false → the ask is persisted, but the flag is never raised.
    let driver = AskStub::new(
        ws,
        db.clone(),
        "run-ask2",
        pending_ask.clone(),
        false,
        false,
    );
    let ctx = RunCtx {
        db: db.clone(),
        driver,
        app: None,
        cancel: Arc::new(AtomicBool::new(false)),
        pending_ask,
        deadlines: Deadlines::default(),
        runs: None,
    };

    drive_run(&ctx, "run-ask2").await;

    let (status, reason): (String, Option<String>) = db
        .lock()
        .query_row(
            "SELECT status, paused_reason FROM wf_run WHERE id='run-ask2'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "paused");
    assert_eq!(
        reason.as_deref(),
        Some("question"),
        "the persisted ask must pause the run even though the poke was missed"
    );
    // No boundary commit was ferried — the gate outcome was not acted on.
    let commits: i64 = db
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM wf_event WHERE run_id='run-ask2' AND type='boundary_commit'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(commits, 0, "no ferry while an answer is outstanding");
}

#[tokio::test]
async fn mailbox_drain_surfaces_a_late_ask_before_the_check() {
    // The tightest race: the agent wrote a wf_ask during its turn, but it is
    // still undispatched when the turn ends — it only becomes persisted when
    // the scheduler drains the mailbox (settle_rpc). If the scheduler checked
    // for a pending ask *without* draining first, it would miss it and act on
    // the gate. persist_in_settle models exactly that ordering.
    let tmp = tempfile::tempdir().unwrap();
    let (db, ws) = scaffold_one_step(tmp.path(), "run-ask3", "wf/ask3-1", Gate::Commit);
    let pending_ask = Arc::new(AtomicBool::new(false));
    // No in-turn persist, no flag — the ask surfaces only via settle_rpc.
    let driver = AskStub::new(ws, db.clone(), "run-ask3", pending_ask.clone(), false, true);
    let ctx = RunCtx {
        db: db.clone(),
        driver,
        app: None,
        cancel: Arc::new(AtomicBool::new(false)),
        pending_ask,
        deadlines: Deadlines::default(),
        runs: None,
    };

    drive_run(&ctx, "run-ask3").await;

    let (status, reason): (String, Option<String>) = db
        .lock()
        .query_row(
            "SELECT status, paused_reason FROM wf_run WHERE id='run-ask3'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "paused");
    assert_eq!(
        reason.as_deref(),
        Some("question"),
        "draining the mailbox before the check must surface the late ask"
    );
}

#[tokio::test]
async fn resume_abandons_a_stale_attempt_then_retries_to_done() {
    let tmp = tempfile::tempdir().unwrap();
    let (db, ws) = scaffold_one_step(tmp.path(), "run-resume", "wf/r-1", Gate::Commit);
    // A prior driver died mid-attempt, leaving a non-terminal step_exec
    // (spec §6.4). Resume must abandon it and start a fresh attempt.
    db.lock()
        .execute(
            "INSERT INTO wf_step_exec (id, run_id, step_id, attempt, iteration, status,
                    gate_mode, agent_id)
                 VALUES ('exec-stale','run-resume','only',1,0,'running','commit','ghost')",
            [],
        )
        .unwrap();
    let ctx = RunCtx {
        db: db.clone(),
        driver: StubDriver::new(ws, true),
        app: None,
        cancel: Arc::new(AtomicBool::new(false)),
        pending_ask: Arc::new(AtomicBool::new(false)),
        deadlines: Deadlines::default(),
        runs: None,
    };
    drive_run(&ctx, "run-resume").await;

    // The stale attempt was abandoned...
    let stale: String = db
        .lock()
        .query_row(
            "SELECT status FROM wf_step_exec WHERE id='exec-stale'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stale, "abandoned");
    // ...a fresh attempt ran to done and the run completed.
    let done: i64 = db
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-resume' AND status='done'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(done, 1);
    let status: String = db
        .lock()
        .query_row("SELECT status FROM wf_run WHERE id='run-resume'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(status, "done");
}

// ── budgets (spec §11.2) ─────────────────────────────────────────────────

/// Scaffold a commit-gated linear run of `step_ids`, no finalize, with an
/// explicit `budgets_json`. Mirrors `scaffold_one_step` but parametric.
fn scaffold_steps(
    tmp: &Path,
    run_id: &str,
    branch: &str,
    step_ids: &[&str],
    budgets_json: &str,
) -> (Db, PathBuf) {
    let source = tmp.join("source");
    std::fs::create_dir_all(&source).unwrap();
    sh(&source, &["init", "-q", "-b", "main"]);
    sh(&source, &["config", "user.email", "t@t.t"]);
    sh(&source, &["config", "user.name", "t"]);
    std::fs::write(source.join("README"), "base").unwrap();
    sh(&source, &["add", "-A"]);
    sh(&source, &["commit", "-qm", "base"]);
    let base_sha = {
        let o = Sh::new("git")
            .current_dir(&source)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    let run_dir = tmp.join("rundir");
    std::fs::create_dir_all(blackboard::blackboard_dir(&run_dir)).unwrap();
    let mut agents = BTreeMap::new();
    agents.insert(
        "coder".to_string(),
        super::super::spec::AgentSpec {
            base: "codex".to_string(),
            model: None,
            effort: None,
            instructions: None,
            skills: vec![],
            mcp_servers: vec![],
            custom_agent: None,
        },
    );
    let spec = Spec {
        version: 1,
        name: "demo".to_string(),
        description: None,
        budgets: None,
        agents,
        workflow: step_ids.iter().map(|id| Block::Step(step(id))).collect(),
        finalize: None,
    };
    let spec_json = serde_json::to_string(&spec).unwrap();
    let db = crate::database::init(tmp).unwrap();
    db.lock()
        .execute(
            "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                    base_sha,status,budgets_json,spent_json,created_at,updated_at)
                 VALUES (?1,'demo',?2,'t','p',?3,?4,?5,?6,'pending',?7,'{}',0,0)",
            rusqlite::params![
                run_id,
                spec_json,
                source.to_string_lossy(),
                run_dir.to_string_lossy(),
                branch,
                base_sha,
                budgets_json,
            ],
        )
        .unwrap();
    (db, tmp.join("ws"))
}

fn eff_json(turns: i64) -> String {
    serde_json::to_string(&EffectiveBudgets {
        turns,
        ..Default::default()
    })
    .unwrap()
}

#[tokio::test]
async fn zero_turn_budget_pauses_before_any_spawn() {
    // Enforcement point: before every spawn (§11.2). A run with no turn
    // budget pauses at the block boundary, having spawned nothing.
    let tmp = tempfile::tempdir().unwrap();
    let (db, ws) = scaffold_steps(tmp.path(), "run-b0", "wf/b0", &["only"], &eff_json(0));
    let ctx = RunCtx {
        db: db.clone(),
        driver: StubDriver::new(ws, true),
        app: None,
        cancel: Arc::new(AtomicBool::new(false)),
        pending_ask: Arc::new(AtomicBool::new(false)),
        deadlines: Deadlines::default(),
        runs: None,
    };
    drive_run(&ctx, "run-b0").await;

    let (status, reason): (String, Option<String>) = db
        .lock()
        .query_row(
            "SELECT status, paused_reason FROM wf_run WHERE id='run-b0'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "paused");
    assert_eq!(reason.as_deref(), Some("budget_exceeded"));
    let execs: i64 = db
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-b0'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(execs, 0, "no attempt was spawned");
}

#[tokio::test]
async fn budget_exceeded_pauses_then_resume_with_patch_completes() {
    // A turn budget of 1 lets step 1's turn run and be counted, then trips
    // the turn-end enforcement point and pauses the two-step run. A resume
    // with a budget patch (simulating `wf_resume(budget_patch)`) lifts the
    // cap and the run drives to done from the paused position.
    let tmp = tempfile::tempdir().unwrap();
    let (db, ws) = scaffold_steps(tmp.path(), "run-b1", "wf/b1", &["s1", "s2"], &eff_json(1));
    let ctx = RunCtx {
        db: db.clone(),
        driver: StubDriver::new(ws, true),
        app: None,
        cancel: Arc::new(AtomicBool::new(false)),
        pending_ask: Arc::new(AtomicBool::new(false)),
        deadlines: Deadlines::default(),
        runs: None,
    };
    drive_run(&ctx, "run-b1").await;

    // Paused for budget, one turn spent, a budget_exceeded event journaled.
    let (status, reason): (String, Option<String>) = db
        .lock()
        .query_row(
            "SELECT status, paused_reason FROM wf_run WHERE id='run-b1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "paused");
    assert_eq!(reason.as_deref(), Some("budget_exceeded"));
    let spent: String = db
        .lock()
        .query_row("SELECT spent_json FROM wf_run WHERE id='run-b1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let ledger = Ledger::from_json(&serde_json::from_str(&spent).unwrap());
    assert_eq!(ledger.turns, 1, "one turn charged before the pause");
    let exceeded: i64 = db
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM wf_event WHERE run_id='run-b1' AND type=?1",
            [event_type::BUDGET_EXCEEDED],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(exceeded, 1, "budget_exceeded journaled");

    // Resume with +10 turns (what `wf_resume`'s patch does), then re-drive.
    {
        let conn = db.lock();
        let bj: String = conn
            .query_row(
                "SELECT budgets_json FROM wf_run WHERE id='run-b1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mut e: EffectiveBudgets = serde_json::from_str(&bj).unwrap();
        e.apply_patch(&Budgets {
            turns: Some(10),
            tokens: None,
            wall_clock_mins: None,
            turns_per_attempt: None,
            max_attempts: None,
            spawn_timeout_secs: None,
            turn_start_timeout_secs: None,
            stall_timeout_secs: None,
            nudge_timeout_secs: None,
            tests_timeout_secs: None,
        });
        conn.execute(
            "UPDATE wf_run SET budgets_json=?1 WHERE id='run-b1'",
            [serde_json::to_string(&e).unwrap()],
        )
        .unwrap();
    }
    drive_run(&ctx, "run-b1").await;

    let status: String = db
        .lock()
        .query_row("SELECT status FROM wf_run WHERE id='run-b1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(status, "done", "resume-with-patch drove the run to done");
    let done: i64 = db
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-b1' AND status='done'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(done, 2, "both steps completed after the patch");
}

#[test]
fn check_resumable_gates_resume_and_retry() {
    // The guard runs before `resume` applies any budget patch, so a rejected
    // resume leaves state untouched. Terminal and approval-paused runs are
    // rejected; resumable pauses pass.
    let tmp = tempfile::tempdir().unwrap();
    let db = crate::database::init(tmp.path()).unwrap();
    let insert = |id: &str, status: &str, reason: Option<&str>| {
        db.lock()
            .execute(
                "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,
                        branch,base_sha,status,paused_reason,budgets_json,spent_json,
                        created_at,updated_at)
                     VALUES (?1,'n','{}','t','p','/r','/d','wf/x','sha',?2,?3,'{}','{}',0,0)",
                rusqlite::params![id, status, reason],
            )
            .unwrap();
    };
    insert("r-done", "done", None);
    insert("r-appr", "paused", Some("approval"));
    insert("r-ques", "paused", Some("question"));
    insert("r-budg", "paused", Some("budget_exceeded"));
    insert("r-blk", "paused", Some("blocked_gate"));

    let conn = db.lock();
    assert!(check_resumable(&conn, "r-done", "resume").is_err());
    assert!(check_resumable(&conn, "r-appr", "resume").is_err());
    // A question-paused run must go through `wf_answer`, not a bare resume —
    // otherwise the step re-runs with no human response folded in (§10.4).
    assert!(check_resumable(&conn, "r-ques", "resume").is_err());
    assert!(check_resumable(&conn, "r-budg", "resume").is_ok());
    assert!(check_resumable(&conn, "r-blk", "retry").is_ok());
}

#[test]
fn delete_guard_and_tree_order_protect_the_cascade() {
    // `wf_delete_run` (§13): the tree is collected children-first (the
    // `parent_run_id` FK has no cascade), and the guard rejects the whole
    // delete while any run in the tree is non-terminal.
    let tmp = tempfile::tempdir().unwrap();
    let db = crate::database::init(tmp.path()).unwrap();
    let insert = |id: &str, parent: Option<&str>, status: &str| {
        db.lock()
            .execute(
                "INSERT INTO wf_run (id,parent_run_id,name,spec_json,task,project_id,
                        repo_path,run_dir,branch,base_sha,status,budgets_json,spent_json,
                        created_at,updated_at)
                     VALUES (?1,?2,'n','{}','t','p','/r','/d','wf/x','sha',?3,'{}','{}',0,0)",
                rusqlite::params![id, parent, status],
            )
            .unwrap();
    };
    insert("r-parent", None, "done");
    insert("r-child", Some("r-parent"), "canceled");
    insert("r-grandchild", Some("r-child"), "running");

    let conn = db.lock();
    let mut order = Vec::new();
    run_tree_post_order(&conn, "r-parent", &mut order);
    assert_eq!(order, vec!["r-grandchild", "r-child", "r-parent"]);

    // One live descendant blocks the whole delete.
    let err = check_deletable(&conn, &order).unwrap_err().to_string();
    assert!(err.contains("cannot delete a running run"), "{err}");

    conn.execute(
        "UPDATE wf_run SET status='failed' WHERE id='r-grandchild'",
        [],
    )
    .unwrap();
    assert!(check_deletable(&conn, &order).is_ok());
}

#[tokio::test]
async fn discard_all_presses_past_a_failure_and_reports_it() {
    // The review blocker: a discard failing mid-loop must not strand the
    // run's other workspaces. Every id is attempted; the surviving one is
    // reported; the "all succeeded" result is false so the caller keeps the
    // run row for a re-delete.
    let ids = vec!["a1".to_string(), "a2".to_string(), "a3".to_string()];
    let mut errors = Vec::new();
    let attempted = std::sync::Mutex::new(Vec::new());
    let all = discard_all(&ids, "r", &mut errors, |id| {
        attempted.lock().unwrap().push(id.clone());
        async move {
            if id == "a2" {
                Err(Error::Other("wedged checkout".into()))
            } else {
                Ok(())
            }
        }
    })
    .await;

    assert!(!all, "a failed discard means not-all-discarded");
    assert_eq!(
        *attempted.lock().unwrap(),
        vec!["a1", "a2", "a3"],
        "every agent is attempted even after a1..a2 — the loop does not bail"
    );
    assert_eq!(errors.len(), 1, "only the failed discard is reported");
    assert!(
        errors[0].contains("a2") && errors[0].contains("wedged checkout"),
        "the failure names the agent and cause: {}",
        errors[0]
    );
}

#[tokio::test]
async fn discard_all_is_all_when_every_discard_succeeds() {
    let ids = vec!["a1".to_string(), "a2".to_string()];
    let mut errors = Vec::new();
    let all = discard_all(&ids, "r", &mut errors, |_| async { Ok(()) }).await;
    assert!(all);
    assert!(errors.is_empty());
}

#[test]
fn staging_multiple_run_dirs_restores_earlier_dirs_on_later_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs");
    let first = runs.join("r-first");
    let second = runs.join("r-second");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::write(first.join("marker"), "first").unwrap();
    std::fs::create_dir_all(&second).unwrap();
    std::fs::write(second.join("marker"), "second").unwrap();

    // Make the second staging rename fail after the first has succeeded.
    let second_staged = runs.join("r-second.deleting");
    std::fs::create_dir_all(&second_staged).unwrap();
    std::fs::write(second_staged.join("collision"), "occupied").unwrap();

    let ids = vec!["r-first".to_string(), "r-second".to_string()];
    let error = stage_run_dirs_at(&ids, |id| Ok(runs.join(id))).unwrap_err();
    assert!(error.to_string().contains("cannot stage run dir"));
    assert!(first.join("marker").exists(), "first dir restored intact");
    assert!(second.join("marker").exists(), "failed dir remains intact");
    assert!(
        !runs.join("r-first.deleting").exists(),
        "earlier staged dir is not stranded"
    );
}

#[test]
fn delete_run_data_cascades_the_runs_rows() {
    // Deleting the `wf_run` row must take its journal, execs, and messages
    // with it (0019 ON DELETE CASCADE + the connection's foreign_keys
    // pragma), and tolerate an already-missing run directory.
    let tmp = tempfile::tempdir().unwrap();
    let db = crate::database::init(tmp.path()).unwrap();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                base_sha,status,budgets_json,spent_json,created_at,updated_at)
             VALUES ('r-del','n','{}','t','p','/r','/d','wf/x','sha','done','{}','{}',0,0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode)
             VALUES ('e1','r-del','s',1,0,'done','verdict')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO wf_event (run_id,seq,ts,type,payload_json)
             VALUES ('r-del',1,0,'run_launched','{}')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO wf_message (id,run_id,kind,body_json,status,created_at)
             VALUES ('m1','r-del','report','{}','delivered',0)",
        [],
    )
    .unwrap();

    // A real run dir with content: it must be gone (not just staged) after.
    let dir = tmp.path().join("runs").join("r-del");
    std::fs::create_dir_all(dir.join("blackboard")).unwrap();
    std::fs::write(dir.join("blackboard").join("task.md"), "t").unwrap();

    delete_run_data_at(&conn, "r-del", &dir).expect("delete succeeds");

    let count = |table: &str| -> i64 {
        conn.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE run_id = 'r-del'"),
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    let runs: i64 = conn
        .query_row("SELECT COUNT(*) FROM wf_run WHERE id='r-del'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(runs, 0);
    assert_eq!(count("wf_step_exec"), 0, "execs cascade");
    assert_eq!(count("wf_event"), 0, "journal cascades");
    assert_eq!(count("wf_message"), 0, "messages cascade");
    assert!(!dir.exists(), "run dir removed");
    assert!(
        !dir.with_file_name("r-del.deleting").exists(),
        "no staged dir left behind"
    );
}

#[test]
fn delete_run_data_restores_the_dir_when_the_row_delete_fails() {
    // The row delete is the commit point: if it fails, the staged dir is
    // renamed back so the surviving run row never points at missing state.
    // The failure here is real — a child run's parent_run_id FK (no
    // cascade) rejects deleting the parent row.
    let tmp = tempfile::tempdir().unwrap();
    let db = crate::database::init(tmp.path()).unwrap();
    let conn = db.lock();
    let insert = |id: &str, parent: Option<&str>| {
        conn.execute(
            "INSERT INTO wf_run (id,parent_run_id,name,spec_json,task,project_id,repo_path,
                    run_dir,branch,base_sha,status,budgets_json,spent_json,created_at,updated_at)
                 VALUES (?1,?2,'n','{}','t','p','/r','/d','wf/x','sha','done','{}','{}',0,0)",
            rusqlite::params![id, parent],
        )
        .unwrap();
    };
    insert("r-parent", None);
    insert("r-child", Some("r-parent"));
    let dir = tmp.path().join("runs").join("r-parent");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("marker"), "m").unwrap();

    let err = delete_run_data_at(&conn, "r-parent", &dir).unwrap_err();
    assert!(err.to_string().contains("FOREIGN KEY"), "{err}");
    assert!(dir.join("marker").exists(), "dir renamed back intact");
    assert!(
        !dir.with_file_name("r-parent.deleting").exists(),
        "staged dir gone after restore"
    );
    let runs: i64 = conn
        .query_row("SELECT COUNT(*) FROM wf_run WHERE id='r-parent'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(runs, 1, "row untouched");
}

#[test]
fn delete_run_data_sweeps_a_crashed_attempts_staged_dir() {
    // A crash between the staging rename and the row delete leaves rows
    // plus a `<id>.deleting` dir and no live dir; the retry must finish
    // the job rather than orphan the staged dir.
    let tmp = tempfile::tempdir().unwrap();
    let db = crate::database::init(tmp.path()).unwrap();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                base_sha,status,budgets_json,spent_json,created_at,updated_at)
             VALUES ('r-crash','n','{}','t','p','/r','/d','wf/x','sha','done','{}','{}',0,0)",
        [],
    )
    .unwrap();
    let dir = tmp.path().join("runs").join("r-crash");
    let staged = dir.with_file_name("r-crash.deleting");
    std::fs::create_dir_all(&staged).unwrap();

    delete_run_data_at(&conn, "r-crash", &dir).expect("retry completes");
    assert!(!staged.exists(), "leftover staged dir swept");
    let runs: i64 = conn
        .query_row("SELECT COUNT(*) FROM wf_run WHERE id='r-crash'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(runs, 0);
}

#[test]
fn startup_recovery_reconciles_staged_run_dirs() {
    // An app exit between the staging rename and the row delete strands a
    // `<id>.deleting` dir. At the next startup: a staged dir whose run row
    // survives is renamed back (the run is openable again without user
    // action); one whose row is gone is swept; live dirs are untouched.
    let tmp = tempfile::tempdir().unwrap();
    let db = crate::database::init(tmp.path()).unwrap();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                base_sha,status,budgets_json,spent_json,created_at,updated_at)
             VALUES ('r-alive','n','{}','t','p','/r','/d','wf/x','sha','done','{}','{}',0,0)",
        [],
    )
    .unwrap();
    let root = tmp.path().join("runs");
    std::fs::create_dir_all(root.join("r-alive.deleting")).unwrap();
    std::fs::write(root.join("r-alive.deleting").join("marker"), "m").unwrap();
    std::fs::create_dir_all(root.join("r-gone.deleting")).unwrap();
    std::fs::create_dir_all(root.join("r-normal")).unwrap();

    recover_staged_run_dirs(&conn, &root);

    assert!(
        root.join("r-alive").join("marker").exists(),
        "surviving run's dir restored intact"
    );
    assert!(!root.join("r-alive.deleting").exists(), "staged name gone");
    assert!(
        !root.join("r-gone.deleting").exists(),
        "completed delete's tail swept"
    );
    assert!(root.join("r-normal").exists(), "live dirs untouched");
}

#[test]
fn commit_done_unless_ask_ties_the_gate_to_the_ask_check() {
    // The atomic commit point (§10.4): finalize `done` only when no ask is
    // queued for the exec; a pending ask blocks the commit so the caller can
    // pause `question` instead of advancing.
    let tmp = tempfile::tempdir().unwrap();
    let db = crate::database::init(tmp.path()).unwrap();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                base_sha,status,budgets_json,spent_json,created_at,updated_at)
             VALUES ('r','n','{}','t','p','/r','/d','wf/x','sha','running','{}','{}',0,0)",
        [],
    )
    .unwrap();
    let mk_exec = |id: &str| {
        conn.execute(
            "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode)
                 VALUES (?1,'r','s',1,0,'running','verdict')",
            [id],
        )
        .unwrap();
    };

    // No ask → commits `done` with the ferried head.
    mk_exec("e-clean");
    assert!(commit_done_unless_ask(&conn, "e-clean", "sha1"));
    let (status, head): (String, Option<String>) = conn
        .query_row(
            "SELECT status, head_end FROM wf_step_exec WHERE id='e-clean'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "done");
    assert_eq!(head.as_deref(), Some("sha1"));

    // A queued ask against the exec → does NOT commit; the exec stays live so
    // the caller pauses `question`.
    mk_exec("e-ask");
    conn.execute(
        "INSERT INTO wf_message (id,run_id,from_step_exec_id,to_step_exec_id,kind,
                body_json,status,created_at)
             VALUES ('m1','r','e-ask',NULL,'ask','{}','queued',0)",
        [],
    )
    .unwrap();
    assert!(!commit_done_unless_ask(&conn, "e-ask", "sha2"));
    let status: String = conn
        .query_row(
            "SELECT status FROM wf_step_exec WHERE id='e-ask'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        status, "running",
        "must not finalize while an ask is pending"
    );
}

// ───────────────────────── parallel stages (S8) ─────────────────────────

/// Markers embedded in a child's goal so [`MatrixDriver`] can script its
/// per-child behaviour off the (goal-bearing) step prompt.
const FAIL: &str = "PZFAIL";
const HANG: &str = "PZHANG";
/// A child that creates the *same* file as its siblings so the second merge
/// of an `integrate: merge` stage conflicts (add/add). §12.3.
const CONFLICT: &str = "PZCONFLICT";
/// The shared file `CONFLICT` children (and the resolver) all touch.
const CONFLICT_FILE: &str = "conflict.txt";

#[derive(Clone, Copy)]
enum Beh {
    Success,
    Fail,
    Hang,
    /// Write `CONFLICT_FILE` with unique content so siblings collide.
    Conflict,
    /// A conflict-resolution step: overwrite `CONFLICT_FILE` to a single
    /// resolved value (removing the markers) and commit.
    Resolve,
}

/// A real-git stub like [`StubDriver`] with per-child behaviour keyed off the
/// step goal: a child whose goal contains `PZFAIL` runs turns but never
/// commits (its `commit` gate stays unmet → failure); `PZHANG` starts a turn
/// and never ends it (until the stage cancels it); anything else commits
/// (success, moving HEAD → `integrate_skipped`).
struct MatrixDriver {
    root: PathBuf,
    tx: broadcast::Sender<StatusEvent>,
    state: parking_lot::Mutex<MatrixState>,
}
#[derive(Default)]
struct MatrixState {
    statuses: HashMap<String, AgentStatus>,
    worktrees: HashMap<String, PathBuf>,
    /// Behaviour fixed on the agent's first prompt (the step prompt carries
    /// the goal marker; a later reprompt does not, so it must not re-derive).
    behavior: HashMap<String, Beh>,
    archived: Vec<String>,
    stopped: Vec<String>,
    count: usize,
}
impl MatrixDriver {
    fn new(root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            root,
            tx: broadcast::channel(256).0,
            state: parking_lot::Mutex::new(MatrixState::default()),
        })
    }
    fn set(&self, id: &str, s: AgentStatus) {
        self.state.lock().statuses.insert(id.to_string(), s.clone());
        let _ = self.tx.send(StatusEvent {
            agent_id: id.to_string(),
            status: s,
        });
    }
}
impl AgentDriver for MatrixDriver {
    fn spawn(
        &self,
        req: SpawnReq,
    ) -> super::super::driver::BoxFuture<'_, Result<super::super::driver::SpawnedAgent>> {
        Box::pin(async move {
            let id = {
                let mut st = self.state.lock();
                st.count += 1;
                format!("m-{}", st.count)
            };
            let dest = self.root.join(&id);
            let base_ref = req.fork_base.clone().unwrap();
            let spec = crate::sandbox::provision::CheckoutSpec {
                source_repo: &req.repo_path,
                base_ref: &base_ref,
                dest: &dest,
            };
            crate::sandbox::provision::provision_forking_run_repo(
                &spec,
                req.run_repo.as_ref().unwrap(),
            )
            .await?;
            sh(&dest, &["config", "user.email", "t@t.t"]);
            sh(&dest, &["config", "user.name", "t"]);
            self.state.lock().worktrees.insert(id.clone(), dest.clone());
            self.set(&id, AgentStatus::Idle);
            Ok(super::super::driver::SpawnedAgent {
                agent_id: id,
                worktree: dest,
            })
        })
    }
    fn status(&self, id: &str) -> Option<AgentStatus> {
        self.state.lock().statuses.get(id).cloned()
    }
    fn subscribe(&self) -> broadcast::Receiver<StatusEvent> {
        self.tx.subscribe()
    }
    fn send_message<'a>(
        &'a self,
        id: &'a str,
        text: String,
    ) -> super::super::driver::BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let (wt, beh) = {
                let mut st = self.state.lock();
                let wt = st.worktrees.get(id).cloned().unwrap();
                let beh = *st.behavior.entry(id.to_string()).or_insert_with(|| {
                    // The resolution step's prompt names conflict markers; a
                    // `CONFLICT` child creates the shared file; others follow
                    // their goal marker.
                    if text.contains("conflict marker") {
                        Beh::Resolve
                    } else if text.contains(CONFLICT) {
                        Beh::Conflict
                    } else if text.contains(HANG) {
                        Beh::Hang
                    } else if text.contains(FAIL) {
                        Beh::Fail
                    } else {
                        Beh::Success
                    }
                });
                (wt, beh)
            };
            self.set(id, AgentStatus::Running);
            match beh {
                Beh::Hang => return Ok(()), // turn never ends — only a cancel unblocks it
                Beh::Fail => {}             // no commit → commit gate stays unmet
                Beh::Success => {
                    std::fs::write(wt.join(format!("{id}.txt")), "work").unwrap();
                    sh(&wt, &["add", "-A"]);
                    sh(&wt, &["commit", "-qm", "child work"]);
                }
                Beh::Conflict => {
                    // Unique content in a shared file → add/add conflict when a
                    // sibling's ref is merged after this one.
                    std::fs::write(wt.join(CONFLICT_FILE), format!("from {id}\n")).unwrap();
                    sh(&wt, &["add", "-A"]);
                    sh(&wt, &["commit", "-qm", "conflicting work"]);
                }
                Beh::Resolve => {
                    // Overwrite the conflicted file with a single resolved value
                    // (markers gone) and commit — satisfies the `commit` gate.
                    std::fs::write(wt.join(CONFLICT_FILE), "resolved\n").unwrap();
                    sh(&wt, &["add", "-A"]);
                    sh(&wt, &["commit", "-qm", "resolve conflict"]);
                }
            }
            self.set(id, AgentStatus::Idle);
            Ok(())
        })
    }
    fn stop<'a>(&'a self, id: &'a str) -> super::super::driver::BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.state.lock().stopped.push(id.to_string());
            Ok(())
        })
    }
    fn archive<'a>(&'a self, id: &'a str) -> super::super::driver::BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.state.lock().archived.push(id.to_string());
            Ok(())
        })
    }
    fn last_activity(&self, _id: &str) -> Option<i64> {
        None
    }
    fn turn_usage(&self, _id: &str) -> Option<super::super::driver::TurnUsage> {
        None
    }
}

/// A commit-gated parallel child; `marker` (`""` / `FAIL` / `HANG`) selects
/// the driver behaviour.
fn cstep(id: &str, marker: &str) -> Step {
    Step {
        id: id.to_string(),
        agent: "coder".to_string(),
        goal: format!("child {id} {marker}"),
        gate: Gate::Commit,
        budgets: None,
        comms: vec![],
    }
}

/// A run whose whole workflow is one `parallel { integrate: none }` block.
/// Returns the db, the workspace root for the driver, and the base SHA.
fn scaffold_parallel(
    tmp: &Path,
    run_id: &str,
    join: Join,
    children: &[Step],
) -> (Db, PathBuf, String) {
    scaffold_parallel_integrate(tmp, run_id, join, Integrate::None, children)
}

#[allow(clippy::too_many_lines)]
fn scaffold_parallel_integrate(
    tmp: &Path,
    run_id: &str,
    join: Join,
    integrate: Integrate,
    children: &[Step],
) -> (Db, PathBuf, String) {
    let source = tmp.join(format!("src-{run_id}"));
    std::fs::create_dir_all(&source).unwrap();
    sh(&source, &["init", "-q", "-b", "main"]);
    sh(&source, &["config", "user.email", "t@t.t"]);
    sh(&source, &["config", "user.name", "t"]);
    std::fs::write(source.join("README"), "base").unwrap();
    sh(&source, &["add", "-A"]);
    sh(&source, &["commit", "-qm", "base"]);
    let base_sha = {
        let o = Sh::new("git")
            .current_dir(&source)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    let run_dir = tmp.join(format!("rd-{run_id}"));
    std::fs::create_dir_all(blackboard::blackboard_dir(&run_dir)).unwrap();
    let mut agents = BTreeMap::new();
    agents.insert(
        "coder".to_string(),
        super::super::spec::AgentSpec {
            base: "codex".to_string(),
            model: None,
            effort: None,
            instructions: None,
            skills: vec![],
            mcp_servers: vec![],
            custom_agent: None,
        },
    );
    let spec = Spec {
        version: 1,
        name: "par".to_string(),
        description: None,
        budgets: None,
        agents,
        workflow: vec![Block::Parallel(Parallel {
            join,
            integrate,
            max_concurrent: None,
            steps: children.to_vec(),
        })],
        finalize: None,
    };
    let spec_json = serde_json::to_string(&spec).unwrap();
    let db = crate::database::init(tmp).unwrap();
    db.lock()
        .execute(
            "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                    base_sha,status,budgets_json,spent_json,created_at,updated_at)
                 VALUES (?1,'par',?2,'t','p',?3,?4,'wf/par-x',?5,'pending','{}','{}',0,0)",
            rusqlite::params![
                run_id,
                spec_json,
                source.to_string_lossy(),
                run_dir.to_string_lossy(),
                base_sha,
            ],
        )
        .unwrap();
    (db, tmp.join(format!("ws-{run_id}")), base_sha)
}

fn par_ctx(db: Db, driver: Arc<MatrixDriver>) -> RunCtx {
    RunCtx {
        db,
        driver,
        app: None,
        cancel: Arc::new(AtomicBool::new(false)),
        pending_ask: Arc::new(AtomicBool::new(false)),
        deadlines: Deadlines::default(),
        runs: None,
    }
}

fn run_status_str(db: &Db, run_id: &str) -> String {
    db.lock()
        .query_row("SELECT status FROM wf_run WHERE id=?1", [run_id], |r| {
            r.get(0)
        })
        .unwrap()
}

fn count_children(db: &Db, run_id: &str, status: &str) -> i64 {
    db.lock()
        .query_row(
            "SELECT COUNT(*) FROM wf_step_exec WHERE run_id=?1 AND status=?2",
            rusqlite::params![run_id, status],
            |r| r.get(0),
        )
        .unwrap()
}

#[tokio::test]
async fn parallel_all_success_reaches_done_and_journals_integrate_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let children = vec![cstep("a", ""), cstep("b", ""), cstep("c", "")];
    let (db, ws, _base) = scaffold_parallel(tmp.path(), "run-pa", Join::All, &children);
    let ctx = par_ctx(db.clone(), MatrixDriver::new(ws));
    drive_run(&ctx, "run-pa").await;

    assert_eq!(run_status_str(&db, "run-pa"), "done");
    assert_eq!(count_children(&db, "run-pa", "done"), 3, "every child done");
    // `integrate: none` — each committing child left its work on its fork.
    let skipped: i64 = db
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM wf_event WHERE run_id='run-pa' AND type=?1",
            [event_type::INTEGRATE_SKIPPED],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(skipped, 3, "one integrate_skipped per committing child");
}

#[tokio::test]
async fn parallel_all_fails_when_a_child_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let children = vec![cstep("ok", ""), cstep("bad", FAIL)];
    let (db, ws, _b) = scaffold_parallel(tmp.path(), "run-af", Join::All, &children);
    let ctx = par_ctx(db.clone(), MatrixDriver::new(ws));
    drive_run(&ctx, "run-af").await;

    let (status, err): (String, Option<String>) = db
        .lock()
        .query_row(
            "SELECT status, error FROM wf_run WHERE id='run-af'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "failed");
    assert!(
        err.unwrap_or_default().contains("parallel stage failed"),
        "failure names its cause"
    );
}

#[tokio::test]
async fn parallel_any_first_success_wins_and_cancels_the_loser() {
    let tmp = tempfile::tempdir().unwrap();
    // One fast success + one hanging child; `any` → success wins and the
    // hanging loser is cancelled + archived (§6.6).
    let children = vec![cstep("win", ""), cstep("slow", HANG)];
    let (db, ws, _b) = scaffold_parallel(tmp.path(), "run-any", Join::Any, &children);
    let driver = MatrixDriver::new(ws);
    let ctx = par_ctx(db.clone(), driver.clone());
    drive_run(&ctx, "run-any").await;

    assert_eq!(run_status_str(&db, "run-any"), "done");
    assert_eq!(count_children(&db, "run-any", "done"), 1, "one winner");
    assert_eq!(count_children(&db, "run-any", "abandoned"), 1, "one loser");

    // The loser was stopped and archived (its chat stays replayable) — the
    // spawn-race fix guarantees the agent id was known when it was cancelled.
    let loser: Option<String> = db
        .lock()
        .query_row(
            "SELECT agent_id FROM wf_step_exec
                 WHERE run_id='run-any' AND status='abandoned'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let loser = loser.expect("cancelled loser has an agent id");
    assert!(
        driver.state.lock().stopped.contains(&loser),
        "loser stopped"
    );
    assert!(
        driver.state.lock().archived.contains(&loser),
        "loser archived"
    );
}

#[tokio::test]
async fn parallel_any_fails_only_when_all_children_fail() {
    let tmp = tempfile::tempdir().unwrap();
    let children = vec![cstep("x", FAIL), cstep("y", FAIL)];
    let (db, ws, _b) = scaffold_parallel(tmp.path(), "run-anf", Join::Any, &children);
    let ctx = par_ctx(db.clone(), MatrixDriver::new(ws));
    drive_run(&ctx, "run-anf").await;

    let (status, err): (String, Option<String>) = db
        .lock()
        .query_row(
            "SELECT status, error FROM wf_run WHERE id='run-anf'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "failed");
    assert!(err
        .unwrap_or_default()
        .contains("all parallel children failed"));
}

#[tokio::test]
async fn resume_parallel_redrives_only_unfinished_children() {
    let tmp = tempfile::tempdir().unwrap();
    // A prior drive finished `done_child` before dying; resume must not
    // re-run it and must drive the remaining child to done (§12.3 / S8).
    let children = vec![cstep("done_child", ""), cstep("todo_child", "")];
    let (db, ws, _b) = scaffold_parallel(tmp.path(), "run-rp", Join::All, &children);
    db.lock()
        .execute("UPDATE wf_run SET status='running' WHERE id='run-rp'", [])
        .unwrap();
    db.lock()
            .execute(
                "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode,head_end)
                 VALUES ('exec-prior','run-rp','done_child',1,0,'done','commit','deadbeef')",
                [],
            )
            .unwrap();
    let ctx = par_ctx(db.clone(), MatrixDriver::new(ws));
    drive_run(&ctx, "run-rp").await;

    let done_child_execs: i64 = db
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-rp' AND step_id='done_child'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        done_child_execs, 1,
        "the done child must not be re-executed"
    );
    let todo_done: i64 = db
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM wf_step_exec
                 WHERE run_id='run-rp' AND step_id='todo_child' AND status='done'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(todo_done, 1, "the unfinished child ran to done");
    assert_eq!(run_status_str(&db, "run-rp"), "done");
}

// ──────────────────── code-producing parallel: merge (S9) ───────────────

/// `pick_winners`: `all` keeps every child in spec order; `any` uses the live
/// winner hint when present, else the earliest finisher (ties → spec order).
#[test]
fn pick_winners_selects_the_right_branch() {
    // (step_id, ref, ended_at) in spec order: a is spec-first, b finished first.
    let done = || {
        vec![
            ("a".to_string(), "ref-a".to_string(), 200),
            ("b".to_string(), "ref-b".to_string(), 100),
        ]
    };

    // `all` → every child, spec order, untouched.
    assert_eq!(
        pick_winners(done(), Join::All, None),
        vec![
            ("a".to_string(), "ref-a".to_string()),
            ("b".to_string(), "ref-b".to_string())
        ]
    );

    // `any` + live winner hint → exactly that child, even if spec-later.
    assert_eq!(
        pick_winners(done(), Join::Any, Some("b")),
        vec![("b".to_string(), "ref-b".to_string())]
    );

    // `any`, no hint (resume) → earliest finisher (b @100), not spec-first (a).
    assert_eq!(
        pick_winners(done(), Join::Any, None),
        vec![("b".to_string(), "ref-b".to_string())]
    );

    // `any`, no hint, tied ended_at → stable fallback to spec order (a).
    let tied = vec![
        ("a".to_string(), "ref-a".to_string(), 100),
        ("b".to_string(), "ref-b".to_string(), 100),
    ];
    assert_eq!(
        pick_winners(tied, Join::Any, None),
        vec![("a".to_string(), "ref-a".to_string())]
    );
}

fn sh_out(dir: &Path, args: &[&str]) -> String {
    let out = Sh::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git");
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn count_events(db: &Db, run_id: &str, ty: &str) -> i64 {
    db.lock()
        .query_row(
            "SELECT COUNT(*) FROM wf_event WHERE run_id=?1 AND type=?2",
            rusqlite::params![run_id, ty],
            |r| r.get(0),
        )
        .unwrap()
}

/// Record a resolution choice on the paused merge cursor — what
/// `wf_resolve_conflict` does, exercised directly so the test can also stage
/// the human's edit before re-driving.
fn set_resolution(db: &Db, run_id: &str, mode: &str) {
    let conn = db.lock();
    let mut cur = get_cursor(&conn, run_id);
    cur.merge
        .as_mut()
        .unwrap()
        .conflict
        .as_mut()
        .unwrap()
        .resolution = Some(mode.to_string());
    set_cursor(&conn, run_id, &cur);
}

/// The tree of the merge stage's integrated result, as a newline-joined file
/// list, read from the run repo (§12.1).
fn merge_tree(tmp: &Path, db: &Db, run_id: &str) -> String {
    let run_repo = tmp.join(format!("rd-{run_id}")).join("repo");
    let merge_ref = {
        let conn = db.lock();
        gitops::step_ref(&latest_done_exec_for_step(&conn, run_id, &merge_step_id(0)).unwrap())
    };
    sh_out(&run_repo, &["ls-tree", "--name-only", "-r", &merge_ref])
}

/// §16: clean merges in spec order integrate every child's work and the run
/// advances onto the merged result.
#[tokio::test]
async fn merge_stage_integrates_children_and_reaches_done() {
    let tmp = tempfile::tempdir().unwrap();
    // Disjoint files → two clean merges.
    let children = vec![cstep("a", ""), cstep("b", "")];
    let (db, ws, _b) =
        scaffold_parallel_integrate(tmp.path(), "run-mg", Join::All, Integrate::Merge, &children);
    let ctx = par_ctx(db.clone(), MatrixDriver::new(ws));
    drive_run(&ctx, "run-mg").await;

    assert_eq!(run_status_str(&db, "run-mg"), "done");
    assert_eq!(
        count_events(&db, "run-mg", event_type::MERGE_DONE),
        2,
        "one merge_done per child, in spec order"
    );
    let files = merge_tree(tmp.path(), &db, "run-mg");
    assert!(
        files.contains("m-1.txt") && files.contains("m-2.txt"),
        "both children's work is present in the integrated tree: {files}"
    );
}

/// §16: an induced conflict pauses the run `conflict` and names the file.
#[tokio::test]
async fn merge_conflict_pauses_with_file_list() {
    let tmp = tempfile::tempdir().unwrap();
    let children = vec![cstep("a", CONFLICT), cstep("b", CONFLICT)];
    let (db, ws, _b) =
        scaffold_parallel_integrate(tmp.path(), "run-mc", Join::All, Integrate::Merge, &children);
    let ctx = par_ctx(db.clone(), MatrixDriver::new(ws));
    drive_run(&ctx, "run-mc").await;

    let (status, reason): (String, Option<String>) = db
        .lock()
        .query_row(
            "SELECT status, paused_reason FROM wf_run WHERE id='run-mc'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "paused");
    assert_eq!(reason.as_deref(), Some("conflict"));

    let payload: String = db
        .lock()
        .query_row(
            "SELECT payload_json FROM wf_event WHERE run_id='run-mc' AND type=?1",
            [event_type::MERGE_CONFLICT],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        payload.contains(CONFLICT_FILE),
        "the conflict names its file: {payload}"
    );
    // The resumable conflict state is persisted on the cursor.
    let cur = get_cursor(&db.lock(), "run-mc");
    assert!(
        cur.merge
            .as_ref()
            .and_then(|m| m.conflict.as_ref())
            .is_some(),
        "conflict recorded for resume"
    );
}

/// §16 mode (a): an agent conflict-resolution step drives the run to done.
#[tokio::test]
async fn merge_conflict_resolved_by_agent_reaches_done() {
    let tmp = tempfile::tempdir().unwrap();
    let children = vec![cstep("a", CONFLICT), cstep("b", CONFLICT)];
    let (db, ws, _b) =
        scaffold_parallel_integrate(tmp.path(), "run-ma", Join::All, Integrate::Merge, &children);
    let ctx = par_ctx(db.clone(), MatrixDriver::new(ws));
    drive_run(&ctx, "run-ma").await;
    assert_eq!(run_status_str(&db, "run-ma"), "paused");

    // `wf_resolve_conflict(run, "agent")` then re-drive.
    set_resolution(&db, "run-ma", "agent");
    drive_run(&ctx, "run-ma").await;

    assert_eq!(run_status_str(&db, "run-ma"), "done");
    // The resolution step ran (its `__resolve_0` exec is done) and the
    // integrated file carries the resolved value, not markers.
    let resolved: i64 = db
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM wf_step_exec
                 WHERE run_id='run-ma' AND step_id='__resolve_0' AND status='done'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(resolved, 1, "the conflict-resolution step ran to done");
    let run_repo = tmp.path().join("rd-run-ma").join("repo");
    let merge_ref = {
        let conn = db.lock();
        gitops::step_ref(&latest_done_exec_for_step(&conn, "run-ma", &merge_step_id(0)).unwrap())
    };
    let body = sh_out(
        &run_repo,
        &["show", &format!("{merge_ref}:{CONFLICT_FILE}")],
    );
    assert!(body.contains("resolved"), "markers resolved: {body}");
    assert!(!body.contains("<<<<<<<"), "no leftover markers: {body}");
}

/// §16 mode (c): the human resolves in the integration worktree and the run
/// resumes to done.
#[tokio::test]
async fn merge_conflict_resolved_by_human_reaches_done() {
    let tmp = tempfile::tempdir().unwrap();
    let children = vec![cstep("a", CONFLICT), cstep("b", CONFLICT)];
    let (db, ws, _b) =
        scaffold_parallel_integrate(tmp.path(), "run-mh", Join::All, Integrate::Merge, &children);
    let ctx = par_ctx(db.clone(), MatrixDriver::new(ws));
    drive_run(&ctx, "run-mh").await;
    assert_eq!(run_status_str(&db, "run-mh"), "paused");

    // The human resolves in the run repo's integration worktree and commits.
    let int_wt = tmp.path().join("rd-run-mh").join("integrate-0");
    std::fs::write(int_wt.join(CONFLICT_FILE), "human-resolved\n").unwrap();
    sh(&int_wt, &["add", "-A"]);
    sh(&int_wt, &["commit", "-qm", "human resolution"]);

    // `wf_resolve_conflict(run, "human")` then re-drive.
    set_resolution(&db, "run-mh", "human");
    drive_run(&ctx, "run-mh").await;

    assert_eq!(run_status_str(&db, "run-mh"), "done");
    let run_repo = tmp.path().join("rd-run-mh").join("repo");
    let merge_ref = {
        let conn = db.lock();
        gitops::step_ref(&latest_done_exec_for_step(&conn, "run-mh", &merge_step_id(0)).unwrap())
    };
    let body = sh_out(
        &run_repo,
        &["show", &format!("{merge_ref}:{CONFLICT_FILE}")],
    );
    assert!(
        body.contains("human-resolved"),
        "human resolution integrated: {body}"
    );
}

/// A slow sibling can race past `any`'s stage-cancel and land its own `done`
/// exec. The merge must integrate exactly ONE branch — the child that
/// FINISHED FIRST, not the first in spec order. Pre-seed both children `done`
/// with `b` (spec-second) finishing *before* `a` (spec-first), then assert the
/// integrated tree carries `b`'s work and drops `a`'s.
#[tokio::test]
async fn merge_any_integrates_the_child_that_finished_first() {
    let tmp = tempfile::tempdir().unwrap();
    let children = vec![cstep("a", ""), cstep("b", "")];
    let (db, _ws, _base) = scaffold_parallel_integrate(
        tmp.path(),
        "run-ma1",
        Join::Any,
        Integrate::Merge,
        &children,
    );

    // Provision the run repo and ferry two real child commits into it, then
    // mark both children `done` — the raced state a live `any` stage can leave
    // behind. `b` finished first (smaller `ended_at`) so `b` is the winner,
    // even though `a` is earlier in spec order.
    let source = tmp.path().join("src-run-ma1");
    let run_dir = tmp.path().join("rd-run-ma1");
    let run_repo = gitops::provision_run_repo(&source, &run_dir).await.unwrap();
    for (child, exec, ended) in [("a", "exec-a", 200_i64), ("b", "exec-b", 100_i64)] {
        let ws = tmp.path().join(format!("wsx-{child}"));
        sh(
            tmp.path(),
            &[
                "clone",
                "-q",
                "--shared",
                source.to_str().unwrap(),
                ws.to_str().unwrap(),
            ],
        );
        sh(&ws, &["config", "user.email", "t@t.t"]);
        sh(&ws, &["config", "user.name", "t"]);
        std::fs::write(ws.join(format!("{child}.txt")), "work").unwrap();
        gitops::boundary_commit(&ws, "child").await.unwrap();
        let r = gitops::pin_step_ref(&ws, exec).await.unwrap();
        gitops::ferry(&ws, &run_repo, &r).await.unwrap();
        db.lock()
                .execute(
                    "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode,head_end,ended_at)
                     VALUES (?1,'run-ma1',?2,1,0,'done','commit','x',?3)",
                    rusqlite::params![exec, child, ended],
                )
                .unwrap();
    }
    db.lock()
        .execute("UPDATE wf_run SET status='running' WHERE id='run-ma1'", [])
        .unwrap();

    let ctx = par_ctx(db.clone(), MatrixDriver::new(tmp.path().join("ws-run-ma1")));
    drive_run(&ctx, "run-ma1").await;

    assert_eq!(run_status_str(&db, "run-ma1"), "done");
    assert_eq!(
        count_events(&db, "run-ma1", event_type::MERGE_DONE),
        1,
        "exactly one branch merged under `any`"
    );
    let files = merge_tree(tmp.path(), &db, "run-ma1");
    assert!(
        files.contains("b.txt") && !files.contains("a.txt"),
        "the first-finished child (b) is integrated, not the spec-first (a): {files}"
    );
}

/// Human resolution must be committed: if the user edits the integration
/// worktree but continues without committing, the run refuses (re-pauses)
/// rather than resetting their edits away and merging on from a marker tree.
#[tokio::test]
async fn merge_human_resolution_requires_a_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let children = vec![cstep("a", CONFLICT), cstep("b", CONFLICT)];
    let (db, ws, _b) = scaffold_parallel_integrate(
        tmp.path(),
        "run-mhu",
        Join::All,
        Integrate::Merge,
        &children,
    );
    let ctx = par_ctx(db.clone(), MatrixDriver::new(ws));
    drive_run(&ctx, "run-mhu").await;
    assert_eq!(run_status_str(&db, "run-mhu"), "paused");

    // Human edits the conflicted file but does NOT commit, then continues.
    let int_wt = tmp.path().join("rd-run-mhu").join("integrate-0");
    std::fs::write(int_wt.join(CONFLICT_FILE), "edited but uncommitted\n").unwrap();
    set_resolution(&db, "run-mhu", "human");
    drive_run(&ctx, "run-mhu").await;

    // Refused: still paused(conflict), not advanced; the choice is cleared so
    // the user must commit and retry, and the edit is left in place (not reset).
    assert_eq!(run_status_str(&db, "run-mhu"), "paused");
    let cur = get_cursor(&db.lock(), "run-mhu");
    assert!(
        cur.merge
            .and_then(|m| m.conflict)
            .and_then(|c| c.resolution)
            .is_none(),
        "resolution cleared — the user must commit first"
    );
    let body = std::fs::read_to_string(int_wt.join(CONFLICT_FILE)).unwrap();
    assert!(
        body.contains("edited but uncommitted"),
        "the uncommitted edit is preserved, not discarded: {body}"
    );
}

/// A committed human "resolution" that still contains conflict markers must be
/// rejected — otherwise the merge would finish with markers in the integrated
/// result. The run re-pauses; it does not reach `done`.
#[tokio::test]
async fn merge_human_resolution_with_markers_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let children = vec![cstep("a", CONFLICT), cstep("b", CONFLICT)];
    let (db, ws, _b) = scaffold_parallel_integrate(
        tmp.path(),
        "run-mhm",
        Join::All,
        Integrate::Merge,
        &children,
    );
    let ctx = par_ctx(db.clone(), MatrixDriver::new(ws));
    drive_run(&ctx, "run-mhm").await;
    assert_eq!(run_status_str(&db, "run-mhm"), "paused");

    // Human commits a *partial* resolution: they stripped the outer
    // <<<<<<< / >>>>>>> bounds but left the ======= divider behind.
    let int_wt = tmp.path().join("rd-run-mhm").join("integrate-0");
    std::fs::write(
        int_wt.join(CONFLICT_FILE),
        "from one side\n=======\nfrom the other side\n",
    )
    .unwrap();
    sh(&int_wt, &["add", "-A"]);
    sh(&int_wt, &["commit", "-qm", "partial resolution"]);
    set_resolution(&db, "run-mhm", "human");
    drive_run(&ctx, "run-mhm").await;

    // Refused: still paused(conflict), not done; the choice is cleared.
    assert_eq!(run_status_str(&db, "run-mhm"), "paused");
    let reason: Option<String> = db
        .lock()
        .query_row(
            "SELECT paused_reason FROM wf_run WHERE id='run-mhm'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(reason.as_deref(), Some("conflict"));
    let cur = get_cursor(&db.lock(), "run-mhm");
    assert!(
        cur.merge
            .and_then(|m| m.conflict)
            .and_then(|c| c.resolution)
            .is_none(),
        "resolution cleared — the user must strip the markers and retry"
    );
}

/// Resolving by *renaming* a still-conflicted file must not slip markers past
/// the guard: the scan covers paths the resolution changed since the snapshot,
/// not just the originally-conflicted paths, so the renamed file is caught.
#[tokio::test]
async fn merge_human_resolution_that_renames_a_marker_file_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let children = vec![cstep("a", CONFLICT), cstep("b", CONFLICT)];
    let (db, ws, _b) = scaffold_parallel_integrate(
        tmp.path(),
        "run-mhr",
        Join::All,
        Integrate::Merge,
        &children,
    );
    let ctx = par_ctx(db.clone(), MatrixDriver::new(ws));
    drive_run(&ctx, "run-mhr").await;
    assert_eq!(run_status_str(&db, "run-mhr"), "paused");

    // Human "resolves" by renaming the conflicted file — its markers ride
    // along to the new path, and the old path disappears.
    let int_wt = tmp.path().join("rd-run-mhr").join("integrate-0");
    sh(&int_wt, &["mv", CONFLICT_FILE, "renamed.txt"]);
    sh(&int_wt, &["commit", "-qm", "rename instead of resolving"]);
    set_resolution(&db, "run-mhr", "human");
    drive_run(&ctx, "run-mhr").await;

    assert_eq!(run_status_str(&db, "run-mhr"), "paused");
    let cur = get_cursor(&db.lock(), "run-mhr");
    assert!(
        cur.merge
            .and_then(|m| m.conflict)
            .and_then(|c| c.resolution)
            .is_none(),
        "the renamed marker file is detected — resolution cleared"
    );
}

// ───────────────────────────── loop blocks (S7) ─────────────────────────

/// A real-git stub whose "agent" writes a configured `verdict.json` into the
/// until-step's blackboard dir each turn (instead of committing code) — the
/// verdict-gated shape a loop's exit step needs. Spawns a real `--shared`
/// clone forking from the run repo so a `done` verdict still ferries.
struct VerdictStub {
    root: PathBuf,
    blackboard: PathBuf,
    step_id: String,
    verdict: String,
    /// When true, each turn also makes a commit in the agent's workspace so a
    /// `commit`-gated body step (e.g. `fix`) advances HEAD. A verdict-gated
    /// `until` step ignores its own commit (that attempt never ferries).
    commit: bool,
    tx: broadcast::Sender<StatusEvent>,
    state: parking_lot::Mutex<StubState>,
}
impl VerdictStub {
    fn new(
        root: PathBuf,
        blackboard: PathBuf,
        step_id: &str,
        verdict: &str,
        commit: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            root,
            blackboard,
            step_id: step_id.to_string(),
            verdict: verdict.to_string(),
            commit,
            tx: broadcast::channel(256).0,
            state: parking_lot::Mutex::new(StubState::default()),
        })
    }
    fn set(&self, id: &str, s: AgentStatus) {
        self.state.lock().statuses.insert(id.to_string(), s.clone());
        let _ = self.tx.send(StatusEvent {
            agent_id: id.to_string(),
            status: s,
        });
    }
}
impl AgentDriver for VerdictStub {
    fn spawn(
        &self,
        req: SpawnReq,
    ) -> super::super::driver::BoxFuture<'_, Result<super::super::driver::SpawnedAgent>> {
        Box::pin(async move {
            let id = {
                let mut st = self.state.lock();
                st.count += 1;
                format!("stub-{}", st.count)
            };
            let dest = self.root.join(&id);
            let base_ref = req.fork_base.clone().unwrap();
            let spec = crate::sandbox::provision::CheckoutSpec {
                source_repo: &req.repo_path,
                base_ref: &base_ref,
                dest: &dest,
            };
            crate::sandbox::provision::provision_forking_run_repo(
                &spec,
                req.run_repo.as_ref().unwrap(),
            )
            .await?;
            self.state.lock().worktrees.insert(id.clone(), dest.clone());
            self.set(&id, AgentStatus::Idle);
            Ok(super::super::driver::SpawnedAgent {
                agent_id: id,
                worktree: dest,
            })
        })
    }
    fn status(&self, id: &str) -> Option<AgentStatus> {
        self.state.lock().statuses.get(id).cloned()
    }
    fn subscribe(&self) -> broadcast::Receiver<StatusEvent> {
        self.tx.subscribe()
    }
    fn send_message<'a>(
        &'a self,
        id: &'a str,
        _text: String,
    ) -> super::super::driver::BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.set(id, AgentStatus::Running);
            // Written after the attempt has subscribed and archived any stale
            // verdict — the ordering the real supervisor produces.
            let dir = blackboard::step_dir(&self.blackboard, &self.step_id).unwrap();
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("verdict.json"), &self.verdict).unwrap();
            if self.commit {
                let wt = self.state.lock().worktrees.get(id).cloned().unwrap();
                sh(&wt, &["config", "user.email", "t@t.t"]);
                sh(&wt, &["config", "user.name", "t"]);
                std::fs::write(wt.join(format!("{id}.txt")), "work").unwrap();
                sh(&wt, &["add", "-A"]);
                sh(&wt, &["commit", "-qm", "agent work"]);
            }
            self.set(id, AgentStatus::Idle);
            Ok(())
        })
    }
    fn stop<'a>(&'a self, _id: &'a str) -> super::super::driver::BoxFuture<'a, Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn archive<'a>(&'a self, _id: &'a str) -> super::super::driver::BoxFuture<'a, Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn last_activity(&self, _id: &str) -> Option<i64> {
        None
    }
    fn turn_usage(&self, _id: &str) -> Option<super::super::driver::TurnUsage> {
        None
    }
}

/// Scaffold a run whose whole workflow is one loop with a verdict-gated
/// `review` exit step. With `with_fix`, a commit-gated `fix` step follows it
/// in the body (the canonical `[review, fix]` shape) so tests can assert what
/// happens to a body step *after* the `until` step. Returns the db, the
/// workspaces root the stub provisions under, and the blackboard dir.
fn scaffold_loop(
    tmp: &Path,
    run_id: &str,
    branch: &str,
    max: u32,
    with_fix: bool,
) -> (Db, PathBuf, PathBuf) {
    let source = tmp.join("source");
    std::fs::create_dir_all(&source).unwrap();
    sh(&source, &["init", "-q", "-b", "main"]);
    sh(&source, &["config", "user.email", "t@t.t"]);
    sh(&source, &["config", "user.name", "t"]);
    std::fs::write(source.join("README"), "base").unwrap();
    sh(&source, &["add", "-A"]);
    sh(&source, &["commit", "-qm", "base"]);
    let base_sha = {
        let o = Sh::new("git")
            .current_dir(&source)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    let run_dir = tmp.join("rundir");
    let blackboard = blackboard::blackboard_dir(&run_dir);
    std::fs::create_dir_all(&blackboard).unwrap();

    let mut agents = BTreeMap::new();
    agents.insert(
        "coder".to_string(),
        super::super::spec::AgentSpec {
            base: "codex".to_string(),
            model: None,
            effort: None,
            instructions: None,
            skills: vec![],
            mcp_servers: vec![],
            custom_agent: None,
        },
    );
    let review = Step {
        id: "review".to_string(),
        agent: "coder".to_string(),
        goal: "review the work".to_string(),
        gate: Gate::Verdict,
        budgets: None,
        comms: vec![],
    };
    let mut body = vec![Block::Step(review)];
    if with_fix {
        body.push(Block::Step(Step {
            id: "fix".to_string(),
            agent: "coder".to_string(),
            goal: "address the feedback".to_string(),
            gate: Gate::Commit,
            budgets: None,
            comms: vec![],
        }));
    }
    let spec = Spec {
        version: 1,
        name: "demo".to_string(),
        description: None,
        budgets: None,
        agents,
        workflow: vec![Block::Loop(Loop {
            max,
            until: super::super::spec::Until {
                step: "review".to_string(),
                verdict: super::super::spec::LoopVerdict::Done,
            },
            body,
        })],
        finalize: None,
    };
    let spec_json = serde_json::to_string(&spec).unwrap();
    let db = crate::database::init(tmp).unwrap();
    db.lock()
        .execute(
            "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                    base_sha,status,budgets_json,spent_json,created_at,updated_at)
                 VALUES (?1,'demo',?2,'t','p',?3,?4,?5,?6,'pending','{}','{}',0,0)",
            rusqlite::params![
                run_id,
                spec_json,
                source.to_string_lossy(),
                run_dir.to_string_lossy(),
                branch,
                base_sha,
            ],
        )
        .unwrap();
    (db, tmp.join("ws"), blackboard)
}

fn count(db: &Db, sql: &str) -> i64 {
    db.lock().query_row(sql, [], |r| r.get(0)).unwrap()
}

fn loop_ctx(db: Db, driver: Arc<VerdictStub>) -> RunCtx {
    RunCtx {
        db,
        driver,
        app: None,
        cancel: Arc::new(AtomicBool::new(false)),
        pending_ask: Arc::new(AtomicBool::new(false)),
        deadlines: Deadlines::default(),
        runs: None,
    }
}

#[tokio::test]
async fn loop_exits_on_first_done_verdict() {
    let tmp = tempfile::tempdir().unwrap();
    let (db, ws, bb) = scaffold_loop(tmp.path(), "run-loop-done", "wf/ld-1", 3, false);
    let driver = VerdictStub::new(
        ws,
        bb,
        "review",
        r#"{"result":"done","summary":"lgtm"}"#,
        false,
    );
    drive_run(&loop_ctx(db.clone(), driver), "run-loop-done").await;

    // Exactly one iteration ran (iteration 0), its review is done, the loop
    // never hit its max, and the run completed.
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-loop-done'"
        ),
        1,
        "one review attempt only"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-loop-done' \
                 AND iteration=0 AND status='done'"
        ),
        1
    );
    assert_eq!(
        count(
            &db,
            &format!(
                "SELECT COUNT(*) FROM wf_event WHERE run_id='run-loop-done' AND type='{}'",
                event_type::LOOP_MAX_REACHED
            )
        ),
        0,
        "loop_max_reached must NOT fire on an early done"
    );
    assert_eq!(run_status_str(&db, "run-loop-done"), "done");
}

#[tokio::test]
async fn loop_revises_until_max_then_continues() {
    let tmp = tempfile::tempdir().unwrap();
    let (db, ws, bb) = scaffold_loop(tmp.path(), "run-loop-max", "wf/lm-1", 3, false);
    // "revise" every turn → the loop runs all `max` iterations, then continues
    // (exhaustion is not failure — spec §6.6).
    let driver = VerdictStub::new(
        ws,
        bb,
        "review",
        r#"{"result":"revise","summary":"again"}"#,
        false,
    );
    drive_run(&loop_ctx(db.clone(), driver), "run-loop-max").await;

    // One blocked review per iteration, at iterations 0..3.
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-loop-max' AND status='blocked'"
        ),
        3
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(DISTINCT iteration) FROM wf_step_exec WHERE run_id='run-loop-max'"
        ),
        3,
        "iterations 0,1,2"
    );
    assert_eq!(
        count(
            &db,
            &format!(
                "SELECT COUNT(*) FROM wf_event WHERE run_id='run-loop-max' AND type='{}'",
                event_type::LOOP_MAX_REACHED
            )
        ),
        1
    );
    assert_eq!(
        run_status_str(&db, "run-loop-max"),
        "done",
        "loop exhaustion continues to done"
    );
}

#[tokio::test]
async fn resume_mid_loop_restores_the_iteration_counter() {
    let tmp = tempfile::tempdir().unwrap();
    let (db, ws, bb) = scaffold_loop(tmp.path(), "run-loop-resume", "wf/lr-1", 3, false);
    // A prior driver died during iteration 1: the cursor records it and a
    // non-terminal attempt is left behind. Resume must pick up at iteration 1
    // (not restart at 0) and run only iterations 1 and 2 before max.
    {
        let conn = db.lock();
        conn.execute(
            "UPDATE wf_run SET cursor_json=?1 WHERE id='run-loop-resume'",
            [r#"{"index":0,"iterations":{"0":1}}"#],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wf_step_exec (id, run_id, step_id, attempt, iteration, status,
                    gate_mode, agent_id)
                 VALUES ('exec-stale','run-loop-resume','review',1,1,'running','verdict','ghost')",
            [],
        )
        .unwrap();
    }
    let driver = VerdictStub::new(
        ws,
        bb,
        "review",
        r#"{"result":"revise","summary":"again"}"#,
        false,
    );
    drive_run(&loop_ctx(db.clone(), driver), "run-loop-resume").await;

    // The counter was restored: nothing ran at iteration 0.
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-loop-resume' AND iteration=0"
        ),
        0,
        "resume must not restart the loop at iteration 0"
    );
    // The stale attempt was abandoned; fresh reviews ran at iterations 1 and 2.
    let stale: String = db
        .lock()
        .query_row(
            "SELECT status FROM wf_step_exec WHERE id='exec-stale'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stale, "abandoned");
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-loop-resume' \
                 AND iteration=2 AND status='blocked'"
        ),
        1,
        "the final iteration ran"
    );
    assert_eq!(
        count(
            &db,
            &format!(
                "SELECT COUNT(*) FROM wf_event WHERE run_id='run-loop-resume' AND type='{}'",
                event_type::LOOP_MAX_REACHED
            )
        ),
        1
    );
    assert_eq!(run_status_str(&db, "run-loop-resume"), "done");
}

/// `until` not last (§6.6): with body `[review, fix]` and a `revise` review
/// each iteration, the trailing `fix` runs *within the same iteration* before
/// the loop restarts — the remaining body is the remediation for a non-`done`
/// verdict, not something to skip.
#[tokio::test]
async fn loop_runs_trailing_body_after_a_revise() {
    let tmp = tempfile::tempdir().unwrap();
    let (db, ws, bb) = scaffold_loop(tmp.path(), "run-loop-fix", "wf/lf-1", 2, true);
    let driver = VerdictStub::new(
        ws,
        bb,
        "review",
        r#"{"result":"revise","summary":"again"}"#,
        true,
    );
    drive_run(&loop_ctx(db.clone(), driver), "run-loop-fix").await;

    // `fix` ran and completed in BOTH iterations (0 and 1) — a revise does not
    // short-circuit the rest of the body.
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-loop-fix' \
                 AND step_id='fix' AND status='done'"
        ),
        2,
        "fix runs once per revise iteration"
    );
    assert_eq!(
        count(
            &db,
            &format!(
                "SELECT COUNT(*) FROM wf_event WHERE run_id='run-loop-fix' AND type='{}'",
                event_type::LOOP_MAX_REACHED
            )
        ),
        1
    );
    assert_eq!(run_status_str(&db, "run-loop-fix"), "done");
}

/// `until` not last (§6.6): a `done` review exits the loop *immediately*,
/// skipping the trailing `fix` — there is nothing to remediate.
#[tokio::test]
async fn loop_skips_trailing_body_on_done() {
    let tmp = tempfile::tempdir().unwrap();
    let (db, ws, bb) = scaffold_loop(tmp.path(), "run-loop-skip", "wf/ls-1", 3, true);
    let driver = VerdictStub::new(
        ws,
        bb,
        "review",
        r#"{"result":"done","summary":"lgtm"}"#,
        true,
    );
    drive_run(&loop_ctx(db.clone(), driver), "run-loop-skip").await;

    // review is done at iteration 0 → the loop exits and `fix` never spawns.
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-loop-skip' AND step_id='fix'"
        ),
        0,
        "a done review skips the trailing fix"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-loop-skip' \
                 AND step_id='review' AND status='done'"
        ),
        1
    );
    assert_eq!(run_status_str(&db, "run-loop-skip"), "done");
}

// ─────────────────────── orchestrate stages (S11) ───────────────────────

#[derive(Clone, Copy, PartialEq)]
enum OrchMode {
    /// Writes a `done` verdict on the concluding prompt.
    Conclude,
    /// Never writes a verdict — the stage should gate on it and pause.
    NeverConclude,
    /// Stalls its turn forever — the engine escalates to the human.
    Stall,
}

/// A real-git stub for orchestrate stages (spec §10.2). Children commit (their
/// `commit` gate); the orchestrator writes its concluding `verdict.json` on the
/// conclude prompt, never writes one, or stalls — per [`OrchMode`]. Roles are
/// told apart by the prompt text the engine composes.
struct OrchDriver {
    root: PathBuf,
    blackboard: PathBuf,
    mode: OrchMode,
    /// When set, a `wf_decide` body the orchestrator "issues" on its first turn
    /// (persisted as a queued decision the way the router would), plus the DB
    /// and run id needed to write it. Lets a test script skip/retry decisions.
    first_decision: Option<(Db, String, serde_json::Value)>,
    tx: broadcast::Sender<StatusEvent>,
    state: parking_lot::Mutex<StubState>,
}
impl OrchDriver {
    fn new(root: PathBuf, blackboard: PathBuf, mode: OrchMode) -> Arc<Self> {
        Arc::new(Self {
            root,
            blackboard,
            mode,
            first_decision: None,
            tx: broadcast::channel(256).0,
            state: parking_lot::Mutex::new(StubState::default()),
        })
    }
    fn new_scripted(
        root: PathBuf,
        blackboard: PathBuf,
        mode: OrchMode,
        db: Db,
        run_id: &str,
        decision: serde_json::Value,
    ) -> Arc<Self> {
        Arc::new(Self {
            root,
            blackboard,
            mode,
            first_decision: Some((db, run_id.to_string(), decision)),
            tx: broadcast::channel(256).0,
            state: parking_lot::Mutex::new(StubState::default()),
        })
    }
    fn set(&self, id: &str, s: AgentStatus) {
        self.state.lock().statuses.insert(id.to_string(), s.clone());
        let _ = self.tx.send(StatusEvent {
            agent_id: id.to_string(),
            status: s,
        });
    }
    /// Persist `decision` as a queued `decision` message from the orchestrator
    /// exec — exactly what `route_decide` does when the orchestrator calls
    /// `wf_decide` (its exec is resolved by `agent_id`, stamped at spawn).
    fn inject_decision(&self, orch_agent_id: &str) {
        let Some((db, run_id, body)) = &self.first_decision else {
            return;
        };
        let conn = db.lock();
        let exec: Option<String> = conn
            .query_row(
                "SELECT id FROM wf_step_exec WHERE run_id = ?1 AND agent_id = ?2",
                rusqlite::params![run_id, orch_agent_id],
                |r| r.get(0),
            )
            .ok();
        if let Some(exec) = exec {
            conn.execute(
                "INSERT INTO wf_message (id, run_id, from_step_exec_id, to_step_exec_id,
                        kind, body_json, status, created_at)
                     VALUES (?1, ?2, ?3, NULL, 'decision', ?4, 'queued', 0)",
                rusqlite::params![
                    format!("dec-{}", uuid::Uuid::new_v4()),
                    run_id,
                    exec,
                    body.to_string(),
                ],
            )
            .unwrap();
        }
    }
}
impl AgentDriver for OrchDriver {
    fn spawn(
        &self,
        req: SpawnReq,
    ) -> super::super::driver::BoxFuture<'_, Result<super::super::driver::SpawnedAgent>> {
        Box::pin(async move {
            let id = {
                let mut st = self.state.lock();
                st.count += 1;
                format!("o-{}", st.count)
            };
            let dest = self.root.join(&id);
            let base_ref = req.fork_base.clone().unwrap();
            let spec = crate::sandbox::provision::CheckoutSpec {
                source_repo: &req.repo_path,
                base_ref: &base_ref,
                dest: &dest,
            };
            crate::sandbox::provision::provision_forking_run_repo(
                &spec,
                req.run_repo.as_ref().unwrap(),
            )
            .await?;
            self.state.lock().worktrees.insert(id.clone(), dest.clone());
            self.set(&id, AgentStatus::Idle);
            Ok(super::super::driver::SpawnedAgent {
                agent_id: id,
                worktree: dest,
            })
        })
    }
    fn status(&self, id: &str) -> Option<AgentStatus> {
        self.state.lock().statuses.get(id).cloned()
    }
    fn subscribe(&self) -> broadcast::Receiver<StatusEvent> {
        self.tx.subscribe()
    }
    fn send_message<'a>(
        &'a self,
        id: &'a str,
        text: String,
    ) -> super::super::driver::BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let is_initial = text.contains("Workflow orchestrator");
            let is_orch = is_initial
                || text.contains("All children are done")
                || text.contains("Updates from your children");
            let is_nudge = text.contains("gone quiet");
            self.set(id, AgentStatus::Running);
            if is_nudge {
                // Keep the (stalling) turn running so the watchdog escalates.
                return Ok(());
            }
            if is_orch {
                // Issue any scripted decision on the opening turn.
                if is_initial {
                    self.inject_decision(id);
                }
                match self.mode {
                    OrchMode::Stall => return Ok(()), // never goes Idle → stall
                    OrchMode::Conclude => {
                        if text.contains("All children are done") {
                            let dir = self.blackboard.join("orchestrate-0");
                            std::fs::create_dir_all(&dir).unwrap();
                            std::fs::write(
                                dir.join("verdict.json"),
                                r#"{"result":"done","summary":"concluded"}"#,
                            )
                            .unwrap();
                        }
                    }
                    OrchMode::NeverConclude => {}
                }
                self.set(id, AgentStatus::Idle);
            } else if text.contains("HANGCHILD") {
                // A child that never finishes its turn — only a cancel (e.g.
                // `skip_child`) can wind it down.
                return Ok(());
            } else {
                // A child: satisfy its `commit` gate.
                let wt = self.state.lock().worktrees.get(id).cloned().unwrap();
                sh(&wt, &["config", "user.email", "t@t.t"]);
                sh(&wt, &["config", "user.name", "t"]);
                std::fs::write(wt.join(format!("{id}.txt")), "work").unwrap();
                sh(&wt, &["add", "-A"]);
                sh(&wt, &["commit", "-qm", "child work"]);
                self.set(id, AgentStatus::Idle);
            }
            Ok(())
        })
    }
    fn stop<'a>(&'a self, _id: &'a str) -> super::super::driver::BoxFuture<'a, Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn archive<'a>(&'a self, _id: &'a str) -> super::super::driver::BoxFuture<'a, Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn last_activity(&self, _id: &str) -> Option<i64> {
        None
    }
    fn turn_usage(&self, _id: &str) -> Option<super::super::driver::TurnUsage> {
        None
    }
}

/// A run whose whole workflow is one orchestrate block (agent `orch`) with a
/// single static, commit-gated child `impl` whose goal is `child_goal` (use
/// `HANGCHILD` for a child that never finishes its turn). Returns the db, the
/// workspace root, the blackboard dir, and the base SHA.
fn scaffold_orchestrate(
    tmp: &Path,
    run_id: &str,
    child_goal: &str,
) -> (Db, PathBuf, PathBuf, String) {
    let source = tmp.join("source");
    std::fs::create_dir_all(&source).unwrap();
    sh(&source, &["init", "-q", "-b", "main"]);
    sh(&source, &["config", "user.email", "t@t.t"]);
    sh(&source, &["config", "user.name", "t"]);
    std::fs::write(source.join("README"), "base").unwrap();
    sh(&source, &["add", "-A"]);
    sh(&source, &["commit", "-qm", "base"]);
    let base_sha = {
        let o = Sh::new("git")
            .current_dir(&source)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    let run_dir = tmp.join("rundir");
    let blackboard = blackboard::blackboard_dir(&run_dir);
    std::fs::create_dir_all(&blackboard).unwrap();

    let mut agents = BTreeMap::new();
    for a in ["orch", "coder"] {
        agents.insert(
            a.to_string(),
            super::super::spec::AgentSpec {
                base: "codex".to_string(),
                model: None,
                effort: None,
                instructions: None,
                skills: vec![],
                mcp_servers: vec![],
                custom_agent: None,
            },
        );
    }
    let child = Step {
        id: "impl".to_string(),
        agent: "coder".to_string(),
        goal: child_goal.to_string(),
        gate: Gate::Commit,
        budgets: None,
        comms: vec![],
    };
    let spec = Spec {
        version: 1,
        name: "orch".to_string(),
        description: None,
        // Short stall/nudge so the stall test escalates in ~2s of real time
        // (no `start_paused` — the real-git provisioning needs the IO reactor).
        // Harmless to the non-stalling tests: their turns end before any tick.
        budgets: Some(Budgets {
            turns: None,
            tokens: None,
            wall_clock_mins: None,
            turns_per_attempt: None,
            max_attempts: None,
            spawn_timeout_secs: None,
            turn_start_timeout_secs: None,
            stall_timeout_secs: Some(1),
            nudge_timeout_secs: Some(1),
            tests_timeout_secs: None,
        }),
        agents,
        workflow: vec![Block::Orchestrate(Orchestrate {
            agent: "orch".to_string(),
            goal: "lead the stage".to_string(),
            children: None,
            body: vec![child],
            join: Join::All,
            integrate: Integrate::None,
            comms: vec![],
            compose: None,
        })],
        finalize: None,
    };
    let spec_json = serde_json::to_string(&spec).unwrap();
    // Freeze the effective budgets from the spec so the short stall/nudge
    // actually take effect (a bare '{}' would deserialize to the defaults).
    let budgets_json = serde_json::to_string(&EffectiveBudgets::resolve(&spec)).unwrap();
    let db = crate::database::init(tmp).unwrap();
    db.lock()
        .execute(
            "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                    base_sha,status,budgets_json,spent_json,created_at,updated_at)
                 VALUES (?1,'orch',?2,'t','p',?3,?4,'wf/orch-x',?5,'pending',?6,'{}',0,0)",
            rusqlite::params![
                run_id,
                spec_json,
                source.to_string_lossy(),
                run_dir.to_string_lossy(),
                base_sha,
                budgets_json,
            ],
        )
        .unwrap();
    (db, tmp.join("ws"), blackboard, base_sha)
}

fn orch_ctx(db: Db, driver: Arc<OrchDriver>) -> RunCtx {
    RunCtx {
        db,
        driver,
        app: None,
        cancel: Arc::new(AtomicBool::new(false)),
        pending_ask: Arc::new(AtomicBool::new(false)),
        // Tick the stall watchdog fast so the stall test resolves quickly.
        deadlines: Deadlines {
            watchdog_tick: std::time::Duration::from_millis(100),
            ..Deadlines::default()
        },
        runs: None,
    }
}

#[tokio::test]
async fn orchestrate_concludes_after_children_and_reaches_done() {
    let tmp = tempfile::tempdir().unwrap();
    let (db, ws, bb, _base) = scaffold_orchestrate(tmp.path(), "run-orch", "implement the slice");
    let ctx = orch_ctx(db.clone(), OrchDriver::new(ws, bb, OrchMode::Conclude));
    drive_run(&ctx, "run-orch").await;

    assert_eq!(run_status_str(&db, "run-orch"), "done");
    // The child ran, and the orchestrator concluded — both terminal `done`.
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-orch' \
                 AND step_id='impl' AND status='done'"
        ),
        1,
        "the child completed"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-orch' \
                 AND step_id='orchestrate-0' AND status='done'"
        ),
        1,
        "the orchestrator concluded"
    );
    // The stage gate is the orchestrator's own verdict.
    let concluded = count(
        &db,
        "SELECT COUNT(*) FROM wf_event WHERE run_id='run-orch' AND type='gate_evaluated' \
             AND json_extract(payload_json,'$.outcome')='done' \
             AND step_exec_id IN (SELECT id FROM wf_step_exec WHERE step_id='orchestrate-0')",
    );
    assert!(
        concluded >= 1,
        "orchestrator's concluding verdict gated the stage"
    );
}

#[tokio::test]
async fn orchestrate_pauses_blocked_when_the_orchestrator_never_concludes() {
    let tmp = tempfile::tempdir().unwrap();
    let (db, ws, bb, _base) = scaffold_orchestrate(tmp.path(), "run-nc", "implement the slice");
    let ctx = orch_ctx(db.clone(), OrchDriver::new(ws, bb, OrchMode::NeverConclude));
    drive_run(&ctx, "run-nc").await;

    // The child finished, but the stage does NOT complete without the
    // orchestrator's concluding verdict — it pauses `blocked_gate` (§6.6).
    let (status, reason): (String, Option<String>) = db
        .lock()
        .query_row(
            "SELECT status, paused_reason FROM wf_run WHERE id='run-nc'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "paused");
    assert_eq!(reason.as_deref(), Some("blocked_gate"));
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-nc' \
                 AND step_id='impl' AND status='done'"
        ),
        1,
        "the child still ran to completion"
    );
}

#[tokio::test]
async fn orchestrator_stall_escalates_to_the_human() {
    let tmp = tempfile::tempdir().unwrap();
    let (db, ws, bb, _base) = scaffold_orchestrate(tmp.path(), "run-stall", "implement the slice");
    let ctx = orch_ctx(db.clone(), OrchDriver::new(ws, bb, OrchMode::Stall));
    drive_run(&ctx, "run-stall").await;

    // A stalled orchestrator does not hang the stage — the engine escalates to
    // the human, pausing the run `question` (§10.2).
    let (status, reason, error): (String, Option<String>, Option<String>) = db
        .lock()
        .query_row(
            "SELECT status, paused_reason, error FROM wf_run WHERE id='run-stall'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, "paused", "run error: {error:?}");
    assert_eq!(reason.as_deref(), Some("question"));
    let stalled = count(
        &db,
        "SELECT COUNT(*) FROM wf_event WHERE run_id='run-stall' AND type='watchdog_stalled'",
    );
    assert!(stalled >= 1, "the orchestrator stall was journaled");
}

#[tokio::test]
async fn resume_does_not_rerun_a_completed_static_child() {
    let tmp = tempfile::tempdir().unwrap();
    let (db, ws, bb, _base) = scaffold_orchestrate(tmp.path(), "run-resume", "implement the slice");
    // Simulate a prior drive that paused before the orchestrator concluded: the
    // static child already finished `done`.
    db.lock()
            .execute(
                "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode,agent_id)
                 VALUES ('impl-prior','run-resume','impl',1,0,'done','commit','prior-agent')",
                [],
            )
            .unwrap();
    let ctx = orch_ctx(db.clone(), OrchDriver::new(ws, bb, OrchMode::Conclude));
    drive_run(&ctx, "run-resume").await;

    assert_eq!(run_status_str(&db, "run-resume"), "done");
    // The already-done child is not executed a second time (§12.3 parity).
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-resume' AND step_id='impl'"
        ),
        1,
        "the completed child must not re-run on resume"
    );
}

#[test]
fn dyn_child_index_is_seeded_from_existing_execs() {
    let tmp = tempfile::tempdir().unwrap();
    let db = crate::database::init(tmp.path()).unwrap();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                base_sha,status,budgets_json,spent_json,created_at,updated_at)
             VALUES ('r','n','{}','t','p','/r','/d','wf/x','sha','running','{}','{}',0,0)",
        [],
    )
    .unwrap();
    assert_eq!(existing_dyn_child_count(&conn, "r", "orchestrate-0"), 0);
    for k in 0..2 {
        conn.execute(
            "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode)
                 VALUES (?1,'r',?2,1,0,'done','verdict')",
            rusqlite::params![format!("e{k}"), format!("orchestrate-0::dyn-{k}")],
        )
        .unwrap();
    }
    // A non-dynamic child and a different stage's children must not count.
    conn.execute(
        "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode)
             VALUES ('x','r','impl',1,0,'done','commit')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode)
             VALUES ('y','r','orchestrate-1::dyn-0',1,0,'done','verdict')",
        [],
    )
    .unwrap();
    assert_eq!(
        existing_dyn_child_count(&conn, "r", "orchestrate-0"),
        2,
        "the next dynamic index skips the two already created"
    );
}

#[tokio::test]
async fn skip_child_cancels_the_child_so_the_stage_can_conclude() {
    let tmp = tempfile::tempdir().unwrap();
    // The child hangs its turn — only a cancel ends it. On its opening turn the
    // orchestrator issues `skip_child`; the engine must cancel that child (not
    // let it stall out) so the stage concludes on the orchestrator's verdict.
    let (db, ws, bb, _base) = scaffold_orchestrate(tmp.path(), "run-skip", "implement HANGCHILD");
    let driver = OrchDriver::new_scripted(
        ws,
        bb,
        OrchMode::Conclude,
        db.clone(),
        "run-skip",
        serde_json::json!({ "decision": "skip_child", "step_id": "impl", "reason": "unneeded" }),
    );
    drive_run(&orch_ctx(db.clone(), driver), "run-skip").await;

    assert_eq!(run_status_str(&db, "run-skip"), "done");
    // The child was cancelled (abandoned), not left to stall out (`error`) or
    // to complete (`done`).
    let bad = count(
        &db,
        "SELECT COUNT(*) FROM wf_step_exec WHERE run_id='run-skip' AND step_id='impl' \
             AND status IN ('error','done')",
    );
    assert_eq!(
        bad, 0,
        "skip_child must cancel the child, not let it stall or finish"
    );
}

#[test]
fn stale_retry_result_is_discarded_by_generation() {
    // A superseded attempt (older generation) that still finishes must not
    // decide the join; only the current generation's result counts (§10.2).
    let tmp = tempfile::tempdir().unwrap();
    let db = crate::database::init(tmp.path()).unwrap();
    db.lock()
        .execute(
            "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                    base_sha,status,budgets_json,spent_json,created_at,updated_at)
                 VALUES ('r','n','{}','t','p','/r','/d','wf/x','sha','running','{}','{}',0,0)",
            [],
        )
        .unwrap();
    for id in ["orch-exec", "c-old", "c-new"] {
        db.lock()
            .execute(
                "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode)
                     VALUES (?1,'r','impl',1,0,'abandoned','verdict')",
                [id],
            )
            .unwrap();
    }
    let ctx = RunCtx {
        db: db.clone(),
        driver: StubDriver::new(tmp.path().join("ws"), true),
        app: None,
        cancel: Arc::new(AtomicBool::new(false)),
        pending_ask: Arc::new(AtomicBool::new(false)),
        deadlines: Deadlines::default(),
        runs: None,
    };
    let mut ledger = Ledger::default();
    let mut outcomes: HashMap<String, ChildStatus> = HashMap::new();
    let child_cancels: HashMap<String, Arc<AtomicBool>> = HashMap::new();
    // Current generation for `impl` is 1 (a retry superseded generation 0).
    let mut child_gen: HashMap<String, u64> = HashMap::new();
    child_gen.insert("impl".to_string(), 1);

    let result = |exec: &str, generation: u64| OrchChildResult {
        step_id: "impl".to_string(),
        exec_id: exec.to_string(),
        generation,
        outcome: ChildOutcome::Success {
            moved_head: false,
            head: None,
        },
        ledger: Ledger::default(),
    };

    // A stale (generation 0) success is ignored — records no join outcome.
    handle_orch_child(
        &ctx,
        "r",
        "orch-exec",
        Join::Any,
        &mut ledger,
        Ok(result("c-old", 0)),
        &mut outcomes,
        &child_cancels,
        &child_gen,
    );
    assert!(
        !outcomes.contains_key("impl"),
        "a superseded attempt must not decide the join"
    );

    // The current-generation result records the outcome.
    handle_orch_child(
        &ctx,
        "r",
        "orch-exec",
        Join::Any,
        &mut ledger,
        Ok(result("c-new", 1)),
        &mut outcomes,
        &child_cancels,
        &child_gen,
    );
    assert!(matches!(outcomes.get("impl"), Some(ChildStatus::Success)));
}

#[test]
fn prior_spawn_decisions_rebuild_dynamic_children_in_order() {
    // On resume, the dynamic children of a prior drive are rebuilt from their
    // persisted spawn decisions (agent + goal), in spawn order, so retry_child
    // can still target `orchestrate-0::dyn-K`.
    let tmp = tempfile::tempdir().unwrap();
    let db = crate::database::init(tmp.path()).unwrap();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                base_sha,status,budgets_json,spent_json,created_at,updated_at)
             VALUES ('r','n','{}','t','p','/r','/d','wf/x','sha','running','{}','{}',0,0)",
        [],
    )
    .unwrap();
    // A prior orchestrator exec for the stage.
    conn.execute(
        "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode,agent_id)
             VALUES ('orch-old','r','orchestrate-0',1,0,'abandoned','verdict','a-old')",
        [],
    )
    .unwrap();
    // Two spawn decisions (ordered) + one unrelated decision that must be
    // excluded.
    let insert_msg = |id: &str, body: &str, ts: i64| {
        conn.execute(
            "INSERT INTO wf_message (id,run_id,from_step_exec_id,to_step_exec_id,kind,
                    body_json,status,created_at)
                 VALUES (?1,'r','orch-old',NULL,'decision',?2,'delivered',?3)",
            rusqlite::params![id, body, ts],
        )
        .unwrap();
    };
    insert_msg(
        "d0",
        r#"{"decision":"spawn_child","agent":"coder","goal":"slice A"}"#,
        1,
    );
    insert_msg(
        "d1",
        r#"{"decision":"spawn_child","agent":"coder","goal":"slice B"}"#,
        2,
    );
    insert_msg("d2", r#"{"decision":"stage_done"}"#, 3);

    let got = prior_spawn_decisions(&conn, "r", "orchestrate-0");
    assert_eq!(
        got,
        vec![
            ("coder".to_string(), "slice A".to_string()),
            ("coder".to_string(), "slice B".to_string()),
        ],
        "spawn decisions rebuild in order, excluding non-spawn decisions"
    );
}

#[test]
fn restored_child_status_maps_only_terminal_execs() {
    assert!(matches!(
        restored_child_status("done"),
        Some(ChildStatus::Success)
    ));
    assert!(matches!(
        restored_child_status("error"),
        Some(ChildStatus::Failure(_))
    ));
    assert!(matches!(
        restored_child_status("blocked"),
        Some(ChildStatus::Failure(_))
    ));
    // In-flight / superseded attempts are not a restorable join outcome.
    assert!(restored_child_status("abandoned").is_none());
    assert!(restored_child_status("running").is_none());
}

#[tokio::test]
async fn resume_restores_a_failed_dynamic_childs_join_outcome() {
    // A dynamic child that failed in a prior drive must still count in the
    // join on resume — otherwise a join:all stage could wrongly conclude
    // `done` from incomplete outcomes.
    let tmp = tempfile::tempdir().unwrap();
    let (db, ws, bb, _base) = scaffold_orchestrate(tmp.path(), "run-djoin", "implement the slice");
    // Simulate a prior drive: the orchestrator spawned dyn-0, which failed.
    db.lock()
            .execute(
                "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode,agent_id)
                 VALUES ('orch-old','run-djoin','orchestrate-0',1,0,'abandoned','verdict','a-old')",
                [],
            )
            .unwrap();
    db.lock()
        .execute(
            "INSERT INTO wf_message (id,run_id,from_step_exec_id,to_step_exec_id,kind,
                    body_json,status,created_at)
                 VALUES ('sp0','run-djoin','orch-old',NULL,'decision',
                    '{\"decision\":\"spawn_child\",\"agent\":\"coder\",\"goal\":\"slice\"}',
                    'delivered',1)",
            [],
        )
        .unwrap();
    db.lock()
        .execute(
            "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode)
                 VALUES ('dyn0','run-djoin','orchestrate-0::dyn-0',1,0,'error','verdict')",
            [],
        )
        .unwrap();

    let ctx = orch_ctx(db.clone(), OrchDriver::new(ws, bb, OrchMode::Conclude));
    drive_run(&ctx, "run-djoin").await;

    let (status, err): (String, Option<String>) = db
        .lock()
        .query_row(
            "SELECT status, error FROM wf_run WHERE id='run-djoin'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        status, "failed",
        "the restored dynamic-child failure must decide the join:all stage"
    );
    assert!(err.unwrap_or_default().contains("orchestrate stage failed"));
}

// ───────────────────────── dynamic composition (S12, §10.3) ─────────────

#[test]
fn child_run_ids_lists_only_direct_sub_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let db = crate::database::init(tmp.path()).unwrap();
    let conn = db.lock();
    for (id, parent) in [
        ("p", None),
        ("c1", Some("p")),
        ("c2", Some("p")),
        ("other", None),
    ] {
        conn.execute(
            "INSERT INTO wf_run (id,parent_run_id,name,spec_json,task,project_id,repo_path,
                    run_dir,branch,base_sha,status,budgets_json,spent_json,created_at,updated_at)
                 VALUES (?1,?2,'n','{}','t','p','/r','/rd','wf/x','s','running','{}','{}',0,0)",
            rusqlite::params![id, parent],
        )
        .unwrap();
    }
    let mut kids = child_run_ids(&conn, "p");
    kids.sort();
    assert_eq!(kids, vec!["c1".to_string(), "c2".to_string()]);
    assert!(child_run_ids(&conn, "other").is_empty());
}

#[test]
fn rebuild_sub_runs_reconstructs_tracking_from_the_journal() {
    let tmp = tempfile::tempdir().unwrap();
    let db = crate::database::init(tmp.path()).unwrap();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                base_sha,status,budgets_json,spent_json,created_at,updated_at)
             VALUES ('run','n','{}','t','p','/r','/rd','wf/x','s','running','{}','{}',0,0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO wf_run (id,parent_run_id,name,spec_json,task,project_id,repo_path,run_dir,
                branch,base_sha,status,budgets_json,spent_json,created_at,updated_at)
             VALUES ('sub','run','n','{}','t','p','/r','/rd','wf/s','s','done','{}','{}',0,0)",
        [],
    )
    .unwrap();
    journal_event(
        &conn,
        None,
        "run",
        event_type::SUBRUN_LAUNCHED,
        Some("orch-exec"),
        &json!({
            "sub_run_id": "sub",
            "block_index": 0,
            "integrate": "merge",
            "reserved_turns": 30,
            "reserved_tokens": 500,
        }),
    );
    let map = rebuild_sub_runs(&conn, "run", 0);
    let st = map.get("sub").expect("sub-run rebuilt");
    assert!(matches!(st.integrate, Integrate::Merge));
    assert_eq!(st.reserved_turns, 30);
    assert_eq!(st.reserved_tokens, 500);
    assert!(
        matches!(st.terminal, Some(ChildStatus::Success)),
        "a `done` sub-run is terminal so it is not reaped again"
    );
    // A different stage's rebuild sees nothing.
    assert!(rebuild_sub_runs(&conn, "run", 1).is_empty());
}

#[test]
fn subrun_terminal_status_maps_run_status() {
    let tmp = tempfile::tempdir().unwrap();
    let db = crate::database::init(tmp.path()).unwrap();
    let conn = db.lock();
    for (id, status) in [
        ("d", "done"),
        ("f", "failed"),
        ("c", "canceled"),
        ("r", "running"),
    ] {
        conn.execute(
            "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                    base_sha,status,budgets_json,spent_json,created_at,updated_at)
                 VALUES (?1,'n','{}','t','p','/r','/rd','wf/x','s',?2,'{}','{}',0,0)",
            rusqlite::params![id, status],
        )
        .unwrap();
    }
    assert!(matches!(
        subrun_terminal_status(&conn, "d"),
        Some(ChildStatus::Success)
    ));
    assert!(matches!(
        subrun_terminal_status(&conn, "f"),
        Some(ChildStatus::Failure(_))
    ));
    assert!(matches!(
        subrun_terminal_status(&conn, "c"),
        Some(ChildStatus::Skipped)
    ));
    assert!(subrun_terminal_status(&conn, "r").is_none());
}

/// A run whose whole workflow is one orchestrate block with `compose` enabled
/// and no static children. The orchestrator's scripted first decision is a
/// `wf_compose` (a one-step, commit-gated fragment). Returns (db, ws, bb).
fn scaffold_compose(tmp: &Path, run_id: &str) -> (Db, PathBuf, PathBuf) {
    let source = tmp.join("source");
    std::fs::create_dir_all(&source).unwrap();
    sh(&source, &["init", "-q", "-b", "main"]);
    sh(&source, &["config", "user.email", "t@t.t"]);
    sh(&source, &["config", "user.name", "t"]);
    std::fs::write(source.join("README"), "base").unwrap();
    sh(&source, &["add", "-A"]);
    sh(&source, &["commit", "-qm", "base"]);
    let base_sha = {
        let o = Sh::new("git")
            .current_dir(&source)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    let run_dir = tmp.join("rundir");
    let blackboard = blackboard::blackboard_dir(&run_dir);
    std::fs::create_dir_all(&blackboard).unwrap();

    let mut agents = BTreeMap::new();
    for a in ["orch", "coder"] {
        agents.insert(
            a.to_string(),
            super::super::spec::AgentSpec {
                base: "codex".to_string(),
                model: None,
                effort: None,
                instructions: None,
                skills: vec![],
                mcp_servers: vec![],
                custom_agent: None,
            },
        );
    }
    let spec = Spec {
        version: 1,
        name: "compose".to_string(),
        description: None,
        budgets: None,
        agents,
        workflow: vec![Block::Orchestrate(Orchestrate {
            agent: "orch".to_string(),
            goal: "lead".to_string(),
            children: None,
            body: vec![],
            join: Join::All,
            integrate: Integrate::None,
            comms: vec![],
            compose: Some(super::super::spec::ComposeLimits {
                max_sub_runs: 2,
                max_depth: 2,
            }),
        })],
        finalize: None,
    };
    let spec_json = serde_json::to_string(&spec).unwrap();
    let db = crate::database::init(tmp).unwrap();
    db.lock()
        .execute(
            "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                    base_sha,status,budgets_json,spent_json,created_at,updated_at)
                 VALUES (?1,'compose',?2,'t','p',?3,?4,'wf/compose-x',?5,'pending','{}','{}',0,0)",
            rusqlite::params![
                run_id,
                spec_json,
                source.to_string_lossy(),
                run_dir.to_string_lossy(),
                base_sha,
            ],
        )
        .unwrap();
    (db, tmp.join("ws"), blackboard)
}

#[tokio::test]
async fn composed_sub_run_runs_to_done_and_merges_at_the_join() {
    let tmp = tempfile::tempdir().unwrap();
    // The sub-run provisions its own run dir under the runs root; point it at
    // the (writable) tempdir. This is the only test that launches a sub-run,
    // so the process-global override doesn't race other tests.
    std::env::set_var("FLETCH_RUNS_ROOT", tmp.path().join("runs"));
    let (db, ws, bb) = scaffold_compose(tmp.path(), "run-compose");
    // The orchestrator composes a one-step, commit-gated sub-run that merges.
    let decision = json!({
        "decision": "compose",
        "plan": {
            "task": "implement the composed slice",
            "fragment": [{
                "step": {
                    "id": "impl",
                    "agent": "coder",
                    "goal": "write code",
                    "gate": { "type": "commit" }
                }
            }],
            "turns": 20,
            "integrate": "merge",
            "base": "parent-head",
            "block_index": 0
        }
    });
    let driver = OrchDriver::new_scripted(
        ws,
        bb,
        OrchMode::Conclude,
        db.clone(),
        "run-compose",
        decision,
    );
    let ctx = orch_ctx(db.clone(), driver);
    drive_run(&ctx, "run-compose").await;

    assert_eq!(
        run_status_str(&db, "run-compose"),
        "done",
        "the composed sub-run drove the parent to done"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM wf_run WHERE parent_run_id='run-compose'"
        ),
        1,
        "exactly one sub-run was created"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM wf_run WHERE parent_run_id='run-compose' AND status='done'"
        ),
        1,
        "the sub-run ran to done"
    );
    assert!(
        count(
            &db,
            "SELECT COUNT(*) FROM wf_event WHERE run_id='run-compose' AND type='subrun_finished'"
        ) >= 1,
        "the sub-run's completion was journaled"
    );
    assert!(
        count(
            &db,
            "SELECT COUNT(*) FROM wf_event WHERE run_id='run-compose' AND type='merge_done'"
        ) >= 1,
        "the sub-run merged at the join"
    );
}

// ───────────────── run-level cancel mid-attempt / mid-stage (§6.5) ─────────

/// A driver whose turns never end: `send_message` flips the agent to
/// `Running` and returns, but `Idle` never follows — the attempt sits in its
/// turn until something (the cancel race) ends it. Models the H2 scenario:
/// a user cancel landing while work is in flight.
struct HoldDriver {
    root: PathBuf,
    tx: broadcast::Sender<StatusEvent>,
    state: parking_lot::Mutex<StubState>,
    turns_started: std::sync::atomic::AtomicUsize,
    stopped: parking_lot::Mutex<Vec<String>>,
}
impl HoldDriver {
    fn new(root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            root,
            tx: broadcast::channel(256).0,
            state: parking_lot::Mutex::new(StubState::default()),
            turns_started: std::sync::atomic::AtomicUsize::new(0),
            stopped: parking_lot::Mutex::new(Vec::new()),
        })
    }
    fn set(&self, id: &str, s: AgentStatus) {
        self.state.lock().statuses.insert(id.to_string(), s.clone());
        let _ = self.tx.send(StatusEvent {
            agent_id: id.to_string(),
            status: s,
        });
    }
}
impl AgentDriver for HoldDriver {
    fn spawn(
        &self,
        req: SpawnReq,
    ) -> super::super::driver::BoxFuture<'_, Result<super::super::driver::SpawnedAgent>> {
        Box::pin(async move {
            let id = {
                let mut st = self.state.lock();
                st.count += 1;
                format!("hold-{}", st.count)
            };
            let dest = self.root.join(&id);
            let base_ref = req.fork_base.clone().unwrap();
            let spec = crate::sandbox::provision::CheckoutSpec {
                source_repo: &req.repo_path,
                base_ref: &base_ref,
                dest: &dest,
            };
            crate::sandbox::provision::provision_forking_run_repo(
                &spec,
                req.run_repo.as_ref().unwrap(),
            )
            .await?;
            self.state.lock().worktrees.insert(id.clone(), dest.clone());
            self.set(&id, AgentStatus::Idle);
            Ok(super::super::driver::SpawnedAgent {
                agent_id: id,
                worktree: dest,
            })
        })
    }
    fn status(&self, id: &str) -> Option<AgentStatus> {
        self.state.lock().statuses.get(id).cloned()
    }
    fn subscribe(&self) -> broadcast::Receiver<StatusEvent> {
        self.tx.subscribe()
    }
    fn send_message<'a>(
        &'a self,
        id: &'a str,
        _text: String,
    ) -> super::super::driver::BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.set(id, AgentStatus::Running);
            self.turns_started
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(()) // Idle never arrives — the turn hangs until cancelled.
        })
    }
    fn stop<'a>(&'a self, id: &'a str) -> super::super::driver::BoxFuture<'a, Result<()>> {
        self.stopped.lock().push(id.to_string());
        Box::pin(async { Ok(()) })
    }
    fn archive<'a>(&'a self, _id: &'a str) -> super::super::driver::BoxFuture<'a, Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn last_activity(&self, _id: &str) -> Option<i64> {
        // "Recent activity" forever, so the stall watchdog never fires and
        // the cancel race is the only thing that can end the turn.
        Some(crate::workflow::now_ms())
    }
    fn turn_usage(&self, _id: &str) -> Option<super::super::driver::TurnUsage> {
        None
    }
}

/// Wait (bounded) until `n` turns have started on the driver.
async fn await_turns(driver: &HoldDriver, n: usize) {
    for _ in 0..400 {
        if driver
            .turns_started
            .load(std::sync::atomic::Ordering::SeqCst)
            >= n
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("driver never reached {n} started turn(s)");
}

#[tokio::test]
async fn cancel_mid_turn_stops_the_attempt_and_cancels_the_run() {
    let tmp = tempfile::tempdir().unwrap();
    let (db, ws) = scaffold_one_step(tmp.path(), "run-midcancel", "wf/mc-1", Gate::Commit);
    let driver = HoldDriver::new(ws);
    let cancel = Arc::new(AtomicBool::new(false));
    let ctx = RunCtx {
        db: db.clone(),
        driver: driver.clone(),
        app: None,
        cancel: cancel.clone(),
        pending_ask: Arc::new(AtomicBool::new(false)),
        deadlines: Deadlines::default(),
        runs: None,
    };
    let drive = tokio::spawn(async move { drive_run(&ctx, "run-midcancel").await });
    await_turns(&driver, 1).await;
    cancel.store(true, Ordering::SeqCst); // user cancel lands mid-turn
    drive.await.unwrap();

    assert_eq!(run_status_str(&db, "run-midcancel"), "canceled");
    assert_eq!(
        count_children(&db, "run-midcancel", "abandoned"),
        1,
        "the in-flight attempt was abandoned, not left running"
    );
    assert!(
        !driver.stopped.lock().is_empty(),
        "the live agent process was stopped"
    );
    assert!(
        count(
            &db,
            "SELECT COUNT(*) FROM wf_event WHERE run_id='run-midcancel' AND type='run_canceled'"
        ) >= 1,
        "the cancel was journaled"
    );
}

#[tokio::test]
async fn cancel_mid_parallel_stage_winds_children_down_and_cancels_the_run() {
    let tmp = tempfile::tempdir().unwrap();
    let children = vec![cstep("a", ""), cstep("b", "")];
    let (db, ws, _base) = scaffold_parallel(tmp.path(), "run-pcancel", Join::All, &children);
    let driver = HoldDriver::new(ws);
    let cancel = Arc::new(AtomicBool::new(false));
    let ctx = RunCtx {
        db: db.clone(),
        driver: driver.clone(),
        app: None,
        cancel: cancel.clone(),
        pending_ask: Arc::new(AtomicBool::new(false)),
        deadlines: Deadlines::default(),
        runs: None,
    };
    let drive = tokio::spawn(async move { drive_run(&ctx, "run-pcancel").await });
    await_turns(&driver, 2).await; // both children mid-turn
    cancel.store(true, Ordering::SeqCst);
    drive.await.unwrap();

    assert_eq!(run_status_str(&db, "run-pcancel"), "canceled");
    assert_eq!(
        count_children(&db, "run-pcancel", "abandoned"),
        2,
        "both in-flight children were abandoned"
    );
    assert_eq!(
        count_children(&db, "run-pcancel", "running"),
        0,
        "no child was left running"
    );
}

// ───────────── resume line state across loop / orchestrate blocks (H1) ─────

fn done_exec(conn: &Connection, id: &str, run_id: &str, step_id: &str, iter: i64) {
    create_step_exec(conn, id, run_id, step_id, 1, iter, "verdict");
    finish_step_exec(conn, id, "done", Some("head-sha"));
}

fn loop_block(body_ids: &[&str], until: &str) -> Block {
    Block::Loop(super::super::spec::Loop {
        max: 3,
        until: super::super::spec::Until {
            step: until.to_string(),
            verdict: Default::default(),
        },
        body: body_ids
            .iter()
            .map(|id| Block::Step(cstep(id, "")))
            .collect(),
    })
}

#[tokio::test]
async fn resume_line_state_advances_past_a_completed_loop() {
    let tmp = tempfile::tempdir().unwrap();
    let (db, _ws) = scaffold_one_step(tmp.path(), "run-loopresume", "wf/lr-1", Gate::Commit);
    let blocks = vec![
        loop_block(&["review", "fix"], "review"),
        Block::Step(cstep("ship", "")),
    ];
    {
        let conn = db.lock();
        // Two iterations: review/fix (iter 0), then the closing review (iter 1).
        done_exec(&conn, "exec-r0", "run-loopresume", "review", 0);
        done_exec(&conn, "exec-f0", "run-loopresume", "fix", 0);
        done_exec(&conn, "exec-r1", "run-loopresume", "review", 1);
    }
    let conn = db.lock();
    // Cursor past the loop (at `ship`): the line must be the loop's last
    // done body exec — not the run base (the H1 silent-work-loss bug).
    let (line_ref, exec) = resume_line_state(&conn, "run-loopresume", &blocks, 1, "base-sha");
    assert_eq!(exec.as_deref(), Some("exec-r1"));
    assert_eq!(line_ref, gitops::step_ref("exec-r1"));
}

#[tokio::test]
async fn resume_line_state_uses_an_orchestrate_stages_merge_exec() {
    let tmp = tempfile::tempdir().unwrap();
    let (db, _ws) = scaffold_one_step(tmp.path(), "run-orchresume", "wf/or-1", Gate::Commit);
    let orch = Block::Orchestrate(super::super::spec::Orchestrate {
        agent: "coder".to_string(),
        goal: "coordinate".to_string(),
        children: None,
        body: vec![],
        join: Join::All,
        integrate: Integrate::None,
        comms: vec![],
        compose: None,
    });
    let blocks = vec![orch, Block::Step(cstep("ship", ""))];
    {
        let conn = db.lock();
        // A composed sub-run merged at the join → synthetic `__merge_0` exec.
        done_exec(&conn, "exec-m0", "run-orchresume", &merge_step_id(0), 0);
    }
    let conn = db.lock();
    let (line_ref, exec) = resume_line_state(&conn, "run-orchresume", &blocks, 1, "base-sha");
    assert_eq!(exec.as_deref(), Some("exec-m0"));
    assert_eq!(line_ref, gitops::step_ref("exec-m0"));
}

#[tokio::test]
async fn resume_line_state_falls_back_to_base_for_an_unmerged_orchestrate() {
    let tmp = tempfile::tempdir().unwrap();
    let (db, _ws) = scaffold_one_step(tmp.path(), "run-orchnone", "wf/on-1", Gate::Commit);
    let orch = Block::Orchestrate(super::super::spec::Orchestrate {
        agent: "coder".to_string(),
        goal: "coordinate".to_string(),
        children: None,
        body: vec![],
        join: Join::All,
        integrate: Integrate::None,
        comms: vec![],
        compose: None,
    });
    let blocks = vec![orch, Block::Step(cstep("ship", ""))];
    let conn = db.lock();
    let (line_ref, exec) = resume_line_state(&conn, "run-orchnone", &blocks, 1, "base-sha");
    assert_eq!(exec, None);
    assert_eq!(line_ref, "base-sha");
}
