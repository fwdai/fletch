mod activity;
mod agent;
mod agent_install;
mod agent_profile;
mod bin_resolve;
mod child_io;
mod commands;
mod database;
mod editors;
mod error;
mod exec_session;
mod git;
mod git_dist;
mod git_state;
mod github;
mod instructions;
mod managed_session;
mod message_queue;
mod model_catalog;
mod names;
mod native_input;
mod new_project;
mod oauth;
mod pty_session;
mod rpc;
mod run_detect;
mod run_session;
mod sandbox;
mod secrets;
mod sentry_scrub;
mod supervisor;
mod telemetry;
mod transcripts;
mod workflow;
mod workflows;
mod workspace;

use parking_lot::Mutex;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::Manager;

use crate::supervisor::Supervisor;
use crate::workspace::WorkspaceManager;

type DbState = Arc<Mutex<Connection>>;

/// The app's bundle identifier. Must match `identifier` in `tauri.conf.json`;
/// macOS derives the app's on-disk folder names from it.
pub(crate) const BUNDLE_ID: &str = "com.fletch.desktop";

/// Fletch's on-disk data directory — `~/Library/Application Support/
/// <BUNDLE_ID>` (with a `dev` subfolder under debug builds), matching
/// what `app.path().app_data_dir()` resolves to in `setup`. Computed without
/// an `AppHandle` so logging can be initialized before the Tauri app is built.
pub(crate) fn data_dir() -> PathBuf {
    let base = dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(BUNDLE_ID);
    if cfg!(debug_assertions) {
        base.join("dev")
    } else {
        base
    }
}

/// Fletch's log directory. On macOS this is `~/Library/Logs/<BUNDLE_ID>`
/// (with a `dev` subfolder under debug builds, mirroring `data_dir`) — the
/// platform convention, where Console.app indexes per-app logs. Elsewhere (the
/// Linux CI build) it stays nested under `data_dir()`. Computed without an
/// `AppHandle` so logging can be initialized before the Tauri app is built,
/// and reused by the `reveal_logs` command.
pub(crate) fn logs_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        if let Some(home) = dirs::home_dir() {
            let base = home.join("Library").join("Logs").join(BUNDLE_ID);
            return if cfg!(debug_assertions) {
                base.join("dev")
            } else {
                base
            };
        }
    }
    data_dir().join("logs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_is_under_the_bundle_id() {
        let dir = data_dir();
        assert!(dir.to_string_lossy().contains(BUNDLE_ID));
        // Tests build in debug, so the dev sandbox subfolder is used.
        assert_eq!(dir.file_name().unwrap(), "dev");
    }

    #[test]
    fn logs_dir_follows_the_platform_convention() {
        let dir = logs_dir();
        if cfg!(target_os = "macos") {
            // ~/Library/Logs/<BUNDLE_ID>, plus the dev subfolder in debug
            // builds (tests build in debug).
            assert!(dir
                .to_string_lossy()
                .contains(&format!("Library/Logs/{BUNDLE_ID}")));
            assert_eq!(dir.file_name().unwrap(), "dev");
            assert!(!dir.starts_with(data_dir()));
        } else {
            // Linux CI fallback: nested under the data dir, as before.
            assert!(dir.starts_with(data_dir()));
            assert_eq!(dir.file_name().unwrap(), "logs");
        }
    }

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
    app: tauri::AppHandle,
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
    supervisor.respawn_provider(&app, &id).await;
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

/// Emit the deferred first `app_opened`. The frontend calls this once, when the
/// user finishes onboarding (after the data-sharing disclosure on the final
/// step). On a fresh install `setup` skips the launch-time `app_opened`, so this
/// is the first such event — sent only once consent has been disclosed.
#[tauri::command]
fn track_app_opened() {
    telemetry::track("app_opened", json!({}));
}

/// The persisted sandbox engine selection (`"sandbox-exec"` | `"docker"`).
/// Reads the in-memory mirror seeded at startup and kept in sync by
/// `set_sandbox_engine`, so no DB handle is needed.
#[tauri::command]
fn get_sandbox_engine() -> String {
    sandbox::selected_engine_kind().as_setting().to_string()
}

