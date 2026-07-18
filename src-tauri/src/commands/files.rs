//! File panel — browse the checkout, view & edit file contents — plus the
//! shared agent → repo → checkout resolution helpers the rest of the command
//! modules build on.

use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tauri::State;

use crate::error::{Error, Result};
use crate::git;
use crate::git_state::{self, FileStatus, StatusKind};
use crate::supervisor::Supervisor;
use crate::workspace::{repo_checkout_path, TrackedRepo};

/// The ref a checkout's *committed* changes are diffed against: the immutable
/// fork-point SHA captured at spawn when known, else the parent branch name
/// (pre-migration agents), which may have drifted from the actual fork point.
/// PR/merge/rebase bases and ahead/behind use `parent_branch` directly instead,
/// since those need a live branch name, not a commit.
pub(super) fn diff_base(repo: &TrackedRepo) -> Option<String> {
    repo.base_sha.clone().or_else(|| repo.parent_branch.clone())
}

// ---------------------------------------------------------------------------
// File panel — browse the checkout, view & edit file contents.
// ---------------------------------------------------------------------------

/// Largest file the viewer will load. Bigger files report `too_large` so
/// the UI shows a "no preview" notice instead of choking the editor.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// One entry in an arbitrary directory listing (for the composer's `@`
/// file-mention autocomplete when the user types a filesystem path).
#[derive(Serialize)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
}

/// A directory listing plus the absolute path that was listed, so the
/// caller can build absolute attachment paths from entry names.
#[derive(Serialize)]
pub struct DirListing {
    /// Absolute, tilde-expanded directory that was read.
    pub base: String,
    pub entries: Vec<DirEntry>,
}

/// One entry in the checkout file list. Directories are derived on the
/// frontend from the path segments; only files are sent over IPC.
#[derive(Serialize)]
pub struct CheckoutFile {
    pub path: String,
    /// Git status vs the parent branch: "M" | "A" | "D" | "R" (None = clean).
    pub status: Option<String>,
    pub additions: u32,
    pub deletions: u32,
}

/// A single file's contents plus the metadata the editor needs.
#[derive(Serialize)]
pub struct CheckoutFileContents {
    pub text: String,
    /// File-extension hint (e.g. "ts", "rs", "py"); "" when unknown.
    pub lang: String,
    pub status: Option<String>,
    /// 1-indexed line numbers the agent added / modified (change gutter).
    pub chg_add: Vec<u32>,
    pub chg_mod: Vec<u32>,
    pub binary: bool,
    pub too_large: bool,
}

/// Collapse a rich git status into the single-letter code the panel renders.
/// Untracked reads as added; conflicted reads as modified.
fn status_code(kind: &StatusKind) -> &'static str {
    match kind {
        StatusKind::Modified | StatusKind::Conflicted => "M",
        StatusKind::Added | StatusKind::Untracked => "A",
        StatusKind::Deleted => "D",
        StatusKind::Renamed => "R",
    }
}

/// Map a path's extension to a language hint for the highlighter.
fn lang_for(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Join a caller-supplied relative path onto the checkout root, rejecting
/// anything that could escape it (absolute paths, `..`, drive prefixes).
fn safe_join(checkout: &Path, rel: &str) -> Result<PathBuf> {
    let p = Path::new(rel);
    let escapes = p.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    });
    if p.is_absolute() || escapes || rel.is_empty() {
        return Err(Error::InvalidPath(rel.to_string()));
    }
    Ok(checkout.join(p))
}

// ── Agent → repo → checkout resolution ────────────────────────────
// Nearly every git/PR command operates on the agent's *primary* (first) repo.
// These helpers centralize that resolution — and its error strings — so the
// command bodies stay focused on the git/gh call they actually make.

/// The agent's primary (first) repo, or an error if the agent has no repos.
pub(super) fn primary_repo(supervisor: &Supervisor, agent_id: &str) -> Result<TrackedRepo> {
    supervisor
        .workspace
        .agent(agent_id)?
        .repos
        .into_iter()
        .next()
        .ok_or_else(|| Error::Other("agent has no repos".into()))
}

/// The agent's primary repo paired with its checkout path.
pub(super) fn primary_repo_checkout(
    supervisor: &Supervisor,
    agent_id: &str,
) -> Result<(TrackedRepo, PathBuf)> {
    let repo = primary_repo(supervisor, agent_id)?;
    let checkout = repo_checkout_path(agent_id, &repo.subdir)?;
    Ok((repo, checkout))
}

