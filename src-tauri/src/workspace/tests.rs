use super::paths::{migrate_checkouts_root_in, occupied_checkout_dirs_in};
use super::*;

/// The one branch that mutates on-disk state: legacy dir present, new dir
/// absent, not overridden → the whole tree moves, old path gone.
#[test]
fn migrate_moves_legacy_dir_when_present_and_new_absent() {
    let td = tempfile::tempdir().unwrap();
    let fletch = td.path().join(".fletch");
    std::fs::create_dir_all(fletch.join("worktrees").join("agent-1").join("repo")).unwrap();

    migrate_checkouts_root_in(&fletch, false);

    assert!(
        !fletch.join("worktrees").exists(),
        "legacy dir should be gone"
    );
    assert!(
        fletch
            .join("workspaces")
            .join("agent-1")
            .join("repo")
            .is_dir(),
        "contents should have moved under the new root"
    );
}

/// Override present → never touch anything, even with a legacy dir sitting
/// there (the caller manages the location, e.g. nested-Fletch Run).
#[test]
fn migrate_is_noop_when_overridden() {
    let td = tempfile::tempdir().unwrap();
    let fletch = td.path().join(".fletch");
    std::fs::create_dir_all(fletch.join("worktrees")).unwrap();

    migrate_checkouts_root_in(&fletch, true);

    assert!(
        fletch.join("worktrees").is_dir(),
        "override must leave legacy dir"
    );
    assert!(
        !fletch.join("workspaces").exists(),
        "override must not create new dir"
    );
}

/// No legacy dir (a fresh install) → nothing to migrate, no new dir created.
#[test]
fn migrate_is_noop_when_legacy_absent() {
    let td = tempfile::tempdir().unwrap();
    let fletch = td.path().join(".fletch");
    std::fs::create_dir_all(&fletch).unwrap();

    migrate_checkouts_root_in(&fletch, false);

    assert!(!fletch.join("workspaces").exists(), "nothing to migrate");
}

/// New dir already exists → leave both untouched; never merge a legacy dir
/// into a live workspaces root.
#[test]
fn migrate_is_noop_when_new_already_exists() {
    let td = tempfile::tempdir().unwrap();
    let fletch = td.path().join(".fletch");
    std::fs::create_dir_all(fletch.join("worktrees").join("old-agent")).unwrap();
    std::fs::create_dir_all(fletch.join("workspaces").join("live-agent")).unwrap();

    migrate_checkouts_root_in(&fletch, false);

    assert!(
        fletch.join("worktrees").join("old-agent").is_dir(),
        "legacy left as-is"
    );
    assert!(
        fletch.join("workspaces").join("live-agent").is_dir(),
        "live root untouched"
    );
    assert!(
        !fletch.join("workspaces").join("old-agent").exists(),
        "must not merge legacy contents into the existing new root"
    );
}

fn test_db() -> Arc<Mutex<Connection>> {
    let dir = tempfile::tempdir().unwrap();
    crate::database::init(dir.path()).unwrap()
}

fn init_repo(dir: &Path) -> PathBuf {
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    repo
}

fn mk_repo(path: &str) -> TrackedRepo {
    TrackedRepo {
        repo_path: PathBuf::from(path),
        subdir: "repo".into(),
        branch: None,
        parent_branch: None,
        base_sha: None,
        pr_number: None,
        pr_url: None,
        pr_title: None,
        pr_state: None,
        label: None,
    }
}

/// Helper: ensure the repo path exists in the repos table so add_agent can find it.
fn seed_repo(db: &Arc<Mutex<Connection>>, repo_path: &str) {
    let conn = db.lock();
    let path = Path::new(repo_path);
    let project_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    let project_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT OR IGNORE INTO projects (id, name, created_at) VALUES (?1, ?2, ?3)",
        rusqlite::params![project_id, project_name, now_millis()],
    )
    .unwrap();
    // Re-read the project_id in case it already existed.
    let pid: String = conn
        .query_row(
            "SELECT id FROM projects WHERE name = ?1",
            [project_name],
            |row| row.get(0),
        )
        .unwrap();
    let repo_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT OR IGNORE INTO repos (id, project_id, path, created_at) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![repo_id, pid, repo_path, now_millis()],
    )
    .unwrap();
}

#[test]
fn persists_across_instances_and_rests_at_idle() {
    let db = test_db();

    // Seed repo paths so add_agent can look them up.
    let td = tempfile::tempdir().unwrap();
    let repo = init_repo(td.path());
    seed_repo(&db, "/r");
    seed_repo(&db, "/r2");

    {
        let wm = WorkspaceManager::new(db.clone());
        wm.add_workspace_repo(repo.clone()).unwrap();
        // A resting agent has no durable disposition — it derives to
        // `Idle` (no live process, not running a turn).
        let mut running = new_agent_record(
            "yosemite".into(),
            "a".into(),
            "claude".into(),
            mk_repo("/r"),
            "c".into(),
            AgentView::Custom,
        );
        wm.add_agent(&mut running).unwrap();

        // An agent the user explicitly Stopped stamps `stopped_at`, so it
        // stays Stopped across reloads (available via manual Resume).
        let mut stopped = new_agent_record(
            "dolomites".into(),
            "s".into(),
            "claude".into(),
            mk_repo("/r2"),
            "sc".into(),
            AgentView::Custom,
        );
        wm.add_agent(&mut stopped).unwrap();
        wm.update_agent_status("dolomites", AgentStatus::Stopped, None)
            .unwrap();
    }

    // Second instance — status is derived, so the resting agent comes
    // back as Idle and the stopped one stays Stopped.
    let wm2 = WorkspaceManager::new(db);
    let cur = wm2.current().unwrap();
    assert!(cur.repos.iter().any(|p| p == &repo));
    assert_eq!(cur.agents.len(), 2);

    let yosemite = cur.agents.iter().find(|a| a.id == "yosemite").unwrap();
    let dolomites = cur.agents.iter().find(|a| a.id == "dolomites").unwrap();
    assert_eq!(yosemite.status, AgentStatus::Idle);
    assert_eq!(dolomites.status, AgentStatus::Stopped);
}

#[test]
fn pending_messages_round_trip_and_scoped_delete() {
    use crate::message_queue::PendingMsg;
    let db = test_db();
    seed_repo(&db, "/r");
    let wm = WorkspaceManager::new(db);
    let mut rec = new_agent_record(
        "yosemite".into(),
        "a".into(),
        "claude".into(),
        mk_repo("/r"),
        "task".into(),
        AgentView::Custom,
    );
    wm.add_agent(&mut rec).unwrap();

    let pm = |id: &str, text: &str| PendingMsg {
        turn_id: id.into(),
        text: text.into(),
        attachments: vec![],
        thinking: None,
    };

    // Enqueue three follow-ups; they read back for this workspace in seq order.
    wm.enqueue_pending_message("yosemite", &pm("t1", "first"))
        .unwrap();
    wm.enqueue_pending_message("yosemite", &pm("t2", "second"))
        .unwrap();
    wm.enqueue_pending_message("yosemite", &pm("t3", "third"))
        .unwrap();

    let all = wm.read_all_pending_messages().unwrap();
    let ids: Vec<_> = all
        .iter()
        .map(|(w, m)| (w.as_str(), m.turn_id.as_str()))
        .collect();
    assert_eq!(
        ids,
        vec![("yosemite", "t1"), ("yosemite", "t2"), ("yosemite", "t3")]
    );

    // A flush delivered t1/t2 while t3 arrived during the delivery window
    // (still queued): delete-except-keep drops the delivered rows, keeps t3.
    wm.delete_pending_messages_except("yosemite", &["t3".to_string()])
        .unwrap();
    let after = wm.read_all_pending_messages().unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].1.turn_id, "t3");

    // Empty keep clears the rest (the normal full-drain flush).
    wm.delete_pending_messages_except("yosemite", &[]).unwrap();
    assert!(wm.read_all_pending_messages().unwrap().is_empty());

    // clear wipes whatever remains (teardown path).
    wm.enqueue_pending_message("yosemite", &pm("t4", "again"))
        .unwrap();
    wm.clear_pending_messages("yosemite").unwrap();
    assert!(wm.read_all_pending_messages().unwrap().is_empty());
}

