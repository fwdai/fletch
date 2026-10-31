//! The images this host has downloaded, and downloading the ones this build
//! boots (port of Calina's image store, with layers and delta downloads).
//!
//! A release (`box-image-v<version>`, built by `packaging/box-image`) holds a
//! `manifest.json` and the files it lists: `base.erofs`, the root every box
//! boots, and `layer-<name>.erofs`, read-only layers init overlays on it
//! ([`OPTIONAL_LAYERS`]: `devtools`, `browser`). Each build names one release
//! ([`PinnedImage::this_build`]); the manifest gives every file's size and
//! sha256, which a download is verified against as it arrives, so a truncated
//! or corrupted download never installs. Next to each file the release may
//! carry `<file>.chunks.json` ([`chunks`]): with an older copy of the same
//! file on this host, only the chunks that changed are fetched ([`fetch`]).
//!
//! ```text
//! <root>/images/
//!   <version>/manifest.json       the release manifest, written before the first file
//!   <version>/base.erofs          a file is here only by renaming a verified download
//!   <version>/layer-<name>.erofs  the layers installed so far; absent means not installed
//!   .staging-*/                   a download in progress; the next install or prune removes leftovers
//! ```
//!
//! A verified download is not trusted forever: the files can be damaged on
//! disk later, and [`ImageStore::verify`] hashes them again before the first
//! boot of a session.

pub mod chunks;
mod fetch;

use std::fs::{self, File};
use std::io;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::fsutil;
use crate::version::version_key;

pub use fetch::Outcome;

pub const BASE_IMAGE: &str = "base.erofs";
pub const MANIFEST: &str = "manifest.json";
/// The base image's entry in a manifest.
pub const BASE: &str = "base";
/// The layers a release may carry besides the base, in the order init stacks
/// them. `devtools`: gcc, python3, pip, Node headers. `browser`: Debian's
/// chromium-headless-shell and the Noto fonts.
pub const OPTIONAL_LAYERS: &[&str] = &["devtools", "browser"];
/// The layers installed unless the user says otherwise.
pub const DEFAULT_LAYERS: &[&str] = &["devtools"];
/// A manifest is a few hundred KB at most (the package lists); anything past this is not one.
const MAX_MANIFEST: u64 = 4 << 20;
const STAGING_PREFIX: &str = ".staging-";
const IMAGES_DIR: &str = "images";

/// Where image releases are downloaded from: one GitHub release per image on
/// the public repository, tagged `box-image-v<version>`, holding
/// `manifest.json` and the files it lists.
pub const RELEASES_URL: &str = "https://github.com/fwdai/fletch/releases/download";

/// The release manifest (`manifest.json`), written by `packaging/box-image`
/// next to the files; a download is verified against its entry's size and sha256.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageManifest {
    pub version: String,
    pub arch: String,
    pub images: Vec<ImageEntry>,
}

/// One file of a release: the base or a layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageEntry {
    /// `base`, or a layer's name.
    pub name: String,
    pub file: String,
    pub size: u64,
    /// Lowercase hex once through [`fetch_manifest`].
    pub sha256: String,
    /// The top-level directories a layer carries (`usr`, `etc`, `var`); `/` for the base.
    #[serde(default)]
    pub dirs: Vec<String>,
    /// The uncompressed tree, for release notes.
    #[serde(default)]
    pub tree_bytes: u64,
    /// `name=version` of every Debian package in it (and `node=`).
    #[serde(default)]
    pub packages: Vec<String>,
}

impl ImageManifest {
    pub fn image(&self, name: &str) -> Option<&ImageEntry> {
        self.images.iter().find(|i| i.name == name)
    }

    pub fn base(&self) -> Option<&ImageEntry> {
        self.image(BASE)
    }

