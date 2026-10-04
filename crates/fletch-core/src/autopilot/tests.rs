// The host-owned half of autopilot: the two switches (same keys and encodings
// the desktop wrote, so existing opt-outs survive), the bounded history log,
// the three ops and the publish pre-approval. Ported where the desktop store
// tested the same rule (`src/store/autopilot.test.ts`, "workspace switch",
// `src/store/autopilotLog.test.ts`, `src/store/publishApproval.test.ts`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::store::{LogScope, LOG_LIMIT, PAUSED_AGENTS_KEY, PROJECT_ENABLED_KEY};
use super::*;
use crate::host::sink::RecordingSink;
use crate::workspace::{new_agent_record, AgentView, TrackedRepo, WorkspaceManager};

/// One database behind both the engine ctx and the supervisor, as in a real
/// host: the switches are read through the ctx, the agents through the
/// supervisor.
pub(crate) fn host() -> (
    Arc<EngineCtx>,
    Arc<RecordingSink>,
    Arc<Supervisor>,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::database::init(dir.path()).unwrap();
    let sink = Arc::new(RecordingSink::new());
    let ctx = Arc::new(EngineCtx::new(sink.clone(), db.clone(), Box::new(|| false)));
    let sup = Arc::new(Supervisor::new(Arc::new(WorkspaceManager::new(db))));
    (ctx, sink, sup, dir)
}

pub(crate) fn tracked_repo(path: &Path, subdir: &str) -> TrackedRepo {
    TrackedRepo {
        repo_path: path.to_path_buf(),
        subdir: subdir.to_string(),
        branch: None,
        parent_branch: None,
        base_sha: None,
        pr_number: None,
        pr_url: None,
        pr_title: None,
        pr_state: None,
        label: None,
        adopted_checkout: None,
    }
}

/// A folder that passes for a repository (`add_workspace_repo` only asks for
/// a `.git`).
fn repo_dir(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    std::fs::create_dir_all(path.join(".git")).unwrap();
    path
}

/// Add agent `id` to the supervisor's workspace with one checkout per entry
/// of `subdirs` (the first is the primary), all in the project of `project`.
fn add_agent(
    sup: &Supervisor,
    root: &Path,
    project: &str,
    id: &str,
    subdirs: &[&str],
) -> AgentRecord {
    let primary = repo_dir(root, project);
    sup.workspace.add_workspace_repo(primary.clone()).unwrap();
    let mut record = new_agent_record(
        id.to_string(),
        id.to_string(),
        "claude".to_string(),
        tracked_repo(&primary, subdirs[0]),
        String::new(),
        AgentView::Custom,
    );
    for subdir in &subdirs[1..] {
        let path = repo_dir(root, &format!("{project}-{subdir}"));
        sup.workspace.add_workspace_repo(path.clone()).unwrap();
        record.repos.push(tracked_repo(&path, subdir));
    }
    sup.workspace.add_agent(&mut record).unwrap();
    sup.workspace.agent(id).unwrap()
}

fn table() -> Table {
    Table::default()
}

fn key(agent: &str, subdir: Option<&str>) -> Key {
    (agent.to_string(), subdir.map(str::to_string))
}

fn states(sink: &RecordingSink) -> Vec<serde_json::Value> {
    sink.events()
        .into_iter()
        .filter(|(name, _)| name == EVENT_STATE)
        .map(|(_, payload)| payload)
        .collect()
}

fn switches_events(sink: &RecordingSink) -> Vec<serde_json::Value> {
    sink.events()
        .into_iter()
        .filter(|(name, _)| name == EVENT_SWITCHES)
        .map(|(_, payload)| payload)
        .collect()
}

fn entry(agent: &str, subdir: Option<&str>, at: i64, outcome: Outcome) -> LogEntry {
    LogEntry {
        id: uuid::Uuid::new_v4().to_string(),
        agent_id: agent.to_string(),
        subdir: subdir.map(str::to_string),
        at,
        outcome,
        rung: DelegationKind::FixChecks,
        attempt: 1,
        reason: None,
    }
}

