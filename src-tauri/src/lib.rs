mod activity;
mod agent;
mod branding;
mod commands;
mod database;
mod error;
mod gh;
mod git;
mod git_state;
mod managed_session;
mod names;
mod pty_session;
mod run_session;
mod sandbox;
mod supervisor;
mod workspace;

use parking_lot::Mutex;
use rusqlite::Connection;
use serde_json::Value;
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
            std::fs::create_dir_all(&app_data)?;

            let db = database::init(&app_data)
                .expect("failed to initialize database");
            app.manage(db.clone());

            let workspace = Arc::new(WorkspaceManager::new(db));
            let supervisor = Arc::new(Supervisor::new(workspace));
            app.manage(supervisor.clone());

            // Auto-resume any agent that was live before the previous
            // shutdown. Runs on a tauri async task so we don't block
            // app boot; events emit as they would for a manual spawn.
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                // Tiny delay so the frontend has time to mount its
                // event listeners and the workspace view before agents
                // start emitting.
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                supervisor.resume_persisted_agents(app_handle);
            });
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
            commands::get_workspace,
            commands::get_agent_diff_stats,
            commands::add_workspace_repo,
            commands::remove_workspace_repo,
            commands::spawn_agent,
            commands::write_to_agent,
            commands::send_user_message,
            commands::resize_agent,
            commands::switch_view,
            commands::resume_agent,
            commands::stop_agent,
            commands::discard_agent,
            commands::archive_agent,
            commands::restore_agent,
            commands::read_session_transcript,
            commands::add_repo_to_agent,
            commands::allocate_draft_name,
            commands::get_git_state,
            commands::get_all_shortstats,
            commands::push_agent,
            commands::pull_agent,
            commands::commit_agent,
            commands::discard_agent_changes,
            commands::stash_agent,
            commands::abort_merge_agent,
            commands::delete_branch_agent,
            commands::create_pr,
            commands::merge_pr,
            commands::get_pr_state,
            commands::open_agent_shell,
            commands::close_agent_shell,
            commands::write_to_shell,
            commands::resize_shell,
            commands::run_start,
            commands::run_stop,
            commands::run_state,
            commands::list_worktree_tree,
            commands::read_worktree_file,
            commands::write_worktree_file,
            commands::rename_worktree_path,
            commands::delete_worktree_path,
            commands::create_worktree_file,
            commands::create_worktree_dir,
            commands::copy_worktree_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running quorum");
}
