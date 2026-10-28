//! The base images this host has downloaded, and downloading the one this
//! build pins (port of Calina's image store).
//!
//! Each build names one image release ([`PinnedImage::this_build`]) with its
//! exact size and sha256, so a download over HTTPS is trusted exactly as far as
//! the app binary is. A verified download is not trusted forever: the file can
//! be damaged on disk later, and [`ImageStore::verify`] hashes it again before
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
const SHA256_FILE: &str = "base.erofs.sha256";
const STAGING_PREFIX: &str = ".staging-";
const IMAGES_DIR: &str = "images";

/// Where image releases are downloaded from: one GitHub release per image,
/// tagged `box-image-v<version>`, holding `base.erofs` and `manifest.json`.
/// The repository is private, so the app cannot fetch release assets from
/// github.com without credentials; the website that already serves the
/// updater's `latest.json` and artifacts proxies `/releases/download/<tag>/<asset>`
/// as well (docs/RELEASING.md).
pub const RELEASES_URL: &str = "https://fletch.sh/releases/download";

/// The release manifest (`manifest.json`), the shape `packaging/box-image`
/// writes and the pin below is copied from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageManifest {
    pub version: String,
    pub arch: String,
    pub file: String,
    pub size: u64,
    pub sha256: String,
}

/// One image as this build would download it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedImage {
    pub version: String,
    /// The URL of `base.erofs` itself.
    pub url: String,
    pub size: u64,
    /// Lowercase hex; empty when this build has no published image.
    pub sha256: String,
}

impl PinnedImage {
    // The `manifest.json` of the GitHub release `box-image-v<VERSION>`, written
    // here by `scripts/box-image-pin.sh` after the `box-image` workflow
    // publishes one (packaging/box-image/README.md, "Publishing a release").
    // Constants rather than an `include_str!` of the manifest because `out/`
    // is a build artifact.
    pub const VERSION: &'static str = "0.1.0";
    pub const SIZE: u64 = 848_310_272;
    pub const SHA256: &'static str =
        "0537e1814736e6788de0de79ee4f0a02ce280a4158a91cba156c95dacb185140";

    /// The image this build pins, from `base_url` (the `box_image_url`
    /// setting) or the GitHub release.
    pub fn this_build(base_url: Option<&str>) -> PinnedImage {
        PinnedImage::pinned(Self::VERSION, Self::SIZE, Self::SHA256, base_url)
    }

    /// A manifest's image, for a dev build pointing at a local or staging release.
    pub fn from_manifest(manifest: &ImageManifest, base_url: Option<&str>) -> PinnedImage {
        PinnedImage::pinned(&manifest.version, manifest.size, &manifest.sha256, base_url)
    }

