//! Turn checkpoints: the code as it stood when a turn was delivered, kept in
//! the checkout so fork ("code as of this message") and rewind can get back to
//! it (docs/fork-and-rewind.md).
//!
//! A checkpoint is a [`snapshot_worktree`] commit — committed plus uncommitted
//! work, minus ignored files, with the HEAD at capture as its parent — pinned
//! at `refs/fletch/checkpoints/<turn_id>` so gc keeps it. It lives in the
//! checkout's own object store, so it goes when the checkout does.
//!
//! A restore can be undone the same way. A checkout has at most one undo
//! point, pinned by [`pin_undo`] before a restore changes it: the code as it
//! stood, at `refs/fletch/undo/latest`, and the checkpoint the restore puts in
//! its place, at `refs/fletch/undo/restored`. [`undo`] puts the checkout back,
//! [`drop_undo`] lets the point go, and [`undo_is_current`] says whether the
//! checkout is still as the restore left it, the one state an undo may
//! replace. Nothing outside the checkout names these refs, so nothing can lose
//! track of them.

use std::path::Path;

use serde::Serialize;

use crate::error::{Error, Result};

use super::branch::rev_parse;
use super::cmd::{git_output, run_git};
use super::worktree::{apply_snapshot, snapshot_worktree};

/// Where a checkout's undo point is pinned: fixed refs, so the next one
/// replaces it and nothing outside the checkout has to remember a name. A
/// namespace of their own, apart from the checkpoints', which a turn's id could
/// otherwise clash with. `UNDO_REF` is the code to go back to, `RESTORED_REF`
/// the checkpoint the restore left in its place.
const UNDO_REF: &str = "refs/fletch/undo/latest";
const RESTORED_REF: &str = "refs/fletch/undo/restored";

/// Snapshot `checkout` and pin it as `turn_id`'s checkpoint, replacing any
/// earlier one. Returns the snapshot's sha.
pub async fn capture(checkout: &Path, turn_id: &str) -> Result<String> {
    pin_snapshot(checkout, &checkpoint_ref(turn_id)?).await
}

/// Pin `checkout`'s undo point, replacing any earlier one, before a
/// [`restore`] to `checkpoint` changes it: a snapshot of the checkout as it
/// stands, and the checkpoint it is about to become.
pub async fn pin_undo(checkout: &Path, checkpoint: &str) -> Result<()> {
    pin_snapshot(checkout, UNDO_REF).await?;
    run_git(
        checkout,
        &["update-ref", RESTORED_REF, checkpoint],
        "pin restored checkpoint",
    )
    .await?;
    Ok(())
}

/// Whether `checkout` has an undo point.
pub async fn has_undo(checkout: &Path) -> Result<bool> {
    Ok(resolve_ref(checkout, UNDO_REF).await?.is_some())
}

/// Whether `checkout` is still exactly as the restore its undo point was
/// pinned for left it: HEAD on the checkpoint's parent, and the working tree,
/// ignored files aside, the checkpoint's tree. Anything since — a commit, an
/// edit, a new file, from any view, terminal or editor — and an undo would
/// discard it. `false` when there is no restore to compare with.
pub async fn undo_is_current(checkout: &Path) -> Result<bool> {
    let Some(restored) = resolve_ref(checkout, RESTORED_REF).await? else {
        return Ok(false);
    };
    if rev_parse(checkout, "HEAD").await? != rev_parse(checkout, &format!("{restored}^")).await? {
        return Ok(false);
    }
    let now = snapshot_worktree(checkout).await?;
    let tree = |commit: &str| format!("{commit}^{{tree}}");
    Ok(rev_parse(checkout, &tree(&now)).await? == rev_parse(checkout, &tree(&restored)).await?)
}

/// Put `checkout` back to its undo point. The point stays until the caller
/// lets it go ([`drop_undo`]), so a failed undo can be tried again.
pub async fn undo(checkout: &Path) -> Result<()> {
    restore(checkout, UNDO_REF).await
}

/// Let `checkout`'s undo point go, if it has one.
pub async fn drop_undo(checkout: &Path) -> Result<()> {
    for refname in [UNDO_REF, RESTORED_REF] {
        run_git(checkout, &["update-ref", "-d", refname], "drop undo point").await?;
    }
    Ok(())
}