    /// The entries an install of `layers` needs: the base, then each named
    /// layer in manifest order. A layer the release does not have is an error.
    pub fn wanted(&self, layers: &[String]) -> Result<Vec<&ImageEntry>> {
        let base = self.base().ok_or_else(|| Error::Other("the manifest has no base image".into()))?;
        let mut wanted = vec![base];
        for entry in &self.images {
            if entry.name != BASE && layers.iter().any(|l| l == &entry.name) {
                wanted.push(entry);
            }
        }
        if let Some(unknown) = layers
            .iter()
            .find(|l| !wanted.iter().any(|e| &e.name == *l))
        {
            return Err(Error::Other(format!(
                "image {} has no `{unknown}` layer",
                self.version
            )));
        }
        Ok(wanted)
    }
}

/// The image this build boots: a version and the release directory holding
/// its `manifest.json` and files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedImage {
    pub version: String,
    /// The release directory, without a trailing slash.
    pub base: String,
}

impl PinnedImage {
    /// The image release this build boots, `box-image-v<VERSION>`
    /// (docs/box.md, "Releases"). `packaging/box-image/bump.sh` moves it with
    /// the image's VERSION; nothing else in the app changes with an image.
    pub const VERSION: &'static str = "0.3.0";

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
        self.file_url(BASE_IMAGE)
    }

    /// The URL of `manifest.json`.
    pub fn manifest_url(&self) -> String {
        self.file_url(MANIFEST)
    }

    /// The URL of one of the release's files.
    pub fn file_url(&self, file: &str) -> String {
        format!("{}/{file}", self.base)
    }
}

pub fn release_url(version: &str) -> String {
    format!("{RELEASES_URL}/box-image-v{version}")
}

/// A layer file installed next to its base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledLayer {
    pub name: String,
    pub file: String,
    pub sha256: String,
}

/// An image a `VmSpec` can boot from: the base and the layers installed with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledImage {
    pub dir: PathBuf,
    pub version: String,
    /// What `base.erofs` was verified against when installed, lowercase hex.
    pub sha256: String,
    /// The layers present, in manifest order (the order init stacks them).
    pub layers: Vec<InstalledLayer>,
}

impl InstalledImage {
    /// `base.erofs`.
    pub fn path(&self) -> PathBuf {
        self.dir.join(BASE_IMAGE)
    }

    pub fn layer_path(&self, layer: &InstalledLayer) -> PathBuf {
        self.dir.join(&layer.file)
    }

    /// The layer files, in stacking order: what a `VmSpec` attaches.
    pub fn layer_paths(&self) -> Vec<PathBuf> {
        self.layers.iter().map(|l| self.layer_path(l)).collect()
    }

