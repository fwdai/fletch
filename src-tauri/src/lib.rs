//! The Fletch desktop shell.
//!
//! The engine lives in `fletch-core` (`crates/fletch-core`) and knows nothing
//! about Tauri. What is left here is the shell around it: the Tauri builder and
//! `setup`, the `#[tauri::command]` wrappers, the tray and its activity
//! monitor, dictation, the OAuth device flows, the updater, and the
//! [`TauriSink`] that carries engine events into the webview.
//!
//! Every engine module is re-exported below, so a `crate::supervisor::…` path
//! in this crate resolves exactly as it did when the module lived here.

// This Mac as a client of *other* Fletch hosts: desktop glue over
// `fletch_proto::client::Dialer`, so it stays a real module here rather than
// joining the engine re-exports below.
mod client;
mod commands;
mod dictation;
mod editors;
mod oauth;
mod provider_login;
mod sentry_scrub;

// ── The engine (`fletch-core`) ────────────────────────────────────────────
// Re-exported rather than imported at each use site: these were `mod`
// declarations until the crate split, and every `crate::x::y` path in the shell
// keeps resolving through them.
pub use fletch_core::{
    activity, agent, agent_install, agent_profile, attachments, bin_resolve, child_io, codegraph,
    database, download, error, exec_session, git, git_dist, git_state, github, host, instructions,
    issues, keychain, linear, managed_session, message_queue, model_catalog, names, native_input,
    new_project, power, pty_session, publish_prefs, roadmap, rpc, run_detect, run_env, run_session,
    sandbox, secrets, slash_commands, supervisor, telemetry, transcripts, usage_scan, verify,
    workflow, workspace,
};
// Paired-device remote access (Settings → Remote control). Desktop-only: the
// host is the machine agents run on, never the phone.
#[cfg(desktop)]
pub use fletch_core::remote;
// The shell's own paths onto the engine's on-disk layout and DB handle.
pub use fletch_core::{build_state_subpath, data_dir, logs_dir, DbState, BUNDLE_ID};

use parking_lot::Mutex;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::Manager;

use crate::supervisor::Supervisor;

/// The desktop's sink: events reach the webview exactly as they did when the
/// engine called `app.emit` itself. The newtype lives here rather than beside
/// [`fletch_core::host::EventSink`] because the trait is the engine's and the
/// `AppHandle` is Tauri's — neither is ours to implement on the other's turf.
pub struct TauriSink(pub tauri::AppHandle);

impl fletch_core::host::EventSink for TauriSink {
    fn emit_value(&self, event: &str, payload: Value) -> Result<(), String> {
        tauri::Emitter::emit(&self.0, event, payload).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_basenames_lists_current_before_legacy() {
        // move_db_aside processes DB_BASENAMES.rev() so the legacy name is moved
        // aside FIRST — the contract that stops migrate from resurrecting the
        // legacy db once the current one is gone. Pin the order this relies on.
        assert_eq!(
            database::DB_BASENAMES,
            &[database::DB_FILENAME, database::LEGACY_DB_FILENAME]
        );
    }

    #[test]
    fn recovery_does_not_resurrect_a_leftover_legacy_db() {
        // A stray `quorum.db` sits next to the live `data.db`. Fresh-start
        // recovery must move BOTH aside; otherwise the retried init() would
        // rename the legacy file back into place and reopen it.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(database::DB_FILENAME), b"current").unwrap();
        std::fs::write(dir.path().join(database::LEGACY_DB_FILENAME), b"legacy").unwrap();

        move_db_aside(dir.path()).unwrap();

        // Neither base name survives, so init() starts truly fresh.
        assert!(!dir.path().join(database::DB_FILENAME).exists());
        assert!(!dir.path().join(database::LEGACY_DB_FILENAME).exists());
        database::init(dir.path()).unwrap();
        assert!(dir.path().join(database::DB_FILENAME).exists());
    }
}

/// Number of daily log files to keep. The rolling appender deletes the oldest
/// beyond this on each rotation, so `logs/` stays bounded instead of growing a
/// file per day forever. Daily rotation → roughly this many days of history.
/// (A user-configurable retention is a plausible future settings option.)
const LOG_RETENTION_FILES: usize = 14;

/// Send tracing output to both stdout (as before) and a daily-rolling file
/// under `logs_dir()`, so a notarized build that crashes in the field leaves a
/// log the user can attach to a bug report. The file writer is synchronous
/// (not buffered) so the last lines before a crash actually reach disk, and is
/// capped at `LOG_RETENTION_FILES` so it self-prunes.
fn init_logging() {
    use tracing_subscriber::prelude::*;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,fletch_lib=debug"));

    let dir = logs_dir();
    let appender = std::fs::create_dir_all(&dir)
        .map_err(|e| format!("create log dir: {e}"))
        .and_then(|()| {
            tracing_appender::rolling::Builder::new()
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .filename_prefix("fletch")
                .filename_suffix("log")
                .max_log_files(LOG_RETENTION_FILES)
                .build(&dir)
                .map_err(|e| format!("open log file: {e}"))
        });
    let file_layer = match appender {
        Ok(appender) => Some(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(appender),
        ),
        Err(e) => {
            eprintln!("file logging disabled ({}): {e}", dir.display());
            None
        }
    };

    // Forward tracing events to Sentry: ERROR and WARN become captured events,
    // so handled failures on users' machines (not just panics) surface in
    // Sentry alongside the local log file. Lower levels become breadcrumbs that
    // give those events context. Capture is a no-op when no DSN is baked in, so
    // this stays inert in dev and unconfigured builds.
    //
    // PRIVACY INVARIANT: keep log messages static string literals and put every
    // dynamic value (paths, argv, repo/branch names, error strings) in a
    // structured field. Fields are dropped before egress unless their key is
    // allowlisted in `sentry_scrub` — the message is what reaches Sentry.
    // Interpolating dynamic data into the message string would leak it. This
    // applies to every macro form, including `target:`-prefixed ones (e.g.
    // streaming docker build output) — those produce breadcrumbs like any
    // other sub-WARN event.
    let sentry_layer = sentry::integrations::tracing::layer().event_filter(|md| {
        use sentry::integrations::tracing::EventFilter;
        match *md.level() {
            tracing::Level::ERROR | tracing::Level::WARN => EventFilter::Event,
            _ => EventFilter::Breadcrumb,
        }
    });

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .with(file_layer)
        .with(sentry_layer)
        .init();
}

/// Chain a panic hook that logs the panic (so it lands in the log file) onto
/// whatever hook is already installed — notably Sentry's, set by
/// `sentry::init`, which we must not clobber. Call after `sentry::init`.
fn install_panic_logging() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(panic = %info, "panic");
        prev(info);
    }));
}

/// Labels for the fatal DB-init dialog's three buttons. Kept as constants so
/// rendering and result-matching share one source of truth.
const DB_ERR_MOVE_ASIDE: &str = "Move Database Aside";
const DB_ERR_REVEAL_LOGS: &str = "Reveal Logs";
const DB_ERR_QUIT: &str = "Quit";

enum DbErrorChoice {
    MoveAside,
    RevealLogs,
    Quit,
}

/// Recover from a failed `database::init` instead of panicking into a launch
/// crash loop (the schema-too-new downgrade case, most often). Shows a native
/// dialog and loops on the user's choice: move the DB aside and start fresh,
/// reveal the logs and re-prompt, or quit. If the move-aside recovery itself
/// fails, we re-prompt with that error rather than quitting silently — the user
/// asked to recover and deserves to see why it didn't work. Only ever resolves
/// by returning a live DB or exiting, so it's total. Runs in `setup` on the
/// main thread — hence rfd's synchronous dialog, not the tauri plugin's, which
/// needs the not-yet-running event loop.
fn recover_from_db_init_failure(
    data_dir: &std::path::Path,
    mut err: crate::error::Error,
) -> DbState {
    loop {
        tracing::error!(error = %err, "database init failed; prompting for recovery");
        match show_db_error_dialog(&err) {
            DbErrorChoice::MoveAside => {
                match move_db_aside(data_dir).and_then(|()| database::init(data_dir)) {
                    Ok(db) => return db,
                    Err(e) => err = e, // recovery failed — loop and re-prompt with why
                }
            }
            DbErrorChoice::RevealLogs => {
                let _ = commands::reveal_logs();
            }
            DbErrorChoice::Quit => std::process::exit(1),
        }
    }
}

