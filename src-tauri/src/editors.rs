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
    /// How the tool opens a folder over ssh, for a checkout kept inside a box.
    /// Only the VS Code family has such a mode.
    remote: Option<Remote>,
}

/// A VS Code-family editor's remote mode: its first CLI takes
/// `--remote ssh-remote+<host> <path>`, resolving `<host>` through the ssh
/// config its `remote.SSH.configFile` setting names (default `~/.ssh/config`),
/// once the extension that provides the mode is installed.
struct Remote {
    /// The extension id to install when its directory is missing; `None` when
    /// the mode is built into the editor.
    extension: Option<&'static str>,
    /// `~/<dot_dir>/extensions` holds the installed extensions.
    dot_dir: &'static str,
    /// `<config dir>/<data_dir>/User/settings.json` is the user's settings.
    data_dir: &'static str,
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
        remote: None,
    }
}

const fn remote_editor(
    id: &'static str,
    label: &'static str,
    clis: &'static [&'static str],
    mac_app: &'static str,
    remote: Remote,
) -> KnownEditor {
    KnownEditor {
        remote: Some(remote),
        ..editor(id, label, clis, mac_app)
    }
}

/// Microsoft's Remote - SSH, licensed to Microsoft's builds only.
const MS_REMOTE_SSH: &str = "ms-vscode-remote.remote-ssh";

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
        remote: None,
    }
}

/// The tools we know how to detect + open, in picker order (editors first,
/// then terminals). Detection filters this to what's actually installed.
const KNOWN: &[KnownEditor] = &[
    // ── editors ──
    remote_editor(
        "cursor",
        "Cursor",
        &["cursor"],
        "Cursor",
        Remote {
            // Cursor's own fork; its marketplace substitutes it for Microsoft's.
            extension: Some("anysphere.remote-ssh"),
            dot_dir: ".cursor",
            data_dir: "Cursor",
        },
    ),
    remote_editor(
        "vscode",
        "VS Code",
        &["code"],
        "Visual Studio Code",
        Remote {
            extension: Some(MS_REMOTE_SSH),
            dot_dir: ".vscode",
            data_dir: "Code",
        },
    ),
    remote_editor(
        "vscode-insiders",
        "VS Code Insiders",
        &["code-insiders"],
        "Visual Studio Code - Insiders",
        Remote {
            extension: Some(MS_REMOTE_SSH),
            dot_dir: ".vscode-insiders",
            data_dir: "Code - Insiders",
        },
    ),
    // No licensed Remote - SSH for VSCodium.
    editor("vscodium", "VSCodium", &["codium"], "VSCodium"),
    remote_editor(
        "windsurf",
        "Windsurf",
        &["windsurf"],
        "Windsurf",
        Remote {
            // Built in; Windsurf's docs warn against installing Microsoft's.
            extension: None,
            dot_dir: ".windsurf",
            data_dir: "Windsurf",
        },
    ),
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
            remote: ed.remote.is_some(),
        }
    }
}

/// What an editor's remote mode needs on this machine before a box can be
/// opened in it (`remote_setup`).
pub struct RemoteSetup {
    /// The ssh config the editor resolves hosts through.
    pub ssh_config: PathBuf,
    /// The extension to install first, when the editor lacks it.
    pub missing_extension: Option<&'static str>,
}

/// The remote-mode editor behind `editor_id` and its CLI; an error for a tool
/// without one. The CLI is the one inside the app's own bundle when it is
/// there: `open -a` cannot pass `--remote` to an app that is already running,
/// and a `code` on PATH may belong to another editor.
fn remote_cli(editor_id: &str) -> Result<(&'static KnownEditor, &'static Remote, PathBuf)> {
    let ed = KNOWN
        .iter()
        .find(|e| e.id == editor_id)
        .ok_or_else(|| Error::Other(format!("unknown editor: {editor_id}")))?;
    let Some(remote) = &ed.remote else {
        return Err(Error::Other(format!(
            "{} cannot open a workspace that runs in a box; it has no remote mode. Use VS Code, Cursor or Windsurf, or the workspace's Files panel.",
            ed.label
        )));
    };
    let cli_name = ed.clis[0];
    let home = dirs::home_dir();
    let cli = bundled_cli(ed, cli_name, home.as_deref())
        .or_else(|| {
            home.as_deref()
                .and_then(|h| bin_resolve::resolve_bin(cli_name, h))
                .map(PathBuf::from)
        })
        .ok_or_else(|| Error::Other(format!("{} is not available", ed.label)))?;
    Ok((ed, remote, cli))
}

