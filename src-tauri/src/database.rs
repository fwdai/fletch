use parking_lot::Mutex;
use rusqlite::{params_from_iter, Connection};
use rusqlite_migration::{Migrations, M};
use serde_json::{json, Map, Value};
use std::path::Path;
use std::sync::Arc;

use crate::error::{Error, Result};

const ALLOWED_TABLES: &[&str] = &[
    "accounts",
    "custom_agents",
    "project_settings",
    "projects",
    "repos",
    "sessions",
    "settings",
    "usage_daily",
    "workspaces",
    "worktrees",
];

/// Base name of the on-disk SQLite database within the app data dir. Neutral
/// (not tied to the product name) so a future rebrand never needs another file
/// migration. Shared by `init` and the recovery `move_db_aside` so both agree.
pub const DB_FILENAME: &str = "data.db";

/// Historical database name from before the app was renamed. Existing installs
/// still have `LEGACY_DB_FILENAME` on disk; `migrate_legacy_db_name` renames it
/// (and its WAL/SHM sidecars) to `DB_FILENAME` on first launch. Kept as a
/// separate constant so the one-time migration is self-documenting.
pub const LEGACY_DB_FILENAME: &str = "quorum.db";

/// Every on-disk database base name the app may have used, current and legacy.
/// Fresh-start recovery moves all of these aside so a leftover legacy file can't
/// be resurrected by `migrate_legacy_db_name` on the retried `init`.
pub const DB_BASENAMES: &[&str] = &[DB_FILENAME, LEGACY_DB_FILENAME];

/// The SQLite database's WAL/SHM sidecar suffixes. The main file plus these
/// three names are the complete on-disk footprint that must move together.
pub const DB_SIDECAR_SUFFIXES: &[&str] = &["", "-wal", "-shm"];

/// `settings` key prefix for per-agent custom binary path overrides. The agent
/// id follows the prefix (e.g. `agent_bin_path_claude`); the value is the raw
/// absolute path the user entered. Shared by the startup loader and the
/// `set_agent_bin_override` command so both agree on the key format.
pub const AGENT_BIN_PREFIX: &str = "agent_bin_path_";

/// Read every `agent_bin_path_*` setting into an id → path map. Called once at
/// startup to seed `bin_resolve`'s in-memory override registry so binary
/// resolution never needs a DB handle. Blank values are skipped (a cleared
/// override).
pub fn load_agent_bin_overrides(conn: &Connection) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let rows = match db_select(conn, "settings", json!({})) {
        Ok(rows) => rows,
        Err(_) => return map,
    };
    for row in rows {
        let key = row.get("key").and_then(Value::as_str).unwrap_or("");
        let value = row.get("value").and_then(Value::as_str).unwrap_or("");
        if let Some(id) = key.strip_prefix(AGENT_BIN_PREFIX) {
            if !value.trim().is_empty() {
                map.insert(id.to_string(), value.to_string());
            }
        }
    }
    map
}

/// Read a single value from the key-value `settings` table. Returns `None` for
/// a missing key (or any read error), so callers can fall back to a default.
pub fn get_setting(conn: &Connection, key: &str) -> Option<String> {
    db_select(conn, "settings", json!({ "where": { "key": key } }))
        .ok()
        .and_then(|rows| rows.into_iter().next())
        .and_then(|row| row.get("value").and_then(Value::as_str).map(str::to_string))
}

/// Upsert a single `settings` value. Mirrors the frontend `setSetting`, so a
/// value written here is readable by the renderer's `getAllSettings`.
pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    db_upsert(
        conn,
        "settings",
        json!({ "key": key, "value": value }),
        "key",
    )?;
    Ok(())
}

/// The signed-in profile's display name and email from the single `accounts`
/// row — the fallback commit identity for repos with no `user.name` /
/// `user.email` configured (see `git_dist::fallback_identity`). Either field
/// may be absent; `None` when there is no account row at all.
pub fn get_account_identity(conn: &Connection) -> Option<(Option<String>, Option<String>)> {
    let row = db_select(conn, "accounts", json!({}))
        .ok()?
        .into_iter()
        .next()?;
    let field = |key: &str| row.get(key).and_then(Value::as_str).map(str::to_string);
    Some((field("name"), field("email")))
}

fn validate_table(table: &str) -> Result<()> {
    if ALLOWED_TABLES.contains(&table) {
        Ok(())
    } else {
        Err(Error::Other(format!("unknown table: {table}")))
    }
}

