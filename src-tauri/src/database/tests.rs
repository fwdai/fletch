use parking_lot::Mutex;
use rusqlite::Connection;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;

use crate::database::connection::{
    get_migrations, migrate_legacy_db_name, open_db, quarantine_orphaned_wal, MIGRATIONS,
};
use crate::database::*;
use crate::error::Error;

fn test_db() -> Arc<Mutex<Connection>> {
    let dir = tempfile::tempdir().unwrap();
    init(dir.path()).unwrap()
}

fn backup_files(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".db.bak-"))
        .collect()
}

#[test]
fn legacy_db_is_renamed_on_init_and_data_is_preserved() {
    let dir = tempfile::tempdir().unwrap();
    // Seed a real DB under the legacy name, then write a marker row.
    let mut conn = open_db(&dir.path().join(LEGACY_DB_FILENAME)).unwrap();
    get_migrations().to_latest(&mut conn).unwrap();
    db_insert(&conn, "projects", json!({ "name": "marker" })).unwrap();
    drop(conn);

    init(dir.path()).unwrap();

    // Legacy file is gone, new file exists, and the marker row survived.
    assert!(!dir.path().join(LEGACY_DB_FILENAME).exists());
    assert!(dir.path().join(DB_FILENAME).exists());
    let conn = open_db(&dir.path().join(DB_FILENAME)).unwrap();
    let rows = db_select(&conn, "projects", json!({ "name": "marker" })).unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn interrupted_migration_reruns_and_keeps_wal_with_its_main_file() {
    // Simulate a crash mid-migration: the sidecar was already renamed to the
    // new name, but the main file is still under the legacy name (the main
    // file is moved last). The guard must still fire and finish the move,
    // leaving `data.db` paired with the `data.db-wal` that carries the
    // committed-but-uncheckpointed rows.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(LEGACY_DB_FILENAME), b"main").unwrap();
    std::fs::write(dir.path().join(format!("{DB_FILENAME}-wal")), b"wal").unwrap();

    migrate_legacy_db_name(dir.path()).unwrap();

    assert!(!dir.path().join(LEGACY_DB_FILENAME).exists());
    assert_eq!(
        std::fs::read(dir.path().join(DB_FILENAME)).unwrap(),
        b"main"
    );
    assert_eq!(
        std::fs::read(dir.path().join(format!("{DB_FILENAME}-wal"))).unwrap(),
        b"wal"
    );
}

#[test]
fn orphaned_wal_without_main_is_quarantined_not_replayed() {
    // WAL/SHM present with no main file — the exact debris an interrupted
    // rename leaves behind. init() must set them aside and start fresh, never
    // letting SQLite recover a main database from the abandoned WAL.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(format!("{DB_FILENAME}-wal")), b"orphan-wal").unwrap();
    std::fs::write(dir.path().join(format!("{DB_FILENAME}-shm")), b"orphan-shm").unwrap();

    init(dir.path()).unwrap();

    // A genuinely fresh db exists and the orphans were preserved as backups.
    assert!(dir.path().join(DB_FILENAME).exists());
    let orphaned: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .filter(|n| n.contains(".orphaned-"))
        .collect();
    assert_eq!(orphaned.len(), 2);
}

#[test]
fn stray_legacy_wal_without_its_main_is_not_migrated_or_resurrected() {
    // A crash mid-recovery can leave quorum.db-wal behind after quorum.db
    // itself was already moved aside. With no legacy main file, migration
    // must not fire (there is nothing to resurrect) and init opens a clean
    // data.db rather than reviving the abandoned database.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(format!("{LEGACY_DB_FILENAME}-wal")),
        b"stray",
    )
    .unwrap();

    init(dir.path()).unwrap();

    assert!(dir.path().join(DB_FILENAME).exists());
    assert!(!dir.path().join(LEGACY_DB_FILENAME).exists());
}

