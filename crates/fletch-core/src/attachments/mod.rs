//! Attachment storage: a staging area under the app-data dir the composer
//! writes to, and adoption into an agent's workspace at send time.
//!
//! Pasted/clipboard files have no path of their own, so [`save_pasted`] writes
//! their bytes out and hands the composer a path to stage. Dragged or browsed
//! files have one, but it may be inside a folder the sandbox now denies agents
//! (Downloads, Desktop, Documents, iCloud Drive — the ones macOS guards with a
//! privacy prompt), so [`stage_copy`] copies them into the same staging area
//! at drop time. Either way the path lands under the app-data dir
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
//!
//! A box agent's workspace lives in its guest, which has no view of the host's
//! files: the host holds only placeholders at the same paths. So when the
//! workspace is a remote tree (`exec::for_path`), `adopt` also writes each file
//! into the guest at the very path the agent is handed, before anything moves
//! on the host. The host copy stays for the transcript to show.
//!
//! A paired phone stages the same way, in chunks over the remote protocol
//! ([`remote`]): its files have no path on this Mac at all, so they are written
//! into the staging area as they arrive and reach the agent through the very
//! same `adopt` at send time.

pub mod remote;

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::exec::Executor;
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

/// The line a message carries for one attachment, after the user's own text:
/// `Attached file: <path>`, or `Attached folder: <path>` for a folder the
/// user granted the agent (`workspace::folder_grants`), which passes through
/// [`adopt`] untouched and is read where it is.
pub fn reference_line(path: &str) -> String {
    let kind = if Path::new(path).is_dir() {
        "folder"
    } else {
        "file"
    };
    format!("Attached {kind}: {path}")
}

/// Root under the app-data dir where the composer stages pasted attachments.
pub(crate) fn staging_root() -> PathBuf {
    crate::data_dir().join(STAGING_SUBDIR)
}

/// The display filename a caller supplied, reduced to its last component so a
/// path-shaped name (`../fletch.db`, `/etc/passwd`) can only ever name a file
/// inside the staging dir it is written to. Empty or all-separator input falls
/// back to `fallback`.
pub fn sanitize_name(name: &str, fallback: &str) -> String {
    Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .unwrap_or(fallback)
        .to_string()
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

/// Copy a file the user dropped or browsed to into a fresh staging dir and
/// return the staged path — the same shape [`save_pasted`] writes, so
/// [`adopt`] carries it into the workspace at send time.
///
/// A dropped file used to stay at its own path, readable because the sandbox
/// left every read open. It no longer does for the folders macOS guards with a
/// privacy prompt — Downloads above all, where screenshots land — so the copy
/// is what puts the file somewhere the agent can read. Taken by the app at drop
/// time, which is also the one moment a privacy prompt for that folder makes
/// sense to the user. Desktop command only: a paired device stages over the
/// chunked upload and never names a path on this Mac.
pub fn stage_copy(path: &Path) -> Result<PathBuf> {
    stage_copy_into(&staging_root(), path)
}

/// Core of [`stage_copy`], with the staging root passed explicitly so it's
/// testable without the process-global app-data root.
fn stage_copy_into(staging: &Path, path: &Path) -> Result<PathBuf> {
    if !path.is_file() {
        return Err(Error::Other(format!(
            "{} is not a file that can be attached",
            path.display()
        )));
    }
    let name = sanitize_name(
        &path.file_name().unwrap_or_default().to_string_lossy(),
        "file",
    );
    let dir = staging.join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(name);
    std::fs::copy(path, &dest)?;
    Ok(dest)
}

/// Move any staged attachments in `paths` into `agent_id`'s workspace, returning
/// the rewritten path list. A path that isn't under the staging root (a dragged
/// or browsed file, kept at its original location) passes through unchanged.
///
/// On the host this is best-effort: if the workspace dir can't be resolved, or
/// a single move fails, the original path is kept and the failure logged — the
/// send still proceeds (the agent then hits the same read error it would have
/// without adoption, rather than losing the message entirely).
///
/// Into a box it is not: a file that cannot be written into the guest fails the
/// send with the reason, since the path would name nothing the agent can read.
/// The guest is written first, so on that error every attachment is still
/// staged and a resend carries it.
///
/// Blocking: a stopped box is booted to receive the files.
pub fn adopt(agent_id: &str, paths: &[String]) -> Result<Vec<String>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let dest_base = match workspace_attachments_dir(agent_id) {
        Ok(dir) => dir,
        Err(e) => {
            tracing::warn!(agent_id, error = %e, "cannot resolve workspace attachments dir; leaving staged paths");
            return Ok(paths.to_vec());
        }
    };
    let exec = crate::exec::for_path(&dest_base);
    adopt_into(&staging_root(), &dest_base, &exec, paths)
}