#[test]
fn pending_messages_excluded_for_archived_workspace() {
    use crate::message_queue::PendingMsg;
    let db = test_db();
    seed_repo(&db, "/r");
    let wm = WorkspaceManager::new(db.clone());
    let mut rec = new_agent_record(
        "yosemite".into(),
        "a".into(),
        "claude".into(),
        mk_repo("/r"),
        "task".into(),
        AgentView::Custom,
    );
    wm.add_agent(&mut rec).unwrap();
    wm.enqueue_pending_message(
        "yosemite",
        &PendingMsg {
            turn_id: "t1".into(),
            text: "hi".into(),
            attachments: vec![],
            thinking: None,
        },
    )
    .unwrap();

    // Archiving the workspace must hide its queued rows from rehydration, so
    // a put-away agent never resurrects a queue on the next launch.
    db.lock()
        .execute(
            "UPDATE workspaces SET archived_at = ?1 WHERE id = 'yosemite'",
            [now_millis()],
        )
        .unwrap();
    assert!(wm.read_all_pending_messages().unwrap().is_empty());
}

#[test]
fn sandbox_engine_stamp_round_trips() {
    let db = test_db();
    seed_repo(&db, "/r");
    seed_repo(&db, "/r2");
    let wm = WorkspaceManager::new(db);

    // A stamped engine persists verbatim and comes back on load — the
    // stickiness contract: spawn paths reuse this, never the live setting.
    let mut stamped = new_agent_record(
        "yosemite".into(),
        "a".into(),
        "claude".into(),
        mk_repo("/r"),
        "t".into(),
        AgentView::Custom,
    );
    stamped.sandbox_engine = Some("docker".into());
    wm.add_agent(&mut stamped).unwrap();
    assert_eq!(
        wm.agent("yosemite").unwrap().sandbox_engine.as_deref(),
        Some("docker")
    );

    // An unstamped record (pre-selection agents) stays NULL, which spawn
    // paths treat as sandbox-exec.
    let mut legacy = new_agent_record(
        "dolomites".into(),
        "b".into(),
        "claude".into(),
        mk_repo("/r2"),
        "t".into(),
        AgentView::Custom,
    );
    wm.add_agent(&mut legacy).unwrap();
    assert_eq!(wm.agent("dolomites").unwrap().sandbox_engine, None);
}

#[test]
fn status_derivation() {
    // Archived workspaces are stopped regardless of error/run state.
    assert_eq!(
        derive_status(true, false, false, None),
        AgentStatus::Stopped
    );
    // User-stopped workspaces are stopped.
    assert_eq!(
        derive_status(false, true, false, None),
        AgentStatus::Stopped
    );
    // A recorded error with no live process surfaces as Error.
    assert_eq!(
        derive_status(false, false, false, Some("boom")),
        AgentStatus::Error
    );
    // A live process wins over a stale error.
    assert_eq!(
        derive_status(false, false, true, Some("boom")),
        AgentStatus::Running
    );
    // A resting, clean workspace derives to Idle (lazy resume on send).
    assert_eq!(derive_status(false, false, false, None), AgentStatus::Idle);
}

#[test]
fn rejects_non_repo_path() {
    let db = test_db();
    let td = tempfile::tempdir().unwrap();
    let wm = WorkspaceManager::new(db);
    let err = wm.add_workspace_repo(td.path().join("nope")).unwrap_err();
    assert!(err.to_string().contains("not a git repository"));
}

#[test]
fn add_repo_idempotent() {
    let db = test_db();
    let td = tempfile::tempdir().unwrap();
    let repo = init_repo(td.path());
    let wm = WorkspaceManager::new(db);
    wm.add_workspace_repo(repo.clone()).unwrap();
    wm.add_workspace_repo(repo.clone()).unwrap();
    let cur = wm.current().unwrap();
    assert_eq!(cur.repos.iter().filter(|p| **p == repo).count(), 1);
}

#[test]
fn rename_project_updates_display_name() {
    let db = test_db();
    let td = tempfile::tempdir().unwrap();
    let repo = init_repo(td.path());
    let wm = WorkspaceManager::new(db);
    wm.add_workspace_repo(repo.clone()).unwrap();

    let pid = wm.current().unwrap().projects[0].project_id.clone();
    let ws = wm.rename_project(&pid, "  My Project  ").unwrap();

    // Name is trimmed and decoupled from the folder path, which is untouched.
    assert_eq!(ws.projects[0].name, "My Project");
    assert_eq!(ws.projects[0].path, repo);
    assert!(ws.repos.contains(&repo));
}

#[test]
fn rename_project_rejects_empty_name() {
    let db = test_db();
    let td = tempfile::tempdir().unwrap();
    let repo = init_repo(td.path());
    let wm = WorkspaceManager::new(db);
    wm.add_workspace_repo(repo).unwrap();
    let pid = wm.current().unwrap().projects[0].project_id.clone();

    let err = wm.rename_project(&pid, "   ").unwrap_err();
    assert!(err.to_string().contains("cannot be empty"));
}