#[test]
fn quarantine_orphaned_wal_is_noop_when_main_present() {
    // A WAL alongside its main file is normal SQLite state — leave it alone.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(DB_FILENAME), b"main").unwrap();
    std::fs::write(dir.path().join(format!("{DB_FILENAME}-wal")), b"wal").unwrap();

    quarantine_orphaned_wal(dir.path()).unwrap();

    assert!(dir.path().join(format!("{DB_FILENAME}-wal")).exists());
}

#[test]
fn migrate_legacy_db_name_is_noop_when_new_file_present() {
    let dir = tempfile::tempdir().unwrap();
    // Both files present (e.g. a stray legacy file after migration): the new
    // one must win untouched, and the legacy file is left alone.
    std::fs::write(dir.path().join(LEGACY_DB_FILENAME), b"legacy").unwrap();
    std::fs::write(dir.path().join(DB_FILENAME), b"current").unwrap();

    migrate_legacy_db_name(dir.path()).unwrap();

    assert_eq!(
        std::fs::read(dir.path().join(DB_FILENAME)).unwrap(),
        b"current"
    );
    assert!(dir.path().join(LEGACY_DB_FILENAME).exists());
}

#[test]
fn init_errors_when_schema_is_from_a_newer_build() {
    let dir = tempfile::tempdir().unwrap();
    init(dir.path()).unwrap();
    // Simulate an app downgrade: a newer build left user_version ahead of
    // the migrations this binary knows about.
    let conn = Connection::open(dir.path().join(DB_FILENAME)).unwrap();
    conn.pragma_update(None, "user_version", (MIGRATIONS.len() + 5) as i64)
        .unwrap();
    drop(conn);
    assert!(matches!(init(dir.path()), Err(Error::SchemaTooNew)));
}

#[test]
fn upgrading_an_older_schema_backs_it_up_first() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open_db(&dir.path().join(DB_FILENAME)).unwrap();
    get_migrations().to_version(&mut conn, 1).unwrap(); // valid schema at v1
    drop(conn);
    init(dir.path()).unwrap(); // v1 -> latest, must back up first

    let backups = backup_files(dir.path());
    assert_eq!(backups.len(), 1);
    // The snapshot must be a complete, readable DB frozen at the pre-upgrade
    // version — proving the online backup captured real content, not a
    // truncated copy.
    let backup = Connection::open(dir.path().join(&backups[0])).unwrap();
    let version: i64 = backup
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 1);
}

/// Schema version at which `worktree_prs` (0025) exists. Pinned rather than
/// derived from `MIGRATIONS.len()`: every later migration would otherwise shift
/// the "one version short of it" baseline these tests upgrade from.
const V_WORKTREE_PRS: usize = 25;

/// The PR-history migration seeds itself from the bindings already on disk, so
/// an upgrading install starts with history rather than with today. Without the
/// backfill the Project Pulse "PRs opened" series — which now reads the log —
/// would blank every past day until each PR happened to be re-fetched. A binding
/// with no persisted state carries no renderable snapshot and is skipped.
#[test]
fn pr_history_migration_backfills_existing_bindings() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open_db(&dir.path().join(DB_FILENAME)).unwrap();
    // One migration short of the history table: the state an existing install
    // upgrades from.
    let before = V_WORKTREE_PRS - 1;
    get_migrations().to_version(&mut conn, before).unwrap();

    conn.execute_batch(
        "INSERT INTO projects (id, name, created_at) VALUES ('p', 'proj', 0);
         INSERT INTO repos (id, project_id, path, created_at) VALUES ('r', 'p', '/r', 0);
         INSERT INTO workspaces (id, project_id, name, created_at) VALUES ('w', 'p', 'ws', 0);
         INSERT INTO worktrees (id, workspace_id, repo_id, subdir, created_at,
                                pr_number, pr_url, pr_title, pr_state, pr_opened_at, pr_merged_at)
              VALUES ('wt1', 'w', 'r', 'repo', 0,
                      42, 'https://x/42', 'feat: landed', 'merged', 1000, 2000);
         INSERT INTO workspaces (id, project_id, name, created_at) VALUES ('w2', 'p', 'ws2', 0);
         INSERT INTO worktrees (id, workspace_id, repo_id, subdir, created_at, pr_number, pr_state)
              VALUES ('wt2', 'w2', 'r', 'repo', 0, 99, NULL);",
    )
    .unwrap();
    drop(conn);

    init(dir.path()).unwrap();

    let conn = Connection::open(dir.path().join(DB_FILENAME)).unwrap();
    let (n, url, state, opened, merged): (i64, String, String, i64, i64) = conn
        .query_row(
            "SELECT number, url, state, opened_at, merged_at FROM worktree_prs
              WHERE workspace_id = 'w' AND subdir = 'repo'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(
        (n, state.as_str(), opened, merged),
        (42, "merged", 1000, 2000)
    );
    assert_eq!(url, "https://x/42");

    // The state-less binding is not invented into history.
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM worktree_prs WHERE workspace_id = 'w2'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "no persisted state means no renderable snapshot");
}

