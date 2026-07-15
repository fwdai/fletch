//! Provider-scoped discovery of user-defined slash commands on disk.
//!
//! Mirrors the transcript-reader idiom (see `agent::TranscriptReader`): a plain
//! struct of `fn` pointers, one instance per provider, selected by a `match`.
//! Adding a provider = write its two functions, bundle them into a
//! `CommandDiscovery`, and add one arm to `discovery_for`. The frontend
//! analogue is a `CommandAdapter` in `src/data/slashCommands`.
//!
//! Kept out of `agent.rs` because command discovery is orthogonal to the runner
//! kind (per-turn vs persistent), so — unlike transcript readers — it does not
//! belong on `PerTurnDescriptor`.

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

/// Where a discovered command came from. Project entries shadow user entries
/// with the same name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandScope {
    User,
    Project,
}

/// A slash command found on disk, shipped to the frontend. Always maps to a
/// `passthrough` command (the `/name` text is forwarded to the agent verbatim).
/// Mirrors the TS `DiscoveredCommand` in `api.ts`.
#[derive(Clone, Serialize)]
pub struct DiscoveredCommand {
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    pub scope: CommandScope,
}

/// A directory to scan for a provider's command files, tagged with the scope
/// its entries carry.
pub struct CommandRoot {
    pub dir: PathBuf,
    pub scope: CommandScope,
}

/// How to discover one provider's user-defined slash commands.
pub struct CommandDiscovery {
    /// Directories to scan, lowest precedence first. `project` is the agent's
    /// project root (None before a project is chosen — the new-agent composer).
    pub roots: fn(project: Option<&Path>) -> Vec<CommandRoot>,
    /// Parse one command file into a command. `name` is the derived command
    /// name (path relative to its root, minus `.md`, segments joined by `:`).
    /// Returns None to skip an unreadable/ignored file.
    pub parse: fn(path: &Path, name: &str, scope: CommandScope) -> Option<DiscoveredCommand>,
}

/// Select a provider's discovery, or None when it has no on-disk commands.
fn discovery_for(provider: &str) -> Option<&'static CommandDiscovery> {
    match provider {
        "claude" => Some(&CLAUDE_COMMANDS),
        _ => None,
    }
}

/// Discover a provider's slash commands under the given project root. Returns
/// commands sorted by name; project entries shadow user entries on name clash.
/// Empty (never an error) when the provider has no discovery or the dirs are
/// absent.
pub fn discover(provider: &str, project: Option<&Path>) -> Vec<DiscoveredCommand> {
    let Some(d) = discovery_for(provider) else {
        return Vec::new();
    };
    // name -> command. Roots are declared lowest-precedence first, so scan them
    // highest-first and keep the first entry seen for a name (see `walk`). A
    // higher-precedence (project) command then wins over a lower one (user), and
    // if the total-count cap is hit it drops the *lowest*-precedence overflow
    // rather than skipping a whole higher-precedence root. The BTreeMap also
    // yields the result already sorted by name.
    let mut by_name: BTreeMap<String, DiscoveredCommand> = BTreeMap::new();
    for root in (d.roots)(project).into_iter().rev() {
        walk(&root.dir, &root.dir, root.scope, d.parse, &mut by_name, 0);
    }
    by_name.into_values().collect()
}

/// Total command files read across all roots for one provider. Guards a
/// pathological `.claude/commands` tree from stalling the scan or bloating IPC.
const MAX_COMMANDS: usize = 500;
/// How deep to recurse into command subdirectories (used for `:` namespacing).
const MAX_DEPTH: usize = 8;

fn walk(
    base: &Path,
    dir: &Path,
    scope: CommandScope,
    parse: fn(&Path, &str, CommandScope) -> Option<DiscoveredCommand>,
    out: &mut BTreeMap<String, DiscoveredCommand>,
    depth: usize,
) {
    if depth > MAX_DEPTH || out.len() >= MAX_COMMANDS {
        return;
    }
    // Missing dir / no permission just yields nothing — a project without a
    // `.claude/commands` directory is the common case, not an error.
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= MAX_COMMANDS {
            return;
        }
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            walk(base, &path, scope, parse, out, depth + 1);
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            if let Some(name) = command_name(base, &path) {
                // First-wins: roots are scanned highest-precedence first, so an
                // already-present name means a higher-precedence root claimed it.
                if !out.contains_key(&name) {
                    if let Some(cmd) = parse(&path, &name, scope) {
                        out.insert(name, cmd);
                    }
                }
            }
        }
    }
}