fn show_db_error_dialog(err: &crate::error::Error) -> DbErrorChoice {
    let (title, body) = db_error_message(err);
    let result = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title(title)
        .set_description(body)
        .set_buttons(rfd::MessageButtons::YesNoCancelCustom(
            DB_ERR_MOVE_ASIDE.into(),
            DB_ERR_REVEAL_LOGS.into(),
            DB_ERR_QUIT.into(),
        ))
        .show();
    match result {
        rfd::MessageDialogResult::Custom(l) if l == DB_ERR_MOVE_ASIDE => DbErrorChoice::MoveAside,
        rfd::MessageDialogResult::Custom(l) if l == DB_ERR_REVEAL_LOGS => DbErrorChoice::RevealLogs,
        _ => DbErrorChoice::Quit,
    }
}

fn db_error_message(err: &crate::error::Error) -> (&'static str, String) {
    let title = "Fletch can't open its database";
    let body = match err {
        crate::error::Error::SchemaTooNew => "This database was created by a newer version of \
            Fletch, so this version can't read it — usually a sign the app was downgraded.\n\n\
            • Move Database Aside — start fresh now; your current database is kept as a backup \
            file you can restore later.\n\
            • Reveal Logs — open the log folder to investigate.\n\
            • Quit — exit so you can reinstall the newer version."
            .to_string(),
        other => format!(
            "Fletch couldn't initialize its database and can't continue:\n\n{other}\n\n\
             • Move Database Aside — start fresh; the existing database is kept as a backup.\n\
             • Reveal Logs — open the log folder.\n\
             • Quit — exit."
        ),
    };
    (title, body)
}

/// Rename the database and its WAL/SHM sidecars out of the way so a fresh one
/// can be created, preserving the old files as timestamped backups. We suffix,
/// never delete — the user's data is always recoverable.
///
/// A sequence of per-file renames is not atomic, so a crash can interrupt it at
/// any point. No single ordering is crash-safe on its own — but paired with
/// `database::init`'s guards (`migrate_legacy_db_name` + `quarantine_orphaned_wal`)
/// this order leaves every interruption point in a safe state:
///
/// * **Legacy basename first.** `migrate_legacy_db_name` resurrects a stale db
///   whenever `quorum.db` exists and `data.db` does not. Moving the legacy main
///   file before `data.db` ever disappears means that trigger state is never
///   produced, so a half-finished recovery can't rename the old db back.
/// * **Main file first within each basename** (`DB_SIDECAR_SUFFIXES` in order —
///   the empty suffix leads). An interruption then leaves the main file gone
///   with its WAL still live-named, which `quarantine_orphaned_wal` sweeps aside
///   on the next launch. The reverse order would risk "main present, WAL moved
///   away", which no guard can detect and which silently drops committed rows.
///
/// (`migrate_legacy_db_name` deliberately moves its main file *last* — the
/// opposite — because it preserves into a live db, where the legacy main name is
/// its re-run sentinel and quarantine can't help once the main file exists.)
fn move_db_aside(data_dir: &std::path::Path) -> crate::error::Result<()> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    // `DB_BASENAMES` is [current, legacy]; `.rev()` processes legacy first.
    for base in database::DB_BASENAMES.iter().rev() {
        for suffix in database::DB_SIDECAR_SUFFIXES {
            let name = format!("{base}{suffix}");
            let src = data_dir.join(&name);
            if src.exists() {
                std::fs::rename(&src, data_dir.join(format!("{name}.moved-{stamp}")))?;
            }
        }
    }
    tracing::warn!("moved database aside; starting fresh");
    Ok(())
}

#[tauri::command]
async fn db_insert(
    table: String,
    data: Value,
    state: tauri::State<'_, DbState>,
) -> Result<String, String> {
    let conn = state.lock();
    database::db_insert(&conn, &table, data).map_err(|e| e.to_string())
}

#[tauri::command]
async fn db_select(
    table: String,
    query: Value,
    state: tauri::State<'_, DbState>,
) -> Result<Value, String> {
    let conn = state.lock();
    let rows = database::db_select(&conn, &table, query).map_err(|e| e.to_string())?;
    serde_json::to_value(rows).map_err(|e| e.to_string())
}

#[tauri::command]
async fn db_update(
    table: String,
    query: Value,
    data: Value,
    state: tauri::State<'_, DbState>,
) -> Result<usize, String> {
    let conn = state.lock();
    database::db_update(&conn, &table, query, data).map_err(|e| e.to_string())
}

#[tauri::command]
async fn db_delete(
    table: String,
    query: Value,
    state: tauri::State<'_, DbState>,
) -> Result<usize, String> {
    let conn = state.lock();
    database::db_delete(&conn, &table, query).map_err(|e| e.to_string())
}

#[tauri::command]
async fn db_upsert(
    table: String,
    data: Value,
    conflict_column: String,
    state: tauri::State<'_, DbState>,
) -> Result<String, String> {
    let conn = state.lock();
    database::db_upsert(&conn, &table, data, &conflict_column).map_err(|e| e.to_string())
}

#[tauri::command]
async fn db_count(
    table: String,
    query: Value,
    state: tauri::State<'_, DbState>,
) -> Result<i64, String> {
    let conn = state.lock();
    database::db_count(&conn, &table, query).map_err(|e| e.to_string())
}

#[tauri::command]
async fn db_query(
    sql: String,
    params: Vec<Value>,
    state: tauri::State<'_, DbState>,
) -> Result<Value, String> {
    let conn = state.lock();
    let rows = database::db_query(&conn, &sql, params).map_err(|e| e.to_string())?;
    serde_json::to_value(rows).map_err(|e| e.to_string())
}

/// Set or clear a per-agent custom binary path override. Writes (or deletes,
/// for an empty path) the `agent_bin_path_<id>` setting, then refreshes the
/// in-memory registry binary resolution reads — keeping the DB and the
/// registry in sync through a single call so the frontend doesn't have to.
#[tauri::command]
async fn set_agent_bin_override(
    id: String,
    path: Option<String>,
    ctx: tauri::State<'_, Arc<host::EngineCtx>>,
    state: tauri::State<'_, DbState>,
    supervisor: tauri::State<'_, Arc<Supervisor>>,
) -> Result<(), String> {
    // Scope the DB guard so it drops before the async respawn below — parking_lot
    // guards aren't Send across await, and the respawn re-locks the DB internally.
    {
        let conn = state.lock();
        let key = format!("{}{}", database::AGENT_BIN_PREFIX, id);
        match path.as_deref().map(str::trim) {
            Some(p) if !p.is_empty() => {
                database::db_upsert(&conn, "settings", json!({ "key": key, "value": p }), "key")
                    .map_err(|e| e.to_string())?;
            }
            _ => {
                database::db_delete(&conn, "settings", json!({ "where": { "key": key } }))
                    .map_err(|e| e.to_string())?;
            }
        }
        bin_resolve::set_agent_overrides(database::load_agent_bin_overrides(&conn));
    }
    // Restart any live agents on this provider so they exec the new binary.
    // Resolution happens only at spawn time, so without this an already-running
    // agent keeps the old binary (and thus the old account) on its next turn.
    supervisor.respawn_provider(ctx.inner(), &id).await;
    Ok(())
}

/// Flip the anonymous-telemetry consent flag. Persists it to `settings` (so the
/// renderer's `getAllSettings` sees it) and toggles the live pipeline, like
/// `set_agent_bin_override` keeps the DB and in-memory state in sync.
///
/// Note the snake_case key: `telemetry_enabled` is backend-owned (written here,
/// never via a frontend `setSetting`), so it intentionally breaks the camelCase
/// convention of frontend-set settings. The renderer reads it as
/// `s.telemetry_enabled`; do not introduce a `setSetting("telemetryEnabled", …)`
/// caller — that would write a different key and silently break the toggle.
#[tauri::command]
async fn set_telemetry_enabled(
    enabled: bool,
    state: tauri::State<'_, DbState>,
) -> Result<(), String> {
    {
        let conn = state.lock();
        database::set_setting(
            &conn,
            "telemetry_enabled",
            if enabled { "true" } else { "false" },
        )
        .map_err(|e| e.to_string())?;
    }
    telemetry::set_enabled(enabled);
    Ok(())
}

