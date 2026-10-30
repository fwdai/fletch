//! Small file operations the image store and tools disks share: durable
//! writes and renames, and removes that tolerate a file already gone.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

/// Write `content` to `path` (created or truncated) and fsync it.
pub fn write_synced(path: &Path, content: &[u8]) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(content)?;
    file.sync_all()
}

/// Write `content` to `path` atomically: [`write_synced`] to `staging`, then
/// a rename into place that is synced too ([`sync_dir`]). A failure removes
/// `staging` and leaves `path` as it was, or (after the rename) whole.
pub fn write_atomic(path: &Path, staging: &Path, content: &[u8]) -> io::Result<()> {
    let written = write_synced(staging, content).and_then(|()| fs::rename(staging, path));
    if written.is_err() {
        let _ = fs::remove_file(staging);
    }
    written?;
    sync_dir(path)
}

/// Sync the directory holding `path`, so a rename into it survives a power
/// loss (on macOS `sync_all` is `F_FULLFSYNC`, which takes a directory too).
pub fn sync_dir(path: &Path) -> io::Result<()> {
    let dir = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    File::open(dir)?.sync_all()
}

/// Remove the file at `path`; one already gone is fine.
pub fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        removed => removed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_atomic_write_replaces_the_file_and_leaves_no_staging() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("marker");
        let staging = dir.path().join("marker.building");
        std::fs::write(&path, b"old").unwrap();
        std::fs::write(&staging, b"left by a crash").unwrap();
        write_atomic(&path, &staging, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert!(!staging.exists());
        // Into a directory that is not there: nothing written, nothing left.
        let absent = dir.path().join("absent/marker");
        assert!(write_atomic(&absent, &dir.path().join("absent/s"), b"x").is_err());
        assert!(!absent.exists());
    }

    #[test]
    fn removing_a_missing_file_is_fine() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        remove_if_exists(&path).unwrap();
        std::fs::write(&path, b"").unwrap();
        remove_if_exists(&path).unwrap();
        assert!(!path.exists());
        // A directory is not a file: that error is reported.
        assert!(remove_if_exists(dir.path()).is_err());
    }
}