/// Derive a command name from a file: its path relative to the scan root, minus
/// the `.md` extension, with directory separators as `:` (Claude's namespacing,
/// e.g. `frontend/component.md` -> `frontend:component`).
fn command_name(base: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(base).ok()?.with_extension("");
    let parts: Vec<String> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().to_string()),
            _ => None,
        })
        .collect();
    if parts.is_empty() {
        return None;
    }
    Some(parts.join(":"))
}

// ---------------------------------------------------------------------------
// Claude adapter
// ---------------------------------------------------------------------------

static CLAUDE_COMMANDS: CommandDiscovery = CommandDiscovery {
    roots: claude_command_roots,
    parse: claude_parse_command,
};

/// Claude reads custom commands from `~/.claude/commands` (user) and
/// `<project>/.claude/commands` (project); project shadows user.
fn claude_command_roots(project: Option<&Path>) -> Vec<CommandRoot> {
    let mut roots = Vec::new();
    if let Some(home) = dirs::home_dir() {
        roots.push(CommandRoot {
            dir: home.join(".claude").join("commands"),
            scope: CommandScope::User,
        });
    }
    if let Some(project) = project {
        roots.push(CommandRoot {
            dir: project.join(".claude").join("commands"),
            scope: CommandScope::Project,
        });
    }
    roots
}

/// The frontmatter keys a Claude command file may declare. All optional;
/// unknown keys (e.g. `allowed-tools`, `model`) are ignored.
#[derive(Default, serde::Deserialize)]
struct ClaudeFrontmatter {
    description: Option<String>,
    #[serde(rename = "argument-hint")]
    argument_hint: Option<String>,
}

fn claude_parse_command(path: &Path, name: &str, scope: CommandScope) -> Option<DiscoveredCommand> {
    let raw = std::fs::read_to_string(path).ok()?;
    let (frontmatter, body) = split_frontmatter(&raw);
    let fm: ClaudeFrontmatter = frontmatter
        .and_then(|y| serde_yaml::from_str(y).ok())
        .unwrap_or_default();
    let description = fm
        .description
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| first_meaningful_line(body))
        .unwrap_or_else(|| "Custom command".to_string());
    let hint = fm
        .argument_hint
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Some(DiscoveredCommand {
        name: name.to_string(),
        description,
        hint,
        scope,
    })
}

/// Split a leading YAML frontmatter block (a `---` line, the YAML, then a
/// closing `---` line) from the markdown body. Line-ending agnostic (LF or
/// CRLF), so Windows-authored command files keep their metadata. Returns
/// (Some(yaml), body) when a complete block is present, else (None, whole
/// input).
fn split_frontmatter(raw: &str) -> (Option<&str>, &str) {
    // Opening fence: the first line must be exactly `---` (tolerating a CR).
    let Some(nl) = raw.find('\n') else {
        return (None, raw);
    };
    if raw[..nl].trim_end_matches('\r') != "---" {
        return (None, raw);
    }
    let rest = &raw[nl + 1..];
    // Closing fence: the next line that is exactly `---`. serde_yaml tolerates
    // the trailing CR on CRLF yaml lines, so the block is passed through as-is.
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches('\n').trim_end_matches('\r') == "---" {
            return (Some(&rest[..offset]), &rest[offset + line.len()..]);
        }
        offset += line.len();
    }
    // No closing fence — not a real frontmatter block.
    (None, raw)
}

