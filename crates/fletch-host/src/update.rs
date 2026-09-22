//! `fletch-host update [<version>] [--check]`: replace this binary with a
//! published release's.
//!
//! The release job (.github/workflows/release.yml, job `host`) uploads, per
//! target triple, `fletch-host-<version>-<target>.tar.gz` plus a `.sha256` and
//! a `.sig`. This module resolves one of those releases through the GitHub API,
//! downloads the three files, and only then touches anything:
//!
//! 1. the checksum must match, **and**
//! 2. the minisign signature must verify against [`UPDATER_PUBKEY`] — the same
//!    key the desktop updater trusts (`plugins.updater.pubkey` in
//!    `src-tauri/tauri.conf.json`, asserted equal by a test below).
//!
//! Both are fail-closed: a missing `.sig`, a missing `.sha256` or a bad either
//! aborts before the swap. An unsigned tarball is not a downgrade in security
//! from "no update command at all" — it is a remote code execution path, so it
//! is refused rather than warned about.
//!
//! The swap itself is one `rename` over the running executable, after the
//! SQLite database has been copied aside. Nothing more: the prepared/committed
//! handoff and the DB rollback are deferred item 17 in
//! docs/multi-host-plan.md §5.3, and deliberately not started here.
//!
//! Privilege is the one thing this module is careful about. A binary that
//! lives in a root-owned directory needs root to replace it, but nothing else
//! about an update needs root — and root writing into a user's data dir is a
//! root-owned-file-overwrite waiting for a planted symlink. So the work is
//! split: [`run`] without `--from` is phase one, run **as the user the host
//! runs as**, which downloads, verifies and backs up into that user's own data
//! dir, and then either swaps the binary itself or prints the one `sudo`
//! command that does. `sudo fletch-host update --from <tarball>` is phase two:
//! it reads the already-verified file (and verifies it again — it trusts the
//! signature, not the caller), writes only its own executable's directory,
//! and restarts the service. It never opens a data dir. Phase one refuses to
//! run under sudo at all, so there is no path on which root's default data dir
//! — or a guess at whose data dir it should be — is ever used.

use std::io::Read;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{admin, service};

/// Where releases come from. The same repo the desktop updater reads.
pub const REPO: &str = "fwdai/fletch";

/// This build's target triple, from `build.rs`. Names the asset to ask for.
pub const TARGET: &str = env!("TARGET");

/// The version this binary is, which is also the version a pre-update database
/// backup is named after.
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The project's updater public key, base64 of a minisign public key file.
///
/// Copied from `plugins.updater.pubkey` in `src-tauri/tauri.conf.json` — the
/// key the desktop's updater trusts and the one the release workflow signs
/// with (`TAURI_SIGNING_PRIVATE_KEY`). Embedded rather than read at runtime
/// because a key an attacker can edit is not a key; `pubkey_matches_tauri_conf`
/// below fails the build's test suite if the two ever drift.
pub const UPDATER_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDY2Njg3OUNDQ0E2MUYxNDUKUldSRjhXSEt6SGxvWnZTdnplUDEzRHVSdmR2OEJwanhLTEJLQ2MzTnVLU0tZVTBnVElzbjV1Vk0K";

/// The file inside every host tarball.
const BINARY_IN_TARBALL: &str = "fletch-host";

/// GitHub wants a user agent and will refuse an anonymous client without one.
const UA: &str = concat!("fletch-host/", env!("CARGO_PKG_VERSION"));

/// The asset this build can install, for a given version.
pub fn asset_name(version: &str) -> String {
    format!("fletch-host-{version}-{TARGET}.tar.gz")
}

/// The version an asset name carries, if it is this build's asset at all —
/// the inverse of [`asset_name`]. Another target's tarball is `None`: a binary
/// for the wrong triple must not be installed over this one.
pub fn version_in_asset_name(name: &str) -> Option<&str> {
    name.strip_prefix("fletch-host-")?
        .strip_suffix(&format!("-{TARGET}.tar.gz"))
        .filter(|version| !version.is_empty() && !version.contains('/'))
}

/// The three URLs an install needs, and the version they belong to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assets {
    pub version: String,
    pub tarball: String,
    pub sha256: String,
    pub sig: String,
}

/// Pull this target's assets out of a GitHub release document.
///
/// Every failure here is a release that cannot be installed, described in terms
/// of what is missing: a target nobody built, or a tarball published without
/// its checksum or signature.
pub fn pick_assets(release: &Value) -> Result<Assets, String> {
    let tag = release["tag_name"]
        .as_str()
        .ok_or("the GitHub release has no tag_name")?;
    let version = tag.strip_prefix('v').unwrap_or(tag).to_string();
    let wanted = asset_name(&version);

    let assets = release["assets"].as_array().cloned().unwrap_or_default();
    let url = |name: &str| -> Option<String> {
        assets.iter().find_map(|asset| {
            (asset["name"].as_str() == Some(name))
                .then(|| asset["browser_download_url"].as_str())
                .flatten()
                .map(str::to_string)
        })
    };

    let tarball = url(&wanted).ok_or_else(|| {
        let published: Vec<&str> = assets
            .iter()
            .filter_map(|asset| asset["name"].as_str())
            .filter(|name| name.starts_with("fletch-host-") && name.ends_with(".tar.gz"))
            .collect();
        format!(
            "{tag} has no {wanted}; it published {}",
            if published.is_empty() {
                "no host archives at all".to_string()
            } else {
                published.join(", ")
            }
        )
    })?;
    // Fail closed, both of them: an update that cannot be verified is not
    // installed. See the module docs.
    let sha256 = url(&format!("{wanted}.sha256"))
        .ok_or_else(|| format!("{tag} published {wanted} without its .sha256; refusing"))?;
    let sig = url(&format!("{wanted}.sig"))
        .ok_or_else(|| format!("{tag} published {wanted} without its .sig; refusing"))?;

    Ok(Assets {
        version,
        tarball,
        sha256,
        sig,
    })
}