/// The tracked repo a panel command targets: the one whose `subdir` matches,
/// or the primary when no subdir is given — which keeps every single-repo
/// caller (and old frontends that don't pass the arg) byte-identical.
pub(super) fn agent_repo_checkout(
    supervisor: &Supervisor,
    agent_id: &str,
    subdir: Option<&str>,
) -> Result<(TrackedRepo, PathBuf)> {
    let Some(s) = subdir else {
        return primary_repo_checkout(supervisor, agent_id);
    };
    let record = supervisor.workspace.agent(agent_id)?;
    let repo = record
        .repos
        .into_iter()
        .find(|r| r.subdir == s)
        .ok_or_else(|| Error::Other(format!("agent has no tracked repo {s:?}")))?;
    let checkout = repo_checkout_path(agent_id, &repo.subdir)?;
    Ok((repo, checkout))
}

/// Best-effort variant for read-only lookups (git / PR state): returns `None`
/// instead of an error when the agent or its repo can't be resolved, so callers
/// can degrade gracefully rather than surfacing a failure.
pub(super) fn agent_repo_checkout_opt(
    supervisor: &Supervisor,
    agent_id: &str,
    subdir: Option<&str>,
) -> Result<Option<(TrackedRepo, PathBuf)>> {
    let Ok(record) = supervisor.workspace.agent(agent_id) else {
        return Ok(None);
    };
    let repo = match subdir {
        None => record.repos.into_iter().next(),
        Some(s) => record.repos.into_iter().find(|r| r.subdir == s),
    };
    let Some(repo) = repo else {
        return Ok(None);
    };
    let checkout = repo_checkout_path(agent_id, &repo.subdir)?;
    Ok(Some((repo, checkout)))
}

/// The agent's branch name, or an error if the checkout has no branch yet.
pub(super) fn repo_branch(repo: &TrackedRepo) -> Result<&str> {
    repo.branch
        .as_deref()
        .ok_or_else(|| Error::Other("agent has no branch yet".into()))
}

/// Resolve the agent's primary checkout and its parent ref (the fork point
/// used for file-tree / per-file diffs).
fn primary_checkout(supervisor: &Supervisor, agent_id: &str) -> Result<(PathBuf, String)> {
    let (repo, checkout) = primary_repo_checkout(supervisor, agent_id)?;
    // File tree / per-file diffs compare committed work against the fork point.
    let parent = diff_base(&repo).unwrap_or_else(|| "main".to_string());
    Ok((checkout, parent))
}

/// Split a repo-prefixed Code-tab path (`"<subdir>/<rel>"`) into its tracked
/// repo and the checkout-relative remainder. Only ever called for multi-repo
/// agents — a single-repo agent's paths are never prefixed (all prefix logic
/// is gated on `repos.len() > 1`), so a real top-level directory that happens
/// to share a repo's name can't be misrouted here.
fn split_repo_path<'a>(repos: &'a [TrackedRepo], path: &str) -> Result<(&'a TrackedRepo, String)> {
    let (first, rest) = path.split_once('/').unwrap_or((path, ""));
    let repo = repos.iter().find(|r| r.subdir == first).ok_or_else(|| {
        let known: Vec<&str> = repos.iter().map(|r| r.subdir.as_str()).collect();
        Error::InvalidPath(format!(
            "{path:?} must start with one of the agent's repo folders: {}",
            known.join(", ")
        ))
    })?;
    if rest.is_empty() {
        return Err(Error::InvalidPath(format!(
            "{path:?} names a repo folder itself, not a path inside it"
        )));
    }
    Ok((repo, rest.to_string()))
}

/// Resolve a Code-tab path to the checkout it lives in: `(checkout root,
/// parent ref, checkout-relative path)`. Single-repo agents use the primary
/// checkout with the path unchanged — the exact legacy behavior. For a
/// multi-repo agent every tree path is prefixed with the repo's `subdir`
/// (see `list_checkout_tree`), so the first segment picks the checkout.
fn checkout_scope_for_path(
    supervisor: &Supervisor,
    agent_id: &str,
    path: &str,
) -> Result<(PathBuf, String, String)> {
    let record = supervisor.workspace.agent(agent_id)?;
    if record.repos.len() <= 1 {
        let (checkout, parent) = primary_checkout(supervisor, agent_id)?;
        return Ok((checkout, parent, path.to_string()));
    }
    let (repo, rel) = split_repo_path(&record.repos, path)?;
    let checkout = repo_checkout_path(agent_id, &repo.subdir)?;
    let parent = diff_base(repo).unwrap_or_else(|| "main".to_string());
    Ok((checkout, parent, rel))
}