/// Core of [`adopt`], with both roots and the workspace's executor passed
/// explicitly so it's testable without the process-global app-data /
/// workspaces roots or a box.
fn adopt_into(
    staging: &Path,
    dest_base: &Path,
    exec: &Executor,
    paths: &[String],
) -> Result<Vec<String>> {
    // Resolve the staging root once, so the per-path containment check below
    // compares real, `..`-and-symlink-free paths. If it can't be resolved
    // (nothing was ever staged), nothing is a staged file — pass everything
    // through untouched.
    let staging = match staging.canonicalize() {
        Ok(root) => root,
        Err(_) => return Ok(paths.to_vec()),
    };
    // Each staged file and where it lands; `None` for a path left as it is.
    let moves: Vec<Option<(PathBuf, PathBuf)>> = paths
        .iter()
        .map(|p| {
            let src = staged_file(&staging, Path::new(p))?;
            let name = src.file_name()?;
            let dest = dest_base.join(uuid::Uuid::new_v4().to_string()).join(name);
            Some((src, dest))
        })
        .collect();
    let remote = matches!(exec, Executor::Remote(_));
    if remote {
        for (src, dest) in moves.iter().flatten() {
            let bytes = std::fs::read(src)?;
            exec.write(dest, &bytes).map_err(|e| {
                Error::Other(format!(
                    "could not copy attachment {} into the box: {e}",
                    dest.file_name().unwrap_or_default().to_string_lossy()
                ))
            })?;
        }
    }
    Ok(paths
        .iter()
        .zip(moves)
        .map(|(p, planned)| match planned {
            Some((src, dest)) => match move_to(&src, &dest) {
                Ok(()) => dest.to_string_lossy().into_owned(),
                // The guest holds it at `dest` already, which is the copy the
                // agent reads; only the transcript's host copy is missing.
                Err(e) if remote => {
                    tracing::warn!(path = %p, error = %e, "failed to keep a host copy of a box attachment");
                    dest.to_string_lossy().into_owned()
                }
                Err(e) => {
                    tracing::warn!(path = %p, error = %e, "failed to adopt attachment; leaving staged path");
                    p.clone()
                }
            },
            None => p.clone(),
        })
        .collect())
}

/// Carry the attachments a workspace adopted from `from_workspace`, its dir on
/// the side it leaves, to `to_workspace`, its dir on the side it moves to (a
/// move between sandboxes, `supervisor::sandbox_move`), each at the same path
/// below: the conversation names them by path. Read where the workspace
/// keeps them: in the guest for a box, which holds every one (adoption writes
/// it there first, the host copy only best-effort). Placed as [`adopt`]
/// places them: written into the guest when `to_workspace` is a box's tree,
/// and always copied on the host, the copy the transcript shows. Nothing to
/// do when none were adopted.
///
/// Blocking: a stopped box is booted to give or receive the files.
pub fn carry(from_workspace: &Path, to_workspace: &Path) -> Result<()> {
    let from = from_workspace.join(WORKSPACE_ATTACHMENTS_DIR);
    let to = to_workspace.join(WORKSPACE_ATTACHMENTS_DIR);
    carry_into(
        &from,
        &crate::exec::for_path(&from),
        &to,
        &crate::exec::for_path(&to),
    )
}