/// `update`, the whole subcommand. `from` selects phase two (see the module
/// docs); everything else is phase one.
pub async fn run(
    data_dir: &Path,
    version: Option<String>,
    check: bool,
    from: Option<PathBuf>,
    allow_downgrade: bool,
) -> Result<(), String> {
    if let Some(tarball) = from {
        return install_from(&tarball).await;
    }

    // Phase one is the host's own user's. Under sudo this process's data dir
    // is root's (no host there), and whose it *should* be is a guess — the
    // sudoer's for a user unit, the service user's for `--system --user
    // fletch` — that a wrong answer turns into the wrong database backed up
    // while the right one is migrated unbacked. So: not a guess, a refusal.
    if let Some(sudoer) = service::sudoer() {
        return Err(format!(
            "`update` downloads, verifies and backs up as the user the host runs as, and sudo \
             is only for the final swap.\nRun `fletch-host update` as that user first — as \
             {}, or e.g. `sudo -u fletch fletch-host update` for a `--system --user fletch` \
             install. It prints the one sudo command that installs the verified file.",
            sudoer.name
        ));
    }

    let client = reqwest::Client::builder()
        .user_agent(UA)
        .build()
        .map_err(|e| format!("cannot build an HTTP client: {e}"))?;

    let api = match &version {
        Some(version) => format!(
            "https://api.github.com/repos/{REPO}/releases/tags/v{}",
            version.trim_start_matches('v')
        ),
        None => format!("https://api.github.com/repos/{REPO}/releases/latest"),
    };
    let release = get_json(&client, &api).await?;
    let assets = pick_assets(&release);

    if check {
        let available = release["tag_name"]
            .as_str()
            .unwrap_or("?")
            .trim_start_matches('v');
        println!("current:   {CURRENT_VERSION}");
        println!("available: {available}");
        println!("target:    {TARGET}");
        match assets {
            Ok(_) => println!(
                "asset:     {} (with .sha256 and .sig)",
                asset_name(available)
            ),
            Err(e) => println!("asset:     unavailable — {e}"),
        }
        println!();
        if available == CURRENT_VERSION {
            println!("Already up to date.");
        } else {
            println!("Run `fletch-host update` to install {available}.");
        }
        return Ok(());
    }

    let assets = assets?;
    if assets.version == CURRENT_VERSION && version.is_none() {
        println!("Already running {CURRENT_VERSION}; nothing to do.");
        println!("(`fletch-host update {CURRENT_VERSION}` reinstalls it anyway.)");
        return Ok(());
    }
    // A named version can be an older one, and going back is the one update
    // that can leave the host unable to start at all. Checked before anything
    // is downloaded.
    if version.is_some() {
        refuse_downgrade(&assets.version, allow_downgrade)?;
    }

    // Where the three downloads live: this user's own data dir, written as this
    // user. Kept after the swap: they are the evidence for what is now
    // installed, re-verifying them needs no network, and the tarball is what
    // phase two installs when the swap needs root.
    let dir = data_dir.join("updates").join(&assets.version);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;

    println!("Downloading {} ...", asset_name(&assets.version));
    let tarball_path = dir.join(asset_name(&assets.version));
    let sha_path = dir.join(format!("{}.sha256", asset_name(&assets.version)));
    let sig_path = dir.join(format!("{}.sig", asset_name(&assets.version)));
    let tarball = fetch_to(&client, &assets.tarball, &tarball_path).await?;
    let sha_text = String::from_utf8(fetch_to(&client, &assets.sha256, &sha_path).await?)
        .map_err(|_| "the .sha256 file is not text".to_string())?;
    let sig_text = String::from_utf8(fetch_to(&client, &assets.sig, &sig_path).await?)
        .map_err(|_| "the .sig file is not text".to_string())?;

    verify_sha256(&tarball, &sha_text)?;
    println!("checksum ok");
    verify_signature(&tarball, &sig_text, UPDATER_PUBKEY)?;
    println!("signature ok (minisign, the desktop updater's key)");

    if let Some(backup) = backup_db(data_dir, CURRENT_VERSION)? {
        println!("database copied to {}", backup.display());
    }

    let exe = current_exe()?;
    match ensure_replaceable(&exe) {
        Ok(_) => {
            let binary = extract_binary(&tarball)?;
            swap(&exe, &binary)?;
            println!("installed {} at {}", assets.version, exe.display());
            restart(&exe, Some(data_dir), None).await;
        }
        // The one step that needs root, handed to root as a single command
        // over a file this user has just verified. Not an error: everything
        // phase one is for has happened.
        Err(why) => {
            println!();
            println!("{why}");
            println!("The download is verified and the database is copied aside. To install it:");
            println!();
            // This binary by its absolute path, not `fletch-host`: root's PATH
            // may resolve that name to another installation, and it is *this*
            // one — the one the service runs — that is to be replaced.
            println!(
                "    sudo {} update --from {}",
                shell_quote(&exe),
                shell_quote(&tarball_path)
            );
        }
    }
    Ok(())
}