/// One checkout's file list (tracked + untracked, deleted dropped), each
/// tagged with its git status vs `parent`. `prefix` (a repo's subdir) is
/// prepended to every path for multi-repo agents' virtual roots.
async fn checkout_tree_files(
    checkout: &Path,
    parent: &str,
    prefix: Option<&str>,
) -> Vec<CheckoutFile> {
    let state = git_state::query(checkout, parent).await.ok();
    let status_for = |path: &str| -> Option<&FileStatus> {
        state.as_ref()?.files.iter().find(|f| f.path == path)
    };

    let mut paths: BTreeSet<String> = git::list_files(checkout)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();
    if let Some(s) = &state {
        for f in &s.files {
            // A deleted file is gone from disk, so a file tree shouldn't show
            // it — and `ls-files --cached` still lists it (it's in the index),
            // so we must actively remove it. Everything else (untracked adds,
            // modifications) belongs in the tree.
            if matches!(f.kind, StatusKind::Deleted) {
                paths.remove(&f.path);
            } else {
                paths.insert(f.path.clone());
            }
        }
    }

    paths
        .into_iter()
        .map(|path| {
            let st = status_for(&path);
            CheckoutFile {
                status: st.map(|f| status_code(&f.kind).to_string()),
                additions: st.map(|f| f.additions).unwrap_or(0),
                deletions: st.map(|f| f.deletions).unwrap_or(0),
                path: match prefix {
                    Some(p) => format!("{p}/{path}"),
                    None => path,
                },
            }
        })
        .collect()
}

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
    let record = supervisor.workspace.agent(&agent_id)?;
    if record.repos.len() <= 1 {
        let (checkout, parent) = primary_checkout(&supervisor, &agent_id)?;
        return Ok(checkout_tree_files(&checkout, &parent, None).await);
    }
    let mut out = Vec::new();
    for repo in &record.repos {
        // One broken checkout shouldn't blank the whole tree — skip it and
        // keep listing the others.
        let Ok(checkout) = repo_checkout_path(&agent_id, &repo.subdir) else {
            continue;
        };
        let parent = diff_base(repo).unwrap_or_else(|| "main".to_string());
        out.extend(checkout_tree_files(&checkout, &parent, Some(&repo.subdir)).await);
    }
    Ok(out)
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

/// Expand a leading `~` (or `~/…`) to the user's home directory. Any other
/// path is returned unchanged. Used to resolve filesystem paths the user
/// types into the composer's `@` mention.
pub(super) fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix('~') {
        if rest.is_empty() || rest.starts_with('/') {
            if let Some(home) = dirs::home_dir() {
                return home.join(rest.strip_prefix('/').unwrap_or(rest));
            }
        }
    }
    PathBuf::from(path)
}

/// List the entries of an arbitrary directory for the composer's `@`
/// mention autocomplete (e.g. `@~/Downloads/`). The path may start with
/// `~`; the resolved absolute directory comes back as `base` so the caller
/// can attach files by absolute path.
#[tauri::command]
pub async fn list_dir(path: String) -> Result<DirListing> {
    // Stop reading well above what the picker shows (the frontend filters and
    // caps display at 10) so a huge directory like /usr/lib or node_modules
    // can't stall the read or bloat the IPC payload. Hidden entries are kept
    // so typing a leading "." can still reveal dotfiles.
    const MAX_ENTRIES: usize = 1000;

    let dir = expand_tilde(&path);
    let read = std::fs::read_dir(&dir)
        .map_err(|e| Error::Other(format!("read_dir {}: {e}", dir.display())))?;

    let mut entries = Vec::new();
    for entry in read.flatten().take(MAX_ENTRIES) {
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        entries.push(DirEntry { name, is_dir });
    }

    Ok(DirListing {
        base: dir.to_string_lossy().to_string(),
        entries,
    })
}

