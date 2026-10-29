//! The base images this host has downloaded, and downloading the one this
//! build boots (port of Calina's image store).
//!
//! Each build names one image release ([`PinnedImage::this_build`]). The
//! release's `manifest.json` gives the exact size and sha256 `base.erofs` is
//! verified against as it arrives, so a truncated or corrupted download never
//! installs. A verified download is not trusted forever: the file can be
//! damaged on disk later, and [`ImageStore::verify`] hashes it again before
//! the first boot of a session.
//!
//! ```text
//! <root>/images/
//!   <version>/base.erofs          an installed image; appears only by renaming a verified download
//!   <version>/base.erofs.sha256   the sha256 it was verified against, lowercase hex
//!   .staging-*/                   a download in progress; the next install or prune removes leftovers
//! ```

use std::fs::{self, File};
use std::io::{self, Write};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::error::{Error, Result};

pub const BASE_IMAGE: &str = "base.erofs";
pub const MANIFEST: &str = "manifest.json";
/// A manifest is a few KB; anything past this is not one.
const MAX_MANIFEST: u64 = 1 << 20;
const SHA256_FILE: &str = "base.erofs.sha256";
const STAGING_PREFIX: &str = ".staging-";
const IMAGES_DIR: &str = "images";

/// Where image releases are downloaded from: one GitHub release per image on
/// the public repository, tagged `box-image-v<version>`, holding `base.erofs`
/// and `manifest.json`.
pub const RELEASES_URL: &str = "https://github.com/fwdai/fletch/releases/download";

/// The release manifest (`manifest.json`), written by `packaging/box-image`
/// next to `base.erofs`; a download is verified against its size and sha256.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageManifest {
    pub version: String,
    pub arch: String,
    pub file: String,
    pub size: u64,
    pub sha256: String,
}

/// The image this build boots: a version and the release directory holding
/// its `base.erofs` and `manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedImage {
    pub version: String,
    /// The release directory, without a trailing slash.
    pub base: String,
}

impl PinnedImage {
    /// The image release this build boots, `box-image-v<VERSION>`
    /// (packaging/box-image/README.md, "Publishing a release"). Bump it once
    /// the release exists; nothing else in the app changes with an image.
    pub const VERSION: &'static str = "0.1.0";

    /// This build's image, from `base_url` (the `box_image_url` setting) or
    /// the GitHub release.
    pub fn this_build(base_url: Option<&str>) -> PinnedImage {
        PinnedImage::new(Self::VERSION, base_url)
    }

    pub fn new(version: &str, base_url: Option<&str>) -> PinnedImage {
        let base = base_url.filter(|url| !url.trim().is_empty()).map_or_else(
            || release_url(version),
            |url| url.trim().trim_end_matches('/').to_owned(),
        );
        PinnedImage {
            version: version.to_owned(),
            base,
        }
    }

    /// The URL of `base.erofs`.
    pub fn url(&self) -> String {
        format!("{}/{BASE_IMAGE}", self.base)
    }

    /// The URL of `manifest.json`.
    pub fn manifest_url(&self) -> String {
        format!("{}/{MANIFEST}", self.base)
    }
}

pub fn release_url(version: &str) -> String {
    format!("{RELEASES_URL}/box-image-v{version}")
}

/// An image a `VmSpec` can boot from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledImage {
    pub dir: PathBuf,
    pub version: String,
    /// What `base.erofs` was verified against when installed, lowercase hex.
    pub sha256: String,
}

