//! Per-repo codegraph index *mirrors* and how a built index reaches an agent's
//! checkout — the mechanism that keeps the index out of the user's source repo.
//!
//! A mirror is a throwaway `git clone --shared` of the source repo living under
//! `~/.fletch/projects/<project_id>/codegraph/<repo>/`. We index the mirror
//! (`codegraph init`/`sync`) and copy the resulting `.codegraph/codegraph.db`
//! into each fresh agent checkout. The db is content-hash based and stores
//! project-root-relative paths, so it is valid in any other checkout of the same
//! repo; the MCP server in the workspace catch-up-syncs and watches from there.
//!
//! Every operation is best-effort: callers log and continue, and a mirror is
//! serialized against itself with a per-path async mutex so two concurrent
//! spawns of the same repo don't race the clone / fetch / reset / sync.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};

use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::error::{Error, Result};
use crate::workspace::projects_root;

/// The index directory name codegraph writes inside a project root, and the
/// line we add to a checkout's `.git/info/exclude` so it never shows in diffs.
const INDEX_DIR: &str = ".codegraph";
/// The index db and its WAL sidecars — the only files worth copying between
/// checkouts. `daemon.pid`/`daemon.sock` are per-checkout runtime state and are
/// deliberately never copied.
const INDEX_DB_FILES: [&str; 3] = ["codegraph.db", "codegraph.db-wal", "codegraph.db-shm"];