#[test]
fn delete_project_cascades_non_archived_agents_and_settings() {
    let db = test_db();
    let td = tempfile::tempdir().unwrap();
    let repo = init_repo(td.path());
    let wm = WorkspaceManager::new(db.clone());
    wm.add_workspace_repo(repo.clone()).unwrap();
    let pid = wm.current().unwrap().projects[0].project_id.clone();

    let mut rec = new_agent_record(
        "yosemite".into(),
        "agent".into(),
        "claude".into(),
        mk_repo(repo.to_str().unwrap()),
        "task".into(),
        AgentView::Custom,
    );
    wm.add_agent(&mut rec).unwrap();
    db.lock()
            .execute(
                "INSERT INTO project_settings (project_id, key, value) VALUES (?1, 'run.dev', 'npm run dev')",
                [&pid],
            )
            .unwrap();
    db.lock()
        .execute(
            "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                    base_sha,status,budgets_json,spent_json,created_at,updated_at)
                 VALUES ('r1','n','{}','t',?1,'/r','/d','wf/x','sha','done','{}','{}',0,0)",
            [&pid],
        )
        .unwrap();

    wm.delete_project(&pid, &["r1".to_string()]).unwrap();
    let ws = wm.current().unwrap();
    assert!(ws.projects.is_empty());
    assert!(ws.repos.is_empty());
    assert!(ws.agents.is_empty());
    let setting_count: i64 = db
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM project_settings WHERE project_id = ?1",
            [&pid],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(setting_count, 0);
    let run_count: i64 = db
        .lock()
        .query_row("SELECT COUNT(*) FROM wf_run WHERE id = 'r1'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(run_count, 0, "workflow rows share the project commit");
}

#[test]
fn delete_project_rolls_back_when_the_workflow_set_changed() {
    let db = test_db();
    let td = tempfile::tempdir().unwrap();
    let repo = init_repo(td.path());
    let wm = WorkspaceManager::new(db.clone());
    wm.add_workspace_repo(repo.clone()).unwrap();
    let pid = wm.current().unwrap().projects[0].project_id.clone();

    let mut rec = new_agent_record(
        "yosemite".into(),
        "agent".into(),
        "claude".into(),
        mk_repo(repo.to_str().unwrap()),
        "task".into(),
        AgentView::Custom,
    );
    wm.add_agent(&mut rec).unwrap();
    db.lock()
        .execute(
            "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                    base_sha,status,budgets_json,spent_json,created_at,updated_at)
                 VALUES ('r1','n','{}','t',?1,'/r','/d','wf/x','sha','done','{}','{}',0,0)",
            [&pid],
        )
        .unwrap();

    let error = wm.delete_project(&pid, &[]).unwrap_err();
    assert!(error.to_string().contains("workflow set changed"));
    let ws = wm.current().unwrap();
    assert_eq!(ws.projects.len(), 1, "project row is rolled back");
    assert_eq!(ws.agents.len(), 1, "agent cascade is rolled back");
    let run_count: i64 = db
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM wf_run WHERE project_id = ?1",
            [&pid],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(run_count, 1, "workflow row is rolled back");
}

#[test]
fn relocate_repo_repoints_path() {
    let db = test_db();
    let td = tempfile::tempdir().unwrap();
    let old = init_repo(td.path());
    let new = td.path().join("moved");
    std::fs::create_dir_all(new.join(".git")).unwrap();

    let wm = WorkspaceManager::new(db);
    wm.add_workspace_repo(old.clone()).unwrap();
    let pid = wm.current().unwrap().projects[0].project_id.clone();

    let ws = wm.relocate_repo(&old, &new).unwrap();
    assert!(ws.repos.contains(&new));
    assert!(!ws.repos.contains(&old));
    // Same project — relocate keeps the id (and any per-project settings).
    assert_eq!(ws.projects[0].project_id, pid);
}

#[test]
fn relocate_repo_repoints_workflow_runs() {
    let db = test_db();
    let td = tempfile::tempdir().unwrap();
    let old = init_repo(td.path());
    let new = td.path().join("moved");
    std::fs::create_dir_all(new.join(".git")).unwrap();

    let wm = WorkspaceManager::new(db.clone());
    wm.add_workspace_repo(old.clone()).unwrap();
    let pid = wm.current().unwrap().projects[0].project_id.clone();
    // A second, unrelated project whose historical run snapshotted the SAME path
    // string as `old` (a previous occupant of that path). `repo_path` isn't
    // unique across projects, so the rewrite must not drag this run along.
    let other_pid = "other-project".to_string();
    db.lock()
        .execute(
            "INSERT INTO projects (id, name, created_at) VALUES (?1, 'other', 0)",
            [&other_pid],
        )
        .unwrap();

    let old_str = old.to_string_lossy().to_string();
    let new_str = new.to_string_lossy().to_string();
    // A run snapshots its repo path at launch; `wf_run.repo_path` has no FK to
    // `repos`, so it won't follow the move unless relocate rewrites it.
    let insert = |id: &str, project_id: &String| {
        db.lock()
            .execute(
                "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                    base_sha,status,budgets_json,spent_json,created_at,updated_at)
                 VALUES (?1,'n','{}','t',?2,?3,'/d','wf/x','sha','done','{}','{}',0,0)",
                [&id.to_string(), project_id, &old_str],
            )
            .unwrap();
    };
    insert("r1", &pid);
    insert("r2", &other_pid);

    wm.relocate_repo(&old, &new).unwrap();

    let repo_path = |id: &str| -> String {
        db.lock()
            .query_row("SELECT repo_path FROM wf_run WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .unwrap()
    };
    assert_eq!(
        repo_path("r1"),
        new_str,
        "this project's run follows the relocate"
    );
    assert_eq!(
        repo_path("r2"),
        old_str,
        "another project's run sharing the path string is left untouched"
    );
}

#[test]
fn relocate_repo_rejects_non_git_dest() {
    let db = test_db();
    let td = tempfile::tempdir().unwrap();
    let old = init_repo(td.path());
    let wm = WorkspaceManager::new(db);
    wm.add_workspace_repo(old.clone()).unwrap();

    let err = wm.relocate_repo(&old, &td.path().join("nope")).unwrap_err();
    assert!(err.to_string().contains("not a git repository"));
}

#[test]
fn relocate_repo_rejects_pinned_collision() {
    let db = test_db();
    let td = tempfile::tempdir().unwrap();
    let a = init_repo(td.path());
    let b = td.path().join("b");
    std::fs::create_dir_all(b.join(".git")).unwrap();

    let wm = WorkspaceManager::new(db);
    wm.add_workspace_repo(a.clone()).unwrap();
    wm.add_workspace_repo(b.clone()).unwrap();

    // Moving `a` onto `b`'s already-pinned path is refused, not silently merged.
    let err = wm.relocate_repo(&a, &b).unwrap_err();
    assert!(err.to_string().contains("already pinned"));
}

#[test]
fn remove_repo_leaves_agents_alone() {
    let db = test_db();
    let td = tempfile::tempdir().unwrap();
    let repo = init_repo(td.path());
    let wm = WorkspaceManager::new(db.clone());
    wm.add_workspace_repo(repo.clone()).unwrap();

    let repo_str = repo.to_str().unwrap();
    let mut rec = new_agent_record(
        "yosemite".into(),
        "a".into(),
        "claude".into(),
        mk_repo(repo_str),
        "".into(),
        AgentView::Custom,
    );
    wm.add_agent(&mut rec).unwrap();
    wm.remove_workspace_repo(&repo).unwrap();
    let cur = wm.current().unwrap();
    // The repo record is deleted, but the agent remains (its checkout
    // may reference a now-deleted repo — that's fine, the sidebar
    // union logic handles it).
    assert_eq!(cur.agents.len(), 1);
}

#[test]
fn attach_repo_creates_multi_repo_project() {
    let db = test_db();
    let td = tempfile::tempdir().unwrap();
    let a = init_repo(td.path());
    let b = td.path().join("b");
    std::fs::create_dir_all(b.join(".git")).unwrap();

    let wm = WorkspaceManager::new(db);
    wm.add_workspace_repo(a.clone()).unwrap();
    let cur = wm.current().unwrap();
    let project_id = cur.projects[0].project_id.clone();

    let outcome = wm.attach_repo_to_project(&project_id, &b).unwrap();
    assert!(matches!(outcome, AttachOutcome::Inserted { .. }));
    let cur = wm.current().unwrap();
    assert_eq!(cur.repos.len(), 2);
    assert!(
        cur.projects.iter().all(|p| p.project_id == project_id),
        "both repos share the project"
    );

    // Re-attaching the same path is a no-op, not an error.
    let outcome = wm.attach_repo_to_project(&project_id, &b).unwrap();
    assert!(matches!(outcome, AttachOutcome::AlreadyAttached));
    assert_eq!(wm.current().unwrap().repos.len(), 2);

    // Undoing an insert removes exactly the inserted row.
    let outcome = wm
        .attach_repo_to_project(&project_id, &td.path().join("c"))
        .unwrap();
    wm.undo_attach(&outcome).unwrap();
    assert_eq!(wm.current().unwrap().repos.len(), 2);
}

#[test]
fn attach_repo_moves_empty_project_and_drops_it() {
    let db = test_db();
    let td = tempfile::tempdir().unwrap();
    let a = init_repo(td.path());
    let b = td.path().join("b");
    std::fs::create_dir_all(b.join(".git")).unwrap();

    let wm = WorkspaceManager::new(db.clone());
    wm.add_workspace_repo(a.clone()).unwrap();
    wm.add_workspace_repo(b.clone()).unwrap();
    let cur = wm.current().unwrap();
    let pid_a = cur
        .projects
        .iter()
        .find(|p| p.path == a)
        .unwrap()
        .project_id
        .clone();
    let pid_b = cur
        .projects
        .iter()
        .find(|p| p.path == b)
        .unwrap()
        .project_id
        .clone();
    assert_ne!(pid_a, pid_b);

    // `b` has no agents/runs, so it folds into `a`'s project and its own
    // now-empty project row is removed.
    let outcome = wm.attach_repo_to_project(&pid_a, &b).unwrap();
    let cur = wm.current().unwrap();
    assert!(cur.projects.iter().all(|p| p.project_id == pid_a));
    let count: i64 = db
        .lock()
        .query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1, "emptied source project row dropped");

    // Undoing the move restores the repo's source project — including the
    // dropped project row, verbatim.
    assert!(matches!(
        outcome,
        AttachOutcome::Moved {
            dropped_source: Some(_),
            ..
        }
    ));
    wm.undo_attach(&outcome).unwrap();
    let cur = wm.current().unwrap();
    assert_eq!(
        cur.projects
            .iter()
            .find(|p| p.path == b)
            .unwrap()
            .project_id,
        pid_b
    );
    let count: i64 = db
        .lock()
        .query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2, "dropped source project row restored");
}

