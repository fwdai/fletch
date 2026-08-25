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
use crate::workspace::TrackedRepo;

/// The ref a checkout's *committed* changes are diffed against: the immutable
/// fork-point SHA captured at spawn when known, else the parent branch name
/// (pre-migration agents), which may have drifted from the actual fork point.
/// PR/merge/rebase bases and ahead/behind use `parent_branch` directly instead,
/// since those need a live branch name, not a commit.
pub(super) fn diff_base(repo: &TrackedRepo) -> Option<String> {
    repo.base_sha.clone().or_else(|| repo.parent_branch.clone())
}

/// Which ref the Code panel's diff surfaces measure against — the user's
/// persisted base switch. `Fork` is everything the agent changed in this
/// workspace (vs [`diff_base`]); `Head` is only uncommitted work (vs the
/// checkout's latest commit), matching the base the file lists already use
/// (`git_state::query` reads status/numstat vs HEAD).
#[derive(Debug, Clone, Copy, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffBaseMode {
    #[default]
    Fork,
    Head,
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
/// anything that could escape it — lexically (absolute paths, `..`, drive
/// prefixes) *and* through symlinks.
///
/// The symlink half is the load-bearing one. Every command below runs in the
/// **host** process, unsandboxed, while the checkout's contents belong to the
/// agent — which can plant a symlink at any path it likes. `fs::write`,
/// `fs::rename` and friends all follow symlinks, so a lexically-clean
/// `README.md` that happens to be a link to `~/.zshrc` would let the file
/// panel's Save write outside the workspace (and its viewer read outside it).
/// Same guard `agent_profile` applies to agent-planted links, applied at the
/// seam where a renderer-supplied path becomes a host filesystem operation.
///
/// Symlinks that stay *inside* the checkout are allowed — repos legitimately
/// contain them — so the test is where the path resolves, not whether a link
/// was traversed. Only the existing prefix can be resolved (these commands also
/// create new files), which leaves a narrow window where the agent replaces a
/// not-yet-existing leaf with a link between this check and the caller's write.
/// Closing that needs `O_NOFOLLOW`-per-component `openat` plumbing; the planted
/// trap this does close is persistent and needs no race to spring.
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
    let joined = checkout.join(p);
    // Compare fully-resolved forms: the checkout path itself reaches us through
    // `~/.fletch/workspaces/…`, which on macOS may sit under a symlinked
    // prefix, so resolving only one side would reject every legitimate path.
    let root = resolve(checkout);
    if !resolve(&joined).starts_with(&root) || escapes_via_symlink(&root, checkout, p) {
        return Err(Error::InvalidPath(rel.to_string()));
    }
    Ok(joined)
}

/// Symlink-resolve the longest existing prefix of `p`, keeping the rest.
fn resolve(p: &Path) -> PathBuf {
    crate::sandbox::policy::resolve_existing_prefix(p)
}

/// Cap on link hops per component, mirroring the kernel's `SYMLOOP_MAX`. A
/// cycle (`x` → `y` → `x`) never terminates on its own; the kernel answers
/// `ELOOP` and so do we, by refusing the path.
const MAX_SYMLINK_HOPS: usize = 32;