/// What opening a box in `editor_id` needs: the ssh config its
/// `remote.SSH.configFile` setting names (else `~/.ssh/config`), and the
/// remote extension if it is not installed.
pub fn remote_setup(editor_id: &str) -> Result<RemoteSetup> {
    let (_, remote, _) = remote_cli(editor_id)?;
    let home = dirs::home_dir().ok_or_else(|| Error::Other("no home directory".into()))?;
    let settings = dirs::config_dir()
        .map(|c| c.join(remote.data_dir).join("User").join("settings.json"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    let ssh_config = json_string_setting(&settings, "remote.SSH.configFile")
        .map(|p| expand_home(&p, &home))
        .unwrap_or_else(|| home.join(".ssh").join("config"));
    let missing_extension = remote
        .extension
        .filter(|ext| !extension_installed(&home.join(remote.dot_dir).join("extensions"), ext));
    Ok(RemoteSetup {
        ssh_config,
        missing_extension,
    })
}

/// Install `extension` into `editor_id` through its CLI, waiting for it: a
/// network fetch of a few seconds. The user agreed to the install first.
pub fn install_extension(editor_id: &str, extension: &str) -> Result<()> {
    let (ed, _, cli) = remote_cli(editor_id)?;
    let mut cmd = Command::new(cli);
    bin_resolve::apply_login_shell_env(&mut cmd);
    let out = cmd
        .args(["--install-extension", extension])
        .output()
        .map_err(|e| Error::Other(format!("install {extension} in {}: {e}", ed.label)))?;
    if out.status.success() {
        return Ok(());
    }
    Err(Error::Other(format!(
        "{} could not install {extension}: {}",
        ed.label,
        String::from_utf8_lossy(&out.stderr).trim()
    )))
}

/// Open `checkout` as it exists on the ssh host `host` — a box's alias in the
/// editor's ssh config — in the tool identified by `editor_id`. The editor
/// connects itself and installs its server in the guest on first use; nothing
/// runs here beyond the launch.
pub fn open_remote(editor_id: &str, host: &str, checkout: &Path) -> Result<()> {
    let (ed, _, cli) = remote_cli(editor_id)?;
    let mut cmd = Command::new(cli);
    bin_resolve::apply_login_shell_env(&mut cmd);
    cmd.arg("--remote")
        .arg(format!("ssh-remote+{host}"))
        .arg(checkout);
    spawn(cmd, ed.label)
}

/// Whether `extensions_dir` holds `extension`: installed extensions live in
/// `<publisher>.<name>-<version>` directories, lower-cased. The version must
/// follow, so `remote-ssh-edit` is not taken for `remote-ssh`.
fn extension_installed(extensions_dir: &Path, extension: &str) -> bool {
    let prefix = format!("{}-", extension.to_ascii_lowercase());
    std::fs::read_dir(extensions_dir)
        .map(|entries| {
            entries.flatten().any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .strip_prefix(&prefix)
                    .is_some_and(|version| version.starts_with(|c: char| c.is_ascii_digit()))
            })
        })
        .unwrap_or(false)
}

/// The non-empty string value of top-level `key` in a VS Code `settings.json`.
/// The file is JSONC — comments and trailing commas on top of JSON — so it is
/// reduced to JSON ([`jsonc_to_json`]) and parsed; a commented-out copy of the
/// key is therefore just a comment, and of duplicate keys the last wins, as in
/// the editor. An unparsable file reads as having no setting.
fn json_string_setting(settings: &str, key: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(&jsonc_to_json(settings)).ok()?;
    value
        .get(key)?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// JSONC to JSON: drops `//` and `/* */` comments outside strings and the
/// comma before a closing `}` or `]`. Strings pass through untouched, escapes
/// included, so a `//` inside one is kept.
fn jsonc_to_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    // Index in `out` of a comma that may turn out to be trailing.
    let mut pending_comma: Option<usize> = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                pending_comma = None;
                out.push(c);
                while let Some(s) = chars.next() {
                    out.push(s);
                    match s {
                        '\\' => {
                            if let Some(e) = chars.next() {
                                out.push(e);
                            }
                        }
                        '"' => break,
                        _ => {}
                    }
                }
            }
            '/' if chars.peek() == Some(&'/') => {
                for s in chars.by_ref() {
                    if s == '\n' {
                        out.push(s);
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = '\0';
                for s in chars.by_ref() {
                    if prev == '*' && s == '/' {
                        break;
                    }
                    prev = s;
                }
            }
            ',' => {
                pending_comma = Some(out.len());
                out.push(c);
            }
            '}' | ']' => {
                if let Some(at) = pending_comma.take() {
                    out.remove(at);
                }
                out.push(c);
            }
            c if c.is_whitespace() => out.push(c),
            _ => {
                pending_comma = None;
                out.push(c);
            }
        }
    }
    out
}

