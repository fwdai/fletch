//! Locate external CLI binaries the way a GUI app must on macOS.
//!
//! A Tauri app launched from Finder / Dock / Spotlight inherits launchd's
//! minimal PATH (`/usr/bin:/bin:/usr/sbin:/sbin`) rather than the user's
//! shell PATH. Binaries in `/usr/bin` (like `git`) resolve fine, but anything
//! installed by Homebrew (`/opt/homebrew/bin`) or a version manager — `gh`,
//! `claude`, `codex`, … — does not, so `Command::new("gh")` fails with
//! ENOENT ("No such file or directory"). Resolve the absolute path first:
//! current PATH → the user's login shell → the usual install dirs.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Resolve `name` to an absolute path, or `None` if it can't be found.
///
/// Tries, in order: the current PATH, the user's login shell (catches
/// nvm / fnm / volta / homebrew setups the GUI process's bare PATH misses),
/// then the usual install dirs under `home` and the Homebrew prefixes.
pub fn resolve_bin(name: &str, home: &Path) -> Option<String> {
    if let Some(path) = command_in_path(name) {
        return Some(path);
    }
    if let Some(path) = command_from_login_shell(name) {
        return Some(path);
    }
    common_bin_paths(name, home)
        .into_iter()
        .find(|candidate| candidate.is_file())
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

fn command_in_path(name: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
        .map(|path| path.to_string_lossy().into_owned())
}

fn command_from_login_shell(name: &str) -> Option<String> {
    let script = format!("command -v {name}");
    let out = Command::new("/bin/zsh")
        .args(["-lc", &script])
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

#[cfg(test)]
mod tests {
    use super::*;

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
        std::fs::write(bin_dir.join(name), b"#!/bin/sh\n").unwrap();

        assert_eq!(
            resolve_bin(name, home.path()).as_deref(),
            Some(bin_dir.join(name).to_string_lossy().as_ref()),
            "a binary not on PATH should resolve via the common install dirs",
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