/// Read a checkout file for the viewer/editor: contents, language hint,
/// git status, and the changed-line numbers driving the gutter.
#[tauri::command]
pub async fn read_checkout_file(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    path: String,
) -> Result<CheckoutFileContents> {
    let (checkout, parent, path) = checkout_scope_for_path(&supervisor, &agent_id, &path)?;
    let abs = safe_join(&checkout, &path)?;
    let lang = lang_for(&path);

    let state = git_state::query(&checkout, &parent).await.ok();
    let status = state
        .as_ref()
        .and_then(|s| s.files.iter().find(|f| f.path == path))
        .map(|f| status_code(&f.kind).to_string());

    let empty = |text: String, binary: bool, too_large: bool| CheckoutFileContents {
        text,
        lang: lang.clone(),
        status: status.clone(),
        chg_add: vec![],
        chg_mod: vec![],
        binary,
        too_large,
    };

    // Deleted by the agent: the file is gone from disk, so show its prior
    // contents from the parent ref (the design lets you re-create it).
    if status.as_deref() == Some("D") {
        let text = git::show_file(&checkout, &parent, &path)
            .await
            .unwrap_or_default();
        return Ok(empty(text, false, false));
    }

    if !abs.is_file() {
        return Ok(empty(String::new(), false, false));
    }
    if std::fs::metadata(&abs)?.len() > MAX_FILE_BYTES {
        return Ok(empty(String::new(), false, true));
    }
    let bytes = std::fs::read(&abs)?;
    if bytes.contains(&0) {
        return Ok(empty(String::new(), true, false));
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();

    let (chg_add, chg_mod) = if matches!(status.as_deref(), Some("M") | Some("R")) {
        git::file_changed_lines(&checkout, &parent, &path)
            .await
            .unwrap_or_default()
    } else {
        (vec![], vec![])
    };

    Ok(CheckoutFileContents {
        text,
        lang,
        status,
        chg_add,
        chg_mod,
        binary: false,
        too_large: false,
    })
}

/// Full unified diff of one checkout file versus the parent branch — the data
/// behind the Code panel's Live view. Returns "" when the file is unchanged.
#[tauri::command]
pub async fn get_file_diff(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    path: String,
) -> Result<String> {
    let (checkout, parent, path) = checkout_scope_for_path(&supervisor, &agent_id, &path)?;
    git::file_diff(&checkout, &parent, &path).await
}

/// Overwrite a checkout file with new contents (the editor's Save / Revert).
#[tauri::command]
pub async fn write_checkout_file(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    path: String,
    contents: String,
) -> Result<()> {
    let (checkout, _parent, path) = checkout_scope_for_path(&supervisor, &agent_id, &path)?;
    let abs = safe_join(&checkout, &path)?;
    if let Some(dir) = abs.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&abs, contents)?;
    Ok(())
}

/// Resolve a not-yet-existing destination inside the checkout: reject path
/// traversal, refuse to clobber an existing entry, and create its parent
/// directory. The create / rename / copy commands all share this so the
/// no-clobber + path-safety contract lives in exactly one place.
fn resolve_new_path(checkout: &Path, rel: &str) -> Result<PathBuf> {
    let abs = safe_join(checkout, rel)?;
    if abs.exists() {
        return Err(Error::Other(format!("\"{rel}\" already exists")));
    }
    if let Some(dir) = abs.parent() {
        std::fs::create_dir_all(dir)?;
    }
    Ok(abs)
}

/// Rename/move a checkout path (file or directory). Refuses to clobber an
/// existing destination so a rename can never silently overwrite a sibling.
/// Source and destination resolve their repo scope independently, so a move
/// between a multi-repo agent's checkouts (sibling directories on the same
/// volume) works like any other rename.
#[tauri::command]
pub async fn rename_checkout_path(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    from: String,
    to: String,
) -> Result<()> {
    let (checkout_from, _parent, from) = checkout_scope_for_path(&supervisor, &agent_id, &from)?;
    let (checkout_to, _parent, to) = checkout_scope_for_path(&supervisor, &agent_id, &to)?;
    let src = safe_join(&checkout_from, &from)?;
    let dst = resolve_new_path(&checkout_to, &to)?;
    std::fs::rename(&src, &dst)?;
    Ok(())
}

