//! Detecting the code editors and terminals installed on the user's machine and
//! opening an agent's checkout in one. Detection is real — we resolve each
//! tool's CLI on the login-shell PATH and, on macOS, look for its `.app` bundle
//! — so the launcher only ever offers tools that are actually present.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::bin_resolve;
use crate::error::{Error, Result};

/// Whether an entry is a code editor or a terminal — drives grouping (and the
/// glyph fallback) in the picker.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    Editor,
    Terminal,
}

/// A known tool plus how to find and launch it.
struct KnownEditor {
    /// Stable id shared with the frontend (drives the launcher tile).
    id: &'static str,
    label: &'static str,
    kind: Kind,
    /// CLI launcher names to try on PATH, first match wins (e.g. `code`).
    clis: &'static [&'static str],
    /// macOS `.app` name (without the extension), used both to detect the app
    /// and to launch it by name via LaunchServices. Only read on macOS.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    mac_app: Option<&'static str>,
    /// The CLI whose `--remote ssh-remote+<host>` opens a folder over ssh —
    /// the VS Code family, with the Remote-SSH extension. Those are the only
    /// tools that can open a checkout kept inside a box.
    remote_cli: Option<&'static str>,
}

const fn editor(
    id: &'static str,
    label: &'static str,
    clis: &'static [&'static str],
    mac_app: &'static str,
) -> KnownEditor {
    KnownEditor {
        id,
        label,
        kind: Kind::Editor,
        clis,
        mac_app: Some(mac_app),
        remote_cli: None,
    }
}

/// An editor whose first CLI also opens folders over ssh.
const fn remote_editor(
    id: &'static str,
    label: &'static str,
    clis: &'static [&'static str],
    mac_app: &'static str,
) -> KnownEditor {
    KnownEditor {
        remote_cli: Some(clis[0]),
        ..editor(id, label, clis, mac_app)
    }
}

const fn terminal(
    id: &'static str,
    label: &'static str,
    clis: &'static [&'static str],
    mac_app: &'static str,
) -> KnownEditor {
    KnownEditor {
        id,
        label,
        kind: Kind::Terminal,
        clis,
        mac_app: Some(mac_app),
        remote_cli: None,
    }
}

/// The tools we know how to detect + open, in picker order (editors first,
/// then terminals). Detection filters this to what's actually installed.
const KNOWN: &[KnownEditor] = &[
    // ── editors ──
    remote_editor("cursor", "Cursor", &["cursor"], "Cursor"),
    remote_editor("vscode", "VS Code", &["code"], "Visual Studio Code"),
    remote_editor(
        "vscode-insiders",
        "VS Code Insiders",
        &["code-insiders"],
        "Visual Studio Code - Insiders",
    ),
    // Microsoft's Remote-SSH extension is licensed to Microsoft's builds only.
    editor("vscodium", "VSCodium", &["codium"], "VSCodium"),
    remote_editor("windsurf", "Windsurf", &["windsurf"], "Windsurf"),
    editor("zed", "Zed", &["zed"], "Zed"),
    editor("sublime", "Sublime Text", &["subl"], "Sublime Text"),
    editor("nova", "Nova", &["nova"], "Nova"),
    editor("bbedit", "BBEdit", &["bbedit"], "BBEdit"),
    editor("textmate", "TextMate", &["mate"], "TextMate"),
    editor("macvim", "MacVim", &["mvim"], "MacVim"),
    editor("neovide", "Neovide", &["neovide"], "Neovide"),
    editor("xcode", "Xcode", &["xed"], "Xcode"),
    editor("intellij", "IntelliJ IDEA", &["idea"], "IntelliJ IDEA"),
    editor("webstorm", "WebStorm", &["webstorm"], "WebStorm"),
    editor("pycharm", "PyCharm", &["pycharm"], "PyCharm"),
    editor("goland", "GoLand", &["goland"], "GoLand"),
    editor("phpstorm", "PhpStorm", &["phpstorm"], "PhpStorm"),
    editor("rubymine", "RubyMine", &["rubymine"], "RubyMine"),
    editor("clion", "CLion", &["clion"], "CLion"),
    editor("rider", "Rider", &["rider"], "Rider"),
    editor(
        "androidstudio",
        "Android Studio",
        &["studio"],
        "Android Studio",
    ),
    // ── terminals ── (all open a checkout folder via `open -a`)
    terminal("terminal", "Terminal", &[], "Terminal"),
    terminal("iterm", "iTerm", &[], "iTerm"),
    terminal("warp", "Warp", &[], "Warp"),
    terminal("ghostty", "Ghostty", &[], "Ghostty"),
    terminal("wezterm", "WezTerm", &["wezterm"], "WezTerm"),
    terminal("kitty", "kitty", &["kitty"], "kitty"),
];

