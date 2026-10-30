//! Silent, background install of the pinned codegraph release bundle into
//! Fletch's tools dir. Follows `git_dist.rs`'s supply-chain posture: the
//! release tarball is fetched from a **pinned version tag** and verified
//! against a **pinned SHA-256** before anything is unpacked — no vendor
//! install script is ever downloaded or executed, so a compromised or edited
//! upstream `main` can't run code here. Bumping the pin means updating both
//! the tag and the digests below, deliberately.
//!
//! There is no UI: every failure is logged and swallowed by callers (indexing
//! simply stays off until a later retry). We never invoke the tool's own
//! `install`/`uninstall` subcommands — those rewrite `~/.claude.json` and
//! other global agent configs.
//!
//! Layout mirrors the vendor installer so the rest of the module is agnostic
//! to how the bundle got there:
//!
//! ```text
//! ~/.fletch/tools/codegraph/
//!   versions/<tag>/        the verified, extracted bundle (bin/, lib/)
//!   bin/codegraph          symlink -> versions/<tag>/bin/codegraph
//!   guest/<tag>/codegraph-linux-<arch>.tar.gz
//!                          the verified Linux bundle a box's tools disk is
//!                          built with (`ensure_guest_bundle`), kept packed
//! ```
//!
//! Both bundles are the same release: their `lib/` (the indexer, its
//! tree-sitter grammars) is byte-identical and only the bundled `node`
//! differs, so an index the host builds is one a box's server reads as its
//! own (`indexed_with_version` must match, hence the one pin).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use tokio::sync::Mutex;

use crate::error::{Error, Result};

/// Pinned codegraph release tag. Bump deliberately when adopting a newer
/// bundle — and re-pin the per-target digests in [`PINNED`] with it.
pub const CODEGRAPH_VERSION: &str = "v1.4.1";

/// Ceiling on a single install run — the bundle is tens of MB and finishes
/// well under a minute, so this only stops a hung download from holding the
/// install lock forever.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// The pinned SHA-256 of each release asset of [`CODEGRAPH_VERSION`] we use,
/// by target. Digests are of the `.tar.gz` exactly as published on the GitHub
/// release (and match the digests the release itself lists).
const PINNED: &[(&str, &str)] = &[
    (
        "darwin-arm64",
        "4a679ae5a5cb9fff900dd59bb786da6a581b7f68f4cf713bdedd137e347d34dc",
    ),
    (
        "darwin-x64",
        "436f96943cfd926ea6d0a8454f18833d21254d5fd9b3d224317b1426132def95",
    ),
    (
        "linux-arm64",
        "0d62c5eb2722f8d19d20f7a1bd974445e18d5294cb59be116a0c3d55ce87591f",
    ),
    (
        "linux-x64",
        "fb585ff5018d6faaa46d282b61f4f689bc7967ed8a1b467a5c556dd7ced9b542",
    ),
];

/// The bundle this host runs (the mirror's indexer, a seatbelt agent's
/// server): macOS only, the one place those run.
fn host_target() -> Option<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("darwin-arm64")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("darwin-x64")
    } else {
        None
    }
}

/// The bundle a box's guest runs: Linux on this host's architecture, which a
/// box never emulates.
fn guest_target() -> Option<&'static str> {
    if cfg!(target_arch = "aarch64") {
        Some("linux-arm64")
    } else if cfg!(target_arch = "x86_64") {
        Some("linux-x64")
    } else {
        None
    }
}

/// Release asset URL and its pinned SHA-256 for `target`, `None` for a target
/// we don't pin.
fn dist_asset(target: &str) -> Option<(String, &'static str)> {
    let (_, sha) = PINNED.iter().find(|(t, _)| *t == target)?;
    Some((
        format!(
            "https://github.com/colbymchenry/codegraph/releases/download/{CODEGRAPH_VERSION}/codegraph-{target}.tar.gz"
        ),
        sha,
    ))
}

/// Serializes installs process-wide: startup, the toggle, and concurrent spawns
/// can all call `ensure_installed`, and two installers racing over the same dir
/// would corrupt the bundle.
fn install_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// The pinned version's bundle dir: `<install_dir>/versions/<tag>`.
fn version_dir(install_dir: &Path) -> PathBuf {
    install_dir.join("versions").join(CODEGRAPH_VERSION)
}