#[test]
fn attach_repo_refuses_project_with_agents() {
    let db = test_db();
    let td = tempfile::tempdir().unwrap();
    let a = init_repo(td.path());
    let b = td.path().join("b");
    std::fs::create_dir_all(b.join(".git")).unwrap();

    let wm = WorkspaceManager::new(db);
    wm.add_workspace_repo(a.clone()).unwrap();
    wm.add_workspace_repo(b.clone()).unwrap();
    let cur = wm.current().unwrap();
    let pid_a = cur
        .projects
        .iter()
        .find(|p| p.path == a)
        .unwrap()
        .project_id
        .clone();

    let mut rec = new_agent_record(
        "yosemite".into(),
        "a".into(),
        "claude".into(),
        mk_repo(b.to_str().unwrap()),
        "".into(),
        AgentView::Custom,
    );
    wm.add_agent(&mut rec).unwrap();

    let err = wm.attach_repo_to_project(&pid_a, &b).unwrap_err();
    assert!(err.to_string().contains("agents or workflow runs"));

    // A rejected attach must not have changed anything (transaction
    // rolled back): `b` still belongs to its own project.
    let cur = wm.current().unwrap();
    assert_ne!(
        cur.projects
            .iter()
            .find(|p| p.path == b)
            .unwrap()
            .project_id,
        pid_a
    );

    // Attaching to a project that no longer exists fails without a row.
    let err = wm
        .attach_repo_to_project("gone", &td.path().join("c"))
        .unwrap_err();
    assert!(err.to_string().contains("project not found"));
    assert_eq!(wm.current().unwrap().repos.len(), 2);
}

#[test]
fn detach_repo_guards() {
    let db = test_db();
    let td = tempfile::tempdir().unwrap();
    let a = init_repo(td.path());
    let b = td.path().join("b");
    std::fs::create_dir_all(b.join(".git")).unwrap();

    let wm = WorkspaceManager::new(db);
    wm.add_workspace_repo(a.clone()).unwrap();
    let project_id = wm.current().unwrap().projects[0].project_id.clone();

    // Last repo can't be detached.
    let err = wm.detach_repo_from_project(&project_id, &a).unwrap_err();
    assert!(err.to_string().contains("at least one repository"));

    // A freshly attached, unused repo detaches cleanly.
    wm.attach_repo_to_project(&project_id, &b).unwrap();
    let cur = wm.detach_repo_from_project(&project_id, &b).unwrap();
    assert_eq!(cur.repos.len(), 1);

    // A repo with an agent checkout is protected from the FK cascade.
    wm.attach_repo_to_project(&project_id, &b).unwrap();
    let mut rec = new_agent_record(
        "dolomites".into(),
        "a".into(),
        "claude".into(),
        mk_repo(b.to_str().unwrap()),
        "".into(),
        AgentView::Custom,
    );
    wm.add_agent(&mut rec).unwrap();
    let err = wm.detach_repo_from_project(&project_id, &b).unwrap_err();
    assert!(err.to_string().contains("used by existing agents"));
}

#[test]
fn repo_label_round_trip_and_clear() {
    let db = test_db();
    let td = tempfile::tempdir().unwrap();
    let repo = init_repo(td.path());
    let wm = WorkspaceManager::new(db);
    wm.add_workspace_repo(repo.clone()).unwrap();

    let cur = wm.set_repo_label(&repo, "  Frontend  ").unwrap();
    assert_eq!(cur.projects[0].label.as_deref(), Some("Frontend"));

    // Blank clears back to the basename fallback (label = None).
    let cur = wm.set_repo_label(&repo, "   ").unwrap();
    assert_eq!(cur.projects[0].label, None);

    let err = wm.set_repo_label(&td.path().join("nope"), "x").unwrap_err();
    assert!(err.to_string().contains("repo not found"));
}

#[test]
fn custom_agent_instructions_and_id_round_trip() {
    let db = test_db();
    let wm = WorkspaceManager::new(db.clone());
    seed_repo(&db, "/r");

    let mut rec = new_agent_record(
        "shasta".into(),
        "a".into(),
        "claude".into(),
        mk_repo("/r"),
        "task".into(),
        AgentView::Custom,
    );
    let id = rec.id.clone();
    rec.instructions = Some("You are the Reviewer. Be terse.".into());
    rec.custom_agent_id = Some("ca-reviewer".into());
    wm.add_agent(&mut rec).unwrap();

    // Read back via load_agent (single-row path)…
    let loaded = wm.agent(&id).unwrap();
    assert_eq!(
        loaded.instructions.as_deref(),
        Some("You are the Reviewer. Be terse.")
    );
    assert_eq!(loaded.custom_agent_id.as_deref(), Some("ca-reviewer"));

    // …and via the full list (query_all_agents path).
    let listed = wm
        .current()
        .unwrap()
        .agents
        .into_iter()
        .find(|a| a.id == id)
        .unwrap();
    assert_eq!(listed.custom_agent_id.as_deref(), Some("ca-reviewer"));

    // A plain built-in spawn leaves both columns null.
    let mut plain = new_agent_record(
        "tahoe".into(),
        "b".into(),
        "claude".into(),
        mk_repo("/r"),
        "task".into(),
        AgentView::Custom,
    );
    let plain_id = plain.id.clone();
    wm.add_agent(&mut plain).unwrap();
    let plain_loaded = wm.agent(&plain_id).unwrap();
    assert_eq!(plain_loaded.instructions, None);
    assert_eq!(plain_loaded.custom_agent_id, None);
}

#[test]
fn forked_context_round_trips_separately_from_the_brief() {
    let db = test_db();
    let wm = WorkspaceManager::new(db.clone());
    seed_repo(&db, "/r");

    // A forked session persists the user brief and the carried digest in
    // separate columns; both must survive a round-trip, kept distinct.
    let mut rec = new_agent_record(
        "rainier".into(),
        "a".into(),
        "claude".into(),
        mk_repo("/r"),
        "task".into(),
        AgentView::Custom,
    );
    let id = rec.id.clone();
    rec.instructions = Some("Be terse.".into());
    rec.forked_context = Some("<!-- ctx -->\nprior convo\n<!-- /ctx -->".into());
    wm.add_agent(&mut rec).unwrap();

    // Single-row path (load_agent → map_agent_row).
    let loaded = wm.agent(&id).unwrap();
    assert_eq!(loaded.instructions.as_deref(), Some("Be terse."));
    assert_eq!(
        loaded.forked_context.as_deref(),
        Some("<!-- ctx -->\nprior convo\n<!-- /ctx -->")
    );

    // Full-list path (query_all_agents → map_agent_row) decodes it too.
    let listed = wm
        .current()
        .unwrap()
        .agents
        .into_iter()
        .find(|a| a.id == id)
        .unwrap();
    assert_eq!(
        listed.forked_context.as_deref(),
        Some("<!-- ctx -->\nprior convo\n<!-- /ctx -->")
    );

    // A non-fork session leaves the column null.
    let mut plain = new_agent_record(
        "hood".into(),
        "b".into(),
        "claude".into(),
        mk_repo("/r"),
        "task".into(),
        AgentView::Custom,
    );
    let plain_id = plain.id.clone();
    wm.add_agent(&mut plain).unwrap();
    assert_eq!(wm.agent(&plain_id).unwrap().forked_context, None);
}

#[test]
fn run_owned_agents_are_hidden_from_the_workspace_list() {
    let db = test_db();
    let wm = WorkspaceManager::new(db.clone());
    seed_repo(&db, "/r");

    let mut rec = new_agent_record(
        "denali".into(),
        "a".into(),
        "claude".into(),
        mk_repo("/r"),
        "step task".into(),
        AgentView::Custom,
    );
    let id = rec.id.clone();
    rec.owner_run_id = Some("run-1".into());
    wm.add_agent(&mut rec).unwrap();

    // Hidden from the sidebar list…
    assert!(wm.current().unwrap().agents.iter().all(|a| a.id != id));
    // …but still loadable by id for the workflow engine.
    assert_eq!(
        wm.agent(&id).unwrap().owner_run_id.as_deref(),
        Some("run-1")
    );
}