/// The roadmap table (0026) lands on an existing install with the shape the DAO
/// assumes: per-project unique codes, cascade from the owning project, and no
/// generic-CRUD access (typed commands only, like the `wf_*` tables).
#[test]
fn roadmap_items_migration_adds_the_table_with_its_constraints() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open_db(&dir.path().join(DB_FILENAME)).unwrap();
    // The version an existing install upgrades from — the table must be created,
    // not assumed to exist.
    get_migrations()
        .to_version(&mut conn, V_WORKTREE_PRS)
        .unwrap();
    drop(conn);

    let db = init(dir.path()).unwrap();
    let conn = db.lock();
    let pid = make_project(&conn);

    let insert = |id: &str, code: &str| {
        conn.execute(
            "INSERT INTO roadmap_items (id, project_id, code, title, horizon, status,
                                        created_at, updated_at)
             VALUES (?1, ?2, ?3, 'a title', 'now', 'open', 0, 0)",
            rusqlite::params![id, pid, code],
        )
    };
    insert("i1", "PRJ-100").unwrap();
    // Codes are the identity the board, the PM and PR titles all quote — one
    // project can never hold two of the same.
    assert!(
        insert("i2", "PRJ-100").is_err(),
        "UNIQUE(project_id, code) must reject a duplicate code"
    );
    // Defaults the DAO relies on rather than writing explicitly.
    let (why, source): (String, String) = conn
        .query_row(
            "SELECT why, source FROM roadmap_items WHERE id = 'i1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((why.as_str(), source.as_str()), ("", "user"));

    // Deleting the project takes its roadmap with it — a roadmap outliving its
    // project would be unreachable rows that still consume codes.
    db_delete(&conn, "projects", json!({ "where": { "id": pid } })).unwrap();
    let left: i64 = conn
        .query_row("SELECT COUNT(*) FROM roadmap_items", [], |r| r.get(0))
        .unwrap();
    assert_eq!(left, 0);

    // Writes go through `roadmap_*` commands so code allocation, JSON
    // marshalling and the row event can't be bypassed.
    assert!(
        db_select(&conn, "roadmap_items", json!({})).is_err(),
        "roadmap_items must stay off the generic CRUD allow-list"
    );
}

/// Schema version at which `roadmap_items` (0026) exists — the version an
/// install upgrading into the workspace-purpose migration comes from. Pinned
/// like `V_WORKTREE_PRS`, for the same reason.
const V_ROADMAP_ITEMS: usize = 26;

