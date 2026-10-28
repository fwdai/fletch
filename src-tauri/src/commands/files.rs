//! File panel — browse the checkout, view & edit file contents.
//!
//! The Tauri half. The path-safety guard (`safe_join`), the agent → repo →
//! checkout resolution and every read and write surface live in
//! `fletch_core::commands::files`, routed through the path-routed executor so a
//! box agent's checkout (which lives in its guest) is reached the same way as
//! a host one; the handlers below are the webview's way in. The resolution
//! helpers are re-exported so the sibling command modules keep reaching them as
//! `super::files::…`.

use std::sync::Arc;
use tauri::State;

use crate::error::{Error, Result};
use crate::git;
use crate::supervisor::Supervisor;
use fletch_core::commands::files::{
    copy_checkout_file_impl, create_checkout_dir_impl, create_checkout_file_impl,
    delete_checkout_path_impl, get_file_diff_impl, list_checkout_tree_impl, list_dir_impl,
    read_checkout_file_impl, rename_checkout_path_impl, write_checkout_file_impl,
};

pub use fletch_core::commands::files::{
    agent_repo_checkout, expand_tilde, primary_repo, primary_repo_checkout, repo_branch,
    CheckoutFile, CheckoutFileContents, DiffBaseMode, DirListing,
};

/// List the agent's checkout files (tracked + untracked), each tagged with
/// its git status vs the parent branch. This mirrors what's actually on disk
/// — like a regular file explorer — so files the agent deleted are dropped
/// rather than lingering as struck-through entries.
///
/// Single-repo agents get today's un-prefixed listing of the primary checkout.
/// A multi-repo agent gets one virtual root per repo: every path is prefixed
/// with the checkout's `subdir`, each repo's status computed against its own
/// fork point — the tree component nests on `/`, so the repos render as
/// top-level folders. The file read/write commands resolve the same prefix
/// back to the owning checkout (`checkout_scope_for_path`).
#[tauri::command]
pub async fn list_checkout_tree(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
) -> Result<Vec<CheckoutFile>> {
    list_checkout_tree_impl(&supervisor, &agent_id).await
}

/// List a repo's files by path (tracked + non-ignored untracked), for the
/// draft (new-workspace) composer's "@" mention autocomplete. Unlike
/// `list_checkout_tree`, this needs no agent — a draft has no checkout yet — so
/// it reads the base repo directly and returns plain paths (no diff status,
/// since there's no fork point to diff against).
#[tauri::command]
pub async fn list_repo_tree(repo_path: String) -> Result<Vec<String>> {
    git::list_files(&expand_tilde(&repo_path)).await
}

/// List the entries of an arbitrary directory for the composer's `@`
/// mention autocomplete (e.g. `@~/Downloads/`). The path may start with
/// `~`; the resolved absolute directory comes back as `base` so the caller
/// can attach files by absolute path.
#[tauri::command]
pub async fn list_dir(path: String) -> Result<DirListing> {
    list_dir_impl(path).await
}

/// Read a checkout file for the viewer/editor: contents, language hint,
/// git status, and the changed-line numbers driving the gutter. `base_mode`
/// picks the ref the gutter (and a deleted file's prior contents) diff
/// against; omitted means the fork point.
#[tauri::command]
pub async fn read_checkout_file(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    path: String,
    base_mode: Option<DiffBaseMode>,
) -> Result<CheckoutFileContents> {
    read_checkout_file_impl(&supervisor, &agent_id, &path, base_mode).await
}

/// Full unified diff of one checkout file — the data behind the Code panel's
/// Live view and the editor's Diff toggle. `base_mode` picks the base ref
/// (fork point when omitted). Returns "" when the file is unchanged.
#[tauri::command]
pub async fn get_file_diff(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    path: String,
    base_mode: Option<DiffBaseMode>,
) -> Result<String> {
    get_file_diff_impl(&supervisor, &agent_id, &path, base_mode).await
}

/// Overwrite a checkout file with new contents (the editor's Save / Revert).
#[tauri::command]
pub async fn write_checkout_file(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    path: String,
    contents: String,
) -> Result<()> {
    write_checkout_file_impl(&supervisor, &agent_id, &path, &contents).await
}

/// Rename/move a checkout path (file or directory). Refuses to clobber an
/// existing destination.
#[tauri::command]
pub async fn rename_checkout_path(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    from: String,
    to: String,
) -> Result<()> {
    rename_checkout_path_impl(&supervisor, &agent_id, &from, &to).await
}

/// Delete a checkout path: a file directly, a directory recursively (the UI
/// guards this behind a confirm step). Already gone is a no-op.
#[tauri::command]
pub async fn delete_checkout_path(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    path: String,
) -> Result<()> {
    delete_checkout_path_impl(&supervisor, &agent_id, &path).await
}

/// Create a new empty file, making parent directories as needed. Refuses to
/// overwrite an existing path.
#[tauri::command]
pub async fn create_checkout_file(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    path: String,
) -> Result<()> {
    create_checkout_file_impl(&supervisor, &agent_id, &path).await
}

/// Create a new directory. Refuses to clobber an existing path.
#[tauri::command]
pub async fn create_checkout_dir(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    path: String,
) -> Result<()> {
    create_checkout_dir_impl(&supervisor, &agent_id, &path).await
}

/// Copy a checkout file to a new path (the explorer's "Duplicate"). Refuses
/// to overwrite an existing destination.
#[tauri::command]
pub async fn copy_checkout_file(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    from: String,
    to: String,
) -> Result<()> {
    copy_checkout_file_impl(&supervisor, &agent_id, &from, &to).await
}

/// Persist a file the user pasted into the composer (a screenshot from the
/// clipboard, or a file copied in Finder) so it can be attached by path like a
/// dropped or browsed file. The webview only sees the pasted bytes, never a
/// path, so the copy lands under `<data_dir>/attachments/<uuid>/<name>` — a
/// per-paste directory keeps the original name intact without collisions.
/// Returns the absolute path.
///
/// The bytes travel as the raw IPC body (no JSON-encoding a screenshot byte by
/// byte); the name rides in the `name` header, sanitized by the frontend to a
/// plain ASCII basename.
#[tauri::command]
pub fn save_pasted_attachment(request: tauri::ipc::Request<'_>) -> Result<String> {
    let bytes = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => bytes,
        _ => return Err(Error::Other("expected raw attachment bytes".into())),
    };
    let name = request
        .headers()
        .get("name")
        .and_then(|v| v.to_str().ok())
        .map(|s| crate::attachments::sanitize_name(s, "pasted"))
        .unwrap_or_else(|| "pasted".to_string());
    // Stage under the app-data dir; the file is moved into the target agent's
    // workspace at send time (`attachments::adopt`), since a confined agent
    // can't read the app-data dir.
    let path = crate::attachments::save_pasted(&name, bytes)?;
    Ok(path.to_string_lossy().into_owned())
}

/// Copy a file the user dropped or browsed to into the attachments staging
/// area and return the staged path, so it reaches the agent's workspace at
/// send time like a pasted one. The agent sandbox denies the folders macOS
/// guards with a privacy prompt (Downloads, Desktop, Documents, iCloud Drive),
/// so a file attached from one has to be copied somewhere the agent can read.
/// The app reads it here, at drop time: if macOS asks about the folder, it asks
/// while the user is attaching a file from it.
#[tauri::command]
pub fn stage_attachment_path(path: String) -> Result<String> {
    let path = crate::attachments::stage_copy(&expand_tilde(&path))?;
    Ok(path.to_string_lossy().into_owned())
}