/// Core of [`carry`], with each side's executor passed explicitly so it's
/// testable without a box.
fn carry_into(from: &Path, source: &Executor, to: &Path, dest: &Executor) -> Result<()> {
    for rel in carried_files(from, source)? {
        let bytes = source.read(&from.join(&rel))?;
        let out = to.join(&rel);
        if matches!(dest, Executor::Remote(_)) {
            dest.write(&out, &bytes)?;
        }
        if let Some(dir) = out.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&out, &bytes)?;
    }
    Ok(())
}

/// The regular files under `dir`, relative to it, where `exec` keeps it. A
/// link is never followed, nor carried, `dir` itself included: the agent can
/// write here, and a link it planted would have the move copy whatever it
/// names, or loop through an ancestor. In a guest that's `find` without `-L`,
/// which examines a link as a link; on the host, a walk by file type.
fn carried_files(dir: &Path, exec: &Executor) -> Result<Vec<PathBuf>> {
    if matches!(exec, Executor::Remote(_)) {
        let out = exec
            .command("find", Path::new("/"), &[])
            .arg(dir)
            .args(["-type", "f", "-print0"])
            .output()?;
        if !out.status.success() {
            // Nothing adopted: there is no dir to list.
            if !exec.exists(dir)? {
                return Ok(Vec::new());
            }
            return Err(Error::Other(format!(
                "listing the attachments in the box failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        return Ok(out
            .stdout
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .filter_map(|p| {
                let path = Path::new(std::str::from_utf8(p).ok()?);
                path.strip_prefix(dir).ok().map(Path::to_path_buf)
            })
            .collect());
    }
    let mut files = Vec::new();
    match std::fs::symlink_metadata(dir) {
        Ok(meta) if meta.is_dir() => walk_files(dir, Path::new(""), &mut files)?,
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(files)
}

/// [`carried_files`] on the host: `rel` is `dir`'s path below the root.
fn walk_files(dir: &Path, rel: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let name = rel.join(entry.file_name());
        if kind.is_dir() {
            walk_files(&entry.path(), &name, files)?;
        } else if kind.is_file() {
            files.push(name);
        }
    }
    Ok(())
}

/// The canonical form of `path` when it resolves to a **regular file at the
/// exact shape [`save_pasted`] writes**: `<staging>/<uuid>/<filename>`. `None`
/// leaves the caller's original string in place, which is the safe outcome for
/// every reject:
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
fn staged_file(staging: &Path, path: &Path) -> Option<PathBuf> {
    let canonical = path.canonicalize().ok()?;
    if !canonical.is_file() {
        return None;
    }
    if canonical.parent().and_then(Path::parent) != Some(staging) {
        return None;
    }
    Some(canonical)
}

/// Move one staged file to `dest` (a fresh UUID dir under the workspace's
/// attachments dir) on the host. `src` is the canonicalized, verified-under-
/// staging source. The empty staging dir it leaves behind is removed
/// best-effort.
fn move_to(src: &Path, dest: &Path) -> Result<()> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Same-volume rename is atomic; fall back to copy+remove across volumes
    // (staging is under `~/Library`, the workspace under `~/.fletch` — usually
    // one volume, but not guaranteed).
    if std::fs::rename(src, dest).is_err() {
        std::fs::copy(src, dest)?;
        let _ = std::fs::remove_file(src);
    }
    if let Some(parent) = src.parent() {
        let _ = std::fs::remove_dir(parent);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::Arc;

    use parking_lot::Mutex;

    use super::*;
    use crate::exec::{DirEntry, RemoteExecutor, Stat};

    /// [`adopt_into`] for a workspace on this host.
    fn adopt_local(staging: &Path, dest_base: &Path, paths: &[String]) -> Vec<String> {
        adopt_into(staging, dest_base, &Executor::Local, paths).unwrap()
    }

    /// A granted folder is named as one, so the agent lists it rather than
    /// trying to read it; files keep the line they always had.
    #[test]
    fn a_folder_attachment_is_named_as_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.md");
        std::fs::write(&file, "x").unwrap();
        let (dir_s, file_s) = (dir.path().to_string_lossy(), file.to_string_lossy());
        assert_eq!(reference_line(&dir_s), format!("Attached folder: {dir_s}"));
        assert_eq!(reference_line(&file_s), format!("Attached file: {file_s}"));
        // A path that is gone (or lives only in a box) stays a file reference.
        assert_eq!(
            reference_line("/nonexistent/x.png"),
            "Attached file: /nonexistent/x.png"
        );
    }

    /// A box's tree as the host sees it through `exec`: records every write,
    /// or refuses them all.
    #[derive(Debug, Default)]
    struct FakeBox {
        writes: Mutex<Vec<(PathBuf, Vec<u8>)>>,
        unreachable: bool,
    }

    impl RemoteExecutor for FakeBox {
        fn command(&self, _: &str, _: &Path, _: &[(String, String)]) -> std::process::Command {
            unreachable!("adoption runs no command in the box")
        }
        fn read(&self, _: &Path) -> io::Result<Vec<u8>> {
            unreachable!("adoption reads nothing from the box")
        }
        fn write(&self, path: &Path, bytes: &[u8], _: u32) -> io::Result<()> {
            if self.unreachable {
                return Err(io::Error::other("ssh to the box failed"));
            }
            self.writes
                .lock()
                .push((path.to_path_buf(), bytes.to_vec()));
            Ok(())
        }
        fn stat(&self, _: &Path) -> io::Result<Option<Stat>> {
            Ok(None)
        }
        fn list(&self, _: &Path) -> io::Result<Vec<DirEntry>> {
            Ok(Vec::new())
        }
        fn create_dir_all(&self, _: &Path) -> io::Result<()> {
            Ok(())
        }
        fn remove(&self, _: &Path) -> io::Result<()> {
            Ok(())
        }
        fn rename(&self, _: &Path, _: &Path) -> io::Result<()> {
            Ok(())
        }
    }

    /// Into a box, each staged file is written into the guest at exactly the
    /// path the agent is handed — the guest has no view of the host's copy —
    /// and the host keeps its copy there too, for the transcript. A path that
    /// was never staged is not sent.
    #[test]
    fn a_box_workspace_receives_each_staged_file_at_the_handed_path() {
        let staging = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let staged = staging.path().join("abc").join("shot.png");
        std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
        std::fs::write(&staged, b"pixels").unwrap();
        let elsewhere = "/elsewhere/notes.txt".to_string();
        let guest = Arc::new(FakeBox::default());

        let out = adopt_into(
            staging.path(),
            workspace.path(),
            &Executor::Remote(guest.clone()),
            &[staged.to_string_lossy().into_owned(), elsewhere.clone()],
        )
        .unwrap();

        let handed = PathBuf::from(&out[0]);
        assert!(handed.starts_with(workspace.path()), "{out:?}");
        assert_eq!(out[1], elsewhere);
        assert_eq!(
            *guest.writes.lock(),
            vec![(handed.clone(), b"pixels".to_vec())],
            "the guest holds the bytes at the handed path, and nothing else"
        );
        assert_eq!(std::fs::read(&handed).unwrap(), b"pixels");
        assert!(!staged.exists());
    }

    /// A moving workspace's attachments land at the same paths on the other
    /// side: into the guest when it is a box's, and on the host either way,
    /// the source left as it was. With none adopted there is nothing to do.
    #[test]
    fn a_moving_workspace_carries_its_attachments_to_the_same_paths() {
        let td = tempfile::tempdir().unwrap();
        let (from, to) = (td.path().join("from"), td.path().join("to"));
        let shot = Path::new("abc").join("shot.png");
        std::fs::create_dir_all(from.join("abc")).unwrap();
        std::fs::write(from.join(&shot), b"pixels").unwrap();
        let local = Executor::Local;

        carry_into(&from, &local, &to, &local).unwrap();
        assert_eq!(std::fs::read(to.join(&shot)).unwrap(), b"pixels");
        assert_eq!(std::fs::read(from.join(&shot)).unwrap(), b"pixels");

        let boxed = td.path().join("boxed");
        let guest = Arc::new(FakeBox::default());
        carry_into(&from, &local, &boxed, &Executor::Remote(guest.clone())).unwrap();
        assert_eq!(
            *guest.writes.lock(),
            vec![(boxed.join(&shot), b"pixels".to_vec())]
        );
        assert_eq!(std::fs::read(boxed.join(&shot)).unwrap(), b"pixels");

        carry_into(&td.path().join("none"), &local, &to, &local).unwrap();
    }

    /// A guest whose files are this host's, reached as a box's are: commands
    /// run here, reads read here. What the remote branch of the carry needs.
    #[derive(Debug)]
    struct HostAsGuest;

    impl RemoteExecutor for HostAsGuest {
        fn command(
            &self,
            program: &str,
            cwd: &Path,
            _: &[(String, String)],
        ) -> std::process::Command {
            let mut cmd = std::process::Command::new(program);
            cmd.current_dir(cwd);
            cmd
        }
        fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
            std::fs::read(path)
        }
        fn write(&self, path: &Path, bytes: &[u8], _: u32) -> io::Result<()> {
            std::fs::write(path, bytes)
        }
        fn stat(&self, path: &Path) -> io::Result<Option<Stat>> {
            Ok(std::fs::metadata(path).ok().map(|m| Stat {
                is_dir: m.is_dir(),
                is_file: m.is_file(),
                len: m.len(),
            }))
        }
        fn list(&self, _: &Path) -> io::Result<Vec<DirEntry>> {
            unreachable!("the carry lists with find, never following a link")
        }
        fn create_dir_all(&self, path: &Path) -> io::Result<()> {
            std::fs::create_dir_all(path)
        }
        fn remove(&self, _: &Path) -> io::Result<()> {
            Ok(())
        }
        fn rename(&self, _: &Path, _: &Path) -> io::Result<()> {
            Ok(())
        }
    }

    /// Out of a box, the guest is read: it holds every attachment, while the
    /// host copy adoption keeps is best-effort and may be missing. A link the
    /// agent planted there, to a file outside or back up the tree, is neither
    /// followed nor carried; with nothing adopted there is nothing to do.
    #[test]
    fn a_box_workspace_carries_what_its_guest_holds_and_no_link() {
        let td = tempfile::tempdir().unwrap();
        let (from, to) = (td.path().join("box-ws"), td.path().join("mac-ws"));
        std::fs::create_dir_all(from.join("abc")).unwrap();
        std::fs::write(from.join("abc").join("shot.png"), b"pixels").unwrap();
        let secret = td.path().join("secret.txt");
        std::fs::write(&secret, b"key").unwrap();
        std::os::unix::fs::symlink(&secret, from.join("abc").join("link.txt")).unwrap();
        std::os::unix::fs::symlink(td.path(), from.join("up")).unwrap();
        let guest = Executor::Remote(Arc::new(HostAsGuest));

        carry_into(&from, &guest, &to, &Executor::Local).unwrap();

        assert_eq!(
            std::fs::read(to.join("abc").join("shot.png")).unwrap(),
            b"pixels"
        );
        assert!(!to.join("abc").join("link.txt").exists());
        assert!(!to.join("up").exists());

        carry_into(&td.path().join("none"), &guest, &to, &Executor::Local).unwrap();
    }

    /// A link in the host's attachments dir is not an attachment: the move
    /// never follows one to copy what it names.
    #[test]
    fn a_link_among_the_attachments_is_left_behind() {
        let td = tempfile::tempdir().unwrap();
        let (from, to) = (td.path().join("from"), td.path().join("to"));
        std::fs::create_dir_all(&from).unwrap();
        let secret = td.path().join("secret.txt");
        std::fs::write(&secret, b"key").unwrap();
        std::os::unix::fs::symlink(&secret, from.join("link.txt")).unwrap();

        carry_into(&from, &Executor::Local, &to, &Executor::Local).unwrap();

        assert!(!to.join("link.txt").exists());
    }

    /// A box that cannot take the files fails the send, naming the file, and
    /// leaves every attachment staged: nothing has moved on the host, so a
    /// resend still carries them.
    #[test]
    fn a_box_that_cannot_take_the_files_fails_and_leaves_them_staged() {
        let staging = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let staged = staging.path().join("abc").join("shot.png");
        std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
        std::fs::write(&staged, b"pixels").unwrap();
        let guest = Arc::new(FakeBox {
            unreachable: true,
            ..FakeBox::default()
        });

        let err = adopt_into(
            staging.path(),
            workspace.path(),
            &Executor::Remote(guest),
            &[staged.to_string_lossy().into_owned()],
        )
        .unwrap_err();

        assert!(err.to_string().contains("shot.png"), "{err}");
        assert!(staged.exists(), "the attachment stays staged");
        assert!(
            std::fs::read_dir(workspace.path())
                .unwrap()
                .next()
                .is_none(),
            "nothing lands in the workspace"
        );
    }

    /// A staged file is moved under the workspace root and its path rewritten;
    /// the original is gone and the bytes survive.
    #[test]
    fn adopts_a_staged_file_into_the_workspace() {
        let staging = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let staged = staging.path().join("abc").join("shot.png");
        std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
        std::fs::write(&staged, b"pixels").unwrap();

        let out = adopt_local(
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

    /// A dropped file is copied into staging at the shape `adopt` carries into
    /// the workspace, under its own name; the original stays where it was.
    #[test]
    fn stages_a_copy_of_a_dropped_file() {
        let staging = tempfile::tempdir().unwrap();
        let downloads = tempfile::tempdir().unwrap();
        let dropped = downloads.path().join("shot.png");
        std::fs::write(&dropped, b"pixels").unwrap();

        let staged = stage_copy_into(staging.path(), &dropped).unwrap();

        assert_eq!(staged.parent().and_then(Path::parent), Some(staging.path()));
        assert_eq!(staged.file_name().unwrap(), "shot.png");
        assert_eq!(std::fs::read(&staged).unwrap(), b"pixels");
        assert!(dropped.exists(), "the original is left in place");

        let workspace = tempfile::tempdir().unwrap();
        let out = adopt_local(
            staging.path(),
            workspace.path(),
            &[staged.to_string_lossy().into_owned()],
        );
        assert!(Path::new(&out[0]).starts_with(workspace.path()));
    }

    /// A directory, or a path that is not there, is refused rather than staged
    /// as an empty file.
    #[test]
    fn refuses_to_stage_what_is_not_a_file() {
        let staging = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        assert!(stage_copy_into(staging.path(), dir.path()).is_err());
        assert!(stage_copy_into(staging.path(), &dir.path().join("missing")).is_err());
        assert!(
            std::fs::read_dir(staging.path()).unwrap().next().is_none(),
            "nothing is left in staging"
        );
    }

    /// A dragged/browsed path (not under the staging root) is left exactly as-is.
    #[test]
    fn leaves_a_non_staged_path_untouched() {
        let staging = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let original = "/Users/someone/pictures/cat.png".to_string();

        let out = adopt_local(
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

        let out = adopt_local(
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

        let out = adopt_local(
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
        let out = adopt_local(staging.path(), workspace.path(), &dirs);

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

        let out = adopt_local(
            staging.path(),
            workspace.path(),
            &[staged.to_string_lossy().into_owned(), dragged.clone()],
        );

        assert!(Path::new(&out[0]).starts_with(workspace.path()));
        assert_eq!(out[1], dragged);
    }
}