#[test]
fn skill_and_mcp_snapshots_round_trip() {
    use crate::agent_profile::{McpServerSnapshot, SkillSnapshot};

    let db = test_db();
    let wm = WorkspaceManager::new(db.clone());
    seed_repo(&db, "/r");

    let mut rec = new_agent_record(
        "rainier".into(),
        "a".into(),
        "claude".into(),
        mk_repo("/r"),
        "task".into(),
        AgentView::Custom,
    );
    let id = rec.id.clone();
    rec.skills = vec![SkillSnapshot {
        name: "Code Review".into(),
        description: "how we review".into(),
        body: "# Review\nBe thorough.".into(),
    }];
    rec.mcp_servers = vec![McpServerSnapshot {
        name: "GitHub".into(),
        transport: "stdio".into(),
        command: "npx".into(),
        args: vec!["-y".into(), "gh-mcp".into()],
        env: vec![("TOKEN".into(), "t".into())],
        ..Default::default()
    }];
    wm.add_agent(&mut rec).unwrap();

    let loaded = wm.agent(&id).unwrap();
    assert_eq!(loaded.skills, rec.skills);
    assert_eq!(loaded.mcp_servers, rec.mcp_servers);

    // A plain spawn keeps both columns NULL → empty vecs.
    let mut plain = new_agent_record(
        "hood".into(),
        "b".into(),
        "claude".into(),
        mk_repo("/r"),
        "task".into(),
        AgentView::Custom,
    );
    let plain_id = plain.id.clone();
    wm.add_agent(&mut plain).unwrap();
    let plain_loaded = wm.agent(&plain_id).unwrap();
    assert!(plain_loaded.skills.is_empty());
    assert!(plain_loaded.mcp_servers.is_empty());
}

#[test]
fn pr_number_persists_and_resets_on_name_reuse() {
    let db = test_db();
    let wm = WorkspaceManager::new(db.clone());
    seed_repo(&db, "/r");

    // Spawn an agent named "denali" and record a PR number for it.
    let mut rec = new_agent_record(
        "denali".into(),
        "a".into(),
        "claude".into(),
        mk_repo("/r"),
        "task".into(),
        AgentView::Custom,
    );
    let id = rec.id.clone();
    wm.add_agent(&mut rec).unwrap();
    let subdir = wm.agent(&id).unwrap().repos[0].subdir.clone();

    // No PR until one is recorded.
    assert_eq!(wm.agent(&id).unwrap().repos[0].pr_number, None);
    wm.set_repo_pr_number(&id, &subdir, 42).unwrap();
    assert_eq!(wm.agent(&id).unwrap().repos[0].pr_number, Some(42));

    // Deleting the agent drops its checkout row. A future agent that reuses
    // the same name (and therefore the same branch) starts with no PR — so
    // it can't resolve to the deleted agent's now-merged PR. This is the
    // crux of binding PR identity to the checkout row, not the branch name.
    wm.remove_agent(&id).unwrap();
    let mut reused = new_agent_record(
        "denali".into(),
        "a".into(),
        "claude".into(),
        mk_repo("/r"),
        "task".into(),
        AgentView::Custom,
    );
    let reused_id = reused.id.clone();
    wm.add_agent(&mut reused).unwrap();
    assert_eq!(wm.agent(&reused_id).unwrap().repos[0].pr_number, None);
}

/// A successful PR fetch persists the full snapshot (number, url, title,
/// state, times) and it loads back on the repo record — the database copy
/// the UI falls back to when GitHub or the checkout is unavailable. A
/// later fetch that omits times must not erase the earlier-observed ones.
#[test]
fn pr_snapshot_persists_and_loads() {
    let db = test_db();
    let wm = WorkspaceManager::new(db.clone());
    seed_repo(&db, "/r");

    let mut rec = new_agent_record(
        "denali".into(),
        "a".into(),
        "claude".into(),
        mk_repo("/r"),
        "task".into(),
        AgentView::Custom,
    );
    let id = rec.id.clone();
    wm.add_agent(&mut rec).unwrap();
    let subdir = wm.agent(&id).unwrap().repos[0].subdir.clone();

    let open = crate::github::PrState {
        number: 42,
        url: "https://github.com/o/r/pull/42".into(),
        title: "feat: thing".into(),
        state: crate::github::PrStatus::Open,
        mergeable: crate::github::Mergeable::Mergeable,
        opened_at: Some(1_000),
        merged_at: None,
    };
    wm.set_repo_pr_snapshot(&id, &subdir, &open).unwrap();
    let repo = wm.agent(&id).unwrap().repos[0].clone();
    assert_eq!(repo.pr_number, Some(42));
    assert_eq!(
        repo.pr_url.as_deref(),
        Some("https://github.com/o/r/pull/42")
    );
    assert_eq!(repo.pr_title.as_deref(), Some("feat: thing"));
    assert_eq!(repo.pr_state.as_deref(), Some("open"));

    // Merge lands; a payload without opened_at keeps the earlier value.
    let merged = crate::github::PrState {
        state: crate::github::PrStatus::Merged,
        mergeable: crate::github::Mergeable::Unknown,
        opened_at: None,
        merged_at: Some(2_000),
        ..open
    };
    wm.set_repo_pr_snapshot(&id, &subdir, &merged).unwrap();
    let repo = wm.agent(&id).unwrap().repos[0].clone();
    assert_eq!(repo.pr_state.as_deref(), Some("merged"));
    let conn = db.lock();
    let (opened, merged_at): (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT pr_opened_at, pr_merged_at FROM worktrees WHERE workspace_id = ?1",
            [&id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        opened,
        Some(1_000),
        "COALESCE must keep the earlier opened_at"
    );
    assert_eq!(merged_at, Some(2_000));
}

#[test]
fn agent_status_transitions() {
    let db = test_db();
    let td = tempfile::tempdir().unwrap();
    let repo = init_repo(td.path());
    let wm = WorkspaceManager::new(db.clone());
    wm.add_workspace_repo(repo).unwrap();

    seed_repo(&db, "/r");
    let mut rec = new_agent_record(
        "test-id".into(),
        "a".into(),
        "claude".into(),
        mk_repo("/r"),
        "c".into(),
        AgentView::Custom,
    );
    let id = rec.id.clone();
    wm.add_agent(&mut rec).unwrap();

    // Fresh agent derives Idle (no durable disposition, no live process).
    assert_eq!(wm.agent(&id).unwrap().status, AgentStatus::Idle);

    // Stopping stamps a durable disposition → derives Stopped.
    wm.update_agent_status(&id, AgentStatus::Stopped, None)
        .unwrap();
    assert_eq!(wm.agent(&id).unwrap().status, AgentStatus::Stopped);

    // Resuming clears the stop disposition → back to Idle.
    wm.update_agent_status(&id, AgentStatus::Running, None)
        .unwrap();
    assert_eq!(wm.agent(&id).unwrap().status, AgentStatus::Idle);

    // Recording an error surfaces as Error (no live process at rest).
    wm.update_agent_status(&id, AgentStatus::Error, Some("boom".into()))
        .unwrap();
    let a = wm.agent(&id).unwrap();
    assert_eq!(a.status, AgentStatus::Error);
    assert_eq!(a.last_error.as_deref(), Some("boom"));

    // Resuming again clears the error → back to Idle.
    wm.update_agent_status(&id, AgentStatus::Spawning, None)
        .unwrap();
    let a = wm.agent(&id).unwrap();
    assert_eq!(a.status, AgentStatus::Idle);
    assert!(a.last_error.is_none());
}

#[test]
fn archive_then_restore_roundtrip() {
    let db = test_db();
    seed_repo(&db, "/some/repo");
    let wm = WorkspaceManager::new(db);

    let mut rec = new_agent_record(
        "yosemite".into(),
        "yosemite".into(),
        "claude".into(),
        mk_repo("/some/repo"),
        "do the thing".into(),
        AgentView::Custom,
    );
    let id = rec.id.clone();
    wm.add_agent(&mut rec).unwrap();

    let archive = ArchiveMetadata {
        archived_at: "2026-05-26T12:00:00+00:00".into(),
        repos: vec![ArchivedRepoSnapshot {
            repo_path: PathBuf::from("/some/repo"),
            subdir: "repo".into(),
            branch_name: Some("feat/do-the-thing".into()),
            branch_tip_sha: Some("deadbeef".into()),
            parent_branch: Some("main".into()),
            parent_branch_sha: Some("cafebabe".into()),
            diff_stats: DiffStats {
                additions: 12,
                deletions: 3,
            },
        }],
        diff_stats: DiffStats {
            additions: 12,
            deletions: 3,
        },
    };
    wm.archive_agent(&id, archive).unwrap();

    let a = wm.agent(&id).unwrap();
    assert!(a.archive.is_some());
    assert!(a.repos.is_empty());
    assert_eq!(a.status, AgentStatus::Stopped);
    // session_id preserved so restore can re-attach claude
    assert!(a.session_id.is_some());

    let arch = a.archive.unwrap();
    assert_eq!(arch.repos.len(), 1);
    assert_eq!(arch.repos[0].branch_tip_sha.as_deref(), Some("deadbeef"));
    assert_eq!(arch.repos[0].diff_stats.additions, 12);
    assert_eq!(arch.repos[0].diff_stats.deletions, 3);

    // Restore puts repos back and clears the archived/stopped
    // disposition, so the record derives Idle. (The supervisor's
    // restore path then drives the live spawn separately.)
    let restored = vec![TrackedRepo {
        repo_path: PathBuf::from("/some/repo"),
        subdir: "repo".into(),
        branch: Some("feat/do-the-thing".into()),
        parent_branch: Some("main".into()),
        base_sha: None,
        pr_number: None,
        pr_url: None,
        pr_title: None,
        pr_state: None,
        label: None,
    }];
    wm.restore_agent(&id, restored).unwrap();
    let a = wm.agent(&id).unwrap();
    assert!(a.archive.is_none());
    assert_eq!(a.repos.len(), 1);
    assert_eq!(a.status, AgentStatus::Idle);
}

#[test]
fn archived_agents_survive_reload_without_reconcile() {
    let db = test_db();
    seed_repo(&db, "/r");

    {
        let wm = WorkspaceManager::new(db.clone());
        let mut rec = new_agent_record(
            "yosemite".into(),
            "yosemite".into(),
            "claude".into(),
            mk_repo("/r"),
            "".into(),
            AgentView::Custom,
        );
        let id = rec.id.clone();
        wm.add_agent(&mut rec).unwrap();
        wm.archive_agent(
            &id,
            ArchiveMetadata {
                archived_at: "2026-05-26T12:00:00+00:00".into(),
                repos: vec![],
                diff_stats: DiffStats::default(),
            },
        )
        .unwrap();
    }

    // Second instance — archived agent should stay archived.
    let wm2 = WorkspaceManager::new(db);
    let cur = wm2.current().unwrap();
    assert_eq!(cur.agents.len(), 1);
    assert!(cur.agents[0].archive.is_some());
    assert_eq!(cur.agents[0].status, AgentStatus::Stopped);
}

// ── session event log ─────────────────────────────────────────────────

/// Seed a minimal workspace+session row and return (workspace_id, wm).
fn make_workspace_with_session(db: &Arc<Mutex<Connection>>) -> (String, WorkspaceManager) {
    let td = tempfile::tempdir().unwrap();
    let repo = init_repo(td.path());
    let repo_str = repo.to_str().unwrap().to_string();
    let wm = WorkspaceManager::new(db.clone());
    wm.add_workspace_repo(repo.clone()).unwrap();

    let mut rec = new_agent_record(
        uuid::Uuid::new_v4().to_string(),
        "evt-test".into(),
        "claude".into(),
        TrackedRepo {
            repo_path: repo,
            subdir: "repo".into(),
            branch: None,
            parent_branch: None,
            base_sha: None,
            pr_number: None,
            pr_url: None,
            pr_title: None,
            pr_state: None,
            label: None,
        },
        "task".into(),
        AgentView::Custom,
    );
    // add_agent needs the repo pre-seeded in repos; add_workspace_repo
    // handles that above. But we also need the repo in the repos table
    // for the checkout join — seed it explicitly so the lookup succeeds.
    seed_repo_path(db, &repo_str);
    wm.add_agent(&mut rec).unwrap();
    let id = rec.id.clone();
    (id, wm)
}

fn seed_repo_path(db: &Arc<Mutex<Connection>>, repo_path: &str) {
    // No-op if already there; used to guarantee the row exists for the
    // checkout FK before add_agent runs the lookup.
    let conn = db.lock();
    let path = std::path::Path::new(repo_path);
    let project_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    let project_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT OR IGNORE INTO projects (id, name, created_at) VALUES (?1, ?2, ?3)",
        rusqlite::params![project_id, project_name, now_millis()],
    )
    .unwrap();
    let pid: String = conn
        .query_row(
            "SELECT id FROM projects WHERE name = ?1",
            [project_name],
            |row| row.get(0),
        )
        .unwrap();
    let repo_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT OR IGNORE INTO repos (id, project_id, path, created_at) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![repo_id, pid, repo_path, now_millis()],
    )
    .unwrap();
}