/// Whether walking `rel` under `checkout` leaves `root` by way of a symlink.
///
/// This is a second pass because resolving the longest existing prefix alone
/// misses the sharpest case: a **dangling** symlink. `canonicalize` fails on
/// one, so the leaf is re-appended verbatim and the path reads as an ordinary
/// in-checkout file — while `fs::write` would happily follow it and *create*
/// the outside target. Reading the links explicitly catches that, and catches
/// it before the file exists. In-tree links resolve back under `root` and pass.
///
/// Each component's links are followed to the **end of the chain**, not one
/// hop. A single hop is not enough: `a` → `b` → `/outside/nope` has an entirely
/// innocent first hop (`b` is in the checkout), and with a dangling tail
/// `resolve` can't see past it either — it gives up at the missing target and
/// re-anchors the remainder under the checkout. `fs::write` has no such trouble
/// and follows all the way to `/outside/nope`, creating it.
fn escapes_via_symlink(root: &Path, checkout: &Path, rel: &Path) -> bool {
    let mut cur = checkout.to_path_buf();
    for component in rel.components() {
        cur.push(component);
        for hop in 0.. {
            let Ok(meta) = std::fs::symlink_metadata(&cur) else {
                // Nothing here yet, so it isn't a link — and nothing can exist
                // beneath a path that doesn't exist.
                return false;
            };
            if !meta.file_type().is_symlink() {
                break;
            }
            if hop >= MAX_SYMLINK_HOPS {
                return true; // cycle or absurd nesting — fail closed
            }
            let Ok(target) = std::fs::read_link(&cur) else {
                return true; // unreadable link — fail closed
            };
            let absolute = if target.is_absolute() {
                target
            } else {
                cur.parent().unwrap_or(checkout).join(target)
            };
            // `resolve` both collapses a fully-resolvable chain in one step and
            // normalizes any `..` in the target; where it stalls (a dangling
            // hop) the loop reads the next link itself.
            cur = resolve(&absolute);
            if !cur.starts_with(root) {
                return true;
            }
        }
    }
    false
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
    let checkout = repo.checkout_path(agent_id)?;
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
    let checkout = repo.checkout_path(agent_id)?;
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
    let checkout = repo.checkout_path(agent_id)?;
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
    let checkout = repo.checkout_path(agent_id)?;
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
        let Ok(checkout) = repo.checkout_path(&agent_id) else {
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
    let (checkout, parent, path) = checkout_scope_for_path(&supervisor, &agent_id, &path)?;
    let parent = match base_mode.unwrap_or_default() {
        DiffBaseMode::Head => "HEAD".to_string(),
        DiffBaseMode::Fork => parent,
    };
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
    let (checkout, parent, path) = checkout_scope_for_path(&supervisor, &agent_id, &path)?;
    let parent = match base_mode.unwrap_or_default() {
        DiffBaseMode::Head => "HEAD".to_string(),
        DiffBaseMode::Fork => parent,
    };
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
            adopted_checkout: None,
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

    /// A checkout + an "outside" dir standing in for the user's home.
    fn checkout_and_outside() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let td = tempfile::tempdir().unwrap();
        let checkout = td.path().join("checkout");
        let outside = td.path().join("outside");
        std::fs::create_dir_all(&checkout).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        (td, checkout, outside)
    }

    /// The agent owns the checkout's contents, so a lexically-clean path can
    /// still be a symlink out of it. `fs::write` would follow it, letting the
    /// editor's Save overwrite (and its viewer read) an arbitrary host file.
    #[test]
    fn rejects_file_symlinked_outside_the_checkout() {
        let (_td, checkout, outside) = checkout_and_outside();
        let secret = outside.join("zshrc");
        std::fs::write(&secret, b"# real user config").unwrap();
        std::os::unix::fs::symlink(&secret, checkout.join("README.md")).unwrap();

        assert!(
            safe_join(&checkout, "README.md").is_err(),
            "a file symlinked out of the checkout must be rejected"
        );
    }

    /// The directory variant: the leaf doesn't exist yet, so only the symlinked
    /// parent gives the escape away — this is the path `create_checkout_file`
    /// and `rename_checkout_path` take.
    #[test]
    fn rejects_path_under_a_symlinked_directory() {
        let (_td, checkout, outside) = checkout_and_outside();
        std::os::unix::fs::symlink(&outside, checkout.join("docs")).unwrap();

        assert!(
            safe_join(&checkout, "docs/new-file.md").is_err(),
            "a new path under a symlinked-out directory must be rejected"
        );
    }

    /// A dangling symlink is the sharpest case: `exists()` is false, so
    /// `resolve_new_path`'s no-clobber guard passes and `fs::write` would
    /// *create* the outside target.
    #[test]
    fn rejects_dangling_symlink_pointing_outside() {
        let (_td, checkout, outside) = checkout_and_outside();
        let not_yet = outside.join("authorized_keys");
        std::os::unix::fs::symlink(&not_yet, checkout.join("notes.txt")).unwrap();
        assert!(!not_yet.exists(), "target must not exist for this case");

        assert!(
            safe_join(&checkout, "notes.txt").is_err(),
            "a dangling symlink out of the checkout must be rejected"
        );
    }

    /// A *chain* of links: the first hop stays in the checkout, so checking one
    /// hop clears it — but the write follows the chain to the end. Compounded
    /// with a dangling tail, since that is what defeats prefix resolution.
    #[test]
    fn rejects_dangling_symlink_chain_out_of_the_checkout() {
        let (_td, checkout, outside) = checkout_and_outside();
        let never = outside.join("nope");
        // a -> b (in-checkout, innocent-looking), b -> outside/nope (missing).
        std::os::unix::fs::symlink("b", checkout.join("a")).unwrap();
        std::os::unix::fs::symlink(&never, checkout.join("b")).unwrap();
        assert!(!never.exists(), "the chain's tail must not exist");

        assert!(
            safe_join(&checkout, "a").is_err(),
            "a symlink chain ending outside the checkout must be rejected"
        );
    }

    /// A longer chain, escaping only on the last hop — guards against the walk
    /// being accidentally bounded at one or two hops.
    #[test]
    fn rejects_long_symlink_chain_escaping_on_the_final_hop() {
        let (_td, checkout, outside) = checkout_and_outside();
        // h0 -> h1 -> h2 -> h3 -> outside/nope (missing)
        for i in 0..3 {
            std::os::unix::fs::symlink(format!("h{}", i + 1), checkout.join(format!("h{i}")))
                .unwrap();
        }
        std::os::unix::fs::symlink(outside.join("nope"), checkout.join("h3")).unwrap();

        assert!(
            safe_join(&checkout, "h0").is_err(),
            "the walk must follow the chain to its end"
        );
    }

    /// The same chain shape, but landing back inside — must still be allowed.
    #[test]
    fn allows_symlink_chain_that_stays_inside_the_checkout() {
        let (_td, checkout, _outside) = checkout_and_outside();
        std::fs::write(checkout.join("real.txt"), b"x").unwrap();
        std::os::unix::fs::symlink("b", checkout.join("a")).unwrap();
        std::os::unix::fs::symlink("real.txt", checkout.join("b")).unwrap();

        assert!(safe_join(&checkout, "a").is_ok(), "in-tree chain must work");
    }

    /// A cycle must not hang the walk, and must fail closed.
    #[test]
    fn rejects_symlink_cycle() {
        let (_td, checkout, _outside) = checkout_and_outside();
        std::os::unix::fs::symlink("y", checkout.join("x")).unwrap();
        std::os::unix::fs::symlink("x", checkout.join("y")).unwrap();

        assert!(
            safe_join(&checkout, "x").is_err(),
            "a cycle must be refused"
        );
    }

    /// A *relative* link target (`../outside`) escapes just as well and takes
    /// the other branch of the target-resolution — it is joined onto the link's
    /// parent rather than used as-is.
    #[test]
    fn rejects_relative_symlink_target_escaping_the_checkout() {
        let (_td, checkout, outside) = checkout_and_outside();
        std::fs::write(outside.join("secret"), b"x").unwrap();
        std::os::unix::fs::symlink("../outside/secret", checkout.join("link.txt")).unwrap();

        assert!(
            safe_join(&checkout, "link.txt").is_err(),
            "a relative symlink target that escapes must be rejected"
        );
    }

    /// Repos legitimately contain symlinks (monorepo links, `node_modules/.bin`).
    /// The test is where a path *lands*, not whether a link was traversed — so
    /// in-tree links must keep working.
    #[test]
    fn allows_symlinks_that_stay_inside_the_checkout() {
        let (_td, checkout, _outside) = checkout_and_outside();
        let real = checkout.join("packages/core");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("index.ts"), b"export {};").unwrap();
        std::os::unix::fs::symlink(&real, checkout.join("linked")).unwrap();

        assert!(
            safe_join(&checkout, "linked/index.ts").is_ok(),
            "an in-tree symlink must remain usable"
        );
        // And a not-yet-existing file beneath one, so Save-as still works.
        assert!(safe_join(&checkout, "linked/new.ts").is_ok());
    }

    /// New files and dirs — the common case — must not be collateral damage.
    #[test]
    fn allows_paths_that_do_not_exist_yet() {
        let (_td, checkout, _outside) = checkout_and_outside();
        assert!(safe_join(&checkout, "src/deeply/nested/new.ts").is_ok());
    }
}
