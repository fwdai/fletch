//! Database lifecycle: on-disk names, embedded migrations, WAL quarantine,
//! legacy-name migration, connection open/backup/snapshot, and the millis clock.

use parking_lot::Mutex;
use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::error::{Error, Result};

use super::settings::{get_setting, set_setting};

/// Base name of the on-disk SQLite database within the app data dir. Neutral
/// (not tied to the product name) so a future rebrand never needs another file
/// migration. Shared by `init` and the recovery `move_db_aside` so both agree.
pub const DB_FILENAME: &str = "data.db";

/// Historical database name from before the app was renamed. Existing installs
/// still have `LEGACY_DB_FILENAME` on disk; `migrate_legacy_db_name` renames it
/// (and its WAL/SHM sidecars) to `DB_FILENAME` on first launch. Kept as a
/// separate constant so the one-time migration is self-documenting.
pub const LEGACY_DB_FILENAME: &str = "quorum.db";

/// The append-only transcript log (`session_records`), next to `DB_FILENAME`
/// and attached to its connection as schema `transcripts`. Its own file so the
/// hot operational database stays small: every whole-database operation on
/// `data.db` (pre-upgrade backup, VACUUM, a migration rewriting a table) would
/// otherwise pay for hundreds of megabytes of log that rarely changes shape.
pub const TRANSCRIPTS_DB_FILENAME: &str = "transcripts.db";

/// Schema name `TRANSCRIPTS_DB_FILENAME` is attached under; every statement
/// touching `session_records` qualifies it with this.
pub const TRANSCRIPTS_SCHEMA: &str = "transcripts";

/// Every on-disk database base name the app may have used, current and legacy.
/// Fresh-start recovery moves all of these aside so a leftover legacy file can't
/// be resurrected by `migrate_legacy_db_name` on the retried `init`, and so the
/// transcript log never outlives the `sessions` rows that own it.
pub const DB_BASENAMES: &[&str] = &[DB_FILENAME, TRANSCRIPTS_DB_FILENAME, LEGACY_DB_FILENAME];

/// The database files `init` opens, each with its own schema and migrations.
const LIVE_DB_BASENAMES: &[&str] = &[DB_FILENAME, TRANSCRIPTS_DB_FILENAME];

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
    include_str!("../../migrations/0025_worktree_pr_history.sql"),
    include_str!("../../migrations/0026_roadmap_items.sql"),
    include_str!("../../migrations/0027_workspace_purpose.sql"),
    include_str!("../../migrations/0028_wf_run_roadmap_item.sql"),
    include_str!("../../migrations/0029_wf_run_pr.sql"),
    include_str!("../../migrations/0030_roadmap_item_events.sql"),
    include_str!("../../migrations/0031_roadmap_proposals.sql"),
    include_str!("../../migrations/0032_roadmap_rank.sql"),
    include_str!("../../migrations/0033_roadmap_holds.sql"),
    include_str!("../../migrations/0034_roadmap_briefs.sql"),
    include_str!("../../migrations/0035_roadmap_items_drop_dormant.sql"),
    include_str!("../../migrations/0036_roadmap_issue_url.sql"),
    include_str!("../../migrations/0037_roadmap_close_reason.sql"),
    include_str!("../../migrations/0038_worktree_adopted_checkout.sql"),
    include_str!("../../migrations/0039_workspace_archive_trace.sql"),
    include_str!("../../migrations/0040_workspace_title.sql"),
    include_str!("../../migrations/0041_session_lineage.sql"),
    include_str!("../../migrations/0042_session_branch_point.sql"),
    include_str!("../../migrations/0043_session_handoff_context.sql"),
    include_str!("../../migrations/0044_user_turn_record_watermark.sql"),
    include_str!("../../migrations/0045_session_transcript_prefix.sql"),
    include_str!("../../migrations/0046_autopilot_log.sql"),
    include_str!("../../migrations/0047_drop_session_records.sql"),
];

/// The transcript log's own migrations, tracked by `transcripts.db`'s
/// `user_version`. Kept apart from `MIGRATIONS` so the operational schema can
/// move without ever touching the log.
pub(crate) const TRANSCRIPT_MIGRATIONS: &[&str] = &[include_str!(
    "../../migrations_transcripts/0001_session_records.sql"
)];

