//! Provider binary resolution and `--version` probing.

use std::path::PathBuf;
use std::process::Command;

use crate::error::{Error, Result};

use super::capabilities::{provider_bin_label, PER_TURN_AGENTS};

/// The probed CLI version for a provider (`v1.2.3`), memoized per process so the
/// `--version` subprocess runs at most once per provider. Stamped onto
/// session_records at ingest so read-time normalizers can branch by version
/// when a vendor format changes. `None` if the binary is missing/unparseable.
pub fn cached_provider_version(provider: &str) -> Option<String> {
    static CACHE: std::sync::OnceLock<
        parking_lot::Mutex<std::collections::HashMap<String, Option<String>>>,
    > = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| parking_lot::Mutex::new(std::collections::HashMap::new()));
    if let Some(v) = cache.lock().get(provider) {
        return v.clone();
    }
    let version = provider_bin_label(provider).and_then(|(bin, label)| {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        resolve_agent_bin(provider, bin, label, &home)
            .ok()
            .and_then(|p| probe_version(&p))
    });
    cache.lock().insert(provider.to_string(), version.clone());
    version
}

/// Locate an agent CLI by name: PATH first, then the user's login shell
/// (catches nvm / fnm / volta / homebrew setups the GUI process's bare
/// PATH misses), then the usual install dirs. `label` is the
/// human-facing product name used only in the not-found error.
pub fn resolve_agent_bin(
    agent_id: &str,
    name: &str,
    label: &str,
    home: &std::path::Path,
) -> Result<String> {
    // A user-set custom path wins over PATH discovery. If it no longer points
    // at an executable we surface a clear error rather than silently falling
    // back to a different binary off PATH — the user chose this one explicitly.
    if let Some(result) = crate::bin_resolve::resolve_agent_override(agent_id, home) {
        return result.map_err(|path| {
            Error::Other(format!(
                "The custom binary path for {label} is not executable: {path}"
            ))
        });
    }
    crate::bin_resolve::resolve_bin(name, home).ok_or_else(|| {
        Error::Other(format!(
            "Could not find the `{name}` executable. Install {label} or make it available on PATH."
        ))
    })
}

// ── Version probing ───────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct ProviderProbe {
    pub id: String,
    pub version: Option<String>,
    pub path: Option<String>,
}

#[derive(serde::Serialize)]
pub struct BinValidation {
    /// The path is an executable regular file (after `~` expansion).
    pub executable: bool,
    /// The version `<path> --version` reported, if it ran and parsed.
    pub version: Option<String>,
}

/// Pre-flight a user-entered custom binary path before it's saved as an
/// override: expand a leading `~`, confirm it's an executable file, and probe
/// `--version` when it is. Powers the providers settings UI's inline feedback.
pub fn validate_bin(path: &str) -> BinValidation {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let expanded = crate::bin_resolve::expand_tilde(path, &home);
    let executable = crate::bin_resolve::is_executable_path(&expanded);
    let version = if executable {
        probe_version(&expanded.to_string_lossy())
    } else {
        None
    };
    BinValidation {
        executable,
        version,
    }
}

#[derive(serde::Serialize)]
pub struct ToolStatus {
    pub installed: bool,
    pub version: Option<String>,
    pub path: Option<String>,
    /// `"system"` or `"portable"` for git (see `git_dist`); `None` for plain
    /// PATH-resolved tools.
    pub source: Option<String>,
}

