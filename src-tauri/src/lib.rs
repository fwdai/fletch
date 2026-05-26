mod activity;
mod agent;
mod branding;
mod commands;
mod error;
mod git;
mod managed_session;
mod names;
mod pty_session;
mod sandbox;
mod supervisor;
mod workspace;

use std::sync::Arc;
use tauri::Manager;

use crate::supervisor::Supervisor;
use crate::workspace::WorkspaceManager;

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
        .setup(|app| {
            let app_data = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_data)?;

            let workspace = Arc::new(WorkspaceManager::new(app_data)?);
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
            commands::get_workspace,
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
            commands::add_repo_to_agent,
        ])
        .run(tauri::generate_context!())
        .expect("error while running quorum");
}