    fn pinned(version: &str, size: u64, sha256: &str, base_url: Option<&str>) -> PinnedImage {
        let base = base_url.filter(|url| !url.trim().is_empty()).map_or_else(
            || release_url(version),
            |url| url.trim_end_matches('/').to_owned(),
        );
        PinnedImage {
            version: version.to_owned(),
            url: format!("{base}/{BASE_IMAGE}"),
            size,
            sha256: sha256.to_ascii_lowercase(),
        }
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

    /// Hashes `image` again and compares it with `pinned` when that pins the
    /// same version, else with the hash recorded when it was installed. On a
    /// mismatch the image is removed and the result is [`Error::Damaged`].
    /// Reads the whole file: call it once per session, off the UI thread.
    pub fn verify(&self, image: &InstalledImage, pinned: Option<&PinnedImage>) -> Result<()> {
        let (size, sha256) = match pinned {
            Some(pin) if pin.version == image.version && !pin.sha256.is_empty() => {
                (Some(pin.size), pin.sha256.as_str())
            }
            _ => (None, image.sha256.as_str()),
        };
        if file_matches(&image.path(), size, sha256)? {
            return Ok(());
        }
        fs::remove_dir_all(&image.dir)
            .map_err(Error::io(format!("remove {}", image.dir.display())))?;
        Err(Error::Damaged {
            version: image.version.clone(),
        })
    }

    /// Downloads `image` unless it is installed and matches its pin already.
    /// The file is hashed as it arrives and refused the moment it runs past its
    /// size; it becomes `<version>/` by one rename once it matched, so a failed
    /// or cancelled install leaves nothing behind. `progress` sees every chunk;
    /// returning `Break` cancels.
    pub async fn install(
        &self,
        image: &PinnedImage,
        mut progress: impl FnMut(Progress) -> ControlFlow<()>,
    ) -> Result<InstalledImage> {
        if image.sha256.is_empty() {
            return Err(Error::NotPublished);
        }
        fs::create_dir_all(&self.dir)
            .map_err(Error::io(format!("create {}", self.dir.display())))?;
        let target = self.dir.join(&image.version);
        let installed = InstalledImage {
            dir: target.clone(),
            version: image.version.clone(),
            sha256: image.sha256.clone(),
        };
        if let Some(existing) = self.find(&image.version) {
            if existing.sha256 == image.sha256
                && file_matches(&existing.path(), Some(image.size), &image.sha256)?
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
            download(image, &staging.join(BASE_IMAGE), &mut progress).await?;
            write_synced(
                &staging.join(SHA256_FILE),
                format!("{}\n", image.sha256).as_bytes(),
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
    let well_formed = sha256.len() == 64
        && sha256
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    (base.is_file() && base.len() > 0 && well_formed).then(|| InstalledImage {
        dir,
        version,
        sha256: sha256.to_owned(),
    })
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

/// Streams `image.url` to a new file at `path`, hashing as it goes, and checks
/// size and sha256. On error the file is the caller's to remove. A `file://`
/// URL (the `box_image_url` setting pointed at a local `packaging/box-image/out`
/// before any release exists) is cloned instead of fetched, then verified the
/// same way.
async fn download(
    image: &PinnedImage,
    path: &Path,
    progress: &mut impl FnMut(Progress) -> ControlFlow<()>,
) -> Result<()> {
    let http = |message: String| Error::Http {
        url: image.url.clone(),
        message,
    };
    if let Some(local) = image.url.strip_prefix("file://") {
        let source = Path::new(local);
        crate::disks::clone(source, path)
            .map_err(Error::io(format!("copy {}", source.display())))?;
        if progress(Progress {
            done: image.size,
            total: image.size,
        })
        .is_break()
        {
            return Err(Error::Cancelled);
        }
        let len = fs::metadata(path)
            .map_err(Error::io(format!("stat {}", path.display())))?
            .len();
        if len != image.size {
            return Err(Error::Size {
                expected: image.size,
                actual: len,
            });
        }
        if !file_matches(path, Some(image.size), &image.sha256)? {
            return Err(Error::Sha256);
        }
        return Ok(());
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| http(e.to_string()))?;
    let mut response = client
        .get(&image.url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| http(e.to_string()))?;
    if let Some(len) = response.content_length() {
        if len != image.size {
            return Err(Error::Size {
                expected: image.size,
                actual: len,
            });
        }
    }

    let mut out = tokio::fs::File::create_new(path)
        .await
        .map_err(Error::io(format!("create {}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut received = 0u64;
    let total = image.size;
    if progress(Progress { done: 0, total }).is_break() {
        return Err(Error::Cancelled);
    }
    while let Some(chunk) = response.chunk().await.map_err(|e| http(e.to_string()))? {
        received += chunk.len() as u64;
        if received > image.size {
            return Err(Error::Size {
                expected: image.size,
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
    if received < image.size {
        return Err(Error::Size {
            expected: image.size,
            actual: received,
        });
    }
    if format!("{:x}", hasher.finalize()) != image.sha256 {
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

    #[test]
    fn manifest_parses_and_pins() {
        let manifest: ImageManifest = serde_json::from_str(
            r#"{"version":"0.1.0","arch":"aarch64","file":"base.erofs","size":123,"sha256":"ABC"}"#,
        )
        .unwrap();
        assert_eq!(manifest.arch, "aarch64");
        let pinned = PinnedImage::from_manifest(&manifest, None);
        assert_eq!(
            pinned.url,
            "https://fletch.sh/releases/download/box-image-v0.1.0/base.erofs"
        );
        assert_eq!(pinned.sha256, "abc");
        let mirrored = PinnedImage::from_manifest(&manifest, Some("https://mirror.test/images/"));
        assert_eq!(mirrored.url, "https://mirror.test/images/base.erofs");
        assert_eq!(
            PinnedImage::this_build(Some("  ")).url,
            PinnedImage::this_build(None).url
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
        store.verify(&current, None).unwrap();
        let pinned = PinnedImage::pinned("0.10.0", 5, HELLO_SHA256, None);
        store.verify(&current, Some(&pinned)).unwrap();
        let wrong_size = PinnedImage::pinned("0.10.0", 6, HELLO_SHA256, None);
        assert!(matches!(
            store.verify(&current, Some(&wrong_size)),
            Err(Error::Damaged { .. })
        ));
        assert!(store.find("0.10.0").is_none());

        let damaged = store.find("0.2.0").unwrap();
        assert!(matches!(
            store.verify(&damaged, None),
            Err(Error::Damaged { .. })
        ));

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

    #[tokio::test]
    async fn install_refuses_an_unpublished_pin() {
        let root = tempfile::tempdir().unwrap();
        let store = ImageStore::new(root.path());
        let result = store
            .install(&PinnedImage::pinned("9.9.9", 0, "", None), |_| {
                ControlFlow::Continue(())
            })
            .await;
        assert!(matches!(result, Err(Error::NotPublished)));
    }

    /// This build's pin is a real one: a version, a size and a 64-hex sha.
    #[test]
    fn this_build_pins_a_published_shape() {
        let pinned = PinnedImage::this_build(None);
        assert!(!pinned.version.is_empty());
        assert!(pinned.size > 0);
        assert_eq!(pinned.sha256.len(), 64);
        assert!(pinned.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    /// A `file://` base URL installs by copying, verified like a download.
    #[tokio::test]
    async fn install_from_a_local_directory_copies_and_verifies() {
        let src = tempfile::tempdir().unwrap();
        fs::write(src.path().join(BASE_IMAGE), b"hello").unwrap();
        let base = format!("file://{}", src.path().display());
        let root = tempfile::tempdir().unwrap();
        let store = ImageStore::new(root.path());

        let installed = store
            .install(
                &PinnedImage::pinned("3.0.0", 5, HELLO_SHA256, Some(&base)),
                |_| ControlFlow::Continue(()),
            )
            .await
            .unwrap();
        assert_eq!(fs::read(installed.path()).unwrap(), b"hello");

        let wrong = PinnedImage::pinned("3.1.0", 5, &"0".repeat(64), Some(&base));
        assert!(matches!(
            store.install(&wrong, |_| ControlFlow::Continue(())).await,
            Err(Error::Sha256)
        ));
        assert!(store.find("3.1.0").is_none());
    }

    #[tokio::test]
    async fn install_downloads_verifies_and_renames() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf).await;
                let body = if buf.starts_with(b"GET /bad/") {
                    b"hellp"
                } else {
                    b"hello"
                };
                let head = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\n";
                s.write_all(head).await.unwrap();
                s.write_all(body).await.unwrap();
            }
        });
        let root = tempfile::tempdir().unwrap();
        let store = ImageStore::new(root.path());

        let bad = PinnedImage::pinned(
            "1.0.0",
            5,
            HELLO_SHA256,
            Some(&format!("http://127.0.0.1:{port}/bad")),
        );
        assert!(matches!(
            store.install(&bad, |_| ControlFlow::Continue(())).await,
            Err(Error::Sha256)
        ));
        assert!(store.installed().unwrap().is_empty());
        assert!(
            fs::read_dir(store.dir()).unwrap().next().is_none(),
            "staging left behind"
        );

        let good = PinnedImage::pinned(
            "1.0.0",
            5,
            HELLO_SHA256,
            Some(&format!("http://127.0.0.1:{port}/good")),
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
        store.verify(&installed, Some(&good)).unwrap();

        let again = store
            .install(&good, |_| ControlFlow::Break(()))
            .await
            .unwrap();
        assert_eq!(
            again, installed,
            "an installed match is not downloaded again"
        );
        let cancelled = PinnedImage::pinned(
            "2.0.0",
            5,
            HELLO_SHA256,
            Some(&format!("http://127.0.0.1:{port}/good")),
        );
        assert!(matches!(
            store.install(&cancelled, |_| ControlFlow::Break(())).await,
            Err(Error::Cancelled)
        ));
        assert!(store.find("2.0.0").is_none());
    }
}
