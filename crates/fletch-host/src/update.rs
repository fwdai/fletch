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

/// `update`, the whole subcommand.
pub async fn run(data_dir: &Path, version: Option<String>, check: bool) -> Result<(), String> {
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

    // Where the three downloads live. Kept after the swap: they are the
    // evidence for what is now installed, and re-verifying them needs no
    // network.
    let dir = data_dir.join("updates").join(&assets.version);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;

    println!("Downloading {} ...", asset_name(&assets.version));
    let tarball = fetch_to(
        &client,
        &assets.tarball,
        &dir.join(asset_name(&assets.version)),
    )
    .await?;
    let sha_text = String::from_utf8(
        fetch_to(
            &client,
            &assets.sha256,
            &dir.join(format!("{}.sha256", asset_name(&assets.version))),
        )
        .await?,
    )
    .map_err(|_| "the .sha256 file is not text".to_string())?;
    let sig_text = String::from_utf8(
        fetch_to(
            &client,
            &assets.sig,
            &dir.join(format!("{}.sig", asset_name(&assets.version))),
        )
        .await?,
    )
    .map_err(|_| "the .sig file is not text".to_string())?;

    verify_sha256(&tarball, &sha_text)?;
    println!("checksum ok");
    verify_signature(&tarball, &sig_text, UPDATER_PUBKEY)?;
    println!("signature ok (minisign, the desktop updater's key)");

    if let Some(backup) = backup_db(data_dir, CURRENT_VERSION)? {
        println!("database copied to {}", backup.display());
    }

    let exe = current_exe()?;
    let binary = extract_binary(&tarball)?;
    swap(&exe, &binary)?;
    println!("installed {} at {}", assets.version, exe.display());

    restart(data_dir).await;
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
    let dir = exe
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", exe.display()))?;
    ensure_writable(dir)?;
    let staged = dir.join(format!(
        "{}.new",
        exe.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(BINARY_IN_TARBALL)
    ));
    std::fs::write(&staged, binary)
        .map_err(|e| format!("cannot write {}: {e}", staged.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("cannot chmod {}: {e}", staged.display()))?;
    }
    std::fs::rename(&staged, exe).map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        format!("cannot replace {}: {e}", exe.display())
    })
}

/// Refuse before downloading nothing useful: an update that cannot land is
/// better said now, with the two ways out.
fn ensure_writable(dir: &Path) -> Result<(), String> {
    let probe = dir.join(".fletch-host-update-probe");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(e) => Err(format!(
            "{} is not writable ({e}).\nRe-run this with sudo, or install fletch-host somewhere \
             that is yours (`install -Dm755 fletch-host ~/.local/bin/`) and update that copy.",
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
async fn restart(data_dir: &Path) {
    if let Ok(path) = service::systemd_user_dir() {
        if path.join(service::UNIT_NAME).exists() {
            report(service::run(
                "systemctl",
                &["--user", "restart", service::UNIT_NAME],
            ));
            return;
        }
    }
    if Path::new("/etc/systemd/system")
        .join(service::UNIT_NAME)
        .exists()
    {
        report(service::run("systemctl", &["restart", service::UNIT_NAME]));
        return;
    }
    if let Ok(plist) = service::launchd_plist_path() {
        if plist.exists() {
            let target = format!(
                "gui/{}/{}",
                nix::unistd::getuid().as_raw(),
                service::LAUNCHD_LABEL
            );
            report(service::run("launchctl", &["kickstart", "-k", &target]));
            return;
        }
    }
    if admin::call(data_dir, "status", json!({})).await.is_ok() {
        println!();
        println!(
            "A host is still serving from the old binary. Stop it (Ctrl-C, or SIGTERM) and run \
             `fletch-host serve` again to pick this version up; `fletch-host service install` \
             would let updates restart it for you."
        );
    }
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
    fn an_unwritable_directory_is_refused_with_the_way_out() {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        }
        let e = swap(&dir.path().join("fletch-host"), b"new").unwrap_err();
        assert!(e.contains("not writable"), "{e}");
        assert!(e.contains("sudo"), "{e}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
}
