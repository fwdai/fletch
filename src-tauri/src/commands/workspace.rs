//! Project / repo management: pinning, attaching, relabeling, renaming, and
//! deleting workspace projects and their tracked repos.

use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;

use crate::error::Result;
use crate::names;
use crate::new_project;
use crate::supervisor::Supervisor;
use crate::workspace::{ProjectDeleteResult, Workspace};

/// Allocate a fresh name from the place pool for a draft agent. The frontend
/// passes the names already taken (live agents + other open drafts); the
/// per-build DB is authoritative for the rest, so there's nothing else to fold
/// in. This is only a preview — `allocate_agent_id` is authoritative at create.
#[tauri::command]
pub fn allocate_draft_name(used: Vec<String>) -> String {
    let reserved: std::collections::HashSet<String> = used.into_iter().collect();
    names::allocate(&reserved)
}

/// Pin a folder as a workspace project. A folder that isn't a git repository
/// yet is initialized (with an initial commit) first, so users who've never
/// heard of git can still point the app at any project folder and get working
/// agents, checkouts, and history.
#[tauri::command]
pub async fn add_workspace_repo(
    supervisor: State<'_, Arc<Supervisor>>,
    repo_path: String,
) -> Result<Workspace> {
    let sup = supervisor.inner().clone();
    let path = PathBuf::from(repo_path);
    new_project::ensure_git_repo(&path).await?;
    sup.add_workspace_repo(path)
}

#[tauri::command]
pub fn remove_workspace_repo(
    supervisor: State<'_, Arc<Supervisor>>,
    repo_path: String,
) -> Result<Workspace> {
    supervisor.remove_workspace_repo(PathBuf::from(repo_path))
}

/// Attach a repo to an existing project (multi-repo projects). Two phases,
/// deliberately ordered so a doomed attach can never mutate the picked
/// folder: the DB attach validates the project and commits first, and only
/// then is a non-git folder initialized (mirroring `add_workspace_repo`).
/// If that filesystem step fails, the DB attach is rolled back precisely.
#[tauri::command]
pub async fn attach_repo_to_project(
    supervisor: State<'_, Arc<Supervisor>>,
    project_id: String,
    repo_path: String,
) -> Result<Workspace> {
    let sup = supervisor.inner().clone();
    let path = PathBuf::from(repo_path);
    let outcome = sup.attach_repo_to_project(&project_id, &path)?;
    if let Err(e) = new_project::ensure_git_repo(&path).await {
        // Roll back the row(s) the DB phase wrote; best-effort — it re-applies
        // keys we wrote moments ago on a local connection, so a failure here
        // means the DB itself is gone, and the init error is the one to show.
        let _ = sup.undo_attach(&outcome);
        return Err(e);
    }
    Ok(sup.workspace.current().expect("workspace initialized"))
}

/// Detach a repo from a project. Rejects the project's last repo and any repo
/// still referenced by an agent checkout (live or archived).
#[tauri::command]
pub fn detach_repo_from_project(
    supervisor: State<'_, Arc<Supervisor>>,
    project_id: String,
    repo_path: String,
) -> Result<Workspace> {
    supervisor.detach_repo_from_project(&project_id, PathBuf::from(repo_path))
}

/// Set a repo's display label within its project ("Frontend", "Gateway").
/// Blank clears back to the folder-basename fallback.
#[tauri::command]
pub fn set_repo_label(
    supervisor: State<'_, Arc<Supervisor>>,
    repo_path: String,
    label: String,
) -> Result<Workspace> {
    supervisor.set_repo_label(PathBuf::from(repo_path), &label)
}

/// Rename a project — a custom display label independent of its folder name.
/// The sidebar and Project Settings header show this instead of the basename.
#[tauri::command]
pub fn rename_project(
    supervisor: State<'_, Arc<Supervisor>>,
    project_id: String,
    name: String,
) -> Result<Workspace> {
    supervisor.rename_project(&project_id, &name)
}

/// Delete a project and all of its workspaces, including non-archived ones.
/// The supervisor refuses while any project agent is actively running.
#[tauri::command]
pub async fn delete_project(
    supervisor: State<'_, Arc<Supervisor>>,
    workflows: State<'_, Arc<crate::workflow::scheduler::WorkflowService>>,
    project_id: String,
) -> Result<ProjectDeleteResult> {
    supervisor
        .inner()
        .clone()
        .delete_project(workflows.inner(), &project_id)
        .await
}

#[tauri::command]
pub fn project_has_running_agents(
    supervisor: State<'_, Arc<Supervisor>>,
    project_id: String,
) -> bool {
    supervisor.project_has_running_agents(&project_id)
}

/// Repoint a pinned repo at a folder the user has moved on disk. Validates the
/// destination is a git repo; existing agents' worktrees are not relinked.
#[tauri::command]
pub fn relocate_repo(
    supervisor: State<'_, Arc<Supervisor>>,
    old_path: String,
    new_path: String,
) -> Result<Workspace> {
    supervisor.relocate_repo(PathBuf::from(old_path), PathBuf::from(new_path))
}
