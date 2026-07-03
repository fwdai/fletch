//! Unified `git` binary resolution plus a self-installing portable fallback.
//!
//! The app must work on machines with no usable git at all — including the
//! deceptive macOS case where `/usr/bin/git` exists but is only the Xcode
//! Command Line Tools shim (running it errors, or pops Apple's multi-GB
//! installer dialog). Resolution therefore requires a git that actually
//! *runs*: system candidates are probed with `--version` (the `/usr/bin`
//! shim is pre-filtered via `xcode-select -p` so the probe itself never
//! triggers the CLT dialog), and when none survives, a pinned dugite-native
//! distribution (the same relocatable git GitHub Desktop ships) is
//! downloaded into the app data dir, checksum-verified, and used instead.
//!
//! Every git subprocess in the app goes through [`command`], so the chosen
//! binary and its relocation env (`GIT_EXEC_PATH`, …) are applied in one
//! place. Agent sessions get the same git on their PATH via [`child_env`].

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::{Arc, OnceLock, RwLock};

use serde_json::{json, Value};

/// Pinned dugite-native release. Bumping the distribution = updating these
/// constants (URLs embed the release tag + git version, checksums pin the
/// exact artifacts).
const DIST_TAG: &str = "v2.53.0-3";
const DIST_URL_BASE: &str =
    "https://github.com/desktop/dugite-native/releases/download/v2.53.0-3/dugite-native-v2.53.0-f49d009-";

/// The one artifact this build can install: platform asset suffix + its
/// SHA-256. `None` on platforms dugite-native doesn't cover — resolution then
/// simply reports git as missing rather than attempting a download.
fn dist_asset() -> Option<(&'static str, &'static str)> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some((
            "macOS-arm64.tar.gz",
            "e561cfc80c755e6f3e938653e81efcd025c9827a5b76dd42778b1159b3fab437",
        )),
        ("macos", "x86_64") => Some((
            "macOS-x64.tar.gz",
            "caf27c36b8834969550535bcd5e58186f970e080d1e175e76d9c1de3aac409ed",
        )),
        ("linux", "x86_64") => Some((
            "ubuntu-x64.tar.gz",
            "b3a85433c8dfde76d21b90938ad2f971653deff4340b1b4d347258c63250eafc",
        )),
        ("linux", "aarch64") => Some((
            "ubuntu-arm64.tar.gz",
            "d562ad433ed0dc1907f44a92fc701597bc577c48d07fe69ee7adddfee836ef4c",
        )),
        ("windows", "x86_64") => Some((
            "windows-x64.tar.gz",
            "f843a87a693bfdabed83b8492bca59db6f64d1168c74d23e2c8dfb7388a97142",
        )),
        _ => None,
    }
}

const GIT_BIN_NAME: &str = if cfg!(windows) { "git.exe" } else { "git" };

/// Which git the app is running on. Serialized into `ToolStatus.source` so
/// the readiness UI can say "bundled" instead of showing an app-support path
/// as if the user installed it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GitSource {
    System,
    Portable,
}

impl GitSource {
    pub fn as_str(self) -> &'static str {
        match self {
            GitSource::System => "system",
            GitSource::Portable => "portable",
        }
    }
}

/// A resolved, runnable git: absolute program path plus the env vars it needs.
/// System git needs none; portable git carries its relocation vars.
pub struct GitBin {
    pub program: PathBuf,
    pub env: Vec<(String, String)>,
    pub source: GitSource,
}

/// Root dir for portable installs (`<app-data>/git-dist`), set once at
/// startup. Versioned subdirs let a future dist bump install alongside the
/// old one and switch atomically.
static INSTALL_ROOT: OnceLock<PathBuf> = OnceLock::new();

/// The resolved binary, cached after the first successful resolution.
/// Explicitly re-set when a portable install completes mid-session, so a
/// `None` (no usable git yet) doesn't stick for the process lifetime.
fn active() -> &'static RwLock<Option<Arc<GitBin>>> {
    static ACTIVE: OnceLock<RwLock<Option<Arc<GitBin>>>> = OnceLock::new();
    ACTIVE.get_or_init(|| RwLock::new(None))
}

/// Fallback commit identity source — a closure reading the signed-in profile
/// (accounts table), installed at startup so git code never needs a DB handle.
type IdentitySource = Box<dyn Fn() -> Option<(Option<String>, Option<String>)> + Send + Sync>;