/// `workspaces.purpose` (0027) lands on an existing install as a nullable
/// column: every workspace already on disk is an ordinary sidebar agent, and
/// stays one. Only rows written with a tag are hidden from the sidebar.
#[test]
fn workspace_purpose_migration_leaves_existing_workspaces_untagged() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open_db(&dir.path().join(DB_FILENAME)).unwrap();
    get_migrations()
        .to_version(&mut conn, V_ROADMAP_ITEMS)
        .unwrap();
    conn.execute_batch(
        "INSERT INTO projects (id, name, created_at) VALUES ('p', 'proj', 0);
         INSERT INTO workspaces (id, project_id, name, created_at) VALUES ('w', 'p', 'ws', 0);",
    )
    .unwrap();
    drop(conn);

    init(dir.path()).unwrap();

    let conn = Connection::open(dir.path().join(DB_FILENAME)).unwrap();
    let existing: Option<String> = conn
        .query_row("SELECT purpose FROM workspaces WHERE id = 'w'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        existing, None,
        "an existing workspace must stay an ordinary sidebar agent"
    );

    // A tagged chat is an ordinary row otherwise — same table, same cascade.
    conn.execute(
        "INSERT INTO workspaces (id, project_id, name, created_at, purpose)
         VALUES ('c', 'p', 'chat', 0, 'roadmap-pm')",
        [],
    )
    .unwrap();
    let tagged: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM workspaces WHERE purpose = 'roadmap-pm'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tagged, 1);
}

#[test]
fn fresh_init_creates_no_backup() {
    let dir = tempfile::tempdir().unwrap();
    init(dir.path()).unwrap();
    assert!(backup_files(dir.path()).is_empty());
}

fn make_project(conn: &Connection) -> String {
    db_insert(conn, "projects", json!({ "name": "test-project" })).unwrap()
}

fn make_workspace(conn: &Connection, project_id: &str) -> String {
    db_insert(
        conn,
        "workspaces",
        json!({ "project_id": project_id, "name": "test-workspace" }),
    )
    .unwrap()
}

#[test]
fn insert_and_select() {
    let db = test_db();
    let conn = db.lock();
    let pid = make_project(&conn);

    let id = db_insert(
        &conn,
        "workspaces",
        json!({ "project_id": pid, "name": "test-workspace" }),
    )
    .unwrap();

    let rows = db_select(&conn, "workspaces", json!({ "where": { "id": id } })).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "test-workspace");
    assert_eq!(rows[0]["task"], "");
}

#[test]
fn update_and_delete() {
    let db = test_db();
    let conn = db.lock();
    let pid = make_project(&conn);
    let id = make_workspace(&conn, &pid);

    let changed = db_update(
        &conn,
        "workspaces",
        json!({ "where": { "id": id } }),
        json!({ "name": "renamed" }),
    )
    .unwrap();
    assert_eq!(changed, 1);

    let rows = db_select(&conn, "workspaces", json!({ "where": { "id": id } })).unwrap();
    assert_eq!(rows[0]["name"], "renamed");

    let deleted = db_delete(&conn, "workspaces", json!({ "where": { "id": id } })).unwrap();
    assert_eq!(deleted, 1);

    let count = db_count(&conn, "workspaces", json!({})).unwrap();
    assert_eq!(count, 0);
}

