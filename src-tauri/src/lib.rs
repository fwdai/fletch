mod activity;
mod agent;
mod bin_resolve;
mod branding;
mod commands;
mod database;
mod error;
mod exec_session;
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
mod workspace;

use parking_lot::Mutex;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::Manager;

use crate::supervisor::Supervisor;
use crate::workspace::WorkspaceManager;

type DbState = Arc<Mutex<Connection>>;

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
    state: tauri::State<'_, DbState>,
) -> Result<(), String> {
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
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,quorum_lib=debug")),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
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

            // Seed the in-memory agent binary override registry so binary
            // resolution (deep in spawn/probe paths, with no DB handle) can
            // honor user-set custom paths without touching the DB each time.
            bin_resolve::set_agent_overrides(database::load_agent_bin_overrides(&db.lock()));

            app.manage(db.clone());

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
            commands::validate_agent_bin,
            commands::discover_supported_models,
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
            }
        });
}
