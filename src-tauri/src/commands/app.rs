//! Misc app-level handlers: workspace snapshot, log/Docker/editor launchers,
//! and an agent's HEAD sha.

use std::sync::Arc;
use tauri::State;

use crate::error::{Error, Result};
use crate::git;
use crate::supervisor::Supervisor;
use crate::workspace::{AgentRecord, Workspace};

use super::files::{agent_repo_checkout, primary_repo_checkout};

#[tauri::command]
pub fn get_workspace(supervisor: State<'_, Arc<Supervisor>>) -> Option<Workspace> {
    supervisor.current_workspace()
}

/// A workflow run's step agents (live + archived). Run-owned agents are hidden
/// from `get_workspace`, so the run monitor fetches them here to render each
/// attempt's preserved chat.
#[tauri::command]
pub fn wf_run_agents(run_id: String, supervisor: State<'_, Arc<Supervisor>>) -> Vec<AgentRecord> {
    supervisor.run_agents(&run_id)
}

/// A project's purpose-tagged chats, newest first — today the Roadmap tab's
/// project-manager chats (`purpose = "roadmap-pm"`). Like run-owned agents these
/// are hidden from `get_workspace`, so the surface that owns them lists them
/// here; the records are ordinary `AgentRecord`s, addressable by id for
/// transcripts, sends and resumes.
#[tauri::command]
pub fn list_project_chats(
    project_id: String,
    purpose: String,
    supervisor: State<'_, Arc<Supervisor>>,
) -> Vec<AgentRecord> {
    supervisor.project_chats(&project_id, &purpose)
}

/// Reveal Fletch's log folder in the OS file manager so a user can attach
/// logs to a bug report. Creates the folder if no session has written to it
/// yet. Fletch ships macOS-only (sandbox-exec), but the CI build runs on
/// Linux, so the opener binary is chosen per-platform rather than hard-coding
/// `open`.
#[tauri::command]
pub fn reveal_logs() -> Result<()> {
    let dir = crate::logs_dir();
    std::fs::create_dir_all(&dir).map_err(|e| Error::Other(format!("create log dir: {e}")))?;
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(&dir)
        .spawn()
        .map_err(|e| Error::Other(format!("open log dir: {e}")))?;
    Ok(())
}

/// Launch Docker Desktop — the "Start Docker Desktop" action on a docker
/// agent's daemon-down error state. macOS-only, like the rest of the sandbox
/// feature (`open -a Docker`); other platforms error so the UI can
/// report it rather than silently no-op. The daemon then takes a few seconds to
/// answer, which the settings pane's probe-retry loop already covers.
#[tauri::command]
pub fn start_docker_desktop() -> Result<()> {
    if cfg!(target_os = "macos") {
        std::process::Command::new("open")
            .args(["-a", "Docker"])
            .spawn()
            .map_err(|e| Error::Other(format!("open Docker Desktop: {e}")))?;
        Ok(())
    } else {
        Err(Error::Other(
            "Starting Docker Desktop from Fletch is only supported on macOS.".into(),
        ))
    }
}

/// The code editors installed on this machine, for the title-bar
/// "Open in editor" launcher. Detected live (see `editors::detect`).
#[tauri::command]
pub fn detect_editors() -> Vec<crate::editors::DetectedEditor> {
    crate::editors::detect()
}

/// Open an agent's primary checkout in the chosen editor.
#[tauri::command]
pub fn open_in_editor(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    editor_id: String,
) -> Result<()> {
    let (_, checkout) = primary_repo_checkout(&supervisor, &agent_id)?;
    crate::editors::open(&editor_id, &checkout)
}

/// The current HEAD commit SHA of an agent's checkout (primary repo when
/// `subdir` is omitted). Powers "promote to workflow", where the run forks from
/// the promoted session's exact working commit rather than a branch tip.
#[tauri::command]
pub async fn agent_head_sha(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    subdir: Option<String>,
) -> Result<String> {
    let (_repo, checkout) = agent_repo_checkout(&supervisor, &agent_id, subdir.as_deref())?;
    git::rev_parse(&checkout, "HEAD").await
}