// ── session record store (canonical) ──────────────────────────────────

#[test]
fn append_and_read_session_records_roundtrip() {
    let db = test_db();
    let (ws_id, wm) = make_workspace_with_session(&db);

    let body = serde_json::json!({"role": "user", "content": "hello"});
    let inserted = wm
        .append_session_records(
            &ws_id,
            "claude",
            "transcript",
            Some("1.2.3"),
            &[("uuid-1", &body)],
        )
        .unwrap();
    assert_eq!(inserted, 1);

    let records = wm.read_session_records(&ws_id).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].provider, "claude");
    assert_eq!(records[0].source, "transcript");
    assert_eq!(records[0].native_id, "uuid-1");
    assert_eq!(records[0].agent_version.as_deref(), Some("1.2.3"));
    assert_eq!(records[0].body, body);
    assert_eq!(records[0].seq, 1);
}

#[test]
fn append_session_record_is_idempotent_on_native_id() {
    let db = test_db();
    let (ws_id, wm) = make_workspace_with_session(&db);

    let first = serde_json::json!({"n": 1});
    let dup = serde_json::json!({"n": 2});
    assert_eq!(
        wm.append_session_records(&ws_id, "pi", "transcript", None, &[("id-a", &first)])
            .unwrap(),
        1
    );
    // Same (session, native_id) — must be ignored, original body retained.
    assert_eq!(
        wm.append_session_records(&ws_id, "pi", "transcript", None, &[("id-a", &dup)])
            .unwrap(),
        0
    );

    let records = wm.read_session_records(&ws_id).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].body, first);
    assert_eq!(records[0].agent_version, None);
}

