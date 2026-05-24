mod agent;
mod commands;
mod error;
mod git;
mod keys;
mod pty_bridge;
mod supervisor;
mod vm;
mod workspace;

use std::path::PathBuf;
use std::sync::Arc;
use tauri::Manager;

use crate::supervisor::Supervisor;
use crate::vm::{RealTartCli, Vm};
use crate::workspace::WorkspaceManager;

/// Locate the bundled `tart` binary.
///
/// Tart ships as a macOS `.app` bundle whose signing + entitlements (notably
/// `com.apple.security.virtualization`) come from an embedded provisioning
/// profile. The bundle structure MUST be preserved — we always invoke the
/// inner executable at `tart.app/Contents/MacOS/tart`, never extract it.
///
/// Layout:
///   dev:  <manifest>/resources/tart/tart.app/Contents/MacOS/tart
///   prod: <resource_dir>/resources/tart/tart.app/Contents/MacOS/tart
fn resolve_tart_binary(app: &tauri::AppHandle) -> std::io::Result<PathBuf> {
    let inner = std::path::Path::new("resources")
        .join("tart")
        .join("tart.app")
        .join("Contents")
        .join("MacOS")
        .join("tart");

    let mut candidates: Vec<PathBuf> = Vec::new();
    if cfg!(debug_assertions) {
        candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(&inner));
    }
    if let Ok(res_dir) = app.path().resource_dir() {
        candidates.push(res_dir.join(&inner));
    }

    candidates
        .into_iter()
        .find(|p| p.exists())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "bundled tart binary not found — did `npm install` run \
                 scripts/download-tart.sh?",
            )
        })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,algiers_lib=debug")),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let app_data = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_data)?;

            let tart_path = resolve_tart_binary(app.handle())?;
            tracing::info!(path = %tart_path.display(), "resolved tart binary");

            let vm = Arc::new(Vm::new(Box::new(RealTartCli::new(tart_path))));
            let workspace = Arc::new(WorkspaceManager::new(app_data.clone())?);

            let app_data_for_keys = app_data.clone();
            let keys = tauri::async_runtime::block_on(async move {
                crate::keys::ensure_key_pair(&app_data_for_keys).await
            })?;

            let supervisor = Arc::new(Supervisor::new(workspace, vm, keys));
            app.manage(supervisor);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_workspace,
            commands::set_repo,
            commands::spawn_agent,
            commands::write_to_agent,
            commands::resize_agent,
            commands::stop_agent,
            commands::discard_worktree,
            commands::get_public_key,
            commands::list_base_images,
        ])
        .run(tauri::generate_context!())
        .expect("error while running algiers");
}
