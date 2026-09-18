//! Project / repo management shared with the remote dispatcher: draft-name
//! allocation, pinning a folder, and the `workspace:changed` announcement
//! every pinning path ends with.

use std::path::PathBuf;

use crate::error::Result;
use crate::names;
use crate::new_project;
use crate::supervisor::Supervisor;
use crate::workspace::Workspace;

/// Allocate a fresh name from the place pool for a draft agent.
///
/// Not a throwaway preview: a draft carries its name into `spawn_agent`, which
/// pins it via `add_agent` (see `lifecycle.rs`), so whatever this returns is
/// the agent's real name — `add_agent_allocating` only runs for the draft-less
/// path. The reserved set is therefore load-bearing, and the DB owns it:
/// `live_agent_ids` supplies the live agents, and the caller passes *only*
/// `drafts` — the open, unpersisted drafts, the one piece of state the frontend
/// has that the DB can't see.
///
/// Deliberately not "caller passes everything that's taken": that let a
/// frontend set that also counted archived agents saturate the ~300-name pool
/// and push roughly a quarter of new workspaces onto a `-2` suffix.
///
/// Shared with the remote dispatcher, so a phone's draft allocation reserves
/// against the same live set.
pub fn allocate_draft_name_impl(supervisor: &Supervisor, drafts: Vec<String>) -> Result<String> {
    let mut reserved = supervisor.workspace.live_agent_ids()?;
    reserved.extend(drafts);
    Ok(names::allocate(&reserved))
}

/// Pin a folder as a workspace project. A folder that isn't a git repository
/// yet is initialized (with an initial commit) first, so users who've never
/// heard of git can still point the app at any project folder and get working
/// agents, checkouts, and history.
///
/// Shared with the remote dispatcher, so a folder pinned from a phone gets the
/// same git initialization a folder picked on the desktop does.
pub async fn add_workspace_repo_impl(
    supervisor: &Supervisor,
    repo_path: String,
) -> Result<Workspace> {
    let path = PathBuf::from(repo_path);
    new_project::ensure_git_repo(&path).await?;
    supervisor.add_workspace_repo(path)
}

/// Emit `workspace:changed` when a command has changed the project list, so
/// every other view — the desktop window, each paired phone — reloads it rather
/// than waiting for its next refresh. The caller still applies the returned
/// `Workspace` itself; the event is for everyone else. Kept out of the `_impl`
/// functions so the remote tests can drive those without a sink.
pub fn announce_workspace<T, E>(
    sink: &dyn crate::host::EventSink,
    result: std::result::Result<T, E>,
) -> std::result::Result<T, E> {
    if result.is_ok() {
        crate::supervisor::emit_workspace_changed(sink);
    }
    result
}
