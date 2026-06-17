//! Locate external CLI binaries the way a GUI app must on macOS.
//!
//! A Tauri app launched from Finder / Dock / Spotlight inherits launchd's
//! minimal PATH (`/usr/bin:/bin:/usr/sbin:/sbin`) rather than the user's
//! shell PATH. Binaries in `/usr/bin` (like `git`) resolve fine, but anything
//! installed by Homebrew (`/opt/homebrew/bin`) or a version manager — `gh`,
//! `claude`, `codex`, … — does not, so `Command::new("gh")` fails with
//! ENOENT ("No such file or directory"). Resolve the absolute path first:
//! current PATH → the user's login-shell PATH → the usual install dirs.

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    let out = Command::new("/bin/zsh")
        .args(["-lc", "print -rn -- $PATH"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        None
    } else {
        Some(path)
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
}
