//! Data disks: a sparse file the guest formats on first boot, an APFS clone of a
//! template, and the `dirty` marker that says a VM may not have unmounted cleanly.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

/// The default workspace disk size. Sparse, so it only takes what is written,
/// and nothing the guest writes can grow it past this.
pub const DEFAULT_DATA_DISK_BYTES: u64 = 8 << 30;

/// Creates an empty sparse `path` of `bytes`. Fails if it exists.
pub fn create_sparse(path: &Path, bytes: u64) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = File::create_new(path)?;
    // Extending an empty file leaves a hole: no blocks are allocated until written.
    file.set_len(bytes)?;
    file.sync_all()
}

/// Clones `from` to `to`: `clonefile(2)` on macOS (instant, shares blocks on
/// APFS), a full copy elsewhere or when the filesystem refuses, with a warning
/// because a template clone is expected to be free.
pub fn clone(from: &Path, to: &Path) -> io::Result<()> {
    if let Err(e) = clone_sharing_blocks(from, to) {
        tracing::warn!(from = %from.display(), to = %to.display(), error = %e, "clonefile unavailable; copying");
        std::fs::copy(from, to)?;
    }
    Ok(())
}

/// Clones `from` to a `to` that does not exist yet, the two sharing every
/// block until one is written. Fails where the filesystem cannot do that,
/// rather than copy: for a caller that clones because a copy costs too much.
pub fn clone_sharing_blocks(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    clonefile(from, to)
}

#[cfg(target_os = "macos")]
fn clonefile(from: &Path, to: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let from = CString::new(from.as_os_str().as_bytes())?;
    let to = CString::new(to.as_os_str().as_bytes())?;
    // SAFETY: two valid NUL-terminated paths; no flags.
    if unsafe { libc::clonefile(from.as_ptr(), to.as_ptr(), 0) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "macos"))]
fn clonefile(_from: &Path, _to: &Path) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

/// Exchanges what is at `a` and `b`, both existing, in one step: there is no
/// moment at which either path is missing. Fails where the filesystem cannot
/// do that, rather than rename twice.
#[cfg(target_os = "macos")]
pub fn exchange(a: &Path, b: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let a = CString::new(a.as_os_str().as_bytes())?;
    let b = CString::new(b.as_os_str().as_bytes())?;
    // SAFETY: two valid NUL-terminated paths; a flag the call defines.
    if unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_SWAP) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "macos"))]
pub fn exchange(_a: &Path, _b: &Path) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

/// Pads `script` to a 512-byte multiple and writes it as a raw disk, so a guest
/// can run it straight off the device (`/bin/sh /dev/vdc`). Scripts never
/// travel in libkrun's argv, which re-splits quoted commands. Test and dev use.
pub fn write_script_disk(script: &[u8], disk: &Path) -> io::Result<()> {
    let mut bytes = script.to_vec();
    bytes.resize(bytes.len().div_ceil(512) * 512, b'\n');
    std::fs::write(disk, bytes)
}

/// Present while a VM runs on the box's disk and removed after a clean stop, so
/// a marker found at start means the last VM was killed: init's fsck will run,
/// and the host logs it. A signal, not an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirty(PathBuf);

impl Dirty {
    pub const FILE: &'static str = "dirty";

    pub fn in_dir(box_dir: &Path) -> Dirty {
        Dirty(box_dir.join(Self::FILE))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn is_set(&self) -> bool {
        self.0.exists()
    }

    /// Marks the disk in use. Returns whether it was already marked.
    pub fn set(&self) -> io::Result<bool> {
        let was_set = self.is_set();
        if !was_set {
            File::create(&self.0)?;
        }
        Ok(was_set)
    }

    pub fn clear(&self) -> io::Result<()> {
        crate::fsutil::remove_if_exists(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_disk_is_created_once() {
        let dir = tempfile::tempdir().unwrap();
        let disk = dir.path().join("ws/data.img");
        create_sparse(&disk, 1 << 30).unwrap();
        let meta = std::fs::metadata(&disk).unwrap();
        assert_eq!(meta.len(), 1 << 30);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert!(meta.blocks() * 512 < 1 << 20, "should take almost no space");
        }
        assert_eq!(
            create_sparse(&disk, 1).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
    }

    #[test]
    fn clone_reproduces_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("template.img");
        std::fs::write(&from, b"template bytes").unwrap();
        let to = dir.path().join("sub/data.img");
        clone(&from, &to).unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"template bytes");
        std::fs::write(&to, b"changed").unwrap();
        assert_eq!(std::fs::read(&from).unwrap(), b"template bytes");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_block_sharing_clone_never_replaces_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("a.img");
        let to = dir.path().join("b.img");
        std::fs::write(&from, b"a").unwrap();
        std::fs::write(&to, b"b").unwrap();
        assert!(clone_sharing_blocks(&from, &to).is_err());
        assert_eq!(std::fs::read(&to).unwrap(), b"b");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_exchange_swaps_two_directories_and_needs_both() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::create_dir(&a).unwrap();
        std::fs::write(a.join("was-in-a"), b"").unwrap();
        assert!(exchange(&a, &b).is_err());

        std::fs::create_dir(&b).unwrap();
        exchange(&a, &b).unwrap();
        assert!(b.join("was-in-a").exists());
        assert!(!a.join("was-in-a").exists());
    }

    #[test]
    fn script_disk_is_sector_aligned() {
        let dir = tempfile::tempdir().unwrap();
        let disk = dir.path().join("s.img");
        write_script_disk(b"#!/bin/sh\necho hi\n", &disk).unwrap();
        let bytes = std::fs::read(&disk).unwrap();
        assert_eq!(bytes.len(), 512);
        assert!(bytes.starts_with(b"#!/bin/sh\necho hi\n"));
        write_script_disk(&[b'x'; 513], &disk).unwrap();
        assert_eq!(std::fs::metadata(&disk).unwrap().len(), 1024);
    }

    #[test]
    fn dirty_marker_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let dirty = Dirty::in_dir(dir.path());
        assert!(!dirty.is_set());
        assert!(!dirty.set().unwrap());
        assert!(dirty.set().unwrap());
        assert!(dirty.is_set());
        dirty.clear().unwrap();
        dirty.clear().unwrap();
        assert!(!dirty.is_set());
    }
}
