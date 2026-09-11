//! Pasted-attachment storage: a staging area under the app-data dir the
//! composer writes to, and adoption into an agent's workspace at send time.
//!
//! Dragged or browsed attachments already have an on-disk path and stay put —
//! reads under `$HOME` are open to confined agents. Pasted/clipboard files have
//! no path of their own, so [`save_pasted`] writes their bytes out and hands
//! the composer a path to stage. That path lands under the app-data dir
//! (`~/Library/Application Support/<BUNDLE_ID>`) — which the sandbox policy
//! denies confined agents from reading, the same guard that keeps `fletch.db`
//! opaque (see `sandbox::seatbelt::deny_app_data_dir`). So an agent handed a
//! staged path can't open it.
//!
//! [`adopt`] closes that gap: just before a message is delivered, it moves each
//! staged file into the target agent's workspace
//! (`~/.fletch/workspaces/<id>/.fletch-attachments/`) and rewrites the path.
//! That location is readable by the agent (it sits under the writable workspace
//! root) and is swept when the workspace is torn down
//! (`remove_agent_dir` → `remove_dir_all`), so a sent attachment's lifetime is
//! the workspace's. Non-staged paths pass through untouched.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::workspace::agent_parent_dir;

/// Subdir of the app-data dir where the composer stages pasted attachments.
/// Under the sandbox-denied app-data tree, so the app (never a confined agent)
/// is what writes and reads here — the file is moved out before any agent sees
/// its path.
const STAGING_SUBDIR: &str = "attachments";

/// Reserved subdir of an agent's workspace holding attachments adopted into it.
/// A dotted, Fletch-owned name so it can't collide with a repo checkout subdir
/// (`allocate_repo_subdir` reserves it), and readable by the agent since it
/// sits under the writable workspace root.
pub const WORKSPACE_ATTACHMENTS_DIR: &str = ".fletch-attachments";

/// Root under the app-data dir where the composer stages pasted attachments.
fn staging_root() -> PathBuf {
    crate::data_dir().join(STAGING_SUBDIR)
}

/// Where adopted attachments live for one agent:
/// `~/.fletch/workspaces/<agent-id>/.fletch-attachments/`.
fn workspace_attachments_dir(agent_id: &str) -> Result<PathBuf> {
    Ok(agent_parent_dir(agent_id)?.join(WORKSPACE_ATTACHMENTS_DIR))
}

/// Write pasted/clipboard `bytes` to a fresh staging dir and return the path.
/// `name` is the (already sanitized) display filename; a UUID dir keeps distinct
/// pastes that share a name apart.
pub fn save_pasted(name: &str, bytes: &[u8]) -> Result<PathBuf> {
    let dir = staging_root().join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(name);
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// Move any staged attachments in `paths` into `agent_id`'s workspace, returning
/// the rewritten path list. A path that isn't under the staging root (a dragged
/// or browsed file, kept at its original location) passes through unchanged.
///
/// Best-effort: if the workspace dir can't be resolved, or a single move fails,
/// the original path is kept and the failure logged — the send still proceeds
/// (the agent then hits the same read error it would have without adoption,
/// rather than losing the message entirely).
pub fn adopt(agent_id: &str, paths: &[String]) -> Vec<String> {
    let dest_base = match workspace_attachments_dir(agent_id) {
        Ok(dir) => dir,
        Err(e) => {
            tracing::warn!(agent_id, error = %e, "cannot resolve workspace attachments dir; leaving staged paths");
            return paths.to_vec();
        }
    };
    adopt_into(&staging_root(), &dest_base, paths)
}

/// Core of [`adopt`], with both roots passed explicitly so it's testable without
/// the process-global app-data / workspaces roots.
fn adopt_into(staging: &Path, dest_base: &Path, paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .map(|p| {
            let path = Path::new(p);
            if !path.starts_with(staging) {
                return p.clone();
            }
            match move_into(dest_base, path) {
                Ok(dest) => dest.to_string_lossy().into_owned(),
                Err(e) => {
                    tracing::warn!(path = %p, error = %e, "failed to adopt attachment; leaving staged path");
                    p.clone()
                }
            }
        })
        .collect()
}

/// Move one staged file to a fresh UUID dir under `dest_base` and return the new
/// path. The empty staging dir it leaves behind is removed best-effort.
fn move_into(dest_base: &Path, path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| Error::Other("staged attachment has no file name".into()))?;
    let dest_dir = dest_base.join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&dest_dir)?;
    let dest = dest_dir.join(name);
    // Same-volume rename is atomic; fall back to copy+remove across volumes
    // (staging is under `~/Library`, the workspace under `~/.fletch` — usually
    // one volume, but not guaranteed).
    if std::fs::rename(path, &dest).is_err() {
        std::fs::copy(path, &dest)?;
        let _ = std::fs::remove_file(path);
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir(parent);
    }
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A staged file is moved under the workspace root and its path rewritten;
    /// the original is gone and the bytes survive.
    #[test]
    fn adopts_a_staged_file_into_the_workspace() {
        let staging = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let staged = staging.path().join("abc").join("shot.png");
        std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
        std::fs::write(&staged, b"pixels").unwrap();

        let out = adopt_into(
            staging.path(),
            workspace.path(),
            &[staged.to_string_lossy().into_owned()],
        );

        assert_eq!(out.len(), 1);
        let moved = Path::new(&out[0]);
        assert!(moved.starts_with(workspace.path()), "moved under workspace: {out:?}");
        assert_eq!(moved.file_name().unwrap(), "shot.png");
        assert_eq!(std::fs::read(moved).unwrap(), b"pixels");
        assert!(!staged.exists(), "the staged copy is gone");
    }

    /// A dragged/browsed path (not under the staging root) is left exactly as-is.
    #[test]
    fn leaves_a_non_staged_path_untouched() {
        let staging = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let original = "/Users/someone/pictures/cat.png".to_string();

        let out = adopt_into(staging.path(), workspace.path(), &[original.clone()]);

        assert_eq!(out, vec![original]);
    }

    /// A mixed list keeps order: staged files move, non-staged pass through.
    #[test]
    fn preserves_order_across_a_mixed_list() {
        let staging = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let staged = staging.path().join("id").join("a.txt");
        std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
        std::fs::write(&staged, b"a").unwrap();
        let dragged = "/elsewhere/b.txt".to_string();

        let out = adopt_into(
            staging.path(),
            workspace.path(),
            &[staged.to_string_lossy().into_owned(), dragged.clone()],
        );

        assert!(Path::new(&out[0]).starts_with(workspace.path()));
        assert_eq!(out[1], dragged);
    }
}