/// Smallest `MIGRATIONS.len()` a build needs to read the schema these
/// migrations produce. Stored in `settings` under `MIN_READER_VERSION_KEY` by
/// every build at or ahead of the database, so an older build opening a newer
/// database can tell an additive change it can live with from one it can't.
///
/// Rule: bump to the new `MIGRATIONS.len()` whenever a migration drops,
/// renames or rebuilds something older code reads (a column, a table, a
/// constraint it relies on); leave it alone for additive migrations (new
/// nullable columns, new tables, new indexes). 47: migration 0047 dropped
/// `session_records` from this file (it lives in `transcripts.db` now), which
/// no earlier build can read around.
pub(crate) const MIN_READER_VERSION: usize = 47;
const _: () = assert!(MIN_READER_VERSION >= 1 && MIN_READER_VERSION <= MIGRATIONS.len());

/// `settings` key holding `MIN_READER_VERSION` of the build that last
/// migrated the database.
pub(crate) const MIN_READER_VERSION_KEY: &str = "schema.min_reader_version";

pub(crate) fn get_migrations() -> Migrations<'static> {
    Migrations::new(MIGRATIONS.iter().map(|&sql| M::up(sql)).collect())
}

pub(crate) fn get_transcript_migrations() -> Migrations<'static> {
    Migrations::new(
        TRANSCRIPT_MIGRATIONS
            .iter()
            .map(|&sql| M::up(sql))
            .collect(),
    )
}

/// The steps of [`init`] that can take real time on a large database, for a
/// host that shows startup progress. Each is reported just before it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbPhase {
    /// The pre-upgrade snapshot. Only when an existing schema is being
    /// upgraded — see `backup_before_upgrade`.
    BackingUp,
    Migrating,
    /// `session_records` rows being copied from `data.db` into
    /// `transcripts.db`, reported after every batch; see
    /// `relocate_session_records`.
    Relocating {
        done: u64,
        total: u64,
    },
}

pub fn init(data_dir: &Path) -> Result<Arc<Mutex<Connection>>> {
    init_with_progress(data_dir, &|_| {})
}

pub fn init_with_progress(
    data_dir: &Path,
    on_phase: &dyn Fn(DbPhase),
) -> Result<Arc<Mutex<Connection>>> {
    std::fs::create_dir_all(data_dir)?;
    migrate_legacy_db_name(data_dir)?;
    quarantine_orphaned_wal(data_dir)?;
    let db_path = data_dir.join(DB_FILENAME);
    let transcripts_path = data_dir.join(TRANSCRIPTS_DB_FILENAME);
    let mut conn = open_db(&db_path)?;
    backup_before_upgrade(&conn, &db_path, MIGRATIONS.len(), on_phase)?;
    prepare_transcripts_db(&transcripts_path, on_phase)?;
    attach_transcripts(&conn, &transcripts_path)?;
    let relocated = relocate_session_records(&conn, on_phase)?;
    on_phase(DbPhase::Migrating);
    migrate_main(&mut conn)?;
    if relocated {
        // The table 0047 dropped only moved to the freelist; this hands its
        // pages back to the filesystem. Without a schema name VACUUM rewrites
        // `main` alone, and it must run outside any transaction.
        conn.execute_batch("VACUUM")?;
    }
    sweep_orphaned_transcripts(&conn)?;
    Ok(Arc::new(Mutex::new(conn)))
}

/// Bring `transcripts.db` to its schema on a connection of its own, so its
/// migrations and pre-upgrade backup are the same code path as `data.db`'s.
/// Closed before the file is attached to the main connection.
fn prepare_transcripts_db(path: &Path, on_phase: &dyn Fn(DbPhase)) -> Result<()> {
    let mut conn = open_db(path)?;
    backup_before_upgrade(&conn, path, TRANSCRIPT_MIGRATIONS.len(), on_phase)?;
    migrate(&mut conn, &get_transcript_migrations())
}

fn attach_transcripts(conn: &Connection, path: &Path) -> Result<()> {
    let path = path
        .to_str()
        .ok_or_else(|| Error::Other(format!("non-UTF-8 database path: {}", path.display())))?;
    conn.execute(
        &format!("ATTACH DATABASE ?1 AS {TRANSCRIPTS_SCHEMA}"),
        [path],
    )?;
    // Per-database, unlike the connection-wide pragmas `open_db` sets; the
    // attached file would otherwise run at the WAL default of FULL.
    conn.execute_batch(&format!("PRAGMA {TRANSCRIPTS_SCHEMA}.synchronous = NORMAL"))?;
    Ok(())
}

