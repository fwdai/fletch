//! Detecting the code editors installed on the user's machine and opening an
//! agent's worktree in one. Detection is real — we resolve each editor's CLI on
//! the login-shell PATH and, on macOS, look for its `.app` bundle — so the
//! launcher only ever offers editors that are actually present.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::bin_resolve;
use crate::error::{Error, Result};

/// A known editor plus how to find and launch it.
struct KnownEditor {
    /// Stable id shared with the frontend (drives the launcher tile).
    id: &'static str,
    label: &'static str,
    /// CLI launcher names to try on PATH, first match wins (e.g. `code`).
    clis: &'static [&'static str],
    /// macOS `.app` name (without the extension) for the LaunchServices
    /// `open -a` fallback when the CLI isn't installed.
    mac_app: Option<&'static str>,
}

/// The editors we know how to detect + open, in the order the picker shows
/// them. Terminal is appended separately (it's always available on macOS).
const KNOWN: &[KnownEditor] = &[
    KnownEditor { id: "cursor", label: "Cursor", clis: &["cursor"], mac_app: Some("Cursor") },
    KnownEditor {
        id: "vscode",
        label: "VS Code",
        clis: &["code"],
        mac_app: Some("Visual Studio Code"),
    },
    KnownEditor {
        id: "windsurf",
        label: "Windsurf",
        clis: &["windsurf"],
        mac_app: Some("Windsurf"),
    },
    KnownEditor { id: "zed", label: "Zed", clis: &["zed"], mac_app: Some("Zed") },
    KnownEditor {
        id: "sublime",
        label: "Sublime Text",
        clis: &["subl"],
        mac_app: Some("Sublime Text"),
    },
];

const TERMINAL_ID: &str = "terminal";

#[derive(Serialize)]
pub struct DetectedEditor {
    pub id: String,
    pub label: String,
}

/// Editors installed on this machine, in picker order. On macOS the system
/// Terminal is always included last, so the launcher is never empty there.
pub fn detect() -> Vec<DetectedEditor> {
    let home = dirs::home_dir();
    let mut found: Vec<DetectedEditor> = KNOWN
        .iter()
        .filter(|ed| is_available(ed, home.as_deref()))
        .map(|ed| DetectedEditor { id: ed.id.into(), label: ed.label.into() })
        .collect();
    if cfg!(target_os = "macos") {
        found.push(DetectedEditor { id: TERMINAL_ID.into(), label: "Terminal".into() });
    }
    found
}

/// Open `worktree` in the editor identified by `editor_id`. Prefers the CLI
/// launcher (opens the folder as a workspace); on macOS falls back to launching
/// the `.app` via LaunchServices when only the app — not its CLI — is present.
pub fn open(editor_id: &str, worktree: &Path) -> Result<()> {
    if editor_id == TERMINAL_ID {
        return open_terminal(worktree);
    }
    let ed = KNOWN
        .iter()
        .find(|e| e.id == editor_id)
        .ok_or_else(|| Error::Other(format!("unknown editor: {editor_id}")))?;
    let home = dirs::home_dir();

    if let Some(cli) = home
        .as_deref()
        .and_then(|h| ed.clis.iter().find_map(|c| bin_resolve::resolve_bin(c, h)))
    {
        let mut cmd = Command::new(&cli);
        bin_resolve::apply_login_shell_env(&mut cmd);
        cmd.arg(worktree);
        return spawn(cmd, ed.label);
    }

    #[cfg(target_os = "macos")]
    if let Some(app) = ed.mac_app {
        let mut cmd = Command::new("open");
        cmd.args(["-a", app]).arg(worktree);
        return spawn(cmd, ed.label);
    }

    Err(Error::Other(format!("{} is not available", ed.label)))
}

/// Whether an editor is installed: its CLI resolves on PATH, or (macOS) its
/// `.app` sits in one of the standard Applications folders.
fn is_available(ed: &KnownEditor, home: Option<&Path>) -> bool {
    let cli = home.is_some_and(|h| ed.clis.iter().any(|c| bin_resolve::resolve_bin(c, h).is_some()));
    cli || mac_app_installed(ed, home)
}

#[cfg(target_os = "macos")]
fn mac_app_installed(ed: &KnownEditor, home: Option<&Path>) -> bool {
    let Some(app) = ed.mac_app else { return false };
    let bundle = format!("{app}.app");
    let mut dirs = vec![PathBuf::from("/Applications"), PathBuf::from("/System/Applications")];
    if let Some(h) = home {
        dirs.push(h.join("Applications"));
    }
    dirs.iter().any(|d| d.join(&bundle).exists())
}

#[cfg(not(target_os = "macos"))]
fn mac_app_installed(_ed: &KnownEditor, _home: Option<&Path>) -> bool {
    false
}

/// Open the worktree in the system terminal.
fn open_terminal(worktree: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("open");
        cmd.args(["-a", "Terminal"]).arg(worktree);
        return spawn(cmd, "Terminal");
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = worktree;
        Err(Error::Other("opening a terminal is only supported on macOS".into()))
    }
}

fn spawn(mut cmd: Command, label: &str) -> Result<()> {
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| Error::Other(format!("open {label}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_rejects_unknown_editor() {
        let err = open("definitely-not-an-editor", Path::new("/tmp")).unwrap_err();
        assert!(matches!(err, Error::Other(_)), "unknown editor id must be an error");
    }

    #[test]
    fn every_known_editor_has_a_launch_path() {
        // A registry entry the launcher can never open (no CLI and, on macOS,
        // no .app) would be a silent dead end — guard against that.
        for ed in KNOWN {
            assert!(
                !ed.clis.is_empty() || ed.mac_app.is_some(),
                "editor {} has no way to be launched",
                ed.id,
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn detect_always_offers_terminal_on_macos() {
        assert!(
            detect().iter().any(|e| e.id == "terminal"),
            "macOS should always offer the Terminal option",
        );
    }
}