/// Change the sandbox engine stamped onto *new* agents. Docker is validated
/// against a live daemon probe before being accepted, so a success here means
/// the choice is actionable. Persists to `settings` and updates the in-memory
/// mirror — like `set_agent_bin_override` keeps the DB and process state in
/// sync. Existing agents are unaffected: each keeps the engine stamped on its
/// record at creation.
#[tauri::command]
async fn set_sandbox_engine(
    engine: String,
    state: tauri::State<'_, DbState>,
) -> Result<(), String> {
    let kind = sandbox::EngineKind::from_setting(&engine)
        .ok_or_else(|| format!("unknown sandbox engine: {engine}"))?;
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

/// Startup seed retry pacing for an unavailable secret store: start at 30s
/// (the realistic case — a login-item launch racing the keychain unlock —
/// resolves quickly) and back off to a slow probe that never gives up. An
/// unavailable store must stay distinct from "no token" for the app's whole
/// lifetime, not just a startup window: a saved login must never read as
/// signed-out only because the keychain stayed locked past some deadline.
const SECRET_SEED_RETRY_START: std::time::Duration = std::time::Duration::from_secs(30);
const SECRET_SEED_RETRY_CAP: std::time::Duration = std::time::Duration::from_secs(300);

/// Seed an in-process secret mirror from the store at startup. A definitive
/// answer (present or absent) applies immediately; an *unavailable* store
/// (`Err` — e.g. the keychain is locked) retries in the background until it
/// gets one. `apply` is a plain fn: both mirrors (`github::set_token`,
/// `docker::auth::set_stored_token`) have that shape.
fn seed_secret_mirror(db: &DbState, key: &'static str, apply: fn(Option<String>)) {
    match secrets::get(&db.lock(), key) {
        Ok(value) => apply(value),
        Err(e) => {
            tracing::warn!(key, error = %e, "secret store unavailable at startup; retrying");
            let db = db.clone();
            tauri::async_runtime::spawn(async move {
                let mut delay = SECRET_SEED_RETRY_START;
                loop {
                    tokio::time::sleep(delay).await;
                    match secrets::get(&db.lock(), key) {
                        // Only a real value is applied: the mirror already
                        // defaults to empty, and a sign-in/paste may have set
                        // it directly while we waited — a late None could only
                        // clobber that fresher token, never fix anything.
                        Ok(Some(value)) => return apply(Some(value)),
                        Ok(None) => return,
                        Err(_) => delay = (delay * 2).min(SECRET_SEED_RETRY_CAP),
                    }
                }
            });
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
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

            let db = match database::init(&data_dir) {
                Ok(db) => db,
                Err(e) => recover_from_db_init_failure(&data_dir, e),
            };

            // Seed the in-memory agent binary override registry so binary
            // resolution (deep in spawn/probe paths, with no DB handle) can
            // honor user-set custom paths without touching the DB each time.
            bin_resolve::set_agent_overrides(database::load_agent_bin_overrides(&db.lock()));

            // Seed the in-memory sandbox engine selection (mirror of the
            // `sandbox_engine` setting) so spawn-time engine resolution —
            // deep in agent code with no DB handle — honors the user's
            // choice. Missing/unknown values keep the sandbox-exec default.
            if let Some(kind) = database::get_setting(&db.lock(), sandbox::ENGINE_SETTING)
                .as_deref()
                .and_then(sandbox::EngineKind::from_setting)
            {
                sandbox::set_selected_engine_kind(kind);
            }

            // Seed the docker launch knobs (image override + resource limits)
            // the same way — mirrored in-process for the spawn path. Slice C2
            // adds the settings UI whose set-commands keep this in sync
            // mid-run; until then changes apply on next launch.
            {
                let conn = db.lock();
                sandbox::docker::set_launch_settings(sandbox::docker::LaunchSettings {
                    image_override: database::get_setting(&conn, sandbox::docker::IMAGE_SETTING),
                    memory: database::get_setting(&conn, sandbox::docker::MEMORY_SETTING),
                    cpus: database::get_setting(&conn, sandbox::docker::CPUS_SETTING),
                });
            }

            // Seed the docker version-refresh loop guard (mirror of the
            // `docker_version_refresh_guard` setting — private bookkeeping,
            // not user-facing) and wire its write-back, so a host CLI pinned
            // away from the registry's latest triggers at most one
            // version-parity rebuild ever, not one per app run. Same mirror
            // idiom as the launch knobs above; the guard is consulted and
            // recorded on spawn/background threads that have no DB handle.
            {
                let seeded: std::collections::HashMap<String, String> =
                    database::get_setting(&db.lock(), sandbox::docker::VERSION_GUARD_SETTING)
                        .and_then(|s| serde_json::from_str(&s).ok())
                        .unwrap_or_default();
                let guard_db = db.clone();
                sandbox::docker::init_version_refresh_guard(seeded, move |attempted| {
                    if let Ok(json) = serde_json::to_string(attempted) {
                        let _ = database::set_setting(
                            &guard_db.lock(),
                            sandbox::docker::VERSION_GUARD_SETTING,
                            &json,
                        );
                    }
                });
            }

            // Seed the in-process container auth token (mirror of the stored
            // `claude_container_token` secret, same pattern as the GitHub
            // token below) so the docker auth chain — resolved at spawn time
            // with no DB handle — sees a token pasted in a previous run.
            seed_secret_mirror(
                &db,
                sandbox::docker::auth::TOKEN_SETTING,
                sandbox::docker::auth::set_stored_token,
            );

            // Unified git resolution: point the portable-install root at app
            // data, wire the fallback commit identity to the signed-in
            // profile, and kick off resolve-or-download in the background —
            // at launch, not at the onboarding readiness screen, so a
            // git-less machine is usually ready before the user gets there.
            git_dist::init(data_dir.join("git-dist"));
            // Seed the in-process GitHub token so API calls and git network
            // auth work without a DB handle (updated on sign-in).
            seed_secret_mirror(&db, github::TOKEN_SETTING, github::set_token);
            {
                let db = db.clone();
                git_dist::set_identity_source(Box::new(move || {
                    database::get_account_identity(&db.lock())
                }));
            }
            {
                use tauri::Emitter;
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(git_dist::startup(move |payload| {
                    let _ = handle.emit("git-dist:state", payload);
                }));
            }

            // Forward docker image-build progress to the UI. The
            // build runs deep in the spawn path (no AppHandle there), so it
            // emits through a process-wide sink installed here — mirroring the
            // git-dist emitter above. Rare (first docker spawn per image), so a
            // single toast fed by these events suffices.
            {
                use tauri::Emitter;
                let handle = app.handle().clone();
                sandbox::docker::set_build_sink(move |event| {
                    let _ = handle.emit("docker:build-progress", event);
                });
            }

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
            // On a fresh install the first `app_opened` is deferred until
            // onboarding completes (see the `track_app_opened` command), so no
            // event is sent before the user has seen the data-sharing disclosure.
            // Once onboarded, every launch reports `app_opened` here as usual.
            if onboarding_complete {
                telemetry::track("app_opened", json!({}));
            }
            if let Some(prev) = prev_version {
                if !prev.is_empty() && prev != version {
                    telemetry::track(
                        "app_updated",
                        json!({ "from_version": prev, "to_version": version }),
                    );
                }
            }

            app.manage(db.clone());

            // One-time move of the legacy on-disk checkouts root
            // (`~/.fletch/worktrees` → `~/.fletch/workspaces`) for installs that
            // predate the rename. Best-effort; runs before the supervisor
            // provisions any checkout so restores resolve to the new location.
            crate::workspace::migrate_default_checkouts_root();

            let workspace = Arc::new(WorkspaceManager::new(db));
            let supervisor = Arc::new(Supervisor::new(workspace));
            app.manage(supervisor.clone());
            // Reload follow-ups that were queued behind an in-flight turn when a
            // prior run exited, so a mid-turn message survives a restart. They
            // rest in the queue and flush on the user's next send (no auto-spawn).
            supervisor.rehydrate_pending_messages();
            // At most one `claude setup-token` capture runs at a time; the
            // code-submit / cancel commands reach it through this slot.
            app.manage(ClaudeSetupState::default());

            // Reclaim nested-Fletch RPC mailbox and checkout roots left in the
            // temp dir by dead instances (dogfooding runs). Live instances'
            // roots are pid-keyed and skipped, so a side-by-side Fletch is left
            // untouched.
            crate::sandbox::cleanup_nested_rpc_roots();
            crate::sandbox::cleanup_nested_checkouts_roots();
            // Same reclamation for docker containers left by dead instances —
            // probe-gated and on its own thread, so startup never waits on it.
            crate::sandbox::docker::sweep_orphans_at_startup();

            // Quitting normally goes through `RunEvent::ExitRequested` (below),
            // but a SIGINT (Ctrl-C under `tauri dev`) or SIGTERM (sent by the
            // OS on logout/restart/shutdown) bypasses it. Catch both via an
            // async listener — safe to do real work here, unlike a raw signal
            // handler — kill the children, then exit cleanly. SIGKILL/crash
            // can't be caught; for those the kernel closes our PTY masters on
            // death, which SIGHUPs each agent's process group as a backstop.
            #[cfg(unix)]
            {
                let supervisor = supervisor.clone();
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    use tokio::signal::unix::{signal, SignalKind};
                    let mut sigint =
                        signal(SignalKind::interrupt()).expect("install SIGINT handler");
                    let mut sigterm =
                        signal(SignalKind::terminate()).expect("install SIGTERM handler");
                    tokio::select! {
                        _ = sigint.recv() => {}
                        _ = sigterm.recv() => {}
                    }
                    tracing::info!("termination signal received; killing child processes");
                    supervisor.shutdown();
                    handle.exit(0);
                });
            }

            // Agents rest at Idle on boot — no process is spawned. The
            // supervisor brings one up lazily on the user's next interaction
            // (the frontend resumes on send), so nothing auto-spawns here.
            Ok(())
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
            track_app_opened,
            get_sandbox_engine,
            set_sandbox_engine,
            probe_docker_engine,
            get_container_auth_status,
            set_container_auth_token,
            clear_container_auth_token,
            connect_claude_container_auth,
            submit_claude_setup_code,
            cancel_claude_container_auth,
            set_docker_launch_settings,
            workflows::workflow_list,
            workflows::workflow_save,
            workflows::workflow_delete,
            workflows::workflow_save_run,
            workflows::workflow_get_run,
            workflows::workflow_list_runs,
            workflows::workflow_save_run_step,
            workflows::workflow_delete_run,
            workflows::workflow_prepare_repo,
            workflows::workflow_ferry_notes,
            workflows::workflow_boundary_commit,
            workflows::workflow_head_sha,
            workflows::workflow_file_exists,
            workflows::workflow_finalize,
            workflow::definition::wf_def_save,
            workflow::definition::wf_def_list,
            workflow::definition::wf_def_delete,
            workflow::definition::wf_def_export_yaml,
            workflow::definition::wf_def_import_yaml,
            oauth::oauth_device_login,
            commands::get_workspace,
            commands::get_agent_diff_stats,
            commands::add_workspace_repo,
            commands::remove_workspace_repo,
            commands::rename_project,
            commands::relocate_repo,
            commands::gh_status,
            commands::gh_repo_list,
            commands::clone_repo,
            commands::create_repo,
            commands::publish_agent,
            commands::github_disconnect,
            commands::spawn_agent,
            commands::write_to_agent,
            commands::send_user_message,
            commands::answer_tool_use,
            commands::resize_agent,
            commands::switch_view,
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
            commands::push_agent,
            commands::pull_agent,
            commands::rebase_agent,
            commands::commit_agent,
            commands::discard_agent_changes,
            commands::stash_agent,
            commands::abort_merge_agent,
            commands::delete_branch_agent,
            commands::list_repo_branches,
            commands::create_pr,
            commands::merge_pr,
            commands::get_pr_state,
            commands::refresh_all_pr_states,
            commands::refresh_all_pr_checks,
            commands::get_pr_checks,
            commands::get_pr_comments,
            commands::open_agent_shell,
            commands::close_agent_shell,
            commands::write_to_shell,
            commands::resize_shell,
            commands::run_start,
            commands::run_stop,
            commands::run_state,
            commands::detect_run_config,
            commands::project_run_config,
            commands::list_checkout_tree,
            commands::list_dir,
            commands::list_prs,
            commands::read_checkout_file,
            commands::get_file_diff,
            commands::write_checkout_file,
            commands::rename_checkout_path,
            commands::delete_checkout_path,
            commands::create_checkout_file,
            commands::create_checkout_dir,
            commands::copy_checkout_file,
            commands::probe_provider_versions,
            commands::check_cli,
            commands::git_dist_install,
            commands::install_agent,
            commands::validate_agent_bin,
            commands::discover_supported_models,
            commands::reveal_logs,
            commands::start_docker_desktop,
            commands::detect_editors,
            commands::open_in_editor,
        ])
        .build(tauri::generate_context!())
        .expect("error while building fletch")
        .run(|app, event| {
            // On quit, explicitly kill every live agent/shell/run child.
            // tauri-managed state isn't reliably dropped on macOS app
            // termination, so the per-session Drop impls can't be trusted to
            // fire — without this, quitting mid-run orphans the processes.
            if let tauri::RunEvent::ExitRequested { .. } = event {
                if let Some(supervisor) = app.try_state::<Arc<Supervisor>>() {
                    supervisor.shutdown();
                }
                // Give in-flight telemetry sends a brief, bounded chance to
                // finish before the runtime tears down, rather than dropping
                // events that fired just before quit (e.g. `pr_opened`).
                tauri::async_runtime::block_on(telemetry::flush(std::time::Duration::from_secs(3)));
            }
        });
}