/// What going back costs, said in full wherever a downgrade is refused: it is
/// the database, not the binary, that does not come back on its own.
const DOWNGRADE_RISK: &str = "An older fletch-host cannot open a database a newer one has \
     migrated — it refuses with \"database schema is newer than this app version\" and will not \
     start. Restore the copy the newer version made before it migrated \
     (<data-dir>/backups/data.db.<version>.bak) over the database first, or the host stays down.";

/// Refuse a named version older than this build's, and refuse one this cannot
/// order at all, unless `--allow-downgrade` says the caller means it.
fn refuse_downgrade(requested: &str, allow_downgrade: bool) -> Result<(), String> {
    if allow_downgrade {
        return Ok(());
    }
    let (Some(wanted), Some(current)) = (version_parts(requested), version_parts(CURRENT_VERSION))
    else {
        return Err(format!(
            "cannot tell whether {requested} is older than this build's {CURRENT_VERSION}: one \
             of them is not a plain <major>.<minor>.<patch> (a pre-release, say), and a guess \
             here is a host that will not start.\n{DOWNGRADE_RISK}\nPass --allow-downgrade to \
             install it anyway."
        ));
    };
    if wanted < current {
        return Err(format!(
            "{requested} is older than this build's {CURRENT_VERSION}; refusing.\n\
             {DOWNGRADE_RISK}\nPass --allow-downgrade to install it anyway."
        ));
    }
    Ok(())
}

/// A release number as three comparable numbers: `0.8.10` → `[0, 8, 10]`.
///
/// Fletch's versions are plain dotted triples (one product version across four
/// manifests, asserted equal by CI), and ordering two of them is the only
/// comparison anything here needs — so it is ten lines rather than a
/// dependency this package, which has no committed lockfile, would have to
/// fetch. Anything that is not three numbers — a pre-release, a git
/// description, a typo — is `None`, and the caller refuses instead of ordering
/// it wrongly.
fn version_parts(version: &str) -> Option<[u64; 3]> {
    let mut fields = version.split('.');
    let mut parts = [0u64; 3];
    for part in parts.iter_mut() {
        *part = fields.next()?.parse().ok()?;
    }
    if fields.next().is_some() {
        return None;
    }
    Some(parts)
}

/// A path as one shell word, for a command the user copies: single-quoted,
/// with any `'` in it closed, escaped and reopened.
fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

/// Phase two, the one `sudo` runs: install a tarball phase one downloaded.
///
/// Verified again here — this process trusts the signature, not the caller or
/// the file's location. It reads that file and its `.sig`, writes only its own
/// executable's directory, and restarts the service. It opens no data dir, so
/// there is nothing for root to leave behind in one, and no symlink in one it
/// could be made to follow.
async fn install_from(tarball_path: &Path) -> Result<(), String> {
    let name = tarball_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{} is not a file name", tarball_path.display()))?;
    let version = version_in_asset_name(name).ok_or_else(|| {
        format!(
            "{name} is not a fletch-host tarball for this build: expected \
             fletch-host-<version>-{TARGET}.tar.gz"
        )
    })?;
    let tarball = std::fs::read(tarball_path)
        .map_err(|e| format!("cannot read {}: {e}", tarball_path.display()))?;
    let sig_path = tarball_path.with_file_name(format!("{name}.sig"));
    let sig_text = std::fs::read_to_string(&sig_path).map_err(|e| {
        format!(
            "cannot read {}: {e}\nThe .sig `fletch-host update` downloaded must sit next to \
             the tarball.",
            sig_path.display()
        )
    })?;
    verify_signature(&tarball, &sig_text, UPDATER_PUBKEY)?;
    println!("signature ok (minisign, the desktop updater's key)");

    let exe = current_exe()?;
    ensure_replaceable(&exe)?;
    let binary = extract_binary(&tarball)?;
    swap(&exe, &binary)?;
    println!("installed {version} at {}", exe.display());

    restart(&exe, None, service::sudoer().as_ref()).await;
    Ok(())
}

/// The running binary's path, resolved through symlinks so the `rename` lands
/// on the real file rather than on a link into it.
pub fn current_exe() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find my own path: {e}"))?;
    exe.canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", exe.display()))
}

async fn get_json(client: &reqwest::Client, url: &str) -> Result<Value, String> {
    let response = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("cannot reach {url}: {e}"))?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("no such release: {url}"));
    }
    if !status.is_success() {
        return Err(format!("{url} answered {status}"));
    }
    response
        .json()
        .await
        .map_err(|e| format!("{url} did not answer JSON: {e}"))
}

