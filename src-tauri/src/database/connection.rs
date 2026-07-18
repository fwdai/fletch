//! Database lifecycle: on-disk names, embedded migrations, WAL quarantine,
//! legacy-name migration, connection open/backup/snapshot, and the millis clock.

use parking_lot::Mutex;
use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};
use std::path::Path;
use std::sync::Arc;

use crate::error::{Error, Result};

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

/// Embedded schema migrations, applied in order. SQLite's `user_version` tracks
/// how many have run, so the length here doubles as the target version: below
/// it means an upgrade is pending, above it means the DB was written by a newer
/// build (a downgrade — see `map_migration_error`).
pub(crate) const MIGRATIONS: &[&str] = &[
    include_str!("../../migrations/0001_initial_schema.sql"),
    include_str!("../../migrations/0002_session_records.sql"),
    include_str!("../../migrations/0003_retire_session_events.sql"),
    include_str!("../../migrations/0004_session_user_turns.sql"),
    include_str!("../../migrations/0005_session_ingest_offset.sql"),
    include_str!("../../migrations/0006_session_effort.sql"),
    include_str!("../../migrations/0007_account_oauth.sql"),
    include_str!("../../migrations/0008_worktree_base_sha.sql"),
    include_str!("../../migrations/0009_session_model.sql"),
    include_str!("../../migrations/0010_worktree_pr_number.sql"),
    include_str!("../../migrations/0011_custom_agents.sql"),
    include_str!("../../migrations/0012_user_turn_timing.sql"),
    include_str!("../../migrations/0013_workspace_sandbox_engine.sql"),
    include_str!("../../migrations/0014_pr_times_and_usage_daily.sql"),
    include_str!("../../migrations/0015_worktree_pr_snapshot.sql"),
    include_str!("../../migrations/0016_pending_messages.sql"),
    include_str!("../../migrations/0017_skills_and_mcp_servers.sql"),
    include_str!("../../migrations/0018_workflows.sql"),
    include_str!("../../migrations/0019_workflows_v1.sql"),
    include_str!("../../migrations/0020_workflow_base_branch.sql"),
    include_str!("../../migrations/0021_session_forked_context.sql"),
    include_str!("../../migrations/0022_repo_label.sql"),
    include_str!("../../migrations/0023_workspace_issue_ref.sql"),
    include_str!("../../migrations/0024_wf_run_issue_ref.sql"),
];

pub(crate) fn get_migrations() -> Migrations<'static> {
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
pub(crate) fn quarantine_orphaned_wal(data_dir: &Path) -> Result<()> {
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
pub(crate) fn migrate_legacy_db_name(data_dir: &Path) -> Result<()> {
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

pub(crate) fn open_db(db_path: &Path) -> Result<Connection> {
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
pub(crate) fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
