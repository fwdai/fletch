//! Locate external CLI binaries the way a GUI app must on macOS.
//!
//! A Tauri app launched from Finder / Dock / Spotlight inherits launchd's
//! minimal PATH (`/usr/bin:/bin:/usr/sbin:/sbin`) rather than the user's
//! shell PATH. Binaries in `/usr/bin` (like `git`) resolve fine, but anything
//! installed by Homebrew (`/opt/homebrew/bin`) or a version manager — `gh`,
//! `claude`, `codex`, … — does not, so `Command::new("gh")` fails with
//! ENOENT ("No such file or directory"). Resolve the absolute path first:
//! current PATH → the user's login-shell PATH → the usual install dirs.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{OnceLock, RwLock};

/// User-set absolute binary paths, keyed by agent id (e.g. "claude",
/// "cursor"). When an agent has an entry here, it overrides PATH discovery
/// entirely (see `resolve_agent_override`) — the user pointed us at a specific
/// binary and we honor it rather than silently picking a different one off
/// PATH. Populated once at startup from the `settings` table and updated by the
/// `set_agent_bin_override` command, so resolution never needs a DB handle.
fn overrides() -> &'static RwLock<HashMap<String, String>> {
    static OVERRIDES: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();
    OVERRIDES.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Replace the entire override map. Called at startup with every
/// `agent_bin_path_*` setting and on each single-key change (the caller folds
/// the change into the full map). An empty path clears that agent's override.
pub fn set_agent_overrides(map: HashMap<String, String>) {
    *overrides().write().unwrap() = map
        .into_iter()
        .filter(|(_, path)| !path.trim().is_empty())
        .collect();
}

/// The raw override path for `agent_id`, if the user set one. Not yet
/// tilde-expanded or executability-checked — that happens in
/// `resolve_agent_override`.
pub fn agent_override(agent_id: &str) -> Option<String> {
    overrides().read().unwrap().get(agent_id).cloned()
}

/// Resolve `agent_id`'s override to an executable absolute path. Returns:
/// - `None` — no override set; caller should fall back to PATH discovery.
/// - `Some(Ok(path))` — override set and points at an executable file.
/// - `Some(Err(path))` — override set but missing / not executable. The path
///   is returned so callers can surface it (the error message, the "missing"
///   UI). We deliberately do NOT fall back to PATH here: the user chose this
///   binary explicitly, so masking a broken choice with a different one off
///   PATH would be more confusing than a clear "not executable" error.
pub fn resolve_agent_override(agent_id: &str, home: &Path) -> Option<Result<String, String>> {
    let raw = agent_override(agent_id)?;
    let expanded = expand_tilde(&raw, home);
    if is_executable(&expanded) {
        Some(Ok(expanded.to_string_lossy().into_owned()))
    } else {
        Some(Err(expanded.to_string_lossy().into_owned()))
    }
}

/// Expand a single leading `~` (or `~/…`) to `home`. Any other use of `~`
/// (e.g. `~user`) is left untouched — we only handle the common case.
pub fn expand_tilde(path: &str, home: &Path) -> PathBuf {
    if path == "~" {
        home.to_path_buf()
    } else if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(path)
    }
}

/// Whether `path` (tilde already expanded) is an executable regular file —
/// the same check resolution uses. Exposed for the `validate_agent_bin`
/// command so the UI can pre-flight a custom path before saving.
pub fn is_executable_path(path: &Path) -> bool {
    is_executable(path)
}

/// Resolve `name` to an absolute path, or `None` if it can't be found.
///
/// Tries, in order: the current PATH, the user's login-shell PATH (catches
/// nvm / fnm / volta / homebrew setups the GUI process's bare PATH misses),
/// then the usual install dirs under `home` and the Homebrew prefixes. Every
/// candidate must be an executable regular file, so a non-executable stub
/// from a partial install is skipped rather than returned (which would fail
/// with a confusing "Permission denied" at spawn time).
pub fn resolve_bin(name: &str, home: &Path) -> Option<String> {
    if let Some(path) = std::env::var_os("PATH").and_then(|p| find_in_path(name, &p)) {
        return Some(path);
    }
    if let Some(path) = login_shell_path().and_then(|p| find_in_path(name, p)) {
        return Some(path);
    }
    common_bin_paths(name, home)
        .into_iter()
        .find(|candidate| is_executable(candidate))
        .map(|candidate| candidate.to_string_lossy().into_owned())
}

/// Exported environment as seen by the user's login shell, cached once per app
/// process. Finder/Dock-launched GUI apps inherit launchd's sparse environment;
/// agent CLIs usually expect the richer shell environment where version
/// managers, API keys, and Homebrew paths are configured.
pub fn login_shell_env() -> Option<&'static HashMap<String, String>> {
    static ENV: OnceLock<Option<HashMap<String, String>>> = OnceLock::new();
    ENV.get_or_init(load_login_shell_env).as_ref()
}

/// Apply the cached login-shell environment to a std process command. Caller
/// supplied env should be layered afterwards when it must win on collision.
pub fn apply_login_shell_env(cmd: &mut Command) {
    if let Some(env) = login_shell_env() {
        for (k, v) in env {
            cmd.env(k, v);
        }
    }
}

fn common_bin_paths(name: &str, home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(format!(".local/bin/{name}")),
        home.join(format!(".npm-global/bin/{name}")),
        home.join(format!(".bun/bin/{name}")),
        PathBuf::from(format!("/opt/homebrew/bin/{name}")),
        PathBuf::from(format!("/usr/local/bin/{name}")),
    ]
}