/// Download to `path` and hand back the bytes.
async fn fetch_to(client: &reqwest::Client, url: &str, path: &Path) -> Result<Vec<u8>, String> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("cannot reach {url}: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("{url} answered {}", response.status()));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("cannot read {url}: {e}"))?
        .to_vec();
    std::fs::write(path, &bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    println!("  {} ({} bytes)", path.display(), bytes.len());
    Ok(bytes)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The digest out of a `sha256sum` line: `<hex>  <filename>`.
pub fn expected_sha256(text: &str) -> Result<String, String> {
    let digest = text
        .split_whitespace()
        .next()
        .ok_or("the .sha256 file is empty")?
        .to_ascii_lowercase();
    if digest.len() != 64 || !digest.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "the .sha256 file does not start with a digest: {digest}"
        ));
    }
    Ok(digest)
}

pub fn verify_sha256(bytes: &[u8], sha_text: &str) -> Result<(), String> {
    let expected = expected_sha256(sha_text)?;
    let actual = sha256_hex(bytes);
    if actual != expected {
        return Err(format!(
            "the download does not match its checksum (expected {expected}, got {actual}); \
             nothing was installed"
        ));
    }
    Ok(())
}

/// Verify a minisign signature the way the desktop updater does.
///
/// Both the key and the signature arrive base64-encoded — that is the form
/// `tauri signer` writes and the form `tauri.conf.json` stores — but a
/// `.sig` written by `minisign`/`rsign2` directly is the plain file, so either
/// is accepted and neither is guessed at: a minisign file always starts with
/// its untrusted comment.
pub fn verify_signature(data: &[u8], sig: &str, pubkey: &str) -> Result<(), String> {
    let pubkey_text = minisign_text(pubkey).map_err(|e| format!("bad updater public key: {e}"))?;
    let public_key = minisign_verify::PublicKey::decode(pubkey_text.trim())
        .map_err(|e| format!("bad updater public key: {e}"))?;
    let sig_text = minisign_text(sig).map_err(|e| format!("bad signature file: {e}"))?;
    let signature = minisign_verify::Signature::decode(sig_text.trim())
        .map_err(|e| format!("bad signature file: {e}"))?;
    // `allow_legacy: true` is the desktop updater's own argument
    // (tauri-plugin-updater), so this trusts exactly what that key signs and
    // nothing else.
    public_key.verify(data, &signature, true).map_err(|e| {
        format!("the download is not signed by the Fletch release key ({e}); nothing was installed")
    })
}

fn minisign_text(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.starts_with("untrusted comment:") {
        return Ok(raw.to_string());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(raw)
        .map_err(|e| format!("not base64: {e}"))?;
    String::from_utf8(bytes).map_err(|_| "not a minisign file".to_string())
}

/// Copy the SQLite database aside, named after the version being replaced.
///
/// A plain copy, and deliberately so: the WAL of a *running* host is not in
/// it, so the snapshot is only as good as the host being stopped — which is
/// what the restart note at the end of an update is about. It exists so that a
/// new binary's migrations have something to go back to, not as a backup
/// system.
pub fn backup_db(data_dir: &Path, from_version: &str) -> Result<Option<PathBuf>, String> {
    let db = data_dir.join(fletch_core::database::DB_FILENAME);
    if !db.exists() {
        return Ok(None);
    }
    let dir = data_dir.join("backups");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let backup = dir.join(format!(
        "{}.{from_version}.bak",
        fletch_core::database::DB_FILENAME
    ));
    std::fs::copy(&db, &backup)
        .map_err(|e| format!("cannot copy {} to {}: {e}", db.display(), backup.display()))?;
    Ok(Some(backup))
}

/// The `fletch-host` file out of a release tarball (a flat archive: the binary,
/// the README and `packaging/`).
pub fn extract_binary(tarball: &[u8]) -> Result<Vec<u8>, String> {
    let gz = flate2::read::GzDecoder::new(std::io::Cursor::new(tarball));
    let mut archive = tar::Archive::new(gz);
    for entry in archive
        .entries()
        .map_err(|e| format!("the download is not a tar archive: {e}"))?
    {
        let mut entry = entry.map_err(|e| format!("cannot read the archive: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("cannot read the archive: {e}"))?
            .to_path_buf();
        if path.file_name().and_then(|n| n.to_str()) != Some(BINARY_IN_TARBALL) {
            continue;
        }
        // `./fletch-host` or `fletch-host`, not `packaging/fletch-host`: the
        // tarball carries a `packaging/` directory, and tar normalises away a
        // leading `./`, so count the components that name something.
        let depth = path
            .components()
            .filter(|part| !matches!(part, std::path::Component::CurDir))
            .count();
        if depth != 1 {
            continue;
        }
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|e| format!("cannot read {BINARY_IN_TARBALL} from the archive: {e}"))?;
        return Ok(bytes);
    }
    Err(format!(
        "the archive has no {BINARY_IN_TARBALL} in it; this is not a host release"
    ))
}

/// Write the new binary beside the old one and `rename` it over.
///
/// `rename` within a directory is atomic, so there is no moment at which the
/// path does not resolve to a whole binary. A process already running from the
/// old inode keeps running it, which is why an installed service is restarted
/// afterwards and a bare `serve` is only told to restart.
pub fn swap(exe: &Path, binary: &[u8]) -> Result<(), String> {
    use std::io::Write as _;

    let dir = ensure_replaceable(exe)?;
    let staged = dir.join(format!(
        "{}.new",
        exe.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(BINARY_IN_TARBALL)
    ));
    // `remove_file` unlinks a symlink rather than following it, and
    // `create_new` (O_EXCL) refuses a path that exists — so a `.new` someone
    // planted here, a link out of this directory say, is neither written
    // through nor renamed over the binary. The permissions go through the
    // handle for the same reason.
    let _ = std::fs::remove_file(&staged);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)
        .map_err(|e| format!("cannot create {}: {e}", staged.display()))?;
    let written = file.write_all(binary).and_then(|()| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o755))?;
        }
        file.sync_all()
    });
    drop(file);
    if let Err(e) = written {
        let _ = std::fs::remove_file(&staged);
        return Err(format!("cannot write {}: {e}", staged.display()));
    }
    std::fs::rename(&staged, exe).map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        format!("cannot replace {}: {e}", exe.display())
    })
}