fn identity_source() -> &'static RwLock<Option<IdentitySource>> {
    static SRC: OnceLock<RwLock<Option<IdentitySource>>> = OnceLock::new();
    SRC.get_or_init(|| RwLock::new(None))
}

/// Set the portable-install root. Must be called once before any resolution
/// that should consider a portable install (startup does this immediately).
pub fn init(install_root: PathBuf) {
    let _ = INSTALL_ROOT.set(install_root);
}

pub fn set_identity_source(f: IdentitySource) {
    *identity_source().write().unwrap() = Some(f);
}

/// Name/email to author commits with when the repo resolves no user.name /
/// user.email: the signed-in profile when available, else a neutral default
/// (so a first commit never dies with git's "Please tell me who you are").
pub fn fallback_identity() -> (String, String) {
    let profile = identity_source()
        .read()
        .unwrap()
        .as_ref()
        .and_then(|f| f());
    let (name, email) = profile.unwrap_or((None, None));
    (
        name.filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "Fletch".to_string()),
        email
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "fletch@localhost".to_string()),
    )
}

/// The resolved git, if any. First call probes (subprocess-cheap, and the
/// startup task warms this immediately); later calls hit the cache.
pub fn resolve() -> Option<Arc<GitBin>> {
    if let Some(bin) = active().read().unwrap().clone() {
        return Some(bin);
    }
    let resolved = resolve_uncached().map(Arc::new);
    if resolved.is_some() {
        *active().write().unwrap() = resolved.clone();
    }
    resolved
}

fn resolve_uncached() -> Option<GitBin> {
    if let Some(bin) = system_git() {
        return Some(bin);
    }
    installed_portable_git()
}

/// A tokio Command for the resolved git, with its env applied and `dir` as
/// cwd. When nothing resolves, falls back to bare `git` so the failure
/// surfaces as the same spawn error it always did — callers need no new path.
pub fn command(dir: &Path) -> tokio::process::Command {
    let mut cmd = bare_command();
    cmd.current_dir(dir);
    cmd
}

/// [`command`] without a cwd — for the rare op whose target dir doesn't exist
/// yet (`git init <path>`).
pub fn bare_command() -> tokio::process::Command {
    match resolve() {
        Some(bin) => {
            let mut cmd = tokio::process::Command::new(&bin.program);
            for (k, v) in &bin.env {
                cmd.env(k, v);
            }
            cmd
        }
        None => tokio::process::Command::new("git"),
    }
}

/// Extra env for spawned agent/terminal children so *their* `git status` /
/// `git diff` calls work too: when the app runs on portable git, prepend its
/// bin dir to the PATH the child would otherwise see (login-shell PATH when
/// known, else the process PATH) and pass the relocation vars along. Empty on
/// system git — the child's own PATH already works.
pub fn child_env() -> Vec<(String, String)> {
    let Some(bin) = resolve() else {
        return Vec::new();
    };
    if bin.source != GitSource::Portable {
        return Vec::new();
    }
    let mut env = bin.env.clone();
    if let Some(bin_dir) = bin.program.parent() {
        let base = crate::bin_resolve::login_shell_env()
            .and_then(|e| e.get("PATH").cloned())
            .or_else(|| std::env::var("PATH").ok())
            .unwrap_or_default();
        let joined = std::env::join_paths(
            std::iter::once(bin_dir.to_path_buf()).chain(std::env::split_paths(&base)),
        );
        if let Ok(path) = joined {
            env.push(("PATH".to_string(), path.to_string_lossy().into_owned()));
        }
    }
    env
}

/// Readiness-check view of resolution, replacing the old presence-only PATH
/// lookup for `git` (which reported the CLT shim as installed).
pub fn tool_status() -> crate::agent::ToolStatus {
    match resolve() {
        Some(bin) => {
            let version = probe_version(&bin);
            crate::agent::ToolStatus {
                installed: true,
                version,
                path: Some(bin.program.to_string_lossy().into_owned()),
                source: Some(bin.source.as_str().to_string()),
            }
        }
        None => crate::agent::ToolStatus {
            installed: false,
            version: None,
            path: None,
            source: None,
        },
    }
}