#[test]
fn skills_and_mcp_servers_crud_through_generic_layer() {
    // The Settings panes CRUD these tables via the generic db_* commands
    // (src/storage/skills.ts, mcpServers.ts). They must be allow-listed, or
    // every insert/select/update/delete fails with "unknown table". This is
    // the regression guard for that gate.
    let db = test_db();
    let conn = db.lock();

    // `updated_at` is NOT NULL with no default; the storage layer supplies it
    // (skills.ts / mcpServers.ts), so the test mirrors that.
    let sk = db_insert(
        &conn,
        "skills",
        json!({ "name": "code-review", "description": "review well", "body": "# Review",
                    "updated_at": 0 }),
    )
    .unwrap();
    let rows = db_select(&conn, "skills", json!({ "where": { "id": sk } })).unwrap();
    assert_eq!(rows[0]["name"], "code-review");

    let mcp = db_insert(
        &conn,
        "mcp_servers",
        json!({ "name": "gh", "transport": "stdio", "command": "npx -y gh-mcp",
                    "updated_at": 0 }),
    )
    .unwrap();
    db_update(
        &conn,
        "mcp_servers",
        json!({ "where": { "id": mcp } }),
        json!({ "url": "https://mcp.example" }),
    )
    .unwrap();
    let rows = db_select(&conn, "mcp_servers", json!({ "where": { "id": mcp } })).unwrap();
    assert_eq!(rows[0]["url"], "https://mcp.example");

    db_delete(&conn, "skills", json!({ "where": { "id": sk } })).unwrap();
    db_delete(&conn, "mcp_servers", json!({ "where": { "id": mcp } })).unwrap();
    assert_eq!(db_count(&conn, "skills", json!({})).unwrap(), 0);
    assert_eq!(db_count(&conn, "mcp_servers", json!({})).unwrap(), 0);
}

#[test]
fn rejects_unknown_table() {
    let db = test_db();
    let conn = db.lock();
    assert!(db_select(&conn, "evil_table", json!({})).is_err());
}

#[test]
fn rejects_invalid_column() {
    let db = test_db();
    let conn = db.lock();
    assert!(db_select(
        &conn,
        "workspaces",
        json!({ "where": { "id; DROP TABLE workspaces": "x" } })
    )
    .is_err());
}

#[test]
fn db_query_rejects_non_select() {
    let db = test_db();
    let conn = db.lock();
    assert!(db_query(&conn, "DELETE FROM workspaces", vec![]).is_err());
    assert!(db_query(&conn, "DROP TABLE workspaces", vec![]).is_err());
}

#[test]
fn auto_generates_uuid_and_timestamp() {
    let db = test_db();
    let conn = db.lock();
    let pid = make_project(&conn);

    let id = db_insert(
        &conn,
        "workspaces",
        json!({ "project_id": pid, "name": "auto" }),
    )
    .unwrap();

    assert!(!id.is_empty());
    assert!(uuid::Uuid::parse_str(&id).is_ok());

    let rows = db_select(&conn, "workspaces", json!({ "where": { "id": id } })).unwrap();
    let created = rows[0]["created_at"].as_i64().unwrap();
    assert!(created > 0);
}

#[test]
fn null_where_clause() {
    // sessions has last_error — verify null WHERE matching using that column
    let db = test_db();
    let conn = db.lock();
    let pid = make_project(&conn);
    let ws_id = make_workspace(&conn, &pid);

    db_insert(
        &conn,
        "sessions",
        json!({ "workspace_id": ws_id, "provider": "claude", "last_error": "boom" }),
    )
    .unwrap();

    db_insert(
        &conn,
        "sessions",
        json!({ "workspace_id": ws_id, "provider": "claude" }),
    )
    .unwrap();

    let rows = db_select(
        &conn,
        "sessions",
        json!({ "where": { "last_error": null } }),
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["provider"], "claude");
    assert!(rows[0]["last_error"].is_null());
}

#[test]
fn schema_has_split_entities() {
    let db = test_db();
    let conn = db.lock();
    let names: std::collections::HashSet<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table'")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    for t in [
        "workspaces",
        "sessions",
        "worktrees",
        "session_records",
        "repos",
        "projects",
        "project_settings",
        "accounts",
        "settings",
    ] {
        assert!(names.contains(t), "missing table {t}");
    }
    assert!(
        !names.contains("agents"),
        "stale table agents still present"
    );
    assert!(
        !names.contains("messages"),
        "stale table messages still present"
    );
    assert!(
        !names.contains("session_events"),
        "retired table session_events still present"
    );
}