const TERMINAL_ID: &str = "terminal";

#[derive(Serialize)]
pub struct DetectedEditor {
    pub id: String,
    pub label: String,
    pub kind: Kind,
    /// Can open a checkout that lives in a box (see `open_remote`).
    pub remote: bool,
}

/// Editors + terminals installed on this machine, in picker order. On macOS the
/// system Terminal is guaranteed present so the launcher is never empty there.
pub fn detect() -> Vec<DetectedEditor> {
    let home = dirs::home_dir();
    let mut found: Vec<DetectedEditor> = KNOWN
        .iter()
        .filter(|ed| is_available(ed, home.as_deref()))
        .map(DetectedEditor::from)
        .collect();
    if cfg!(target_os = "macos") && !found.iter().any(|e| e.id == TERMINAL_ID) {
        found.push(DetectedEditor {
            id: TERMINAL_ID.into(),
            label: "Terminal".into(),
            kind: Kind::Terminal,
            remote: false,
        });
    }
    found
}

impl From<&KnownEditor> for DetectedEditor {
    fn from(ed: &KnownEditor) -> Self {
        DetectedEditor {
            id: ed.id.into(),
            label: ed.label.into(),
            kind: ed.kind,
            remote: ed.remote_cli.is_some(),
        }
    }
}

/// Open `checkout` as it exists on the ssh host `host` — a box's alias in the
/// user's ssh config — in the tool identified by `editor_id`. The editor
/// connects itself and installs its server in the guest on first use; nothing
/// runs here beyond the launch. The CLI is the one inside the app's own
/// bundle when it is there: `open -a` cannot pass `--remote` to an app that
/// is already running, and a `code` on PATH may belong to another editor.
pub fn open_remote(editor_id: &str, host: &str, checkout: &Path) -> Result<()> {
    let ed = KNOWN
        .iter()
        .find(|e| e.id == editor_id)
        .ok_or_else(|| Error::Other(format!("unknown editor: {editor_id}")))?;
    let Some(cli_name) = ed.remote_cli else {
        return Err(Error::Other(format!(
            "{} cannot open a workspace that runs in a box; it has no remote mode. Use VS Code, Cursor or Windsurf, or the workspace's Files panel.",
            ed.label
        )));
    };
    let home = dirs::home_dir();
    let cli = bundled_cli(ed, cli_name, home.as_deref())
        .or_else(|| {
            home.as_deref()
                .and_then(|h| bin_resolve::resolve_bin(cli_name, h))
                .map(PathBuf::from)
        })
        .ok_or_else(|| Error::Other(format!("{} is not available", ed.label)))?;
    let mut cmd = Command::new(cli);
    bin_resolve::apply_login_shell_env(&mut cmd);
    cmd.arg("--remote")
        .arg(format!("ssh-remote+{host}"))
        .arg(checkout);
    spawn(cmd, ed.label)
}

/// The CLI a VS Code-family app ships at `<App>.app/Contents/Resources/app/bin/<cli>`.
#[cfg(target_os = "macos")]
fn bundled_cli(ed: &KnownEditor, cli: &str, home: Option<&Path>) -> Option<PathBuf> {
    let app = ed.mac_app?;
    app_dirs(home)
        .into_iter()
        .map(|d| d.join(format!("{app}.app/Contents/Resources/app/bin/{cli}")))
        .find(|p| p.is_file())
}

#[cfg(not(target_os = "macos"))]
fn bundled_cli(_ed: &KnownEditor, _cli: &str, _home: Option<&Path>) -> Option<PathBuf> {
    None
}