fn validate_column(col: &str) -> Result<()> {
    if !col.is_empty()
        && col.len() <= 64
        && col.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        Ok(())
    } else {
        Err(Error::Other(format!("invalid column name: {col}")))
    }
}

/// Embedded schema migrations, applied in order. SQLite's `user_version` tracks
/// how many have run, so the length here doubles as the target version: below
/// it means an upgrade is pending, above it means the DB was written by a newer
/// build (a downgrade — see `map_migration_error`).
const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_initial_schema.sql"),
    include_str!("../migrations/0002_session_records.sql"),
    include_str!("../migrations/0003_retire_session_events.sql"),
    include_str!("../migrations/0004_session_user_turns.sql"),
    include_str!("../migrations/0005_session_ingest_offset.sql"),
    include_str!("../migrations/0006_session_effort.sql"),
    include_str!("../migrations/0007_account_oauth.sql"),
    include_str!("../migrations/0008_worktree_base_sha.sql"),
    include_str!("../migrations/0009_session_model.sql"),
    include_str!("../migrations/0010_worktree_pr_number.sql"),
    include_str!("../migrations/0011_custom_agents.sql"),
    include_str!("../migrations/0012_user_turn_timing.sql"),
    include_str!("../migrations/0013_workspace_sandbox_engine.sql"),
    include_str!("../migrations/0014_pr_times_and_usage_daily.sql"),
    include_str!("../migrations/0015_worktree_pr_snapshot.sql"),
    include_str!("../migrations/0016_pending_messages.sql"),
    include_str!("../migrations/0017_skills_and_mcp_servers.sql"),
    include_str!("../migrations/0018_workflows.sql"),
    include_str!("../migrations/0019_workflows_v1.sql"),
    include_str!("../migrations/0020_workflow_base_branch.sql"),
    include_str!("../migrations/0021_session_forked_context.sql"),
];

fn get_migrations() -> Migrations<'static> {
    Migrations::new(MIGRATIONS.iter().map(|&sql| M::up(sql)).collect())
}

pub fn init(data_dir: &Path) -> Result<Arc<Mutex<Connection>>> {
    std::fs::create_dir_all(data_dir)?;
    migrate_legacy_db_name(data_dir)?;
    quarantine_orphaned_wal(data_dir)?;
    let db_path = data_dir.join(DB_FILENAME);
    let mut conn = open_db(&db_path)?;
    backup_before_upgrade(&conn, &db_path)?;
    get_migrations()
        .to_latest(&mut conn)
        .map_err(map_migration_error)?;
    Ok(Arc::new(Mutex::new(conn)))
}

/// Move aside any WAL/SHM sidecars that have no companion main database file.
/// Normal SQLite operation never produces this state — the main file is always
/// created before its WAL — so an orphaned `data.db-wal` can only be debris from
/// an interrupted rename (a crash mid-migration or mid fresh-start recovery).
/// If we opened `data.db` with that WAL still in place, SQLite would recover the
/// main file *from the orphaned WAL*, silently resurrecting committed rows the
/// interrupted operation meant to abandon. Runs after `migrate_legacy_db_name`
/// (which may legitimately recreate the main file) and before `open_db`. We
/// suffix the orphans as timestamped backups rather than delete them, so nothing
/// is ever lost irrecoverably. This is the correct-by-construction backstop for
/// every rename path: no matter how the main file went missing, its stray WAL is
/// never replayed into a supposedly fresh database.
fn quarantine_orphaned_wal(data_dir: &Path) -> Result<()> {
    if data_dir.join(DB_FILENAME).exists() {
        return Ok(());
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    for suffix in DB_SIDECAR_SUFFIXES {
        if suffix.is_empty() {
            continue; // the main file itself — its absence is what we guarded on
        }
        let name = format!("{DB_FILENAME}{suffix}");
        let src = data_dir.join(&name);
        if src.exists() {
            std::fs::rename(&src, data_dir.join(format!("{name}.orphaned-{stamp}")))?;
            tracing::warn!(name = %name, "quarantined orphaned database sidecar; no main database present");
        }
    }
    Ok(())
}

/// One-time rename of the pre-rebrand `quorum.db` (and its `-wal`/`-shm`
/// sidecars) to `DB_FILENAME`. Runs before `open_db` so existing installs keep
/// their data instead of silently starting on a fresh empty database. Idempotent
/// and safe: it only renames when the legacy file exists and the new one does
/// not, so once migrated (or on a clean install) it is a no-op. We rename rather
/// than copy — the user's data is never duplicated or deleted.
///
/// The main file is moved **last** (sidecars first). The migration's guard keys
/// off the main file's name, so if we crash mid-rename the main file is still
/// under the legacy name and the next launch simply re-runs and finishes the
/// remaining moves. Were the main file moved first, an interruption would leave
/// `data.db` present (skipping the guard) but its `data.db-wal` still under the
/// legacy name — opening it would silently drop committed WAL-only rows.
fn migrate_legacy_db_name(data_dir: &Path) -> Result<()> {
    let legacy_main = data_dir.join(LEGACY_DB_FILENAME);
    let new_main = data_dir.join(DB_FILENAME);
    if !legacy_main.exists() || new_main.exists() {
        return Ok(());
    }
    for suffix in DB_SIDECAR_SUFFIXES.iter().rev() {
        let src = data_dir.join(format!("{LEGACY_DB_FILENAME}{suffix}"));
        if src.exists() {
            std::fs::rename(&src, data_dir.join(format!("{DB_FILENAME}{suffix}")))?;
        }
    }
    tracing::info!(
        from = LEGACY_DB_FILENAME,
        to = DB_FILENAME,
        "migrated legacy database"
    );
    Ok(())
}

fn open_db(db_path: &Path) -> Result<Connection> {
    let conn = Connection::open(db_path)?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA temp_store = MEMORY;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )?;
    Ok(conn)
}