fn expand_home(path: &str, home: &Path) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if path == "~" => home.to_path_buf(),
        None => PathBuf::from(path),
    }
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
            .filter(|e| e.remote.is_some())
            .map(|e| e.id)
            .collect();
        assert_eq!(remote, ["cursor", "vscode", "vscode-insiders", "windsurf"]);
        // The remote launch goes through the first CLI, which must exist.
        for ed in KNOWN.iter().filter(|e| e.remote.is_some()) {
            assert!(!ed.clis.is_empty(), "{}", ed.id);
        }
    }

    #[test]
    fn extension_is_found_by_its_directory_prefix() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!extension_installed(dir.path(), MS_REMOTE_SSH));
        std::fs::create_dir(dir.path().join("ms-vscode-remote.remote-ssh-edit-0.87.0")).unwrap();
        assert!(
            !extension_installed(dir.path(), MS_REMOTE_SSH),
            "a sibling extension sharing the prefix does not count"
        );
        std::fs::create_dir(dir.path().join("ms-vscode-remote.remote-ssh-0.120.0")).unwrap();
        assert!(extension_installed(dir.path(), MS_REMOTE_SSH));
        assert!(!extension_installed(
            &dir.path().join("missing"),
            MS_REMOTE_SSH
        ));
    }

    #[test]
    fn settings_are_read_as_jsonc() {
        let settings = r#"{
            // my settings
            "editor.fontSize": 13,
            "remote.SSH.configFile": "~/dotfiles/ssh\\config",
            "remote.SSH.path": "/usr/bin/ssh",
        }"#;
        assert_eq!(
            json_string_setting(settings, "remote.SSH.configFile").as_deref(),
            Some("~/dotfiles/ssh\\config")
        );
        assert_eq!(json_string_setting(settings, "remote.SSH.port"), None);
        assert_eq!(json_string_setting("{\"k\": \"\"}", "k"), None);
        assert_eq!(json_string_setting("{\"k\": 3}", "k"), None);
        assert_eq!(json_string_setting("not json", "k"), None);
    }

    #[test]
    fn commented_out_settings_do_not_count() {
        // A commented-out copy before the live one, in both comment styles,
        // plus a line comment holding a quote that must not open a string.
        let settings = r#"{
            // "remote.SSH.configFile": "~/old/config",
            /* "remote.SSH.configFile": "~/older/config", */
            // it's fine
            "remote.SSH.configFile": "~/live/config", // trailing note
            "remote.SSH.configFile": "~/last/config",
            "url": "https://example.com/x", /* a URL keeps its slashes */
        }"#;
        assert_eq!(
            json_string_setting(settings, "remote.SSH.configFile").as_deref(),
            Some("~/last/config"),
            "the last live value wins, as in VS Code"
        );
        assert_eq!(
            json_string_setting(settings, "url").as_deref(),
            Some("https://example.com/x")
        );
        // Only commented out: no setting at all, so the default applies.
        let only_comment = "{\n  // \"remote.SSH.configFile\": \"~/old/config\"\n}";
        assert_eq!(
            json_string_setting(only_comment, "remote.SSH.configFile"),
            None
        );
    }

    #[test]
    fn jsonc_reduction_handles_trailing_commas_and_strings() {
        assert_eq!(jsonc_to_json("[1, 2, ]"), "[1, 2 ]");
        assert_eq!(jsonc_to_json("{\"a\": [1,],}"), "{\"a\": [1]}");
        assert_eq!(jsonc_to_json("{\"a\": \"x, //y\"}"), "{\"a\": \"x, //y\"}");
        assert_eq!(
            jsonc_to_json("{\"a\": \"q\\\"//\"}"),
            "{\"a\": \"q\\\"//\"}"
        );
        assert_eq!(jsonc_to_json("1 /* c */ 2"), "1  2");
        // A comma followed by a value is kept; a line comment leaves its newline.
        assert_eq!(jsonc_to_json("[1,\n// c\n2]"), "[1,\n\n2]");
        let home = Path::new("/Users/me");
        assert_eq!(
            expand_home("~/dotfiles/config", home),
            PathBuf::from("/Users/me/dotfiles/config")
        );
        assert_eq!(
            expand_home("/etc/ssh/config", home),
            PathBuf::from("/etc/ssh/config")
        );
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