#[test]
fn append_session_records_batches_in_one_pass_and_is_idempotent() {
    let db = test_db();
    let (ws_id, wm) = make_workspace_with_session(&db);

    let a = serde_json::json!({"n": 1});
    let b = serde_json::json!({"n": 2});
    let c = serde_json::json!({"n": 3});

    // First batch: all three land, seq contiguous in order.
    let inserted = wm
        .append_session_records(
            &ws_id,
            "claude",
            "transcript",
            None,
            &[("id-a", &a), ("id-b", &b), ("id-c", &c)],
        )
        .unwrap();
    assert_eq!(inserted, 3);

    let records = wm.read_session_records(&ws_id).unwrap();
    assert_eq!(
        records.iter().map(|r| r.seq).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(
        records
            .iter()
            .map(|r| r.native_id.as_str())
            .collect::<Vec<_>>(),
        vec!["id-a", "id-b", "id-c"],
    );

    // Re-running with two already-stored + one new inserts only the new one,
    // and seq stays contiguous (ignored dups don't burn a seq).
    let d = serde_json::json!({"n": 4});
    let inserted = wm
        .append_session_records(
            &ws_id,
            "claude",
            "transcript",
            None,
            &[("id-b", &b), ("id-c", &c), ("id-d", &d)],
        )
        .unwrap();
    assert_eq!(inserted, 1);

    let records = wm.read_session_records(&ws_id).unwrap();
    assert_eq!(
        records.iter().map(|r| r.seq).collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
    assert_eq!(records[3].native_id, "id-d");
    assert_eq!(records[3].body, d);
}

#[test]
fn ingest_offset_and_record_count_roundtrip() {
    let db = test_db();
    let (ws_id, wm) = make_workspace_with_session(&db);

    // Defaults before anything is ingested.
    assert_eq!(wm.session_ingest_offset(&ws_id).unwrap(), 0);
    assert_eq!(wm.session_record_count(&ws_id).unwrap(), 0);

    let a = serde_json::json!({"n": 1});
    let b = serde_json::json!({"n": 2});
    wm.append_session_records(
        &ws_id,
        "claude",
        "transcript",
        None,
        &[("x", &a), ("y", &b)],
    )
    .unwrap();
    // record_count tracks MAX(seq) — the start index for the next tail read.
    assert_eq!(wm.session_record_count(&ws_id).unwrap(), 2);

    wm.set_session_ingest_offset(&ws_id, 4096).unwrap();
    assert_eq!(wm.session_ingest_offset(&ws_id).unwrap(), 4096);
}

#[test]
fn session_records_seq_increments_in_order() {
    let db = test_db();
    let (ws_id, wm) = make_workspace_with_session(&db);

    let a = serde_json::json!({"a": 1});
    let b = serde_json::json!({"a": 2});
    wm.append_session_records(&ws_id, "pi", "transcript", None, &[("ln:0", &a)])
        .unwrap();
    wm.append_session_records(&ws_id, "pi", "transcript", None, &[("ln:1", &b)])
        .unwrap();

    let records = wm.read_session_records(&ws_id).unwrap();
    let seqs: Vec<i64> = records.iter().map(|r| r.seq).collect();
    assert_eq!(seqs, vec![1, 2]);
    assert_eq!(records[0].native_id, "ln:0");
    assert_eq!(records[1].native_id, "ln:1");
}

#[test]
fn read_session_records_empty_when_none() {
    let db = test_db();
    let (ws_id, wm) = make_workspace_with_session(&db);
    assert!(wm.read_session_records(&ws_id).unwrap().is_empty());
}

#[test]
fn append_session_record_to_workspace_with_no_session_is_noop() {
    let db = test_db();
    make_workspace_with_session(&db);
    let wm = WorkspaceManager::new(db.clone());
    // Unknown workspace id → no session → nothing inserted, read empty.
    let body = serde_json::json!({});
    let inserted = wm
        .append_session_records("no-such-ws", "claude", "transcript", None, &[("x", &body)])
        .unwrap();
    assert_eq!(inserted, 0);
    assert!(wm.read_session_records("no-such-ws").unwrap().is_empty());
}

#[test]
fn insert_and_read_user_turns_roundtrip() {
    let db = test_db();
    let (ws_id, wm) = make_workspace_with_session(&db);

    assert!(wm
        .insert_user_turn(&ws_id, "turn-1", "hello", &["/tmp/a.png".into()])
        .unwrap());

    let turns = wm.read_user_turns(&ws_id).unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].turn_id, "turn-1");
    assert_eq!(turns[0].seq, 1);
    assert_eq!(turns[0].text, "hello");
    assert_eq!(turns[0].attachments, vec!["/tmp/a.png".to_string()]);
    assert_eq!(turns[0].native_id, None);
    // Timing is unset until the turn starts/ends.
    assert_eq!(turns[0].started_at, None);
    assert_eq!(turns[0].ended_at, None);
}

#[test]
fn user_turn_timing_start_then_end() {
    let db = test_db();
    let (ws_id, wm) = make_workspace_with_session(&db);
    wm.insert_user_turn(&ws_id, "turn-1", "hello", &[]).unwrap();

    // Start stamps started_at; end stamps ended_at on the open turn.
    wm.mark_user_turn_started("turn-1", 1000).unwrap();
    let started = wm.read_user_turns(&ws_id).unwrap()[0].started_at;
    assert_eq!(started, Some(1000));
    assert_eq!(wm.read_user_turns(&ws_id).unwrap()[0].ended_at, None);

    let closed = wm
        .mark_user_turn_ended(&ws_id)
        .unwrap()
        .expect("open turn closed");
    let turn = wm.read_user_turns(&ws_id).unwrap().remove(0);
    assert_eq!(turn.started_at, started, "start clock not reset by end");
    assert!(
        turn.ended_at >= turn.started_at,
        "ended_at after started_at"
    );
    assert_eq!(
        closed.duration_ms,
        turn.ended_at.unwrap() - turn.started_at.unwrap(),
        "duration_ms matches stored ended_at − started_at"
    );
    assert_eq!(closed.record_count, 0, "no records ingested in this test");
}

#[test]
fn mark_user_turn_started_is_idempotent() {
    let db = test_db();
    let (ws_id, wm) = make_workspace_with_session(&db);
    wm.insert_user_turn(&ws_id, "turn-1", "hello", &[]).unwrap();

    wm.mark_user_turn_started("turn-1", 1000).unwrap();
    let first = wm.read_user_turns(&ws_id).unwrap()[0].started_at;
    // A delivery retry re-stamps — but the guard keeps the original clock.
    wm.mark_user_turn_started("turn-1", 2000).unwrap();
    assert_eq!(wm.read_user_turns(&ws_id).unwrap()[0].started_at, first);
}

#[test]
fn mark_user_turn_ended_skips_turns_that_never_started() {
    let db = test_db();
    let (ws_id, wm) = make_workspace_with_session(&db);
    // A row with no started_at (e.g. a never-delivered turn, or the resting
    // Idle emitted at spawn) must not get an ended_at.
    wm.insert_user_turn(&ws_id, "turn-1", "hello", &[]).unwrap();
    assert!(
        wm.mark_user_turn_ended(&ws_id).unwrap().is_none(),
        "no open turn to close"
    );
    assert_eq!(wm.read_user_turns(&ws_id).unwrap()[0].ended_at, None);
}

#[test]
fn insert_user_turn_is_idempotent_on_turn_id() {
    let db = test_db();
    let (ws_id, wm) = make_workspace_with_session(&db);

    assert!(wm.insert_user_turn(&ws_id, "turn-1", "first", &[]).unwrap());
    // Same turn_id (a send retry) — ignored, original retained, no new row.
    assert!(!wm
        .insert_user_turn(&ws_id, "turn-1", "second", &[])
        .unwrap());

    let turns = wm.read_user_turns(&ws_id).unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].text, "first");
}

#[test]
fn associate_pending_user_turns_matches_attachment_path_then_text() {
    let db = test_db();
    let (ws_id, wm) = make_workspace_with_session(&db);

    // Two outgoing turns: one with an attachment, one plain.
    wm.insert_user_turn(&ws_id, "t1", "look at this", &["/tmp/diagram.png".into()])
        .unwrap();
    wm.insert_user_turn(&ws_id, "t2", "now refactor", &[])
        .unwrap();

    // Transcript user-message records as the agent logged them (attachment
    // turn carries the injected reference line; plain turn is just text).
    let rec_a = serde_json::json!({"role": "user", "text": "look at this\nAttached file: /tmp/diagram.png"});
    let rec_b = serde_json::json!({"role": "user", "text": "now refactor"});
    wm.append_session_records(
        &ws_id,
        "claude",
        "transcript",
        None,
        &[("rec-A", &rec_a), ("rec-B", &rec_b)],
    )
    .unwrap();

    let n = wm.associate_pending_user_turns(&ws_id).unwrap();
    assert_eq!(n, 2);

    let turns = wm.read_user_turns(&ws_id).unwrap();
    assert_eq!(turns[0].native_id.as_deref(), Some("rec-A"));
    assert_eq!(turns[1].native_id.as_deref(), Some("rec-B"));

    // Idempotent: re-running associates nothing new.
    assert_eq!(wm.associate_pending_user_turns(&ws_id).unwrap(), 0);
}

#[test]
fn associate_matches_multiline_text() {
    let db = test_db();
    let (ws_id, wm) = make_workspace_with_session(&db);

    // Multi-line message — the transcript stores it JSON-escaped (\n → \\n).
    let text = "Please do this:\n- step one\n- step two";
    wm.insert_user_turn(&ws_id, "t1", text, &[]).unwrap();

    let rec_body = serde_json::json!({"role": "user", "text": text});
    wm.append_session_records(
        &ws_id,
        "claude",
        "transcript",
        None,
        &[("rec-1", &rec_body)],
    )
    .unwrap();

    let n = wm.associate_pending_user_turns(&ws_id).unwrap();
    assert_eq!(n, 1);
    let turns = wm.read_user_turns(&ws_id).unwrap();
    assert_eq!(turns[0].native_id.as_deref(), Some("rec-1"));
}

#[test]
fn associate_leaves_unmatched_turn_pending() {
    let db = test_db();
    let (ws_id, wm) = make_workspace_with_session(&db);

    // Sent, but the agent never logged it (call failed) — no transcript row.
    wm.insert_user_turn(&ws_id, "t1", "never delivered", &[])
        .unwrap();
    assert_eq!(wm.associate_pending_user_turns(&ws_id).unwrap(), 0);

    let turns = wm.read_user_turns(&ws_id).unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].native_id, None); // still pending → renders standalone
}