/// `data.db`'s migrations, or none when the file was written by a newer build
/// whose stored reader floor admits this one. The floor lives in `settings`,
/// so only this file has one; `transcripts.db` goes through `migrate` alone.
fn migrate_main(conn: &mut Connection) -> Result<()> {
    let applied = user_version(conn)?;
    if applied > MIGRATIONS.len() && schema_readable_by_this_build(conn) {
        tracing::info!(
            applied,
            known = MIGRATIONS.len(),
            "schema is ahead of this build but within its reader floor; not migrating"
        );
        return Ok(());
    }
    // A database ahead of this build without a floor that admits it fails in
    // here as `DatabaseTooFarAhead`.
    migrate(conn, &get_migrations())?;
    // Reached only when this build is at or ahead of the database, so a
    // compatible older reader never lowers the floor a newer build wrote.
    set_setting(
        conn,
        MIN_READER_VERSION_KEY,
        &MIN_READER_VERSION.to_string(),
    )?;
    Ok(())
}

/// Apply `migrations`, leaving the connection with foreign keys enforced.
///
/// Foreign-key enforcement must be OFF while migrations run, and that is the
/// caller's job — rusqlite_migration never touches the pragma. With it on, a
/// table-rebuild migration (CREATE new / INSERT SELECT / DROP old / RENAME,
/// e.g. 0035) fires ON DELETE CASCADE at the DROP and silently deletes every
/// child row pointing at the rebuilt table. The pragma is a no-op inside a
/// transaction, so it has to be set here, outside the per-migration
/// transactions the runner opens. The other half of running with enforcement
/// off (SQLite's documented rebuild procedure): verify no migration left a
/// dangling reference before trusting the schema.
fn migrate(conn: &mut Connection, migrations: &Migrations<'static>) -> Result<()> {
    conn.pragma_update(None, "foreign_keys", false)?;
    migrations.to_latest(conn).map_err(map_migration_error)?;
    let violations: i64 =
        conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check()", [], |r| {
            r.get(0)
        })?;
    if violations > 0 {
        return Err(Error::Other(format!(
            "schema migration left {violations} foreign-key violation(s)"
        )));
    }
    conn.pragma_update(None, "foreign_keys", true)?;
    Ok(())
}

/// Rows copied per transaction by `relocate_session_records`: ~20 MB at the
/// observed ~4 KB average body, small enough that an interrupted run loses
/// under a second of work and the progress report moves visibly.
const RELOCATE_BATCH: i64 = 5000;

/// Copy `main.session_records` into `transcripts.session_records`, rowid
/// ordered in `RELOCATE_BATCH`-row transactions, resuming past whatever an
/// interrupted run already landed. Returns whether there was a table to copy.
/// The main table is dropped by migration 0047, which runs after this: that
/// is what records the move in `user_version` and lets a half-done copy pick
/// up on the next launch instead of starting over.
fn relocate_session_records(conn: &Connection, on_phase: &dyn Fn(DbPhase)) -> Result<bool> {
    if !table_exists(conn, "main", "session_records")? {
        return Ok(false);
    }
    let total: u64 = conn.query_row("SELECT COUNT(*) FROM main.session_records", [], |r| {
        r.get(0)
    })?;
    let mut last_id: i64 = conn.query_row(
        &format!("SELECT COALESCE(MAX(id), 0) FROM {TRANSCRIPTS_SCHEMA}.session_records"),
        [],
        |r| r.get(0),
    )?;
    let mut done: u64 = conn.query_row(
        "SELECT COUNT(*) FROM main.session_records WHERE id <= ?1",
        [last_id],
        |r| r.get(0),
    )?;
    on_phase(DbPhase::Relocating { done, total });
    while let Some((copied, end_id)) = relocate_batch(conn, last_id, RELOCATE_BATCH)? {
        done += copied;
        last_id = end_id;
        on_phase(DbPhase::Relocating { done, total });
    }
    tracing::info!(
        rows = total,
        "relocated session_records into transcripts.db"
    );
    Ok(true)
}