/// A migration failure where the DB's `user_version` exceeds our migration count
/// means the schema was written by a newer build — almost always an app
/// downgrade. Surface it as the typed `SchemaTooNew` so startup can offer
/// recovery instead of crash-looping; everything else stays a generic error.
fn map_migration_error(e: rusqlite_migration::Error) -> Error {
    use rusqlite_migration::{Error as ME, MigrationDefinitionError as MDE};
    match e {
        ME::MigrationDefinition(MDE::DatabaseTooFarAhead) => Error::SchemaTooNew,
        other => Error::Other(format!("migration failed: {other}")),
    }
}

/// Snapshot the DB aside before applying migrations, but only when an existing
/// schema is genuinely being upgraded: a fresh DB (`user_version == 0`) has
/// nothing to lose, and an already-current or schema-ahead DB isn't migrated.
/// Gives the user a restore point if a forward migration goes wrong or they
/// later downgrade.
fn backup_before_upgrade(conn: &Connection, db_path: &Path) -> Result<()> {
    let applied: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if applied <= 0 || applied as usize >= MIGRATIONS.len() {
        return Ok(());
    }
    let backup = db_path.with_extension(format!("db.bak-v{applied}-{}", now_millis()));
    snapshot_to(conn, &backup)?;
    tracing::info!(backup = %backup.display(), "backed up DB before schema upgrade");
    Ok(())
}

/// Write a consistent snapshot of `conn` to `dest` via SQLite's online backup
/// API. Unlike checkpoint-then-`fs::copy`, this reads *through* the connection,
/// so it always captures committed WAL frames — a plain copy would silently
/// omit them whenever an external reader (Spotlight, Time Machine, a backup
/// agent) holds the WAL and leaves the checkpoint incomplete (`busy != 0`).
fn snapshot_to(conn: &Connection, dest: &Path) -> Result<()> {
    let mut dst = Connection::open(dest)?;
    let backup = rusqlite::backup::Backup::new(conn, &mut dst)?;
    backup.run_to_completion(100, std::time::Duration::from_millis(50), None)?;
    Ok(())
}

/// Milliseconds since the Unix epoch, or 0 if the system clock is set before
/// 1970. Degrades rather than panicking so a backwards clock can't turn the
/// backup-then-migrate path (or any timestamped insert) into a hard crash.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn json_to_sql(value: &Value) -> Result<Box<dyn rusqlite::ToSql>> {
    match value {
        Value::Null => Ok(Box::new(rusqlite::types::Null)),
        Value::Bool(b) => Ok(Box::new(if *b { 1i64 } else { 0i64 })),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Box::new(i))
            } else if let Some(f) = n.as_f64() {
                Ok(Box::new(f))
            } else {
                Err(Error::Other("unsupported number type".into()))
            }
        }
        Value::String(s) => Ok(Box::new(s.clone())),
        Value::Array(_) | Value::Object(_) => Ok(Box::new(serde_json::to_string(value)?)),
    }
}