// ── mid-turn follow-up messages (coalesced delivery + live injection) ──

#[test]
fn coalesced_follow_ups_persist_one_row_that_matches_one_record() {
    // Per-turn flush (A5-A): N queued follow-ups coalesce into ONE prompt,
    // delivered as one turn → one transcript record → one user_turn row
    // that matches 1:1. No orphans.
    let db = test_db();
    let (ws_id, wm) = make_workspace_with_session(&db);

    wm.insert_user_turn(&ws_id, "t-coalesced", "first\n\nsecond", &[])
        .unwrap();

    let rec = serde_json::json!({"role": "user", "text": "first\n\nsecond"});
    wm.append_session_records(&ws_id, "codex", "transcript", None, &[("rec-1", &rec)])
        .unwrap();

    assert_eq!(wm.associate_pending_user_turns(&ws_id).unwrap(), 1);
    let turns = wm.read_user_turns(&ws_id).unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].native_id.as_deref(), Some("rec-1"));
}

#[test]
fn live_injected_follow_ups_each_match_their_own_record() {
    // Claude live: each injected message is its own transcript user record,
    // so two follow-ups inside one turn window match N→N (no coalescing).
    let db = test_db();
    let (ws_id, wm) = make_workspace_with_session(&db);

    wm.insert_user_turn(&ws_id, "t1", "original", &[]).unwrap();
    wm.insert_user_turn(&ws_id, "t2", "actually also do X", &[])
        .unwrap();

    let rec_a = serde_json::json!({"role": "user", "text": "original"});
    let rec_b = serde_json::json!({"role": "user", "text": "actually also do X"});
    wm.append_session_records(
        &ws_id,
        "claude",
        "transcript",
        None,
        &[("rec-A", &rec_a), ("rec-B", &rec_b)],
    )
    .unwrap();

    assert_eq!(wm.associate_pending_user_turns(&ws_id).unwrap(), 2);
    let turns = wm.read_user_turns(&ws_id).unwrap();
    assert_eq!(turns[0].native_id.as_deref(), Some("rec-A"));
    assert_eq!(turns[1].native_id.as_deref(), Some("rec-B"));
}

#[test]
fn per_message_rows_orphan_against_a_coalesced_record() {
    // Guards the A5-A decision: if a coalesced delivery (one merged record)
    // were persisted as N separate rows instead of one, the claim-set lets
    // only the first row match and the rest orphan forever. This is the bug
    // we avoid by persisting a single coalesced row — documented here so a
    // future change back to per-message rows fails loudly.
    let db = test_db();
    let (ws_id, wm) = make_workspace_with_session(&db);

    wm.insert_user_turn(&ws_id, "t1", "first", &[]).unwrap();
    wm.insert_user_turn(&ws_id, "t2", "second", &[]).unwrap();

    let rec = serde_json::json!({"role": "user", "text": "first\n\nsecond"});
    wm.append_session_records(&ws_id, "codex", "transcript", None, &[("rec-1", &rec)])
        .unwrap();

    // Only one row can claim the single record; the other stays pending.
    assert_eq!(wm.associate_pending_user_turns(&ws_id).unwrap(), 1);
    let pending = wm
        .read_user_turns(&ws_id)
        .unwrap()
        .into_iter()
        .filter(|t| t.native_id.is_none())
        .count();
    assert_eq!(
        pending, 1,
        "the unclaimed row orphans — hence we coalesce to one row"
    );
}

#[test]
fn allocate_subdir_handles_collision() {
    let used = vec!["luxembourg".to_string()];
    assert_eq!(
        allocate_repo_subdir(Path::new("/foo/luxembourg"), &used),
        "luxembourg-2"
    );
    let used2 = vec!["luxembourg".to_string(), "luxembourg-2".to_string()];
    assert_eq!(
        allocate_repo_subdir(Path::new("/bar/luxembourg"), &used2),
        "luxembourg-3"
    );
    assert_eq!(
        allocate_repo_subdir(Path::new("/foo/fresh"), &used),
        "fresh"
    );
}

#[test]
fn occupied_checkout_dirs_lists_only_subdirs() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("kilimanjaro")).unwrap();
    std::fs::create_dir_all(root.path().join("seychelles")).unwrap();
    // A stray file (not a dir) must not be reported as an occupied name.
    std::fs::write(root.path().join("notes.txt"), b"x").unwrap();

    let found = occupied_checkout_dirs_in(root.path());
    assert_eq!(found.len(), 2);
    assert!(found.contains("kilimanjaro"));
    assert!(found.contains("seychelles"));
    assert!(!found.contains("notes.txt"));
}

#[test]
fn occupied_checkout_dirs_empty_when_root_missing() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("does-not-exist");
    assert!(occupied_checkout_dirs_in(&missing).is_empty());
}

/// Mark a workspace archived directly (tests don't go through the full
/// archive flow, which needs live checkouts on disk).
fn mark_archived(db: &Arc<Mutex<Connection>>, id: &str) {
    let conn = db.lock();
    conn.execute(
        "UPDATE workspaces SET archived_at = ?1 WHERE id = ?2",
        rusqlite::params![now_millis(), id],
    )
    .unwrap();
}

#[test]
fn add_agent_reuses_archived_name() {
    let db = test_db();
    seed_repo(&db, "/r");
    let wm = WorkspaceManager::new(db.clone());

    let mut first = new_agent_record(
        "kilimanjaro".into(),
        "first".into(),
        "claude".into(),
        mk_repo("/r"),
        String::new(),
        AgentView::Custom,
    );
    wm.add_agent(&mut first).unwrap();
    mark_archived(&db, "kilimanjaro");

    // Recycling the freed name must succeed — the archived row is evicted
    // rather than tripping the primary-key constraint.
    let mut second = new_agent_record(
        "kilimanjaro".into(),
        "second".into(),
        "claude".into(),
        mk_repo("/r"),
        String::new(),
        AgentView::Custom,
    );
    wm.add_agent(&mut second).unwrap();

    let conn = db.lock();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM workspaces WHERE id = 'kilimanjaro'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "exactly one row should remain");
    let (name, archived): (String, Option<i64>) = conn
        .query_row(
            "SELECT name, archived_at FROM workspaces WHERE id = 'kilimanjaro'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(name, "second", "the live agent should have replaced it");
    assert!(archived.is_none(), "the recycled agent must be live");
}

#[test]
fn add_agent_does_not_evict_a_live_name_clash() {
    let db = test_db();
    seed_repo(&db, "/r");
    let wm = WorkspaceManager::new(db.clone());

    let mut first = new_agent_record(
        "kilimanjaro".into(),
        "first".into(),
        "claude".into(),
        mk_repo("/r"),
        String::new(),
        AgentView::Custom,
    );
    wm.add_agent(&mut first).unwrap();

    // A *live* id clash is a real bug: the INSERT must fail loudly rather
    // than clobber the running agent.
    let mut clash = new_agent_record(
        "kilimanjaro".into(),
        "second".into(),
        "claude".into(),
        mk_repo("/r"),
        String::new(),
        AgentView::Custom,
    );
    assert!(wm.add_agent(&mut clash).is_err());

    let conn = db.lock();
    let name: String = conn
        .query_row(
            "SELECT name FROM workspaces WHERE id = 'kilimanjaro'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(name, "first", "the original live agent must survive");
}

#[test]
fn allocate_agent_id_excludes_archived_from_reservation() {
    let db = test_db();
    seed_repo(&db, "/r");
    let wm = WorkspaceManager::new(db.clone());

    // Fill the whole pool with archived agents, then one live agent.
    for place in names::PLACES {
        let mut rec = new_agent_record(
            (*place).into(),
            (*place).into(),
            "claude".into(),
            mk_repo("/r"),
            String::new(),
            AgentView::Custom,
        );
        wm.add_agent(&mut rec).unwrap();
        mark_archived(&db, place);
    }

    // Every pool name is archived (so all are reusable) — the allocator
    // should hand back a bare pool name, never a "-N" exhaustion suffix.
    let id = wm.allocate_agent_id().unwrap();
    assert!(
        names::PLACES.contains(&id.as_str()),
        "expected a reusable pool name, got {id}"
    );
}