async fn pin_snapshot(checkout: &Path, refname: &str) -> Result<String> {
    let sha = snapshot_worktree(checkout).await?;
    run_git(checkout, &["update-ref", refname, &sha], "pin snapshot").await?;
    Ok(sha)
}

/// `turn_id`'s checkpoint in `checkout`, or `None` when it has none.
pub async fn resolve(checkout: &Path, turn_id: &str) -> Result<Option<String>> {
    resolve_ref(checkout, &checkpoint_ref(turn_id)?).await
}

async fn resolve_ref(checkout: &Path, refname: &str) -> Result<Option<String>> {
    let out = git_output(checkout, &["rev-parse", "--verify", "--quiet", refname]).await?;
    match out.status.code() {
        Some(0) => Ok(Some(
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        )),
        // `--verify --quiet` exits 1, silently, when the ref doesn't exist.
        Some(1) => Ok(None),
        _ => Err(Error::Git(format!(
            "resolve {refname} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))),
    }
}

/// Copy `turn_id`'s checkpoint from the `source` checkout (a fork's parent)
/// into `dest` under the same ref, and return its sha. Fetched by ref rather
/// than by sha, so `source` needs no `allowAnySHA1InWant`, and the pin keeps
/// it in `dest` until it is applied ([`restore`]).
pub async fn fetch_into(dest: &Path, source: &Path, turn_id: &str) -> Result<String> {
    let refname = checkpoint_ref(turn_id)?;
    let source = source
        .to_str()
        .ok_or_else(|| Error::InvalidPath(source.display().to_string()))?;
    // `+`: a re-captured checkpoint is never a fast-forward of the old one.
    let refspec = format!("+{refname}:{refname}");
    run_git(
        dest,
        &["fetch", "--no-tags", source, &refspec],
        "fetch checkpoint",
    )
    .await?;
    rev_parse(dest, &refname).await
}

/// Put `checkout` back to checkpoint `sha`: the working tree as captured, and
/// HEAD at the commit it was on then (the snapshot's parent), with the
/// uncommitted part showing as uncommitted again. Moves the branch HEAD is on,
/// if any. Work since — commits, edits, new untracked files — leaves the tree;
/// ignored files stay as they are.
pub async fn restore(checkout: &Path, sha: &str) -> Result<()> {
    apply_snapshot(checkout, sha, &format!("{sha}^")).await
}

/// A commit that restoring a checkpoint takes off the checkout's branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LeavingCommit {
    pub sha: String,
    pub subject: String,
    /// Some `origin` branch already has it, so it lives on there; dropping it
    /// from a branch that was pushed takes a force push.
    pub pushed: bool,
}

/// The commits restoring checkpoint `sha` takes off the branch in `checkout`:
/// those HEAD reaches and the checkpoint's HEAD doesn't, newest first.
/// "Pushed" is measured against `origin`'s refs, as the disposal check does.
pub async fn leaving_commits(checkout: &Path, sha: &str) -> Result<Vec<LeavingCommit>> {
    let target = format!("{sha}^");
    let log = run_git(
        checkout,
        &["log", "--format=%H%x09%s", "HEAD", "--not", &target],
        "list leaving commits",
    )
    .await?;
    let unpushed = run_git(
        checkout,
        &["rev-list", "HEAD", "--not", &target, "--remotes=origin"],
        "list unpushed commits",
    )
    .await?;
    let unpushed = String::from_utf8_lossy(&unpushed.stdout);
    let unpushed: Vec<&str> = unpushed.lines().collect();
    Ok(String::from_utf8_lossy(&log.stdout)
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(sha, subject)| LeavingCommit {
            pushed: !unpushed.contains(&sha),
            sha: sha.to_string(),
            subject: subject.to_string(),
        })
        .collect())
}

/// The ref `turn_id`'s checkpoint is pinned at. Turn ids are client-minted
/// UUIDs and end up in a ref name and a fetch refspec, so anything but a plain
/// token is refused rather than escaped.
fn checkpoint_ref(turn_id: &str) -> Result<String> {
    let plain = !turn_id.is_empty()
        && turn_id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_');
    if !plain {
        return Err(Error::Other(format!(
            "not a valid turn id for a checkpoint: {turn_id:?}"
        )));
    }
    Ok(format!("refs/fletch/checkpoints/{turn_id}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{commit_all, init_repo};
    use std::path::PathBuf;

    const TURN: &str = "6f1c2a9e-0b7d-4c8e-9f3a-2d5b8e1c4a70";

    /// A repo with an identity, a `.gitignore` and a first commit, at
    /// `<dir>/<name>`.
    async fn repo(dir: &Path, name: &str) -> PathBuf {
        let repo = dir.join(name);
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo).await.unwrap();
        run_git(&repo, &["config", "user.email", "t@example.com"], "config")
            .await
            .unwrap();
        run_git(&repo, &["config", "user.name", "Tester"], "config")
            .await
            .unwrap();
        std::fs::write(repo.join(".gitignore"), b"ignored.txt\n").unwrap();
        std::fs::write(repo.join("keep.txt"), b"base").unwrap();
        std::fs::write(repo.join("drop.txt"), b"remove me").unwrap();
        commit_all(&repo, "base").await.unwrap();
        repo
    }

    /// Commit a second file, then leave uncommitted work of every kind: a
    /// modification, a deletion, a new file and an ignored file.
    async fn work_in_progress(repo: &Path) {
        std::fs::write(repo.join("committed.txt"), b"committed").unwrap();
        commit_all(repo, "second").await.unwrap();
        std::fs::write(repo.join("keep.txt"), b"modified").unwrap();
        std::fs::remove_file(repo.join("drop.txt")).unwrap();
        std::fs::write(repo.join("new.txt"), b"added").unwrap();
        std::fs::write(repo.join("ignored.txt"), b"secret").unwrap();
    }

    /// `path` in `rev`'s tree, or `None` when the tree lacks it.
    async fn file_at(repo: &Path, rev: &str, path: &str) -> Option<Vec<u8>> {
        let out = git_output(repo, &["show", &format!("{rev}:{path}")])
            .await
            .unwrap();
        out.status.success().then_some(out.stdout)
    }

    async fn status(repo: &Path) -> String {
        let out = run_git(repo, &["status", "--porcelain"], "status")
            .await
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[tokio::test]
    async fn capture_pins_committed_and_uncommitted_work_but_not_ignored_files() {
        let td = tempfile::tempdir().unwrap();
        let repo = repo(td.path(), "repo").await;
        work_in_progress(&repo).await;
        let head = rev_parse(&repo, "HEAD").await.unwrap();
        let status_before = status(&repo).await;

        let sha = capture(&repo, TURN).await.unwrap();

        assert_eq!(resolve(&repo, TURN).await.unwrap(), Some(sha.clone()));
        assert_eq!(rev_parse(&repo, &format!("{sha}^")).await.unwrap(), head);
        assert_eq!(
            file_at(&repo, &sha, "committed.txt").await.as_deref(),
            Some(&b"committed"[..])
        );
        assert_eq!(
            file_at(&repo, &sha, "keep.txt").await.as_deref(),
            Some(&b"modified"[..])
        );
        assert_eq!(
            file_at(&repo, &sha, "new.txt").await.as_deref(),
            Some(&b"added"[..])
        );
        assert_eq!(file_at(&repo, &sha, "drop.txt").await, None);
        assert_eq!(file_at(&repo, &sha, "ignored.txt").await, None);
        // The checkout itself is left exactly as it was.
        assert_eq!(rev_parse(&repo, "HEAD").await.unwrap(), head);
        assert_eq!(status(&repo).await, status_before);
    }

    #[tokio::test]
    async fn a_checkpoint_survives_gc() {
        let td = tempfile::tempdir().unwrap();
        let repo = repo(td.path(), "repo").await;
        work_in_progress(&repo).await;
        let sha = capture(&repo, TURN).await.unwrap();

        run_git(&repo, &["gc", "--prune=now", "--quiet"], "gc")
            .await
            .unwrap();

        assert_eq!(resolve(&repo, TURN).await.unwrap(), Some(sha.clone()));
        assert_eq!(
            file_at(&repo, &sha, "new.txt").await.as_deref(),
            Some(&b"added"[..])
        );
    }

    #[tokio::test]
    async fn resolve_is_none_without_a_checkpoint() {
        let td = tempfile::tempdir().unwrap();
        let repo = repo(td.path(), "repo").await;
        assert_eq!(resolve(&repo, TURN).await.unwrap(), None);
    }

    #[tokio::test]
    async fn turn_ids_that_are_not_plain_tokens_are_refused() {
        let td = tempfile::tempdir().unwrap();
        let repo = repo(td.path(), "repo").await;
        for bad in [
            "",
            "../heads/main",
            "a:refs/heads/main",
            "a b",
            "a/b",
            "x~1",
        ] {
            assert!(capture(&repo, bad).await.is_err(), "{bad:?}");
            assert!(resolve(&repo, bad).await.is_err(), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn restore_brings_back_the_files_and_head_at_capture() {
        let td = tempfile::tempdir().unwrap();
        let repo = repo(td.path(), "repo").await;
        work_in_progress(&repo).await;
        let head = rev_parse(&repo, "HEAD").await.unwrap();
        let sha = capture(&repo, TURN).await.unwrap();
        let status_at_capture = status(&repo).await;

        // The turn goes on: a commit, then more uncommitted work of every kind.
        std::fs::write(repo.join("keep.txt"), b"later").unwrap();
        commit_all(&repo, "after the checkpoint").await.unwrap();
        std::fs::remove_file(repo.join("new.txt")).unwrap();
        std::fs::write(repo.join("committed.txt"), b"edited later").unwrap();
        std::fs::write(repo.join("stray.txt"), b"created later").unwrap();
        std::fs::write(repo.join("ignored.txt"), b"ignored, later").unwrap();

        restore(&repo, &sha).await.unwrap();

        assert_eq!(rev_parse(&repo, "HEAD").await.unwrap(), head);
        let read = |name| std::fs::read(repo.join(name)).ok();
        assert_eq!(read("keep.txt").as_deref(), Some(&b"modified"[..]));
        assert_eq!(read("new.txt").as_deref(), Some(&b"added"[..]));
        assert_eq!(read("committed.txt").as_deref(), Some(&b"committed"[..]));
        assert_eq!(read("drop.txt"), None);
        assert_eq!(read("stray.txt"), None, "untracked work since is removed");
        assert_eq!(
            read("ignored.txt").as_deref(),
            Some(&b"ignored, later"[..]),
            "ignored files are never touched"
        );
        // The uncommitted part reads as uncommitted again, as it did then.
        assert_eq!(status(&repo).await, status_at_capture);
    }

    /// A `--shared` clone of `source` at `dest`, the shape of every checkout.
    async fn checkout_of(source: &Path, dest: &Path) {
        run_git(
            dest.parent().unwrap(),
            &[
                "clone",
                "-q",
                "--shared",
                source.to_str().unwrap(),
                dest.to_str().unwrap(),
            ],
            "clone",
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn fetch_into_carries_a_checkpoint_to_another_checkout() {
        // A parent workspace and its fork, each a checkout of the same source,
        // so the parent's commit and snapshot exist only in the parent's store.
        let td = tempfile::tempdir().unwrap();
        let source = repo(td.path(), "source").await;
        let base = rev_parse(&source, "HEAD").await.unwrap();
        let (parent, fork) = (td.path().join("parent"), td.path().join("fork"));
        checkout_of(&source, &parent).await;
        checkout_of(&source, &fork).await;
        work_in_progress(&parent).await;
        let sha = capture(&parent, TURN).await.unwrap();

        assert_eq!(fetch_into(&fork, &parent, TURN).await.unwrap(), sha);
        assert_eq!(resolve(&fork, TURN).await.unwrap(), Some(sha.clone()));
        // Fetching again after a re-capture replaces the pin.
        std::fs::write(parent.join("new.txt"), b"added again").unwrap();
        let recaptured = capture(&parent, TURN).await.unwrap();
        assert_eq!(fetch_into(&fork, &parent, TURN).await.unwrap(), recaptured);

        // Restored, the fork has the parent's code as it was: HEAD on the
        // parent's commit (not the fork's base), the edits uncommitted on top.
        restore(&fork, &recaptured).await.unwrap();
        let parent_head = rev_parse(&parent, "HEAD").await.unwrap();
        assert_ne!(parent_head, base);
        assert_eq!(rev_parse(&fork, "HEAD").await.unwrap(), parent_head);
        assert_eq!(status(&fork).await, status(&parent).await);
        let read = |name| std::fs::read(fork.join(name)).ok();
        assert_eq!(read("new.txt").as_deref(), Some(&b"added again"[..]));
        assert_eq!(read("committed.txt").as_deref(), Some(&b"committed"[..]));
        assert_eq!(read("drop.txt"), None);
        assert_eq!(read("ignored.txt"), None);
    }
}
