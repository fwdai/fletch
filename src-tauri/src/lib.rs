mod activity;
mod agent;
mod bin_resolve;
mod commands;
mod database;
mod error;
mod exec_session;
mod extensions;
mod gh;
mod git;
mod git_state;
mod instructions;
mod managed_session;
mod model_catalog;
mod names;
mod new_project;
mod oauth;
mod pty_session;
mod rpc;
mod run_detect;
mod run_session;
mod sandbox;
mod supervisor;
mod telemetry;
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

/// Quorum's on-disk data directory — `~/Library/Application Support/
/// com.quorum.desktop` (with a `dev` subfolder under debug builds), matching
/// what `app.path().app_data_dir()` resolves to in `setup`. Computed without
/// an `AppHandle` so logging can be initialized before the Tauri app is built,
/// and reused by the `reveal_logs` command.
pub(crate) fn data_dir() -> PathBuf {
    let base = dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("com.quorum.desktop");
    if cfg!(debug_assertions) {
        base.join("dev")
    } else {
        base
    }
}

pub(crate) fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_is_under_the_bundle_id_and_logs_nest_within() {
        let dir = data_dir();
        assert!(dir.to_string_lossy().contains("com.quorum.desktop"));
        // Tests build in debug, so the dev sandbox subfolder is used.
        assert_eq!(dir.file_name().unwrap(), "dev");
        assert!(logs_dir().starts_with(&dir));
        assert_eq!(logs_dir().file_name().unwrap(), "logs");
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
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,quorum_lib=debug"));

    let dir = logs_dir();
    let appender = std::fs::create_dir_all(&dir)
        .map_err(|e| format!("create log dir: {e}"))
        .and_then(|()| {
            tracing_appender::rolling::Builder::new()
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .filename_prefix("quorum")
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
        database::set_setting(&conn, "telemetry_enabled", if enabled { "true" } else { "false" })
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Error/crash reporting. The DSN is baked in at build time via
    // `QUORUM_SENTRY_DSN` (empty/unset → a disabled, no-op client, so dev and
    // unconfigured builds send nothing). This captures app health — Rust
    // panics and unhandled frontend errors — not user telemetry, so it is on
    // regardless of any future data-sharing toggle. `_sentry` must stay bound
    // for the whole process so the client flushes on exit; `run()` blocks
    // below, so this scope lives until quit.
    let _sentry = sentry::init((
        option_env!("QUORUM_SENTRY_DSN").filter(|s| !s.is_empty()),
        sentry::ClientOptions {
            release: sentry::release_name!(),
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
        .plugin(tauri_plugin_process::init());

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

            let db = database::init(&data_dir)
                .expect("failed to initialize database");

            // Apply migrations declared by any extensions present in this
            // build, tracked separately from the core schema (see
            // extensions::apply_extension_migrations). No-op when none exist.
            extensions::apply_extension_migrations(&db.lock())
                .expect("failed to apply extension migrations");

            // Seed the in-memory agent binary override registry so binary
            // resolution (deep in spawn/probe paths, with no DB handle) can
            // honor user-set custom paths without touching the DB each time.
            bin_resolve::set_agent_overrides(database::load_agent_bin_overrides(&db.lock()));

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

            // Build the extension command registry once from the generated
            // glue and share it as state for `ext_invoke` to dispatch against.
            app.manage(Arc::new(extensions::build_api()));

            let workspace = Arc::new(WorkspaceManager::new(db));
            let supervisor = Arc::new(Supervisor::new(workspace));
            app.manage(supervisor.clone());

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
                    let mut sigint = signal(SignalKind::interrupt())
                        .expect("install SIGINT handler");
                    let mut sigterm = signal(SignalKind::terminate())
                        .expect("install SIGTERM handler");
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
            extensions::ext_invoke,
            oauth::oauth_device_login,
            commands::get_workspace,
            commands::get_agent_diff_stats,
            commands::add_workspace_repo,
            commands::remove_workspace_repo,
            commands::gh_status,
            commands::gh_repo_list,
            commands::clone_repo,
            commands::create_repo,
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
            commands::list_worktree_tree,
            commands::list_dir,
            commands::list_prs,
            commands::read_worktree_file,
            commands::get_file_diff,
            commands::write_worktree_file,
            commands::rename_worktree_path,
            commands::delete_worktree_path,
            commands::create_worktree_file,
            commands::create_worktree_dir,
            commands::copy_worktree_file,
            commands::probe_provider_versions,
            commands::check_cli,
            commands::validate_agent_bin,
            commands::discover_supported_models,
            commands::reveal_logs,
        ])
        .build(tauri::generate_context!())
        .expect("error while building quorum")
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
                tauri::async_runtime::block_on(telemetry::flush(
                    std::time::Duration::from_secs(3),
                ));
            }
        });
}
