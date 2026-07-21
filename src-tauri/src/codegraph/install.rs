//! Silent, background install of the pinned codegraph release bundle into
//! Fletch's tools dir. Mirrors `agent_install.rs`'s process handling but with no
//! UI: there is no user-facing progress, and every failure is logged and
//! swallowed by callers (indexing simply stays off until a later retry).
//!
//! The vendor `install.sh` honors `CODEGRAPH_VERSION`, `CODEGRAPH_INSTALL_DIR`,
//! and `CODEGRAPH_BIN_DIR`, which is exactly how we redirect it away from its
//! defaults (`~/.codegraph`, `~/.local/bin`) and into `~/.fletch/tools/`. We
//! never invoke the tool's own `install`/`uninstall` subcommands — those rewrite
//! `~/.claude.json` and other global agent configs.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::error::{Error, Result};

/// Pinned codegraph release tag. Verified against the repo's latest release at
/// implementation time; bump deliberately when adopting a newer bundle.
pub const CODEGRAPH_VERSION: &str = "v1.4.1";

/// Where the vendor installer script is fetched from.
const INSTALL_SCRIPT_URL: &str =
    "https://raw.githubusercontent.com/colbymchenry/codegraph/main/install.sh";

/// Records the version last installed by us, alongside the bundle, so
/// `ensure_installed` can skip a reinstall when the pinned version already
/// matches. Lives in the install dir (not app-data).
const STAMP_FILE: &str = ".fletch-version";

/// Ceiling on a single installer run — the bundle is tens of MB and finishes
/// well under a minute, so this only stops a hung download from holding the
/// install lock forever.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Serializes installs process-wide: startup, the toggle, and concurrent spawns
/// can all call `ensure_installed`, and two installers racing over the same dir
/// would corrupt the bundle.
fn install_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Ensure the pinned codegraph bundle is installed under `~/.fletch/tools/` and
/// return the absolute path to its binary. Returns immediately when the binary
/// already exists and the version stamp matches the pin; otherwise downloads and
/// runs the vendor `install.sh` (redirected via env vars), then writes the
/// stamp. Serialized so concurrent callers don't race the same install dir.
pub async fn ensure_installed() -> Result<PathBuf> {
    let bin = super::bin_path()?;
    if bin.exists() && stamp_matches(&super::install_dir()?).await {
        return Ok(bin);
    }

    let _guard = install_lock().lock().await;
    // Re-check under the lock: a racing caller may have finished the install
    // while we waited.
    if bin.exists() && stamp_matches(&super::install_dir()?).await {
        return Ok(bin);
    }

    let install_dir = super::install_dir()?;
    let bin_dir = super::bin_dir()?;
    tokio::fs::create_dir_all(&install_dir)
        .await
        .map_err(|e| Error::Other(format!("create codegraph install dir: {e}")))?;

    let script = install_dir.join("install.sh");
    download_script(INSTALL_SCRIPT_URL, &script).await?;

    tracing::info!(version = CODEGRAPH_VERSION, "installing codegraph bundle");
    run_installer(&script, &install_dir, &bin_dir).await?;

    if !bin.exists() {
        return Err(Error::Other(format!(
            "codegraph installer finished but no binary at {}",
            bin.display()
        )));
    }
    write_stamp(&install_dir).await;
    tracing::info!(path = %bin.display(), "codegraph installed");
    Ok(bin)
}

/// True when the install dir's version stamp equals the pinned version.
async fn stamp_matches(install_dir: &std::path::Path) -> bool {
    match tokio::fs::read_to_string(install_dir.join(STAMP_FILE)).await {
        Ok(s) => s.trim() == CODEGRAPH_VERSION,
        Err(_) => false,
    }
}

/// Record the pinned version so later runs skip the reinstall. Best-effort — a
/// missing stamp only costs a redundant reinstall next launch.
async fn write_stamp(install_dir: &std::path::Path) {
    if let Err(e) =
        tokio::fs::write(install_dir.join(STAMP_FILE), CODEGRAPH_VERSION.as_bytes()).await
    {
        tracing::warn!(error = %e, "failed to write codegraph version stamp; continuing");
    }
}

/// Download the installer script to `dest` (reqwest — already a dep).
async fn download_script(url: &str, dest: &std::path::Path) -> Result<()> {
    let body = reqwest::get(url)
        .await
        .map_err(|e| Error::Other(format!("download codegraph install.sh: {e}")))?
        .error_for_status()
        .map_err(|e| Error::Other(format!("download codegraph install.sh: {e}")))?
        .bytes()
        .await
        .map_err(|e| Error::Other(format!("read codegraph install.sh: {e}")))?;
    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(|e| Error::Other(format!("create {}: {e}", dest.display())))?;
    file.write_all(&body)
        .await
        .map_err(|e| Error::Other(format!("write {}: {e}", dest.display())))?;
    file.flush()
        .await
        .map_err(|e| Error::Other(format!("flush {}: {e}", dest.display())))?;
    Ok(())
}

/// Run `sh <script>` with the redirect env vars set, streaming nothing but
/// capturing the last output line for the error message. Follows
/// `agent_install.rs`'s process handling (login-shell env, kill_on_drop, a
/// bounded timeout).
async fn run_installer(
    script: &std::path::Path,
    install_dir: &std::path::Path,
    bin_dir: &std::path::Path,
) -> Result<()> {
    let mut command = tokio::process::Command::new("sh");
    command.arg(script);
    // GUI processes inherit launchd's sparse env; the installer expects a normal
    // user environment (PATH, HOME, proxy vars) to fetch the bundle.
    if let Some(env) = crate::bin_resolve::login_shell_env() {
        command.envs(env);
    }
    command
        .env("CODEGRAPH_VERSION", CODEGRAPH_VERSION)
        .env("CODEGRAPH_INSTALL_DIR", install_dir)
        .env("CODEGRAPH_BIN_DIR", bin_dir)
        // Never let the installer chatter to a daemon or phone home.
        .env("CODEGRAPH_TELEMETRY", "0")
        .env("CODEGRAPH_NO_DAEMON", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let output = tokio::time::timeout(INSTALL_TIMEOUT, command.output())
        .await
        .map_err(|_| Error::Other("codegraph installer timed out".into()))?
        .map_err(|e| Error::Other(format!("spawn codegraph installer: {e}")))?;

    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let last = stderr.lines().rev().find(|l| !l.trim().is_empty());
    Err(Error::Other(match last {
        Some(msg) => format!("codegraph installer exited with {}: {msg}", output.status),
        None => format!("codegraph installer exited with {}", output.status),
    }))
}