fn row_to_json(
    row: &rusqlite::Row,
    columns: &[String],
) -> std::result::Result<Map<String, Value>, rusqlite::Error> {
    let mut map = Map::new();
    for (i, col) in columns.iter().enumerate() {
        let val = match row.get_ref(i)? {
            rusqlite::types::ValueRef::Null => Value::Null,
            rusqlite::types::ValueRef::Integer(n) => json!(n),
            rusqlite::types::ValueRef::Real(f) => json!(f),
            rusqlite::types::ValueRef::Text(s) => {
                Value::String(String::from_utf8_lossy(s).into_owned())
            }
            rusqlite::types::ValueRef::Blob(b) => Value::String(hex_encode(b)),
        };
        map.insert(col.clone(), val);
    }
    Ok(map)
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn append_where(
    where_obj: &Map<String, Value>,
    sql: &mut String,
    params: &mut Vec<Box<dyn rusqlite::ToSql>>,
) -> Result<()> {
    let mut clauses = Vec::new();
    for (col, val) in where_obj {
        validate_column(col)?;
        if val.is_null() {
            clauses.push(format!("{col} IS NULL"));
        } else {
            let idx = params.len() + 1;
            clauses.push(format!("{col} = ?{idx}"));
            params.push(json_to_sql(val)?);
        }
    }
    if !clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&clauses.join(" AND "));
    }
    Ok(())
}

// ── Public API ──────────────────────────────────────────────────────────────

pub fn db_insert(conn: &Connection, table: &str, mut data: Value) -> Result<String> {
    validate_table(table)?;
    let obj = data
        .as_object_mut()
        .ok_or_else(|| Error::Other("data must be a JSON object".into()))?;

    let id = match obj.get("id").and_then(|v| v.as_str()) {
        Some(existing) => existing.to_string(),
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            obj.insert("id".to_string(), json!(id));
            id
        }
    };

    let now = now_millis();
    obj.entry("created_at").or_insert(json!(now));

    let mut columns = Vec::new();
    let mut placeholders = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    for (i, (col, val)) in obj.iter().enumerate() {
        validate_column(col)?;
        columns.push(col.as_str());
        placeholders.push(format!("?{}", i + 1));
        params.push(json_to_sql(val)?);
    }

    let sql = format!(
        "INSERT INTO {table} ({}) VALUES ({})",
        columns.join(", "),
        placeholders.join(", ")
    );

    conn.prepare(&sql)?.execute(params_from_iter(params))?;

    Ok(id)
}

pub fn db_select(conn: &Connection, table: &str, query: Value) -> Result<Vec<Map<String, Value>>> {
    validate_table(table)?;
    let q = query
        .as_object()
        .ok_or_else(|| Error::Other("query must be a JSON object".into()))?;

    let mut sql = format!("SELECT * FROM {table}");
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(w) = q.get("where").and_then(|v| v.as_object()) {
        if !w.is_empty() {
            append_where(w, &mut sql, &mut params)?;
        }
    }

    if let Some(col) = q.get("orderBy").and_then(|v| v.as_str()) {
        validate_column(col)?;
        let dir = match q.get("orderDirection").and_then(|v| v.as_str()) {
            Some(d) if d.eq_ignore_ascii_case("desc") => "DESC",
            _ => "ASC",
        };
        sql.push_str(&format!(" ORDER BY {col} {dir}"));
    }

    if let Some(n) = q.get("limit").and_then(|v| v.as_i64()) {
        if n > 0 {
            sql.push_str(&format!(" LIMIT {n}"));
        }
    }

    if let Some(n) = q.get("offset").and_then(|v| v.as_i64()) {
        if n > 0 {
            sql.push_str(&format!(" OFFSET {n}"));
        }
    }

    let mut stmt = conn.prepare(&sql)?;
    let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

    let rows: std::result::Result<Vec<_>, _> = stmt
        .query_map(params_from_iter(params), |row| row_to_json(row, &columns))?
        .collect();

    Ok(rows?)
}

pub fn db_update(conn: &Connection, table: &str, query: Value, data: Value) -> Result<usize> {
    validate_table(table)?;
    let q = query
        .as_object()
        .ok_or_else(|| Error::Other("query must be a JSON object".into()))?;
    let d = data
        .as_object()
        .ok_or_else(|| Error::Other("data must be a JSON object".into()))?;

    if d.is_empty() {
        return Err(Error::Other("cannot update with empty data".into()));
    }

    let mut set_clauses = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    for (col, val) in d {
        validate_column(col)?;
        let idx = params.len() + 1;
        set_clauses.push(format!("{col} = ?{idx}"));
        params.push(json_to_sql(val)?);
    }

    let mut sql = format!("UPDATE {table} SET {}", set_clauses.join(", "));

    if let Some(w) = q.get("where").and_then(|v| v.as_object()) {
        if !w.is_empty() {
            append_where(w, &mut sql, &mut params)?;
        }
    }

    let changed = conn.prepare(&sql)?.execute(params_from_iter(params))?;
    Ok(changed)
}