    /// This image with only the layers `names` lists, or `None` when one of
    /// them is not installed.
    pub fn with_layers(&self, names: &[String]) -> Option<InstalledImage> {
        let layers: Vec<InstalledLayer> = self
            .layers
            .iter()
            .filter(|l| names.contains(&l.name))
            .cloned()
            .collect();
        (layers.len() == names.len()).then(|| InstalledImage {
            layers,
            ..self.clone()
        })
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
                (!name.starts_with('.')).then(|| image_at(entry.path(), &name))?
            })
            .collect();
        images.sort_by_key(|image| (version_key(&image.version), image.version.clone()));
        Ok(images)
    }

    /// The newest installed image, if any.
    pub fn current(&self) -> io::Result<Option<InstalledImage>> {
        Ok(self.installed()?.pop())
    }

    pub fn find(&self, version: &str) -> Option<InstalledImage> {
        image_at(self.dir.join(version), version)
    }

    /// Hashes `image`'s files again and compares them with the hashes
    /// recorded when they were installed. On a mismatch the image is removed
    /// and the result is [`Error::Damaged`]. Reads every file: call it once
    /// per session, off the UI thread.
    pub fn verify(&self, image: &InstalledImage) -> Result<()> {
        let mut files = vec![(image.path(), image.sha256.as_str())];
        files.extend(
            image
                .layers
                .iter()
                .map(|l| (image.layer_path(l), l.sha256.as_str())),
        );
        for (path, sha256) in files {
            if file_matches(&path, None, sha256)? {
                continue;
            }
            fs::remove_dir_all(&image.dir)
                .map_err(Error::io(format!("remove {}", image.dir.display())))?;
            return Err(Error::Damaged {
                version: image.version.clone(),
            });
        }
        Ok(())
    }

    /// Downloads what `image` with `layers` needs and is not installed yet.
    /// The manifest is fetched first and must describe `image.version`; each
    /// file is then verified as it arrives and lands in `<version>/` by one
    /// rename, so a failed or cancelled install leaves nothing half-done. A
    /// file an older version has on this host seeds a delta download.
    /// `progress` sees every chunk, over all files; returning `Break` cancels.
    pub async fn install(
        &self,
        image: &PinnedImage,
        layers: &[String],
        mut progress: impl FnMut(Progress) -> ControlFlow<()>,
    ) -> Result<InstalledImage> {
        let manifest = fetch_manifest(image).await?;
        let wanted = manifest.wanted(layers)?;
        fs::create_dir_all(&self.dir)
            .map_err(Error::io(format!("create {}", self.dir.display())))?;
        let target = self.dir.join(&image.version);

        let mut todo = Vec::new();
        for entry in wanted {
            let path = target.join(&entry.file);
            if path.is_file() && file_matches(&path, Some(entry.size), &entry.sha256)? {
                continue;
            }
            todo.push(entry);
        }
        if todo.is_empty() {
            // Everything is in place; the manifest may still be missing (an
            // install interrupted between the last file and nothing else).
            self.write_manifest(&target, &manifest)?;
            return self.installed_with(&image.version, layers);
        }

        self.remove_staging()?;
        let staging = self.dir.join(format!(
            "{STAGING_PREFIX}{}-{}",
            image.version,
            std::process::id()
        ));
        fs::create_dir(&staging).map_err(Error::io(format!("create {}", staging.display())))?;
        let total: u64 = todo.iter().map(|e| e.size).sum();
        let result = async {
            let mut done_before = 0u64;
            for entry in &todo {
                let seeds = self.seeds_for(&entry.file)?;
                let staged = staging.join(&entry.file);
                let url = image.file_url(&entry.file);
                let want = fetch::Want {
                    url: &url,
                    size: entry.size,
                    sha256: &entry.sha256,
                };
                let outcome = fetch::fetch(&want, &staged, &seeds, &mut |p| {
                    progress(Progress {
                        done: done_before + p.done,
                        total,
                    })
                })
                .await?;
                tracing::info!(file = %entry.file, ?outcome, "box image file installed");
                done_before += entry.size;
                // Into place: the manifest first, so a file is never there
                // without the record of what it was verified against.
                self.write_manifest(&target, &manifest)?;
                let final_path = target.join(&entry.file);
                fs::rename(&staged, &final_path)
                    .and_then(|()| fsutil::sync_dir(&target))
                    .map_err(Error::io(format!("install {}", final_path.display())))?;
            }
            Ok::<(), Error>(())
        }
        .await;
        let _ = fs::remove_dir_all(&staging);
        result?;
        self.installed_with(&image.version, layers)
    }

    /// Removes every installed version but `keep`, and leftovers of interrupted installs.
    pub fn prune(&self, keep: &str) -> io::Result<()> {
        self.remove_staging().map_err(io::Error::other)?;
        for entry in self.entries()? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with('.') && name != keep && entry.path().is_dir() {
                fs::remove_dir_all(entry.path())?;
            }
        }
        Ok(())
    }

    fn installed_with(&self, version: &str, layers: &[String]) -> Result<InstalledImage> {
        self.find(version)
            .and_then(|image| image.with_layers(layers))
            .ok_or_else(|| Error::Other(format!("image {version} did not install")))
    }

    fn write_manifest(&self, target: &Path, manifest: &ImageManifest) -> Result<()> {
        fs::create_dir_all(target).map_err(Error::io(format!("create {}", target.display())))?;
        let path = target.join(MANIFEST);
        let bytes = serde_json::to_vec_pretty(manifest).map_err(|e| Error::Other(e.to_string()))?;
        if fs::read(&path).ok().as_deref() == Some(bytes.as_slice()) {
            return Ok(());
        }
        fsutil::write_synced(&path, &bytes).map_err(Error::io(format!("write {}", path.display())))
    }

    /// Older (or damaged same-version) copies of `file` on this host: the
    /// seeds of a delta download.
    fn seeds_for(&self, file: &str) -> Result<Vec<PathBuf>> {
        let mut seeds = Vec::new();
        for entry in self.entries().map_err(Error::io("list the images"))? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let candidate = entry.path().join(file);
            if fs::metadata(&candidate).map(|m| m.is_file() && m.len() > 0).unwrap_or(false) {
                seeds.push(candidate);
            }
        }
        Ok(seeds)
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