fn probe_version(bin: &GitBin) -> Option<String> {
    let mut cmd = StdCommand::new(&bin.program);
    cmd.arg("--version");
    for (k, v) in &bin.env {
        cmd.env(k, v);
    }
    let out = cmd.output().ok()?;
    crate::agent::parse_semver(&String::from_utf8_lossy(&out.stdout))
}

// ---------------------------------------------------------------------------
// System git
// ---------------------------------------------------------------------------

fn system_git() -> Option<GitBin> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    crate::bin_resolve::resolve_bin_candidates("git", &home)
        .into_iter()
        .find(|candidate| system_git_usable(Path::new(candidate)))
        .map(|program| GitBin {
            program: PathBuf::from(program),
            env: Vec::new(),
            source: GitSource::System,
        })
}

/// Whether a system git candidate actually runs. On macOS, `/usr/bin/git` is
/// checked via `xcode-select -p` instead of being executed: running the CLT
/// shim without developer tools pops Apple's installer dialog, which a
/// background probe must never do.
fn system_git_usable(candidate: &Path) -> bool {
    if cfg!(target_os = "macos") && candidate.starts_with("/usr/bin") && !clt_present() {
        return false;
    }
    StdCommand::new(candidate)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Whether the Xcode Command Line Tools (or full Xcode) are actually
/// installed — i.e. the `/usr/bin` tool shims are backed by real binaries.
fn clt_present() -> bool {
    let Ok(out) = StdCommand::new("/usr/bin/xcode-select").arg("-p").output() else {
        return false;
    };
    if !out.status.success() {
        return false;
    }
    let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    !dir.is_empty() && Path::new(&dir).exists()
}

// ---------------------------------------------------------------------------
// Portable git
// ---------------------------------------------------------------------------

fn install_dir() -> Option<PathBuf> {
    INSTALL_ROOT.get().map(|root| root.join(DIST_TAG))
}

fn installed_portable_git() -> Option<GitBin> {
    let dir = install_dir()?;
    let program = dir.join("bin").join(GIT_BIN_NAME);
    if !program.is_file() {
        return None;
    }
    Some(GitBin {
        program,
        env: portable_env(&dir),
        source: GitSource::Portable,
    })
}

/// Relocation env for a portable dist dir. Each var is set only when its
/// target exists, so a layout change in a future dist degrades to git's own
/// defaults instead of pointing at nothing.
fn portable_env(dir: &Path) -> Vec<(String, String)> {
    let mut env = Vec::new();
    let mut push_if_dir = |key: &str, path: PathBuf| {
        if path.is_dir() {
            env.push((key.to_string(), path.to_string_lossy().into_owned()));
        }
    };
    push_if_dir("GIT_EXEC_PATH", dir.join("libexec/git-core"));
    push_if_dir("GIT_TEMPLATE_DIR", dir.join("share/git-core/templates"));
    let cacert = dir.join("ssl/cacert.pem");
    if cacert.is_file() {
        env.push((
            "GIT_SSL_CAINFO".to_string(),
            cacert.to_string_lossy().into_owned(),
        ));
    }
    env
}

/// Download + verify + install the portable dist, then activate it. Idempotent:
/// returns immediately when already installed. Errors are strings — the caller
/// (startup task) logs and reports them; nothing else depends on this.
pub async fn ensure_installed(on_progress: impl Fn(u64, Option<u64>)) -> Result<(), String> {
    if installed_portable_git().is_some() {
        activate_portable();
        return Ok(());
    }
    let root = INSTALL_ROOT
        .get()
        .ok_or("portable git install root not initialized")?
        .clone();
    let (asset, sha) = dist_asset().ok_or_else(|| {
        format!(
            "no portable git build for {}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    std::fs::create_dir_all(&root).map_err(|e| format!("create {}: {e}", root.display()))?;

    let url = format!("{DIST_URL_BASE}{asset}");
    tracing::info!(%url, "downloading portable git");
    let tarball = download_verified(&url, sha, on_progress).await?;

    let dest = root.join(DIST_TAG);
    tokio::task::spawn_blocking(move || extract_dist(&tarball, &dest))
        .await
        .map_err(|e| format!("extract task: {e}"))??;

    if installed_portable_git().is_none() {
        return Err("portable git dist extracted but bin/git is missing".to_string());
    }
    activate_portable();
    tracing::info!("portable git installed");
    Ok(())
}

/// Point resolution at the (just-)installed portable git. Overwrites a cached
/// `None`-in-progress state; never downgrades a working system git because
/// this only runs when system resolution already failed.
fn activate_portable() {
    if let Some(bin) = installed_portable_git() {
        *active().write().unwrap() = Some(Arc::new(bin));
    }
}

/// Stream the artifact to a temp file, hashing as it downloads; error unless
/// the digest matches the pinned SHA-256. Returns the temp path.
async fn download_verified(
    url: &str,
    expected_sha: &str,
    on_progress: impl Fn(u64, Option<u64>),
) -> Result<PathBuf, String> {
    use sha2::Digest;
    use tokio::io::AsyncWriteExt;

    let resp = reqwest::get(url)
        .await
        .map_err(|e| format!("download: {e}"))?
        .error_for_status()
        .map_err(|e| format!("download: {e}"))?;
    let total = resp.content_length();

    let root = INSTALL_ROOT.get().expect("checked by caller").clone();
    let tmp_path = root.join(format!("download-{}.tmp", std::process::id()));
    let mut file = tokio::fs::File::create(&tmp_path)
        .await
        .map_err(|e| format!("create {}: {e}", tmp_path.display()))?;

    let mut hasher = sha2::Sha256::new();
    let mut received: u64 = 0;
    let mut last_reported: u64 = 0;
    let mut resp = resp;
    loop {
        let chunk = match resp.chunk().await {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(e) => {
                let _ = tokio::fs::remove_file(&tmp_path).await;
                return Err(format!("download: {e}"));
            }
        };
        hasher.update(&chunk);
        if let Err(e) = file.write_all(&chunk).await {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(format!("write {}: {e}", tmp_path.display()));
        }
        received += chunk.len() as u64;
        // Report at ~5% steps (or every 8 MiB when the size is unknown) so the
        // UI gets a live number without an event per network chunk.
        let step = total.map(|t| t / 20).unwrap_or(8 * 1024 * 1024).max(1);
        if received - last_reported >= step {
            last_reported = received;
            on_progress(received, total);
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);
    on_progress(received, total);

    let digest = format!("{:x}", hasher.finalize());
    if !digest.eq_ignore_ascii_case(expected_sha) {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(format!(
            "portable git checksum mismatch (got {digest}, expected {expected_sha})"
        ));
    }
    Ok(tmp_path)
}

/// Unpack `tarball` and move the dist into `dest`. Extraction goes to a
/// sibling temp dir first so `dest` only ever appears complete; the tarball
/// is always removed. dugite-native tarballs unpack their layout (`bin/`,
/// `libexec/`, …) at the archive root, but a single wrapping top-level dir is
/// tolerated in case a future release adds one.
fn extract_dist(tarball: &Path, dest: &Path) -> Result<(), String> {
    let staging = dest.with_extension("partial");
    let result = (|| {
        if staging.exists() {
            std::fs::remove_dir_all(&staging).map_err(|e| format!("clear staging: {e}"))?;
        }
        std::fs::create_dir_all(&staging).map_err(|e| format!("create staging: {e}"))?;

        let file = std::fs::File::open(tarball).map_err(|e| format!("open tarball: {e}"))?;
        let gz = flate2::read::GzDecoder::new(std::io::BufReader::new(file));
        tar::Archive::new(gz)
            .unpack(&staging)
            .map_err(|e| format!("unpack: {e}"))?;

        let dist_root = locate_dist_root(&staging)
            .ok_or_else(|| "archive does not contain bin/git".to_string())?;

        if dest.exists() {
            std::fs::remove_dir_all(dest).map_err(|e| format!("clear dest: {e}"))?;
        }
        std::fs::rename(&dist_root, dest).map_err(|e| format!("install dist: {e}"))?;
        Ok(())
    })();

    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_file(tarball);
    result
}

/// The dir holding `bin/git` — the unpack root itself, or its single
/// subdirectory.
fn locate_dist_root(unpacked: &Path) -> Option<PathBuf> {
    if unpacked.join("bin").join(GIT_BIN_NAME).is_file() {
        return Some(unpacked.to_path_buf());
    }
    let entries: Vec<_> = std::fs::read_dir(unpacked)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    match entries.as_slice() {
        [single] if single.join("bin").join(GIT_BIN_NAME).is_file() => Some(single.clone()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------

/// Resolve at launch and, when no usable git exists, install the portable
/// dist — kicked off from `setup` so a git-less machine is likely ready by
/// the time the user reaches onboarding's readiness screen. `emit` receives
/// JSON state payloads for the `git-dist:state` frontend event.
pub async fn startup(emit: impl Fn(Value) + Send + Sync + 'static) {
    emit(json!({ "phase": "checking" }));
    let resolved = tokio::task::spawn_blocking(resolve).await.ok().flatten();
    if let Some(bin) = resolved {
        emit(json!({ "phase": "ready", "source": bin.source.as_str() }));
        return;
    }
    emit(json!({ "phase": "downloading" }));
    let emit = Arc::new(emit);
    let progress_emit = emit.clone();
    let result = ensure_installed(move |received, total| {
        progress_emit(json!({
            "phase": "downloading",
            "received": received,
            "total": total,
        }));
    })
    .await;
    match result {
        Ok(()) => emit(json!({ "phase": "ready", "source": "portable" })),
        Err(e) => {
            tracing::warn!(error = %e, "portable git install failed");
            emit(json!({ "phase": "failed", "error": e }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a gzipped tar with the given file paths (contents are the path
    /// bytes; `bin/git` entries get the exec bit).
    fn make_tarball(dir: &Path, files: &[&str]) -> PathBuf {
        let path = dir.join("dist.tar.gz");
        let file = std::fs::File::create(&path).unwrap();
        let gz = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        for f in files {
            let data = f.as_bytes();
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(if f.ends_with("bin/git") { 0o755 } else { 0o644 });
            header.set_cksum();
            tar.append_data(&mut header, f, data).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
        path
    }

    /// The layout dugite-native actually ships: `bin/git` at the archive root.
    #[test]
    fn extract_installs_root_layout() {
        let td = tempfile::tempdir().unwrap();
        let tarball = make_tarball(td.path(), &["bin/git", "libexec/git-core/git-remote-https"]);
        let dest = td.path().join("v-test");

        extract_dist(&tarball, &dest).unwrap();

        assert!(dest.join("bin/git").is_file());
        assert!(dest.join("libexec/git-core/git-remote-https").is_file());
        assert!(!tarball.exists(), "tarball must be cleaned up");
        assert!(
            !dest.with_extension("partial").exists(),
            "staging dir must be cleaned up",
        );
    }

    /// A future dist that wraps everything in one top-level dir still installs.
    #[test]
    fn extract_unwraps_single_top_level_dir() {
        let td = tempfile::tempdir().unwrap();
        let tarball = make_tarball(td.path(), &["git-dist/bin/git"]);
        let dest = td.path().join("v-test");

        extract_dist(&tarball, &dest).unwrap();

        assert!(dest.join("bin/git").is_file());
    }

    /// An archive with no git binary must fail loudly, not install garbage.
    #[test]
    fn extract_rejects_archive_without_git() {
        let td = tempfile::tempdir().unwrap();
        let tarball = make_tarball(td.path(), &["README.md"]);
        let dest = td.path().join("v-test");

        assert!(extract_dist(&tarball, &dest).is_err());
        assert!(!dest.exists());
    }

    /// Relocation env points only at dirs that exist, so a layout change in a
    /// future dist degrades gracefully instead of exporting dangling paths.
    #[test]
    fn portable_env_sets_only_existing_paths() {
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(td.path().join("libexec/git-core")).unwrap();

        let env = portable_env(td.path());

        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"GIT_EXEC_PATH"));
        assert!(!keys.contains(&"GIT_TEMPLATE_DIR"));
        assert!(!keys.contains(&"GIT_SSL_CAINFO"));
    }

    /// Identity falls back field-by-field: a profile with only a name still
    /// gets the default email, and blank strings don't count as set.
    #[test]
    fn fallback_identity_fills_missing_fields() {
        set_identity_source(Box::new(|| Some((Some("Ada".into()), Some("  ".into())))));
        assert_eq!(
            fallback_identity(),
            ("Ada".to_string(), "fletch@localhost".to_string()),
        );
        *identity_source().write().unwrap() = None;
        assert_eq!(
            fallback_identity(),
            ("Fletch".to_string(), "fletch@localhost".to_string()),
        );
    }
}