/// Flip code-indexing consent. Persists it to `settings` (so the renderer's
/// `getAllSettings` sees it as `s.code_indexing_enabled`) and updates the
/// in-process mirror the spawn path reads — the same persist-then-mirror shape
/// as `set_sandbox_engine`/`set_telemetry_enabled`. Backend-owned snake_case key.
///
/// Turning it ON kicks a best-effort background task: install the codegraph
/// bundle, then warm the index mirror for every pinned repo so the first agent
/// spawn after a fresh enable already has an index to copy in. Turning it OFF
/// does nothing else — indexes die with their workspaces, no cleanup needed.
#[tauri::command]
async fn set_code_indexing_enabled(
    enabled: bool,
    state: tauri::State<'_, DbState>,
    supervisor: tauri::State<'_, Arc<Supervisor>>,
) -> Result<(), String> {
    {
        let conn = state.lock();
        database::set_setting(
            &conn,
            codegraph::SETTING,
            if enabled { "true" } else { "false" },
        )
        .map_err(|e| e.to_string())?;
    }
    codegraph::set_enabled(enabled);
    if enabled {
        // Snapshot the pinned repos (path + owning project) up front so the
        // background task holds no DB handle across awaits.
        let repos: Vec<(String, PathBuf)> = supervisor
            .workspace
            .current()
            .map(|w| {
                w.projects
                    .into_iter()
                    .map(|p| (p.project_id, p.path))
                    .collect()
            })
            .unwrap_or_default();
        tauri::async_runtime::spawn(async move {
            warm_codegraph_index(repos).await;
        });
    }
    Ok(())
}

/// Best-effort: ensure codegraph is installed, then build/refresh the index
/// mirror for each `(project_id, source_repo)`. Every step logs and continues —
/// a failure here just means indexing warms up on a later spawn.
async fn warm_codegraph_index(repos: Vec<(String, PathBuf)>) {
    let bin = match codegraph::ensure_installed().await {
        Ok(bin) => bin,
        Err(e) => {
            tracing::warn!(error = %e, "codegraph install failed; indexing stays off until retry");
            return;
        }
    };
    for (project_id, source_repo) in repos {
        let mirror = match codegraph::mirror_dir(&project_id, &source_repo) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(error = %e, repo = %source_repo.display(), "codegraph mirror path failed");
                continue;
            }
        };
        if let Err(e) = codegraph::ensure_mirror(&source_repo, &mirror, &bin).await {
            tracing::warn!(error = %e, repo = %source_repo.display(), "codegraph mirror warm-up failed");
        }
    }
}

/// Send `app_opened` at most once per process.
///
/// Two callers race for it: `setup` at launch (for an already-onboarded
/// install) and the `track_app_opened` command when the onboarding overlay
/// mounts (for a fresh install, where the launch-time send is deferred until
/// the data-sharing disclosure is on screen). Replaying onboarding from
/// Settings › Developer puts both in play within one run, so without this flag
/// that replay would double-count the launch.
fn send_app_opened() {
    static SENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if SENT.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    telemetry::track("app_opened", json!({}));
}

/// Emit the deferred first `app_opened`. The frontend calls this when the
/// onboarding overlay mounts on a *fresh install* — the welcome step carries
/// the data-sharing disclosure, so the event lands only once consent has been
/// disclosed. On a fresh install `setup` skips the launch-time `app_opened`, so
/// this is the first such event.
///
/// Safe to call spuriously: `send_app_opened` collapses repeat calls, so a
/// StrictMode double-mount, the belt-and-braces call from `closeOnboarding`,
/// and an onboarding replay are all no-ops.
#[tauri::command]
fn track_app_opened() {
    send_app_opened();
}

/// Emit a product event raised by the renderer — the onboarding funnel and the
/// activation-path moments that only the frontend can observe (a step viewed, a
/// device-flow sign-in abandoned, a project added). Backend-observable events
/// (`agent_spawned`, `pr_opened`, `turn_completed`) stay in the backend.
///
/// Consent and identity are enforced downstream by `telemetry::track`, so this
/// is a no-op when the user has opted out.
///
/// `props` must stay categorical — counts, enums, booleans. Never pass paths,
/// repo/branch names, prompts, or raw error strings: the frontend helper
/// (`src/util/track.ts`) is the typed gate that keeps call sites honest.
#[tauri::command]
fn track_event(event: String, props: Option<serde_json::Value>) {
    telemetry::track(&event, props.unwrap_or_else(|| json!({})));
}

/// The persisted sandbox engine selection (`"sandbox-exec"` | `"docker"` |
/// `"podman"`).
/// Reads the in-memory mirror seeded at startup and kept in sync by
/// `set_sandbox_engine`, so no DB handle is needed.
#[tauri::command]
fn get_sandbox_engine() -> String {
    sandbox::selected_engine_kind().as_setting().to_string()
}

/// What each sandbox engine actually guarantees — one report per engine, so the
/// settings picker can show the trade rather than just the names.
///
/// Every engine rather than the selected one: neither is a superset of the other
/// (each is stronger on a different claim), so "which engine am I on" doesn't
/// answer "what am I protected from". Each report carries its `engine` spelling,
/// which joins `get_sandbox_engine`'s value.
#[tauri::command]
fn describe_sandbox_isolation() -> Vec<sandbox::IsolationReport> {
    sandbox::EngineKind::ALL
        .iter()
        .map(|&kind| sandbox::describe_isolation(kind))
        .collect()
}

/// Whether an agent must get the user's approval before publishing.
#[tauri::command]
fn get_publish_confirmation() -> bool {
    rpc::approval::enabled()
}

/// Turn the publish-approval prompt on or off.
///
/// Off by default and deliberately so: autopilot publishes while nobody is
/// watching, and a prompt would hang it until the decision timeout and then
/// refuse. Turning this on trades unattended publishing for a gate.
#[tauri::command]
fn set_publish_confirmation(enabled: bool, state: tauri::State<'_, DbState>) -> Result<(), String> {
    {
        let conn = state.lock();
        database::set_setting(
            &conn,
            rpc::approval::SETTING,
            if enabled { "true" } else { "false" },
        )
        .map_err(|e| e.to_string())?;
    }
    rpc::approval::set_enabled(enabled);
    Ok(())
}

/// Record the user's answer to one publish-approval prompt. An id that already
/// timed out is ignored, so a late answer can never publish anything.
#[tauri::command]
fn answer_publish_approval(id: String, approved: bool) {
    rpc::approval::answer(&id, approved);
}

/// Whether finishing a turn alerts at all (chime, banner, phone push); needing
/// input always does. Backend-owned so the phone push (which runs without a
/// DB handle) and the frontend read one key: `notify_turn_complete`.
#[tauri::command]
fn set_notify_turn_complete(enabled: bool, state: tauri::State<'_, DbState>) -> Result<(), String> {
    {
        let conn = state.lock();
        database::set_setting(
            &conn,
            remote::push::TURN_COMPLETE_SETTING,
            if enabled { "true" } else { "false" },
        )
        .map_err(|e| e.to_string())?;
    }
    remote::push::set_turn_complete(enabled);
    Ok(())
}

/// How long a publish-approval prompt waits before denying; `0` waits until
/// answered.
#[tauri::command]
fn set_publish_approval_wait(secs: u64, state: tauri::State<'_, DbState>) -> Result<(), String> {
    {
        let conn = state.lock();
        database::set_setting(&conn, rpc::approval::WAIT_SETTING, &secs.to_string())
            .map_err(|e| e.to_string())?;
    }
    rpc::approval::set_wait_secs(secs);
    Ok(())
}

/// The prefix prepended to every branch an agent creates. Validated and
/// trimmed; the stored form is returned so the UI shows exactly what applies.
/// Empty clears it.
#[tauri::command]
fn set_branch_prefix(prefix: String, state: tauri::State<'_, DbState>) -> Result<String, String> {
    let prefix = publish_prefs::validate_branch_prefix(&prefix)?;
    {
        let conn = state.lock();
        database::set_setting(&conn, publish_prefs::BRANCH_PREFIX_SETTING, &prefix)
            .map_err(|e| e.to_string())?;
    }
    publish_prefs::set_branch_prefix(&prefix);
    Ok(prefix)
}

/// Whether pull requests Fletch opens start as drafts.
#[tauri::command]
fn set_draft_prs(enabled: bool, state: tauri::State<'_, DbState>) -> Result<(), String> {
    {
        let conn = state.lock();
        database::set_setting(
            &conn,
            publish_prefs::DRAFT_PRS_SETTING,
            if enabled { "true" } else { "false" },
        )
        .map_err(|e| e.to_string())?;
    }
    publish_prefs::set_draft_prs(enabled);
    Ok(())
}

