//! The Fletch engine.
//!
//! Everything that runs agents: the database, workspaces, git, GitHub, the
//! sandboxes, the agent protocol, the supervisor, workflows, the roadmap, the
//! codegraph and the remote host. No `tauri` anywhere in this crate or in its
//! dependency graph — the desktop shell (`src-tauri`) and the headless host
//! both depend on it, and a Linux host must not compile a webview.
//!
//! The host boundary is `host::boot` (start the engine), `host::EngineCtx` (the
//! engine's view of its host), `host::Sink` (where events go) and
//! `host::spawn` (the runtime the engine's tasks run on).

pub mod activity;
pub mod agent;
pub mod agent_install;
pub mod agent_profile;
pub mod attachments;
pub mod bin_resolve;
pub mod child_io;
pub mod codegraph;
/// The engine halves of the desktop's Tauri commands. Each `*_impl` is the
/// body a `#[tauri::command]` wrapper in `src-tauri/src/commands/` calls and
/// the remote dispatcher (`remote::dispatch`) calls directly, so a phone and
/// the desktop window take the same code path.
pub mod commands;
pub mod database;
pub mod download;
pub mod error;
pub mod exec_session;
pub mod git;
pub mod git_dist;
pub mod git_state;
pub mod github;
pub mod host;
pub mod instructions;
pub mod issues;
pub mod keychain;
pub mod linear;
pub mod managed_session;
pub mod message_queue;
pub mod model_catalog;
pub mod names;
pub mod native_input;
pub mod new_project;
pub mod oauth;
pub mod power;
pub mod pty_session;
pub mod publish_prefs;
pub mod remote;
pub mod roadmap;
pub mod rpc;
pub mod run_detect;
pub mod run_env;
pub mod run_session;
pub mod sandbox;
pub mod secrets;
pub mod slash_commands;
pub mod supervisor;
pub mod telemetry;
pub mod transcripts;
pub mod usage_scan;
pub mod verify;
pub mod workflow;
pub mod workspace;

use parking_lot::Mutex;
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::Arc;

/// The managed DB handle every command that reads or writes settings asks for.
pub type DbState = Arc<Mutex<Connection>>;

/// The app's bundle identifier. Must match `identifier` in `tauri.conf.json`;
/// macOS derives the app's on-disk folder names from it.
pub const BUNDLE_ID: &str = "com.fletch.desktop";

/// Fletch's on-disk data directory — `~/Library/Application Support/
/// <BUNDLE_ID>` (with a `dev` subfolder under debug builds), matching
/// what `app.path().app_data_dir()` resolves to in the desktop's `setup`.
/// Computed without an `AppHandle` so logging can be initialized before the
/// Tauri app is built, and so a headless host resolves the same directory.
pub fn data_dir() -> PathBuf {
    let base = dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(BUNDLE_ID);
    if cfg!(debug_assertions) {
        base.join("dev")
    } else {
        base
    }
}

/// The per-build path segment for a kind of on-disk state under a shared base
/// (`~/.fletch`, or an override root): `<leaf>` for release, `dev/<leaf>` for
/// debug — the same `dev` split [`data_dir`] applies to app data. Release keeps
/// the historical flat segment, so existing installs need no migration.
///
/// Every root a *live* agent's state hangs off must go through this: a debug
/// instance and a release install have separate DBs but one filesystem, so from
/// a shared root each build's startup housekeeping sees the other's live state
/// as garbage — and their name allocators, drawing from one pool, can hand both
/// builds the same agent id. The `dev` prefix makes the two roots siblings, not
/// nested, so neither build's sweep can even see the other's.
pub fn build_state_subpath(leaf: &str) -> PathBuf {
    if cfg!(debug_assertions) {
        PathBuf::from("dev").join(leaf)
    } else {
        PathBuf::from(leaf)
    }
}

/// Fletch's log directory. On macOS this is `~/Library/Logs/<BUNDLE_ID>`
/// (with a `dev` subfolder under debug builds, mirroring `data_dir`) — the
/// platform convention, where Console.app indexes per-app logs. Elsewhere (the
/// Linux CI build) it stays nested under `data_dir()`. Computed without an
/// `AppHandle` so logging can be initialized before the Tauri app is built,
/// and reused by the `reveal_logs` command.
pub fn logs_dir() -> PathBuf {
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
}