// ── the switches ───────────────────────────────────────────────────────────

#[test]
fn only_an_explicit_off_switches_a_project_off() {
    let (ctx, _sink, sup, dir) = host();
    let mut ids = Vec::new();
    for (i, value) in ["0", "false", "1", "true", "nope", ""].iter().enumerate() {
        let a = add_agent(
            &sup,
            dir.path(),
            &format!("p{i}"),
            &format!("a{i}"),
            &["repo"],
        );
        ctx.db
            .lock()
            .execute(
                "INSERT INTO project_settings (project_id, key, value) VALUES (?1, ?2, ?3)",
                rusqlite::params![a.project_id, PROJECT_ENABLED_KEY, value],
            )
            .unwrap();
        ids.push(a.project_id);
    }
    let switches = store::read_switches(&ctx.db.lock()).unwrap();
    let mut expected = vec![ids[0].clone(), ids[1].clone()];
    expected.sort();
    assert_eq!(switches.disabled_projects, expected);
    // No row at all: on, the default.
    assert!(switches.project_on("never-touched"));
}

#[test]
fn turning_a_project_off_writes_the_one_row_and_on_deletes_it() {
    let (ctx, _sink, sup, dir) = host();
    let a = add_agent(&sup, dir.path(), "proj", "aare", &["repo"]);
    let conn = ctx.db.lock();
    store::set_project(&conn, &a.project_id, false).unwrap();
    store::set_project(&conn, &a.project_id, false).unwrap();
    let value: String = conn
        .query_row(
            "SELECT value FROM project_settings WHERE project_id = ?1 AND key = ?2",
            rusqlite::params![a.project_id, PROJECT_ENABLED_KEY],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(value, "0", "the desktop's encoding, so its reader agrees");
    store::set_project(&conn, &a.project_id, true).unwrap();
    let rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM project_settings WHERE key = ?1",
            [PROJECT_ENABLED_KEY],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0, "on is the default, so on is no row");
}