/// Change the sandbox engine stamped onto *new* agents. Container engines are
/// validated live before being accepted — Docker against a daemon probe, Podman
/// against `sandbox::podman::availability` plus
/// `sandbox::podman::launch_blocker` — so a success here means the choice is
/// actionable. Persists to `settings` and updates the in-memory mirror — like
/// `set_agent_bin_override` keeps the DB and process state in sync. Existing
/// agents are unaffected: each keeps the engine stamped on its record at
/// creation.
#[tauri::command]
async fn set_sandbox_engine(
    engine: String,
    state: tauri::State<'_, DbState>,
) -> Result<(), String> {
    let kind = sandbox::EngineKind::from_setting(&engine)
        .ok_or_else(|| format!("unknown sandbox engine: {engine}"))?;
    if kind == sandbox::EngineKind::Podman {
        // Same gate Docker gets below — a stored engine is stamped onto every
        // new agent, so one whose runtime can't launch fails every spawn — and
        // here rather than in the UI alone because the setting is reachable over
        // IPC. "Available" isn't "launchable": `podman info` can answer over a
        // remote default connection that every launch then refuses, hence the
        // blocker check on the same `spawn_blocking`.
        let (probe, blocker) = tauri::async_runtime::spawn_blocking(|| {
            let probe = sandbox::podman_availability();
            let blocker = matches!(probe, sandbox::PodmanAvailability::Available { .. })
                .then(sandbox::podman_launch_blocker)
                .flatten();
            (probe, blocker)
        })
        .await
        .map_err(|e| e.to_string())?;
        match probe {
            sandbox::PodmanAvailability::Available { .. } => {
                if let Some(blocker) = blocker {
                    return Err(blocker);
                }
            }
            sandbox::PodmanAvailability::NotInstalled => {
                return Err("Podman is not installed — install Podman first.".into())
            }
            sandbox::PodmanAvailability::MachineDown => {
                return Err(
                    "The Podman machine isn't running — run `podman machine start` first.".into(),
                )
            }
        }
    }
    if kind == sandbox::EngineKind::Docker {
        // `spawn_blocking`: the probe can block up to its 2s timeout.
        let probe = tauri::async_runtime::spawn_blocking(sandbox::docker_availability)
            .await
            .map_err(|e| e.to_string())?;
        match probe {
            sandbox::DockerAvailability::Available { .. } => {}
            sandbox::DockerAvailability::NotInstalled => {
                return Err("Docker is not installed — install Docker Desktop first.".into())
            }
            sandbox::DockerAvailability::DaemonDown => {
                return Err("Docker isn't running — start Docker Desktop first.".into())
            }
        }
    }
    {
        let conn = state.lock();
        database::set_setting(&conn, sandbox::ENGINE_SETTING, kind.as_setting())
            .map_err(|e| e.to_string())?;
    }
    sandbox::set_selected_engine_kind(kind);
    Ok(())
}

/// Probe the local Docker installation for the settings UI. Async +
/// `spawn_blocking` because the probe can block up to its 2s timeout.
#[tauri::command]
async fn probe_docker_engine() -> Result<sandbox::DockerAvailability, String> {
    tauri::async_runtime::spawn_blocking(sandbox::docker_availability)
        .await
        .map_err(|e| e.to_string())
}

/// Probe the local Podman installation for the settings UI. Async +
/// `spawn_blocking` because the probe can block up to its 2s timeout.
#[tauri::command]
async fn probe_podman_engine() -> Result<sandbox::PodmanAvailability, String> {
    tauri::async_runtime::spawn_blocking(sandbox::podman_availability)
        .await
        .map_err(|e| e.to_string())
}

/// Which step of the container auth chain would supply Anthropic credentials
/// to a docker agent right now — the settings UI status row. Async +
/// `spawn_blocking` because the first resolution may load the login-shell env
/// (runs a shell).
#[tauri::command]
async fn get_container_auth_status() -> Result<sandbox::docker::auth::ContainerAuthStatus, String> {
    tauri::async_runtime::spawn_blocking(sandbox::docker::auth::status)
        .await
        .map_err(|e| e.to_string())
}

/// Store a pasted `claude setup-token` for containerized agents under the
/// `claude_container_token` secret — the OS keychain on release macOS builds
/// (see `secrets`), the same posture as `github_token`. Trims; rejects empty;
/// unexpected shapes are accepted with a warning (which, like every log line
/// here, never includes the token itself). Persists and then updates the
/// in-process mirror, like `set_sandbox_engine` and `github::set_token`.
#[tauri::command]
async fn set_container_auth_token(
    token: String,
    state: tauri::State<'_, DbState>,
) -> Result<(), String> {
    store_container_token(&state, &token)
}

/// Persist-then-mirror core shared by the paste command
/// ([`set_container_auth_token`]) and the automated capture flow
/// ([`connect_claude_container_auth`]): normalize + shape-check (warning, never
/// logging the token, on an unrecognized shape), store the
/// `claude_container_token` secret, then update the in-process mirror the
/// spawn path reads — so a change applies to the next docker spawn without a
/// restart. Same shape as `github::set_token`.
fn store_container_token(db: &DbState, raw_token: &str) -> Result<(), String> {
    let (token, recognized) = sandbox::docker::auth::normalize_token(raw_token)?;
    if !recognized {
        tracing::warn!(
            "container auth token doesn't look like a `claude setup-token` value \
             (sk-ant-oat…); storing it anyway"
        );
    }
    {
        let conn = db.lock();
        secrets::set(&conn, sandbox::docker::auth::TOKEN_SETTING, &token)
            .map_err(|e| e.to_string())?;
    }
    sandbox::docker::auth::set_stored_token(Some(token));
    Ok(())
}

/// A `claude setup-token` capture in flight, held so the code-submit and cancel
/// commands can reach its live PTY. At most one runs at a time; dropping the
/// stored session kills the PTY (see [`sandbox::docker::setup_token`]).
type ClaudeSetupState = Mutex<Option<sandbox::docker::setup_token::ClaudeSetup>>;

/// Drive `claude setup-token` on the user's behalf: spawn it under a PTY, emit
/// the consent URL and the auth-code prompt to the UI (`claude-setup:url` /
/// `claude-setup:awaiting-code`), and — once the user completes browser consent
/// and submits the code via [`submit_claude_setup_code`] — capture the emitted
/// token and store it through the shared [`store_container_token`] path (no
/// paste, no restart). Resolves when the token is stored; errors on timeout,
/// cancel, a missing `claude` CLI, or an exit with no token. The token never
/// reaches the frontend or the logs.
#[tauri::command]
async fn connect_claude_container_auth(
    app: tauri::AppHandle,
    state: tauri::State<'_, DbState>,
    setup: tauri::State<'_, ClaudeSetupState>,
) -> Result<(), String> {
    use tauri::Emitter;

    let home = dirs::home_dir().ok_or("Could not determine your home directory.")?;
    let bin = bin_resolve::resolve_bin("claude", &home).ok_or(
        "Couldn't find the `claude` CLI on your PATH. Install Claude Code, then try again.",
    )?;

    let (tx, rx) = std::sync::mpsc::channel::<crate::error::Result<String>>();
    let emit_handle = app.clone();
    let emit: Arc<dyn Fn(sandbox::docker::setup_token::SetupEvent) + Send + Sync> =
        Arc::new(move |event| {
            use sandbox::docker::setup_token::SetupEvent;
            match event {
                SetupEvent::Url(url) => {
                    let _ = emit_handle.emit("claude-setup:url", url);
                }
                SetupEvent::AwaitingCode => {
                    let _ = emit_handle.emit("claude-setup:awaiting-code", ());
                }
            }
        });

    // Claim the single-flight slot atomically with the spawn: checking and
    // storing under one lock hold closes the check→store window a concurrent
    // connect (double "already in progress") or cancel (sees an empty slot,
    // no-ops, then this PTY publishes and runs uncancelled until timeout) would
    // otherwise slip through. `start` is a quick spawn and holds no `.await`.
    {
        let mut slot = setup.lock();
        if slot.is_some() {
            return Err("A Claude connection is already in progress.".into());
        }
        let session = sandbox::docker::setup_token::ClaudeSetup::start(
            std::path::Path::new(&bin),
            &std::env::temp_dir(),
            emit,
            tx,
        )
        .map_err(|e| e.to_string())?;
        *slot = Some(session);
    }

    // Wait off the async runtime: the user drives a browser consent in between,
    // so the ceiling is generous. Blank the slot on any outcome — dropping the
    // session kills the PTY (success, error, or timeout alike).
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        rx.recv_timeout(std::time::Duration::from_secs(300))
    })
    .await
    .map_err(|e| e.to_string())?;
    let _ = setup.lock().take();

    match outcome {
        Ok(Ok(token)) => store_container_token(&state, &token),
        Ok(Err(e)) => Err(e.to_string()),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            Err("Timed out waiting for the token. Re-run and complete the browser sign-in.".into())
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err("Claude setup ended unexpectedly.".into())
        }
    }
}

/// Feed the user's auth code to the live `claude setup-token` PTY started by
/// [`connect_claude_container_auth`].
#[tauri::command]
async fn submit_claude_setup_code(
    code: String,
    setup: tauri::State<'_, ClaudeSetupState>,
) -> Result<(), String> {
    let guard = setup.lock();
    match guard.as_ref() {
        Some(session) => session.submit_code(&code).map_err(|e| e.to_string()),
        None => Err("No Claude connection is in progress.".into()),
    }
}