/// The first non-empty, non-heading line of a command body, used as a fallback
/// description when the frontmatter omits one. Truncated for the compact
/// autocomplete row (char-safe, so it never splits a UTF-8 boundary).
fn first_meaningful_line(body: &str) -> Option<String> {
    const MAX: usize = 120;
    body.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            if l.chars().count() > MAX {
                let head: String = l.chars().take(MAX).collect();
                format!("{head}…")
            } else {
                l.to_string()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_frontmatter_and_body() {
        let (fm, body) = split_frontmatter("---\ndescription: Hi\n---\nBody here\n");
        assert_eq!(fm, Some("description: Hi\n"));
        assert_eq!(body, "Body here\n");
    }

    #[test]
    fn no_frontmatter_returns_whole_body() {
        let (fm, body) = split_frontmatter("Just a body\nmore");
        assert_eq!(fm, None);
        assert_eq!(body, "Just a body\nmore");
    }

    #[test]
    fn splits_crlf_frontmatter() {
        let (fm, body) = split_frontmatter("---\r\ndescription: Hi\r\n---\r\nBody\r\n");
        assert_eq!(fm, Some("description: Hi\r\n"));
        assert_eq!(body, "Body\r\n");
    }

    #[test]
    fn parses_crlf_authored_command() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("win.md");
        std::fs::write(
            &file,
            "---\r\ndescription: Windows cmd\r\nargument-hint: <x>\r\n---\r\nbody\r\n",
        )
        .unwrap();
        let cmd = claude_parse_command(&file, "win", CommandScope::User).unwrap();
        assert_eq!(cmd.description, "Windows cmd");
        assert_eq!(cmd.hint.as_deref(), Some("<x>"));
    }

    #[test]
    fn falls_back_to_first_meaningful_line() {
        assert_eq!(
            first_meaningful_line("# Title\n\nDo the thing.\n"),
            Some("Do the thing.".to_string())
        );
    }

    #[test]
    fn command_name_namespaces_subdirs() {
        let base = Path::new("/root/.claude/commands");
        let file = Path::new("/root/.claude/commands/frontend/component.md");
        assert_eq!(
            command_name(base, file),
            Some("frontend:component".to_string())
        );
    }

    #[test]
    fn parses_frontmatter_description_and_hint() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("deploy.md");
        std::fs::write(
            &file,
            "---\ndescription: Ship it\nargument-hint: <env>\nmodel: opus\n---\nrun the deploy\n",
        )
        .unwrap();
        let cmd = claude_parse_command(&file, "deploy", CommandScope::Project).unwrap();
        assert_eq!(cmd.description, "Ship it");
        assert_eq!(cmd.hint.as_deref(), Some("<env>"));
        assert_eq!(cmd.scope, CommandScope::Project);
    }

    #[test]
    fn description_falls_back_to_body_when_frontmatter_absent() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.md");
        std::fs::write(&file, "# Heading\n\nJust do the thing.\n").unwrap();
        let cmd = claude_parse_command(&file, "note", CommandScope::User).unwrap();
        assert_eq!(cmd.description, "Just do the thing.");
        assert_eq!(cmd.hint, None);
    }

    #[test]
    fn walk_discovers_nested_files_and_dedupes_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let commands = dir.path().join("commands");
        std::fs::create_dir_all(commands.join("frontend")).unwrap();
        std::fs::write(commands.join("build.md"), "top-level build").unwrap();
        std::fs::write(commands.join("frontend/x.md"), "nested command").unwrap();
        std::fs::write(commands.join("notes.txt"), "ignored, not markdown").unwrap();

        let mut out: BTreeMap<String, DiscoveredCommand> = BTreeMap::new();
        walk(
            &commands,
            &commands,
            CommandScope::Project,
            claude_parse_command,
            &mut out,
            0,
        );

        let names: Vec<&str> = out.keys().map(String::as_str).collect();
        assert_eq!(names, vec!["build", "frontend:x"]);
    }

    #[test]
    fn project_shadows_user_via_highest_first_scan() {
        // Mirror discover's ordering: roots declared lowest-first ([user,
        // project]) are scanned reversed (project first) with first-wins, so a
        // shared name keeps the project entry and a user-only name survives.
        let user = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            user.path().join("shared.md"),
            "---\ndescription: from user\n---\n",
        )
        .unwrap();
        std::fs::write(user.path().join("useronly.md"), "user only").unwrap();
        std::fs::write(
            project.path().join("shared.md"),
            "---\ndescription: from project\n---\n",
        )
        .unwrap();

        let roots = [
            CommandRoot {
                dir: user.path().to_path_buf(),
                scope: CommandScope::User,
            },
            CommandRoot {
                dir: project.path().to_path_buf(),
                scope: CommandScope::Project,
            },
        ];
        let mut out: BTreeMap<String, DiscoveredCommand> = BTreeMap::new();
        for root in roots.into_iter().rev() {
            walk(
                &root.dir,
                &root.dir,
                root.scope,
                claude_parse_command,
                &mut out,
                0,
            );
        }

        assert_eq!(out["shared"].description, "from project");
        assert_eq!(out["shared"].scope, CommandScope::Project);
        assert!(out.contains_key("useronly"));
    }
}