impl InstalledImage {
    pub fn path(&self) -> PathBuf {
        self.dir.join(BASE_IMAGE)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}

pub struct ImageStore {
    dir: PathBuf,
}

impl ImageStore {
    /// `root` is the box data dir; images go under `root/images`.
    pub fn new(root: &Path) -> ImageStore {
        ImageStore {
            dir: root.join(IMAGES_DIR),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Every installed image, oldest first.
    pub fn installed(&self) -> io::Result<Vec<InstalledImage>> {
        let mut images: Vec<InstalledImage> = self
            .entries()?
            .into_iter()
            .filter_map(|entry| {
                let name = entry.file_name().to_str()?.to_owned();
                (!name.starts_with('.')).then(|| image_at(entry.path(), name))?
            })
            .collect();
        images.sort_by_key(|image| version_key(&image.version));
        Ok(images)
    }

    /// The newest installed image, if any.
    pub fn current(&self) -> io::Result<Option<InstalledImage>> {
        Ok(self.installed()?.pop())
    }

    pub fn find(&self, version: &str) -> Option<InstalledImage> {
        image_at(self.dir.join(version), version.to_owned())
    }

    /// Hashes `image` again and compares it with the hash recorded when it was
    /// installed. On a mismatch the image is removed and the result is
    /// [`Error::Damaged`]. Reads the whole file: call it once per session, off
    /// the UI thread.
    pub fn verify(&self, image: &InstalledImage) -> Result<()> {
        if file_matches(&image.path(), None, &image.sha256)? {
            return Ok(());
        }
        fs::remove_dir_all(&image.dir)
            .map_err(Error::io(format!("remove {}", image.dir.display())))?;
        Err(Error::Damaged {
            version: image.version.clone(),
        })
    }

    /// Downloads `image` unless it is installed and matches its release
    /// manifest already. The manifest is fetched first and must describe
    /// `image.version`; the file is then hashed as it arrives and refused the
    /// moment it runs past the manifest's size. It becomes `<version>/` by one
    /// rename once it matched, so a failed or cancelled install leaves nothing
    /// behind. `progress` sees every chunk; returning `Break` cancels.
    pub async fn install(
        &self,
        image: &PinnedImage,
        mut progress: impl FnMut(Progress) -> ControlFlow<()>,
    ) -> Result<InstalledImage> {
        let manifest = fetch_manifest(image).await?;
        fs::create_dir_all(&self.dir)
            .map_err(Error::io(format!("create {}", self.dir.display())))?;
        let target = self.dir.join(&image.version);
        let installed = InstalledImage {
            dir: target.clone(),
            version: image.version.clone(),
            sha256: manifest.sha256.clone(),
        };
        if let Some(existing) = self.find(&image.version) {
            if existing.sha256 == manifest.sha256
                && file_matches(&existing.path(), Some(manifest.size), &manifest.sha256)?
            {
                return Ok(installed);
            }
        }

        self.remove_staging()?;
        let staging = self.dir.join(format!(
            "{STAGING_PREFIX}{}-{}",
            image.version,
            std::process::id()
        ));
        fs::create_dir(&staging).map_err(Error::io(format!("create {}", staging.display())))?;
        let result = async {
            download(
                &image.url(),
                &manifest,
                &staging.join(BASE_IMAGE),
                &mut progress,
            )
            .await?;
            write_synced(
                &staging.join(SHA256_FILE),
                format!("{}\n", manifest.sha256).as_bytes(),
            )?;
            // A same-version directory that did not match (a damaged copy) is replaced.
            if target.exists() {
                fs::remove_dir_all(&target)
                    .map_err(Error::io(format!("remove {}", target.display())))?;
            }
            fs::rename(&staging, &target)
                .and_then(|()| File::open(&self.dir)?.sync_all())
                .map_err(Error::io(format!("install {}", target.display())))
        }
        .await;
        if result.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        result?;
        Ok(installed)
    }

    /// Removes every installed version but `keep`, and leftovers of interrupted installs.
    pub fn prune(&self, keep: &str) -> io::Result<()> {
        self.remove_staging().map_err(io::Error::other)?;
        for image in self.installed()? {
            if image.version != keep {
                fs::remove_dir_all(&image.dir)?;
            }
        }
        Ok(())
    }

    fn remove_staging(&self) -> Result<()> {
        for entry in self.entries().map_err(Error::io("list the images"))? {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(STAGING_PREFIX)
            {
                fs::remove_dir_all(entry.path())
                    .map_err(Error::io(format!("remove {}", entry.path().display())))?;
            }
        }
        Ok(())
    }

    fn entries(&self) -> io::Result<Vec<fs::DirEntry>> {
        match fs::read_dir(&self.dir) {
            Ok(entries) => entries.collect(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }
}

/// `a.b.c` as numbers, so `0.10.0` sorts after `0.9.0`; anything else sorts by text.
fn version_key(version: &str) -> (Vec<u64>, String) {
    let parts: Option<Vec<u64>> = version.split('.').map(|p| p.parse().ok()).collect();
    (parts.unwrap_or_default(), version.to_owned())
}

/// The image in `dir`, if it has a non-empty `base.erofs` and a well-formed recorded hash.
fn image_at(dir: PathBuf, version: String) -> Option<InstalledImage> {
    let base = fs::metadata(dir.join(BASE_IMAGE)).ok()?;
    let recorded = fs::read_to_string(dir.join(SHA256_FILE)).ok()?;
    let sha256 = recorded.trim();
    (base.is_file() && base.len() > 0 && well_formed_sha256(sha256)).then(|| InstalledImage {
        dir,
        version,
        sha256: sha256.to_owned(),
    })
}

/// 64 lowercase hex digits.
fn well_formed_sha256(sha256: &str) -> bool {
    sha256.len() == 64
        && sha256
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether the file at `path` has `size` (checked first, without reading) and `sha256`.
pub fn file_matches(path: &Path, size: Option<u64>, sha256: &str) -> Result<bool> {
    let mut file = File::open(path).map_err(Error::io(format!("open {}", path.display())))?;
    if let Some(size) = size {
        let len = file
            .metadata()
            .map_err(Error::io(format!("stat {}", path.display())))?
            .len();
        if len != size {
            return Ok(false);
        }
    }
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher).map_err(Error::io(format!("read {}", path.display())))?;
    Ok(format!("{:x}", hasher.finalize()) == sha256.to_ascii_lowercase())
}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| Error::Other(e.to_string()))
}

/// The `manifest.json` of `image`'s release, checked to describe that version
/// with a non-zero size and a well-formed sha256 (returned lowercase). A
/// `file://` base is read from disk.
async fn fetch_manifest(image: &PinnedImage) -> Result<ImageManifest> {
    let url = image.manifest_url();
    let bad = |message: String| Error::Manifest {
        url: url.clone(),
        message,
    };
    let bytes = if let Some(local) = url.strip_prefix("file://") {
        fs::read(local).map_err(Error::io(format!("read {local}")))?
    } else {
        let http = |e: reqwest::Error| Error::Http {
            url: url.clone(),
            message: e.to_string(),
        };
        let mut response = client()?
            .get(&url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(http)?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(http)? {
            bytes.extend_from_slice(&chunk);
            if bytes.len() as u64 > MAX_MANIFEST {
                return Err(bad(format!("larger than {MAX_MANIFEST} bytes")));
            }
        }
        bytes
    };
    let manifest: ImageManifest = serde_json::from_slice(&bytes).map_err(|e| bad(e.to_string()))?;
    if manifest.version != image.version {
        return Err(bad(format!(
            "describes image {}, not the {} this build boots",
            manifest.version, image.version
        )));
    }
    if manifest.size == 0 {
        return Err(bad("size is 0".to_owned()));
    }
    let sha256 = manifest.sha256.to_ascii_lowercase();
    if !well_formed_sha256(&sha256) {
        return Err(bad("sha256 is not 64 hex digits".to_owned()));
    }
    Ok(ImageManifest { sha256, ..manifest })
}

/// Streams `url` to a new file at `path`, hashing as it goes, and checks the
/// size and sha256 `manifest` lists. On error the file is the caller's to
/// remove. A `file://` URL (the `box_image_url` setting pointed at a local
/// `packaging/box-image/out`) is cloned instead of fetched, then verified the
/// same way.
async fn download(
    url: &str,
    manifest: &ImageManifest,
    path: &Path,
    progress: &mut impl FnMut(Progress) -> ControlFlow<()>,
) -> Result<()> {
    let http = |e: reqwest::Error| Error::Http {
        url: url.to_owned(),
        message: e.to_string(),
    };
    if let Some(local) = url.strip_prefix("file://") {
        let source = Path::new(local);
        crate::disks::clone(source, path)
            .map_err(Error::io(format!("copy {}", source.display())))?;
        if progress(Progress {
            done: manifest.size,
            total: manifest.size,
        })
        .is_break()
        {
            return Err(Error::Cancelled);
        }
        let len = fs::metadata(path)
            .map_err(Error::io(format!("stat {}", path.display())))?
            .len();
        if len != manifest.size {
            return Err(Error::Size {
                expected: manifest.size,
                actual: len,
            });
        }
        if !file_matches(path, Some(manifest.size), &manifest.sha256)? {
            return Err(Error::Sha256);
        }
        return Ok(());
    }
    let mut response = client()?
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(http)?;
    if let Some(len) = response.content_length() {
        if len != manifest.size {
            return Err(Error::Size {
                expected: manifest.size,
                actual: len,
            });
        }
    }

    let mut out = tokio::fs::File::create_new(path)
        .await
        .map_err(Error::io(format!("create {}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut received = 0u64;
    let total = manifest.size;
    if progress(Progress { done: 0, total }).is_break() {
        return Err(Error::Cancelled);
    }
    while let Some(chunk) = response.chunk().await.map_err(http)? {
        received += chunk.len() as u64;
        if received > manifest.size {
            return Err(Error::Size {
                expected: manifest.size,
                actual: received,
            });
        }
        hasher.update(&chunk);
        out.write_all(&chunk)
            .await
            .map_err(Error::io(format!("write {}", path.display())))?;
        if progress(Progress {
            done: received,
            total,
        })
        .is_break()
        {
            return Err(Error::Cancelled);
        }
    }
    if received < manifest.size {
        return Err(Error::Size {
            expected: manifest.size,
            actual: received,
        });
    }
    if format!("{:x}", hasher.finalize()) != manifest.sha256 {
        return Err(Error::Sha256);
    }
    out.sync_all()
        .await
        .map_err(Error::io(format!("sync {}", path.display())))
}

fn write_synced(path: &Path, content: &[u8]) -> Result<()> {
    let mut file =
        File::create_new(path).map_err(Error::io(format!("create {}", path.display())))?;
    file.write_all(content)
        .and_then(|()| file.sync_all())
        .map_err(Error::io(format!("write {}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELLO_SHA256: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    /// A `manifest.json` as `packaging/box-image/build.sh` writes it.
    fn manifest_json(version: &str, size: u64, sha256: &str) -> String {
        serde_json::json!({
            "version": version, "arch": "aarch64", "file": BASE_IMAGE,
            "size": size, "sha256": sha256, "packages": ["bash=5.2"],
        })
        .to_string()
    }

    /// A local release directory: `base.erofs` holding `content` and a
    /// manifest describing it as `version` with `sha256`.
    fn local_release(version: &str, content: &[u8], sha256: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(BASE_IMAGE), content).unwrap();
        fs::write(
            dir.path().join(MANIFEST),
            manifest_json(version, content.len() as u64, sha256),
        )
        .unwrap();
        dir
    }

    #[test]
    fn a_pin_names_its_release_files() {
        let pinned = PinnedImage::new("0.1.0", None);
        assert_eq!(
            pinned.url(),
            "https://github.com/fwdai/fletch/releases/download/box-image-v0.1.0/base.erofs"
        );
        assert_eq!(
            pinned.manifest_url(),
            "https://github.com/fwdai/fletch/releases/download/box-image-v0.1.0/manifest.json"
        );
        let mirrored = PinnedImage::new("0.1.0", Some("https://mirror.test/images/"));
        assert_eq!(mirrored.url(), "https://mirror.test/images/base.erofs");
        assert_eq!(
            PinnedImage::this_build(Some("  ")),
            PinnedImage::this_build(None)
        );
    }

    #[test]
    fn sha256_verification_of_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        fs::write(&path, b"hello").unwrap();
        assert!(file_matches(&path, Some(5), HELLO_SHA256).unwrap());
        assert!(file_matches(&path, None, &HELLO_SHA256.to_ascii_uppercase()).unwrap());
        assert!(!file_matches(&path, Some(6), HELLO_SHA256).unwrap());
        assert!(!file_matches(&path, None, &"0".repeat(64)).unwrap());
    }

    #[test]
    fn installed_images_are_found_verified_and_pruned() {
        let root = tempfile::tempdir().unwrap();
        let store = ImageStore::new(root.path());
        assert_eq!(store.current().unwrap(), None);

        let put = |version: &str, sha: &str| {
            let dir = store.dir().join(version);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(BASE_IMAGE), b"hello").unwrap();
            fs::write(dir.join(SHA256_FILE), format!("{sha}\n")).unwrap();
        };
        put("0.9.0", HELLO_SHA256);
        put("0.10.0", HELLO_SHA256);
        put("0.2.0", &"0".repeat(64));
        fs::create_dir_all(store.dir().join(".staging-x-1")).unwrap();

        let current = store.current().unwrap().unwrap();
        assert_eq!(current.version, "0.10.0");
        assert_eq!(current.path(), store.dir().join("0.10.0").join(BASE_IMAGE));
        store.verify(&current).unwrap();
        assert!(store.find("0.10.0").is_some());

        let damaged = store.find("0.2.0").unwrap();
        assert!(matches!(store.verify(&damaged), Err(Error::Damaged { .. })));
        assert!(store.find("0.2.0").is_none());

        store.prune("0.9.0").unwrap();
        let left: Vec<_> = store
            .installed()
            .unwrap()
            .into_iter()
            .map(|i| i.version)
            .collect();
        assert_eq!(left, ["0.9.0"]);
        assert!(!store.dir().join(".staging-x-1").exists());
    }

    /// A manifest of another version, or a malformed one, is refused before
    /// anything is downloaded.
    #[tokio::test]
    async fn install_refuses_a_manifest_that_is_not_this_builds_image() {
        let root = tempfile::tempdir().unwrap();
        let store = ImageStore::new(root.path());
        let store = &store;
        let install = |version: &'static str, src: &tempfile::TempDir| {
            let base = format!("file://{}", src.path().display());
            let pin = PinnedImage::new(version, Some(&base));
            async move { store.install(&pin, |_| ControlFlow::Continue(())).await }
        };

        let other = local_release("3.1.0", b"hello", HELLO_SHA256);
        assert!(matches!(
            install("3.0.0", &other).await,
            Err(Error::Manifest { .. })
        ));
        let malformed = local_release("3.0.0", b"hello", "not a sha");
        assert!(matches!(
            install("3.0.0", &malformed).await,
            Err(Error::Manifest { .. })
        ));
        let missing = tempfile::tempdir().unwrap();
        assert!(matches!(
            install("3.0.0", &missing).await,
            Err(Error::Io { .. })
        ));
        assert!(store.installed().unwrap().is_empty());
    }

    /// This build names a real release on the public repository.
    #[test]
    fn this_build_names_a_release() {
        let pinned = PinnedImage::this_build(None);
        assert!(!pinned.version.is_empty());
        assert_eq!(pinned.base, release_url(&pinned.version));
        assert!(pinned.base.starts_with(RELEASES_URL));
    }

    /// A `file://` base URL installs by copying, verified like a download.
    #[tokio::test]
    async fn install_from_a_local_directory_copies_and_verifies() {
        let root = tempfile::tempdir().unwrap();
        let store = ImageStore::new(root.path());

        let src = local_release("3.0.0", b"hello", HELLO_SHA256);
        let base = format!("file://{}", src.path().display());
        let installed = store
            .install(&PinnedImage::new("3.0.0", Some(&base)), |_| {
                ControlFlow::Continue(())
            })
            .await
            .unwrap();
        assert_eq!(fs::read(installed.path()).unwrap(), b"hello");
        assert_eq!(installed.sha256, HELLO_SHA256);

        let wrong = local_release("3.1.0", b"hello", &"0".repeat(64));
        let base = format!("file://{}", wrong.path().display());
        assert!(matches!(
            store
                .install(&PinnedImage::new("3.1.0", Some(&base)), |_| {
                    ControlFlow::Continue(())
                })
                .await,
            Err(Error::Sha256)
        ));
        assert!(store.find("3.1.0").is_none());
    }

    #[tokio::test]
    async fn install_downloads_verifies_and_renames() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        // A release per `/<version>/<good|bad>/`: the manifest always describes
        // "hello", which `bad` then does not serve.
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 1024];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).into_owned();
                let path = request.split_whitespace().nth(1).unwrap_or("");
                let mut parts = path.trim_start_matches('/').splitn(3, '/');
                let version = parts.next().unwrap_or("").to_owned();
                let kind = parts.next().unwrap_or("");
                let file = parts.next().unwrap_or("");
                let body: Vec<u8> = if file == MANIFEST {
                    manifest_json(&version, 5, HELLO_SHA256).into_bytes()
                } else if kind == "bad" {
                    b"hellp".to_vec()
                } else {
                    b"hello".to_vec()
                };
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                s.write_all(head.as_bytes()).await.unwrap();
                s.write_all(&body).await.unwrap();
            }
        });
        let root = tempfile::tempdir().unwrap();
        let store = ImageStore::new(root.path());