#[test]
fn workspace_hierarchy_cascades() {
    let db = test_db();
    let conn = db.lock();
    let pid = db_insert(&conn, "projects", json!({ "name": "p" })).unwrap();
    let ws = db_insert(
        &conn,
        "workspaces",
        json!({ "project_id": pid, "name": "halifax" }),
    )
    .unwrap();
    let sess = db_insert(
        &conn,
        "sessions",
        json!({ "workspace_id": ws, "provider": "claude" }),
    )
    .unwrap();
    // session_records is written via dedicated functions, not the generic
    // layer, so it isn't in ALLOWED_TABLES — insert/count with raw SQL.
    conn.execute(
            "INSERT INTO session_records (session_id, seq, provider, source, native_id, body, created_at)
             VALUES (?1, 1, 'claude', 'transcript', 'x', '{}', 0)",
            [&sess],
        )
        .unwrap();
    db_delete(&conn, "workspaces", json!({ "where": { "id": ws } })).unwrap();
    assert_eq!(db_count(&conn, "sessions", json!({})).unwrap(), 0);
    let records: i64 = conn
        .query_row("SELECT COUNT(*) FROM session_records", [], |r| r.get(0))
        .unwrap();
    assert_eq!(records, 0);
}

#[test]
fn upsert_settings() {
    let db = test_db();
    let conn = db.lock();

    db_upsert(
        &conn,
        "settings",
        json!({ "key": "theme", "value": "dark" }),
        "key",
    )
    .unwrap();

    let rows = db_select(&conn, "settings", json!({ "where": { "key": "theme" } })).unwrap();
    assert_eq!(rows[0]["value"], "dark");

    db_upsert(
        &conn,
        "settings",
        json!({ "key": "theme", "value": "light" }),
        "key",
    )
    .unwrap();

    let rows = db_select(&conn, "settings", json!({ "where": { "key": "theme" } })).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["value"], "light");
}

#[test]
fn load_agent_bin_overrides_picks_prefixed_keys_and_skips_blanks() {
    let db = test_db();
    let conn = db.lock();

    let set = |k: &str, v: &str| {
        db_upsert(&conn, "settings", json!({ "key": k, "value": v }), "key").unwrap();
    };
    set("theme", "dark"); // unrelated setting, must be ignored
    set("agent_bin_path_claude", "/opt/homebrew/bin/claude");
    set("agent_bin_path_cursor", "~/bin/cursor-agent");
    set("agent_bin_path_codex", "   "); // blank → cleared, must be skipped

    let map = load_agent_bin_overrides(&conn);
    assert_eq!(map.len(), 2);
    assert_eq!(map["claude"], "/opt/homebrew/bin/claude");
    assert_eq!(map["cursor"], "~/bin/cursor-agent");
    assert!(!map.contains_key("codex"));
    assert!(!map.contains_key("theme"));
}

#[test]
fn project_repo_agent_worktree_hierarchy() {
    let db = test_db();
    let conn = db.lock();

    let pid = db_insert(&conn, "projects", json!({ "name": "my-app" })).unwrap();

    let repo_id = db_insert(
        &conn,
        "repos",
        json!({ "project_id": pid, "path": "/code/my-app" }),
    )
    .unwrap();

    let ws_id = db_insert(
        &conn,
        "workspaces",
        json!({ "project_id": pid, "name": "olympus" }),
    )
    .unwrap();

    db_insert(
        &conn,
        "worktrees",
        json!({
            "workspace_id": ws_id,
            "repo_id": repo_id,
            "subdir": "my-app",
            "parent_branch": "main"
        }),
    )
    .unwrap();

    // Deleting the project cascades to repos, workspaces, worktrees
    db_delete(&conn, "projects", json!({ "where": { "id": pid } })).unwrap();
    assert_eq!(db_count(&conn, "repos", json!({})).unwrap(), 0);
    assert_eq!(db_count(&conn, "workspaces", json!({})).unwrap(), 0);
    assert_eq!(db_count(&conn, "worktrees", json!({})).unwrap(), 0);
}