/// The directory `exe` sits in, once it is known to be one this process may
/// replace a file in: writable, and — when this process is root — root's own
/// and nobody else's to write, since a directory another user can write is one
/// they can stage a rename in, and root renaming their file over its own
/// binary is the takeover the signature check exists to prevent.
pub fn ensure_replaceable(exe: &Path) -> Result<PathBuf, String> {
    let dir = exe
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", exe.display()))?;
    let me = nix::unistd::Uid::effective();
    if me.is_root() && !directory_is_private_to(dir, me.as_raw())? {
        return Err(format!(
            "{} is not root's alone (owned by root, writable by nobody else), so root will \
             not replace a binary in it. Install fletch-host into a root-owned directory \
             such as /usr/local/bin, or into your own and update it as yourself.",
            dir.display()
        ));
    }
    ensure_writable(dir)?;
    Ok(dir.to_path_buf())
}

/// Owned by `uid`, and writable by no one else (no group or other write bit).
fn directory_is_private_to(dir: &Path, uid: u32) -> Result<bool, String> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(dir).map_err(|e| format!("cannot stat {}: {e}", dir.display()))?;
    Ok(meta.uid() == uid && meta.mode() & 0o022 == 0)
}

/// The fact, stated once; what to do about it depends on who is asking (phase
/// one prints the `sudo` command, phase two has no further way out).
fn ensure_writable(dir: &Path) -> Result<(), String> {
    let probe = dir.join(".fletch-host-update-probe");
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(e) => Err(format!(
            "{} is not writable by this user ({e}).",
            dir.display()
        )),
    }
}

/// Restart whatever is running the old binary, or say what has to happen.
///
/// Never kills a `serve` it did not start: a host with agents mid-run is not
/// something an update command gets to end. Failures here are printed, not
/// returned — the new binary is already installed, and an update that says
/// "installed, but the restart failed" is more useful than one that looks like
/// it failed entirely.
/// Only a unit that runs *this* binary — the file just replaced — is
/// restarted. A system unit and a user unit can coexist on one machine (an
/// operator's `--system --user fletch` beside their own), and restarting by
/// precedence would bounce the wrong one either way round. Each unit names
/// its executable (`ExecStart=` / `ProgramArguments`), so the match is on
/// that path, and every unit that matches is restarted. The sudoer's user
/// unit is restarted as them (root has no `systemctl --user` session of
/// theirs), their launchd agent in their `gui/<uid>` domain.
///
/// `data_dir` is where a bare `serve`'s admin socket would be; phase two has
/// none to offer (it opens no data dir) and so only knows about installed
/// units.
async fn restart(exe: &Path, data_dir: Option<&Path>, sudoer: Option<&nix::unistd::User>) {
    let mut restarted = false;

    let system_unit = Path::new("/etc/systemd/system").join(service::UNIT_NAME);
    if unit_runs(&system_unit, exe, service::systemd_unit_exec) {
        report(service::run("systemctl", &["restart", service::UNIT_NAME]));
        restarted = true;
    }

    let user_unit_dir = match sudoer {
        Some(user) => Some(service::systemd_user_dir_in(&user.dir)),
        None => service::systemd_user_dir().ok(),
    };
    if let Some(dir) = user_unit_dir {
        if unit_runs(
            &dir.join(service::UNIT_NAME),
            exe,
            service::systemd_unit_exec,
        ) {
            report(match sudoer {
                Some(user) => {
                    let runtime_dir = format!("XDG_RUNTIME_DIR=/run/user/{}", user.uid.as_raw());
                    service::run(
                        "sudo",
                        &[
                            "-u",
                            &user.name,
                            &runtime_dir,
                            "systemctl",
                            "--user",
                            "restart",
                            service::UNIT_NAME,
                        ],
                    )
                }
                None => service::run("systemctl", &["--user", "restart", service::UNIT_NAME]),
            });
            restarted = true;
        }
    }

    let (plist, uid) = match sudoer {
        Some(user) => (
            Some(service::launchd_plist_path_in(&user.dir)),
            user.uid.as_raw(),
        ),
        None => (
            service::launchd_plist_path().ok(),
            nix::unistd::getuid().as_raw(),
        ),
    };
    if let Some(plist) = plist {
        if unit_runs(&plist, exe, service::launchd_plist_exec) {
            let target = format!("gui/{uid}/{}", service::LAUNCHD_LABEL);
            report(service::run("launchctl", &["kickstart", "-k", &target]));
            restarted = true;
        }
    }

    if restarted {
        return;
    }
    let Some(data_dir) = data_dir else {
        println!();
        println!(
            "No installed service found. If a host is running from the old binary, restart it \
             to pick this version up."
        );
        return;
    };
    if admin::call(data_dir, "status", json!({})).await.is_ok() {
        println!();
        println!(
            "A host is still serving from the old binary. Stop it (Ctrl-C, or SIGTERM) and run \
             `fletch-host serve` again to pick this version up; `fletch-host service install` \
             would let updates restart it for you."
        );
    }
}