        let bad = PinnedImage::new("1.0.0", Some(&format!("http://127.0.0.1:{port}/1.0.0/bad")));
        assert!(matches!(
            store.install(&bad, |_| ControlFlow::Continue(())).await,
            Err(Error::Sha256)
        ));
        assert!(store.installed().unwrap().is_empty());
        assert!(
            fs::read_dir(store.dir()).unwrap().next().is_none(),
            "staging left behind"
        );

        let good = PinnedImage::new(
            "1.0.0",
            Some(&format!("http://127.0.0.1:{port}/1.0.0/good")),
        );
        let mut seen = Vec::new();
        let installed = store
            .install(&good, |p| {
                seen.push(p.done);
                ControlFlow::Continue(())
            })
            .await
            .unwrap();
        assert_eq!(installed.version, "1.0.0");
        assert_eq!(fs::read(installed.path()).unwrap(), b"hello");
        assert_eq!(seen.last(), Some(&5));
        assert_eq!(store.current().unwrap(), Some(installed.clone()));
        store.verify(&installed).unwrap();

        let again = store
            .install(&good, |_| ControlFlow::Break(()))
            .await
            .unwrap();
        assert_eq!(
            again, installed,
            "an installed match is not downloaded again"
        );
        let cancelled = PinnedImage::new(
            "2.0.0",
            Some(&format!("http://127.0.0.1:{port}/2.0.0/good")),
        );
        assert!(matches!(
            store.install(&cancelled, |_| ControlFlow::Break(())).await,
            Err(Error::Cancelled)
        ));
        assert!(store.find("2.0.0").is_none());
    }
}