/// One transaction of `relocate_session_records`: the `limit` rows after
/// `after_id`. Returns how many there were and the last id copied, or `None`
/// when nothing is left.
pub(crate) fn relocate_batch(
    conn: &Connection,
    after_id: i64,
    limit: i64,
) -> Result<Option<(u64, i64)>> {
    let tx = conn.unchecked_transaction()?;
    let (count, end_id): (u64, Option<i64>) = tx.query_row(
        "SELECT COUNT(*), MAX(id) FROM
           (SELECT id FROM main.session_records WHERE id > ?1 ORDER BY id LIMIT ?2)",
        [after_id, limit],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let Some(end_id) = end_id else {
        return Ok(None);
    };
    tx.execute(
        &format!(
            "INSERT OR IGNORE INTO {TRANSCRIPTS_SCHEMA}.session_records
                (id, session_id, seq, provider, source, native_id, agent_version, body, created_at)
             SELECT id, session_id, seq, provider, source, native_id, agent_version, body, created_at
               FROM main.session_records WHERE id > ?1 AND id <= ?2 ORDER BY id"
        ),
        [after_id, end_id],
    )?;
    tx.commit()?;
    Ok(Some((count, end_id)))
}

fn table_exists(conn: &Connection, schema: &str, table: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM {schema}.sqlite_master WHERE type = 'table' AND name = ?1"),
        [table],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Drop transcript rows whose session no longer exists. A WAL commit is atomic
/// per file, not across the two, so a crash between the explicit transcript
/// delete and the `sessions` cascade it precedes can leave rows behind; this
/// is what makes that window benign. Skipped when `main.sessions` is empty: a
/// fresh or moved-aside main database must never wipe a transcripts file.
fn sweep_orphaned_transcripts(conn: &Connection) -> Result<()> {
    let sessions: i64 = conn.query_row("SELECT COUNT(*) FROM main.sessions", [], |r| r.get(0))?;
    if sessions == 0 {
        return Ok(());
    }
    // Distinct session ids come off the (session_id, seq) index, so this reads
    // the index rather than every body.
    let removed = conn.execute(
        &format!(
            "DELETE FROM {TRANSCRIPTS_SCHEMA}.session_records WHERE session_id IN
               (SELECT DISTINCT session_id FROM {TRANSCRIPTS_SCHEMA}.session_records
                EXCEPT SELECT id FROM main.sessions)"
        ),
        [],
    )?;
    if removed > 0 {
        tracing::info!(rows = removed, "swept orphaned session_records");
    }
    Ok(())
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
    for basename in LIVE_DB_BASENAMES {
        quarantine_orphaned_wal_of(data_dir, basename)?;
    }
    Ok(())
}

fn quarantine_orphaned_wal_of(data_dir: &Path, basename: &str) -> Result<()> {
    if data_dir.join(basename).exists() {
        return Ok(());
    }
    let stamp = now_millis();
    for suffix in DB_SIDECAR_SUFFIXES {
        if suffix.is_empty() {
            continue; // the main file itself — its absence is what we guarded on
        }
        let name = format!("{basename}{suffix}");
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

/// Number of migrations applied to the database (SQLite's `user_version`,
/// which rusqlite_migration uses as its schema version).
fn user_version(conn: &Connection) -> Result<usize> {
    let applied: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    Ok(usize::try_from(applied).unwrap_or(0))
}

/// Whether the floor the last migrating build stored admits this build. A
/// missing or unparsable floor (a database written before the rule existed, or
/// a schema that moved `settings` itself) is not readable: nothing vouched
/// for it.
fn schema_readable_by_this_build(conn: &Connection) -> bool {
    get_setting(conn, MIN_READER_VERSION_KEY)
        .and_then(|v| v.trim().parse::<usize>().ok())
        .is_some_and(|floor| MIGRATIONS.len() >= floor)
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

/// The applied schema version when `backup_before_upgrade` will snapshot: an
/// existing schema (`user_version > 0`) below `migration_count`, the file's
/// target version. A fresh DB has nothing to lose and a current or
/// schema-ahead DB isn't migrated.
fn pending_upgrade_from(conn: &Connection, migration_count: usize) -> Result<Option<i64>> {
    let applied: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    Ok((applied > 0 && (applied as usize) < migration_count).then_some(applied))
}

/// Complete pre-upgrade backups retained per database file; older ones are
/// deleted once a new backup lands.
pub(crate) const BACKUPS_TO_KEEP: usize = 2;

/// Pages copied per `sqlite3_backup_step` call: 16 MB at the 4 KB default page
/// size. The backup runs during startup with no concurrent writer, so the loop
/// is tuned for throughput, not for yielding.
const SNAPSHOT_PAGES_PER_STEP: i32 = 4096;

/// Suffix a backup carries while it is still being written. `prune_backups`
/// treats any file with it as an interrupted copy.
const PARTIAL_SUFFIX: &str = ".partial";

/// Snapshot the DB aside before applying migrations (see `pending_upgrade_from`
/// for when), announcing `DbPhase::BackingUp` first. Gives the user a restore
/// point if a forward migration goes wrong or they later downgrade. Older
/// backups and leftovers from interrupted copies are pruned only after the new
/// one is complete, so a failed backup never costs an existing restore point.
/// Per file: `data.db` and `transcripts.db` each pass their own path and
/// migration count.
fn backup_before_upgrade(
    conn: &Connection,
    db_path: &Path,
    migration_count: usize,
    on_phase: &dyn Fn(DbPhase),
) -> Result<()> {
    let Some(applied) = pending_upgrade_from(conn, migration_count)? else {
        return Ok(());
    };
    on_phase(DbPhase::BackingUp);
    let backup = backup_path(db_path, applied);
    snapshot_to(conn, &backup)?;
    tracing::info!(backup = %backup.display(), "backed up DB before schema upgrade");
    // Housekeeping only: a leftover we cannot delete must not block the launch.
    if let Err(e) = prune_backups(db_path, BACKUPS_TO_KEEP) {
        tracing::warn!(error = %e, "pruning old DB backups failed");
    }
    Ok(())
}

/// `<db>.bak-v<applied>-<now_millis>`, next to the database. The millisecond
/// suffix is what `prune_backups` orders by.
fn backup_path(db_path: &Path, applied: i64) -> PathBuf {
    let name = db_path.file_name().unwrap_or_default().to_string_lossy();
    db_path.with_file_name(format!("{name}.bak-v{applied}-{}", now_millis()))
}

/// Write a consistent snapshot of `conn` to `dest` via SQLite's online backup
/// API. Unlike checkpoint-then-`fs::copy`, this reads *through* the connection,
/// so it always captures committed WAL frames — a plain copy would silently
/// omit them whenever an external reader (Spotlight, Time Machine, a backup
/// agent) holds the WAL and leaves the checkpoint incomplete (`busy != 0`).
///
/// The copy lands in `<dest>.partial` and is renamed into place only once
/// complete, so a file under the final name is always a whole database and an
/// interrupted copy (force-quit mid-backup) is identifiable by its suffix.
/// Steps run back to back: `Backup::run_to_completion` sleeps after every step
/// including successful ones, which turned a ~1 GB copy into minutes of pure
/// waiting. Only `Busy`/`Locked` (another writer holds the source) pause.
fn snapshot_to(conn: &Connection, dest: &Path) -> Result<()> {
    use rusqlite::backup::StepResult;

    let partial = partial_path(dest);
    {
        let mut dst = Connection::open(&partial)?;
        let backup = rusqlite::backup::Backup::new(conn, &mut dst)?;
        loop {
            match backup.step(SNAPSHOT_PAGES_PER_STEP)? {
                StepResult::Done => break,
                StepResult::Busy | StepResult::Locked => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                _ => {}
            }
        }
    }
    std::fs::rename(&partial, dest)?;
    Ok(())
}

fn partial_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(PARTIAL_SUFFIX);
    dest.with_file_name(name)
}

/// Delete stale pre-upgrade backups of `db_path`: every `<db>.bak-*` file that
/// is a `.partial` copy or a `-journal` sidecar (debris from an interrupted
/// `snapshot_to`), then all complete backups beyond the newest `keep`, ordered
/// by the millisecond suffix in the name and falling back to mtime for names
/// that don't parse. Files moved aside by fresh-start recovery (`.moved-*`)
/// don't match the prefix and are never touched.
pub(crate) fn prune_backups(db_path: &Path, keep: usize) -> Result<()> {
    let (Some(dir), Some(name)) = (db_path.parent(), db_path.file_name()) else {
        return Ok(());
    };
    let prefix = format!("{}.bak-", name.to_string_lossy());
    let mut complete: Vec<(i64, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if !file_name.starts_with(&prefix) {
            continue;
        }
        let path = entry.path();
        if file_name.ends_with(PARTIAL_SUFFIX) || file_name.ends_with("-journal") {
            std::fs::remove_file(&path)?;
            tracing::info!(file = %path.display(), "removed interrupted DB backup");
            continue;
        }
        let stamp = file_name
            .rsplit('-')
            .next()
            .and_then(|s| s.parse::<i64>().ok())
            .or_else(|| {
                let modified = entry.metadata().ok()?.modified().ok()?;
                let since_epoch = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
                Some(since_epoch.as_millis() as i64)
            })
            .unwrap_or(0);
        complete.push((stamp, path));
    }
    complete.sort_by_key(|(stamp, _)| std::cmp::Reverse(*stamp));
    for (_, path) in complete.into_iter().skip(keep) {
        std::fs::remove_file(&path)?;
        tracing::info!(file = %path.display(), "pruned old DB backup");
    }
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