/// Abandon an in-flight [`connect_claude_container_auth`]: drop the session
/// (killing the PTY), which makes the waiting connect command return an error
/// the frontend ignores via its run-id guard.
#[tauri::command]
async fn cancel_claude_container_auth(
    setup: tauri::State<'_, ClaudeSetupState>,
) -> Result<(), String> {
    setup.lock().take();
    Ok(())
}

/// Drop the stored container token (delete the secret + clear the mirror,
/// mirroring `github_disconnect`). Later chain steps take over, if any.
#[tauri::command]
async fn clear_container_auth_token(state: tauri::State<'_, DbState>) -> Result<(), String> {
    {
        let conn = state.lock();
        secrets::delete(&conn, sandbox::docker::auth::TOKEN_SETTING).map_err(|e| e.to_string())?;
    }
    sandbox::docker::auth::set_stored_token(None);
    Ok(())
}

/// Persist the docker launch knobs (`docker_image` override + `docker_memory` /
/// `docker_cpus` limits) and update the in-process mirror the spawn path reads,
/// so a change applies to the next docker spawn without a restart. Blank values
/// clear the setting (the launch path falls back to its defaults). Same
/// persist-then-mirror shape as `set_sandbox_engine` — the mirror
/// (`sandbox::docker::LaunchSettings`) is the whole struct, so all three are
/// written together.
#[tauri::command]
async fn set_docker_launch_settings(
    image: Option<String>,
    memory: Option<String>,
    cpus: Option<String>,
    state: tauri::State<'_, DbState>,
) -> Result<(), String> {
    // Blank → None: a cleared field must not be stored as a launch override
    // (an empty `--memory`/`--cpus` value or `docker_image` would break `docker
    // run`), and the mirror treats blank as "use default" anyway.
    let norm = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let image = norm(image);
    let memory = norm(memory);
    let cpus = norm(cpus);
    {
        let conn = state.lock();
        // All three must land together. Written individually, a mid-loop
        // failure (image commits, then memory errors) would leave a mixed
        // config committed to the DB — one the UI never shows, since it reverts
        // all three optimistically and we skip the mirror update on error, so a
        // restart would silently hydrate the partial write. The transaction
        // rolls back on any failure, keeping DB, mirror, and UI in sync.
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        for (key, value) in [
            (sandbox::docker::IMAGE_SETTING, &image),
            (sandbox::docker::MEMORY_SETTING, &memory),
            (sandbox::docker::CPUS_SETTING, &cpus),
        ] {
            database::set_setting(&tx, key, value.as_deref().unwrap_or(""))
                .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
    }
    sandbox::docker::set_launch_settings(sandbox::docker::LaunchSettings {
        image_override: image,
        memory,
        cpus,
    });
    Ok(())
}

/// Persist the podman launch knobs (`podman_image` override + `podman_memory` /
/// `podman_cpus` limits) and update the in-process mirror the spawn path reads.
/// The docker command's twin in every respect but the keys and the mirror it
/// writes — the two runtimes keep separate knobs so a user running both can
/// point each at its own image and limits.
#[tauri::command]
async fn set_podman_launch_settings(
    image: Option<String>,
    memory: Option<String>,
    cpus: Option<String>,
    state: tauri::State<'_, DbState>,
) -> Result<(), String> {
    // Blank → None, as in the docker twin above: a cleared field must not be
    // stored as a launch override.
    let norm = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let image = norm(image);
    let memory = norm(memory);
    let cpus = norm(cpus);
    {
        let conn = state.lock();
        // All three must land together, for the reason the docker twin above
        // spells out: a partial write is a config the UI never shows.
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        for (key, value) in [
            (sandbox::podman::IMAGE_SETTING, &image),
            (sandbox::podman::MEMORY_SETTING, &memory),
            (sandbox::podman::CPUS_SETTING, &cpus),
        ] {
            database::set_setting(&tx, key, value.as_deref().unwrap_or(""))
                .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
    }
    sandbox::podman::set_launch_settings(sandbox::podman::LaunchSettings {
        image_override: image,
        memory,
        cpus,
    });
    Ok(())
}

/// Set once the user has confirmed a quit (via the active-work dialog) or a
/// termination signal has already killed the children — both mean the next
/// `ExitRequested` must proceed straight to shutdown without re-prompting.
static QUIT_CONFIRMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Reveal and focus the main window — the tray "Open Fletch" action and a
/// left-click on the tray icon. Closing the window only hides it, so this is
/// how a menu-bar-resident app comes back to the foreground.
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// Live count of agents whose runtime status is `Running` (the in-memory
/// source of truth; a resting DB record derives to `Idle`).
fn running_agent_count(app: &tauri::AppHandle) -> usize {
    app.try_state::<Arc<Supervisor>>()
        .map(|s| {
            s.statuses
                .lock()
                .values()
                .filter(|st| **st == crate::workspace::AgentStatus::Running)
                .count()
        })
        .unwrap_or(0)
}

/// Count workflow runs in an active (`pending`/`running`) state, read straight
/// from the DB so a run whose driver is mid-restart still counts.
fn active_run_count(app: &tauri::AppHandle) -> usize {
    app.try_state::<DbState>()
        .and_then(|db| {
            db.lock()
                .query_row(
                    "SELECT COUNT(*) FROM wf_run WHERE status IN ('pending','running')",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .ok()
        })
        .map(|n| n as usize)
        .unwrap_or(0)
}

/// Body of the "quit while work is active" confirm dialog.
fn quit_warning(agents: usize, runs: usize) -> String {
    let mut parts = Vec::new();
    if agents > 0 {
        parts.push(format!(
            "{agents} agent{} working",
            if agents == 1 { "" } else { "s" }
        ));
    }
    if runs > 0 {
        parts.push(format!(
            "{runs} workflow{} running",
            if runs == 1 { "" } else { "s" }
        ));
    }
    format!(
        "{}. Quitting stops them now.\n\nWorkflows resume when you reopen Fletch; \
         ad-hoc agents will need a manual resume.",
        parts.join(", ")
    )
}

/// The tray's status-line menu item, published here once (and if) the tray is
/// successfully built. The activity monitor's callback writes through it when
/// present and no-ops when absent — so the tray and the sleep assertion are
/// fully decoupled: a tray that never appears leaves this `None` forever without
/// affecting activity tracking.
type TrayStatusSlot = Arc<Mutex<Option<tauri::menu::MenuItem<tauri::Wry>>>>;

/// Arm the activity monitor and start the status-event subscription. This is the
/// sleep-assertion + activity-tracking half of the menu-bar feature and MUST NOT
/// depend on the tray: it is called unconditionally and cannot fail, so a
/// later tray-build error can never leave the monitor unarmed (which would
/// silently disable idle-sleep prevention mid-fleet). `status_slot` is where the
/// tray, if it builds, publishes its status item for the callback to update.
///
/// Called from inside `host::boot` (through `BootConfig::on_supervisor`) so the
/// subscription is live before any run resumes.
fn arm_activity_monitor(supervisor: &Arc<Supervisor>, status_slot: &TrayStatusSlot) {
    // The status-line callback updates the tray item *only if one has been
    // published*; with no tray it is a no-op, while activity is still tracked
    // and the assertion still toggles.
    {
        let status_slot = status_slot.clone();
        power::ActivityMonitor::global().arm(std::sync::Arc::new(move |agents, runs| {
            if let Some(item) = status_slot.lock().as_ref() {
                let _ = item.set_text(power::status_line(agents, runs));
            }
        }));
    }

    // Feed agent activity off the existing status broadcast — no polling.
    // Subscribe first, then seed the running set, then follow transitions: the
    // subscribe-before-read discipline (see `Supervisor::subscribe_status`)
    // makes a fast Running→Idle flap unlosable. A lagged receiver resyncs from
    // the live status map.
    let running_of = |sup: &Arc<Supervisor>| -> std::collections::HashSet<String> {
        sup.statuses
            .lock()
            .iter()
            .filter(|(_, s)| **s == crate::workspace::AgentStatus::Running)
            .map(|(id, _)| id.clone())
            .collect()
    };
    let mut rx = supervisor.subscribe_status();
    power::ActivityMonitor::global().resync_agents(running_of(supervisor));
    let supervisor = supervisor.clone();
    tauri::async_runtime::spawn(async move {
        use tokio::sync::broadcast::error::{RecvError, TryRecvError};
        loop {
            match rx.recv().await {
                Ok(ev) => power::ActivityMonitor::global().set_agent_running(
                    &ev.agent_id,
                    ev.status == crate::workspace::AgentStatus::Running,
                ),
                Err(RecvError::Lagged(_)) => {
                    // Skipped events. The live status map is the source of
                    // truth — `set_status` updates it *before* it broadcasts —
                    // so it is never staler than a dropped event. Drain the
                    // still-buffered backlog first (all of it predates "now"),
                    // *then* snapshot: resyncing after the drain means a stale
                    // buffered event can't be re-applied on top of the fresh
                    // snapshot (the bug: a lag across Running→Idle could
                    // otherwise leave the assertion stuck active), while an
                    // event landing mid-drain is captured by the later snapshot.
                    while let Ok(_) | Err(TryRecvError::Lagged(_)) = rx.try_recv() {}
                    power::ActivityMonitor::global().resync_agents(running_of(&supervisor));
                }
                Err(RecvError::Closed) => break,
            }
        }
    });
}

/// Build the menu-bar tray (best-effort UI). On success, publish its status item
/// into `status_slot` so the already-armed monitor starts updating the menu
/// line. A failure here is non-fatal — the monitor stays armed regardless (see
/// `arm_activity_monitor`); the caller logs and continues.
fn setup_tray(app: &tauri::AppHandle, status_slot: &TrayStatusSlot) -> tauri::Result<()> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let status_item = MenuItemBuilder::with_id("fletch:status", power::status_line(0, 0))
        .enabled(false)
        .build(app)?;
    let open_item = MenuItemBuilder::with_id("fletch:open", "Open Fletch").build(app)?;
    let quit_item = MenuItemBuilder::with_id("fletch:quit", "Quit Fletch").build(app)?;
    let menu = MenuBuilder::new(app)
        .item(&open_item)
        .separator()
        .item(&status_item)
        .separator()
        .item(&quit_item)
        .build()?;

    let mut tray = TrayIconBuilder::with_id("fletch:tray")
        .tooltip("Fletch")
        .menu(&menu)
        // macOS: keep left-click for revealing the window; the menu opens on
        // right-click (the platform convention for a status item).
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "fletch:open" => show_main_window(app),
            // Goes through `ExitRequested`, so the active-work confirm still
            // applies to a tray-initiated quit.
            "fletch:quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    // macOS: a monochrome template mark (black pixels + alpha, generated by
    // `scripts/gen_tray_icon.py`). `icon_as_template(true)` lets the system tint
    // it for light/dark menu bars and click-highlight. Shipped as raw 44x44
    // RGBA and compiled in via `include_bytes!` — so no PNG-decode crate is
    // pulled in and a missing/wrong-size asset is a build error, not a silent
    // runtime fallback.
    #[cfg(target_os = "macos")]
    {
        const TRAY_RGBA: &[u8] = include_bytes!("../icons/tray-macos-template.rgba");
        const TRAY_DIM: u32 = 44;
        const _: () = assert!(TRAY_RGBA.len() == (TRAY_DIM * TRAY_DIM * 4) as usize);
        let icon = tauri::image::Image::new(TRAY_RGBA, TRAY_DIM, TRAY_DIM);
        tray = tray.icon(icon).icon_as_template(true);
    }
    // Other platforms have no template-image semantics; use the color app icon
    // so the tray entry is visible.
    #[cfg(not(target_os = "macos"))]
    if let Some(icon) = app.default_window_icon().cloned() {
        tray = tray.icon(icon);
    }
    let _tray = tray.build(app)?;

    // Publish the status item and render the current line at once (the monitor
    // is already armed and may have counted activity before the tray appeared).
    *status_slot.lock() = Some(status_item);
    power::ActivityMonitor::global().refresh();

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // rustls has two crypto backends compiled in (see `rustls` in Cargo.toml)
    // and panics on the first TLS client it is asked to build unless one has
    // been installed as the process default. The relay link was that first
    // client on autostart, and its task died with the panic — so the relay
    // only ever came up when switched on by hand, after something else had
    // installed a provider. `Err` means one is already installed; fine.
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Error/crash reporting. The DSN is baked in at build time via
    // `QUORUM_SENTRY_DSN` (empty/unset → a disabled, no-op client, so dev and
    // unconfigured builds send nothing). This captures app health — Rust
    // panics and unhandled frontend errors — not user telemetry, so it is on
    // regardless of any future data-sharing toggle. `_sentry` must stay bound
    // for the whole process so the client flushes on exit; `run()` blocks
    // below, so this scope lives until quit.
    //
    // Because crash reporting is not gated on consent, the payload carries the
    // privacy burden: `before_send`/`before_breadcrumb` scrub every event and
    // breadcrumb down to its static message plus a small allowlist of
    // categorical fields (paths, argv, error strings, hostname, etc. never
    // egress). See `sentry_scrub` for the invariant and how to allowlist a new
    // field.
    let _sentry = sentry::init((
        option_env!("QUORUM_SENTRY_DSN").filter(|s| !s.is_empty()),
        sentry::ClientOptions {
            release: sentry::release_name!(),
            before_send: Some(std::sync::Arc::new(sentry_scrub::scrub_event)),
            before_breadcrumb: Some(std::sync::Arc::new(sentry_scrub::scrub_breadcrumb)),
            ..Default::default()
        },
    ));

    // Native hard-crash capture (segfault, abort, stack overflow) — things the
    // panic hook can't see. Spawns a lightweight handler child that re-execs
    // this binary, watches the parent, and uploads a minidump via the sentry
    // client on a crash. Only when a DSN is configured: with no DSN there's
    // nothing to upload, so we skip spawning the child entirely (dev stays
    // clean). Bound for the process lifetime so the handler keeps running.
    // Placed before `init_logging` so the handler child is detected and exits
    // before touching the log file or building the app.
    #[cfg(not(target_os = "ios"))]
    let _minidump = _sentry
        .is_enabled()
        .then(|| tauri_plugin_sentry::minidump::init(&_sentry));

    init_logging();
    install_panic_logging();

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_sentry::init(&_sentry))
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_notification::init());

    // Persist window size/position/maximized state across restarts. Desktop
    // only — the plugin isn't available on mobile targets. VISIBLE is excluded
    // from the tracked flags so the plugin doesn't reveal the window during
    // restore (which happens early, before the webview has painted, and is the
    // source of the white launch flash). The frontend shows it after first
    // paint instead — see `revealAppWindow` / `src/main.tsx`.
    #[cfg(desktop)]
    let builder = builder.plugin(
        tauri_plugin_window_state::Builder::default()
            .with_state_flags(
                tauri_plugin_window_state::StateFlags::all()
                    & !tauri_plugin_window_state::StateFlags::VISIBLE,
            )
            .build(),
    );

    builder
        .setup(|app| {
            let app_data = app.path().app_data_dir()?;
            let data_dir = if cfg!(debug_assertions) {
                app_data.join("dev")
            } else {
                app_data
            };
            std::fs::create_dir_all(&data_dir)?;

            // Whisper dictation weights live under the data dir; the engine
            // loads from here and Settings downloads into it. Before `boot`
            // because `boot` can bring the remote listener up, and a phone's
            // dictation op reads this root.
            dictation::whisper::init(data_dir.join("whisper-models"));

            // Where the tray publishes its status item once (and if) it builds.
            // Created before the engine because the activity monitor — armed
            // inside `boot`, see `on_supervisor` below — writes through it.
            let tray_status_slot: TrayStatusSlot = Arc::new(Mutex::new(None));

            // The engine: everything that is not about a window. The same
            // function a headless host calls, in-process here (see `host::boot`
            // for the step order, which is unchanged from when it lived inline).
            let host::Engine {
                ctx: engine_ctx,
                db,
                supervisor,
                workflows: wf_service,
                remote: remote_state,
                ..
            } = host::boot(host::BootConfig {
                data_dir: data_dir.clone(),
                // The desktop's checkout and mailbox roots are its own
                // `~/.fletch` — unless a parent Fletch redirected this process
                // (the nested-Fletch Run), in which case they are the parent's
                // and this engine sweeps neither.
                state_roots: host::StateRoots::from_env(),
                // Events reach the webview exactly as they did when the engine
                // held the `AppHandle` itself; `boot` fans this out to its own
                // broadcast for the remote taps.
                sink: Arc::new(TauriSink(app.handle().clone())),
                focus: {
                    let focus_app = app.handle().clone();
                    Box::new(move || {
                        focus_app
                            .get_webview_window("main")
                            .map(|window| window.is_focused().unwrap_or(false))
                            .unwrap_or(false)
                    })
                },
                // Tauri's runtime, which is a tokio runtime: the engine's
                // background tasks keep running on the same threads as before.
                runtime: tauri::async_runtime::handle().inner().clone(),
                remote: host::RemoteBoot::Desktop,
                // The five `dictation_*` ops: the engine keeps them on the wire
                // and asks this for them, because whisper.cpp is built on macOS
                // only and lives here beside the mic code it shares.
                dictation: Some(Box::new(|ctx| {
                    Arc::new(dictation::dispatch::DictationDispatch::new(ctx))
                })),
                // A failed `database::init` is a native dialog and a retry loop
                // on the main thread, not an error the engine can resolve.
                recover_db: Some({
                    let data_dir = data_dir.clone();
                    Box::new(move |e| recover_from_db_init_failure(&data_dir, e))
                }),
                // Arm the activity monitor (idle-sleep assertion + activity
                // tracking) *before* any work is resumed inside `boot`, so the
                // run-level signal is tight from the first instant: runs
                // re-driven by `resume_active_runs` register into an
                // already-armed monitor (and subscribing before resume means no
                // resumed agent's status event is missed). Infallible and
                // independent of the tray — a tray failure can never leave
                // sleep-prevention disabled. The tray is built below and
                // late-publishes its status item into this slot.
                on_supervisor: Some({
                    let slot = tray_status_slot.clone();
                    Box::new(move |supervisor| arm_activity_monitor(supervisor, &slot))
                }),
                // A signal (logout/shutdown/Ctrl-C) is not a user choice we can
                // prompt on — once the children are dead, flag the quit as
                // confirmed so the `ExitRequested` handler skips the active-work
                // dialog, and let Tauri exit.
                signals: Some({
                    let handle = app.handle().clone();
                    Box::new(move || {
                        QUIT_CONFIRMED.store(true, std::sync::atomic::Ordering::SeqCst);
                        handle.exit(0);
                    })
                }),
            })?;

            // Anonymous product telemetry. Mint (or read) the install's random
            // distinct id, read the opt-out consent flag, and detect a version
            // change since the last launch — all from `settings`, before any
            // event fires. No-op in unconfigured builds (no PostHog key baked
            // in), so dev sends nothing.
            let version = app.package_info().version.to_string();
            let (distinct_id, telemetry_enabled, onboarding_complete, prev_version) = {
                let conn = db.lock();
                let distinct_id = match database::get_setting(&conn, "telemetry_distinct_id") {
                    Some(id) if !id.trim().is_empty() => id,
                    _ => {
                        let id = uuid::Uuid::new_v4().to_string();
                        let _ = database::set_setting(&conn, "telemetry_distinct_id", &id);
                        id
                    }
                };
                // Opt-out: anything but an explicit "false" means enabled.
                let enabled =
                    database::get_setting(&conn, "telemetry_enabled").as_deref() != Some("false");
                // Frontend-owned flag (camelCase), written when the user finishes
                // the first-run onboarding — the flow that carries the data-sharing
                // disclosure. Absent on a brand-new install.
                let onboarded =
                    database::get_setting(&conn, "onboardingComplete").as_deref() == Some("true");
                let prev = database::get_setting(&conn, "last_seen_version");
                let _ = database::set_setting(&conn, "last_seen_version", &version);
                (distinct_id, enabled, onboarded, prev)
            };
            telemetry::init(distinct_id, telemetry_enabled, version.clone());
            // On a fresh install the first `app_opened` is deferred until the
            // onboarding overlay is on screen (see the `track_app_opened`
            // command) — its welcome step carries the data-sharing disclosure,
            // so no event is sent before the user has been told. Once onboarded,
            // every launch reports `app_opened` here as usual.
            if onboarding_complete {
                send_app_opened();
            }
            if let Some(prev) = prev_version {
                if !prev.is_empty() && prev != version {
                    telemetry::track(
                        "app_updated",
                        json!({ "from_version": prev, "to_version": version }),
                    );
                }
            }

            // Dictation's auto-stop opt-out, mirrored in-process for the
            // capture threads that have no DB handle. Desktop-side: it belongs
            // to a capture session, not to the engine.
            dictation::set_auto_stop(dictation::parse_auto_stop(
                database::get_setting(&db.lock(), dictation::AUTO_STOP_SETTING).as_deref(),
            ));

            // Everything the 212 commands read back out of managed state. The
            // engine reaches these through its `EngineCtx` instead; `manage` is
            // the desktop's own lookup table.
            app.manage(db);
            app.manage(engine_ctx);
            app.manage(supervisor);
            app.manage(wf_service);
            // At most one `claude setup-token` capture runs at a time; the
            // code-submit / cancel commands reach it through this slot.
            app.manage(ClaudeSetupState::default());
            // Live provider sign-in PTYs (Settings → Providers), one per
            // provider. Empty until the user starts one.
            app.manage(provider_login::ProviderLoginSessions::default());
            // Paired-device remote access, as `boot` left it: the taps are in
            // and the listener is running if the user had it on.
            #[cfg(desktop)]
            if let Some(state) = remote_state {
                app.manage(state);
            }

            // The other direction: this Mac as a client of other Fletch hosts.
            // Independent of the block above — no listener, no device store,
            // its own key — and unconditional, because the four commands are
            // registered unconditionally and a command whose state is not
            // managed panics when the webview calls it.
            app.manage(client::dialer(app.handle(), &data_dir.join("remote")));

            // Menu-bar tray (close-to-tray + status line) — the second half of
            // the laptop-GUI hardening feature; the first half (the activity
            // monitor) was already armed above, before any work resumed.
            // Best-effort: a failure logs and continues — the app still starts
            // and the monitor stays armed. The tray publishes its status item
            // into `tray_status_slot` for the monitor's status-line callback,
            // then `setup_tray` refreshes the line to reflect activity that may
            // already have been counted before the tray appeared.
            if let Err(e) = setup_tray(app.handle(), &tray_status_slot) {
                tracing::error!(error = %e, "menu-bar tray setup failed; continuing without it");
            }

            Ok(())
        })
        // Close ≠ quit: closing the main window hides it and the app keeps
        // running in the menu bar (reopen via the tray). Cmd-Q still quits —
        // that path fires `ExitRequested`, handled in `run` below. Following
        // the macOS convention keeps a long fleet alive across an accidental
        // window close.
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
            // Dropping a file is as much a user gesture as picking one, but the
            // dialog plugin only grants the fs scope for paths IT returned, so a
            // dropped path arrives with none and the webview's `stat`/`readFile`
            // is refused (src/components/Composer/stageAttachment.ts). Granted
            // per path here, through the same `allow_file` the dialog uses. The
            // alternative — a home-wide scope in the capability file — would
            // hand the webview's `fs:allow-write-text-file` every text file
            // under $HOME, for the sake of reading the one that was dropped.
            //
            // On ordering, since this grant has to be in place before the JS
            // side acts on the same drop: this listener runs AFTER tauri has
            // emitted `tauri://drag-drop` to the webview, not before — the
            // internal handler goes first and the global listeners after it
            // (tauri/src/manager/window.rs, `on_window_event`). It is still in
            // time. The emit only schedules the event's delivery into the
            // webview, while this runs to completion on the main thread before
            // the event loop turns; and the JS side's first fs call is an
            // awaited `stat`, which is another IPC round trip back into this
            // process.
            tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) => {
                // `try_fs_scope` rather than `fs_scope`: the latter panics if
                // the fs plugin is absent, and a drop must never take the app
                // down over a file it could not read.
                let Some(scope) = tauri_plugin_fs::FsExt::try_fs_scope(window) else {
                    tracing::warn!("fs plugin not registered; dropped paths stay unreadable");
                    return;
                };
                for path in paths {
                    if let Err(e) = scope.allow_file(path) {
                        tracing::warn!(
                            error = %e,
                            path = %path.display(),
                            "could not grant a dropped path to the fs scope"
                        );
                    }
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            db_insert,
            db_select,
            db_update,
            db_delete,
            db_upsert,
            db_count,
            db_query,
            set_agent_bin_override,
            set_telemetry_enabled,
            set_code_indexing_enabled,
            track_app_opened,
            track_event,
            get_sandbox_engine,
            set_sandbox_engine,
            describe_sandbox_isolation,
            get_publish_confirmation,
            set_publish_confirmation,
            answer_publish_approval,
            set_publish_approval_wait,
            set_branch_prefix,
            set_draft_prs,
            set_notify_turn_complete,
            probe_docker_engine,
            probe_podman_engine,
            get_container_auth_status,
            set_container_auth_token,
            clear_container_auth_token,
            connect_claude_container_auth,
            submit_claude_setup_code,
            cancel_claude_container_auth,
            set_docker_launch_settings,
            set_podman_launch_settings,
            commands::wf_list_runs,
            commands::wf_get_run,
            commands::wf_events,
            commands::wf_launch,
            commands::wf_cancel,
            commands::wf_resume,
            commands::wf_retry,
            commands::wf_approve,
            commands::wf_reject,
            commands::wf_run_diff,
            commands::wf_resolve_conflict,
            commands::wf_delete_run,
            commands::wf_answer,
            commands::wf_def_save,
            commands::wf_def_list,
            commands::wf_def_delete,
            commands::wf_def_export_yaml,
            commands::wf_def_import_yaml,
            commands::roadmap_list_items,
            commands::roadmap_get_item,
            commands::roadmap_create_item,
            commands::roadmap_update_item,
            commands::roadmap_set_rank,
            commands::roadmap_hand_off_item,
            commands::roadmap_item_review,
            commands::roadmap_merge_item_pr,
            commands::roadmap_note_review_feedback,
            commands::roadmap_hold_item,
            commands::roadmap_release_item,
            commands::roadmap_hold_project,
            commands::roadmap_release_project,
            commands::roadmap_get_project_hold,
            commands::roadmap_reclaim_item,
            commands::roadmap_reject_item,
            commands::roadmap_reopen_item,
            commands::roadmap_delete_item,
            commands::roadmap_list_item_events,
            commands::roadmap_latest_events,
            commands::roadmap_list_proposals,
            commands::roadmap_accept_proposal,
            commands::roadmap_reject_proposal,
            commands::roadmap_get_order_proposal,
            commands::roadmap_accept_order_proposal,
            commands::roadmap_reject_order_proposal,
            commands::roadmap_get_brief,
            commands::roadmap_get_brief_proposal,
            commands::roadmap_accept_brief_proposal,
            commands::roadmap_reject_brief_proposal,
            oauth::oauth_device_login,
            commands::get_workspace,
            commands::wf_run_agents,
            commands::list_project_chats,
            commands::agent_head_sha,
            commands::add_workspace_repo,
            commands::remove_workspace_repo,
            commands::attach_repo_to_project,
            commands::detach_repo_from_project,
            commands::set_repo_label,
            commands::rename_project,
            commands::delete_project,
            commands::project_has_running_agents,
            commands::relocate_repo,
            commands::gh_status,
            commands::gh_repo_list,
            commands::clone_repo,
            commands::create_repo,
            commands::publish_agent,
            commands::github_disconnect,
            commands::spawn_agent,
            commands::fork_agent,
            commands::write_to_agent,
            commands::send_user_message,
            commands::answer_tool_use,
            commands::resize_agent,
            commands::switch_view,
            commands::set_agent_effort,
            commands::set_agent_model,
            commands::resume_agent,
            commands::stop_agent,
            commands::discard_agent,
            commands::archive_agent,
            commands::restore_agent,
            commands::read_session_records,
            commands::read_user_turns,
            commands::sync_session,
            commands::append_live_record,
            commands::add_repo_to_agent,
            commands::allocate_draft_name,
            commands::get_git_state,
            commands::get_all_shortstats,
            commands::get_all_git_meta,
            commands::refresh_base_freshness,
            commands::push_agent,
            commands::pull_agent,
            commands::rebase_agent,
            commands::commit_agent,
            commands::discard_agent_changes,
            commands::stash_agent,
            commands::abort_merge_agent,
            commands::clear_checkout_config,
            commands::delete_branch_agent,
            commands::list_repo_branches,
            commands::repo_default_branch,
            commands::create_pr,
            commands::merge_pr,
            commands::get_pr_state,
            commands::refresh_all_pr_status,
            commands::get_pr_checks,
            commands::get_pr_live,
            commands::get_pr_history,
            commands::get_pr_threads,
            commands::open_agent_shell,
            commands::close_agent_shell,
            commands::write_to_shell,
            commands::resize_shell,
            commands::open_provider_login,
            commands::close_provider_login,
            commands::write_provider_login,
            commands::resize_provider_login,
            commands::run_start,
            commands::run_stop,
            commands::run_state,
            commands::detect_run_config,
            commands::run_verification,
            commands::project_run_config,
            commands::read_env_file_keys,
            commands::get_env_override,
            commands::set_env_override,
            commands::clear_env_override,
            commands::list_checkout_tree,
            commands::list_dir,
            commands::discover_slash_commands,
            commands::run_claude_command,
            commands::list_prs,
            commands::list_repo_prs,
            commands::list_tracker_issues,
            commands::issue_comments,
            commands::set_agent_issue_ref,
            commands::linear_status,
            commands::linear_connect,
            commands::linear_disconnect,
            commands::linear_list_teams,
            commands::list_repo_tree,
            commands::read_checkout_file,
            commands::get_file_diff,
            commands::write_checkout_file,
            commands::rename_checkout_path,
            commands::delete_checkout_path,
            commands::create_checkout_file,
            commands::create_checkout_dir,
            commands::copy_checkout_file,
            commands::save_pasted_attachment,
            commands::probe_provider_versions,
            commands::check_cli,
            commands::git_dist_install,
            commands::install_agent,
            commands::cancel_agent_install,
            commands::validate_agent_bin,
            commands::discover_supported_models,
            commands::scan_usage_transcripts,
            commands::probe_provider_auth,
            commands::reveal_logs,
            commands::start_docker_desktop,
            commands::detect_editors,
            commands::open_in_editor,
            commands::submit_feedback,
            // Paired-device remote access. `generate_handler!` takes a flat
            // path list with no room for a `cfg`, so these ride along
            // unconditionally; the module behind them is desktop-gated, which
            // is the only target this app ships for.
            commands::remote_status,
            commands::remote_set_enabled,
            commands::remote_set_port,
            commands::remote_set_relay,
            commands::remote_begin_pairing,
            commands::remote_revoke_device,
            // This Mac as a client of other hosts (`client/`), the same four
            // commands the phone registers over the same dialer.
            client::remote_connect,
            client::remote_send,
            client::remote_close,
            client::remote_device_public_key,
            dictation::dictation_availability,
            dictation::dictation_start,
            dictation::dictation_stop,
            dictation::dictation_model_status,
            dictation::set_dictation_engine,
            dictation::set_dictation_model,
            dictation::set_dictation_auto_stop,
            dictation::dictation_model_download,
            dictation::dictation_model_remove,
        ])
        .build(tauri::generate_context!())
        .expect("error while building fletch")
        .run(|app, event| {
            // Dock-icon click (or any macOS reopen) while the app is running
            // but the main window is hidden — closing the window only hides it
            // (see the `CloseRequested` handler), so re-show it here, matching
            // the tray's "Open Fletch" behavior. macOS-only event.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                show_main_window(app);
                return;
            }
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                use std::sync::atomic::Ordering;
                // Quit confirmation: a quit kills every live agent/run child
                // (below), so when work is active we prompt first — unless the
                // user already confirmed, or a termination signal set the flag
                // (see the SIGINT/SIGTERM handler). Cancel keeps the app
                // running; Quit re-triggers the exit with the flag set, so the
                // second `ExitRequested` falls straight through to shutdown.
                if !QUIT_CONFIRMED.load(Ordering::SeqCst) {
                    let agents = running_agent_count(app);
                    let runs = active_run_count(app);
                    if agents > 0 || runs > 0 {
                        api.prevent_exit();
                        let handle = app.clone();
                        use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
                        app.dialog()
                            .message(quit_warning(agents, runs))
                            .title("Quit Fletch?")
                            .buttons(MessageDialogButtons::OkCancelCustom(
                                "Quit".into(),
                                "Cancel".into(),
                            ))
                            .show(move |confirmed| {
                                if confirmed {
                                    QUIT_CONFIRMED.store(true, Ordering::SeqCst);
                                    handle.exit(0);
                                }
                            });
                        return;
                    }
                }

                // No active work, or the user confirmed: proceed. Explicitly
                // kill every live agent/shell/run child — tauri-managed state
                // isn't reliably dropped on macOS app termination, so the
                // per-session Drop impls can't be trusted to fire; without this,
                // quitting mid-run orphans the processes.
                if let Some(supervisor) = app.try_state::<Arc<Supervisor>>() {
                    supervisor.shutdown();
                }
                // Same reasoning for a sign-in left open in Settings: clearing
                // the map drops each session, which kills its PTY.
                if let Some(logins) = app.try_state::<provider_login::ProviderLoginSessions>() {
                    logins.lock().clear();
                }
                // Give in-flight telemetry sends a brief, bounded chance to
                // finish before the runtime tears down, rather than dropping
                // events that fired just before quit (e.g. `pr_opened`).
                tauri::async_runtime::block_on(telemetry::flush(std::time::Duration::from_secs(3)));
            }
        });
}