/// The image in `dir`, if it has a manifest for `version` and a non-empty
/// base file; the layers are the manifest's other entries whose file is there.
fn image_at(dir: PathBuf, version: &str) -> Option<InstalledImage> {
    let manifest: ImageManifest = serde_json::from_slice(&fs::read(dir.join(MANIFEST)).ok()?).ok()?;
    if manifest.version != version {
        return None;
    }
    let present = |file: &str| {
        fs::metadata(dir.join(file))
            .map(|m| m.is_file() && m.len() > 0)
            .unwrap_or(false)
    };
    let base = manifest.base()?;
    if !present(&base.file) || !well_formed_sha256(&base.sha256) {
        return None;
    }
    let layers = manifest
        .images
        .iter()
        .filter(|e| e.name != BASE && present(&e.file) && well_formed_sha256(&e.sha256))
        .map(|e| InstalledLayer {
            name: e.name.clone(),
            file: e.file.clone(),
            sha256: e.sha256.clone(),
        })
        .collect();
    Some(InstalledImage {
        dir,
        version: version.to_owned(),
        sha256: base.sha256.clone(),
        layers,
    })
}

/// 64 lowercase hex digits.
pub(crate) fn well_formed_sha256(sha256: &str) -> bool {
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
    let actual = sha256_of(&mut file).map_err(Error::io(format!("read {}", path.display())))?;
    Ok(actual == sha256.to_ascii_lowercase())
}