/// True when the pinned bundle is fully installed: the versioned bundle has
/// its launcher and the `bin/codegraph` symlink resolves to an existing file
/// (`exists()` follows symlinks).
fn install_complete(install_dir: &Path, bin: &Path) -> bool {
    version_dir(install_dir)
        .join("bin")
        .join("codegraph")
        .is_file()
        && bin.exists()
}

/// Ensure the pinned codegraph bundle is installed under `~/.fletch/tools/`
/// and return the absolute path to its binary. Returns immediately when the
/// pinned version is already in place; otherwise downloads the release
/// tarball, verifies its SHA-256 against the pin, extracts it, and points the
/// `bin/codegraph` symlink at it. Serialized so concurrent callers don't race
/// the same install dir.
pub async fn ensure_installed() -> Result<PathBuf> {
    let bin = super::bin_path()?;
    let install_dir = super::install_dir()?;
    if install_complete(&install_dir, &bin) {
        return Ok(bin);
    }

    let _guard = install_lock().lock().await;
    // Re-check under the lock: a racing caller may have finished the install
    // while we waited.
    if install_complete(&install_dir, &bin) {
        return Ok(bin);
    }

    let (url, sha) = host_target()
        .and_then(dist_asset)
        .ok_or_else(|| Error::Other("codegraph: no pinned bundle for this platform".into()))?;
    tokio::fs::create_dir_all(&install_dir)
        .await
        .map_err(|e| Error::Other(format!("create codegraph install dir: {e}")))?;

    tracing::info!(version = CODEGRAPH_VERSION, "installing codegraph bundle");
    let dest = version_dir(&install_dir);
    tokio::time::timeout(INSTALL_TIMEOUT, async {
        // No progress consumer: this install is silent (see the module docs).
        let tarball =
            crate::download::download_verified(&url, sha, &install_dir, |_, _| {}).await?;
        // Blocking CPU/fs work off the async runtime.
        let dest = dest.clone();
        tokio::task::spawn_blocking(move || extract_bundle(&tarball, &dest))
            .await
            .map_err(|e| Error::Other(format!("codegraph extract task: {e}")))?
    })
    .await
    .map_err(|_| Error::Other("codegraph install timed out".into()))??;

    link_launcher(&dest, &bin).await?;
    if !install_complete(&install_dir, &bin) {
        return Err(Error::Other(format!(
            "codegraph install finished but no binary at {}",
            bin.display()
        )));
    }
    prune_old_versions(&install_dir, &dest).await;
    tracing::info!(path = %bin.display(), "codegraph installed");
    Ok(bin)
}

/// Ensure the pinned Linux bundle a box's tools disk is built with is on this
/// host, verified, and return its path. Kept packed: the disk build pushes the
/// tarball into its build box as is (`fletch_box::tools::build`), so nothing in
/// a guest ever downloads it. Same posture as [`ensure_installed`] — pinned
/// tag, pinned digest, checked while it streams — and serialized with it, since
/// both download into this install dir.
pub async fn ensure_guest_bundle() -> Result<PathBuf> {
    let target = guest_target()
        .ok_or_else(|| Error::Other("codegraph: no guest bundle for this architecture".into()))?;
    let dir = super::install_dir()?.join("guest").join(CODEGRAPH_VERSION);
    let bundle = dir.join(format!("codegraph-{target}.tar.gz"));
    // Only ever renamed into place after its digest matched.
    if bundle.is_file() {
        return Ok(bundle);
    }

    let _guard = install_lock().lock().await;
    if bundle.is_file() {
        return Ok(bundle);
    }
    let (url, sha) = dist_asset(target)
        .ok_or_else(|| Error::Other(format!("codegraph: no pinned bundle for {target}")))?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| Error::Other(format!("create codegraph guest dir: {e}")))?;
    // A download cut off by a caller's timeout (a tools disk build waits only
    // so long) leaves its temp file; this process's next one reuses the name,
    // a later process's does not.
    clear_stale_tmp(&dir).await;

    tracing::info!(
        version = CODEGRAPH_VERSION,
        target,
        "downloading codegraph guest bundle"
    );
    let tmp = tokio::time::timeout(
        INSTALL_TIMEOUT,
        crate::download::download_verified(&url, sha, &dir, |_, _| {}),
    )
    .await
    .map_err(|_| Error::Other("codegraph guest bundle download timed out".into()))??;
    tokio::fs::rename(&tmp, &bundle)
        .await
        .map_err(|e| Error::Other(format!("install codegraph guest bundle: {e}")))?;
    if let Some(guest_root) = dir.parent() {
        prune_old_versions_in(guest_root, &dir).await;
    }
    tracing::info!(path = %bundle.display(), "codegraph guest bundle ready");
    Ok(bundle)
}