/// Resolve a plain CLI on PATH and probe its `--version`. Used by the
/// first-run readiness check for required tools that aren't agent providers.
/// `git` goes through `git_dist` instead: presence isn't enough there (the
/// macOS CLT shim exists but doesn't run) and a portable install counts.
pub fn check_cli(name: &str) -> ToolStatus {
    if name == "git" {
        return crate::git_dist::tool_status();
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let path = crate::bin_resolve::resolve_bin(name, &home);
    let version = path.as_deref().and_then(probe_version);
    ToolStatus {
        installed: path.is_some(),
        version,
        path,
        source: None,
    }
}

/// Probe every known provider in parallel and return their resolved path +
/// version string. Missing/uninstalled providers return `None` for both fields;
/// the frontend falls back to the hardcoded defaults in that case.
///
/// Every `--version` here is bounded (see [`VERSION_TIMEOUT`]), so one wedged
/// CLI — or a custom binary path pointed at something that never exits — costs
/// a slow probe rather than a caller that never returns.
pub async fn probe_all_providers() -> Vec<ProviderProbe> {
    probe_providers(true).await
}

/// The same set, resolved but never *run*: no `--version`, no subprocess at
/// all. For callers that only need to know whether a provider's CLI is on this
/// machine (`fletch-host status`), where executing six vendor binaries to
/// answer that would be both wasteful and a way to hang.
pub async fn resolve_all_providers() -> Vec<ProviderProbe> {
    probe_providers(false).await
}

async fn probe_providers(with_version: bool) -> Vec<ProviderProbe> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));

    // (id, bin_name, human_label)
    let mut targets: Vec<(&str, &str, &str)> = vec![("claude", "claude", "Claude Code")];
    for d in PER_TURN_AGENTS {
        targets.push((d.id, d.bin, d.label));
    }

    let mut handles = Vec::new();
    for (id, bin, label) in targets {
        let home = home.clone();
        let id = id.to_string();
        let bin = bin.to_string();
        let label = label.to_string();
        handles.push(tokio::spawn(async move {
            let resolving = id.clone();
            // Resolution touches the filesystem (and, once per process, the
            // login shell), so it stays on the blocking pool.
            let path = tokio::task::spawn_blocking(move || {
                resolve_agent_bin(&resolving, &bin, &label, &home).ok()
            })
            .await
            .ok()
            .flatten();
            let version = match path.as_deref() {
                Some(path) if with_version => probe_version_bounded(path).await,
                _ => None,
            };
            ProviderProbe { id, version, path }
        }));
    }

    let mut results = Vec::new();
    for handle in handles {
        if let Ok(probe) = handle.await {
            results.push(probe);
        }
    }
    results
}

/// Ceiling on a single `--version`. Every CLI here answers in well under a
/// second; a probe that reaches this is a binary that is wedged, prompting, or
/// not the CLI the user thought they pointed us at.
const VERSION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// [`probe_version`] with a deadline, and a child that dies with it:
/// `kill_on_drop` means the timeout drops the `Command` future and reaps the
/// process rather than leaving it running with nobody waiting on it. A probe
/// that times out reports no version, exactly like one that printed nothing.
async fn probe_version_bounded(bin: &str) -> Option<String> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.arg("--version").kill_on_drop(true);
    crate::bin_resolve::apply_login_shell_env(cmd.as_std_mut());
    let out = tokio::time::timeout(VERSION_TIMEOUT, cmd.output())
        .await
        .ok()?
        .ok()?;
    parse_semver(&version_text(&out.stdout, &out.stderr))
}

/// Run `<bin> --version` and extract the first semver-like token from stdout
/// (or stderr as fallback). Returns `None` if the binary errors or emits no
/// recognisable version.
fn probe_version(bin: &str) -> Option<String> {
    let mut cmd = Command::new(bin);
    cmd.arg("--version");
    crate::bin_resolve::apply_login_shell_env(&mut cmd);
    let out = cmd.output().ok()?;
    parse_semver(&version_text(&out.stdout, &out.stderr))
}

/// What a CLI said its version was: stdout, or stderr when stdout was silent.
fn version_text(stdout: &[u8], stderr: &[u8]) -> String {
    let said = if stdout.is_empty() { stderr } else { stdout };
    String::from_utf8_lossy(said).into_owned()
}

/// Extract the first `N.N[.N[.N]]` token from arbitrary version output.
/// Strips a leading `v` from each word before testing so `v1.0.42` and
/// `1.0.42` both match. Returns the token with a `v` prefix.
pub fn parse_semver(s: &str) -> Option<String> {
    for word in s.split_whitespace() {
        let word = word.trim_start_matches('v');
        // Accept anything that is purely digit-and-dot with at least one dot.
        if word.contains('.')
            && word.chars().all(|c| c.is_ascii_digit() || c == '.')
            && !word.starts_with('.')
            && !word.ends_with('.')
        {
            return Some(format!("v{word}"));
        }
    }
    None
}