/// Find an executable named `name` by walking a colon-separated `path`
/// (PATH-style). `name` is only ever used as a path component — never as
/// shell input — so there is no command-injection surface here.
fn find_in_path<P: AsRef<OsStr>>(name: &str, path: P) -> Option<String> {
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
        .map(|candidate| candidate.to_string_lossy().into_owned())
}

/// The PATH as seen by the user's login shell, which sources `.zprofile` /
/// `.zshrc` and thus picks up version-manager and Homebrew dirs the GUI's
/// inherited PATH lacks. We ask the shell *only* for its `$PATH` and walk it
/// ourselves — no binary name is ever interpolated into a shell command, so
/// this cannot be used for command injection.
fn login_shell_path() -> Option<String> {
    login_shell_env().and_then(|env| env.get("PATH").cloned())
}

fn load_login_shell_env() -> Option<HashMap<String, String>> {
    let out = Command::new("/bin/zsh")
        .args(["-lc", "env"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }

    let mut env = HashMap::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if k.is_empty() || matches!(k, "PWD" | "OLDPWD" | "SHLVL" | "_") {
            continue;
        }
        env.insert(k.to_string(), v.to_string());
    }

    if env.is_empty() {
        None
    } else {
        Some(env)
    }
}

/// An executable regular file the current process may run. `is_file()` alone
/// is not enough: a regular file without any execute bit would still match
/// and then fail at spawn with "Permission denied" instead of "not found".
fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_executable(path: &Path) {
        std::fs::write(path, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// The bug this module fixes: a binary that lives outside the GUI process's
    /// inherited PATH (e.g. Homebrew's `gh` under `/opt/homebrew/bin`) must
    /// still resolve via the fallback install dirs under `home`.
    #[test]
    fn resolves_binary_outside_path_via_home_dir() {
        let home = tempfile::tempdir().unwrap();
        let bin_dir = home.path().join(".local/bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        // A name that won't exist on PATH or via the login shell, so resolution
        // can only succeed through the common-dir fallback.
        let name = "quorum-fake-cli-xyz";
        write_executable(&bin_dir.join(name));

        assert_eq!(
            resolve_bin(name, home.path()).as_deref(),
            Some(bin_dir.join(name).to_string_lossy().as_ref()),
            "a binary not on PATH should resolve via the common install dirs",
        );
    }

    /// A non-executable file (e.g. a leftover stub from a partial install) is
    /// not a usable binary and must be skipped, not returned.
    #[test]
    fn skips_non_executable_file() {
        let home = tempfile::tempdir().unwrap();
        let bin_dir = home.path().join(".local/bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let name = "quorum-fake-cli-nonexec";
        let file = bin_dir.join(name);
        std::fs::write(&file, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();

        assert_eq!(
            resolve_bin(name, home.path()),
            None,
            "a non-executable file must not be resolved as a runnable binary",
        );
    }

    #[test]
    fn returns_none_when_binary_is_nowhere() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_bin("quorum-definitely-not-installed-zzz", home.path()),
            None,
        );
    }

    #[test]
    fn expand_tilde_handles_bare_home_subpath_and_absolute() {
        let home = Path::new("/Users/test");
        assert_eq!(expand_tilde("~", home), PathBuf::from("/Users/test"));
        assert_eq!(
            expand_tilde("~/bin/claude", home),
            PathBuf::from("/Users/test/bin/claude"),
        );
        // An absolute path (and a `~user` form we don't handle) is untouched.
        assert_eq!(
            expand_tilde("/opt/homebrew/bin/claude", home),
            PathBuf::from("/opt/homebrew/bin/claude"),
        );
        assert_eq!(expand_tilde("~bob/x", home), PathBuf::from("~bob/x"));
    }

    /// The override registry is process-global, so this is the ONLY test that
    /// mutates it — keeping resolution deterministic without cross-test races.
    /// It covers the two branches `resolve_agent_override` reports: an
    /// executable override wins, a non-executable one is surfaced as `Err`
    /// (never silently falling back to PATH).
    #[test]
    fn agent_override_wins_when_executable_and_errors_when_not() {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("custom-claude");
        write_executable(&bin);
        let bin_str = bin.to_string_lossy().into_owned();

        // No override set → caller falls back to PATH (None here).
        assert!(resolve_agent_override("override-test-agent", home.path()).is_none());

        set_agent_overrides(HashMap::from([
            ("override-test-agent".to_string(), bin_str.clone()),
            // Blank paths are dropped, so this clears rather than sets.
            ("blank-agent".to_string(), "   ".to_string()),
            ("missing-agent".to_string(), "/no/such/binary-xyz".to_string()),
        ]));

        assert_eq!(
            resolve_agent_override("override-test-agent", home.path()),
            Some(Ok(bin_str)),
            "an executable override must win over PATH discovery",
        );
        assert!(
            resolve_agent_override("blank-agent", home.path()).is_none(),
            "a blank override is dropped and falls back to PATH",
        );
        assert!(
            matches!(
                resolve_agent_override("missing-agent", home.path()),
                Some(Err(_)),
            ),
            "a non-executable override is reported as Err, not silently ignored",
        );

        // Tilde expansion flows through resolution too.
        set_agent_overrides(HashMap::from([(
            "tilde-agent".to_string(),
            "~/custom-claude".to_string(),
        )]));
        // home dir here is the tempdir, where custom-claude is executable.
        assert_eq!(
            resolve_agent_override("tilde-agent", home.path()),
            Some(Ok(bin.to_string_lossy().into_owned())),
            "a leading ~ in an override expands to home before the exec check",
        );

        // Leave the global registry empty for any later-running test.
        set_agent_overrides(HashMap::new());
    }
}