/// Stable per-repo identifier: the hex SHA-256 of the canonicalized source path,
/// truncated. Canonicalization means two spellings of the same repo share a
/// mirror; a path that can't be canonicalized (e.g. already gone) falls back to
/// the raw path, which is still stable for a given string.
fn repo_identifier(source_repo: &Path) -> String {
    let canonical = source_repo
        .canonicalize()
        .unwrap_or_else(|_| source_repo.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(canonical.to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    hex16(&digest)
}

/// First 16 bytes of a digest as lowercase hex (32 chars) — plenty to avoid
/// collisions across a user's repo set without an unwieldy dir name.
fn hex16(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(32);
    for b in bytes.iter().take(16) {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Absolute path to a repo's index mirror:
/// `~/.fletch/projects/<project_id>/codegraph/<repo-id>/`.
pub fn mirror_dir(project_id: &str, source_repo: &Path) -> Result<PathBuf> {
    Ok(projects_root()?
        .join(project_id)
        .join("codegraph")
        .join(repo_identifier(source_repo)))
}

/// Per-mirror lock registry: serializes clone/fetch/reset/sync on one mirror dir
/// so concurrent spawns of the same repo don't race. Keyed by the mirror path.
fn mirror_lock(dir: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<StdMutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    let map = LOCKS.get_or_init(|| StdMutex::new(HashMap::new()));
    let mut guard = map.lock().unwrap();
    guard
        .entry(dir.to_path_buf())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// Ensure the mirror exists and holds an initial index. If the mirror dir is
/// missing, `git clone --shared` the source into it and run `codegraph init`.
/// A no-op (bar the lock) when the mirror is already present.
pub async fn ensure_mirror(
    source_repo: &Path,
    mirror_dir: &Path,
    codegraph_bin: &Path,
) -> Result<()> {
    let lock = mirror_lock(mirror_dir);
    let _guard = lock.lock().await;

    if mirror_dir.join(".git").exists() {
        return Ok(());
    }
    if let Some(parent) = mirror_dir.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| Error::Other(format!("create codegraph mirror parent: {e}")))?;
    }
    // A leftover partial dir (crashed clone) would fail the clone; clear it.
    if mirror_dir.exists() {
        let _ = tokio::fs::remove_dir_all(mirror_dir).await;
    }

    let source = path_str(source_repo)?;
    let dest = path_str(mirror_dir)?;
    let out = crate::git_dist::command(source_repo)
        .args(["clone", "--shared", &source, &dest])
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| Error::Other(format!("spawn git clone for mirror: {e}")))?;
    if !out.status.success() {
        let _ = tokio::fs::remove_dir_all(mirror_dir).await;
        return Err(Error::Git(format!(
            "clone --shared for codegraph mirror failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }

    run_codegraph(codegraph_bin, mirror_dir, "init").await
}

/// Advance an existing mirror to `sha` and re-index incrementally: fetch the
/// commit from the source, hard-reset the mirror to it, then `codegraph sync`.
/// Codegraph's content-hash change detection only re-parses files that differ,
/// so this is cheap on a small delta.
pub async fn advance_mirror(
    mirror_dir: &Path,
    source_repo: &Path,
    sha: &str,
    codegraph_bin: &Path,
) -> Result<()> {
    let lock = mirror_lock(mirror_dir);
    let _guard = lock.lock().await;

    let source = path_str(source_repo)?;
    // Best-effort fetch: the mirror is a `--shared` clone borrowing the source's
    // live object store via alternates, so a fork-point commit reachable in the
    // source is already visible here and `reset --hard` alone advances the tree.
    // A raw-SHA fetch over local transport is also refused by default
    // (`uploadpack.allowAnySHA1InWant` off), so treat a fetch failure as a no-op
    // and let the reset be authoritative — it fails loudly if the object really
    // is missing.
    match crate::git_dist::command(mirror_dir)
        .args(["fetch", &source, sha])
        .kill_on_drop(true)
        .output()
        .await
    {
        Ok(out) if out.status.success() => {}
        Ok(out) => tracing::debug!(
            sha,
            stderr = %String::from_utf8_lossy(&out.stderr).trim(),
            "codegraph mirror fetch by sha not served; relying on borrowed objects"
        ),
        Err(e) => tracing::debug!(error = %e, "codegraph mirror fetch spawn failed; continuing"),
    }
    let reset = crate::git_dist::command(mirror_dir)
        .args(["reset", "--hard", sha])
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| Error::Other(format!("spawn git reset for mirror: {e}")))?;
    if !reset.status.success() {
        return Err(Error::Git(format!(
            "reset --hard {sha} in codegraph mirror failed: {}",
            String::from_utf8_lossy(&reset.stderr).trim()
        )));
    }

    run_codegraph(codegraph_bin, mirror_dir, "sync").await
}

/// Run a codegraph subcommand (`init` / `sync`) against a mirror. cwd is the
/// mirror and the target path is passed explicitly; `CODEGRAPH_NO_DAEMON=1` +
/// `CODEGRAPH_TELEMETRY=0` keep host-side runs from leaving a daemon or phoning
/// home. Never runs `install`/`uninstall`.
async fn run_codegraph(bin: &Path, mirror_dir: &Path, subcommand: &str) -> Result<()> {
    let dest = path_str(mirror_dir)?;
    let out = tokio::process::Command::new(bin)
        .arg(subcommand)
        .arg(&dest)
        .current_dir(mirror_dir)
        .env("CODEGRAPH_NO_DAEMON", "1")
        .env("CODEGRAPH_TELEMETRY", "0")
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| Error::Other(format!("spawn codegraph {subcommand}: {e}")))?;
    if !out.status.success() {
        return Err(Error::Other(format!(
            "codegraph {subcommand} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Copy the mirror's built index db (and WAL sidecars, if present) into
/// `<checkout>/.codegraph/`, then register the exclude entry. Per-checkout
/// runtime files (`daemon.pid`/`daemon.sock`) and anything else are skipped: a
/// copied daemon socket/pid would point another checkout's runtime at stale
/// state. A no-op when the mirror has no `codegraph.db` yet (a late index copies
/// on a subsequent spawn).
pub async fn copy_index_into(mirror_dir: &Path, checkout: &Path) -> Result<()> {
    let src_index = mirror_dir.join(INDEX_DIR);
    let db = src_index.join(INDEX_DB_FILES[0]);
    if !db.exists() {
        return Ok(());
    }
    let dst_index = checkout.join(INDEX_DIR);
    tokio::fs::create_dir_all(&dst_index)
        .await
        .map_err(|e| Error::Other(format!("create checkout .codegraph: {e}")))?;
    for name in INDEX_DB_FILES {
        let from = src_index.join(name);
        if from.exists() {
            tokio::fs::copy(&from, dst_index.join(name))
                .await
                .map_err(|e| Error::Other(format!("copy {name} into checkout: {e}")))?;
        }
    }
    append_git_exclude(checkout).await
}

/// Add `.codegraph` to a checkout's `.git/info/exclude` so the copied index
/// never surfaces in `git status`/diffs. Idempotent: the line is added at most
/// once, and the file (and its parent) is created if missing.
pub async fn append_git_exclude(checkout: &Path) -> Result<()> {
    let info_dir = checkout.join(".git").join("info");
    tokio::fs::create_dir_all(&info_dir)
        .await
        .map_err(|e| Error::Other(format!("create .git/info: {e}")))?;
    let exclude = info_dir.join("exclude");
    let existing = tokio::fs::read_to_string(&exclude)
        .await
        .unwrap_or_default();
    if existing.lines().any(|l| l.trim() == INDEX_DIR) {
        return Ok(());
    }
    let mut next = existing;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(INDEX_DIR);
    next.push('\n');
    tokio::fs::write(&exclude, next)
        .await
        .map_err(|e| Error::Other(format!("write .git/info/exclude: {e}")))?;
    Ok(())
}

fn path_str(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| Error::InvalidPath(path.display().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_path_derivation_is_stable_and_namespaced() {
        // Derive against the real `projects_root()` without mutating the
        // process-global env (which would race parallel tests). We only assert
        // structure relative to that root.
        let root = projects_root().unwrap();
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("acme");
        std::fs::create_dir_all(&repo).unwrap();

        let a = mirror_dir("proj-1", &repo).unwrap();
        let b = mirror_dir("proj-1", &repo).unwrap();
        assert_eq!(a, b, "same repo → same mirror dir");

        // Namespaced under the project and the codegraph subdir.
        assert!(a.starts_with(root.join("proj-1").join("codegraph")));

        // A different project yields a different dir even for the same repo.
        let c = mirror_dir("proj-2", &repo).unwrap();
        assert_ne!(a, c);
    }

    #[test]
    fn repo_identifier_differs_by_path() {
        let td = tempfile::tempdir().unwrap();
        let one = td.path().join("one");
        let two = td.path().join("two");
        std::fs::create_dir_all(&one).unwrap();
        std::fs::create_dir_all(&two).unwrap();
        assert_ne!(repo_identifier(&one), repo_identifier(&two));
        // 32 hex chars (16 bytes).
        assert_eq!(repo_identifier(&one).len(), 32);
    }

    #[tokio::test]
    async fn append_git_exclude_is_idempotent() {
        let td = tempfile::tempdir().unwrap();
        let checkout = td.path();
        // No .git/info yet — must be created.
        append_git_exclude(checkout).await.unwrap();
        append_git_exclude(checkout).await.unwrap();
        append_git_exclude(checkout).await.unwrap();
        let body = std::fs::read_to_string(checkout.join(".git/info/exclude")).unwrap();
        let count = body.lines().filter(|l| l.trim() == ".codegraph").count();
        assert_eq!(count, 1, "the exclude line must appear exactly once");
    }

    #[tokio::test]
    async fn append_git_exclude_preserves_existing_content() {
        let td = tempfile::tempdir().unwrap();
        let checkout = td.path();
        let info = checkout.join(".git/info");
        std::fs::create_dir_all(&info).unwrap();
        std::fs::write(info.join("exclude"), "# user rules\n*.log").unwrap();
        append_git_exclude(checkout).await.unwrap();
        let body = std::fs::read_to_string(info.join("exclude")).unwrap();
        assert!(body.contains("# user rules"));
        assert!(body.contains("*.log"));
        assert!(body.contains(".codegraph"));
        // The missing trailing newline before our append must not glue lines.
        assert!(body.contains("*.log\n.codegraph"));
    }

    #[tokio::test]
    async fn copy_index_skips_daemon_files_and_copies_db() {
        let td = tempfile::tempdir().unwrap();
        let mirror = td.path().join("mirror");
        let src_index = mirror.join(".codegraph");
        std::fs::create_dir_all(&src_index).unwrap();
        std::fs::write(src_index.join("codegraph.db"), b"DB").unwrap();
        std::fs::write(src_index.join("codegraph.db-wal"), b"WAL").unwrap();
        std::fs::write(src_index.join("codegraph.db-shm"), b"SHM").unwrap();
        std::fs::write(src_index.join("daemon.pid"), b"1234").unwrap();
        std::fs::write(src_index.join("daemon.sock"), b"").unwrap();

        let checkout = td.path().join("checkout");
        std::fs::create_dir_all(&checkout).unwrap();
        copy_index_into(&mirror, &checkout).await.unwrap();

        let dst = checkout.join(".codegraph");
        assert_eq!(std::fs::read(dst.join("codegraph.db")).unwrap(), b"DB");
        assert_eq!(std::fs::read(dst.join("codegraph.db-wal")).unwrap(), b"WAL");
        assert_eq!(std::fs::read(dst.join("codegraph.db-shm")).unwrap(), b"SHM");
        assert!(
            !dst.join("daemon.pid").exists(),
            "daemon.pid must be skipped"
        );
        assert!(
            !dst.join("daemon.sock").exists(),
            "daemon.sock must be skipped"
        );
        // The copy also registered the exclude entry.
        let body = std::fs::read_to_string(checkout.join(".git/info/exclude")).unwrap();
        assert!(body.contains(".codegraph"));
    }

    #[tokio::test]
    async fn copy_index_noop_when_no_db() {
        let td = tempfile::tempdir().unwrap();
        let mirror = td.path().join("mirror");
        std::fs::create_dir_all(mirror.join(".codegraph")).unwrap();
        // WAL present but no primary db → nothing to copy yet.
        std::fs::write(mirror.join(".codegraph/codegraph.db-wal"), b"WAL").unwrap();
        let checkout = td.path().join("checkout");
        std::fs::create_dir_all(&checkout).unwrap();
        copy_index_into(&mirror, &checkout).await.unwrap();
        assert!(
            !checkout.join(".codegraph").exists(),
            "no index dir should be created without a db"
        );
    }
}