/// Open `checkout` in the tool identified by `editor_id`.
///
/// On macOS we launch the specific `.app` by name via LaunchServices — an
/// unambiguous target, so a shared/hijacked CLI can't open the wrong editor
/// (Cursor, a VS Code fork, symlinks its own binary onto the `code` command;
/// launching VS Code through that CLI would open Cursor). The CLI is only a
/// fallback: when the app-launch fails, or off macOS.
pub fn open(editor_id: &str, checkout: &Path) -> Result<()> {
    let ed = KNOWN
        .iter()
        .find(|e| e.id == editor_id)
        .ok_or_else(|| Error::Other(format!("unknown editor: {editor_id}")))?;

    #[cfg(target_os = "macos")]
    if let Some(app) = ed.mac_app {
        // `-a <app>` targets that exact app; `open` exits nonzero if it isn't
        // registered, in which case we fall through to the CLI.
        let launched = Command::new("open")
            .args(["-a", app])
            .arg(checkout)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if launched {
            return Ok(());
        }
    }

    let home = dirs::home_dir();
    if let Some(cli) = home
        .as_deref()
        .and_then(|h| ed.clis.iter().find_map(|c| bin_resolve::resolve_bin(c, h)))
    {
        let mut cmd = Command::new(&cli);
        bin_resolve::apply_login_shell_env(&mut cmd);
        cmd.arg(checkout);
        return spawn(cmd, ed.label);
    }

    Err(Error::Other(format!("{} is not available", ed.label)))
}

/// Whether a tool is installed: its CLI resolves on PATH, or (macOS) its `.app`
/// sits in one of the standard Applications folders.
fn is_available(ed: &KnownEditor, home: Option<&Path>) -> bool {
    let cli = home.is_some_and(|h| {
        ed.clis
            .iter()
            .any(|c| bin_resolve::resolve_bin(c, h).is_some())
    });
    cli || mac_app_installed(ed, home)
}

#[cfg(target_os = "macos")]
fn mac_app_installed(ed: &KnownEditor, home: Option<&Path>) -> bool {
    let Some(app) = ed.mac_app else { return false };
    let bundle = format!("{app}.app");
    app_dirs(home).iter().any(|d| d.join(&bundle).exists())
}

/// The GUI-app locations plus Utilities (where the system Terminal lives) —
/// modern macOS keeps built-ins under /System/Applications.
#[cfg(target_os = "macos")]
fn app_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/Applications/Utilities"),
        PathBuf::from("/System/Applications"),
        PathBuf::from("/System/Applications/Utilities"),
    ];
    if let Some(h) = home {
        dirs.push(h.join("Applications"));
    }
    dirs
}

#[cfg(not(target_os = "macos"))]
fn mac_app_installed(_ed: &KnownEditor, _home: Option<&Path>) -> bool {
    false
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
        assert!(
            matches!(err, Error::Other(_)),
            "unknown editor id must be an error"
        );
    }

    #[test]
    fn open_remote_refuses_tools_without_a_remote_mode() {
        let err = open_remote("xcode", "fletch-a1", Path::new("/tmp")).unwrap_err();
        assert!(err.to_string().contains("Xcode"), "{err}");
        let err = open_remote("terminal", "fletch-a1", Path::new("/tmp")).unwrap_err();
        assert!(err.to_string().contains("Terminal"), "{err}");
    }

    #[test]
    fn remote_editors_are_the_vscode_family() {
        let remote: Vec<&str> = KNOWN
            .iter()
            .filter(|e| e.remote_cli.is_some())
            .map(|e| e.id)
            .collect();
        assert_eq!(remote, ["cursor", "vscode", "vscode-insiders", "windsurf"]);
        // The remote CLI is also a detection CLI, so a remote-capable editor
        // found on PATH is launchable through the same binary.
        for ed in KNOWN {
            if let Some(cli) = ed.remote_cli {
                assert!(ed.clis.contains(&cli), "{}", ed.id);
            }
        }
    }

    #[test]
    fn every_known_tool_has_a_launch_path() {
        // A registry entry the launcher can never open (no CLI and, on macOS,
        // no .app) would be a silent dead end — guard against that.
        for ed in KNOWN {
            assert!(
                !ed.clis.is_empty() || ed.mac_app.is_some(),
                "{} has no way to be launched",
                ed.id,
            );
        }
    }

    #[test]
    fn editor_ids_are_unique() {
        let mut ids: Vec<&str> = KNOWN.iter().map(|e| e.id).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "duplicate editor id in KNOWN");
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