/// The sha256 of everything `reader` yields, lowercase hex.
pub(crate) fn sha256_of(mut reader: impl io::Read) -> io::Result<String> {
    let mut hasher = Sha256::new();
    io::copy(&mut reader, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// The `manifest.json` of `image`'s release, checked to describe that version
/// with a base entry and, for every entry, a plain file name, a non-zero size
/// and a well-formed sha256 (returned lowercase). A `file://` base is read
/// from disk.
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
        let response = fetch::client()?
            .get(&url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(http)?;
        fetch::read_body(response, MAX_MANIFEST, &url).await?
    };
    let mut manifest: ImageManifest =
        serde_json::from_slice(&bytes).map_err(|e| bad(e.to_string()))?;
    if manifest.version != image.version {
        return Err(bad(format!(
            "describes image {}, not the {} this build boots",
            manifest.version, image.version
        )));
    }
    if manifest.base().is_none() {
        return Err(bad("has no base image".to_owned()));
    }
    for entry in &mut manifest.images {
        if entry.file.is_empty() || entry.file.contains('/') || entry.file.starts_with('.') {
            return Err(bad(format!("`{}` names the file {:?}", entry.name, entry.file)));
        }
        if entry.size == 0 {
            return Err(bad(format!("`{}` has size 0", entry.name)));
        }
        entry.sha256 = entry.sha256.to_ascii_lowercase();
        if !well_formed_sha256(&entry.sha256) {
            return Err(bad(format!("`{}`'s sha256 is not 64 hex digits", entry.name)));
        }
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    const HELLO_SHA256: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    fn sha(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn strings(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    /// A `manifest.json` as `packaging/box-image/build.sh` writes it, for a
    /// base of `base` bytes and the given `(name, bytes)` layers.
    fn manifest_json(version: &str, base: &[u8], layers: &[(&str, &[u8])]) -> String {
        let mut images = vec![serde_json::json!({
            "name": "base", "file": BASE_IMAGE, "size": base.len(), "sha256": sha(base),
            "dirs": ["/"], "tree_bytes": 1, "packages": ["bash=5.2"],
        })];
        for (name, bytes) in layers {
            images.push(serde_json::json!({
                "name": name, "file": format!("layer-{name}.erofs"), "size": bytes.len(),
                "sha256": sha(bytes), "dirs": ["usr", "etc", "var"], "tree_bytes": 1, "packages": [],
            }));
        }
        serde_json::json!({ "version": version, "arch": "aarch64", "images": images }).to_string()
    }

    /// A local release directory.
    fn local_release(version: &str, base: &[u8], layers: &[(&str, &[u8])]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(BASE_IMAGE), base).unwrap();
        for (name, bytes) in layers {
            fs::write(dir.path().join(format!("layer-{name}.erofs")), bytes).unwrap();
        }
        fs::write(
            dir.path().join(MANIFEST),
            manifest_json(version, base, layers),
        )
        .unwrap();
        dir
    }

    fn pin_to(version: &str, dir: &tempfile::TempDir) -> PinnedImage {
        PinnedImage::new(version, Some(&format!("file://{}", dir.path().display())))
    }

    fn go(_: Progress) -> ControlFlow<()> {
        ControlFlow::Continue(())
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
        assert_eq!(
            pinned.file_url("layer-devtools.erofs"),
            "https://github.com/fwdai/fletch/releases/download/box-image-v0.1.0/layer-devtools.erofs"
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
    fn a_manifest_knows_what_an_install_wants() {
        let m: ImageManifest =
            serde_json::from_str(&manifest_json("1.0.0", b"base", &[("devtools", b"d"), ("browser", b"b")]))
                .unwrap();
        let names = |w: Vec<&ImageEntry>| w.iter().map(|e| e.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(m.wanted(&[]).unwrap()), ["base"]);
        assert_eq!(
            names(m.wanted(&strings(&["browser", "devtools"])).unwrap()),
            ["base", "devtools", "browser"],
            "manifest order, not the request's"
        );
        assert!(m.wanted(&strings(&["gpu"])).is_err());
    }

    #[test]
    fn installed_images_are_found_verified_and_pruned() {
        let root = tempfile::tempdir().unwrap();
        let store = ImageStore::new(root.path());
        assert_eq!(store.current().unwrap(), None);

        let put = |version: &str, base: &[u8], layers: &[(&str, &[u8])], damage: bool| {
            let dir = store.dir().join(version);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(MANIFEST), manifest_json(version, base, layers)).unwrap();
            fs::write(dir.join(BASE_IMAGE), if damage { b"xxxxx" as &[u8] } else { base }).unwrap();
            for (name, bytes) in layers {
                fs::write(dir.join(format!("layer-{name}.erofs")), bytes).unwrap();
            }
        };
        put("0.9.0", b"hello", &[], false);
        put("0.10.0", b"hello", &[("devtools", b"dev")], false);
        put("0.2.0", b"hello", &[], true);
        // A manifest that promises a layer whose file is missing: the base
        // alone is installed.
        let dir = store.dir().join("0.11.0");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(MANIFEST), manifest_json("0.11.0", b"hello", &[("browser", b"b")])).unwrap();
        fs::write(dir.join(BASE_IMAGE), b"hello").unwrap();
        fs::create_dir_all(store.dir().join(".staging-x-1")).unwrap();

        let current = store.current().unwrap().unwrap();
        assert_eq!(current.version, "0.11.0");
        assert!(current.layers.is_empty());
        assert!(current.with_layers(&strings(&["browser"])).is_none());
        assert!(current.with_layers(&[]).is_some());

        let ten = store.find("0.10.0").unwrap();
        assert_eq!(ten.path(), store.dir().join("0.10.0").join(BASE_IMAGE));
        assert_eq!(ten.layers.len(), 1);
        assert_eq!(
            ten.layer_paths(),
            [store.dir().join("0.10.0").join("layer-devtools.erofs")]
        );
        assert_eq!(ten.with_layers(&strings(&["devtools"])), Some(ten.clone()));
        assert!(ten.with_layers(&[]).unwrap().layers.is_empty());
        store.verify(&ten).unwrap();

        let damaged = store.find("0.2.0").unwrap();
        assert!(matches!(store.verify(&damaged), Err(Error::Damaged { .. })));
        assert!(store.find("0.2.0").is_none());
        // A damaged layer takes the whole version down too.
        fs::write(store.dir().join("0.10.0").join("layer-devtools.erofs"), b"bad").unwrap();
        assert!(matches!(store.verify(&ten), Err(Error::Damaged { .. })));
        assert!(store.find("0.10.0").is_none());

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

        let other = local_release("3.1.0", b"hello", &[]);
        assert!(matches!(
            store.install(&pin_to("3.0.0", &other), &[], go).await,
            Err(Error::Manifest { .. })
        ));
        let malformed = tempfile::tempdir().unwrap();
        fs::write(malformed.path().join(MANIFEST), r#"{"version":"3.0.0","arch":"aarch64","images":[{"name":"base","file":"base.erofs","size":5,"sha256":"nope"}]}"#).unwrap();
        assert!(matches!(
            store.install(&pin_to("3.0.0", &malformed), &[], go).await,
            Err(Error::Manifest { .. })
        ));
        let missing = tempfile::tempdir().unwrap();
        assert!(matches!(
            store.install(&pin_to("3.0.0", &missing), &[], go).await,
            Err(Error::Io { .. })
        ));
        let good = local_release("3.0.0", b"hello", &[]);
        assert!(matches!(
            store.install(&pin_to("3.0.0", &good), &strings(&["browser"]), go).await,
            Err(Error::Other(_))
        ), "a layer the release lacks");
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

    /// A `file://` base URL installs by copying, verified like a download;
    /// layers come one install at a time, and a damaged file is replaced.
    #[tokio::test]
    async fn install_from_a_local_directory_copies_and_verifies() {
        let root = tempfile::tempdir().unwrap();
        let store = ImageStore::new(root.path());
        let src = local_release("3.0.0", b"hello", &[("devtools", b"dev"), ("browser", b"www")]);
        let pin = pin_to("3.0.0", &src);

        let installed = store.install(&pin, &strings(&["devtools"]), go).await.unwrap();
        assert_eq!(fs::read(installed.path()).unwrap(), b"hello");
        assert_eq!(installed.sha256, HELLO_SHA256);
        assert_eq!(installed.layers.len(), 1);
        assert_eq!(fs::read(&installed.layer_paths()[0]).unwrap(), b"dev");
        assert!(store.find("3.0.0").unwrap().with_layers(&strings(&["browser"])).is_none());

        let more = store.install(&pin, &strings(&["devtools", "browser"]), go).await.unwrap();
        assert_eq!(more.layers.len(), 2);
        assert_eq!(fs::read(&more.layer_paths()[1]).unwrap(), b"www");

        fs::write(installed.path(), b"hellp").unwrap();
        let again = store.install(&pin, &strings(&["devtools"]), go).await.unwrap();
        assert_eq!(fs::read(again.path()).unwrap(), b"hello", "a damaged file is replaced");
        assert_eq!(store.find("3.0.0").unwrap().layers.len(), 2, "the other layer stays");

        let wrong = tempfile::tempdir().unwrap();
        fs::write(wrong.path().join(BASE_IMAGE), b"hello").unwrap();
        fs::write(wrong.path().join(MANIFEST), manifest_json("3.1.0", b"other", &[])).unwrap();
        assert!(matches!(
            store.install(&pin_to("3.1.0", &wrong), &[], go).await,
            Err(Error::Sha256)
        ));
        assert!(store.find("3.1.0").is_none());
    }

    /// A release served over HTTP: `files` by path; GET with a `Range` header
    /// is answered with 206. Every request line (with its Range, if any) is
    /// recorded.
    async fn serve(files: HashMap<String, Vec<u8>>) -> (u16, Arc<Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let log = Arc::new(Mutex::new(Vec::new()));
        let seen = log.clone();
        let files = Arc::new(files);
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                let files = files.clone();
                let seen = seen.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buf[..n]).into_owned();
                    let path = request.split_whitespace().nth(1).unwrap_or("").to_owned();
                    let range = request
                        .lines()
                        .find_map(|l| l.strip_prefix("range: ").or_else(|| l.strip_prefix("Range: ")))
                        .map(|r| r.trim().to_owned());
                    seen.lock().unwrap().push(format!("{path} {}", range.as_deref().unwrap_or("")));
                    let Some(body) = files.get(path.trim_start_matches('/')) else {
                        s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                            .await
                            .unwrap();
                        return;
                    };
                    let (status, body): (&str, Vec<u8>) = match range.as_deref().and_then(|r| r.strip_prefix("bytes=")) {
                        Some(r) => {
                            let (a, b) = r.split_once('-').unwrap();
                            let (a, b): (usize, usize) = (a.parse().unwrap(), b.parse().unwrap());
                            ("206 Partial Content", body[a..=b].to_vec())
                        }
                        None => ("200 OK", body.clone()),
                    };
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    s.write_all(head.as_bytes()).await.unwrap();
                    s.write_all(&body).await.unwrap();
                });
            }
        });
        (port, log)
    }

    fn noise(seed: u64, len: usize) -> Vec<u8> {
        let mut x = seed | 1;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect()
    }

    fn chunk_list_json(bytes: &[u8]) -> Vec<u8> {
        let mut chunker = chunks::Chunker::new();
        chunker.update(bytes);
        let list = chunks::ChunkList {
            algorithm: chunks::ALGORITHM.into(),
            avg: chunks::AVG,
            size: bytes.len() as u64,
            sha256: sha(bytes),
            chunks: chunker.finish(),
        };
        serde_json::to_vec(&list).unwrap()
    }

    #[tokio::test]
    async fn install_downloads_verifies_and_renames() {
        let mut files = HashMap::new();
        files.insert("1.0.0/good/manifest.json".to_owned(), manifest_json("1.0.0", b"hello", &[]).into_bytes());
        files.insert("1.0.0/good/base.erofs".to_owned(), b"hello".to_vec());
        files.insert("1.0.0/bad/manifest.json".to_owned(), manifest_json("1.0.0", b"hello", &[]).into_bytes());
        files.insert("1.0.0/bad/base.erofs".to_owned(), b"hellp".to_vec());
        files.insert("2.0.0/good/manifest.json".to_owned(), manifest_json("2.0.0", b"hello", &[]).into_bytes());
        files.insert("2.0.0/good/base.erofs".to_owned(), b"hello".to_vec());
        let (port, _log) = serve(files).await;
        let root = tempfile::tempdir().unwrap();
        let store = ImageStore::new(root.path());

        let bad = PinnedImage::new("1.0.0", Some(&format!("http://127.0.0.1:{port}/1.0.0/bad")));
        assert!(matches!(
            store.install(&bad, &[], go).await,
            Err(Error::Sha256)
        ));
        assert!(store.installed().unwrap().is_empty());
        assert!(
            fs::read_dir(store.dir()).unwrap().all(|e| e.unwrap().file_name() == "1.0.0"),
            "staging left behind"
        );
        assert!(store.find("1.0.0").is_none(), "a manifest alone is not an install");

        let good = PinnedImage::new(
            "1.0.0",
            Some(&format!("http://127.0.0.1:{port}/1.0.0/good")),
        );
        let mut seen = Vec::new();
        let installed = store
            .install(&good, &[], |p| {
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

        let again = store.install(&good, &[], |_| ControlFlow::Break(())).await.unwrap();
        assert_eq!(
            again, installed,
            "an installed match is not downloaded again"
        );
        let cancelled = PinnedImage::new(
            "2.0.0",
            Some(&format!("http://127.0.0.1:{port}/2.0.0/good")),
        );
        assert!(matches!(
            store.install(&cancelled, &[], |_| ControlFlow::Break(())).await,
            Err(Error::Cancelled)
        ));
        assert!(store.find("2.0.0").is_none());
    }

    /// With an older version on disk and a chunk list in the release, a new
    /// version is assembled from the old file plus range requests, and the
    /// result is byte for byte the new file.
    #[tokio::test]
    async fn an_older_image_seeds_a_delta_download() {
        let v1 = noise(1, 3 << 20);
        let mut v2 = v1[..1 << 20].to_vec();
        v2.extend_from_slice(&noise(2, 200_000));
        v2.extend_from_slice(&v1[1 << 20..]);
        let layer = noise(3, 1 << 20);
        let mut files = HashMap::new();
        files.insert("1.0.0/manifest.json".to_owned(), manifest_json("1.0.0", &v1, &[]).into_bytes());
        files.insert("1.0.0/base.erofs".to_owned(), v1.clone());
        files.insert("2.0.0/manifest.json".to_owned(), manifest_json("2.0.0", &v2, &[("devtools", &layer[..])]).into_bytes());
        files.insert("2.0.0/base.erofs".to_owned(), v2.clone());
        files.insert("2.0.0/base.erofs.chunks.json".to_owned(), chunk_list_json(&v2));
        files.insert("2.0.0/layer-devtools.erofs".to_owned(), layer.clone());
        // A chunk list that lies about the layer: the delta is abandoned for a
        // whole download, which the manifest then verifies.
        files.insert("2.0.0/layer-devtools.erofs.chunks.json".to_owned(), chunk_list_json(&noise(9, 1 << 20)));
        let (port, log) = serve(files).await;
        let root = tempfile::tempdir().unwrap();
        let store = ImageStore::new(root.path());
        let at = |v: &str| PinnedImage::new(v, Some(&format!("http://127.0.0.1:{port}/{v}")));

        store.install(&at("1.0.0"), &[], go).await.unwrap();
        log.lock().unwrap().clear();
        let two = store.install(&at("2.0.0"), &strings(&["devtools"]), go).await.unwrap();
        assert_eq!(fs::read(two.path()).unwrap(), v2);
        assert_eq!(fs::read(&two.layer_paths()[0]).unwrap(), layer);

        let requests = log.lock().unwrap().clone();
        let base_requests: Vec<&String> = requests.iter().filter(|r| r.starts_with("/2.0.0/base.erofs ")).collect();
        assert!(!base_requests.is_empty() && base_requests.iter().all(|r| r.contains("bytes=")),
            "the base came by range requests only: {requests:?}");
        let ranged: u64 = base_requests
            .iter()
            .map(|r| {
                let (a, b) = r.rsplit_once("bytes=").unwrap().1.split_once('-').unwrap();
                b.parse::<u64>().unwrap() - a.parse::<u64>().unwrap() + 1
            })
            .sum();
        assert!(ranged < v2.len() as u64 / 2, "fetched {ranged} of {}", v2.len());
        assert!(requests.iter().any(|r| r == "/2.0.0/layer-devtools.erofs "),
            "the layer fell back to a whole download: {requests:?}");
        store.verify(&two).unwrap();
        // The seed is gone once pruned; the next install of 2.0.0 is a no-op.
        store.prune("2.0.0").unwrap();
        assert_eq!(store.installed().unwrap().len(), 1);
    }
}