/// A download temp file untouched this long has no live request behind it:
/// [`crate::download::READ_TIMEOUT`] tears a stalled one down well before.
const STALE_TMP_AFTER: Duration = Duration::from_secs(10 * 60);
const _: () = assert!(STALE_TMP_AFTER.as_secs() >= 5 * crate::download::READ_TIMEOUT.as_secs());

/// Remove [`crate::download`] temp files in `dir` that are stale by age, not
/// by pid: another app instance sharing the tools dir (a dev build beside a
/// release) may be mid-download here.
async fn clear_stale_tmp(dir: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if !crate::download::is_tmp_name(&entry.file_name().to_string_lossy()) {
            continue;
        }
        let idle = entry
            .metadata()
            .await
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok());
        if idle.is_some_and(|d| d >= STALE_TMP_AFTER) {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
}

/// Unpack `tarball` and move the bundle into `dest`. Extraction goes to a
/// sibling staging dir first so `dest` only ever appears complete; the tarball
/// and staging dir are always removed. Release archives wrap their content in
/// a single `codegraph-<target>/` dir, but a flat layout is tolerated too.
fn extract_bundle(tarball: &Path, dest: &Path) -> Result<()> {
    // Appended, not `with_extension` — the version dir name contains dots
    // (`v1.4.1`), which with_extension would truncate at.
    let mut staging = dest.as_os_str().to_owned();
    staging.push(".partial");
    let staging = PathBuf::from(staging);
    let result = (|| {
        if staging.exists() {
            std::fs::remove_dir_all(&staging)
                .map_err(|e| Error::Other(format!("clear staging: {e}")))?;
        }
        std::fs::create_dir_all(&staging)
            .map_err(|e| Error::Other(format!("create staging: {e}")))?;

        let file =
            std::fs::File::open(tarball).map_err(|e| Error::Other(format!("open tarball: {e}")))?;
        let gz = flate2::read::GzDecoder::new(std::io::BufReader::new(file));
        tar::Archive::new(gz)
            .unpack(&staging)
            .map_err(|e| Error::Other(format!("unpack codegraph bundle: {e}")))?;

        let bundle_root = locate_bundle_root(&staging).ok_or_else(|| {
            Error::Other("codegraph archive does not contain bin/codegraph".into())
        })?;

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::Other(format!("create versions dir: {e}")))?;
        }
        if dest.exists() {
            std::fs::remove_dir_all(dest).map_err(|e| Error::Other(format!("clear dest: {e}")))?;
        }
        std::fs::rename(&bundle_root, dest)
            .map_err(|e| Error::Other(format!("install codegraph bundle: {e}")))?;
        Ok(())
    })();

    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_file(tarball);
    result
}

/// The dir holding `bin/codegraph` — the unpack root itself, or its single
/// subdirectory (the archive's `codegraph-<target>/` wrapper).
fn locate_bundle_root(unpacked: &Path) -> Option<PathBuf> {
    if unpacked.join("bin").join("codegraph").is_file() {
        return Some(unpacked.to_path_buf());
    }
    let dirs: Vec<_> = std::fs::read_dir(unpacked)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    match dirs.as_slice() {
        [only] if only.join("bin").join("codegraph").is_file() => Some(only.clone()),
        _ => None,
    }
}