/// Delete a checkout path. Files are removed directly; directories are
/// removed recursively (the UI guards this behind a confirm step). Deleting a
/// path that's already gone is a no-op, so concurrent deletes don't error.
#[tauri::command]
pub async fn delete_checkout_path(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    path: String,
) -> Result<()> {
    let (checkout, _parent, path) = checkout_scope_for_path(&supervisor, &agent_id, &path)?;
    let abs = safe_join(&checkout, &path)?;
    if abs.is_dir() {
        std::fs::remove_dir_all(&abs)?;
    } else if abs.exists() {
        std::fs::remove_file(&abs)?;
    }
    Ok(())
}

/// Create a new empty file, making parent directories as needed. Refuses to
/// overwrite an existing path.
#[tauri::command]
pub async fn create_checkout_file(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    path: String,
) -> Result<()> {
    let (checkout, _parent, path) = checkout_scope_for_path(&supervisor, &agent_id, &path)?;
    let abs = resolve_new_path(&checkout, &path)?;
    std::fs::write(&abs, "")?;
    Ok(())
}

/// Create a new directory. Refuses to clobber an existing path.
#[tauri::command]
pub async fn create_checkout_dir(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    path: String,
) -> Result<()> {
    let (checkout, _parent, path) = checkout_scope_for_path(&supervisor, &agent_id, &path)?;
    let abs = resolve_new_path(&checkout, &path)?;
    std::fs::create_dir_all(&abs)?;
    Ok(())
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
    let (checkout_from, _parent, from) = checkout_scope_for_path(&supervisor, &agent_id, &from)?;
    let (checkout_to, _parent, to) = checkout_scope_for_path(&supervisor, &agent_id, &to)?;
    let src = safe_join(&checkout_from, &from)?;
    let dst = resolve_new_path(&checkout_to, &to)?;
    std::fs::copy(&src, &dst)?;
    Ok(())
}

#[cfg(test)]
mod split_repo_path_tests {
    use super::split_repo_path;
    use crate::workspace::TrackedRepo;

    fn repo(subdir: &str) -> TrackedRepo {
        TrackedRepo {
            repo_path: std::path::PathBuf::from(format!("/src/{subdir}")),
            subdir: subdir.into(),
            branch: None,
            parent_branch: None,
            base_sha: None,
            pr_number: None,
            pr_url: None,
            pr_title: None,
            pr_state: None,
            label: None,
        }
    }

    #[test]
    fn routes_first_segment_to_the_matching_repo() {
        let repos = [repo("frontend"), repo("backend")];
        let (r, rel) = split_repo_path(&repos, "backend/src/main.rs").unwrap();
        assert_eq!(r.subdir, "backend");
        assert_eq!(rel, "src/main.rs");
    }

    #[test]
    fn rejects_unknown_prefix_listing_tracked_folders() {
        let repos = [repo("frontend"), repo("backend")];
        let err = split_repo_path(&repos, "shared/util.ts").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("frontend"),
            "should list tracked folders: {msg}"
        );
        assert!(
            msg.contains("backend"),
            "should list tracked folders: {msg}"
        );
    }

    #[test]
    fn rejects_a_bare_repo_root() {
        // "frontend" alone names the checkout itself — never a file operation
        // target (renaming/deleting a repo root must not be possible).
        let repos = [repo("frontend"), repo("backend")];
        assert!(split_repo_path(&repos, "frontend").is_err());
        assert!(split_repo_path(&repos, "frontend/").is_err());
    }
}

#[cfg(test)]
mod safe_join_tests {
    use super::safe_join;
    use std::path::Path;

    #[test]
    fn accepts_nested_relative_path() {
        let wt = Path::new("/tmp/wt");
        assert_eq!(
            safe_join(wt, "src/server/checkout.ts").unwrap(),
            wt.join("src/server/checkout.ts")
        );
    }

    #[test]
    fn rejects_parent_traversal() {
        let wt = Path::new("/tmp/wt");
        assert!(safe_join(wt, "../secrets").is_err());
        assert!(safe_join(wt, "src/../../etc/passwd").is_err());
    }

    #[test]
    fn rejects_absolute_and_empty() {
        let wt = Path::new("/tmp/wt");
        assert!(safe_join(wt, "/etc/passwd").is_err());
        assert!(safe_join(wt, "").is_err());
    }
}