#[test]
fn pauses_persist_as_one_json_list_pruned_of_agents_that_are_gone() {
    let (ctx, _sink, _sup, _dir) = host();
    let conn = ctx.db.lock();
    crate::database::set_setting(&conn, PAUSED_AGENTS_KEY, r#"["gone","aare"]"#).unwrap();
    let live = |id: &str| id != "gone";
    store::set_agent_paused(&conn, "rhine", true, live).unwrap();
    assert_eq!(
        crate::database::get_setting(&conn, PAUSED_AGENTS_KEY).as_deref(),
        Some(r#"["aare","rhine"]"#)
    );
    // Un-pausing touches only the agent asked for.
    store::set_agent_paused(&conn, "aare", false, live).unwrap();
    assert_eq!(
        store::read_switches(&conn).unwrap().paused_agents,
        ["rhine"]
    );
    // A list that is not one reads as nothing paused, as the desktop parsed it.
    crate::database::set_setting(&conn, PAUSED_AGENTS_KEY, "{oops").unwrap();
    assert!(store::read_switches(&conn)
        .unwrap()
        .paused_agents
        .is_empty());
}

// ── the log ────────────────────────────────────────────────────────────────

#[test]
fn the_log_reads_newest_first_and_keeps_every_field() {
    let (ctx, _sink, _sup, _dir) = host();
    let conn = ctx.db.lock();
    let first = entry("aare", None, 10, Outcome::Dispatch);
    let mut gave_up = entry("aare", None, 20, Outcome::GiveUp);
    gave_up.attempt = 3;
    gave_up.reason = Some(GiveUpReason::BudgetSpent);
    gave_up.rung = DelegationKind::Resolve;
    store::append_log(&conn, &first).unwrap();
    store::append_log(&conn, &gave_up).unwrap();
    let rows = store::read_log(&conn, LogScope::Agent("aare")).unwrap();
    assert_eq!(rows, [gave_up.clone(), first]);
    let wire = serde_json::to_value(&gave_up).unwrap();
    assert_eq!(wire["outcome"], "give-up");
    assert_eq!(wire["rung"], "resolve");
    assert_eq!(wire["reason"], "budget-spent");
    assert_eq!(wire["subdir"], serde_json::Value::Null);
}

#[test]
fn the_log_keeps_the_newest_rows_per_checkout_and_bounds_each_separately() {
    let (ctx, _sink, _sup, _dir) = host();
    let conn = ctx.db.lock();
    for at in 0..(LOG_LIMIT as i64 + 5) {
        store::append_log(&conn, &entry("aare", None, at, Outcome::Retry)).unwrap();
    }
    store::append_log(&conn, &entry("aare", Some("web"), 0, Outcome::Retry)).unwrap();
    let primary = store::read_log(&conn, LogScope::Checkout("aare", None)).unwrap();
    assert_eq!(primary.len(), LOG_LIMIT);
    assert_eq!(primary[0].at, LOG_LIMIT as i64 + 4);
    assert_eq!(primary.last().unwrap().at, 5, "the oldest five went");
    let web = store::read_log(&conn, LogScope::Checkout("aare", Some("web"))).unwrap();
    assert_eq!(web.len(), 1, "another checkout's rows are its own");
    assert_eq!(
        store::read_log(&conn, LogScope::All).unwrap().len(),
        LOG_LIMIT + 1
    );
}

#[test]
fn the_history_of_a_discarded_agent_is_pruned_and_an_archived_ones_kept() {
    let (ctx, _sink, sup, dir) = host();
    add_agent(&sup, dir.path(), "proj", "kept", &["repo"]);
    let conn = ctx.db.lock();
    store::append_log(&conn, &entry("kept", None, 1, Outcome::Settle)).unwrap();
    store::append_log(&conn, &entry("discarded", None, 1, Outcome::Settle)).unwrap();
    store::prune_orphans(&conn).unwrap();
    let left: Vec<String> = store::read_log(&conn, LogScope::All)
        .unwrap()
        .into_iter()
        .map(|e| e.agent_id)
        .collect();
    assert_eq!(left, ["kept"]);
}

#[test]
fn autopilot_log_names_the_primary_by_its_own_subdir_too() {
    let (ctx, _sink, sup, dir) = host();
    add_agent(&sup, dir.path(), "proj", "aare", &["repo", "web"]);
    {
        let conn = ctx.db.lock();
        store::append_log(&conn, &entry("aare", None, 1, Outcome::Dispatch)).unwrap();
        store::append_log(&conn, &entry("aare", Some("web"), 2, Outcome::Dispatch)).unwrap();
    }
    let primary = autopilot_log_impl(&ctx, &sup, Some("aare"), Some("repo")).unwrap();
    assert_eq!(primary.len(), 1);
    assert_eq!(primary[0].subdir, None);
    let web = autopilot_log_impl(&ctx, &sup, Some("aare"), Some("web")).unwrap();
    assert_eq!(web[0].subdir.as_deref(), Some("web"));
    assert_eq!(
        autopilot_log_impl(&ctx, &sup, Some("aare"), None)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(autopilot_log_impl(&ctx, &sup, None, None).unwrap().len(), 2);
}

// ── autopilot_state ────────────────────────────────────────────────────────

#[test]
fn state_lists_every_checkout_keyed_like_the_delegations() {
    let (ctx, _sink, sup, dir) = host();
    let a = add_agent(&sup, dir.path(), "proj", "aare", &["repo", "web"]);
    let t = table();
    t.enroll(&key("aare", Some("web")));
    t.update(&key("aare", Some("web")), |tr| {
        tr.state
            .open_cycle(DelegationKind::FixChecks, "sig".into(), "sit".into(), 42)
    });

    let snap = state(&t, &ctx, &sup, Some("aare")).unwrap();
    let wire = serde_json::to_value(&snap).unwrap();
    assert_eq!(wire["disabled_projects"], serde_json::json!([]));
    assert_eq!(wire["paused_agents"], serde_json::json!([]));
    let rows = wire["checkouts"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["subdir"], serde_json::Value::Null);
    assert_eq!(rows[0]["project_id"], a.project_id);
    assert_eq!(rows[0]["enrolled"], true, "on by default");
    assert_eq!(rows[0]["cycle"], serde_json::Value::Null);
    assert_eq!(rows[1]["subdir"], "web");
    assert_eq!(
        rows[1]["cycle"],
        serde_json::json!({ "rung": "fix-checks", "attempt": 1, "phase": "working", "since": 42 })
    );
}

#[test]
fn state_of_every_agent_leaves_out_the_archived_and_reports_the_switches() {
    let (ctx, _sink, sup, dir) = host();
    let a = add_agent(&sup, dir.path(), "proj", "aare", &["repo"]);
    add_agent(&sup, dir.path(), "other", "rhone", &["repo"]);
    crate::workspace::tests::mark_archived(&ctx.db, "rhone");
    store::set_project(&ctx.db.lock(), &a.project_id, false).unwrap();
    let snap = state(&table(), &ctx, &sup, None).unwrap();
    let ids: Vec<&str> = snap.checkouts.iter().map(|c| c.agent_id.as_str()).collect();
    assert!(ids.contains(&"aare"));
    assert_eq!(snap.disabled_projects, std::slice::from_ref(&a.project_id));
    let row = &snap.checkouts[ids.iter().position(|i| *i == "aare").unwrap()];
    assert!(!row.project_enabled);
    assert!(!row.enrolled);
}

// ── autopilot_set ──────────────────────────────────────────────────────────

#[test]
fn set_takes_exactly_one_of_the_two_ids() {
    let (ctx, sink, sup, _dir) = host();
    for (project, agent) in [(None, None), (Some("p"), Some("a")), (Some("  "), None)] {
        let err = set(&table(), &ctx, &sup, project, agent, true).unwrap_err();
        assert!(err.to_string().contains("exactly one"), "{err}");
    }
    let err = set(&table(), &ctx, &sup, None, Some("ghost"), false).unwrap_err();
    assert!(matches!(err, Error::AgentNotFound(_)), "{err}");
    assert!(sink.events().is_empty());
}

#[test]
fn pausing_an_agent_stops_it_now_and_says_so_for_every_checkout() {
    let (ctx, sink, sup, dir) = host();
    add_agent(&sup, dir.path(), "proj", "aare", &["repo", "web"]);
    let t = table();
    for subdir in [None, Some("web")] {
        t.enroll(&key("aare", subdir));
    }
    t.update(&key("aare", None), |tr| {
        tr.state
            .open_cycle(DelegationKind::FixChecks, "sig".into(), "sit".into(), 1)
    });

    let snap = set(&t, &ctx, &sup, None, Some("aare"), false).unwrap();

    assert_eq!(snap.paused_agents, ["aare"]);
    assert!(
        t.keys().is_empty(),
        "both checkouts dropped, the cycle with them"
    );
    let rows = states(&sink);
    assert_eq!(rows.len(), 2);
    for row in &rows {
        assert_eq!(row["paused"], true);
        assert_eq!(row["enrolled"], false);
        assert_eq!(row["cycle"], serde_json::Value::Null);
    }
    // Resuming says so too; the driver enrolls at its next pass.
    let snap = set(&t, &ctx, &sup, None, Some("aare"), true).unwrap();
    assert!(snap.paused_agents.is_empty());
    let resumed = states(&sink);
    assert_eq!(resumed.len(), 4);
    assert!(resumed[2..].iter().all(|r| r["enrolled"] == true));
}

#[test]
fn switching_a_project_off_drops_only_its_agents() {
    let (ctx, sink, sup, dir) = host();
    let a = add_agent(&sup, dir.path(), "proj", "aare", &["repo"]);
    add_agent(&sup, dir.path(), "other", "rhone", &["repo"]);
    let t = table();
    t.enroll(&key("aare", None));
    t.enroll(&key("rhone", None));

    let snap = set(&t, &ctx, &sup, Some(&a.project_id), None, false).unwrap();

    assert_eq!(snap.disabled_projects, std::slice::from_ref(&a.project_id));
    assert_eq!(t.keys(), [key("rhone", None)]);
    let rows = states(&sink);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["agent_id"], "aare");
    assert_eq!(rows[0]["project_enabled"], false);
    // The whole host's snapshot comes back, the untouched agent included.
    assert_eq!(snap.checkouts.len(), 2);
}

#[test]
fn switching_a_project_with_no_agents_still_tells_every_client() {
    let (ctx, sink, sup, dir) = host();
    add_agent(&sup, dir.path(), "other", "rhone", &["repo"]);
    let path = repo_dir(dir.path(), "empty");
    let ws = sup.workspace.add_workspace_repo(path.clone()).unwrap();
    let empty = ws
        .projects
        .iter()
        .find(|p| p.path == path)
        .unwrap()
        .project_id
        .clone();
    let t = table();

    set(&t, &ctx, &sup, Some(&empty), None, false).unwrap();

    let events = sink.events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].0, EVENT_SWITCHES);
    assert_eq!(
        events[0].1,
        serde_json::json!({ "disabled_projects": [empty], "paused_agents": [] })
    );
    assert!(states(&sink).is_empty(), "no checkout's row changed");
}