/// Point `<install_dir>/bin/codegraph` at the freshly installed bundle's
/// launcher, replacing whatever the symlink pointed at before.
async fn link_launcher(bundle_dir: &Path, bin: &Path) -> Result<()> {
    let target = bundle_dir.join("bin").join("codegraph");
    if let Some(parent) = bin.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| Error::Other(format!("create codegraph bin dir: {e}")))?;
    }
    // Remove-then-link: `symlink` refuses to overwrite. `remove_file` also
    // clears a dangling symlink (`exists()` would report false for those).
    match tokio::fs::remove_file(bin).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::Other(format!("replace codegraph launcher: {e}"))),
    }
    tokio::fs::symlink(&target, bin)
        .await
        .map_err(|e| Error::Other(format!("link codegraph launcher: {e}")))
}

/// Drop bundles other than the one just installed so upgrades don't accumulate
/// ~50 MB dirs forever. Best-effort; safe even if an old daemon still runs an
/// older bundle (POSIX keeps the inodes alive until that process exits).
async fn prune_old_versions(install_dir: &Path, keep: &Path) {
    prune_old_versions_in(&install_dir.join("versions"), keep).await;
}

/// Remove every directory under `versions` but `keep`.
async fn prune_old_versions_in(versions: &Path, keep: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(versions).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path != keep && path.is_dir() {
            if let Err(e) = tokio::fs::remove_dir_all(&path).await {
                tracing::debug!(path = %path.display(), error = %e, "prune old codegraph bundle");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build `<root>/<wrapper?>/bin/codegraph` for layout tests.
    fn make_bundle(root: &Path, wrapper: Option<&str>) {
        let base = match wrapper {
            Some(w) => root.join(w),
            None => root.to_path_buf(),
        };
        std::fs::create_dir_all(base.join("bin")).unwrap();
        std::fs::write(base.join("bin").join("codegraph"), b"#!/bin/sh\n").unwrap();
    }

    #[test]
    fn locate_bundle_root_accepts_flat_layout() {
        let td = tempfile::tempdir().unwrap();
        make_bundle(td.path(), None);
        assert_eq!(locate_bundle_root(td.path()), Some(td.path().to_path_buf()));
    }

    #[test]
    fn locate_bundle_root_accepts_single_wrapper_dir() {
        let td = tempfile::tempdir().unwrap();
        make_bundle(td.path(), Some("codegraph-darwin-arm64"));
        assert_eq!(
            locate_bundle_root(td.path()),
            Some(td.path().join("codegraph-darwin-arm64"))
        );
    }

    #[test]
    fn locate_bundle_root_rejects_missing_launcher() {
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(td.path().join("lib")).unwrap();
        assert_eq!(locate_bundle_root(td.path()), None);
    }

    #[test]
    fn dist_asset_is_pinned_on_macos() {
        // On the platforms we build for, the asset must carry the pinned tag
        // in its URL and a full-length digest.
        if let Some((url, sha)) = host_target().and_then(dist_asset) {
            assert!(url.contains(CODEGRAPH_VERSION));
            assert_eq!(sha.len(), 64);
        }
    }

    #[test]
    fn every_target_is_pinned_to_one_release() {
        for (target, sha) in PINNED {
            let (url, pinned) = dist_asset(target).unwrap();
            assert!(
                url.ends_with(&format!("/{CODEGRAPH_VERSION}/codegraph-{target}.tar.gz")),
                "{url}"
            );
            assert_eq!(pinned, *sha);
            assert_eq!(sha.len(), 64);
            assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));
        }
        assert!(dist_asset("win32-x64").is_none());
    }

    #[test]
    fn a_box_guest_has_a_pinned_bundle_on_every_box_host() {
        // A box runs Linux on the host's own architecture; both are pinned.
        let guest = guest_target().unwrap();
        assert!(guest.starts_with("linux-"), "{guest}");
        assert!(dist_asset(guest).is_some());
        if let Some(host) = host_target() {
            assert_ne!(host, guest, "the guest never runs the host's bundle");
        }
    }

    #[tokio::test]
    async fn link_launcher_replaces_existing_symlink() {
        let td = tempfile::tempdir().unwrap();
        let old = td.path().join("old");
        let new = td.path().join("new");
        make_bundle(&old, None);
        make_bundle(&new, None);
        let bin = td.path().join("bin").join("codegraph");
        link_launcher(&old, &bin).await.unwrap();
        link_launcher(&new, &bin).await.unwrap();
        assert_eq!(
            tokio::fs::read_link(&bin).await.unwrap(),
            new.join("bin").join("codegraph")
        );
    }
}
