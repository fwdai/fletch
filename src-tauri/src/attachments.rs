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
    // Resolve the staging root once, so the per-path containment check below
    // compares real, `..`-and-symlink-free paths. If it can't be resolved
    // (nothing was ever staged), nothing is a staged file — pass everything
    // through untouched.
    let staging = match staging.canonicalize() {
        Ok(root) => root,
        Err(_) => return paths.to_vec(),
    };
    paths
        .iter()
        .map(|p| adopt_one(&staging, dest_base, Path::new(p)).unwrap_or_else(|| p.clone()))
        .collect()
}

/// Move `path` into `dest_base` and return the rewritten path, but only if it
/// resolves to a **regular file at the exact shape [`save_pasted`] writes**:
/// `<staging>/<uuid>/<filename>`. `None` leaves the caller's original string in
/// place, which is the safe outcome for every reject:
/// - a dragged/browsed file kept at its own location;
/// - a `..` or symlink path that escapes staging — moving it would drag an
///   app-data file (e.g. `fletch.db`) into agent-readable space, whereas
///   leaving it is safe (the sandbox still denies the agent that path);
/// - the staging root itself or any directory beneath it — `starts_with` is
///   reflexive, so a plain prefix test would accept the root and `rename` the
///   whole staging tree (every unsent attachment) into one agent's workspace.
///
/// The strict `<staging>/<uuid>/<filename>` shape (a regular file exactly one
/// dir below the canonical staging root) rejects all three at once, without a
/// separate `starts_with`.
fn adopt_one(staging: &Path, dest_base: &Path, path: &Path) -> Option<String> {
    let canonical = path.canonicalize().ok()?;
    if !canonical.is_file() {
        return None;
    }
    if canonical.parent().and_then(Path::parent) != Some(staging) {
        return None;
    }
    match move_into(dest_base, &canonical) {
        Ok(dest) => Some(dest.to_string_lossy().into_owned()),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "failed to adopt attachment; leaving staged path");
            None
        }
    }
}

/// Move one staged file to a fresh UUID dir under `dest_base` and return the new
/// path. `path` is the canonicalized, verified-under-staging source. The empty
/// staging dir it leaves behind is removed best-effort.
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
        assert!(
            moved.starts_with(workspace.path()),
            "moved under workspace: {out:?}"
        );
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

        let out = adopt_into(
            staging.path(),
            workspace.path(),
            std::slice::from_ref(&original),
        );

        assert_eq!(out, vec![original]);
    }

    /// A `..` path that escapes the staging root is never moved: it lexically
    /// "starts with" the staging root (the flaw a plain `starts_with` would
    /// miss), but its canonical form resolves to a sibling of staging — e.g.
    /// `fletch.db` — which must stay put, out of agent-readable space.
    #[test]
    fn a_traversal_path_out_of_staging_is_not_adopted() {
        let staging = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let secret = staging.path().parent().unwrap().join("fletch.db");
        std::fs::write(&secret, b"secrets").unwrap();
        let escape = staging.path().join("..").join("fletch.db");
        // The old, component-wise check would have accepted this.
        assert!(escape.starts_with(staging.path()));
        let escape_str = escape.to_string_lossy().into_owned();

        let out = adopt_into(
            staging.path(),
            workspace.path(),
            std::slice::from_ref(&escape_str),
        );

        assert_eq!(out.len(), 1);
        assert!(
            !Path::new(&out[0]).starts_with(workspace.path()),
            "escaped path must not be moved into the workspace: {out:?}"
        );
        assert!(secret.exists(), "the escaped file must stay put");
    }

    /// A symlink inside staging pointing at a file outside it is not followed
    /// into the workspace — canonicalization resolves the link before the
    /// containment check, so the real target stays where it is.
    #[test]
    #[cfg(unix)]
    fn a_symlink_escaping_staging_is_not_adopted() {
        let staging = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let secret = staging.path().parent().unwrap().join("fletch.db");
        std::fs::write(&secret, b"secrets").unwrap();
        let link = staging.path().join("innocent.png");
        std::os::unix::fs::symlink(&secret, &link).unwrap();

        let out = adopt_into(
            staging.path(),
            workspace.path(),
            std::slice::from_ref(&link.to_string_lossy().into_owned()),
        );

        assert!(!Path::new(&out[0]).starts_with(workspace.path()));
        assert!(secret.exists(), "the symlink target must stay put");
    }

    /// Neither the staging root nor a directory beneath it is adopted: moving
    /// the root would drag every other unsent attachment into this agent's
    /// workspace, and `starts_with` treats the root as a prefix of itself, so
    /// only the regular-file + strict-shape check refuses them.
    #[test]
    fn a_directory_under_staging_is_never_adopted() {
        let staging = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        // Another paste's staged file, sitting under the root, unsent.
        let uuid_dir = staging.path().join("someuuid");
        let other = uuid_dir.join("secret.png");
        std::fs::create_dir_all(&uuid_dir).unwrap();
        std::fs::write(&other, b"other").unwrap();

        let dirs = [
            staging.path().to_string_lossy().into_owned(), // the root itself
            uuid_dir.to_string_lossy().into_owned(),       // a dir beneath it
        ];
        let out = adopt_into(staging.path(), workspace.path(), &dirs);

        assert_eq!(out, dirs.to_vec(), "directories pass through, unmoved");
        assert!(other.exists(), "no other staged file is relocated");
        assert!(
            std::fs::read_dir(workspace.path())
                .unwrap()
                .next()
                .is_none(),
            "nothing lands in the workspace"
        );
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