#[test]
fn every_write_announces_both_lists_before_the_rows() {
    let (ctx, sink, sup, dir) = host();
    let a = add_agent(&sup, dir.path(), "proj", "aare", &["repo"]);
    let t = table();

    set(&t, &ctx, &sup, Some(&a.project_id), None, false).unwrap();
    set(&t, &ctx, &sup, None, Some("aare"), false).unwrap();

    let names: Vec<String> = sink.events().into_iter().map(|(n, _)| n).collect();
    assert_eq!(
        names,
        [EVENT_SWITCHES, EVENT_STATE, EVENT_SWITCHES, EVENT_STATE]
    );
    let last = switches_events(&sink).pop().unwrap();
    assert_eq!(
        last,
        serde_json::json!({ "disabled_projects": [a.project_id], "paused_agents": ["aare"] })
    );
}

// ── the publish pre-approval ───────────────────────────────────────────────

#[test]
fn an_enrolled_checkout_pre_approves_its_pushes_and_nothing_else() {
    // The global table, so the gate's own entry point is what is tested; the
    // agent id is this test's alone.
    let k = key("pre-auth-tyne", None);
    global().enroll(&k);
    assert!(pre_authorizes("pre-auth-tyne", None, "git_push"));
    // Opening a PR creates a new artifact under the user's name: always asks.
    assert!(!pre_authorizes("pre-auth-tyne", None, "open_pr"));
    // Scoped to the checkout: the agent's other repo is not covered.
    assert!(!pre_authorizes("pre-auth-tyne", Some("web"), "git_push"));
    global().remove(&k);
    assert!(
        !pre_authorizes("pre-auth-tyne", None, "git_push"),
        "a paused or switched-off checkout asks again"
    );
}

#[test]
fn a_secondary_checkout_is_covered_by_its_own_enrollment_only() {
    let k = key("pre-auth-wear", Some("web"));
    global().enroll(&k);
    assert!(pre_authorizes("pre-auth-wear", Some("web"), "git_push"));
    assert!(!pre_authorizes("pre-auth-wear", None, "git_push"));
    global().remove(&k);
}

/// The three event names are the wire contract (docs/remote-protocol.md).
#[test]
fn the_wire_names_are_the_documented_ones() {
    assert_eq!(EVENT_STATE, "autopilot:state");
    assert_eq!(EVENT_LOG, "autopilot:event");
    assert_eq!(EVENT_SWITCHES, "autopilot:switches");
    assert_eq!(PROJECT_ENABLED_KEY, "autopilot.enabled");
    assert_eq!(PAUSED_AGENTS_KEY, "autopilotPausedAgents");
}