pub fn db_delete(conn: &Connection, table: &str, query: Value) -> Result<usize> {
    validate_table(table)?;
    let q = query
        .as_object()
        .ok_or_else(|| Error::Other("query must be a JSON object".into()))?;

    let mut sql = format!("DELETE FROM {table}");
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(w) = q.get("where").and_then(|v| v.as_object()) {
        if !w.is_empty() {
            append_where(w, &mut sql, &mut params)?;
        }
    }

    let changed = conn.prepare(&sql)?.execute(params_from_iter(params))?;
    Ok(changed)
}

pub fn db_count(conn: &Connection, table: &str, query: Value) -> Result<i64> {
    validate_table(table)?;
    let q = query
        .as_object()
        .ok_or_else(|| Error::Other("query must be a JSON object".into()))?;

    let mut sql = format!("SELECT COUNT(*) FROM {table}");
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(w) = q.get("where").and_then(|v| v.as_object()) {
        if !w.is_empty() {
            append_where(w, &mut sql, &mut params)?;
        }
    }

    let count: i64 = conn.query_row(&sql, params_from_iter(params), |row| row.get(0))?;
    Ok(count)
}

/// INSERT ... ON CONFLICT(conflict_col) DO UPDATE SET ...
/// `data` contains all columns for the insert. On conflict, every column
/// in `data` except the conflict column is updated.
pub fn db_upsert(
    conn: &Connection,
    table: &str,
    data: Value,
    conflict_column: &str,
) -> Result<String> {
    validate_table(table)?;
    let conflict_cols: Vec<&str> = conflict_column.split(',').map(|s| s.trim()).collect();
    if conflict_cols.is_empty() {
        return Err(Error::Other("conflict_column must not be empty".into()));
    }
    for col in &conflict_cols {
        validate_column(col)?;
    }
    let obj = data
        .as_object()
        .ok_or_else(|| Error::Other("data must be a JSON object".into()))?;

    if obj.is_empty() {
        return Err(Error::Other("cannot upsert with empty data".into()));
    }

    let mut columns = Vec::new();
    let mut placeholders = Vec::new();
    let mut update_clauses = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    for (i, (col, val)) in obj.iter().enumerate() {
        validate_column(col)?;
        columns.push(col.as_str());
        placeholders.push(format!("?{}", i + 1));
        params.push(json_to_sql(val)?);
        if !conflict_cols.contains(&col.as_str()) {
            update_clauses.push(format!("{col} = excluded.{col}"));
        }
    }

    let conflict_target = conflict_cols.join(", ");
    let sql = if update_clauses.is_empty() {
        format!(
            "INSERT INTO {table} ({}) VALUES ({}) ON CONFLICT({conflict_target}) DO NOTHING",
            columns.join(", "),
            placeholders.join(", "),
        )
    } else {
        format!(
            "INSERT INTO {table} ({}) VALUES ({}) ON CONFLICT({conflict_target}) DO UPDATE SET {}",
            columns.join(", "),
            placeholders.join(", "),
            update_clauses.join(", ")
        )
    };

    conn.prepare(&sql)?.execute(params_from_iter(params))?;

    let id = conflict_cols
        .first()
        .and_then(|c| obj.get(*c))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Ok(id)
}

pub fn db_query(
    conn: &Connection,
    sql: &str,
    params: Vec<Value>,
) -> Result<Vec<Map<String, Value>>> {
    let sql_trimmed = sql.trim();
    if !sql_trimmed
        .get(..6)
        .map(|s| s.eq_ignore_ascii_case("select"))
        .unwrap_or(false)
    {
        return Err(Error::Other(
            "db_query only allows SELECT statements".into(),
        ));
    }

    let sql_params: Vec<Box<dyn rusqlite::ToSql>> =
        params.iter().map(json_to_sql).collect::<Result<_>>()?;

    let mut stmt = conn.prepare(sql_trimmed)?;
    let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

    let rows: std::result::Result<Vec<_>, _> = stmt
        .query_map(params_from_iter(sql_params), |row| {
            row_to_json(row, &columns)
        })?
        .collect();

    Ok(rows?)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