/// Whether the unit file at `unit`, if there is one, runs `exe`. `exec_of`
/// reads the executable out of the unit's text. Both sides are compared
/// resolved: the unit was written from a resolved path, and a symlink put in
/// front of the binary later must not hide the match.
fn unit_runs(unit: &Path, exe: &Path, exec_of: fn(&str) -> Option<PathBuf>) -> bool {
    let Ok(text) = std::fs::read_to_string(unit) else {
        return false;
    };
    let Some(unit_exe) = exec_of(&text) else {
        return false;
    };
    let resolve = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    resolve(&unit_exe) == resolve(exe)
}

fn report(result: Result<(), String>) {
    if let Err(e) = result {
        println!("the new binary is installed, but the restart failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway minisign key, generated for this test with the same
    /// `tauri signer` the release job uses, over the payload below. It is not
    /// the project's key: this proves the verification path, and
    /// `pubkey_matches_tauri_conf` proves the key the path is pointed at.
    const TEST_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDk3RDExNTk4QkI1ODM0REMKUldUY05GaTdtQlhSbDBxcEFleFE0R0taUHc0Q2xaWG5aWDlsYjhjNFMrZlNYcmxWMFJkbFZ3SkQK";
    const TEST_SIG: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVUY05GaTdtQlhSbDVuMVpSRTRMYmxzQlJzd2dVekRVMHJvT2J6YlprMFdDeS9qNzhRR1NCc3FZK2pMZUJEM2RsWitMSHZzRkdFSTYwc1kvOGl3enhaTnhjMlpteG00T0EwPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzg5NzUxODg5CWZpbGU6cGF5bG9hZC5iaW4KQkF4TFFGS2FXNDI4OElMbUx5ZVpyUUNpUTlaTWJZOEVFWHh6SlJ2ck4rV05uRUVhN3F0azE2RUw5YUd1VlVBUkE1YytoQ3RQcTBJNjVLZ09zWTlZQ0E9PQo=";
    const TEST_PAYLOAD: &[u8] = b"fletch-host update signature test vector\n";
    const TEST_PAYLOAD_SHA256: &str =
        "9cdd16a1b603ad942322d5672adefcd45c99005b457e78f1fe3367abddefb804";

    fn release(assets: &[&str]) -> Value {
        json!({
            "tag_name": "v0.9.0",
            "assets": assets.iter().map(|name| json!({
                "name": name,
                "browser_download_url": format!("https://example.invalid/{name}"),
            })).collect::<Vec<_>>(),
        })
    }

    #[test]
    fn asset_is_named_after_this_build() {
        assert_eq!(
            asset_name("0.9.0"),
            format!("fletch-host-0.9.0-{TARGET}.tar.gz")
        );
        // `build.rs` filled it in, so it is a triple and not the empty string.
        assert!(TARGET.contains('-'), "{TARGET}");
    }

    #[test]
    fn picks_this_targets_three_files() {
        let tarball = asset_name("0.9.0");
        let picked = pick_assets(&release(&[
            "fletch-host-0.9.0-some-other-target.tar.gz",
            &tarball,
            &format!("{tarball}.sha256"),
            &format!("{tarball}.sig"),
        ]))
        .unwrap();
        assert_eq!(
            picked,
            Assets {
                version: "0.9.0".to_string(),
                tarball: format!("https://example.invalid/{tarball}"),
                sha256: format!("https://example.invalid/{tarball}.sha256"),
                sig: format!("https://example.invalid/{tarball}.sig"),
            }
        );
    }

    #[test]
    fn a_release_without_a_signature_is_refused() {
        let tarball = asset_name("0.9.0");
        let e = pick_assets(&release(&[&tarball, &format!("{tarball}.sha256")])).unwrap_err();
        assert!(e.contains("without its .sig"), "{e}");

        let e = pick_assets(&release(&[&tarball, &format!("{tarball}.sig")])).unwrap_err();
        assert!(e.contains("without its .sha256"), "{e}");
    }

    #[test]
    fn a_release_without_this_target_lists_what_it_has() {
        let e =
            pick_assets(&release(&["fletch-host-0.9.0-riscv64-unknown-none.tar.gz"])).unwrap_err();
        assert!(e.contains("riscv64-unknown-none"), "{e}");
        assert!(e.contains(&asset_name("0.9.0")), "{e}");
    }

    #[test]
    fn checksums_are_compared_against_the_sha256sum_line() {
        let line = format!("{TEST_PAYLOAD_SHA256}  fletch-host-0.9.0-x86_64.tar.gz\n");
        assert_eq!(sha256_hex(TEST_PAYLOAD), TEST_PAYLOAD_SHA256);
        verify_sha256(TEST_PAYLOAD, &line).unwrap();
        // BSD `shasum` and GNU `sha256sum` differ in spacing, and an
        // uppercase digest is still a digest.
        verify_sha256(TEST_PAYLOAD, &TEST_PAYLOAD_SHA256.to_ascii_uppercase()).unwrap();

        let e = verify_sha256(b"something else", &line).unwrap_err();
        assert!(e.contains("does not match its checksum"), "{e}");
        assert!(expected_sha256("not a digest at all").is_err());
        assert!(expected_sha256("").is_err());
    }

    #[test]
    fn signatures_verify_and_tampering_does_not() {
        verify_signature(TEST_PAYLOAD, TEST_SIG, TEST_PUBKEY).unwrap();

        let e = verify_signature(b"tampered payload\n", TEST_SIG, TEST_PUBKEY).unwrap_err();
        assert!(e.contains("not signed by the Fletch release key"), "{e}");

        // The project's own key did not sign the test vector, and a signature
        // from the wrong key is a refusal, not a warning.
        let e = verify_signature(TEST_PAYLOAD, TEST_SIG, UPDATER_PUBKEY).unwrap_err();
        assert!(e.contains("not signed by the Fletch release key"), "{e}");

        let e = verify_signature(TEST_PAYLOAD, "not a signature", TEST_PUBKEY).unwrap_err();
        assert!(e.contains("bad signature file"), "{e}");
    }

    #[test]
    fn a_plain_minisign_file_verifies_too() {
        // `minisign -Sm` and `rsign2` write the file itself, not base64 of it.
        let plain = String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(TEST_SIG)
                .unwrap(),
        )
        .unwrap();
        assert!(plain.starts_with("untrusted comment:"));
        verify_signature(TEST_PAYLOAD, &plain, TEST_PUBKEY).unwrap();
    }

    /// The key this binary trusts and the key the desktop trusts are the same
    /// key, or one of them is trusting a key nobody signs with.
    #[test]
    fn pubkey_matches_tauri_conf() {
        const CONF: &str = include_str!("../../../src-tauri/tauri.conf.json");
        let conf: Value = serde_json::from_str(CONF).expect("tauri.conf.json is JSON");
        assert_eq!(
            conf["plugins"]["updater"]["pubkey"].as_str(),
            Some(UPDATER_PUBKEY),
            "update::UPDATER_PUBKEY has drifted from plugins.updater.pubkey in tauri.conf.json"
        );
    }

    #[test]
    fn the_database_is_copied_aside_under_the_old_version() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(backup_db(dir.path(), "0.7.32").unwrap(), None);

        let db = dir.path().join(fletch_core::database::DB_FILENAME);
        std::fs::write(&db, b"sqlite pretend").unwrap();
        let backup = backup_db(dir.path(), "0.7.32").unwrap().unwrap();
        assert_eq!(
            backup,
            dir.path()
                .join("backups")
                .join(format!("{}.0.7.32.bak", fletch_core::database::DB_FILENAME))
        );
        assert_eq!(std::fs::read(&backup).unwrap(), b"sqlite pretend");
        // The original stays where the engine expects it.
        assert!(db.exists());
    }

    #[test]
    fn the_binary_comes_out_of_a_flat_tarball() {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let add = |builder: &mut tar::Builder<&mut Vec<u8>>, name: &str, body: &[u8]| {
                let mut header = tar::Header::new_gnu();
                header.set_size(body.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                builder.append_data(&mut header, name, body).unwrap();
            };
            // A decoy at a deeper level: `packaging/` travels in the tarball.
            add(&mut builder, "./packaging/fletch-host", b"not the binary");
            add(&mut builder, "./README.md", b"# fletch-host");
            add(&mut builder, "./fletch-host", b"\x7fELF pretend");
            builder.finish().unwrap();
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, &tar_bytes).unwrap();
        let tarball = gz.finish().unwrap();

        assert_eq!(extract_binary(&tarball).unwrap(), b"\x7fELF pretend");
        assert!(extract_binary(b"not a tarball at all").is_err());
    }

    #[test]
    fn the_swap_replaces_the_file_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("fletch-host");
        std::fs::write(&exe, b"old").unwrap();
        swap(&exe, b"new").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        // Nothing staged is left behind.
        assert!(!dir.path().join("fletch-host.new").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&exe).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
    }

    #[test]
    fn the_preflight_names_the_directory_the_swap_will_use() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("fletch-host");
        assert_eq!(ensure_replaceable(&exe).unwrap(), dir.path());
        assert!(
            std::fs::read_dir(dir.path()).unwrap().next().is_none(),
            "the probe file must not be left behind"
        );
    }

    #[test]
    fn an_unwritable_directory_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        }
        let e = swap(&dir.path().join("fletch-host"), b"new").unwrap_err();
        assert!(e.contains("not writable"), "{e}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    /// A `.new` planted as a symlink out of the directory is unlinked, not
    /// written through and not renamed over the binary — the file it pointed
    /// at is untouched and the binary is the new bytes.
    #[test]
    fn a_planted_staging_symlink_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("fletch-host");
        std::fs::write(&exe, b"old").unwrap();
        let victim = dir.path().join("victim");
        std::fs::write(&victim, b"precious").unwrap();
        std::os::unix::fs::symlink(&victim, dir.path().join("fletch-host.new")).unwrap();

        swap(&exe, b"new").unwrap();

        assert_eq!(std::fs::read(&victim).unwrap(), b"precious");
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert!(
            std::fs::symlink_metadata(&exe)
                .unwrap()
                .file_type()
                .is_file(),
            "the binary must be a regular file, not the planted link"
        );
    }

    /// What root demands of the directory before it will replace a file in
    /// it. Exercised for this user's uid: the predicate is the same.
    #[test]
    fn a_private_directory_is_owned_and_writable_by_its_owner_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let me = nix::unistd::getuid().as_raw();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(directory_is_private_to(dir.path(), me).unwrap());
        assert!(!directory_is_private_to(dir.path(), me + 1).unwrap());
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o775)).unwrap();
        assert!(!directory_is_private_to(dir.path(), me).unwrap());
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(!directory_is_private_to(dir.path(), me).unwrap());
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    /// A unit is "this binary's" when its executable resolves to the file
    /// just replaced — through a symlink too — and not otherwise.
    #[test]
    fn a_unit_is_restarted_only_when_it_runs_the_replaced_binary() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("fletch-host");
        std::fs::write(&exe, b"bin").unwrap();
        let other = dir.path().join("other-fletch-host");
        std::fs::write(&other, b"bin").unwrap();
        let link = dir.path().join("link-to-exe");
        std::os::unix::fs::symlink(&exe, &link).unwrap();

        let unit = dir.path().join("fletch-host.service");
        std::fs::write(
            &unit,
            format!("[Service]\nExecStart={} serve --port 1\n", exe.display()),
        )
        .unwrap();
        assert!(unit_runs(&unit, &exe, service::systemd_unit_exec));
        assert!(unit_runs(&unit, &link, service::systemd_unit_exec));
        assert!(!unit_runs(&unit, &other, service::systemd_unit_exec));
        assert!(!unit_runs(
            &dir.path().join("missing.service"),
            &exe,
            service::systemd_unit_exec
        ));

        let plist = dir.path().join("com.fletch.host.plist");
        std::fs::write(
            &plist,
            format!(
                "<plist><dict><key>ProgramArguments</key><array><string>{}</string>\
                 <string>serve</string></array></dict></plist>",
                other.display()
            ),
        )
        .unwrap();
        assert!(unit_runs(&plist, &other, service::launchd_plist_exec));
        assert!(!unit_runs(&plist, &exe, service::launchd_plist_exec));
    }

    #[test]
    fn the_printed_command_quotes_its_paths_as_shell_words() {
        assert_eq!(
            shell_quote(Path::new("/usr/local/bin/fletch-host")),
            "'/usr/local/bin/fletch-host'"
        );
        assert_eq!(
            shell_quote(Path::new("/Users/me/Application Support/x.tar.gz")),
            "'/Users/me/Application Support/x.tar.gz'"
        );
        assert_eq!(shell_quote(Path::new("/it's/here")), "'/it'\\''s/here'");
    }

    #[test]
    fn versions_order_as_numbers_and_only_as_numbers() {
        assert_eq!(version_parts("0.8.0"), Some([0, 8, 0]));
        // Not string order: 10 is after 9, and 0.8.0 is after 0.10.0's opposite.
        assert!(version_parts("0.8.10").unwrap() > version_parts("0.8.9").unwrap());
        assert!(version_parts("0.10.0").unwrap() > version_parts("0.8.0").unwrap());
        assert!(version_parts("1.0.0").unwrap() > version_parts("0.99.99").unwrap());
        // Anything that is not three plain numbers has no order here.
        for odd in ["0.9.0-rc.1", "0.9", "0.9.0.1", "v0.9.0", "", "0.9.x"] {
            assert_eq!(version_parts(odd), None, "{odd}");
        }
        // This build's own version is one of the orderable ones, or every
        // comparison below degrades into the "cannot tell" refusal.
        assert!(
            version_parts(CURRENT_VERSION).is_some(),
            "{CURRENT_VERSION}"
        );
    }

    /// Going back is refused, and the refusal says what it would cost, because
    /// the binary comes back and the migrated database does not.
    #[test]
    fn an_older_version_is_refused_unless_the_caller_asks_for_it() {
        let e = refuse_downgrade("0.0.1", false).unwrap_err();
        assert!(e.contains("older than this build's"), "{e}");
        assert!(e.contains("schema is newer"), "{e}");
        assert!(e.contains("--allow-downgrade"), "{e}");
        refuse_downgrade("0.0.1", true).unwrap();

        // Reinstalling this version, or installing a newer one, is not a
        // downgrade.
        refuse_downgrade(CURRENT_VERSION, false).unwrap();
        refuse_downgrade("9999.0.0", false).unwrap();

        // A version this cannot order is refused rather than guessed at.
        let e = refuse_downgrade("0.9.0-rc.1", false).unwrap_err();
        assert!(e.contains("cannot tell"), "{e}");
        refuse_downgrade("0.9.0-rc.1", true).unwrap();
    }

    #[test]
    fn the_version_comes_out_of_this_builds_asset_name_only() {
        assert_eq!(version_in_asset_name(&asset_name("0.7.32")), Some("0.7.32"));
        assert_eq!(
            version_in_asset_name("fletch-host-0.7.32-riscv64gc-unknown-none.tar.gz"),
            None
        );
        assert_eq!(
            version_in_asset_name(&format!("fletch-host--{TARGET}.tar.gz")),
            None
        );
        assert_eq!(version_in_asset_name("fletch-host.tar.gz"), None);
        assert_eq!(
            version_in_asset_name(&format!("not-a-host-1.0-{TARGET}.tar.gz")),
            None
        );
    }
}
